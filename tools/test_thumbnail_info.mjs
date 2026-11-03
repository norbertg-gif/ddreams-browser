import test from "node:test";
import assert from "node:assert/strict";
import { DEFAULT_THUMBNAIL_INFO, THUMBNAIL_FIELD_GROUPS, formatKilobytes } from "../src/thumbnailInfo.ts";

test("thumbnail sizes use rounded-up KB and grouped thousands", () => {
  assert.equal(formatKilobytes(0), "0 KB");
  assert.equal(formatKilobytes(1024), "1 KB");
  assert.equal(formatKilobytes(24_772_096), "24 192 KB");
  assert.equal(formatKilobytes(null), "");
});

test("registry default preserves the current one-line name and size badge", () => {
  assert.deepEqual(DEFAULT_THUMBNAIL_INFO.fields, [{ id: "name", lines: 1 }]);
  assert.ok(THUMBNAIL_FIELD_GROUPS.some(group => group.name === "EXIF" && group.fields.some(field => field.id === "iso")));
  assert.ok(THUMBNAIL_FIELD_GROUPS.some(group => group.fields.some(field => field.id === "keywords")));
});
