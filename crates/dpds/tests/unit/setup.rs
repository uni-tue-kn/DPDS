//! Startup validation and the constants.
//!
//! `validate_lcore_count(lcores, num_ports)` (`lcores` counts the main lcore):
//! Ok only for exactly `1 + num_ports` (main + one worker per direction).
//! Otherwise `Err(LcoreCount { found, required })`, Display counting LINKS
//! (not ports), for `--links` and for the `--config*` shorthand alike:
//! "<n> link(s) need <n+1> lcores (main + one worker per link), e.g. -l 0-<n>"
//! (grammatical number is free: "1 link needs 2 lcores ..." and "1 link(s) need
//! 2 lcores ..." are both accepted; the word "port" must not appear) and the
//! found count.
//!
//! `validate_port_count(n)`: Ok for 1 or 2, else `Err(PortCount(n))`.
//!
//! The v1 helper `egress_port` (`direction ^ 1`) is gone (spec v12 C4); a
//! source check below pins that.
//!
//! Pool size (N): the pool of a direction is not chosen by the user any more,
//! it is `required_mbufs(buffer_capacity, rx_desc, tx_desc)` = `capacity + rx +
//! tx + MBUF_CACHE_SIZE * 3 / 2 + 2 * BURST_SIZE` = `capacity + rx + tx + 768 +
//! 64` (plain integer sum, saturating, no rounding). `--num-mbufs`,
//! `DEFAULT_NUM_MBUFS`, `check_pool` and the "pool too small" error are gone.

use dpds::consts::*;
use dpds::setup::{required_mbufs, validate_lcore_count, validate_port_count, SetupError};

/// Asserts that `msg` is the lcore-count error for `links` links: it counts
/// links, names `links + 1` lcores, the "main + one worker per link" rule and
/// the `-l 0-<links>` example.
fn assert_lcore_message(msg: &str, links: usize, found: usize) {
    let lcores = links + 1;
    let singular = format!("{links} link needs {lcores} lcores");
    let plural = format!("{links} links need {lcores} lcores");
    let literal = format!("{links} link(s) need {lcores} lcores");
    assert!(
        [singular, plural, literal].iter().any(|h| msg.contains(h)),
        "links {links}: head of {msg:?}"
    );
    let tail = format!("lcores (main + one worker per link), e.g. -l 0-{links}");
    assert!(
        msg.contains(&tail),
        "links {links}: tail {tail:?} of {msg:?}"
    );
    assert!(
        !msg.contains("port"),
        "links {links}: the message counts links, not ports: {msg:?}"
    );
    assert!(
        msg.contains(&found.to_string()),
        "links {links}: names the found count {found}: {msg:?}"
    );
}

#[test]
fn one_link_needs_exactly_two_lcores() {
    assert!(validate_lcore_count(2, 1).is_ok());
    for n in [0usize, 1, 3, 4, 64] {
        let err = validate_lcore_count(n, 1).unwrap_err();
        assert_eq!(
            err,
            SetupError::LcoreCount {
                found: n,
                required: 2
            }
        );
        assert_lcore_message(&err.to_string(), 1, n);
    }
}

#[test]
fn two_links_need_exactly_three_lcores() {
    assert!(validate_lcore_count(3, 2).is_ok());
    for n in [0usize, 1, 2, 4, 5, 64] {
        let err = validate_lcore_count(n, 2).unwrap_err();
        assert_eq!(
            err,
            SetupError::LcoreCount {
                found: n,
                required: 3
            }
        );
        assert_lcore_message(&err.to_string(), 2, n);
    }
}

#[test]
fn the_old_two_lcores_only_rule_is_gone_for_two_ports() {
    // 2 lcores were the only valid count; with two ports they are now too few.
    assert!(validate_lcore_count(2, 2).is_err());
    // ... and 3 lcores were an error with one port, they still are.
    assert!(validate_lcore_count(3, 1).is_err());
}

#[test]
fn one_or_two_ports_are_valid() {
    assert!(validate_port_count(1).is_ok());
    assert!(validate_port_count(2).is_ok());
}

#[test]
fn zero_or_three_plus_ports_are_errors() {
    for n in [0usize, 3, 4, 16] {
        let err = validate_port_count(n).unwrap_err();
        assert_eq!(err, SetupError::PortCount(n));
        assert!(err.to_string().contains(&n.to_string()), "{n}: {err}");
    }
}

#[test]
fn setup_error_is_a_std_error() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<SetupError>();
}

#[test]
fn the_stale_v1_helper_egress_port_is_gone() {
    // Directed links name their egress port; `direction ^ 1` has no meaning.
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/setup.rs"))
        .expect("src/setup.rs");
    assert!(
        !src.contains("egress_port"),
        "egress_port must be removed from setup.rs"
    );
}

// ── constants ───────────────────────────────────────────────────────

#[test]
fn constants_match_the_spec() {
    assert_eq!(BURST_SIZE, 32);
    assert_eq!(NIC_DESCRIPTORS, 4096);
    assert_eq!(MBUF_CACHE_SIZE, 512);
    assert_eq!(MTU, 1500);
}

#[test]
fn hardware_counters_are_polled_every_250_ms_like_preof() {
    assert_eq!(HW_STATS_POLL_INTERVAL_MS, 250);
}

