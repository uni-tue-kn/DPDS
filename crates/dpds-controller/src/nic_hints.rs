//! The per-NIC hints (spec 33), one set for every PCI NIC of the posted links: Ethernet flow
//! control, PCIe MaxReadReq, the mlx5 port buffer and the mlx5 firmware settings. Each one reads
//! its state with a read-only tool query or a file and, when it is not tuned, lists the root
//! commands that would tune it. The commands are display strings; DPDS never runs them.
//!
//! The only tools that run are `ethtool --json -a <netdev>` (or the text form of an old ethtool),
//! `dcb -j buffer show dev <netdev>` and `mstconfig|mlxconfig -d <bdf> -e -j <file> q`, each with
//! a timeout, and none while an emulation is starting, running or stopping. A missing tool is a
//! hint naming its package, never a failure.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::discovery::Interface;
use crate::hints::{escape_control, hint, nic_label, Ctx, Hint, Level};

/// The PCIe maximum, the target of MaxReadReq.
const MAX_READ_REQ_TARGET: u32 = 4096;
/// preof's firmware values (no tool reports a device maximum: the firmware clamps silently).
const FW_MAX_ACC_OUT_READ: u64 = 256;
const FW_CQE_COMPRESSION: u64 = 1;
/// How long one tool call may take before it is killed.
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
/// What all the tool calls of one request may take together.
const TOOL_BUDGET: Duration = Duration::from_secs(60);

/// Why a tool gave no state.
enum Gap {
    /// The first real call failed with "not found".
    Missing,
    /// An emulation is starting, running or stopping, so no tool ran.
    NotChecked,
    /// The request's time budget for tools was used up before this call.
    BudgetUsed,
    /// The tool ran past its timeout and was killed.
    TimedOut(String),
    /// The tool ran but its answer cannot be used.
    Failed(String),
}

type Tool<T> = Result<T, Gap>;

/// The time left of the request's tool budget, `Err` when none is left or no tool may run.
fn time_left(ctx: &Ctx) -> Tool<Duration> {
    if !ctx.run_tools {
        return Err(Gap::NotChecked);
    }
    let spent = ctx.roots.exec.now().saturating_duration_since(ctx.started);
    match TOOL_BUDGET.saturating_sub(spent) {
        left if left.is_zero() => Err(Gap::BudgetUsed),
        left => Ok(left),
    }
}

/// Runs a read-only query and returns its stdout.
fn run(ctx: &Ctx, argv: &[&str]) -> Tool<String> {
    let timeout = time_left(ctx)?.min(TOOL_TIMEOUT);
    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    match ctx.roots.exec.run(&argv, timeout) {
        Err(e) if e.kind() == ErrorKind::NotFound => Err(Gap::Missing),
        Err(e) if e.kind() == ErrorKind::TimedOut => Err(Gap::TimedOut(format!(
            "{} timed out after {} s and was stopped",
            argv.join(" "),
            timeout.as_secs()
        ))),
        Err(e) => Err(Gap::Failed(format!("{} could not run: {e}", argv[0]))),
        Ok(out) if out.code != 0 => Err(Gap::Failed(format!(
            "{} exited with {}: {}",
            argv.join(" "),
            out.code,
            out.stderr.trim()
        ))),
        Ok(out) => Ok(out.stdout),
    }
}

/// The netdev of a NIC as far as a tool or a shell command may be given it: only letters, digits
/// and `_ . @ -`, and not starting with `-` (it would be read as an option).
fn tool_netdev(nic: &Interface) -> Option<&str> {
    nic.netdev.as_deref().filter(|n| {
        !n.is_empty()
            && !n.starts_with('-')
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | '-'))
    })
}

/// Why a NIC has a netdev that no tool or command may use, when it has one.
fn unsafe_netdev(nic: &Interface) -> Option<String> {
    let name = nic.netdev.as_deref()?;
    tool_netdev(nic).is_none().then(|| {
        format!(
            "the interface name {} is not safe to put into a shell command",
            escape_control(name)
        )
    })
}

/// Why a NIC gets no command: the host uses it, or DPDS cannot use it.
pub(crate) fn blocked(nic: &Interface) -> Option<&str> {
    if nic.uplink || !nic.dpds_usable {
        Some(nic.reason.as_deref().unwrap_or("the host uses this NIC"))
    } else {
        None
    }
}

