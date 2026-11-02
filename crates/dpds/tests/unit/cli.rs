//! Argument parsing:
//! `dpds <EAL args...> -- [--config <path>] [--config0 <path>] [--config1 <path>]`
//!
//! `parse_args(args)` takes the full argv including argv[0].
//! - Everything before the FIRST `--` (argv[0] included, the `--` itself
//!   excluded) is `Cli::eal_args`; everything after it is app args.
//! - No `--` means no app args (all defaults).
//! - `Cli` keeps the raw flags: `config`, `config0`, `config1` are `None`
//!   when absent. The pool size is no longer an option (it is derived from
//!   `buffer_capacity` and the descriptor counts, see `setup::required_mbufs`):
//!   `--num-mbufs` is an unknown flag and `DEFAULT_NUM_MBUFS` is gone.
//! - Unknown app arg (`--num-mbufs` included), missing option value, a second
//!   `--`, or a flag given TWICE => `Err(CliError)` whose Display contains
//!   "Usage" and names the offending argument.
//! - `Cli::direction_configs(num_ports)`: the config of direction `d` is
//!   `--config<d>` if given, else `--config`, else `config.json`. With one
//!   port `--config1` is an error naming `--config1`.

use std::path::PathBuf;

use dpds::cli::{parse_args, Cli};

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn app(flags: &[&str]) -> Cli {
    let mut v = vec!["dpds", "--"];
    v.extend_from_slice(flags);
    parse_args(&args(&v)).unwrap()
}

fn paths(p: &[&str]) -> Vec<PathBuf> {
    p.iter().map(PathBuf::from).collect()
}

#[test]
fn defaults_when_no_double_dash_is_present() {
    let cli = parse_args(&args(&["dpds", "-l", "0-2", "--no-huge"])).unwrap();
    assert_eq!(cli.eal_args, args(&["dpds", "-l", "0-2", "--no-huge"]));
    assert_eq!(cli.config, None);
    assert_eq!(cli.config0, None);
    assert_eq!(cli.config1, None);
}

#[test]
fn only_argv0() {
    let cli = parse_args(&args(&["dpds"])).unwrap();
    assert_eq!(cli.eal_args, args(&["dpds"]));
    assert_eq!(cli.config, None);
}

#[test]
fn trailing_double_dash_means_no_app_args() {
    let cli = parse_args(&args(&["dpds", "-l", "0-3", "--"])).unwrap();
    assert_eq!(cli.eal_args, args(&["dpds", "-l", "0-3"]));
    assert_eq!(cli.config, None);
}

#[test]
fn all_app_options_are_parsed_and_eal_args_are_split_off() {
    let cli = parse_args(&args(&[
        "dpds",
        "-l",
        "0-2",
        "-a",
        "0000:65:00.0",
        "--",
        "--config",
        "/etc/dpds.json",
        "--config0",
        "d0.json",
        "--config1",
        "d1.json",
    ]))
    .unwrap();
    assert_eq!(
        cli.eal_args,
        args(&["dpds", "-l", "0-2", "-a", "0000:65:00.0"])
    );
    assert_eq!(cli.config, Some(PathBuf::from("/etc/dpds.json")));
    assert_eq!(cli.config0, Some(PathBuf::from("d0.json")));
    assert_eq!(cli.config1, Some(PathBuf::from("d1.json")));
}

#[test]
fn app_options_are_order_independent() {
    let cli = parse_args(&args(&[
        "dpds",
        "--",
        "--config1",
        "b.json",
        "--config0",
        "a.json",
        "--config",
        "c.json",
    ]))
    .unwrap();
    assert_eq!(cli.eal_args, args(&["dpds"]));
    assert_eq!(cli.config, Some(PathBuf::from("c.json")));
    assert_eq!(cli.config0, Some(PathBuf::from("a.json")));
    assert_eq!(cli.config1, Some(PathBuf::from("b.json")));
}

