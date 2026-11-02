//! Counter structs, burst-bucket mapping and the dashboard / CSV text formats.
//!
//! The expected strings below are derived by hand from `emulator/dpds.c`
//! (`print_stats`, `print_burst_hist`, `write_stats_csv`) and must match
//! byte for byte, including the ANSI sequences:
//!
//! - every dashboard line ends with `\x1b[K\n` (clear to end of line)
//! - `overwrite == true` prefixes the whole block with `\x1b[<N>A` where
//!   N = 6 fixed lines + one line per NIC (`nics.len()`); `false` => no prefix
//! - per-second values (`pkt/s`, burst histogram) are `cur - prev`;
//!   `Total`/drops/worker lines are absolute
//! - `format_burst_hist` output: `"  {label} burst/s:"`, then for each bucket
//!   `" [lo]:d"` (last bucket, and any bucket with lo == hi) or `" [lo-hi]:d"`,
//!   then `" | avg: {:.1}"` only when the summed bucket deltas are > 0
//!   (avg = delta_pkts / total bursts), then `"\x1b[K\n"`
//! - CSV = header line + one values line, each ending in `\n`; no `\x1b`.

use std::mem::{align_of, size_of};
use std::sync::atomic::Ordering::Relaxed;

use dpds::stats::*;

const EL: &str = "\x1b[K";

// ---------- burst bucket mapping ----------

#[test]
fn burst_bucket_is_floor_log2() {
    let expected = [
        (1usize, 0usize),
        (2, 1),
        (3, 1),
        (4, 2),
        (5, 2),
        (7, 2),
        (8, 3),
        (9, 3),
        (15, 3),
        (16, 4),
    ];
    for (n, bucket) in expected {
        assert_eq!(burst_bucket(n), bucket, "burst of {n}");
    }
}

#[test]
fn every_burst_size_up_to_max_maps_inside_the_histogram() {
    for n in 1..=dpds::consts::MAX_BURST_SIZE {
        assert!(burst_bucket(n) < dpds::consts::BURST_HIST_BUCKETS);
    }
}

// ---------- counter structs ----------

#[test]
fn stage_counters_are_cache_line_aligned() {
    assert_eq!(align_of::<RxStats>(), 64);
    assert_eq!(align_of::<WorkerStats>(), 64);
    assert_eq!(align_of::<TxStats>(), 64);
    assert_eq!(size_of::<RxStats>() % 64, 0);
    assert_eq!(size_of::<WorkerStats>() % 64, 0);
    assert_eq!(size_of::<TxStats>() % 64, 0);
}

#[test]
fn stage_counters_live_on_separate_cache_lines_in_pipeline_stats() {
    let p = PipelineStats::default();
    let rx = &p.rx as *const _ as usize;
    let wk = &p.worker as *const _ as usize;
    let tx = &p.tx as *const _ as usize;
    assert_eq!(rx % 64, 0);
    assert_eq!(wk % 64, 0);
    assert_eq!(tx % 64, 0);
    assert!(rx.abs_diff(wk) >= 64 && wk.abs_diff(tx) >= 64 && rx.abs_diff(tx) >= 64);
}

#[test]
fn counters_are_shareable_across_lcores() {
    fn assert_sync_send<T: Sync + Send>() {}
    assert_sync_send::<PipelineStats>();
    assert_sync_send::<RxStats>();
    assert_sync_send::<WorkerStats>();
    assert_sync_send::<TxStats>();
}

#[test]
fn default_counters_are_zero_and_snapshot_is_default() {
    let p = PipelineStats::default();
    assert_eq!(p.snapshot(), StatsSnapshot::default());
    assert_eq!(p.rx.received.load(Relaxed), 0);
    assert_eq!(p.rx.burst_hist.len(), 5);
    assert_eq!(p.tx.burst_hist.len(), 5);
}

#[test]
fn snapshot_copies_every_counter_field() {
    let p = PipelineStats::default();
    p.rx.received.store(1, Relaxed);
    p.rx.enqueued.store(2, Relaxed);
    p.rx.rx_ring_drops.store(3, Relaxed);
    for (i, h) in p.rx.burst_hist.iter().enumerate() {
        h.store(10 + i as u64, Relaxed);
    }
    p.worker.received.store(4, Relaxed);
    p.worker.sent.store(5, Relaxed);
    p.worker.emulator_drops.store(6, Relaxed);
    p.worker.tx_ring_drops.store(7, Relaxed);
    p.tx.received.store(8, Relaxed);
    p.tx.transmitted.store(9, Relaxed);
    p.tx.tx_drops.store(10, Relaxed);
    for (i, h) in p.tx.burst_hist.iter().enumerate() {
        h.store(20 + i as u64, Relaxed);
    }

    let s = p.snapshot();
    assert_eq!(s.rx.received, 1);
    assert_eq!(s.rx.enqueued, 2);
    assert_eq!(s.rx.rx_ring_drops, 3);
    assert_eq!(s.rx.burst_hist, [10, 11, 12, 13, 14]);
    assert_eq!(s.worker.received, 4);
    assert_eq!(s.worker.sent, 5);
    assert_eq!(s.worker.emulator_drops, 6);
    assert_eq!(s.worker.tx_ring_drops, 7);
    assert_eq!(s.tx.received, 8);
    assert_eq!(s.tx.transmitted, 9);
    assert_eq!(s.tx.tx_drops, 10);
    assert_eq!(s.tx.burst_hist, [20, 21, 22, 23, 24]);
}

