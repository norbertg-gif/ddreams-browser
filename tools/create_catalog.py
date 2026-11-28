"""Creates an empty DDreams catalog (for an installation without an ACDSee database).

    C:\\DDreams\\modules\\python\\Scripts\\python.exe tools\\create_catalog.py --out C:\\DDreams\\catalog\\catalog.sqlite

The schema is the same one the ACDSee importer writes; DDreams adds its own tables on first
start. The catalog holds no folders, photos or keywords, only a few sample categories that can
be renamed or deleted in the app. An existing file is never overwritten.
"""
from __future__ import annotations

import argparse
import datetime as dt
import os
import pathlib
import sqlite3
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent / "import_acdsee"))
from import_acdsee import schema  # noqa: E402

# (root name, child category names). Each root has a top-level category of the same name,
# as in catalogs imported from ACDSee.
SAMPLE_CATEGORIES = [("Ľudia", ["Rodina"]), ("Miesta", [])]


def create_catalog(out: pathlib.Path) -> None:
    if out.exists():
        raise SystemExit(f"Refusing to overwrite existing file: {out}")
    out.parent.mkdir(parents=True, exist_ok=True)
    handle = tempfile.NamedTemporaryFile(prefix=out.name + ".", suffix=".tmp", dir=out.parent, delete=False)
    temp = pathlib.Path(handle.name)
    handle.close()
    try:
        conn = sqlite3.connect(temp)
        try:
            schema(conn)
            now = dt.datetime.now().astimezone().isoformat(timespec="seconds")
            conn.executemany("INSERT INTO meta VALUES (?,?)", [
                ("catalog_modified", "0"), ("created_at", now), ("text_encoding", "UTF-8")])
            conn.executemany("INSERT INTO ddreams_meta VALUES (?,?)", [
                ("schema_version", "1"), ("catalog_owner", "ddreams"), ("created_from", "empty"),
                ("import_date", now), ("catalog_id_limits_seeded", "1")])
            category_id = 0
            for root_id, (root_name, children) in enumerate(SAMPLE_CATEGORIES, start=1):
                conn.execute("INSERT INTO category_roots(id,name) VALUES (?,?)", (root_id, root_name))
                category_id += 1
                top_id = category_id
                conn.execute("INSERT INTO categories VALUES (?,?,?,?)", (top_id, None, root_id, root_name))
                for child in children:
                    category_id += 1
                    conn.execute("INSERT INTO categories VALUES (?,?,?,?)", (category_id, top_id, root_id, child))
            conn.commit()
        finally:
            conn.close()
        os.replace(temp, out)
    except BaseException:
        temp.unlink(missing_ok=True)
        raise


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", required=True, type=pathlib.Path)
    args = parser.parse_args(argv)
    create_catalog(args.out)
    print(f"Created empty catalog: {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
