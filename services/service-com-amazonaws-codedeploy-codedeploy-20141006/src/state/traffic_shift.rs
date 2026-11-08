// SPDX-License-Identifier: CC0-1.0

//! The INCREMENTS a deployment shifts its traffic in: how much each one moves,
//! and when it falls due.
//!
//! A deployment's resolved deployment configuration says how traffic moves from
//! the version a client is on to the version it is deploying: "The configuration
//! that specifies how traffic is shifted from one version of a Lambda function to
//! another version during an Lambda deployment"
//! (`smithy_query(query="shape",shape="TrafficRoutingConfig")`). Its
//! `TrafficRoutingType` is `TimeBasedCanary`, `TimeBasedLinear` or `AllAtOnce`
//! (`smithy_query(query="shape",shape="TrafficRoutingType")`), and the first two
//! describe a shift in SEVERAL increments: a canary "shifts traffic … in two
//! increments", the first of them `canaryPercentage` with `canaryInterval` "the
//! number of minutes between the first and second traffic shifts"; a linear shift
//! moves `linearPercentage` "at the start of each increment", with
//! `linearInterval` minutes "between each incremental traffic shift".
//!
//! So a deployment under such a configuration is NOT a single cutover: each
//! increment but the last parks its share of the traffic on the version being
//! shifted TO while the alias keeps invoking the one it is shifting FROM —
//! observable on that alias as `RoutingConfig.AdditionalVersionWeights` naming the
//! target version — and the deployment stays non-terminal until the last increment
//! moves the alias over and clears the weighted routing. That window is also what
//! makes the stops a deployment in flight can take reachable: an alarm sounding
//! during it ([`super::alarm_gate`]), or a caller's `StopDeployment`.
//!
//! This module is only the SCHEDULE — what each increment does and when — which
//! is a question about the configuration alone. What the alias the increments move
//! is comes from the revision's AppSpec ([`super::appspec`]), carrying one out is
//! [`super::lambda_traffic_shift`]'s, and the write itself
//! [`super::lambda_alias`]'s.

use std::collections::BTreeMap;

use crate::ops::types::TrafficRoutingConfig;

use super::appspec::LambdaTrafficShift;
use super::lambda_alias::AliasMove;
use super::{deployment_completion_due, Deployment};

/// The `TrafficRoutingType` values that shift traffic in increments over time
/// (`smithy_query(query="shape",shape="TrafficRoutingType")`: TimeBasedCanary |
/// TimeBasedLinear | AllAtOnce).
pub const TRAFFIC_ROUTING_TIME_BASED_CANARY: &str = "TimeBasedCanary";
/// See [`TRAFFIC_ROUTING_TIME_BASED_CANARY`].
pub const TRAFFIC_ROUTING_TIME_BASED_LINEAR: &str = "TimeBasedLinear";

/// How long this emulator holds one traffic increment per MINUTE the configuration
/// asked to wait between increments.
///
/// CodeDeploy measures `canaryInterval` and `linearInterval` in minutes, and that
/// duration is COMPRESSED here onto the same clock scale as every other transition
/// this emulator owes: an emulated deployment is served inside one caller's poll
/// loop, so a window measured in minutes would close after every client that reads
/// the alias to see the increment has given up, and the increment the
/// configuration asked for would be unobservable. The span is therefore CHOSEN,
/// and proportional to the configured interval, so a longer interval still holds
/// its increment longer than a shorter one; what is NOT emulator-chosen is the
/// number of increments, their percentages and their order, which come from the
/// configuration itself.
const TRAFFIC_SHIFT_MILLIS_PER_CONFIGURED_MINUTE: i64 = 2_000;

/// The largest number of increments one linear configuration is walked in.
///
/// `linearPercentage` is held to 1..=99 where the configuration is created, so a
/// linear shift has at most a hundred increments; the bound is stated here as well
/// so this schedule is finite whatever a stored configuration carries.
const MAX_INCREMENTS: usize = 100;

/// What one traffic increment moves, and when: the share of the traffic that sits
/// on the version being shifted to once the increment has happened, and how long
/// after the deployment started that is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Increment {
    /// The percentage of the traffic on the TARGET version from this increment
    /// on. The last increment of every schedule is the whole move: 100.
    pub percent: i32,
    /// How long after the deployment starts this increment falls due.
    pub offset_ms: i64,
}

impl Increment {
    /// Whether this increment completes the move, which is the one that moves the
    /// alias over rather than weighting traffic onto the target version.
    #[must_use]
    pub fn is_whole_move(&self) -> bool {
        self.percent >= 100
    }

