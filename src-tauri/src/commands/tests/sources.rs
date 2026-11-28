use super::sources::{
    CatalogFolderNode, classify_source_with, collect_catalog_missing_sources, require_missing_source,
    tree_children_from_listing,
};
use crate::db;

#[test]
fn missing_and_offline_sources_are_mutually_exclusive_and_volume_is_cached() {
    let mut cache = std::collections::HashMap::new();
    let mut probes = 0;
    let offline = classify_source_with("X:\\photos", &mut cache, |p| {
        probes += 1;
        p != "X:\\"
    });
    assert_eq!(offline, (true, false));
    assert_eq!(probes, 1);
    let second = classify_source_with("X:\\other", &mut cache, |p| {
        probes += 1;
        p != "X:\\"
    });
    assert_eq!(second, (true, false));
    assert_eq!(probes, 1, "volume probe is cached and unreachable paths are not probed");
    let mut cache = std::collections::HashMap::new();
    let missing = classify_source_with("F:\\missing", &mut cache, |p| p == "F:\\");
    assert_eq!(missing, (false, true));
    assert_eq!(
        require_missing_source(false, false).unwrap_err(),
        "Priečinok existuje na disku – z katalógu ho vyraď cez vylúčenie"
    );
}

#[test]
fn settings_sources_are_topmost_and_include_descendant_asset_counts() {
    let nodes = vec![
        CatalogFolderNode {
            id: 1,
            parent: None,
            path: "X:\\archive".into(),
            count: 2,
        },
        CatalogFolderNode {
            id: 2,
            parent: Some(1),
            path: "X:\\archive\\child".into(),
            count: 3,
        },
        CatalogFolderNode {
            id: 3,
            parent: None,
            path: "F:\\missing".into(),
            count: 4,
        },
        CatalogFolderNode {
            id: 4,
            parent: Some(3),
            path: "F:\\missing\\child".into(),
            count: 5,
        },
        CatalogFolderNode {
            id: 5,
            parent: None,
            path: "F:\\live".into(),
            count: 1,
        },
    ];
    let sources = collect_catalog_missing_sources(nodes, |p| p == "F:\\" || p == "F:\\live");
    assert_eq!(sources.len(), 2);
    assert_eq!(
        (
            sources[0].path.as_str(),
            sources[0].assets,
            sources[0].offline,
            sources[0].missing
        ),
        ("X:\\archive", 5, true, false)
    );
    assert_eq!(
        (
            sources[1].path.as_str(),
            sources[1].assets,
            sources[1].offline,
            sources[1].missing
        ),
        ("F:\\missing", 9, false, true)
    );
}

#[test]
fn tree_child_rows_ignore_catalogue_only_folders() {
    let live = db::Folder {
        id: 1,
        parent_id: Some(10),
        root_id: Some(10),
        name: "exists".into(),
        path: Some("F:\\root\\exists".into()),
        count: 4,
        folder_ids: vec![1],
        virtual_node: None,
        offline: false,
        missing: false,
    };
    let catalogue_only = db::Folder {
        id: 2,
        parent_id: Some(10),
        root_id: Some(10),
        name: "removed".into(),
        path: Some("F:\\root\\removed".into()),
        count: 2,
        folder_ids: vec![2],
        virtual_node: None,
        offline: false,
        missing: false,
    };
    let rows = tree_children_from_listing(vec![live], vec![catalogue_only]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "exists");
}
