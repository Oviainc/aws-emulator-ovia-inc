// SPDX-License-Identifier: CC0-1.0

//! The periodic HEALTH CHECK of one registered instance: what one check observes, and the
//! verdict a run of observations turns into.
//!
//! `DescribeInstanceHealth` reports the OUTCOME of the load balancer checking each
//! registered instance, not the age of its registration: an instance that answers the
//! check for `HealthyThreshold` consecutive checks is the one state in which a load
//! balancer routes traffic to it, and one that answers none of them for
//! `UnhealthyThreshold` consecutive checks is out of service with the instance as the
//! cause. Both thresholds are the load balancer's own configuration — "the number of
//! consecutive health check successes required before considering an unhealthy instance
//! healthy" and its failing counterpart — so the verdict only MOVES once that many checks
//! in a row disagree with it ([`HealthProbe::fold`]).
//!
//! # What one check observes
//!
//! A real check opens a connection to the instance on the load balancer's configured
//! `Target`, and this emulator makes no outbound connection — so what a check can observe
//! is whether the emulator holds something that stands behind the registration. An
//! instance is EC2's resource, so EC2 is asked ([`observe_instances`]):
//!
//! * an instance EC2 reports `running` is one with a backend behind it, so the check is
//!   answered;
//! * an instance EC2 reports in a state that serves nothing answers no check, and the
//!   condition reported for it is the sentence `InstanceState$Description` words for that
//!   state ([`DESCRIPTION_INSTANCE_PENDING`] and the two beside it);
//! * an id EC2 holds no instance for has nothing behind it either, so its checks fail as
//!   the instance's own doing;
//! * with no EC2 to ask at all, NOTHING is observed: no check ran, so the registration
//!   keeps the health it has rather than being failed on silence.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

use super::{
    HealthCheckConfig, InstanceHealth, HEALTH_CHECK_SPAN, HEALTH_FAILED_HEALTH_CHECKS,
    HEALTH_REGISTRATION_IN_PROGRESS, REASON_CODE_INSTANCE, STATE_OUT_OF_SERVICE,
};

/// `InstanceState$State`, from that member's documented valid values
/// (`InService` | `OutOfService` | `Unknown`): the state of an instance the load balancer
/// routes traffic to.
pub const STATE_IN_SERVICE: &str = "InService";

/// The `InstanceState$ReasonCode` and `$Description` of an instance there is no cause to
/// report for.
///
/// Both members are documented over the `OutOfService` instances — `ReasonCode` is
/// "information about the cause of `OutOfService` instances" and is one of
/// `ELB` | `Instance` | `N/A` — so an instance that IS in service reports the value both
/// members enumerate for "not applicable".
pub const NOT_APPLICABLE: &str = "N/A";

/// The health of an instance whose checks the load balancer's `HealthyThreshold` has
/// decided in the instance's favour: in service, with no cause to report.
pub const HEALTH_IN_SERVICE: InstanceHealth = InstanceHealth {
    state: STATE_IN_SERVICE,
    reason_code: NOT_APPLICABLE,
    description: NOT_APPLICABLE,
};

/// One of the descriptions `InstanceState$Description` enumerates: the instance is still
/// coming up, so nothing on it answers a check yet.
pub const DESCRIPTION_INSTANCE_PENDING: &str = "Instance is in pending state.";

/// The description `InstanceState$Description` enumerates for an instance that has been
/// stopped: it is there, and nothing on it answers a check.
pub const DESCRIPTION_INSTANCE_STOPPED: &str = "Instance is in stopped state.";

/// The description `InstanceState$Description` enumerates for an instance that has been
/// terminated.
pub const DESCRIPTION_INSTANCE_TERMINATED: &str = "Instance is in terminated state.";

/// What ONE health check of a registered instance observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceObservation {
    /// The backend answered the check.
    Answered,
    /// Nothing answered it, with the sentence `InstanceState$Description` words for the
    /// condition the check ran into — what a failing verdict then reports.
    Unanswered { description: &'static str },
}

/// The two thresholds the verdict of one load balancer's checks moves on: how many
/// consecutive checks have to agree before the verdict follows them.
///
/// A threshold the load balancer reports as less than one still takes one check, so no
/// verdict is ever reached by no check at all.
#[derive(Debug, Clone, Copy)]
pub struct HealthThresholds {
    pub healthy: u32,
    pub unhealthy: u32,
}