    /// The write to the alias this increment is, for the shift `declared`.
    ///
    /// An increment that completes the move puts the alias ON the target version
    /// with no weighted routing left. Every earlier one leaves the alias invoking
    /// the version it is already invoking — the version the deployment is shifting
    /// FROM — and splits that share of its invocations off onto the target
    /// version, which is what `AdditionalVersionWeights` expresses.
    #[must_use]
    pub fn alias_move(&self, declared: &LambdaTrafficShift) -> AliasMove {
        if self.is_whole_move() {
            return AliasMove {
                function_version: Some(declared.target_version.clone()),
                additional_version_weights: BTreeMap::new(),
            };
        }
        AliasMove {
            function_version: None,
            additional_version_weights: BTreeMap::from([(
                declared.target_version.clone(),
                weight_of(self.percent),
            )]),
        }
    }
}

/// The schedule of a shift that moves all of the traffic in ONE step, which is
/// what `AllAtOnce` describes and what a rollback performs.
#[must_use]
pub fn single_shift() -> Vec<Increment> {
    vec![Increment {
        percent: 100,
        offset_ms: 0,
    }]
}

/// The increments a deployment under `config` shifts traffic in, in order.
///
/// `AllAtOnce`, a configuration with no traffic routing at all (the EC2/
/// on-premises configurations carry `minimumHealthyHosts` instead) and a type this
/// service does not know are all ONE increment — the whole move, at the instant
/// the deployment's traffic routing is complete — so every deployment has a
/// schedule and a single cutover is the same code path as a canary's last step.
#[must_use]
pub fn increments(config: Option<&TrafficRoutingConfig>) -> Vec<Increment> {
    let whole_move = single_shift();
    let Some(config) = config else {
        return whole_move;
    };
    match config.r#type.as_deref() {
        Some(TRAFFIC_ROUTING_TIME_BASED_CANARY) => {
            let Some(canary) = config.time_based_canary.as_ref() else {
                return whole_move;
            };
            let hold = hold_millis(canary.canary_interval);
            vec![
                Increment {
                    percent: canary.canary_percentage,
                    offset_ms: 0,
                },
                Increment {
                    percent: 100,
                    offset_ms: hold,
                },
            ]
        }
        Some(TRAFFIC_ROUTING_TIME_BASED_LINEAR) => {
            let Some(linear) = config.time_based_linear.as_ref() else {
                return whole_move;
            };
            let hold = hold_millis(linear.linear_interval);
            let step = linear.linear_percentage.max(1);
            (0..MAX_INCREMENTS)
                .map(|index| Increment {
                    percent: i32::try_from(index + 1)
                        .unwrap_or(i32::MAX)
                        .saturating_mul(step)
                        .min(100),
                    offset_ms: i64::try_from(index).unwrap_or(0) * hold,
                })
                // Up to and including the increment that completes the move.
                .scan(false, |done, increment| {
                    if *done {
                        return None;
                    }
                    *done = increment.is_whole_move();
                    Some(increment)
                })
                .collect()
        }
        // `AllAtOnce`, and a type outside the modelled enum, which the create of a
        // configuration refuses: one shift of all of the traffic.
        _ => whole_move,
    }
}

/// How long one increment is held for a configured interval of `minutes`; see
/// [`TRAFFIC_SHIFT_MILLIS_PER_CONFIGURED_MINUTE`].
fn hold_millis(minutes: i32) -> i64 {
    i64::from(minutes.max(0)) * TRAFFIC_SHIFT_MILLIS_PER_CONFIGURED_MINUTE
}

/// The share of an alias's invocations a percentage stands for, which is the unit
/// the alias reports it in: `AdditionalVersionWeights` carries the share as a
/// fraction between 0 and 1.
fn weight_of(percent: i32) -> f64 {
    f64::from(percent.clamp(0, 100)) / 100.0
}

/// The instant a deployment created at `create_time_ms` and started at
/// `starts_at_ms` completes its traffic routing, given the increments it shifts
/// traffic in: the last increment's own instant, and never earlier than the
/// completion the clock owes a deployment that shifts in one step.
#[must_use]
pub fn traffic_routing_due_ms(
    create_time_ms: i64,
    starts_at_ms: i64,
    increments: &[Increment],
) -> i64 {
    let last_increment = increments
        .last()
        .map_or(0, |increment| starts_at_ms + increment.offset_ms);
    last_increment.max(deployment_completion_due(create_time_ms))
}

/// The increment `deployment` owes its alias at `now_ms`, and its position in the
/// schedule — or `None` when it owes none yet.
///
/// The LAST increment that has fallen due, since a deployment nobody read for
/// several of them owes the alias the state it should hold now rather than each
/// state it passed through.
#[must_use]
pub fn due_increment(deployment: &Deployment, now_ms: i64) -> Option<(usize, Increment)> {
    let due = deployment
        .traffic_increments
        .iter()
        .take_while(|increment| deployment.starts_at_ms + increment.offset_ms <= now_ms)
        .count();
    if due <= deployment.traffic_increments_applied {
        return None;
    }
    deployment
        .traffic_increments
        .get(due - 1)
        .map(|increment| (due - 1, *increment))
}

