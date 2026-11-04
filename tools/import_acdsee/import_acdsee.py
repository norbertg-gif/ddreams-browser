#!/usr/bin/env python3
"""Import a read-only copy of an ACDSee Visual FoxPro database to SQLite."""

from __future__ import annotations

import argparse
import collections
import contextlib
import datetime as dt
import json
import ntpath
import os
import pathlib
import sqlite3
import struct
import tempfile
from typing import Any, Iterator


TABLES = ("Asset", "Category", "CategoryRoot", "Keyword", "JoinCategoryAsset",
          "JoinKeywordAsset", "Folder", "FolderRoot", "FileType", "Person")


class DBF:
    def __init__(self, path: pathlib.Path):
        self.path = path
        self.data = path.read_bytes()
        if len(self.data) < 33:
            raise ValueError(f"{path}: truncated DBF header")
        self.count, self.header_len, self.record_len = struct.unpack_from("<IHH", self.data, 4)
        if self.header_len < 33 or self.record_len < 1:
            raise ValueError(f"{path}: invalid DBF dimensions")
        self.fields: list[tuple[str, str, int, int]] = []
        offset = 32
        while offset + 32 <= self.header_len and self.data[offset] != 0x0D:
            desc = self.data[offset:offset + 32]
            name = desc[:11].split(b"\0", 1)[0].decode("ascii")
            kind = chr(desc[11])
            length = desc[16] + (desc[17] << 8 if kind == "C" else 0)
            if not name or length <= 0:
                raise ValueError(f"{path}: invalid field descriptor at {offset}")
            self.fields.append((name, kind, length, desc[17]))
            offset += 32
        if offset + 1 > self.header_len or sum(f[2] for f in self.fields) + 1 != self.record_len:
            raise ValueError(f"{path}: field lengths do not match record length")
        if self.header_len + self.count * self.record_len > len(self.data):
            raise ValueError(f"{path}: truncated records")

    def records(self, memo: "FPT | None" = None) -> Iterator[tuple[bool, dict[str, Any]]]:
        for index in range(self.count):
            start = self.header_len + index * self.record_len
            row = self.data[start:start + self.record_len]
            deleted = row[0] == 0x2A
            pos = 1
            values: dict[str, Any] = {}
            for name, kind, length, _ in self.fields:
                raw = row[pos:pos + length]
                pos += length
                values[name] = decode_field(kind, raw, memo, self.path, index)
            yield deleted, values


class FPT:
    def __init__(self, path: pathlib.Path):
        self.data = path.read_bytes()
        if len(self.data) < 8:
            raise ValueError(f"{path}: truncated FPT header")
        self.block_size = struct.unpack_from(">H", self.data, 6)[0]
        if not self.block_size:
            raise ValueError(f"{path}: invalid FPT block size")

    def memo(self, pointer: int) -> str | None:
        if not pointer:
            return None
        offset = pointer * self.block_size
        if offset + 8 > len(self.data):
            raise ValueError(f"FPT memo pointer {pointer} is outside file")
        _memo_type, size = struct.unpack_from(">II", self.data, offset)
        if offset + 8 + size > len(self.data):
            raise ValueError(f"FPT memo {pointer} is truncated")
        raw = self.data[offset + 8:offset + 8 + size]
        return raw.decode("utf-8", "replace").rstrip("\0") or None


def decode_field(kind: str, raw: bytes, memo: FPT | None, path: pathlib.Path, row: int) -> Any:
    if kind == "C":
        return raw.rstrip(b" \0").decode("utf-8", "replace")
    if kind == "M":
        if memo is None:
            return None
        return memo.memo(struct.unpack_from("<I", raw)[0])
    if kind == "B":
        return struct.unpack("<d", raw)[0]
    if kind == "I":
        return struct.unpack("<i", raw)[0]
    if kind == "7":
        julian, millis = struct.unpack("<II", raw)
        if not julian or julian == 0xFFFFFFFF or millis >= 86_400_000:
            return None
        try:
            value = dt.datetime(1, 1, 1) + dt.timedelta(days=julian - 1_721_426, milliseconds=millis)
        except (OverflowError, ValueError):
            return None
        return value.isoformat(timespec="milliseconds").rstrip("0").rstrip(".")
    if kind == "L":
        return raw[:1].upper() in (b"T", b"Y")
    if kind == "D":
        text = raw.decode("ascii", "ignore")
        try:
            return dt.date(int(text[:4]), int(text[4:6]), int(text[6:8])).isoformat()
        except (ValueError, IndexError):
            return None
    raise ValueError(f"{path}: unsupported DBF type {kind!r} at record {row + 1}")


