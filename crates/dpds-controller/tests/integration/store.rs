//! State persistence (B1): atomic write, reload, corrupt file.

use std::fs;

use crate::common::*;
use dpds_controller::model::{Settings, StoredState};
use dpds_controller::store::{self, STATE_FILE_NAME};
use serde_json::{json, Value};

fn sample() -> StoredState {
    let mut s = state_with(vec![
        link(1, "a-to-b", NIC0, NIC1, Some(2)),
        link(4, "loop", "net_null0", "net_null0", None),
    ]);
    s.settings = Settings {
        main_lcore: Some(0),
        eal_extra_args: vec!["--no-huge".into(), "-m".into(), "256".into()],
        // the poll interval stays at its default
        ..Settings::default()
    };
    s
}

#[test]
fn missing_directory_loads_the_default_state() {
    let tmp = tempfile::tempdir().unwrap();
    let s = store::load(&tmp.path().join("does/not/exist")).unwrap();
    assert!(s.links.is_empty());
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
    assert_eq!(v["links"][0]["name"], "a-to-b");
    assert_eq!(v["links"][0]["ingress"], NIC0);
    assert_eq!(v["links"][0]["egress"], NIC1);
    assert_eq!(v["links"][0]["worker"], 2);
    assert!(v["links"][0]["config"].is_object());
    assert_eq!(v["links"][1]["ingress"], "net_null0");
    assert_eq!(v["links"][1]["egress"], "net_null0");
    assert!(absent(&v["links"][1], "worker"));
    for old in ["ports", "configs", "workers"] {
        assert!(v["links"][0].get(old).is_none(), "`{old}` in {v}");
    }
    for k in ["main_lcore", "eal_extra_args", "poll_interval_ms"] {
        assert!(
            v["settings"].get(k).is_some(),
            "settings.{k} missing in {v}"
        );
    }
    // spec v17: the mlx5 devargs are fixed (`txq_mpw_en=1`), not stored
    assert!(
        v["settings"].get("mlx5_devargs").is_none(),
        "settings.mlx5_devargs stored in {v}"
    );
    assert_eq!(
        v["settings"].as_object().unwrap().len(),
        3,
        "unexpected settings keys in {v}"
    );
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

// ---- a state file in the old (paired-link) shape is refused (spec v10) --------------------

/// What a controller of the paired-link model wrote: `ports`, `configs`, `workers`. The format
/// was never released, so there is no migration: the controller refuses to start over it.
fn old_shape_state() -> Value {
    json!({
        "links": [
            {"id": 5, "name": "a-b", "ports": [NIC0, NIC1],
             "configs": [cfg_json(1.0e6, 1.0e5), cfg_json(2.0e6, 0.0)], "workers": [2, 6]},
            {"id": 7, "name": "loop", "ports": ["net_null0"], "configs": [cfg_json(3.0e6, 0.0)]}
        ],
        "settings": {"main_lcore": 1,
                     "eal_extra_args": [], "poll_interval_ms": 250},
        "next_id": 9
    })
}

#[test]
fn a_state_file_with_the_old_link_shape_is_refused_naming_the_file_and_left_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(STATE_FILE_NAME);
    let bytes = serde_json::to_vec_pretty(&old_shape_state()).unwrap();
    fs::write(&path, &bytes).unwrap();
    let host = FakeHost::bare_metal();
    let err = match dpds_controller::api::router(dpds_controller::api::Deps {
        state_dir: tmp.path().to_path_buf(),
        roots: host.roots(),
        supervisor: FakeSupervisor::new(),
        telemetry: FakeTelemetry::new(),
        dpds_bin: "/x".into(),
        runtime_dir: "/y".into(),
        dpds_has_telemetry: false,
        is_root: false,
    }) {
        Ok(_) => {
            panic!("the router must refuse a state file with `ports`/`configs`/`workers` links")
        }
        Err(e) => e.to_string(),
    };
    assert!(
        err.contains(STATE_FILE_NAME),
        "the error must name the state file: {err}"
    );
    // no migration and no rewrite: the operator's file is exactly as it was
    assert_eq!(fs::read(&path).unwrap(), bytes);
    // the typed loader refuses it too, naming the file
    let e = store::load(tmp.path()).expect_err("old shape must not load");
    assert!(e.to_string().contains(STATE_FILE_NAME), "{e}");
}
