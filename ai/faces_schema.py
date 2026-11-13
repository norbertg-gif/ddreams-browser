"""Small, dependency-free schema initializer shared by the AI service and importer."""

from __future__ import annotations

import sqlite3


def ensure_faces_schema(conn: sqlite3.Connection) -> None:
    conn.executescript("""
        PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS persons(
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE COLLATE NOCASE,
            created REAL
        );
        CREATE TABLE IF NOT EXISTS faces(
            id INTEGER PRIMARY KEY,
            asset_id INTEGER NOT NULL,
            x REAL, y REAL, w REAL, h REAL,
            det_score REAL,
            source TEXT NOT NULL CHECK(source IN ('ai','acdsee')),
            ext_id INTEGER,
            person_id INTEGER,
            state TEXT NOT NULL,
            score REAL,
            vector BLOB
        );
        CREATE UNIQUE INDEX IF NOT EXISTS faces_source_ext_id ON faces(source, ext_id);
        CREATE INDEX IF NOT EXISTS faces_asset_id ON faces(asset_id);
        CREATE INDEX IF NOT EXISTS faces_person_id ON faces(person_id);
        CREATE INDEX IF NOT EXISTS faces_state ON faces(state);
        CREATE TABLE IF NOT EXISTS face_thumbs(face_id INTEGER PRIMARY KEY, jpeg BLOB);
        CREATE TABLE IF NOT EXISTS face_scan(
            asset_id INTEGER PRIMARY KEY, size INTEGER, mtime_ns INTEGER, version INTEGER, faces INTEGER
        );
        CREATE TABLE IF NOT EXISTS face_rejected(
            face_id INTEGER NOT NULL, person_id INTEGER NOT NULL, PRIMARY KEY(face_id, person_id)
        );
    """)
    conn.commit()
