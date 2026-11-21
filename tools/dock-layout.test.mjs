import assert from "node:assert/strict";
import { test } from "node:test";
import {
  defaultLayout, ensureSections, hideSection, knownSections, moveDockItem, normalizeDockLayout,
  patchGroup, propertySection, renamePreset, savePreset, sectionCollapsed, showSection, switchPreset,
} from "../src/dockLayout.ts";

const section = id => ({ kind: "section", id });
const tab = id => ({ kind: "tab", id });

function assertUniqueSections(layout) {
  const ids = knownSections(layout);
  assert.equal(new Set(ids).size, ids.length);
  for (const group of [...layout.left, ...layout.right]) assert.ok(group.panels.includes(group.active));
}

test("moves a whole section before or after another section within or between panes", () => {
  let layout = defaultLayout();
  layout = moveDockItem(layout, section("catalog.colors"),
    { kind: "stack", tab: "catalog", section: "catalog.edit" });
  assert.equal(layout.sections.catalog[0], "catalog.colors");
  layout = moveDockItem(layout, section("catalog.colors"),
    { kind: "stack", tab: "ai", section: "ai.incoming", after: true });
  assert.deepEqual(layout.sections.ai.slice(0, 2), ["ai.incoming", "catalog.colors"]);
  assert.ok(!layout.sections.catalog.includes("catalog.colors"));
  assertUniqueSections(layout);
});

test("creates new columns at edges and gaps and removes columns emptied by a move", () => {
  let layout = defaultLayout();
  layout = moveDockItem(layout, section("catalog.ratings"),
    { kind: "column", side: "right", before: "g-catalog" });
  const column = layout.right[1];
  assert.deepEqual(layout.sections[column.active], ["catalog.ratings"]);
  assert.equal(column.width, 280);
  assert.equal(layout.right[2].id, "g-catalog");
  layout = moveDockItem(layout, section("catalog.ratings"),
    { kind: "column", side: "left", before: "g-nav" });
  assert.equal(layout.left[0].active, column.active);
  assert.ok(!layout.right.some(group => group.id === column.id));
  layout = moveDockItem(layout, section("catalog.ratings"), { kind: "stack", tab: "catalog" });
  assert.equal(layout.left.length, 1);
  assert.equal(layout.sections.catalog.at(-1), "catalog.ratings");
  assert.equal(layout.sections[column.active], undefined);
  assertUniqueSections(layout);
});

test("adds a section as its own tab and moves an existing pane with its sections", () => {
  let layout = moveDockItem(defaultLayout(), section("ai.aiSuggestions"),
    { kind: "tab", group: "g-nav", before: "calendar" });
  const standalone = layout.left[0].active;
  assert.deepEqual(layout.left[0].panels, ["folders", standalone, "calendar"]);
  assert.deepEqual(layout.sections[standalone], ["ai.aiSuggestions"]);
  const items = layout.sections.catalog;
  layout = moveDockItem(layout, tab("catalog"), { kind: "tab", group: "g-nav" });
  assert.ok(!layout.right.some(group => group.id === "g-catalog"));
  assert.equal(layout.left[0].active, "catalog");
  assert.deepEqual(layout.sections.catalog, items);
  assertUniqueSections(layout);
});

test("hides and restores sections and removes their empty parent panes", () => {
  let layout = defaultLayout();
  for (const id of layout.sections.catalog) layout = hideSection(layout, id);
  assert.equal(layout.sections.catalog, undefined);
  assert.ok(!layout.right.some(group => group.id === "g-catalog"));
  assert.ok(layout.hidden.includes("catalog.people"));
  layout = showSection(layout, "catalog.people");
  assert.equal(layout.right.at(-1).active, "catalog");
  assert.deepEqual(layout.sections.catalog, ["catalog.people"]);
  assert.ok(!layout.hidden.includes("catalog.people"));
  assertUniqueSections(layout);
});

test("saved layouts switch columns, widths, tabs, hidden sections and collapse state without mutating the preset", () => {
  let layout = hideSection(defaultLayout(), "catalog.people");
  layout = moveDockItem(layout, section("catalog.labels"), { kind: "tab", group: "g-nav" });
  layout = patchGroup(layout, "g-nav", { width: 440, collapsed: true });
  layout = { ...layout, collapsed: { "catalog.ratings": true, "catalog.edit": false, "catalog.autoGroup:camera": false } };
  let presets = savePreset([], " Výber ", layout);
  assert.equal(savePreset(presets, "Výber", defaultLayout()), presets);
  presets = renamePreset(presets, "Výber", "Triedenie");
  assert.equal(presets[0].name, "Triedenie");
  const dynamic = propertySection("EXIF · Nikon");
  const current = ensureSections(defaultLayout(), [dynamic]);
  const switched = switchPreset(current, presets[0]);
  assert.deepEqual(switched.left[0], layout.left[0]);
  assert.deepEqual(switched.hidden, ["catalog.people"]);
  assert.equal(sectionCollapsed(switched, "catalog.ratings"), true);
  assert.equal(sectionCollapsed(switched, "catalog.edit"), false);
  assert.equal(sectionCollapsed(switched, "catalog.autoGroup:camera"), false);
  assert.ok(switched.sections.properties.includes(dynamic));
  assert.ok(!presets[0].layout.sections.properties.includes(dynamic));
  assertUniqueSections(switched);
});

test("migrates the exact old arrangement and respects hidden and detached sections on reload", () => {
  const legacy = {
    left: [{ id: "left-custom", panels: ["catalog", "folders"], active: "catalog", width: 501, collapsed: true }],
    right: [{ id: "right-custom", panels: ["ai", "calendar", "properties"], active: "ai", width: 203, collapsed: false }],
  };
  let layout = normalizeDockLayout(legacy, { "ai.aiClusters": true });
  assert.deepEqual(layout.left, legacy.left);
  assert.deepEqual(layout.right, legacy.right);
  assert.deepEqual(layout.sections.catalog, defaultLayout().sections.catalog);
  assert.equal(sectionCollapsed(layout, "ai.aiClusters"), true);
  layout = hideSection(layout, "catalog.special");
  layout = moveDockItem(layout, section("catalog.unknownFaces"), { kind: "tab", group: "right-custom" });
  const reloaded = normalizeDockLayout(JSON.parse(JSON.stringify(layout)));
  assert.deepEqual(reloaded.sections[layout.right[0].active], ["catalog.unknownFaces"]);
  assert.ok(reloaded.hidden.includes("catalog.special"));
  assert.ok(!reloaded.sections.catalog.includes("catalog.unknownFaces"));
  assertUniqueSections(reloaded);
});