/// The hints of every used PCI NIC, NIC by NIC.
pub(crate) fn nic_hints(ctx: &Ctx) -> Vec<Hint> {
    let mut hints = Vec::new();
    for nic in ctx.used_nics() {
        let label = &nic_label(nic);
        let pause = tool_netdev(nic).map(|nd| read_pause(ctx, nd));
        let mut set = vec![
            flow_control(nic, label, pause.as_ref()),
            maxreadreq(ctx, nic, label),
        ];
        let mlx5 = nic.vendor_id.as_deref() == Some("0x15b3")
            && nic.driver.as_deref() == Some("mlx5_core");
        if mlx5 {
            set.push(port_buffer(ctx, nic, label, pause.as_ref()));
            set.push(firmware(ctx, nic, label));
        }
        if let Some(why) = blocked(nic) {
            for h in &mut set {
                // only the tuning commands go; the install command of a missing tool stays
                let had = h.commands.len();
                h.commands.retain(|c| c.starts_with("apt install "));
                if h.commands.len() < had {
                    h.detail
                        .push_str(&format!("; no command is suggested for this NIC: {why}"));
                }
            }
        }
        hints.extend(set);
    }
    hints
}

/// The info hint for a state that could not be read, with the install command when the tool is
/// missing.
fn gap_hint(
    id: &str,
    title: &str,
    label: &str,
    what: &str,
    tool: &str,
    package: &str,
    gap: &Gap,
) -> Hint {
    match gap {
        Gap::Missing => hint(
            id,
            Level::Info,
            title,
            format!("{tool} is not installed, so the {what} of {label} cannot be checked"),
            &format!(
                "Install the {} package, then check again.",
                if tool == "dcb" {
                    "iproute2 (5.11 or later provides dcb)"
                } else {
                    package
                }
            ),
        )
        .with_commands(vec![format!("apt install {package}")]),
        Gap::NotChecked => hint(
            id,
            Level::Info,
            title,
            format!(
                "the {what} of {label} is not checked while an emulation is running (reading it runs {tool})"
            ),
            "Stop the emulation to check it.",
        ),
        Gap::BudgetUsed => hint(
            id,
            Level::Info,
            title,
            format!(
                "the {what} of {label} is not checked (time budget of {} s used up)",
                TOOL_BUDGET.as_secs()
            ),
            "Check again.",
        ),
        Gap::TimedOut(why) | Gap::Failed(why) => hint(
            id,
            Level::Info,
            title,
            format!("the {what} of {label} cannot be read: {why}"),
            "",
        ),
    }
}

/// What a firmware reset needs on a virtual machine.
fn hypervisor_note(ctx: &Ctx) -> &'static str {
    if ctx.roots.in_vm() {
        "; this host is a virtual machine: reset the firmware from the hypervisor host"
    } else {
        ""
    }
}

// ---- Ethernet flow control --------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Pause {
    autoneg: bool,
    rx: bool,
    tx: bool,
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// `ethtool --json -a <netdev>`, or the text output of an ethtool without `--json`.
fn read_pause(ctx: &Ctx, netdev: &str) -> Tool<Pause> {
    let json = run(ctx, &["ethtool", "--json", "-a", netdev]).and_then(|out| {
        pause_from_json(&out)
            .ok_or_else(|| Gap::Failed("ethtool --json -a printed no pause settings".into()))
    });
    if !matches!(json, Err(Gap::Failed(_))) {
        return json;
    }
    run(ctx, &["ethtool", "-a", netdev]).and_then(|out| {
        pause_from_text(&out)
            .ok_or_else(|| Gap::Failed("ethtool -a printed no pause settings".into()))
    })
}

fn pause_from_json(out: &str) -> Option<Pause> {
    let v: Value = serde_json::from_str(out).ok()?;
    let e = v.as_array()?.first()?;
    let flag = |k: &str| match e.get(k)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => Some(s == "on"),
        _ => None,
    };
    Some(Pause {
        autoneg: flag("autonegotiate")?,
        rx: flag("rx")?,
        tx: flag("tx")?,
    })
}

fn pause_from_text(out: &str) -> Option<Pause> {
    let value = |key: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .map(|v| v.trim() == "on")
    };
    Some(Pause {
        autoneg: value("Autonegotiate")?,
        rx: value("RX")?,
        tx: value("TX")?,
    })
}

