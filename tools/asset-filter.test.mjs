import assert from "node:assert/strict";
import { test } from "node:test";
import { buildAssetFilter, effectiveViewFilters, readViewSort, sortKeyForView, updateViewSort } from "../src/assetFilter.ts";

test("asset filters preserve file group, dates and the tagged selection override", () => {
  const view = {kind: "date", title: "Jún", dateSource: "modified", dateFrom: "2025-06-01", dateTo: "2025-06-30"};
  const build = (filters, showOtherFiles) =>
    buildAssetFilter(view, "Jún", "name", false, effectiveViewFilters(filters, showOtherFiles), true);
  const base = build({}, false);
  assert.equal(base.fileGroup, "media");
  assert.equal(base.includeChildren, false);
  const tagged = {...base, taggedOnly: true};
  assert.equal(tagged.dateFrom, "2025-06-01");
  assert.equal(tagged.dateTo, "2025-06-30");
  assert.equal(tagged.dateSource, "modified");
  assert.equal(tagged.fileGroup, "media");
  assert.equal(build({fileGroup: "raw"}, true).fileGroup, "raw");
  assert.equal(build({fileGroup: "all"}, false).fileGroup, null);
  assert.equal(build({}, true).fileGroup, null);
});

test("visiting a folder reads its own sort without changing any stored sorting", () => {
  const folderA = sortKeyForView({kind: "folder", title: "A", folderPath: "F:/Fotky/A/"});
  const folderB = sortKeyForView({kind: "folder", title: "B", folderPath: "F:\\Fotky\\B"});
  const saved = Object.freeze({
    [folderA]: Object.freeze({sort: "dateTaken", descending: true, stamp: 10}),
    [folderB]: Object.freeze({sort: "name", descending: false, stamp: 11}),
  });
  assert.equal(folderA, "folder:f:\\fotky\\a");
  assert.equal(readViewSort(saved, folderA).sort, "dateTaken");
  assert.equal(readViewSort(saved, folderB).sort, "name");
  assert.deepEqual(readViewSort(saved, "new-folder"), {sort: "name", descending: false});
  const updated = updateViewSort(saved, folderB, "size", true);
  assert.equal(readViewSort(updated, folderB).sort, "size");
  assert.equal(readViewSort(saved, folderB).sort, "name");
  assert.deepEqual(readViewSort(updated, folderA), readViewSort(saved, folderA));
});
