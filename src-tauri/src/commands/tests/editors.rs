use super::Editor;
use super::files::{allowed_editor, newest_photoshop, read_editor_settings, write_editor_settings};
use serde_json::Value;

#[test]
fn editor_settings_round_trip() {
    let dir = std::env::temp_dir().join(format!(
        "ddreams-editors-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, r#"{"other":42}"#).unwrap();
    let editors = vec![Editor {
        name: "Test".into(),
        path: "x.exe".into(),
        multi: true,
    }];
    write_editor_settings(&path, &editors).unwrap();
    assert_eq!(read_editor_settings(&path).unwrap(), editors);
    let json: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(json["other"], 42);
    write_editor_settings(&path, &[]).unwrap();
    assert!(read_editor_settings(&path).unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn editor_lookup_accepts_saved_and_detected_paths_and_rejects_others() {
    let saved = Editor {
        name: "Test".into(),
        path: r"C:\Editory\Úpravy.exe".into(),
        multi: false,
    };
    let photoshop = Editor {
        name: "Adobe Photoshop".into(),
        path: r"C:\Adobe\Photoshop.exe".into(),
        multi: true,
    };
    let detect = || Some(photoshop.clone());
    assert_eq!(
        allowed_editor(std::slice::from_ref(&saved), r"c:\editory\úpravy.EXE", || panic!(
            "saved editor should skip detection"
        ))
        .unwrap(),
        saved
    );
    assert_eq!(
        allowed_editor(&[], r"c:\adobe\photoshop.EXE", detect).unwrap(),
        photoshop
    );
    assert_eq!(
        allowed_editor(std::slice::from_ref(&saved), r"c:\adobe\photoshop.EXE", detect).unwrap(),
        photoshop
    );
    assert!(allowed_editor(&[saved], r"C:\other.exe", detect).is_err());
}

#[test]
fn photoshop_fallback_selects_newest_installed() {
    let dir = std::env::temp_dir().join(format!(
        "ddreams-photoshop-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let old = dir.join("Adobe Photoshop 2024/Photoshop.exe");
    let new = dir.join("Adobe Photoshop 2026/Photoshop.exe");
    let missing = dir.join("Adobe Photoshop 2027/Photoshop.exe");
    for p in [&old, &new] {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "").unwrap();
    }
    assert_eq!(newest_photoshop([old, new.clone(), missing]), Some(new));
    std::fs::remove_dir_all(dir).unwrap();
}