// ---------- format_burst_hist ----------

#[test]
fn burst_hist_line_with_bursts_has_labels_deltas_and_average() {
    let cur = [15u64, 30, 50, 70, 90];
    let prev = [5u64, 10, 10, 10, 10];
    // deltas 10,20,40,60,80 => 210 bursts; 420 pkts => avg 2.0
    let line = format_burst_hist("RX", &cur, &prev, 420);
    assert_eq!(
        line,
        format!("  RX burst/s: [1]:10 [2-3]:20 [4-7]:40 [8-15]:60 [16]:80 | avg: 2.0{EL}\n")
    );
}

#[test]
fn burst_hist_line_without_bursts_omits_average() {
    let z = [0u64; 5];
    let line = format_burst_hist("TX", &z, &z, 0);
    assert_eq!(
        line,
        format!("  TX burst/s: [1]:0 [2-3]:0 [4-7]:0 [8-15]:0 [16]:0{EL}\n")
    );
    assert!(!line.contains("avg"));
}

#[test]
fn burst_hist_average_rounds_like_printf_one_decimal() {
    let z = [0u64; 5];
    // 1 pkt / 4 bursts = 0.25 -> printf("%.1f") gives "0.2" (round-half-even on the exact value)
    let line = format_burst_hist("RX", &[4, 0, 0, 0, 0], &z, 1);
    assert!(line.contains(" | avg: 0.2\x1b[K"), "{line:?}");
    // 500 / 105 = 4.76... -> 4.8
    let line = format_burst_hist("RX", &[5, 10, 20, 30, 40], &z, 500);
    assert!(line.contains(" | avg: 4.8\x1b[K"), "{line:?}");
}

#[test]
fn cumulative_histogram_uses_a_zero_baseline() {
    let line = format_burst_hist("RX", &[3, 0, 0, 0, 1], &[0; 5], 19);
    assert_eq!(
        line,
        format!("  RX burst/s: [1]:3 [2-3]:0 [4-7]:0 [8-15]:0 [16]:1 | avg: 4.8{EL}\n")
    );
}

// ---------- format_dashboard ----------

fn sample() -> (StatsSnapshot, StatsSnapshot, Vec<(u16, NicStats)>) {
    let mut cur = StatsSnapshot::default();
    cur.rx.received = 1500;
    cur.rx.rx_ring_drops = 3;
    cur.rx.burst_hist = [10, 20, 30, 40, 50];
    cur.worker.received = 1400;
    cur.worker.sent = 1300;
    cur.worker.emulator_drops = 70;
    cur.worker.tx_ring_drops = 30;
    cur.tx.transmitted = 1290;
    cur.tx.tx_drops = 10;
    cur.tx.burst_hist = [1, 2, 3, 4, 5];

    let mut prev = StatsSnapshot::default();
    prev.rx.received = 1000;
    prev.rx.burst_hist = [5, 10, 10, 10, 10];
    prev.tx.transmitted = 800;

    let nics = vec![
        (
            0u16,
            NicStats {
                imissed: 1,
                ierrors: 2,
                oerrors: 3,
                rx_nombuf: 4,
            },
        ),
        (
            1u16,
            NicStats {
                imissed: 50,
                ierrors: 60,
                oerrors: 70,
                rx_nombuf: 80,
            },
        ),
    ];
    (cur, prev, nics)
}

fn expected_block() -> String {
    [
        format!("--- Emulator Stats (1s interval) ---{EL}\n"),
        format!("RX:     500 pkt/s | Total: 1500 | Ring drops: 3{EL}\n"),
        format!("  RX burst/s: [1]:5 [2-3]:10 [4-7]:20 [8-15]:30 [16]:40 | avg: 4.8{EL}\n"),
        format!("Worker: recv 1400 | sent 1300 | emulator drops: 70 | TX ring drops: 30{EL}\n"),
        format!("TX:     490 pkt/s | Total: 1290 | TX drops: 10{EL}\n"),
        format!("  TX burst/s: [1]:1 [2-3]:2 [4-7]:3 [8-15]:4 [16]:5 | avg: 32.7{EL}\n"),
        format!("NIC 0:  imissed=1 ierr=2 oerr=3 rx_nombuf=4{EL}\n"),
        format!("NIC 1:  imissed=50 ierr=60 oerr=70 rx_nombuf=80{EL}\n"),
    ]
    .concat()
}

