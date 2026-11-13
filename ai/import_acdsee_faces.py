"""One-off import of ACDSee face annotations into faces.sqlite (no AI imports)."""

from __future__ import annotations

import argparse
import sqlite3
import struct
import time
from collections import Counter
from pathlib import Path
from typing import Any

from faces_schema import ensure_faces_schema


def read_dbf(path: str | Path) -> list[dict[str, Any]]:
    data = Path(path).read_bytes()
    if len(data) < 33:
        raise ValueError(f"DBF hlavička je neúplná: {path}")
    records = struct.unpack_from("<I", data, 4)[0]
    header_len, record_len = struct.unpack_from("<HH", data, 8)
    fields = []
    pos = 32
    while pos < header_len and data[pos] != 0x0D:
        desc = data[pos:pos + 32]
        if len(desc) < 32:
            raise ValueError(f"Neúplný DBF field descriptor: {path}")
        name = desc[:11].split(b"\0", 1)[0].decode("ascii", errors="replace").strip()
        kind, length = chr(desc[11]), desc[16]
        fields.append((name, kind, length))
        pos += 32
    rows = []
    for index in range(records):
        start = header_len + index * record_len
        raw = data[start:start + record_len]
        if len(raw) != record_len:
            break
        if raw[:1] == b"*":
            continue
        offset, row = 1, {}
        for name, kind, length in fields:
            value = raw[offset:offset + length]; offset += length
            if kind == "B":
                row[name] = struct.unpack("<d", value)[0]
            elif kind == "I":
                row[name] = struct.unpack("<i", value)[0]
            elif kind == "C":
                row[name] = value.rstrip(b" \0").decode("utf-8", errors="replace")
            else:
                row[name] = value
        rows.append(row)
    return rows


def import_acdsee_faces(acdsee_dir: str | Path, catalog_path: str | Path, faces_path: str | Path) -> dict[str, Any]:
    root = Path(acdsee_dir)
    faces = read_dbf(root / "Face.dbf")
    people = read_dbf(root / "Person.dbf")
    recognized = read_dbf(root / "RecognizedPerson.dbf")
    catalog = sqlite3.connect(f"{Path(catalog_path).resolve().as_uri()}?mode=ro", uri=True)
    try:
        listed = {int(r[0]) for r in catalog.execute("SELECT id FROM assets WHERE is_listed=1")}
    finally:
        catalog.close()
    db = sqlite3.connect(faces_path)
    ensure_faces_schema(db)
    try:
        person_names = {int(row["PERSON_ID"]): str(row.get("NAME", "")).strip() for row in people if row.get("PERSON_ID") is not None and str(row.get("NAME", "")).strip()}
        person_ids: dict[int, int] = {}
        for ext_person, name in person_names.items():
            existing = db.execute("SELECT id FROM persons WHERE name=? COLLATE NOCASE", (name,)).fetchone()
            if existing:
                person_ids[ext_person] = int(existing[0])
            else:
                cur = db.execute("INSERT INTO persons(name,created) VALUES(?,?)", (name, time.time()))
                person_ids[ext_person] = int(cur.lastrowid)
        rec_by_desc = {}
        for row in recognized:
            try: rec_by_desc[int(row["DESC_ID_R"])] = row
            except (KeyError, TypeError, ValueError): continue
        counts: Counter[str] = Counter()
        skipped = 0
        for row in faces:
            try:
                asset_id, ext_id = int(row["ASSET_ID"]), int(row["FACE_ID"])
                if asset_id not in listed or db.execute("SELECT 1 FROM faces WHERE source='acdsee' AND ext_id=?", (ext_id,)).fetchone():
                    skipped += 1; continue
                desc_id = int(row["DESC_ID"])
                rec = rec_by_desc.get(desc_id)
                person_id, state = None, "unknown"
                if rec is not None:
                    acdsee_person = int(rec["PERSON_ID"])
                    person_id = person_ids.get(acdsee_person)
                    auto = int(rec["ISAUTO"])
                    state = {1: "acdsee", -1: "confirmed", 2: "suggested", 3: "suggested"}.get(auto, "unknown")
                    if person_id is None: state, person_id = "unknown", None
                x, bottom_y, width, height = (float(row[k]) for k in ("DBB_X", "DBB_Y", "DBB_W", "DBB_H"))
                db.execute("INSERT INTO faces(asset_id,x,y,w,h,det_score,source,ext_id,person_id,state,score,vector) VALUES(?,?,?,?,?,NULL,'acdsee',?,?,?,NULL,NULL)",
                           (asset_id, x, 1.0 - bottom_y - height, width, height, ext_id, person_id, state))
                counts[state] += 1
            except (KeyError, TypeError, ValueError, OverflowError, sqlite3.IntegrityError):
                skipped += 1
        db.commit()
        return {"persons": len(person_ids), "faces": dict(counts), "skipped": skipped}
    finally:
        db.close()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--acdsee", required=True, help="Directory containing Face.dbf, Person.dbf, RecognizedPerson.dbf")
    parser.add_argument("--catalog", required=True)
    parser.add_argument("--faces", required=True)
    args = parser.parse_args(argv)
    result = import_acdsee_faces(args.acdsee, args.catalog, args.faces)
    print(f"Osoby: {result['persons']}; tváre podľa stavu: {result['faces']}; preskočené: {result['skipped']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