impl HealthThresholds {
    /// The thresholds `health_check` configures.
    pub fn of(health_check: &HealthCheckConfig) -> Self {
        Self {
            healthy: Self::at_least_one(health_check.healthy_threshold),
            unhealthy: Self::at_least_one(health_check.unhealthy_threshold),
        }
    }

    fn at_least_one(configured: i32) -> u32 {
        configured.max(1).unsigned_abs()
    }

    /// The longest run of agreeing checks either threshold can ask for: folding more
    /// checks of the SAME observation than this cannot move the verdict any further, which
    /// is what bounds the work one settle does.
    fn longest_run(self) -> u32 {
        self.healthy.max(self.unhealthy)
    }
}

/// The verdict the checks of one registration have reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Neither threshold has been reached yet: the checks the registration owes are still
    /// running, which is what `InstanceState$Description`'s "Instance registration is
    /// still in progress." words.
    Checking,
    /// `HealthyThreshold` consecutive checks were answered.
    InService,
    /// `UnhealthyThreshold` consecutive checks were not, carrying the condition the last
    /// of them observed.
    OutOfService { description: &'static str },
}

/// Where the periodic health check of ONE registration has got to: when its next check
/// comes due, what the checks since the last verdict observed, and the verdict they
/// reached.
///
/// The due instant is STORED and never slept for: a check is owed to the clock, so the
/// checks that have fallen due since the last read are the ones a read materialises
/// ([`HealthProbe::settle`]), and the same ones were owed at that instant whether or not
/// anybody called.
#[derive(Debug, Clone)]
pub struct HealthProbe {
    /// The instant this registration's next check falls due.
    next_check_at: SystemTime,
    /// Consecutive answered checks since the last unanswered one.
    passes: u32,
    /// Consecutive unanswered checks since the last answered one.
    failures: u32,
    verdict: Verdict,
}

impl HealthProbe {
    /// The checks a registration made at `registered_at` owes: none has run yet, so it
    /// carries no verdict, and the first falls due one [`HEALTH_CHECK_SPAN`] later.
    pub fn starting_at(registered_at: SystemTime) -> Self {
        Self {
            next_check_at: registered_at
                .checked_add(HEALTH_CHECK_SPAN)
                .unwrap_or(registered_at),
            passes: 0,
            failures: 0,
            verdict: Verdict::Checking,
        }
    }

    /// Run every check the clock has made due at `now`, each observing `observed`, and
    /// fold what they saw into the verdict.
    ///
    /// `observed` of `None` is "nothing could be observed": no check ran, so neither the
    /// schedule nor the verdict moves. The checks of one settle all observed the same
    /// thing, so at most [`HealthThresholds::longest_run`] of them are folded — a longer
    /// run of the same observation leaves the verdict exactly where that run already put
    /// it, and the work one call does stays bounded by the configured thresholds.
    pub fn settle(
        &mut self,
        observed: Option<InstanceObservation>,
        thresholds: HealthThresholds,
        now: SystemTime,
    ) {
        let Some(observed) = observed else {
            return;
        };
        let due = self.checks_due(now);
        if due == 0 {
            return;
        }
        self.next_check_at = self
            .next_check_at
            .checked_add(HEALTH_CHECK_SPAN.saturating_mul(due))
            .unwrap_or(now);
        for _ in 0..due.min(thresholds.longest_run()) {
            self.fold(observed, thresholds);
        }
    }

    /// How many checks of this registration have fallen due at `now`: the one due at
    /// [`HealthProbe::next_check_at`], plus one per [`HEALTH_CHECK_SPAN`] since.
    fn checks_due(&self, now: SystemTime) -> u32 {
        let Ok(since) = now.duration_since(self.next_check_at) else {
            return 0;
        };
        let span = HEALTH_CHECK_SPAN.as_millis().max(1);
        u32::try_from(since.as_millis() / span)
            .unwrap_or(u32::MAX)
            .saturating_add(1)
    }

    /// Move this registration's check schedule as its registration instant moves from
    /// `from` to `to`, keeping the checks that have already run: the next check falls due
    /// the same distance from the registration instant as before.
    pub fn reschedule(&mut self, from: SystemTime, to: SystemTime) {
        self.next_check_at = match to.duration_since(from) {
            Ok(later) => self.next_check_at.checked_add(later),
            Err(earlier) => self.next_check_at.checked_sub(earlier.duration()),
        }
        .unwrap_or(to);
    }

