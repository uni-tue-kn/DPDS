//! Argument parsing: `dpds <EAL args...> -- [--config <path>] [--num-mbufs <N>]`
//! (the `-o <stats.csv>` option is gone together with the CSV export)
//!
//! `parse_args(args)` takes the full argv including argv[0].
//! - Everything before the FIRST `--` (argv[0] included, the `--` itself
//!   excluded) is `Cli::eal_args`; everything after it is app args.
//! - No `--` means no app args (all defaults).
//! - Defaults: `config = "config.json"`,
//!   `num_mbufs = DEFAULT_NUM_MBUFS` (1_048_575, per port).
//! - Unknown app arg, missing option value, non-numeric `--num-mbufs`, or a
//!   second `--` => `Err(CliError)` whose Display contains "Usage".
//! - If an option is given twice, the last occurrence wins.

use std::path::PathBuf;

use dpds::cli::parse_args;
use dpds::consts::DEFAULT_NUM_MBUFS;

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[test]
fn defaults_when_no_double_dash_is_present() {
    let cli = parse_args(&args(&["dpds", "-l", "0-1", "--no-huge"])).unwrap();
    assert_eq!(cli.eal_args, args(&["dpds", "-l", "0-1", "--no-huge"]));
    assert_eq!(cli.config, PathBuf::from("config.json"));
    assert_eq!(cli.num_mbufs, DEFAULT_NUM_MBUFS);
    assert_eq!(cli.num_mbufs, 1_048_575);
}

#[test]
fn only_argv0() {
    let cli = parse_args(&args(&["dpds"])).unwrap();
    assert_eq!(cli.eal_args, args(&["dpds"]));
    assert_eq!(cli.config, PathBuf::from("config.json"));
}

#[test]
fn trailing_double_dash_means_no_app_args() {
    let cli = parse_args(&args(&["dpds", "-l", "0-3", "--"])).unwrap();
    assert_eq!(cli.eal_args, args(&["dpds", "-l", "0-3"]));
    assert_eq!(cli.config, PathBuf::from("config.json"));
    assert_eq!(cli.num_mbufs, DEFAULT_NUM_MBUFS);
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
        "--num-mbufs",
        "4096",
    ]))
    .unwrap();
    assert_eq!(
        cli.eal_args,
        args(&["dpds", "-l", "0-2", "-a", "0000:65:00.0"])
    );
    assert_eq!(cli.config, PathBuf::from("/etc/dpds.json"));
    assert_eq!(cli.num_mbufs, 4096);
}

#[test]
fn app_options_are_order_independent() {
    let cli = parse_args(&args(&[
        "dpds",
        "--",
        "--num-mbufs",
        "77",
        "--config",
        "c.json",
    ]))
    .unwrap();
    assert_eq!(cli.eal_args, args(&["dpds"]));
    assert_eq!(cli.config, PathBuf::from("c.json"));
    assert_eq!(cli.num_mbufs, 77);
}

#[test]
fn each_option_alone_leaves_the_others_at_their_defaults() {
    let cli = parse_args(&args(&["dpds", "--", "--config", "y.json"])).unwrap();
    assert_eq!(cli.config, PathBuf::from("y.json"));

    let cli = parse_args(&args(&["dpds", "--", "--num-mbufs", "10"])).unwrap();
    assert_eq!(cli.num_mbufs, 10);
    assert_eq!(cli.config, PathBuf::from("config.json"));
}

#[test]
fn split_happens_at_the_first_double_dash_only() {
    // A second `--` is an app argument, and an unknown one.
    let err = parse_args(&args(&[
        "dpds",
        "--",
        "--num-mbufs",
        "5",
        "--",
        "--config",
        "b",
    ]))
    .unwrap_err();
    assert!(err.to_string().contains("Usage"), "{err}");
}

#[test]
fn eal_flags_that_look_like_app_flags_stay_with_eal() {
    // `--config` before the `--` is an EAL arg, not ours.
    let cli = parse_args(&args(&["dpds", "-o", "eal-thing", "--config", "eal", "--"])).unwrap();
    assert_eq!(
        cli.eal_args,
        args(&["dpds", "-o", "eal-thing", "--config", "eal"])
    );
    assert_eq!(cli.config, PathBuf::from("config.json"));
}

#[test]
fn last_occurrence_of_an_option_wins() {
    let cli = parse_args(&args(&[
        "dpds", "--", "--config", "a.json", "--config", "b.json",
    ]))
    .unwrap();
    assert_eq!(cli.config, PathBuf::from("b.json"));
}

#[test]
fn the_csv_output_option_is_gone() {
    // `-o <stats.csv>` was removed with the CSV export: now an unknown arg.
    let err = parse_args(&args(&["dpds", "--", "-o", "out.csv"])).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Usage"), "{msg}");
    assert!(msg.contains("-o"), "the message names the bad arg: {msg}");
    // ... and the usage text no longer advertises it or the CSV.
    let usage = msg.lines().find(|l| l.contains("Usage")).unwrap();
    assert!(!usage.contains("-o"), "{usage}");
    assert!(!usage.contains("csv"), "{usage}");
}

#[test]
fn unknown_app_arg_is_an_error_with_usage() {
    let err = parse_args(&args(&["dpds", "--", "--bogus"])).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Usage"), "{msg}");
    assert!(
        msg.contains("--bogus"),
        "message should name the bad arg: {msg}"
    );
    // The usage text lists every supported option.
    assert!(msg.contains("--config"), "{msg}");
    assert!(msg.contains("--num-mbufs"), "{msg}");
}

#[test]
fn stray_positional_app_arg_is_an_error() {
    assert!(parse_args(&args(&["dpds", "--", "config.json"])).is_err());
}

#[test]
fn option_without_value_is_an_error() {
    for opt in ["--config", "--num-mbufs"] {
        let err = parse_args(&args(&["dpds", "--", opt])).unwrap_err();
        assert!(err.to_string().contains("Usage"), "{opt}: {err}");
    }
}

#[test]
fn non_numeric_or_negative_num_mbufs_is_an_error() {
    for bad in ["abc", "-5", "1.5", ""] {
        assert!(
            parse_args(&args(&["dpds", "--", "--num-mbufs", bad])).is_err(),
            "{bad:?} must be rejected"
        );
    }
}

#[test]
fn cli_error_is_a_std_error() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<dpds::cli::CliError>();
}