def ident(value: Any) -> int | None:
    if value is None:
        return None
    number = int(value)
    return number if number else None


def load_table(source: pathlib.Path, name: str) -> tuple[list[dict[str, Any]], int, int]:
    dbf = DBF(source / f"{name}.dbf")
    memo_path = source / f"{name}.fpt"
    memo = FPT(memo_path) if memo_path.exists() else None
    live: list[dict[str, Any]] = []
    deleted = 0
    for is_deleted, row in dbf.records(memo):
        if is_deleted:
            deleted += 1
        else:
            live.append(row)
    return live, deleted, dbf.count


def resolved_paths(folders: list[dict[str, Any]], roots: list[dict[str, Any]]) -> dict[int, str | None]:
    root_by_id = {}
    for root in roots:
        value = str(root.get("SYS_PATH") or root.get("NAME") or "")
        if len(value) == 2 and value[1] == ":":
            value += "\\"
        root_by_id[ident(root.get("FOLD_RT_ID"))] = value
    by_id = {ident(f.get("FOLDER_ID")): f for f in folders if ident(f.get("FOLDER_ID")) is not None}
    cache: dict[int, str | None] = {}

    def resolve(fid: int, seen: set[int] | None = None) -> str | None:
        if fid in cache:
            return cache[fid]
        seen = seen or set()
        if fid in seen:
            return None
        seen.add(fid)
        folder = by_id.get(fid)
        if folder is None:
            return None
        root_id = ident(folder.get("FOLD_RT_ID"))
        parent_id = ident(folder.get("PRNT_ID"))
        name = str(folder.get("NAME") or "")
        if parent_id:
            parent = resolve(parent_id, seen)
            path = ntpath.join(parent, name) if parent else None
        else:
            root = root_by_id.get(root_id)
            path = root if root else None
            if path and name not in ("", "\\", "/"):
                path = ntpath.join(path, name)
        cache[fid] = path
        return path

    for fid in by_id:
        resolve(fid)
    return cache