#[test]
fn dashboard_first_print_has_no_cursor_up() {
    let (cur, prev, nics) = sample();
    assert_eq!(
        format_dashboard(&cur, &prev, &nics, false),
        expected_block()
    );
}

#[test]
fn dashboard_overwrite_moves_cursor_up_by_six_plus_port_count() {
    let (cur, prev, nics) = sample();
    assert_eq!(
        format_dashboard(&cur, &prev, &nics, true),
        format!("\x1b[8A{}", expected_block())
    );
}

#[test]
fn dashboard_cursor_up_count_tracks_number_of_nics() {
    let (cur, prev, nics) = sample();
    let one = &nics[..1];
    let out = format_dashboard(&cur, &prev, one, true);
    assert!(out.starts_with("\x1b[7A--- Emulator Stats"), "{out:?}");
    assert_eq!(out.matches("NIC ").count(), 1);
    // Every line of the block (not the cursor prefix) is cleared.
    assert_eq!(out.matches("\x1b[K\n").count(), 7);
}

#[test]
fn dashboard_with_all_zero_stats() {
    let z = StatsSnapshot::default();
    let nics = [(0u16, NicStats::default())];
    assert_eq!(
        format_dashboard(&z, &z, &nics, false),
        [
            format!("--- Emulator Stats (1s interval) ---{EL}\n"),
            format!("RX:     0 pkt/s | Total: 0 | Ring drops: 0{EL}\n"),
            format!("  RX burst/s: [1]:0 [2-3]:0 [4-7]:0 [8-15]:0 [16]:0{EL}\n"),
            format!("Worker: recv 0 | sent 0 | emulator drops: 0 | TX ring drops: 0{EL}\n"),
            format!("TX:     0 pkt/s | Total: 0 | TX drops: 0{EL}\n"),
            format!("  TX burst/s: [1]:0 [2-3]:0 [4-7]:0 [8-15]:0 [16]:0{EL}\n"),
            format!("NIC 0:  imissed=0 ierr=0 oerr=0 rx_nombuf=0{EL}\n"),
        ]
        .concat()
    );
}

#[test]
fn dashboard_nic_line_uses_the_dpdk_port_id_not_the_index() {
    let z = StatsSnapshot::default();
    let nics = [(3u16, NicStats::default())];
    let out = format_dashboard(&z, &z, &nics, false);
    assert!(out.contains("NIC 3:  imissed=0"), "{out:?}");
}

// ---------- format_csv ----------

const CSV_HEADER_TWO_PORTS: &str = "rx_received,rx_ring_drops,\
rx_burst_1,rx_burst_2_3,rx_burst_4_7,rx_burst_8_15,rx_burst_16,\
worker_received,worker_sent,worker_emulator_drops,worker_tx_ring_drops,\
tx_transmitted,tx_drops,\
tx_burst_1,tx_burst_2_3,tx_burst_4_7,tx_burst_8_15,tx_burst_16,\
nic_0_imissed,nic_0_ierrors,nic_0_oerrors,nic_0_rx_nombuf,\
nic_1_imissed,nic_1_ierrors,nic_1_oerrors,nic_1_rx_nombuf";

#[test]
fn csv_header_and_row_match_the_c_format() {
    let (cur, _prev, nics) = sample();
    let csv = format_csv(&cur, &nics);
    let expected = format!(
        "{CSV_HEADER_TWO_PORTS}\n\
         1500,3,10,20,30,40,50,1400,1300,70,30,1290,10,1,2,3,4,5,1,2,3,4,50,60,70,80\n"
    );
    assert_eq!(csv, expected);
}

#[test]
fn csv_has_exactly_two_lines_with_matching_column_counts() {
    let (cur, _prev, nics) = sample();
    let csv = format_csv(&cur, &nics);
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0].split(',').count(),
        lines[1].split(',').count(),
        "header and values must have the same number of columns"
    );
    assert!(csv.ends_with('\n'));
    assert!(!csv.contains('\x1b'));
}

#[test]
fn csv_single_port_has_only_that_ports_columns() {
    let z = StatsSnapshot::default();
    let csv = format_csv(&z, &[(0u16, NicStats::default())]);
    let header = csv.lines().next().unwrap();
    assert!(header.ends_with(",nic_0_imissed,nic_0_ierrors,nic_0_oerrors,nic_0_rx_nombuf"));
    assert!(!header.contains("nic_1_"));
    assert_eq!(header.split(',').count(), 2 + 5 + 4 + 2 + 5 + 4);
}

#[test]
fn csv_nic_columns_use_the_port_id() {
    let z = StatsSnapshot::default();
    let csv = format_csv(&z, &[(5u16, NicStats::default())]);
    assert!(csv.lines().next().unwrap().contains("nic_5_rx_nombuf"));
}