fn flow_control(nic: &Interface, label: &str, pause: Option<&Tool<Pause>>) -> Hint {
    let id = format!("flow_control:{}", nic.pci);
    let title = format!("Flow control {label}");
    let pause = match pause {
        None => {
            let why = if let Some(why) = unsafe_netdev(nic) {
                format!("{why}, so no tool is run for it")
            } else {
                format!(
                    "{} has no network device (a userspace driver is bound)",
                    nic.pci
                )
            };
            return hint(
                &id,
                Level::Info,
                &title,
                format!("{why}, so its flow control cannot be read"),
                "",
            );
        }
        Some(Err(gap)) => {
            return gap_hint(
                &id,
                &title,
                label,
                "flow control",
                "ethtool",
                "ethtool",
                gap,
            )
        }
        Some(Ok(p)) => *p,
    };
    if !pause.rx && !pause.tx {
        return hint(
            &id,
            Level::Ok,
            &title,
            format!("flow control of {label} is off (rx off, tx off)"),
            "",
        );
    }
    let mut command = format!("ethtool -A {label} rx off tx off");
    if pause.autoneg {
        // mlx5 refuses to change the pause settings while autonegotiation is on
        command.push_str(" autoneg off");
    }
    hint(
        &id,
        Level::Info,
        &title,
        format!(
            "pause frames of {label}: rx {}, tx {}, autonegotiation {}; the target is rx off, tx off",
            on_off(pause.rx),
            on_off(pause.tx),
            on_off(pause.autoneg)
        ),
        "Turn flow control off with the command shown.",
    )
    .with_commands(vec![command])
}

// ---- PCIe MaxReadReq --------------------------------------------------------------------------

/// Offset of the PCIe Device Control register in a PCI config space.
fn device_control(config: &[u8]) -> Option<usize> {
    let mut ptr = usize::from(*config.get(0x34)? & 0xfc);
    for _ in 0..48 {
        if ptr == 0 {
            return None;
        }
        let (id, next) = (*config.get(ptr)?, *config.get(ptr + 1)?);
        if id == 0x10 {
            config.get(ptr + 9)?;
            return Some(ptr + 8);
        }
        ptr = usize::from(next & 0xfc);
    }
    None
}

/// MaxReadReq in bytes from the PCIe capability in the PCI config space.
fn max_read_req(config: &[u8]) -> Option<u32> {
    let at = device_control(config)?;
    let ctl = u16::from_le_bytes([config[at], config[at + 1]]);
    match (ctl >> 12) & 7 {
        code @ 0..=5 => Some(128 << code),
        _ => None,
    }
}

fn maxreadreq(ctx: &Ctx, nic: &Interface, label: &str) -> Hint {
    let roots = ctx.roots;
    let id = format!("pcie_maxreadreq:{}", nic.pci);
    let title = format!("PCIe MaxReadReq {label}");
    let config: PathBuf = roots.pci_dir(&nic.pci).join("config");
    let Some(bytes) = fs::read(&config).ok().as_deref().and_then(max_read_req) else {
        return hint(
            &id,
            Level::Info,
            &title,
            format!("{}: not readable (needs root and a PCIe device)", nic.pci),
            "",
        );
    };
    let level = match bytes {
        0..=1023 => Level::Warn,
        1024..=4095 => Level::Info,
        _ => Level::Ok,
    };
    let mut detail = format!(
        "{}: {bytes} B, the target is {MAX_READ_REQ_TARGET} B",
        nic.pci
    );
    if bytes >= MAX_READ_REQ_TARGET {
        return hint(&id, level, &title, detail, "");
    }
    if roots.in_vm() {
        detail.push_str(
            "; this host is a virtual machine, so the value read back is the hypervisor's copy of the register: verify it on the hypervisor host",
        );
    }
    let suggestion =
        "Raise the PCIe maximum read request size to 4096 B; 1024 B and above is acceptable.";
    if fs::symlink_metadata(roots.pci_dir(&nic.pci).join("physfn")).is_ok() {
        detail.push_str(
            "; this is a virtual function, so no command is suggested: its MaxReadReq belongs to the physical function",
        );
        return hint(&id, level, &title, detail, suggestion);
    }
    hint(&id, level, &title, detail, suggestion)
        .with_commands(vec![format!("setpci -s {} CAP_EXP+8.w=5000:7000", nic.pci)])
}

// ---- NIC port buffer (dcb) --------------------------------------------------------------------

struct PortBuffer {
    prio: Vec<u64>,
    sizes: Vec<u64>,
    total: u64,
}