    /// Fold ONE check's observation into the verdict, which moves only once a threshold's
    /// worth of consecutive checks has agreed.
    fn fold(&mut self, observed: InstanceObservation, thresholds: HealthThresholds) {
        match observed {
            InstanceObservation::Answered => {
                self.failures = 0;
                self.passes = self.passes.saturating_add(1);
                if self.passes >= thresholds.healthy {
                    self.verdict = Verdict::InService;
                }
            }
            InstanceObservation::Unanswered { description } => {
                self.passes = 0;
                self.failures = self.failures.saturating_add(1);
                if self.failures >= thresholds.unhealthy {
                    self.verdict = Verdict::OutOfService { description };
                }
            }
        }
    }

    /// The health the three `InstanceState` members report for this registration.
    ///
    /// A registration whose checks have reached no verdict is Elastic Load Balancing's own
    /// doing — the checks it owes are still running — while a failing verdict is the
    /// instance's, with the condition the checks ran into.
    pub fn reported_health(&self) -> InstanceHealth {
        match self.verdict {
            Verdict::Checking => HEALTH_REGISTRATION_IN_PROGRESS,
            Verdict::InService => HEALTH_IN_SERVICE,
            Verdict::OutOfService { description } => InstanceHealth {
                state: STATE_OUT_OF_SERVICE,
                reason_code: REASON_CODE_INSTANCE,
                description,
            },
        }
    }
}

/// What one check of each of `instance_ids` observes, keyed by instance id — see the
/// module docs for what a check can observe at all.
///
/// The account's EC2 inventory is read ONCE for the whole batch, so the cost does not grow
/// with the number of registrations times the size of the inventory. EC2's own clock-owed
/// moves are materialised through EC2's own settle before it is read: an instance whose
/// move to `running` has already fallen due is running whether or not anybody described
/// it, so a check does not fail an instance that EC2's own reader reports as up.
pub fn observe_instances<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_ids: &BTreeSet<String>,
) -> BTreeMap<String, Option<InstanceObservation>> {
    if instance_ids.is_empty() {
        return BTreeMap::new();
    }
    let Some(inventory) = ec2::service_state(engine) else {
        // Nothing to ask, so nothing is observed about any of them.
        return instance_ids.iter().map(|id| (id.clone(), None)).collect();
    };
    let mut store = inventory.lock();
    store.materialize_due_transitions();
    instance_ids
        .iter()
        .map(|instance_id| {
            let observed = match store.instances.get(instance_id) {
                Some(instance) => observation_of(instance.state),
                // The account holds no such instance, so nothing stands behind the
                // registration: its checks fail as the instance's own doing.
                None => InstanceObservation::Unanswered {
                    description: HEALTH_FAILED_HEALTH_CHECKS.description,
                },
            };
            (instance_id.clone(), Some(observed))
        })
        .collect()
}