def schema(conn: sqlite3.Connection) -> None:
    conn.executescript("""
      PRAGMA foreign_keys=OFF;
      CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
      CREATE TABLE folders(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT NOT NULL,path TEXT);
      CREATE TABLE folder_roots(id INTEGER PRIMARY KEY,name TEXT NOT NULL,path TEXT);
      CREATE TABLE assets(id INTEGER PRIMARY KEY,folder_id INTEGER,name TEXT NOT NULL,ext TEXT,ext_lower TEXT NOT NULL,is_listed INTEGER NOT NULL,size REAL,width INTEGER,height INTEGER,bpp INTEGER,caption TEXT,notes TEXT,author TEXT,rating INTEGER,tagged INTEGER,label TEXT,file_modified TEXT,exif_date TEXT,acdsee_date TEXT,md_dirty REAL,is_orphan INTEGER,path TEXT);
      CREATE TABLE categories(id INTEGER PRIMARY KEY,parent_id INTEGER,root_id INTEGER,name TEXT NOT NULL);
      CREATE TABLE category_roots(id INTEGER PRIMARY KEY,name TEXT NOT NULL);
      CREATE TABLE keywords(id INTEGER PRIMARY KEY,parent_id INTEGER,name TEXT NOT NULL);
      CREATE TABLE asset_categories(asset_id INTEGER,category_id INTEGER,PRIMARY KEY(asset_id,category_id));
      CREATE TABLE asset_keywords(asset_id INTEGER,keyword_id INTEGER,PRIMARY KEY(asset_id,keyword_id));
      CREATE TABLE persons(id INTEGER PRIMARY KEY,name TEXT NOT NULL);
      CREATE INDEX idx_assets_folder_id ON assets(folder_id);
      CREATE INDEX idx_assets_exif_date ON assets(exif_date);
      CREATE INDEX idx_assets_name ON assets(name);
      CREATE INDEX idx_assets_folder_listed_name ON assets(folder_id,is_listed,name COLLATE NOCASE,id);
      CREATE INDEX idx_assets_listed_name ON assets(is_listed,name COLLATE NOCASE,id,size,tagged);
      CREATE INDEX idx_assets_listed_ext_name ON assets(is_listed,ext_lower,name COLLATE NOCASE,id,size);
      CREATE INDEX idx_assets_listed_tagged_name ON assets(is_listed,tagged,name COLLATE NOCASE,id,size);
      CREATE INDEX idx_assets_listed_exif_date_ext ON assets(is_listed,exif_date,ext_lower);
      CREATE INDEX idx_assets_listed_acdsee_date_ext ON assets(is_listed,acdsee_date,ext_lower);
      CREATE INDEX idx_assets_listed_file_modified_ext ON assets(is_listed,file_modified,ext_lower);
      CREATE INDEX idx_folders_parent_id ON folders(parent_id);
      CREATE INDEX idx_folders_root_id ON folders(root_id);
      CREATE INDEX idx_categories_parent_id ON categories(parent_id);
      CREATE INDEX idx_categories_root_id ON categories(root_id);
      CREATE INDEX idx_keywords_parent_id ON keywords(parent_id);
      CREATE INDEX idx_asset_categories_category_id ON asset_categories(category_id);
      CREATE INDEX idx_asset_keywords_keyword_id ON asset_keywords(keyword_id);
      CREATE INDEX idx_asset_categories_category_asset ON asset_categories(category_id,asset_id);
      CREATE INDEX idx_asset_keywords_keyword_asset ON asset_keywords(keyword_id,asset_id);
    """)
    try:
        conn.execute("CREATE VIRTUAL TABLE assets_fts USING fts5(name, caption, notes, content='assets', content_rowid='id')")
        conn.executescript("""
          CREATE TRIGGER assets_ai AFTER INSERT ON assets BEGIN INSERT INTO assets_fts(rowid,name,caption,notes) VALUES(new.id,new.name,new.caption,new.notes); END;
        """)
    except sqlite3.OperationalError:
        pass