#[cfg(test)]
mod tests {
    //! What a traffic routing configuration schedules, over the configuration
    //! forms a client creates. The same rule is asserted end to end, through the
    //! operations a client calls, in `tests/all/lambda_traffic_shifting.rs`.

    use super::*;
    use crate::ops::types::{TimeBasedCanary, TimeBasedLinear};

    /// A `TimeBasedCanary` configuration.
    fn canary(percentage: i32, interval: i32) -> TrafficRoutingConfig {
        TrafficRoutingConfig {
            r#type: Some(TRAFFIC_ROUTING_TIME_BASED_CANARY.to_string()),
            time_based_canary: Some(TimeBasedCanary {
                canary_percentage: percentage,
                canary_interval: interval,
            }),
            time_based_linear: None,
        }
    }

    /// A `TimeBasedLinear` configuration.
    fn linear(percentage: i32, interval: i32) -> TrafficRoutingConfig {
        TrafficRoutingConfig {
            r#type: Some(TRAFFIC_ROUTING_TIME_BASED_LINEAR.to_string()),
            time_based_canary: None,
            time_based_linear: Some(TimeBasedLinear {
                linear_percentage: percentage,
                linear_interval: interval,
            }),
        }
    }

    #[test]
    fn a_configuration_schedules_the_increments_it_describes() {
        let all_at_once = TrafficRoutingConfig {
            r#type: Some("AllAtOnce".to_string()),
            time_based_canary: None,
            time_based_linear: None,
        };
        let cases: &[(&str, Option<TrafficRoutingConfig>, Vec<i32>)] = &[
            ("no traffic routing at all", None, vec![100]),
            ("AllAtOnce", Some(all_at_once), vec![100]),
            ("a 10% canary", Some(canary(10, 2)), vec![10, 100]),
            ("a 99% canary", Some(canary(99, 1)), vec![99, 100]),
            (
                "a 25% linear shift",
                Some(linear(25, 1)),
                vec![25, 50, 75, 100],
            ),
            (
                "a 30% linear shift, whose last increment is the remainder",
                Some(linear(30, 1)),
                vec![30, 60, 90, 100],
            ),
            ("a 50% linear shift", Some(linear(50, 3)), vec![50, 100]),
        ];
        for (case, config, expected) in cases {
            let scheduled: Vec<i32> = increments(config.as_ref())
                .iter()
                .map(|increment| increment.percent)
                .collect();
            assert_eq!(
                &scheduled, expected,
                "case {case:?}: expected the increments {expected:?}, got {scheduled:?}"
            );
        }
    }

    #[test]
    fn each_increment_falls_due_one_configured_interval_after_the_one_before() {
        let cases: &[(&str, TrafficRoutingConfig, i32)] = &[
            ("a canary over two minutes", canary(10, 2), 2),
            ("a canary over no wait at all", canary(10, 0), 0),
            ("a linear shift every minute", linear(25, 1), 1),
            ("a linear shift every five minutes", linear(50, 5), 5),
        ];
        for (case, config, minutes) in cases {
            let scheduled = increments(Some(config));
            let hold = i64::from(*minutes) * TRAFFIC_SHIFT_MILLIS_PER_CONFIGURED_MINUTE;
            for (index, increment) in scheduled.iter().enumerate() {
                let expected = i64::try_from(index).expect("the index fits") * hold;
                assert_eq!(
                    increment.offset_ms, expected,
                    "case {case:?}: increment {index} must fall due at {expected} ms, got {}",
                    increment.offset_ms
                );
            }
        }
    }

    #[test]
    fn an_increment_weights_the_target_version_until_the_last_one_moves_the_alias() {
        let declared = LambdaTrafficShift {
            function_name: "orders-fn".to_string(),
            alias_name: "live".to_string(),
            current_version: Some("1".to_string()),
            target_version: "2".to_string(),
        };
        let moves: Vec<AliasMove> = increments(Some(&canary(10, 1)))
            .iter()
            .map(|increment| increment.alias_move(&declared))
            .collect();
        let expected = vec![
            AliasMove {
                function_version: None,
                additional_version_weights: BTreeMap::from([("2".to_string(), 0.1)]),
            },
            AliasMove {
                function_version: Some("2".to_string()),
                additional_version_weights: BTreeMap::new(),
            },
        ];
        assert_eq!(
            moves, expected,
            "a canary weights the target version, then moves the alias onto it: expected \
             {expected:?}, got {moves:?}"
        );
    }
}
