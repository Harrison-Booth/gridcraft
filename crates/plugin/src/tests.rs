use gridcraft_core::{CellError, Value};
use serde_json::json;

use crate::values::{from_json, to_json};
use crate::{Manifest, parse_reply};

fn manifest(functions: serde_json::Value) -> String {
    json!({"id": "t.plugin", "name": "T", "run": {"command": "true"}, "functions": functions}).to_string()
}

#[test]
fn manifests_are_validated() {
    let m = Manifest::parse(&manifest(json!([{"name": "t.one", "maxArgs": 2}]))).unwrap();
    assert_eq!(m.functions[0].name, "T.ONE");
    assert_eq!((m.functions[0].min_args, m.functions[0].max_args), (0, 2));
    for bad in [
        json!([{"name": "1BAD"}]),
        json!([{"name": "BAD NAME"}]),
        json!([{"name": "T.X"}, {"name": "t.x"}]),
        json!([{"name": "T.X", "minArgs": 3, "maxArgs": 2}]),
        json!([{"name": "T.X", "maxArgs": 300}]),
    ] {
        assert!(Manifest::parse(&manifest(bad.clone())).is_err(), "{bad}");
    }
    assert!(Manifest::parse(r#"{"id": "bad id", "name": "x", "run": {"command": "x"}}"#).is_err());
    assert!(Manifest::parse(r#"{"id": "x", "name": "x", "run": {"command": ""}}"#).is_err());
    assert!(Manifest::parse("not json").is_err());
}

#[test]
fn one_manifest_serves_several_apps() {
    // WordCraft's text menu means nothing here; `uiFor.gridcraft` replaces `ui`.
    let both = json!({
        "id": "t.both", "name": "Both", "run": {"command": "true"}, "apps": ["wordcraft", "GridCraft"],
        "functions": [{"name": "T.TWICE", "minArgs": 1, "maxArgs": 1}],
        "commands": [{"id": "tidy", "label": "Tidy"}],
        "ui": {"menus": [{"menu": "text", "command": "tidy"}]},
        "uiFor": {"gridcraft": {"menus": [{"menu": "row", "command": "tidy"}]}, "wordcraft": {"menus": [{"menu": "picture", "command": "tidy"}]}}
    });
    let m = Manifest::parse(&both.to_string()).unwrap();
    assert_eq!(m.ui.menus.len(), 1);
    assert_eq!(m.ui.menus[0].menu, crate::MenuKind::Row);
    assert!(m.for_this_app());
    let only_word = json!({"id": "t.word", "name": "Word", "run": {"command": "true"}, "apps": ["wordcraft"]});
    assert!(Manifest::parse(&only_word.to_string()).unwrap_err().contains("not GridCraft"));
    // Without `uiFor.gridcraft`, `ui` is this app's: another app's menu names are an error.
    let plain = json!({"id": "t.p", "name": "P", "run": {"command": "true"}, "commands": [{"id": "go", "label": "Go"}], "ui": {"menus": [{"menu": "text", "command": "go"}]}});
    assert!(Manifest::parse(&plain.to_string()).is_err());
}

#[test]
fn ui_declarations_must_name_the_plugins_commands() {
    let with_ui = |ui: serde_json::Value| {
        json!({"id": "t.ui", "name": "T", "run": {"command": "true"},
            "commands": [{"id": "go", "label": "Go"}], "ui": ui})
        .to_string()
    };
    let m = Manifest::parse(&with_ui(json!({
        "ribbon": [{"tab": "Data", "group": "Mine", "buttons": [{"command": "go", "size": "small", "params": {"n": 1}}]}],
        "menus": [{"command": "go"}, {"menu": "sheetTab", "command": "go", "label": "Go here"}]
    })))
    .unwrap();
    assert_eq!(m.ui.ribbon[0].buttons[0].size, crate::ButtonSize::Small);
    assert_eq!(m.ui.menus[0].menu, crate::MenuKind::Cell);
    assert_eq!(m.ui.menus[1].menu, crate::MenuKind::SheetTab);
    assert_eq!(m.command("go").map(|c| c.label.as_str()), Some("Go"));
    for bad in [
        json!({"ribbon": [{"tab": "Data", "group": "G", "buttons": [{"command": "nope"}]}]}),
        json!({"ribbon": [{"tab": "Data", "group": "G", "buttons": []}]}),
        json!({"ribbon": [{"tab": " ", "group": "G", "buttons": [{"command": "go"}]}]}),
        json!({"ribbon": [{"tab": "Data", "group": "G", "buttons": [{"command": "go", "size": "huge"}]}]}),
        json!({"menus": [{"command": "nope"}]}),
        json!({"menus": [{"menu": "statusBar", "command": "go"}]}),
        json!({"menus": [{"command": "go", "label": "x".repeat(65)}]}),
    ] {
        assert!(Manifest::parse(&with_ui(bad.clone())).is_err(), "{bad}");
    }
}

#[test]
fn homepages_must_be_web_links() {
    let with = |url: &str| json!({"id": "t.home", "name": "T", "run": {"command": "true"}, "homepage": url}).to_string();
    assert_eq!(Manifest::parse(&with("https://example.org/doc")).unwrap().homepage, "https://example.org/doc");
    assert!(Manifest::parse(&with("")).is_ok());
    for bad in ["javascript:alert(1)", "file:///etc/passwd", "example.org", "https://a b"] {
        assert!(Manifest::parse(&with(bad)).is_err(), "{bad}");
    }
}

#[test]
fn turned_off_plugins_lose_their_functions_and_commands() {
    let m = Manifest::parse(
        &json!({"id": "t.toggle", "name": "T", "run": {"command": "./never-started"},
            "functions": [{"name": "TOGGLE.F"}], "commands": [{"id": "go", "label": "Go"}]})
        .to_string(),
    )
    .unwrap();
    crate::install(m, std::env::temp_dir()).unwrap();
    assert!(gridcraft_calc::is_known_function("TOGGLE.F"));
    assert_eq!(crate::set_enabled("t.toggle", false), Ok(true));
    assert_eq!(crate::set_enabled("t.toggle", false), Ok(false));
    assert!(!gridcraft_calc::is_known_function("TOGGLE.F"));
    assert_eq!(crate::get("t.toggle").map(|p| p.status()), Some("off".into()));
    assert!(crate::run_command("t.toggle", "go", json!({}), json!({})).unwrap_err().contains("turned off"));
    assert_eq!(crate::set_enabled("t.toggle", true), Ok(true));
    assert!(gridcraft_calc::is_known_function("TOGGLE.F"));
    // Off, its function name is free; another plugin taking it blocks turning it back on.
    crate::set_enabled("t.toggle", false).unwrap();
    let other =
        Manifest::parse(&json!({"id": "t.toggle2", "name": "T2", "run": {"command": "x"}, "functions": [{"name": "TOGGLE.F"}]}).to_string()).unwrap();
    crate::install(other, std::env::temp_dir()).unwrap();
    assert!(crate::set_enabled("t.toggle", true).is_err());
    assert!(crate::unload("t.toggle"));
    assert!(gridcraft_calc::is_known_function("TOGGLE.F"), "unloading the off plugin leaves the other's function");
    assert!(crate::unload("t.toggle2"));
    assert!(crate::set_enabled("t.toggle", true).is_err());
}

#[test]
fn built_in_function_names_are_refused() {
    let m = Manifest::parse(&manifest(json!([{"name": "SUM"}]))).unwrap();
    let e = crate::install(m, std::env::temp_dir()).err().unwrap();
    assert!(e.contains("built-in"), "{e}");
}

#[test]
fn values_round_trip() {
    let arr = from_json(&json!([[1, "a"], [true, {"error": "#N/A"}]]));
    let Value::Array(a) = &arr else { panic!("{arr:?}") };
    assert_eq!((a.rows, a.cols), (2, 2));
    assert_eq!(a.get(1, 1), Some(&Value::Error(CellError::NA)));
    assert_eq!(to_json(&arr), json!([[1.0, "a"], [true, {"error": "#N/A"}]]));
    assert_eq!(from_json(&json!(null)), Value::Empty);
    assert_eq!(from_json(&json!({"error": "#nope"})), Value::Error(CellError::Value));
    assert_eq!(from_json(&json!({"x": 1})), Value::Error(CellError::Value));
    assert_eq!(from_json(&json!([])), Value::Error(CellError::Calc));
    assert_eq!(from_json(&json!([[]])), Value::Error(CellError::Calc));
    assert_eq!(from_json(&json!([[[1]]])), from_json(&json!([[{"error": "#VALUE!"}]])));
    // A flat list is a row; ragged rows are padded with #N/A.
    let Value::Array(row) = from_json(&json!([1, 2, 3])) else { panic!() };
    assert_eq!((row.rows, row.cols), (1, 3));
    let Value::Array(r) = from_json(&json!([[1], [2, 3]])) else { panic!() };
    assert_eq!(r.get(0, 1), Some(&Value::Error(CellError::NA)));
    // Text is capped at Excel's 32,767 characters.
    let Value::Text(t) = from_json(&json!("é".repeat(40_000))) else { panic!() };
    assert_eq!(t.chars().count(), 32_767);
}

#[test]
fn command_replies_are_checked() {
    let r = parse_reply(json!({"commands": [{"command": "cell.set", "params": {"cell": "A1"}}], "message": "hi"})).unwrap();
    assert_eq!(r.commands.len(), 1);
    assert_eq!(r.message.as_deref(), Some("hi"));
    assert!(parse_reply(json!({"commands": {}})).is_err());
    assert!(parse_reply(json!({"commands": [{"params": {}}]})).is_err());
    assert!(parse_reply(json!(null)).unwrap().commands.is_empty());
}

mod install {
    use std::io::Write;

    use crate::install::{Repo, Version, parse_github_url, pick_asset, unpack};

    fn repo(owner: &str, name: &str, version: Version) -> Repo {
        Repo { owner: owner.into(), name: name.into(), version }
    }

    #[test]
    fn github_addresses_are_read() {
        for url in ["https://github.com/acme/fin-tools", "github.com/acme/fin-tools/", "http://www.github.com/acme/fin-tools.git", "acme/fin-tools"] {
            assert_eq!(parse_github_url(url), Ok(repo("acme", "fin-tools", Version::Latest)), "{url}");
        }
        assert_eq!(parse_github_url("https://github.com/acme/fin/releases/tag/v1.2.0"), Ok(repo("acme", "fin", Version::Release("v1.2.0".into()))));
        assert_eq!(parse_github_url("https://github.com/acme/fin/tree/feat/x"), Ok(repo("acme", "fin", Version::Tree("feat/x".into()))));
        assert_eq!(parse_github_url("https://github.com/acme/fin/releases/latest"), Ok(repo("acme", "fin", Version::Latest)));
        for bad in [
            "",
            "https://gitlab.com/acme/fin",
            "https://github.com/acme",
            "https://github.com/../fin",
            "https://github.com/acme/fin/blob/main/x",
            "https://github.com/acme/fin/tree/../../x",
            "ftp://github.com/acme/fin",
            "a/b/c",
        ] {
            assert!(parse_github_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_release_zip_for_this_platform_is_picked() {
        let names: Vec<String> = [
            "fin-1.0-linux-x86_64.zip",
            "fin-1.0-macos-x86_64.zip",
            "fin-1.0-macos-aarch64.zip",
            "fin-1.0-windows-x64.zip",
            "fin-1.0-macos-aarch64.tar.gz",
            "checksums.txt",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(pick_asset(&names, "macos", "aarch64"), Some(2));
        assert_eq!(pick_asset(&names, "macos", "x86_64"), Some(1));
        assert_eq!(pick_asset(&names, "windows", "x86_64"), Some(3));
        assert_eq!(pick_asset(&names, "linux", "aarch64"), None, "no build for it: install the source");
        let universal: Vec<String> = ["fin-darwin-universal.zip", "fin-linux-amd64.zip"].map(String::from).to_vec();
        assert_eq!(pick_asset(&universal, "macos", "aarch64"), Some(0));
        let any: Vec<String> = ["fin-plugin.zip", "fin-linux-arm64.zip"].map(String::from).to_vec();
        assert_eq!(pick_asset(&any, "macos", "aarch64"), Some(0), "a zip naming no platform suits every one");
        assert_eq!(pick_asset(&any, "linux", "aarch64"), Some(1), "this platform's build beats the generic zip");
        assert_eq!(pick_asset(&["darwin-tools-windows.zip".to_string()], "macos", "x86_64"), Some(0));
        assert_eq!(pick_asset(&[], "macos", "aarch64"), None);
    }

    fn zip(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(&mut out);
        for (name, data, mode) in files {
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(*mode);
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
        out.into_inner()
    }

    fn manifest(id: &str) -> Vec<u8> {
        serde_json::json!({"id": id, "name": "Fin", "run": {"command": "./run.sh"}}).to_string().into_bytes()
    }

    #[test]
    fn a_source_zip_unpacks_into_a_folder_named_after_the_plugin() {
        let root = std::env::temp_dir().join(format!("gridcraft-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let m = manifest("acme.fin");
        let bytes = zip(&[
            ("acme-fin-1a2b3c/README.md", b"readme", 0o644),
            ("acme-fin-1a2b3c/plugin.json", &m, 0o644),
            ("acme-fin-1a2b3c/run.sh", b"#!/bin/sh\n", 0o755),
            ("acme-fin-1a2b3c/lib/util.py", b"x = 1\n", 0o644),
            ("acme-fin-1a2b3c/examples/other/plugin.json", b"{}", 0o644),
            ("elsewhere/secret.txt", b"not part of the plugin", 0o644),
        ]);
        let dir = unpack(&bytes, &root).unwrap();
        assert_eq!(dir, root.join("acme.fin"));
        assert!(dir.join("plugin.json").is_file() && dir.join("lib/util.py").is_file() && dir.join("examples/other/plugin.json").is_file());
        assert!(!dir.join("elsewhere").exists() && !root.join("elsewhere").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join("run.sh")).unwrap().permissions().mode() & 0o777, 0o755);
            assert_eq!(std::fs::metadata(dir.join("README.md")).unwrap().permissions().mode() & 0o777, 0o644);
        }
        crate::load(&dir).unwrap();
        assert!(crate::unload("acme.fin"));

        // Installing again replaces the earlier copy, leftovers included.
        std::fs::write(dir.join("stale.txt"), "old").unwrap();
        let again = unpack(&zip(&[("plugin.json", &m, 0o644), ("run.sh", b"new", 0o755)]), &root).unwrap();
        assert_eq!(again, dir);
        assert!(!dir.join("stale.txt").exists() && !dir.join("lib").exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1, "no staging folders left behind");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unsafe_or_foreign_zips_are_refused() {
        let root = std::env::temp_dir().join(format!("gridcraft-unpack-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(unpack(b"not a zip", &root).unwrap_err().contains("not a zip"));
        assert!(unpack(&zip(&[("README.md", b"hi", 0o644)]), &root).unwrap_err().contains("plugin.json"));
        assert!(unpack(&zip(&[("plugin.json", &manifest(".."), 0o644)]), &root).is_err(), "an id can't climb out of the folder");
        assert!(unpack(&zip(&[("plugin.json", b"{", 0o644)]), &root).is_err());
        // Paths escaping the plugin folder are skipped, not written.
        let m = manifest("acme.safe");
        let dir = unpack(&zip(&[("plugin.json", &m, 0o644), ("../escaped.txt", b"x", 0o644), ("/abs.txt", b"x", 0o644)]), &root).unwrap();
        assert!(!root.join("escaped.txt").exists() && !std::env::temp_dir().join("escaped.txt").exists());
        assert!(dir.join("plugin.json").is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