fn read_port_buffer(ctx: &Ctx, netdev: &str) -> Tool<PortBuffer> {
    let out = run(ctx, &["dcb", "-j", "buffer", "show", "dev", netdev])?;
    let list = |v: &Value, key: &str| -> Option<Vec<u64>> {
        v.get(key)?.as_array()?.iter().map(Value::as_u64).collect()
    };
    let parsed = serde_json::from_str::<Value>(&out).ok().and_then(|v| {
        let v = if v.is_array() { v.get(0)?.clone() } else { v };
        Some(PortBuffer {
            prio: list(&v, "prio_buffer")?,
            sizes: list(&v, "buffer_size")?,
            total: v.get("total_size")?.as_u64()?,
        })
    });
    match parsed {
        Some(p) if p.prio.len() == 8 && !p.sizes.is_empty() => Ok(p),
        _ => Err(Gap::Failed("dcb printed no buffer layout".into())),
    }
}

fn port_buffer(ctx: &Ctx, nic: &Interface, label: &str, pause: Option<&Tool<Pause>>) -> Hint {
    let id = format!("port_buffer:{}", nic.pci);
    let title = format!("Port buffer {label}");
    let Some(netdev) = tool_netdev(nic) else {
        let why =
            unsafe_netdev(nic).unwrap_or_else(|| format!("{} has no network device", nic.pci));
        return hint(
            &id,
            Level::Info,
            &title,
            format!("{why}, so its port buffer cannot be read"),
            "",
        );
    };
    let layout = match read_port_buffer(ctx, netdev) {
        Ok(l) => l,
        Err(gap) => return gap_hint(&id, &title, label, "port buffer", "dcb", "iproute2", &gap),
    };
    let buffer = layout.prio[0] as usize;
    let tuned = layout.sizes.get(buffer) == Some(&layout.total)
        && layout
            .sizes
            .iter()
            .enumerate()
            .all(|(i, s)| i == buffer || *s == 0);
    if tuned {
        return hint(
            &id,
            Level::Ok,
            &title,
            format!(
                "the whole port buffer ({} B) of {label} is in buffer {buffer}",
                layout.total
            ),
            "",
        );
    }
    let list = |v: &[u64]| v.iter().map(u64::to_string).collect::<Vec<_>>().join(", ");
    let mut detail = format!(
        "buffer sizes of {label}: {} B of {} B in total; priorities 0 to 7 use buffers {}. NVIDIA advises against changing it on ConnectX-6 Dx, and the layout is reset by a driver reset or a VM start. The command shrinks every other buffer to 0",
        list(&layout.sizes),
        layout.total,
        list(&layout.prio)
    );
    let one_buffer = layout.prio.iter().all(|p| *p == layout.prio[0]);
    let flow_off = matches!(pause, Some(Ok(p)) if !p.rx && !p.tx);
    if !one_buffer {
        detail.push_str(
            "; the priorities use different buffers, and giving everything to one would starve the others, so no command is suggested",
        );
    } else if !flow_off {
        detail.push_str(
            "; turn flow control off first (run the flow control command), then check again; no command is suggested until then",
        );
    }
    let h = hint(
        &id,
        Level::Info,
        &title,
        detail,
        "Give the whole port buffer to the buffer all priorities use (dcb buffer set).",
    );
    if !(one_buffer && flow_off) {
        return h;
    }
    h.with_commands(vec![format!(
        "dcb buffer set dev {netdev} buffer-size all:0 {buffer}:{}",
        layout.total
    )])
}

// ---- NIC firmware (mstconfig) -----------------------------------------------------------------

/// One firmware parameter as `mstconfig -j` reports it.
#[derive(Debug, Clone)]
struct FwParam {
    next: Option<u64>,
    current: Option<u64>,
    next_text: String,
}

struct Firmware {
    advanced: FwParam,
    max_acc_out_read: FwParam,
    cqe_compression: FwParam,
}

/// A value as `mstconfig -j` prints it: a number, `"256"`, or an enum such as `"True(1)"`.
fn fw_number(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => match s.split_once('(') {
            Some((_, rest)) => rest.strip_suffix(')')?.parse().ok(),
            None => s.parse().ok(),
        },
        _ => None,
    }
}

fn fw_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn fw_param(tlv: &Value, name: &str) -> FwParam {
    let p = tlv.get(name);
    let next = p.and_then(|p| p.get("next_value"));
    FwParam {
        next: next.and_then(fw_number),
        current: p.and_then(|p| p.get("current_value")).and_then(fw_number),
        next_text: next.map_or_else(|| "not listed".to_string(), fw_text),
    }
}

/// Function 0 of the card: the same bus and device with function digit 0.
fn function_zero(pci: &str) -> String {
    match pci.rsplit_once('.') {
        Some((head, _)) => format!("{head}.0"),
        None => pci.to_string(),
    }
}

