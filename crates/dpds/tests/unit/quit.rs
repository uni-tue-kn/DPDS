//! The single shutdown flag.
//!
//! `Quit::new()` is not requested; `request()` makes `is_requested()` true
//! (and it stays true); `Default` equals `new`. `dpds::quit::QUIT` is the
//! process-wide instance (not touched by tests).

use dpds::quit::Quit;

#[test]
fn a_new_flag_is_not_requested() {
    assert!(!Quit::new().is_requested());
    assert!(!Quit::default().is_requested());
}

#[test]
fn request_sets_the_flag_and_it_stays_set() {
    let q = Quit::new();
    q.request();
    assert!(q.is_requested());
    q.request();
    assert!(q.is_requested());
}

#[test]
fn the_flag_is_visible_across_threads() {
    let q = std::sync::Arc::new(Quit::new());
    let q2 = q.clone();
    std::thread::spawn(move || q2.request()).join().unwrap();
    assert!(q.is_requested());
}
