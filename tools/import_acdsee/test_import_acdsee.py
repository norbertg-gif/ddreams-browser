import datetime as dt
import contextlib
import importlib.util
import pathlib
import sqlite3
import struct
import tempfile
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("import_acdsee.py")
SPEC = importlib.util.spec_from_file_location("import_acdsee", MODULE_PATH)
import_acdsee = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(import_acdsee)


def dbf(path, fields, records):
    header_len = 32 + 32 * len(fields) + 1
    record_len = 1 + sum(length for _, kind, length in fields)
    header = bytearray(header_len)
    header[0] = 0x31
    struct.pack_into("<IHH", header, 4, len(records), header_len, record_len)
    offset = 1
    descriptors = bytearray()
    for name, kind, length in fields:
        item = bytearray(32)
        item[:len(name)] = name.encode("ascii")
        item[11] = ord(kind)
        if kind == "C" and length > 255:
            item[16] = length & 255
            item[17] = length >> 8
        else:
            item[16] = length
        struct.pack_into("<I", item, 12, offset)
        offset += length
        descriptors.extend(item)
    header[32:32 + len(descriptors)] = descriptors
    header[-1] = 0x0D
    body = bytearray()
    for deleted, values in records:
        row = bytearray(b"*" if deleted else b" ")
        for name, kind, length in fields:
            value = values.get(name)
            if kind == "C":
                raw = (value or "").encode("utf-8")[:length].ljust(length, b" ")
            elif kind == "M":
                raw = struct.pack("<I", value or 0)
            elif kind == "B":
                raw = struct.pack("<d", float(value or 0))
            elif kind == "I":
                raw = struct.pack("<i", int(value or 0))
            elif kind == "7":
                if value is None:
                    raw = bytes(length)
                else:
                    julian = value.date().toordinal() + 1_721_425
                    millis = ((value.hour * 60 + value.minute) * 60 + value.second) * 1000 + value.microsecond // 1000
                    raw = struct.pack("<II", julian, millis)
            else:
                raise AssertionError(kind)
            row.extend(raw)
        body.extend(row)
    path.write_bytes(header + body + b"\x1a")