#[test]
fn each_option_alone_leaves_the_others_unset() {
    let cli = app(&["--config", "y.json"]);
    assert_eq!(cli.config, Some(PathBuf::from("y.json")));
    assert_eq!((cli.config0, cli.config1), (None, None));

    let cli = app(&["--config0", "y.json"]);
    assert_eq!(cli.config0, Some(PathBuf::from("y.json")));
    assert_eq!((cli.config, cli.config1), (None, None));

    let cli = app(&["--config1", "y.json"]);
    assert_eq!(cli.config1, Some(PathBuf::from("y.json")));
    assert_eq!((cli.config, cli.config0), (None, None));
}

#[test]
fn split_happens_at_the_first_double_dash_only() {
    // A second `--` is an app argument, and an unknown one.
    let err = parse_args(&args(&[
        "dpds", "--", "--config", "a", "--", "--config", "b",
    ]))
    .unwrap_err();
    assert!(err.to_string().contains("Usage"), "{err}");
}

#[test]
fn eal_flags_that_look_like_app_flags_stay_with_eal() {
    let cli = parse_args(&args(&[
        "dpds",
        "-o",
        "eal-thing",
        "--config",
        "eal",
        "--config1",
        "eal",
        "--",
    ]))
    .unwrap();
    assert_eq!(
        cli.eal_args,
        args(&[
            "dpds",
            "-o",
            "eal-thing",
            "--config",
            "eal",
            "--config1",
            "eal"
        ])
    );
    assert_eq!(cli.config, None);
    assert_eq!(cli.config1, None);
}

#[test]
fn a_flag_given_twice_is_an_error_naming_the_flag() {
    // Replaces the old "last occurrence wins" rule (now an error).
    for flag in ["--config", "--config0", "--config1"] {
        let value = "a.json";
        let err = parse_args(&args(&["dpds", "--", flag, value, flag, value])).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Usage"), "{flag}: {msg}");
        assert!(msg.contains(flag), "the message names {flag}: {msg}");
    }
    // Different values, and not adjacent.
    assert!(parse_args(&args(&[
        "dpds",
        "--",
        "--config",
        "a",
        "--config0",
        "x",
        "--config",
        "b"
    ]))
    .is_err());
}

#[test]
fn different_config_flags_together_are_not_duplicates() {
    // `--config` and `--config0` are different flags; neither is `--config0` twice.
    assert!(parse_args(&args(&["dpds", "--", "--config", "a", "--config0", "b"])).is_ok());
}

#[test]
fn the_csv_output_option_is_still_gone() {
    let err = parse_args(&args(&["dpds", "--", "-o", "out.csv"])).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Usage"), "{msg}");
    assert!(msg.contains("-o"), "the message names the bad arg: {msg}");
    let usage = msg.lines().find(|l| l.contains("Usage")).unwrap();
    assert!(!usage.contains("-o"), "{usage}");
    assert!(!usage.contains("csv"), "{usage}");
}

#[test]
fn unknown_app_arg_is_an_error_with_usage_listing_every_option() {
    let err = parse_args(&args(&["dpds", "--", "--bogus"])).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Usage"), "{msg}");
    assert!(
        msg.contains("--bogus"),
        "message should name the bad arg: {msg}"
    );
    for flag in ["--config ", "--config0", "--config1"] {
        assert!(msg.contains(flag), "usage lists {flag}: {msg}");
    }
    assert!(
        !msg.contains("num-mbufs"),
        "the removed option is not advertised: {msg}"
    );
}

#[test]
fn near_miss_flags_are_unknown() {
    for bad in [
        "--config2",
        "--config-0",
        "--Config",
        "--num-mbuf",
        "config",
    ] {
        assert!(
            parse_args(&args(&["dpds", "--", bad, "x"])).is_err(),
            "{bad} must be rejected"
        );
    }
}

#[test]
fn stray_positional_app_arg_is_an_error() {
    assert!(parse_args(&args(&["dpds", "--", "config.json"])).is_err());
}

