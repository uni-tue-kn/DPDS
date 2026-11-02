//! Input validation (C3): port names, the EAL argument allowlist and the mlx5
//! devargs allowlist. Everything the controller puts on the `dpds` command line
//! comes from here or from its own code, never from a free-form string.

/// A DPDK device name: a PCI address (returned in lower case) or a vdev that
/// takes no arguments, `net_null<N>` or `net_ring<N>`. `net_pcap` and any
/// `,key=value` devargs are refused: they make EAL create files or open
/// interfaces on behalf of the (root) controller.
pub fn port_name(name: &str) -> Result<String, String> {
    if is_pci(name) {
        return Ok(name.to_ascii_lowercase());
    }
    if is_vdev(name) {
        return Ok(name.to_string());
    }
    Err(format!(
        "port {name:?} is neither a PCI address (0000:65:00.0) nor a vdev (net_null0, net_ring0; no devargs, no net_pcap)"
    ))
}

/// `dddd:bb:dd.f` with hex digits and a function of 0 to 7.
fn is_pci(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 12
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b':',
            10 => *c == b'.',
            11 => (b'0'..=b'7').contains(c),
            _ => c.is_ascii_hexdigit(),
        })
}

/// `net_(null|ring)<n>`, nothing else.
fn is_vdev(s: &str) -> bool {
    ["null", "ring"].iter().any(|k| {
        s.strip_prefix("net_")
            .and_then(|r| r.strip_prefix(k))
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()))
    })
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.len() <= 12 && s.bytes().all(|c| c.is_ascii_digit())
}

/// Allowed `settings.eal_extra_args`: `--no-huge`, `--no-pci`, `-m <n>`, `-n <n>`,
/// `--socket-mem=<n>[,<n>...]` and `--log-level=<spec>`. DPDK takes the value of a
/// long option after `=`, so those are single tokens.
pub fn eal_extra_args(args: &[String]) -> Result<(), String> {
    let allowed = "allowed: --no-huge, --no-pci, -m <MB>, -n <channels>, --socket-mem=<MB>[,<MB>...], --log-level=<spec>";
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--no-huge" | "--no-pci" => {}
            "-m" | "-n" => match it.next() {
                Some(v) if digits(v) => {}
                Some(v) => return Err(format!("eal_extra_args: {a} needs a number, got {v:?}")),
                None => return Err(format!("eal_extra_args: {a} needs a number")),
            },
            _ => {
                let ok = match a.split_once('=') {
                    Some(("--socket-mem", v)) => v.split(',').all(digits),
                    Some(("--log-level", v)) => log_spec(v),
                    _ => false,
                };
                if !ok {
                    return Err(format!("eal_extra_args: {a:?} is not allowed ({allowed})"));
                }
            }
        }
    }
    Ok(())
}

/// `7`, `lib.eal:debug`, `pmd.net.mlx5:8`: no whitespace, no leading `-`.
fn log_spec(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'*' | b':' | b'-'))
}

/// mlx5 devargs the controller lets through; `mprq_*` is never one of them.
fn mlx5_key(k: &str) -> bool {
    matches!(
        k,
        "txq_mpw_en"
            | "txq_inline"
            | "rxq_cqe_comp_en"
            | "rxq_pkt_pad_en"
            | "rx_vec_en"
            | "tx_pp"
            | "tx_skew"
    ) || k
        .strip_prefix("txq_inline_")
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'))
}

/// `key=value[,key=value...]` over the allowlisted keys; empty is fine.
pub fn mlx5_devargs(devargs: &str) -> Result<(), String> {
    if devargs.to_lowercase().contains("mprq") {
        return Err(
            "mlx5_devargs must not contain mprq_en: the controller never enables MPRQ (README, \"What not to do\")".into(),
        );
    }
    if devargs.is_empty() {
        return Ok(());
    }
    for part in devargs.split(',') {
        let ok = part.split_once('=').is_some_and(|(k, v)| {
            mlx5_key(k)
                && !v.is_empty()
                && v.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.'))
        });
        if !ok {
            return Err(format!(
                "mlx5_devargs: {part:?} is not allowed (key=value with a key of txq_mpw_en, txq_inline[_*], rxq_cqe_comp_en, rxq_pkt_pad_en, rx_vec_en, tx_pp, tx_skew)"
            ));
        }
    }
    Ok(())
}