/// Creates a directory only this process can use, for the `-j` output of one call.
fn private_dir() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let dir = std::env::temp_dir().join(format!(
        "dpds-mst-{}-{}-{nanos}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

/// Queries the firmware settings with `tool` (`mstconfig` or `mlxconfig`).
fn query_firmware(ctx: &Ctx, tool: &str, bdf: &str) -> Tool<Firmware> {
    time_left(ctx)?;
    let dir = private_dir()
        .map_err(|e| Gap::Failed(format!("no temporary directory for {tool}: {e}")))?;
    let file = dir.join("query.json");
    let answer = run(
        ctx,
        &[tool, "-d", bdf, "-e", "-j", &file.to_string_lossy(), "q"],
    );
    let json = fs::read_to_string(&file);
    let _ = fs::remove_dir_all(&dir);
    answer?;
    let parsed = json
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let tlv = parsed.as_ref().and_then(|v| {
        v.as_object()?
            .values()
            .find_map(|d| d.get("tlv_configuration"))
    });
    match tlv {
        Some(t) if t.get("ADVANCED_PCI_SETTINGS").is_some() => Ok(Firmware {
            advanced: fw_param(t, "ADVANCED_PCI_SETTINGS"),
            max_acc_out_read: fw_param(t, "MAX_ACC_OUT_READ"),
            cqe_compression: fw_param(t, "CQE_COMPRESSION"),
        }),
        _ => Err(Gap::Failed(format!("{tool} printed no firmware settings"))),
    }
}

fn firmware(ctx: &Ctx, nic: &Interface, label: &str) -> Hint {
    let id = format!("firmware:{}", nic.pci);
    let title = format!("NIC firmware {label}");
    let bdf = function_zero(&nic.pci);
    // mstconfig (mstflint) first, mlxconfig (MFT) when it is not installed
    let (tool, state) = match query_firmware(ctx, "mstconfig", &bdf) {
        Err(Gap::Missing) => ("mlxconfig", query_firmware(ctx, "mlxconfig", &bdf)),
        other => ("mstconfig", other),
    };
    let fw = match state {
        Ok(fw) => fw,
        Err(gap) => {
            return gap_hint(
                &id,
                &title,
                label,
                "firmware settings",
                "mstconfig",
                "mstflint",
                &gap,
            )
        }
    };
    let targets = [
        (&fw.advanced, 1),
        (&fw.max_acc_out_read, FW_MAX_ACC_OUT_READ),
        (&fw.cqe_compression, FW_CQE_COMPRESSION),
    ];
    let next_tuned = targets.iter().all(|(p, t)| p.next == Some(*t));
    if next_tuned {
        let behind = targets
            .iter()
            .any(|(p, t)| p.current.is_some_and(|c| c != *t));
        return if behind {
            hint(
                &id,
                Level::Info,
                &title,
                format!("the next-boot firmware settings of {label} are set (ADVANCED_PCI_SETTINGS=1, MAX_ACC_OUT_READ={FW_MAX_ACC_OUT_READ}, CQE_COMPRESSION={FW_CQE_COMPRESSION}) but the running firmware uses other values: it is pending a firmware reset, or the firmware clamped a value{}", hypervisor_note(ctx)),
                "",
            )
        } else {
            hint(
                &id,
                Level::Ok,
                &title,
                format!("the firmware settings of {label} are tuned"),
                "",
            )
        };
    }
    let set = |args: &str| format!("{tool} -d {bdf} -y set {args}");
    hint(
        &id,
        Level::Info,
        &title,
        format!(
            "next-boot firmware settings of {label}: ADVANCED_PCI_SETTINGS={}, MAX_ACC_OUT_READ={}, CQE_COMPRESSION={}; the targets are ADVANCED_PCI_SETTINGS=1, MAX_ACC_OUT_READ={FW_MAX_ACC_OUT_READ}, CQE_COMPRESSION={FW_CQE_COMPRESSION}. The commands write the NVRAM of the whole card of {}: the change persists across reboots and takes effect only after a firmware reset, which DPDS never runs{}",
            fw.advanced.next_text, fw.max_acc_out_read.next_text, fw.cqe_compression.next_text, nic.pci, hypervisor_note(ctx)
        ),
        "Set the firmware values with mstconfig; they take effect after a firmware reset.",
    )
    .with_commands(vec![
        set("ADVANCED_PCI_SETTINGS=1"),
        set(&format!(
            "MAX_ACC_OUT_READ={FW_MAX_ACC_OUT_READ} CQE_COMPRESSION={FW_CQE_COMPRESSION}"
        )),
    ])
}
