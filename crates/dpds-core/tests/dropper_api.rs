//! Direct tests of the dropper through its public API. Only
//! deterministic cases here (p/r/k/h in {0, 1}); statistics are in `loss.rs`.

use dpds_core::dropper::{Dropper, LossModel};

#[test]
fn dropper_new_is_public_and_random_zero_never_drops() {
    let mut d = Dropper::new(LossModel::Random(0.0));
    assert!((0..1000).all(|_| !d.dropper_drop()));
}

#[test]
fn random_one_always_drops() {
    let mut d = Dropper::new(LossModel::Random(1.0));
    assert!((0..1000).all(|_| d.dropper_drop()));
}

#[test]
fn gilbert_elliot_always_starts_in_the_good_state() {
    // Initial flag false would mean Bad (h = 0 drops everything, r = 0 never
    // leaves); the constructor must override it.
    let mut d = Dropper::new(LossModel::GilbertElliot(0.0, 0.0, 1.0, 0.0, false));
    assert!((0..100).all(|_| !d.dropper_drop()));
}

#[test]
fn gilbert_elliot_with_p_and_r_one_alternates() {
    // Good: keep (k = 1) then -> Bad (p = 1); Bad: drop (h = 0) then -> Good (r = 1).
    let mut d = Dropper::new(LossModel::GilbertElliot(1.0, 1.0, 1.0, 0.0, true));
    for i in 0..100 {
        assert_eq!(d.dropper_drop(), i % 2 == 1, "packet {i}");
    }
}

#[test]
fn gilbert_elliot_bad_state_absorbing() {
    let mut d = Dropper::new(LossModel::GilbertElliot(1.0, 0.0, 1.0, 0.0, true));
    assert!(!d.dropper_drop());
    assert!((0..100).all(|_| d.dropper_drop()));
}

#[test]
fn gilbert_elliot_good_state_with_k_zero_drops_everything() {
    let mut d = Dropper::new(LossModel::GilbertElliot(0.0, 1.0, 0.0, 1.0, true));
    assert!((0..100).all(|_| d.dropper_drop()));
}