/// What a check observes about an instance EC2 reports in `state`.
///
/// Only a `running` instance has a backend behind it. Each other state that
/// `InstanceState$Description` words a sentence for reports that sentence; one it words
/// none for — an instance on its way out — is simply an instance that answers no check.
fn observation_of(state: ec2::InstanceStateName) -> InstanceObservation {
    let description = match state {
        ec2::InstanceStateName::Running => return InstanceObservation::Answered,
        ec2::InstanceStateName::Pending => DESCRIPTION_INSTANCE_PENDING,
        ec2::InstanceStateName::Stopped => DESCRIPTION_INSTANCE_STOPPED,
        ec2::InstanceStateName::Terminated => DESCRIPTION_INSTANCE_TERMINATED,
        ec2::InstanceStateName::Stopping | ec2::InstanceStateName::ShuttingDown => {
            HEALTH_FAILED_HEALTH_CHECKS.description
        }
    };
    InstanceObservation::Unanswered { description }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A health check configuring both thresholds.
    fn health_check(healthy: i32, unhealthy: i32) -> HealthCheckConfig {
        HealthCheckConfig {
            healthy_threshold: healthy,
            unhealthy_threshold: unhealthy,
            ..HealthCheckConfig::default_for_instance_port(80)
        }
    }

    /// The verdict follows the run of CHECKS, whichever way they go: a registration whose
    /// checks are answered reaches `InService` on the healthy threshold, one whose checks
    /// are not reaches `OutOfService` on the unhealthy one, and neither verdict is reached
    /// before that many checks have fallen due.
    #[test]
    fn the_verdict_follows_the_run_of_checks() {
        let registered_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for (case, healthy, unhealthy, observed, expected) in [
            (
                "answered, one check short of the healthy threshold",
                2,
                2,
                InstanceObservation::Answered,
                (1u32, HEALTH_REGISTRATION_IN_PROGRESS),
            ),
            (
                "answered, the healthy threshold reached",
                2,
                2,
                InstanceObservation::Answered,
                (2, HEALTH_IN_SERVICE),
            ),
            (
                "answered, a high healthy threshold reached",
                10,
                2,
                InstanceObservation::Answered,
                (10, HEALTH_IN_SERVICE),
            ),
            (
                "unanswered, one check short of the unhealthy threshold",
                2,
                3,
                InstanceObservation::Unanswered {
                    description: HEALTH_FAILED_HEALTH_CHECKS.description,
                },
                (2, HEALTH_REGISTRATION_IN_PROGRESS),
            ),
            (
                "unanswered, the unhealthy threshold reached",
                2,
                3,
                InstanceObservation::Unanswered {
                    description: HEALTH_FAILED_HEALTH_CHECKS.description,
                },
                (3, HEALTH_FAILED_HEALTH_CHECKS),
            ),
            (
                "unanswered by a stopped instance",
                2,
                2,
                InstanceObservation::Unanswered {
                    description: DESCRIPTION_INSTANCE_STOPPED,
                },
                (
                    2,
                    InstanceHealth {
                        state: STATE_OUT_OF_SERVICE,
                        reason_code: REASON_CODE_INSTANCE,
                        description: DESCRIPTION_INSTANCE_STOPPED,
                    },
                ),
            ),
        ] {
            let (checks, wanted) = expected;
            let thresholds = HealthThresholds::of(&health_check(healthy, unhealthy));
            let mut probe = HealthProbe::starting_at(registered_at);
            // One settle per check, each at the instant that check falls due, so the fold
            // is the run of checks the clock really owed.
            for check in 1..=checks {
                let at = registered_at + HEALTH_CHECK_SPAN.saturating_mul(check);
                probe.settle(Some(observed), thresholds, at);
            }
            let reported = probe.reported_health();
            assert_eq!(
                reported, wanted,
                "case {case:?}: after {checks} checks, expected {wanted:?}, got {reported:?}"
            );
        }
    }

    /// A settle that observes nothing moves neither the schedule nor the verdict: a
    /// verdict is never assumed from silence.
    #[test]
    fn a_check_that_observed_nothing_moves_nothing() {
        let registered_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let thresholds = HealthThresholds::of(&health_check(2, 2));
        let mut probe = HealthProbe::starting_at(registered_at);
        let long_after = registered_at + HEALTH_CHECK_SPAN.saturating_mul(100);
        probe.settle(None, thresholds, long_after);
        assert_eq!(
            probe.reported_health(),
            HEALTH_REGISTRATION_IN_PROGRESS,
            "nothing observed must leave the registration where it was, got {:?}",
            probe.reported_health()
        );
        // The checks it did not run are still owed, so the next settle that DOES observe
        // something reaches a verdict.
        probe.settle(
            Some(InstanceObservation::Answered),
            thresholds,
            long_after,
        );
        assert_eq!(
            probe.reported_health(),
            HEALTH_IN_SERVICE,
            "the owed checks must still be owed, got {:?}",
            probe.reported_health()
        );
    }

    /// A long gap between two reads folds the checks the clock owed, and the verdict a run
    /// of the same observation reaches does not depend on how many more of them ran.
    #[test]
    fn a_run_longer_than_the_threshold_reaches_the_same_verdict() {
        let registered_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let thresholds = HealthThresholds::of(&health_check(2, 2));
        for gap in [2u32, 3, 50, 10_000] {
            let mut probe = HealthProbe::starting_at(registered_at);
            probe.settle(
                Some(InstanceObservation::Answered),
                thresholds,
                registered_at + HEALTH_CHECK_SPAN.saturating_mul(gap),
            );
            let reported = probe.reported_health();
            assert_eq!(
                reported, HEALTH_IN_SERVICE,
                "case {gap} checks in one settle: expected {HEALTH_IN_SERVICE:?}, got {reported:?}"
            );
        }
    }
}
