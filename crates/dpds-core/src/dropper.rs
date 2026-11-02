use rand::rngs::SmallRng;
use rand::SeedableRng;
use rand_distr::{Distribution, Uniform};
use serde::{Deserialize, Serialize};

#[repr(C)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum LossModel {
    // Random model parameter:
    // p = probability of packet loss
    Random(f64),
    /// GilbertElliot model parameters:
    /// p = transition probability from Good to Bad state
    /// r = transition probability from Bad to Good state
    /// k = probability of correct transmission in Good state
    /// h = probability of correct transmission in Bad state
    /// state_is_good: bool,
    GilbertElliot(f64, f64, f64, f64, bool),
}

pub struct Dropper {
    rng: SmallRng,
    uniform: Uniform<f64>,
    loss_model: LossModel,
}

impl Dropper {
    pub(crate) fn new(mut loss_model: LossModel) -> Self {
        let rng = SmallRng::from_rng(&mut rand::rng());
        let uniform = Uniform::new(0.0f64, 1.0f64).unwrap();

        // Ensure GilbertElliot always starts in the Good state
        if let LossModel::GilbertElliot(_, _, _, _, state_is_good) = &mut loss_model {
            *state_is_good = true;
        }

        Dropper {
            rng,
            uniform,
            loss_model,
        }
    }

    /// Returns true if the current packet should be dropped according to the
    /// configured loss model.
    pub fn dropper_drop(&mut self) -> bool {
        match &mut self.loss_model {
            LossModel::Random(p) => {
                // Fast path: skip RNG sampling entirely when no loss configured
                if *p == 0.0 {
                    return false;
                }
                self.uniform.sample(&mut self.rng) < *p
            }
            LossModel::GilbertElliot(p, r, k, h, state_is_good) => {
                // Determine if packet is lost based on current state
                let sample_loss = self.uniform.sample(&mut self.rng);
                let drop = if *state_is_good {
                    sample_loss > *k
                } else {
                    sample_loss > *h
                };

                // Transition to next state
                let sample_transition = self.uniform.sample(&mut self.rng);
                if *state_is_good {
                    if sample_transition < *p {
                        *state_is_good = false;
                    }
                } else if sample_transition < *r {
                    *state_is_good = true;
                }

                drop
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_zero_never_drops() {
        let mut d = Dropper::new(LossModel::Random(0.0));
        for _ in 0..1000 {
            assert!(!d.dropper_drop());
        }
    }

    #[test]
    fn random_one_always_drops() {
        let mut d = Dropper::new(LossModel::Random(1.0));
        for _ in 0..1000 {
            assert!(d.dropper_drop());
        }
    }

    #[test]
    fn gilbert_elliot_starts_in_good_state() {
        // k = 1.0 (no loss in Good), h = 0.0 (always lost in Bad), and no
        // transition out of Good (p = 0.0): the first packet must survive.
        let mut d = Dropper::new(LossModel::GilbertElliot(0.0, 0.0, 1.0, 0.0, false));
        assert!(!d.dropper_drop());
    }
}