def import_data(source: pathlib.Path, out: pathlib.Path) -> str:
    tables: dict[str, list[dict[str, Any]]] = {}
    counts: dict[str, tuple[int, int]] = {}
    for name in TABLES:
        tables[name], deleted, total = load_table(source, name)
        counts[name] = (len(tables[name]), deleted)

    folders, roots = tables["Folder"], tables["FolderRoot"]
    paths = resolved_paths(folders, roots)
    root_paths = {}
    for root in roots:
        root_path = str(root.get("SYS_PATH") or root.get("NAME") or "")
        if len(root_path) == 2 and root_path[1] == ":":
            root_path += "\\"
        root_paths[ident(root.get("FOLD_RT_ID"))] = root_path
    folder_ids = {ident(f.get("FOLDER_ID")) for f in folders}
    assets = tables["Asset"]
    asset_ids = {ident(a.get("ASSET_ID")) for a in assets}
    categories = tables["Category"]
    category_ids = {ident(c.get("CAT_ID")) for c in categories}
    keywords = tables["Keyword"]
    keyword_ids = {ident(k.get("KW_ID")) for k in keywords}
    category_missing_asset = category_missing_category = 0
    keyword_missing_asset = keyword_missing_keyword = 0
    for r in tables["JoinCategoryAsset"]:
        category_missing_asset += ident(r.get("ASSET_ID")) not in asset_ids
        category_missing_category += ident(r.get("CAT_ID")) not in category_ids
    for r in tables["JoinKeywordAsset"]:
        keyword_missing_asset += ident(r.get("ASSET_ID")) not in asset_ids
        keyword_missing_keyword += ident(r.get("KW_ID")) not in keyword_ids

    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists():
        try:
            with contextlib.closing(sqlite3.connect(f"file:{out.resolve().as_posix()}?mode=ro", uri=True)) as old:
                modified = old.execute("SELECT value FROM meta WHERE key='catalog_modified'").fetchone()
            if modified and modified[0] == "1":
                raise ValueError(f"Refusing to overwrite user-modified catalog (catalog_modified=1): {out}")
        except sqlite3.OperationalError:
            pass
    handle = tempfile.NamedTemporaryFile(prefix=out.name + ".", suffix=".tmp", dir=out.parent, delete=False)
    temp = pathlib.Path(handle.name)
    handle.close()
    try:
        conn = sqlite3.connect(temp)
        try:
            schema(conn)
            now = dt.datetime.now().astimezone().isoformat(timespec="seconds")
            meta = {"catalog_modified": "0", "imported_at": now, "source_path": str(source.resolve()),
                    "source_format": "ACDSee Visual FoxPro DBF/FPT", "text_encoding": "UTF-8",
                    "date_type_7": "FoxPro Julian day + milliseconds since midnight",
                    "md_dirty_interpretation": "raw value; meaning not verified"}
            conn.executemany("INSERT INTO meta VALUES (?,?)", meta.items())
            conn.executemany("INSERT INTO folders VALUES (?,?,?,?,?)", [
                (ident(f.get("FOLDER_ID")), ident(f.get("PRNT_ID")), ident(f.get("FOLD_RT_ID")), f.get("NAME") or "", paths.get(ident(f.get("FOLDER_ID")))) for f in folders])
            conn.executemany("INSERT INTO folder_roots VALUES (?,?,?)", [
                (ident(r.get("FOLD_RT_ID")), r.get("NAME") or "", root_paths.get(ident(r.get("FOLD_RT_ID")))) for r in roots])
            conn.executemany("INSERT INTO category_roots VALUES (?,?)", [(ident(r.get("CAT_ROOTID")), r.get("NAME") or "") for r in tables["CategoryRoot"]])
            conn.executemany("INSERT INTO categories VALUES (?,?,?,?)", [(ident(c.get("CAT_ID")), ident(c.get("PRNT_ID")), ident(c.get("CAT_ROOTID")), c.get("NAME") or "") for c in categories])
            conn.executemany("INSERT INTO keywords VALUES (?,?,?)", [(ident(k.get("KW_ID")), ident(k.get("PRNT_ID")), k.get("NAME") or "") for k in keywords])
            conn.executemany("INSERT INTO persons VALUES (?,?)", [(ident(p.get("PERSON_ID")), p.get("NAME") or "") for p in tables["Person"]])
            asset_rows = []
            unresolved = 0
            for a in assets:
                aid = ident(a.get("ASSET_ID")); fid = ident(a.get("FOLDER_ID")); folder_path = paths.get(fid)
                unresolved += fid not in folder_ids or folder_path is None
                name = a.get("NAME") or ""
                fullpath = ntpath.join(folder_path, name) if folder_path else None
                ext = pathlib.PureWindowsPath(name).suffix.lower().lstrip(".")
                asset_rows.append((aid, fid, name, ext, ext, int(bool(ext) and ext != "xmp"), a.get("SIZE"), a.get("WIDTH"), a.get("HEIGHT"), a.get("BPP"),
                    a.get("CAPTION"), a.get("NOTES"), a.get("AUTHOR"), a.get("RATING"), a.get("TAGGED"), a.get("LABEL"),
                    a.get("FTMODIFIED"), a.get("EXIFDATE"), a.get("ACDDATE"), a.get("MDDIRTY"), a.get("ISORPHAN"), fullpath))
            conn.executemany("INSERT INTO assets VALUES (" + ",".join("?" for _ in range(22)) + ")", asset_rows)
            valid_ac = [(ident(r.get("ASSET_ID")), ident(r.get("CAT_ID"))) for r in tables["JoinCategoryAsset"] if ident(r.get("ASSET_ID")) in asset_ids and ident(r.get("CAT_ID")) in category_ids]
            valid_ak = [(ident(r.get("ASSET_ID")), ident(r.get("KW_ID"))) for r in tables["JoinKeywordAsset"] if ident(r.get("ASSET_ID")) in asset_ids and ident(r.get("KW_ID")) in keyword_ids]
            conn.executemany("INSERT OR IGNORE INTO asset_categories VALUES (?,?)", valid_ac)
            conn.executemany("INSERT OR IGNORE INTO asset_keywords VALUES (?,?)", valid_ak)
            conn.commit()
        finally:
            conn.close()
        os.replace(temp, out)
    except Exception:
        temp.unlink(missing_ok=True)
        raise

    md = collections.Counter(str(a.get("MDDIRTY")) for a in assets)
    target_folder_ids = {ident(f.get("FOLDER_ID")) for f in folders if "2019-poltár" in str(paths.get(ident(f.get("FOLDER_ID"))) or "").lower()}
    target_md = collections.Counter(str(a.get("MDDIRTY")) for a in assets if ident(a.get("FOLDER_ID")) in target_folder_ids)
    existing = sum(bool(p and pathlib.Path(p).exists()) for p in paths.values())
    roots_report = [(ident(r.get("FOLD_RT_ID")), r.get("NAME"), root_paths.get(ident(r.get("FOLD_RT_ID")))) for r in roots]
    cat_roots = {ident(r.get("CAT_ROOTID")): r.get("NAME") for r in tables["CategoryRoot"]}
    topcats = [(cat_roots.get(ident(c.get("CAT_ROOTID")), f"root {ident(c.get('CAT_ROOTID'))}"), c.get("NAME")) for c in categories if not ident(c.get("PRNT_ID"))]
    diacritics = [a.get("NAME") for a in assets if any(ch in (a.get("NAME") or "") for ch in "ľščťžáôĽŠČŤŽÁÔ")][:5]
    lines = ["ACDSee import report", f"Source: {source.resolve()}", f"Output: {out.resolve()}", "",
             "Records (live / deleted):"]
    lines.extend(f"  {name}: {counts[name][0]} / {counts[name][1]}" for name in TABLES)
    lines += ["", f"JoinCategoryAsset rows: {len(tables['JoinCategoryAsset'])}; missing asset={category_missing_asset}, missing category={category_missing_category}",
              f"JoinKeywordAsset rows: {len(tables['JoinKeywordAsset'])}; missing asset={keyword_missing_asset}, missing keyword={keyword_missing_keyword}",
              f"Assets with unresolved folder: {unresolved} / {len(assets)}", f"Folders existing on disk now: {existing} / {len(folders)}", "",
             "Folder roots (id | name | base path from SYS_PATH, falling back to NAME for device roots):"]
    lines.extend(f"  {rid} | {name} | {path}" for rid, name, path in roots_report)
    lines += ["", "Sample asset names with diacritics: " + (", ".join(diacritics) if diacritics else "(none found)"),
              "MDDIRTY raw value distribution (meaning unverified): " + json.dumps(dict(md), ensure_ascii=False, sort_keys=True),
              "MDDIRTY in folder path containing 2019-poltár: " + json.dumps(dict(target_md), ensure_ascii=False, sort_keys=True),
              "", "Category roots and top-level categories:"]
    for rid, root_name in cat_roots.items():
        names = [str(name) for root, name in topcats if root == root_name]
        lines.append(f"  {rid} | {root_name}: " + (", ".join(names) if names else "(none)"))
    lines += ["", "Field notes: DBF C strings decoded as UTF-8 (replacement on invalid bytes); B is little-endian IEEE-754 double; I is little-endian signed int32; type 7 decoded as FoxPro Julian-day/millisecond datetime. MDDIRTY is preserved raw; its embed-pending interpretation is not asserted."]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=pathlib.Path)
    parser.add_argument("--out", required=True, type=pathlib.Path)
    args = parser.parse_args()
    if not args.source.is_dir():
        parser.error(f"source directory does not exist: {args.source}")
    try:
        print(import_data(args.source, args.out))
    except Exception as exc:
        parser.exit(1, f"Import failed: {exc}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
