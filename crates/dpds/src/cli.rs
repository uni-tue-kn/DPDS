//! Command line: `dpds <EAL args...> -- [--config <path>] [--num-mbufs <N>]`

use std::fmt;
use std::path::PathBuf;

use crate::consts::DEFAULT_NUM_MBUFS;

const USAGE: &str = "Usage: dpds <EAL args...> -- [--config <path>] [--num-mbufs <N>]";

/// Parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    /// Everything before the first `--` (argv[0] included), for `Eal::new`.
    pub eal_args: Vec<String>,
    /// Emulator config file (default `config.json`).
    pub config: PathBuf,
    /// Mbufs per port; the pool holds this times the number of ports.
    pub num_mbufs: u32,
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

/// Parses the full argv (including argv[0]). Everything before the first
/// `--` belongs to the EAL, everything after it to the application.
pub fn parse_args(args: &[String]) -> Result<Cli, CliError> {
    let (eal_args, app_args) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &args[args.len()..]),
    };

    let mut cli = Cli {
        eal_args: eal_args.to_vec(),
        config: PathBuf::from("config.json"),
        num_mbufs: DEFAULT_NUM_MBUFS,
    };

    let mut it = app_args.iter();
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| error(format_args!("missing value for {arg}")))
        };
        match arg.as_str() {
            "--config" => cli.config = PathBuf::from(value()?),
            "--num-mbufs" => {
                let v = value()?;
                cli.num_mbufs = v
                    .parse()
                    .map_err(|_| error(format_args!("invalid --num-mbufs value '{v}'")))?;
            }
            other => return Err(error(format_args!("unknown argument '{other}'"))),
        }
    }
    Ok(cli)
}