def fixtures(folder):
    folder.mkdir()
    fpt = bytearray(128)
    struct.pack_into(">I", fpt, 0, 2)
    struct.pack_into(">H", fpt, 6, 64)
    payload = "poznámka".encode("utf-8")
    struct.pack_into(">II", fpt, 64, 1, len(payload))
    fpt[72:72 + len(payload)] = payload
    (folder / "Asset.fpt").write_bytes(fpt)
    when = dt.datetime(2019, 4, 20, 10, 53, 43)
    dbf(folder / "Asset.dbf", [
        ("NAME", "C", 300), ("FOLDER_ID", "B", 8), ("SIZE", "B", 8),
        ("WIDTH", "I", 4), ("HEIGHT", "I", 4), ("BPP", "I", 4),
        ("CAPTION", "M", 4), ("NOTES", "M", 4), ("AUTHOR", "M", 4),
        ("RATING", "I", 4), ("TAGGED", "I", 4), ("LABEL", "M", 4),
        ("FTMODIFIED", "7", 8), ("EXIFDATE", "7", 8), ("ACDDATE", "7", 8),
        ("MDDIRTY", "B", 8), ("ISORPHAN", "I", 4), ("ASSET_ID", "B", 8),
    ], [(False, {"NAME": "žltý.jpg", "FOLDER_ID": 10, "SIZE": 123, "WIDTH": 2, "HEIGHT": 3,
                 "BPP": 24, "CAPTION": 1, "NOTES": 1, "RATING": 4, "TAGGED": 1,
                 "EXIFDATE": when, "ACDDATE": when, "MDDIRTY": 1, "ASSET_ID": 100}),
        (True, {"NAME": "deleted.jpg", "ASSET_ID": 101})])
    dbf(folder / "Folder.dbf", [("NAME", "C", 300), ("FOLD_RT_ID", "B", 8), ("PRNT_ID", "B", 8), ("FOLDER_ID", "B", 8)],
        [(False, {"NAME": "\\", "FOLD_RT_ID": 1, "FOLDER_ID": 1}),
         (False, {"NAME": "2019-poltár", "FOLD_RT_ID": 1, "PRNT_ID": 1, "FOLDER_ID": 10})])
    dbf(folder / "FolderRoot.dbf", [("NAME", "C", 40), ("SYS_PATH", "C", 40), ("FOLD_RT_ID", "B", 8)],
        [(False, {"NAME": "F:", "SYS_PATH": "F:", "FOLD_RT_ID": 1})])
    dbf(folder / "Category.dbf", [("NAME", "C", 255), ("CAT_ROOTID", "B", 8), ("PRNT_ID", "B", 8), ("CAT_ID", "B", 8)],
        [(False, {"NAME": "Príroda", "CAT_ROOTID": 1, "CAT_ID": 5})])
    dbf(folder / "CategoryRoot.dbf", [("NAME", "C", 255), ("CAT_ROOTID", "B", 8)], [(False, {"NAME": "Tags", "CAT_ROOTID": 1})])
    dbf(folder / "Keyword.dbf", [("NAME", "C", 255), ("PRNT_ID", "B", 8), ("KW_ID", "B", 8)], [(False, {"NAME": "les", "KW_ID": 8})])
    dbf(folder / "JoinCategoryAsset.dbf", [("CAT_ID", "B", 8), ("ASSET_ID", "B", 8)],
        [(False, {"CAT_ID": 5, "ASSET_ID": 100}), (False, {"CAT_ID": 99, "ASSET_ID": 100}), (False, {"CAT_ID": 5, "ASSET_ID": 999})])
    dbf(folder / "JoinKeywordAsset.dbf", [("KW_ID", "B", 8), ("ASSET_ID", "B", 8)], [(False, {"KW_ID": 8, "ASSET_ID": 100})])
    dbf(folder / "FileType.dbf", [("NAME", "C", 40), ("EXT_LIST", "C", 200), ("FILE_TP_ID", "B", 8)], [])
    dbf(folder / "Person.dbf", [("NAME", "C", 240), ("PERSON_ID", "B", 8)], [(False, {"NAME": "Jana", "PERSON_ID": 1})])


class ImportTests(unittest.TestCase):
    def test_import_decodes_and_reports_data(self):
        with tempfile.TemporaryDirectory() as temp:
            source = pathlib.Path(temp) / "source"
            fixtures(source)
            out = pathlib.Path(temp) / "catalog.sqlite"
            report = import_acdsee.import_data(source, out)
            with contextlib.closing(sqlite3.connect(out)) as conn:
                self.assertEqual(conn.execute("select name,notes,exif_date from assets").fetchone(),
                                 ("žltý.jpg", "poznámka", "2019-04-20T10:53:43"))
                self.assertEqual(conn.execute("select path from folders where id=10").fetchone()[0], "F:\\2019-poltár")
                self.assertEqual(conn.execute("select count(*) from asset_categories").fetchone()[0], 1)
                self.assertEqual(conn.execute("select value from meta where key='catalog_modified'").fetchone()[0], "0")
                self.assertEqual(conn.execute("select count(*) from assets_fts").fetchone()[0], 1)
                self.assertEqual(conn.execute("select ext_lower,is_listed from assets").fetchone(), ("jpg", 1))
                self.assertEqual(conn.execute("select name,path from folder_roots").fetchone(), ("F:", "F:\\"))
            self.assertIn("Asset: 1 / 1", report)
            self.assertIn("missing asset=1, missing category=1", report)
            self.assertIn("MDDIRTY in folder path containing 2019-poltár", report)

    def test_refuses_modified_catalog_without_replacing_it(self):
        with tempfile.TemporaryDirectory() as temp:
            source = pathlib.Path(temp) / "source"
            fixtures(source)
            out = pathlib.Path(temp) / "catalog.sqlite"
            with contextlib.closing(sqlite3.connect(out)) as conn:
                conn.execute("create table meta(key text primary key,value text)")
                conn.execute("insert into meta values('catalog_modified','1')")
                conn.commit()
            before = out.read_bytes()
            with self.assertRaisesRegex(ValueError, "catalog_modified=1"):
                import_acdsee.import_data(source, out)
            self.assertEqual(out.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
