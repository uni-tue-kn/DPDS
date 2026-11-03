//! Command line: `dpds <EAL args...> -- [--config <path>] [--config0 <path>]
//! [--config1 <path>] | --links <path>`

use std::fmt;
use std::path::PathBuf;

const USAGE: &str = "Usage: dpds <EAL args...> -- [--config <path>] [--config0 <path>] \
                     [--config1 <path>] | --links <path>";

/// Parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    /// Everything before the first `--` (argv[0] included), for `Eal::new`.
    pub eal_args: Vec<String>,
    /// `--config`: DPDS config of every shorthand link without its own flag.
    pub config: Option<PathBuf>,
    /// `--config0`: DPDS config of link `dir0` (port 0 -> port 1, or port 0 -> port 0).
    pub config0: Option<PathBuf>,
    /// `--config1`: DPDS config of link `dir1` (port 1 -> port 0, needs two ports).
    pub config1: Option<PathBuf>,
    /// `--links`: the multi-link file (excludes the config flags).
    pub links: Option<PathBuf>,
}

/// A bad application argument; the message includes the usage text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError(String);

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

fn error(problem: impl fmt::Display) -> CliError {
    CliError(format!("{problem}\n{USAGE}"))
}

/// Stores `value` in `slot`, refusing a second occurrence of `flag`.
fn set<T>(slot: &mut Option<T>, flag: &str, value: T) -> Result<(), CliError> {
    if slot.replace(value).is_some() {
        return Err(error(format_args!("{flag} given more than once")));
    }
    Ok(())
}

/// Parses the full argv (including argv[0]). Everything before the first
/// `--` belongs to the EAL, everything after it to the application.
pub fn parse_args(args: &[String]) -> Result<Cli, CliError> {
    let (eal_args, app_args) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &args[args.len()..]),
    };

    let mut cli = Cli {
        eal_args: eal_args.to_vec(),
        config: None,
        config0: None,
        config1: None,
        links: None,
    };

    let mut it = app_args.iter();
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| error(format_args!("missing value for {arg}")))
        };
        match arg.as_str() {
            "--config" => set(&mut cli.config, arg, PathBuf::from(value()?))?,
            "--config0" => set(&mut cli.config0, arg, PathBuf::from(value()?))?,
            "--config1" => set(&mut cli.config1, arg, PathBuf::from(value()?))?,
            "--links" => set(&mut cli.links, arg, PathBuf::from(value()?))?,
            other => return Err(error(format_args!("unknown argument '{other}'"))),
        }
    }
    if cli.links.is_some() {
        for (flag, slot) in [
            ("--config", &cli.config),
            ("--config0", &cli.config0),
            ("--config1", &cli.config1),
        ] {
            if slot.is_some() {
                return Err(error(format_args!(
                    "--links cannot be combined with {flag}"
                )));
            }
        }
    }
    Ok(cli)
}

impl Cli {
    /// The config file of every direction (one per port): `--config<d>`, else
    /// `--config`, else `config.json`. `--config1` is an error with one port.
    pub fn direction_configs(&self, num_ports: usize) -> Result<Vec<PathBuf>, CliError> {
        if num_ports < 2 && self.config1.is_some() {
            return Err(error("--config1 needs two ports, found one"));
        }
        let fallback = self
            .config
            .clone()
            .unwrap_or_else(|| PathBuf::from("config.json"));
        Ok([&self.config0, &self.config1]
            .into_iter()
            .take(num_ports)
            .map(|c| c.clone().unwrap_or_else(|| fallback.clone()))
            .collect())
    }
}