// ── pool size ───────────────────────────────────────────────────────

#[test]
fn required_mbufs_is_the_exact_sum_of_its_terms() {
    // 512 * 3 / 2 = 768, 2 * 32 = 64.
    assert_eq!(required_mbufs(0, 0, 0), 768 + 64);
    assert_eq!(
        required_mbufs(100, 4096, 4096),
        100 + 4096 + 4096 + 768 + 64
    );
    assert_eq!(required_mbufs(4096, 4096, 4096), 13_120);
    assert_eq!(required_mbufs(100_000, 4096, 4096), 109_024);
    // each term counts exactly once
    let base = required_mbufs(10, 20, 30);
    assert_eq!(base, 10 + 20 + 30 + 832);
    assert_eq!(required_mbufs(11, 20, 30), base + 1);
    assert_eq!(required_mbufs(10, 21, 30), base + 1);
    assert_eq!(required_mbufs(10, 20, 31), base + 1);
}

#[test]
fn the_cache_term_uses_integer_division_of_the_constant() {
    // MBUF_CACHE_SIZE * 3 / 2 evaluated in integers, 2 * BURST_SIZE added.
    let fixed = required_mbufs(0, 0, 0);
    assert_eq!(
        fixed,
        (MBUF_CACHE_SIZE * 3 / 2) as u64 + 2 * BURST_SIZE as u64
    );
}

#[test]
fn the_derived_size_is_the_plain_sum_without_rounding() {
    // Not rounded to 2^q - 1 (what DPDK recommends for ring-backed pools):
    // 100 + 4096 + 4096 + 768 + 64 = 9124.
    assert_eq!(required_mbufs(100, 4096, 4096), 9_124);
    assert_ne!((required_mbufs(100, 4096, 4096) + 1).count_ones(), 1);
    // a 1M buffer: exactly 1_000_000 + 8192 + 832
    assert_eq!(required_mbufs(1_000_000, 4096, 4096), 1_009_024);
}

#[test]
fn rx_and_tx_descriptors_are_separate_terms() {
    // The caller passes the RX ring of port d and the TX ring of port e(d).
    assert_eq!(required_mbufs(10, 4096, 512), 10 + 4096 + 512 + 832);
    assert_eq!(required_mbufs(10, 512, 4096), 10 + 512 + 4096 + 832);
    assert_eq!(required_mbufs(10, 0, 0), 10 + 832);
}

#[test]
fn the_size_follows_the_applied_descriptor_counts_not_the_constant() {
    // A driver that clamped the rings to 512 / 256: that is what counts.
    assert_eq!(required_mbufs(100, 512, 256), 100 + 512 + 256 + 832);
    assert!(required_mbufs(100, 512, 256) < required_mbufs(100, NIC_DESCRIPTORS, NIC_DESCRIPTORS));
}

#[test]
fn each_direction_gets_its_own_size_from_its_own_ports_and_capacity() {
    // 2 ports, ring sizes of the ports differ: direction d reads RX of port d
    // and TX of the other port (links port 0 -> 1 and port 1 -> 0).
    let rx = [512u16, 2048];
    let tx = [1024u16, 4096];
    let cap = [1_000usize, 3_000];
    let sizes: Vec<u64> = (0..2usize)
        .map(|d| required_mbufs(cap[d], rx[d], tx[d ^ 1]))
        .collect();
    assert_eq!(sizes[0], 1_000 + 512 + 4096 + 832);
    assert_eq!(sizes[1], 3_000 + 2048 + 1024 + 832);
    assert_ne!(sizes[0], sizes[1]);
}

#[test]
fn the_size_function_never_panics_on_huge_capacities() {
    // buffer_capacity comes straight from a JSON file (usize).
    assert!(required_mbufs(usize::MAX, u16::MAX, u16::MAX) >= u32::MAX as u64);
    assert_eq!(required_mbufs(usize::MAX, 4096, 4096), u64::MAX);
    assert!(required_mbufs(u32::MAX as usize + 1, 0, 0) > u32::MAX as u64);
}

// ── links mode: 1 + number of links lcores ──────────────────────────
//
// With `--links` every link is one direction with its own worker, so the
// number of directions is the number of links (a hairpin, a one-way link, a
// ring member: one worker each) and the rule `validate_lcore_count(lcores,
// num_links)` is the same function: Ok iff `lcores == 1 + num_links`.

#[test]
fn the_lcore_rule_for_any_number_of_directions_is_one_plus_directions() {
    for directions in 1usize..=8 {
        assert!(validate_lcore_count(1 + directions, directions).is_ok());
        for wrong in [directions, 2 + directions, 0] {
            assert_eq!(
                validate_lcore_count(wrong, directions).unwrap_err(),
                SetupError::LcoreCount {
                    found: wrong,
                    required: 1 + directions
                }
            );
        }
    }
    // a ring of three links: 3 workers, 4 lcores
    let msg = validate_lcore_count(3, 3).unwrap_err().to_string();
    assert_lcore_message(&msg, 3, 3);
    // eight links
    let msg = validate_lcore_count(2, 8).unwrap_err().to_string();
    assert_lcore_message(&msg, 8, 2);
}
