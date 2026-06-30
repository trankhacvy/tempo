//! Trader archetypes. Each persona maps `(inventory, rng)` to a per-round plan:
//! how many orders to send, which side(s), whether to cross the spread, and a size
//! multiplier. Pure and deterministic given the rng.

use crate::rng::SimRng;

pub const BUY: u8 = 0;
pub const SELL: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Persona {
    /// Small, roughly symmetric flow — ambient volume that keeps the tape alive.
    Noise,
    /// Directional runs that push the clearing price one way for a stretch.
    Momentum,
    /// Rests inside the spread; adds book depth and usually does not fill.
    Passive,
    /// Large, one-sided, leverage-building flow — a liquidation candidate (Phase B).
    Reckless,
}

impl Persona {
    pub fn parse(s: &str) -> Persona {
        match s.trim().to_ascii_lowercase().as_str() {
            "momentum" => Persona::Momentum,
            "passive" => Persona::Passive,
            "reckless" => Persona::Reckless,
            _ => Persona::Noise,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Persona::Noise => "noise",
            Persona::Momentum => "momentum",
            Persona::Passive => "passive",
            Persona::Reckless => "reckless",
        }
    }

    /// Whether this persona's orders cross the spread (and so can fill). Passive
    /// rests inside and is the only non-crossing persona.
    pub fn crosses(&self) -> bool {
        !matches!(self, Persona::Passive)
    }

    /// Per-order size multiplier applied to the configured base size.
    pub fn size_mult(&self) -> u64 {
        match self {
            Persona::Reckless => 4,
            Persona::Momentum => 2,
            _ => 1,
        }
    }

    /// Decide this round's plan. `max_orders` is the per-round budget (already
    /// clamped to the protocol's per-trader cap by the caller).
    pub fn plan_round(&self, inventory: i64, rng: &mut SimRng, max_orders: u8) -> RoundPlan {
        let cap = max_orders.max(1);
        match self {
            Persona::Noise => RoundPlan {
                count: rng.range(1, cap.min(2) as u64) as u8,
                side: SidePlan::Random,
            },
            Persona::Passive => RoundPlan {
                count: rng.range(1, cap.min(2) as u64) as u8,
                side: SidePlan::Random,
            },
            Persona::Momentum => RoundPlan {
                count: rng.range(1, cap.min(3) as u64) as u8,
                side: SidePlan::Fixed(if rng.bool() { BUY } else { SELL }),
            },
            Persona::Reckless => {
                // Build toward (and beyond) a one-sided position. If already heavily
                // one-sided, keep pressing that side to drive toward liquidation.
                let side = if inventory > 0 {
                    BUY
                } else if inventory < 0 {
                    SELL
                } else if rng.bool() {
                    BUY
                } else {
                    SELL
                };
                RoundPlan {
                    count: rng.range(1, cap.min(2) as u64) as u8,
                    side: SidePlan::Fixed(side),
                }
            }
        }
    }
}

/// How a round picks the side for each of its orders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidePlan {
    Fixed(u8),
    Random,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoundPlan {
    pub count: u8,
    side: SidePlan,
}

impl RoundPlan {
    pub fn next_side(&self, rng: &mut SimRng) -> u8 {
        match self.side {
            SidePlan::Fixed(s) => s,
            SidePlan::Random => {
                if rng.bool() {
                    BUY
                } else {
                    SELL
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrips_known_personas() {
        for p in [
            Persona::Noise,
            Persona::Momentum,
            Persona::Passive,
            Persona::Reckless,
        ] {
            assert_eq!(Persona::parse(p.as_str()), p);
        }
        assert_eq!(Persona::parse("garbage"), Persona::Noise);
    }

    #[test]
    fn passive_is_the_only_non_crossing() {
        assert!(!Persona::Passive.crosses());
        assert!(Persona::Noise.crosses());
        assert!(Persona::Momentum.crosses());
        assert!(Persona::Reckless.crosses());
    }

    #[test]
    fn reckless_presses_its_existing_side() {
        let mut rng = SimRng::new(3);
        let long = Persona::Reckless.plan_round(500, &mut rng, 3);
        assert_eq!(long.next_side(&mut rng), BUY);
        let short = Persona::Reckless.plan_round(-500, &mut rng, 3);
        assert_eq!(short.next_side(&mut rng), SELL);
    }

    #[test]
    fn momentum_holds_one_side_for_the_round() {
        let mut rng = SimRng::new(11);
        let plan = Persona::Momentum.plan_round(0, &mut rng, 3);
        let s0 = plan.next_side(&mut rng);
        for _ in 0..5 {
            assert_eq!(plan.next_side(&mut rng), s0);
        }
    }

    #[test]
    fn count_never_exceeds_budget() {
        let mut rng = SimRng::new(99);
        for _ in 0..1000 {
            for p in [
                Persona::Noise,
                Persona::Momentum,
                Persona::Passive,
                Persona::Reckless,
            ] {
                let plan = p.plan_round(0, &mut rng, 3);
                assert!((1..=3).contains(&plan.count));
            }
        }
    }
}