#[test]
fn option_without_value_is_an_error() {
    for opt in ["--config", "--config0", "--config1"] {
        let err = parse_args(&args(&["dpds", "--", opt])).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Usage"), "{opt}: {msg}");
        assert!(msg.contains(opt), "{opt}: {msg}");
    }
}

#[test]
fn num_mbufs_is_an_unknown_flag_in_every_form() {
    // The pool size is derived now; the old option is a usage error whatever
    // follows it (a number, nothing, `=value`, a bad number).
    for form in [
        &["--num-mbufs", "4096"][..],
        &["--num-mbufs"][..],
        &["--num-mbufs=4096"][..],
        &["--num-mbufs", "abc"][..],
        &["--config", "a.json", "--num-mbufs", "16384"][..],
        &["--num-mbufs", "16384", "--config", "a.json"][..],
    ] {
        let mut v = vec!["dpds", "--"];
        v.extend_from_slice(form);
        let err = parse_args(&args(&v)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Usage"), "{form:?}: {msg}");
        assert!(
            msg.contains("--num-mbufs"),
            "{form:?}: names the flag: {msg}"
        );
        assert!(
            msg.contains("unknown"),
            "{form:?}: says it is unknown: {msg}"
        );
    }
}

#[test]
fn cli_error_is_a_std_error() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<dpds::cli::CliError>();
}

// ── per-direction resolution ─────────────────────────────────────────

#[test]
fn without_any_config_flag_every_direction_uses_config_json() {
    let cli = app(&[]);
    assert_eq!(cli.direction_configs(1).unwrap(), paths(&["config.json"]));
    assert_eq!(
        cli.direction_configs(2).unwrap(),
        paths(&["config.json", "config.json"])
    );
}

#[test]
fn config_applies_to_every_direction() {
    let cli = app(&["--config", "all.json"]);
    assert_eq!(cli.direction_configs(1).unwrap(), paths(&["all.json"]));
    assert_eq!(
        cli.direction_configs(2).unwrap(),
        paths(&["all.json", "all.json"])
    );
}

#[test]
fn config_d_overrides_config_for_its_direction_only() {
    let cli = app(&["--config", "all.json", "--config1", "one.json"]);
    assert_eq!(
        cli.direction_configs(2).unwrap(),
        paths(&["all.json", "one.json"])
    );

    let cli = app(&["--config", "all.json", "--config0", "zero.json"]);
    assert_eq!(
        cli.direction_configs(2).unwrap(),
        paths(&["zero.json", "all.json"])
    );
    assert_eq!(cli.direction_configs(1).unwrap(), paths(&["zero.json"]));

    let cli = app(&[
        "--config",
        "all.json",
        "--config0",
        "zero.json",
        "--config1",
        "one.json",
    ]);
    assert_eq!(
        cli.direction_configs(2).unwrap(),
        paths(&["zero.json", "one.json"])
    );
}

#[test]
fn a_missing_direction_config_falls_back_to_config_json_not_to_the_other_direction() {
    let cli = app(&["--config0", "zero.json"]);
    assert_eq!(
        cli.direction_configs(2).unwrap(),
        paths(&["zero.json", "config.json"])
    );
    let cli = app(&["--config1", "one.json"]);
    assert_eq!(
        cli.direction_configs(2).unwrap(),
        paths(&["config.json", "one.json"])
    );
}

#[test]
fn config1_with_one_port_is_an_error_naming_the_flag() {
    for cli in [
        app(&["--config1", "one.json"]),
        app(&[
            "--config",
            "a.json",
            "--config0",
            "b.json",
            "--config1",
            "c.json",
        ]),
    ] {
        let err = cli.direction_configs(1).unwrap_err();
        assert!(err.to_string().contains("--config1"), "{err}");
        // ... but the same flags are fine with two ports.
        assert_eq!(cli.direction_configs(2).unwrap().len(), 2);
    }
}

#[test]
fn config0_with_one_port_is_fine() {
    let cli = app(&["--config0", "zero.json"]);
    assert_eq!(cli.direction_configs(1).unwrap(), paths(&["zero.json"]));
}
