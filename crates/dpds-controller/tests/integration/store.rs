//! State persistence (B1): atomic write, reload, corrupt file.

use std::fs;

use crate::common::*;
use dpds_controller::model::{Settings, StoredState};
use dpds_controller::store::{self, STATE_FILE_NAME};
use serde_json::{json, Value};

fn sample() -> StoredState {
    let mut s = state_with(vec![
        link(1, "a-b", &[NIC0, NIC1], Some(vec![2, 6])),
        link(4, "loop", &["net_null0"], None),
    ]);
    s.settings = Settings {
        main_lcore: Some(0),
        mlx5_devargs: "txq_mpw_en=1".into(),
        eal_extra_args: vec!["--no-huge".into(), "-m".into(), "256".into()],
        poll_interval_ms: 250,
    };
    s
}

#[test]
fn missing_directory_loads_the_default_state() {
    let tmp = tempfile::tempdir().unwrap();
    let s = store::load(&tmp.path().join("does/not/exist")).unwrap();
    assert!(s.links.is_empty());
    assert_eq!(s.settings.mlx5_devargs, "txq_mpw_en=1");
    assert_eq!(s.settings.poll_interval_ms, 500);
    assert_eq!(s.settings.main_lcore, None);
    assert!(s.settings.eal_extra_args.is_empty());
}

#[test]
fn save_creates_the_directory_and_roundtrips() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("nested/state");
    let s = sample();
    store::save(&dir, &s).unwrap();
    let back = store::load(&dir).unwrap();
    assert_eq!(
        serde_json::to_value(&back).unwrap(),
        serde_json::to_value(&s).unwrap()
    );
}

#[test]
fn file_has_the_documented_top_level_shape() {
    let tmp = tempfile::tempdir().unwrap();
    store::save(tmp.path(), &sample()).unwrap();
    let v: Value =
        serde_json::from_slice(&fs::read(tmp.path().join(STATE_FILE_NAME)).unwrap()).unwrap();
    assert_eq!(v["links"].as_array().unwrap().len(), 2);
    assert_eq!(v["links"][0]["name"], "a-b");
    assert_eq!(v["links"][0]["ports"], json!([NIC0, NIC1]));
    assert_eq!(v["links"][0]["workers"], json!([2, 6]));
    for k in [
        "main_lcore",
        "mlx5_devargs",
        "eal_extra_args",
        "poll_interval_ms",
    ] {
        assert!(
            v["settings"].get(k).is_some(),
            "settings.{k} missing in {v}"
        );
    }
}

#[test]
fn save_is_atomic_leaves_no_temp_files_and_overwrites() {
    let tmp = tempfile::tempdir().unwrap();
    store::save(tmp.path(), &sample()).unwrap();
    let mut second = sample();
    second.links.pop();
    store::save(tmp.path(), &second).unwrap();
    let names: Vec<_> = fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec![STATE_FILE_NAME.to_string()],
        "stray files: {names:?}"
    );
    assert_eq!(store::load(tmp.path()).unwrap().links.len(), 1);
}

#[test]
fn corrupt_state_file_is_a_clear_error_not_a_silent_reset() {
    for garbage in ["{not json", "", "[]", "{\"links\": 5}"] {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(STATE_FILE_NAME), garbage).unwrap();
        let err = store::load(tmp.path())
            .err()
            .unwrap_or_else(|| panic!("{garbage:?} must not load"));
        let msg = err.to_string();
        assert!(
            msg.contains(STATE_FILE_NAME),
            "error must name the file: {msg}"
        );
        // the corrupt file is left untouched for the operator
        assert_eq!(
            fs::read_to_string(tmp.path().join(STATE_FILE_NAME)).unwrap(),
            garbage
        );
    }
}
