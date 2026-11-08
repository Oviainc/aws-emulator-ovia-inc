// SPDX-License-Identifier: CC0-1.0

//! The API faults a RUNNING experiment imposes on another service, published where that
//! service reads them.
//!
//! # The rule this module implements
//!
//! An `aws:fis:inject-api-*` action does not describe a fault, it IS one: "Cause API
//! requests made with the specified IAM roles to fail with an internal error", as this
//! service's own action catalog states it ([`super::ACTION_CATALOG`]). So while such an
//! action is running, the calls it names fail — and that failure is answered by the service
//! whose API the action's `service` parameter names, not by FIS.
//!
//! What an action names is read off the action itself, never guessed: the `service` whose API
//! is affected, the `operations` of it, the `percentage` of calls, the `duration` it lasts,
//! and the IAM roles its `Roles` target RESOLVED to ([`super::action_target_arns`], the same
//! reading every other action's resources go through).
//!
//! # How long a fault lasts
//!
//! The DURATION the action asks for ([`fault_span`]), which is the caller's own value: an
//! action asked to fail a role's calls for two minutes fails them for two minutes. A fault is
//! measured by MAKING the calls, so that span is what a client observes, and the emulator's
//! compressed run clock ([`super::RUN_CLOCK_SCALE`], which is what makes an experiment's
//! STATUS reach a terminal value in seconds rather than in minutes) is not what a fault is
//! shortened to. It is never shorter than the window the run's plan gave the action
//! ([`super::ActionRun`]), so a fault covers at least every instant the experiment reports
//! the action running for.
//!
//! The two clocks therefore differ, deliberately, and the difference is only ever visible to
//! a client that does not look: one that READS its experiment is told the run is over and the
//! fault is withdrawn in the same breath (below), while one that reads nothing has its calls
//! failed for the span it asked for — which is what the real service does for that whole span
//! too. The alternative, compressing the fault as well, would answer a client that asked for
//! two minutes of injected failure with a couple of seconds of it, which is the one thing the
//! action exists to provide.
//!
//! # How it reaches the affected service
//!
//! Through the shared registry in the one engine both services hold
//! ([`::service_common::injected_api_faults`]), for the reason that module records: the
//! affected service cannot depend on this one. This module is the only WRITER — it publishes
//! the whole live set of an experiment's leases each time that experiment is settled
//! ([`publish_api_faults`]), so a fault is installed as the action starts and WITHDRAWN as
//! the settle that ended the action reports it, whether the action ended because its span
//! elapsed, because the experiment was stopped, or because a stop condition halted the run.
//! A client watching its experiment therefore never sees a fault outlive the run it belongs
//! to, and one that watches nothing has its calls failed for the span it asked for.
//!
//! A write a REPORTING operation performs, and only one the clock already owed: the action
//! started, or ended, at the instant the run's plan put it at, whether or not anybody polled
//! — which is why a client that starts an experiment and then polls it finds the fault live,
//! and finds it gone once the run is over.
//!
//! # What is NOT published
//!
//! An action whose injected failure this emulator cannot STATE is not published, and nothing
//! is invented for it: `aws:ec2:api-error` takes the error code to inject as a caller-written
//! `errorCode` parameter, and the HTTP status that goes with an arbitrary code is not
//! something this emulator can derive from the code — so that action affects no call here,
//! exactly as it did before, rather than failing one with a status nothing documents.

use std::time::{Duration, SystemTime};

use ::service_common::injected_api_faults::{
    ApiFaultLease, InjectedApiFaults, InjectedFault, MAX_LEASE_OPERATIONS, MAX_LEASE_PRINCIPALS,
    EVERY_CALL,
};

use super::{
    action_target_arns, ActionRunStatus, ExperimentRecord, ExperimentStatus,
    RESOURCE_TYPE_IAM_ROLE,
};

/// The published action id whose fault is an internal error: "Cause API requests made with
/// the specified IAM roles to fail with an internal error."
pub const ACTION_ID_FIS_INJECT_API_INTERNAL_ERROR: &str = "aws:fis:inject-api-internal-error";

/// The published action id whose fault is a throttle: "Cause API requests made with the
/// specified IAM roles to be throttled."
pub const ACTION_ID_FIS_INJECT_API_THROTTLE_ERROR: &str = "aws:fis:inject-api-throttle-error";

/// The published action id whose fault is an unavailable error: "Cause API requests made with
/// the specified IAM roles to fail with an unavailable error."
pub const ACTION_ID_FIS_INJECT_API_UNAVAILABLE_ERROR: &str =
    "aws:fis:inject-api-unavailable-error";

/// The action parameter naming the service whose API operations are affected, as the catalog
/// publishes it ("The AWS service whose API operations are affected.").
const PARAMETER_SERVICE: &str = "service";

/// The action parameter naming the operations to affect ("The API operations to affect,
/// separated by commas.").
const PARAMETER_OPERATIONS: &str = "operations";

/// The action parameter naming the share of calls to affect ("The percentage of calls to
/// affect."), which is optional.
const PARAMETER_PERCENTAGE: &str = "percentage";

/// How the `operations` parameter separates the operations it names.
const OPERATIONS_SEPARATOR: char = ',';

/// The longest a fault stands, whatever duration an action asks for: the longest span this
/// service accepts for an action's `duration` at all, which is the documented range of the
/// one action whose range FIS publishes
/// ([`WAIT_DURATION_MAX_MINUTES`](super::WAIT_DURATION_MAX_MINUTES) minutes for
/// `aws:fis:wait`). A caller cannot make a fault outlive that by asking for a longer span.
const MAX_FAULT_SPAN: Duration =
    Duration::from_secs(super::WAIT_DURATION_MAX_MINUTES * SECONDS_PER_MINUTE);

/// Seconds in a minute, for reading [`MAX_FAULT_SPAN`]'s documented bound in minutes.
const SECONDS_PER_MINUTE: u64 = 60;

/// The error code and HTTP status each injected fault class answers with.
///
/// The internal-error class is the one a recording of a live experiment measured: HTTP 500
/// with the error code `InternalError` and the message below. The throttle and unavailable
/// classes answer the code and status their own error class carries — a throttled request is
/// `RequestLimitExceeded` at 503, the status every other refusal of this kind in this
/// emulator answers with, and an unavailable one is `Unavailable` at 503 — with the sentence
/// that error class is worded with.
fn injected_fault(action_id: &str) -> Option<InjectedFault> {
    match action_id {
        ACTION_ID_FIS_INJECT_API_INTERNAL_ERROR => Some(InjectedFault {
            status: 500,
            code: "InternalError".to_string(),
            message: "An internal error has occurred".to_string(),
        }),
        ACTION_ID_FIS_INJECT_API_THROTTLE_ERROR => Some(InjectedFault {
            status: 503,
            code: "RequestLimitExceeded".to_string(),
            message: "Request limit exceeded.".to_string(),
        }),
        ACTION_ID_FIS_INJECT_API_UNAVAILABLE_ERROR => Some(InjectedFault {
            status: 503,
            code: "Unavailable".to_string(),
            message: "The server is overloaded and cannot handle the request.".to_string(),
        }),
        _ => None,
    }
}

/// Whether `action_id` is one of the API-fault actions this module publishes a lease for.
#[must_use]
pub fn injects_an_api_fault(action_id: &str) -> bool {
    injected_fault(action_id).is_some()
}

impl ExperimentRecord {
    /// The API-fault leases this experiment has LIVE, as of the status and the action
    /// progress the settle just left on it.
    ///
    /// Empty for an experiment that is no longer running the action — one that has reached a
    /// terminal status, one a stop is taking down, and one whose action the run has finished
    /// or never reached — which is what withdraws the fault: the live set is recomputed from
    /// the record each time, so nothing has to remember to take a lease away.
    ///
    /// The walk is over this experiment's own actions, bounded by
    /// [`MAX_ACTIONS_PER_TEMPLATE`](super::MAX_ACTIONS_PER_TEMPLATE), and each lease's
    /// operations and principals are bounded by the registry's own ceilings.
    pub fn live_api_fault_leases(&self) -> Vec<ApiFaultLease> {
        if self.status != ExperimentStatus::Running {
            return Vec::new();
        }
        let mut leases = Vec::new();
        for (name, run) in &self.action_runs {
            if run.status != ActionRunStatus::Running {
                continue;
            }
            let Some(action) = self.actions.get(name) else {
                continue;
            };
            let Some(action_id) = action.action_id.as_deref() else {
                continue;
            };
            let Some(fault) = injected_fault(action_id) else {
                continue;
            };
            let parameters = action.parameters.as_ref();
            let Some(service) = parameters
                .and_then(|parameters| parameters.get(PARAMETER_SERVICE))
                .map(String::as_str)
                .map(str::trim)
                .filter(|service| !service.is_empty())
            else {
                // The action names no service, so there is no API for the fault to be
                // about: nothing is published rather than a fault aimed at every service.
                continue;
            };
            let operations = named_operations(
                parameters.and_then(|parameters| parameters.get(PARAMETER_OPERATIONS)),
            );
            if operations.is_empty() {
                continue;
            }
            // The `Roles` target's resolution, by the same reading every other action's
            // resources go through: the roles the client was told the action would act on.
            let mut principals = action_target_arns(
                action.targets.as_ref(),
                &self.targets,
                &self.resolved_targets,
                RESOURCE_TYPE_IAM_ROLE,
            );
            principals.truncate(MAX_LEASE_PRINCIPALS);
            if principals.is_empty() {
                // No role resolved, so the fault affects nobody: a target that identified
                // no resource is not a licence to fail every caller's calls.
                continue;
            }
            let Some(percentage) = named_percentage(
                parameters.and_then(|parameters| parameters.get(PARAMETER_PERCENTAGE)),
            ) else {
                // The action states a share this emulator cannot read as one, so WHICH
                // calls it affects is not known: nothing is published rather than a fault
                // widened to every call of the targeted roles.
                continue;
            };
            leases.push(ApiFaultLease {
                experiment_id: self.id.clone(),
                action_name: name.clone(),
                service: service.to_string(),
                operations,
                principals,
                percentage,
                fault,
                from: run.start_due,
                until: run
                    .start_due
                    .checked_add(fault_span(
                        parameters.and_then(|parameters| {
                            parameters.get(super::DURATION_PARAMETER)
                        }),
                    ))
                    .unwrap_or(run.end_due)
                    .max(run.end_due),
            });
        }
        leases
    }

    /// Whether any action of this experiment injects an API fault, so the registry has
    /// something to be told about it — including that it has nothing live any more.
    #[must_use]
    pub fn injects_api_faults(&self) -> bool {
        self.actions.values().any(|action| {
            action
                .action_id
                .as_deref()
                .is_some_and(injects_an_api_fault)
        })
    }
}

impl super::ComAmazonawsFisFaultinjectionsimulatorState {
    /// Every experiment that injects an API fault, with the leases it has LIVE right now.
    ///
    /// Call it on a SETTLED store
    /// ([`settle_experiments`](super::ComAmazonawsFisFaultinjectionsimulatorState::settle_experiments)):
    /// the settle is what starts and ends an action, and the live set follows from where the
    /// run stands. Then publish what it hands back with this service's guard released
    /// ([`publish_api_faults`]).
    ///
    /// An experiment that injects a fault is listed even when its live set is EMPTY: that
    /// empty set is what withdraws the fault of an action the run has finished, stopped or
    /// cancelled, so a withdrawal needs no record of its own.
    ///
    /// The walk is over the whole experiment map, bounded by
    /// [`MAX_EXPERIMENTS`](super::MAX_EXPERIMENTS), exactly as every other whole-store
    /// settle of this service is.
    pub fn live_api_fault_leases(&self) -> Vec<(String, Vec<ApiFaultLease>)> {
        self.experiments
            .values()
            .filter(|record| record.injects_api_faults())
            .map(|record| (record.id.clone(), record.live_api_fault_leases()))
            .collect()
    }
}

/// How long the fault an action injects LASTS: the span its `duration` parameter asks for,
/// `Duration::ZERO` for an action that asks for none.
///
/// The duration is the CALLER's own value — "duration: The duration of the action, in ISO
/// 8601 format", read through [`super::iso8601_span`], the one reading of such a value in
/// this emulator ([`super::action_span`] is the same read on an action held as a template's)
/// — so an action asked to fail a role's calls for two minutes fails them for two minutes.
/// That is what a client measures the fault by: it is observed by making the calls, not by
/// reading a status.
///
/// Clamped to [`MAX_FAULT_SPAN`], and never shorter than the action's own run window (the
/// caller of this takes the longer of the two), so a fault is live for at least as long as
/// the run reports the action running.
fn fault_span(duration: Option<&String>) -> Duration {
    duration
        .and_then(|duration| super::iso8601_span(duration))
        .unwrap_or(Duration::ZERO)
        .min(MAX_FAULT_SPAN)
}

/// The operations an `operations` parameter names: the comma-separated list it carries, with
/// the blanks around each name dropped and an empty name left out, at most
/// [`MAX_LEASE_OPERATIONS`] of them.
fn named_operations(value: Option<&String>) -> Vec<String> {
    value
        .map(|value| {
            value
                .split(OPERATIONS_SEPARATOR)
                .map(str::trim)
                .filter(|operation| !operation.is_empty())
                .take(MAX_LEASE_OPERATIONS)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The share of calls a `percentage` parameter names, or `None` for a value that is no share
/// at all.
///
/// The parameter is OPTIONAL ("The percentage of calls to affect", not required), and an
/// action that leaves it out affects every call it names ([`EVERY_CALL`]) — the only share
/// this emulator can apply where it was told none.
///
/// A value that is PRESENT and is not a share — text that is no number, or a number outside
/// 0..=100 — is answered `None`, so no lease is published for it: an unreadable share says
/// nothing about which calls are affected, and reading it as the whole share would widen the
/// fault to every call of the targeted roles on the strength of a value this emulator could
/// not read.
fn named_percentage(value: Option<&String>) -> Option<u8> {
    let Some(stated) = value.map(|value| value.trim()).filter(|value| !value.is_empty()) else {
        return Some(EVERY_CALL);
    };
    stated
        .parse::<u8>()
        .ok()
        .filter(|percentage| *percentage <= EVERY_CALL)
}

/// Publish what `leases` say about `experiment_id` into the shared registry on `engine`, and
/// forget what has elapsed.
///
/// Called with this service's own store guard RELEASED, exactly as an action's other effects
/// are carried out ([`super::carry_out_action_effects`]): the registry is a different store
/// in the same engine, and no reader of it holds this service's guard.
///
/// An engine carrying no registry cannot happen — the registry is a type of the shared crate
/// and the engine makes one on first use — but an engine that is not the shared one has none,
/// and then there is nowhere for the fault to be published to.
pub fn publish_api_faults<Eng: Send + Sync + 'static>(
    engine: &Eng,
    experiment_id: &str,
    leases: Vec<ApiFaultLease>,
    now: SystemTime,
) {
    let Some(registry) = injected_api_faults(engine) else {
        ::tracing::warn!(
            %experiment_id,
            "no injected-fault registry is mounted on this engine, so the experiment's API \
             faults reach nothing"
        );
        return;
    };
    let mut registry = registry.lock();
    registry.publish(experiment_id, leases);
    registry.forget_elapsed(now);
}

/// The emulator's injected-fault registry in the shared engine.
///
/// A SHARED type ([`::service_common::injected_api_faults::InjectedApiFaults`]) and not a
/// field of this service's own state: the service that runs the experiment and the service
/// whose API the experiment's fault affects have to reach ONE registry rather than two copies
/// of it.
pub fn injected_api_faults<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<InjectedApiFaults>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<InjectedApiFaults>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One action id put to the fault table: the failure its class injects, or none.
    struct Class {
        case: &'static str,
        action_id: &'static str,
        /// The error code and HTTP status the class answers, or `None` for an action that
        /// injects no API fault at all.
        failure: Option<(&'static str, u16)>,
    }

    /// RULE: each API-fault action class answers its own error code and status, and an
    /// action outside the family injects nothing.
    #[test]
    fn each_fault_class_states_its_own_failure() {
        let rows = [
            Class {
                case: "the internal-error class",
                action_id: ACTION_ID_FIS_INJECT_API_INTERNAL_ERROR,
                failure: Some(("InternalError", 500)),
            },
            Class {
                case: "the throttle class",
                action_id: ACTION_ID_FIS_INJECT_API_THROTTLE_ERROR,
                failure: Some(("RequestLimitExceeded", 503)),
            },
            Class {
                case: "the unavailable class",
                action_id: ACTION_ID_FIS_INJECT_API_UNAVAILABLE_ERROR,
                failure: Some(("Unavailable", 503)),
            },
            Class {
                case: "an action that waits",
                action_id: super::super::ACTION_ID_FIS_WAIT,
                failure: None,
            },
            Class {
                case: "an action that reboots instances",
                action_id: super::super::ACTION_ID_EC2_REBOOT_INSTANCES,
                failure: None,
            },
            Class {
                case: "an action whose code the caller names",
                action_id: "aws:ec2:api-error",
                failure: None,
            },
        ];
        for row in &rows {
            let stated = injected_fault(row.action_id).map(|fault| (fault.code, fault.status));
            let actual = stated
                .as_ref()
                .map(|(code, status)| (code.as_str(), *status));
            assert_eq!(
                actual, row.failure,
                "case {:?}: expected {:?}, got {actual:?}",
                row.case, row.failure
            );
            assert_eq!(
                injects_an_api_fault(row.action_id),
                row.failure.is_some(),
                "case {:?}: expected injects_an_api_fault {}, got the other",
                row.case,
                row.failure.is_some()
            );
        }
    }

    /// RULE: the operations affected are the comma-separated names the parameter carries,
    /// blanks dropped, and no more than the ceiling.
    #[test]
    fn the_named_operations_are_the_ones_the_parameter_lists() {
        let long = (0..MAX_LEASE_OPERATIONS * 2)
            .map(|index| format!("Describe{index}"))
            .collect::<Vec<String>>()
            .join(",");
        // (case, the parameter value, the operations affected)
        let rows: &[(&str, Option<String>, Vec<&str>)] = &[
            (
                "one operation",
                Some("DescribeAvailabilityZones".to_string()),
                vec!["DescribeAvailabilityZones"],
            ),
            (
                "several operations",
                Some("DescribeAvailabilityZones,DescribeRegions".to_string()),
                vec!["DescribeAvailabilityZones", "DescribeRegions"],
            ),
            (
                "a list written with blanks",
                Some(" DescribeAvailabilityZones , DescribeRegions ".to_string()),
                vec!["DescribeAvailabilityZones", "DescribeRegions"],
            ),
            (
                "a list carrying an empty name",
                Some("DescribeRegions,,".to_string()),
                vec!["DescribeRegions"],
            ),
            ("no parameter at all", None, Vec::new()),
            ("an empty parameter", Some(String::new()), Vec::new()),
        ];
        for (case, value, expected) in rows {
            let named = named_operations(value.as_ref());
            assert_eq!(
                named, *expected,
                "case {case:?}: expected {expected:?}, got {named:?}"
            );
        }
        let capped = named_operations(Some(&long));
        assert_eq!(
            capped.len(),
            MAX_LEASE_OPERATIONS,
            "case \"more operations than the ceiling\": expected {MAX_LEASE_OPERATIONS}, got {}",
            capped.len()
        );
    }

    /// RULE: a fault lasts the duration its action asked for, never less than nothing and
    /// never past the longest span this service accepts for one.
    #[test]
    fn the_fault_lasts_the_duration_the_action_asked_for() {
        // (case, the parameter value, the span the fault lasts)
        let rows: &[(&str, Option<&str>, Duration)] = &[
            ("two minutes", Some("PT2M"), Duration::from_secs(120)),
            ("one minute", Some("PT1M"), Duration::from_secs(60)),
            ("an hour", Some("PT1H"), Duration::from_secs(3_600)),
            ("thirty seconds", Some("PT30S"), Duration::from_secs(30)),
            ("no duration at all", None, Duration::ZERO),
            ("a value that is no duration", Some("a while"), Duration::ZERO),
            (
                "a span whose length is not fixed",
                Some("P1Y"),
                Duration::ZERO,
            ),
            ("the longest span accepted", Some("PT720M"), MAX_FAULT_SPAN),
            ("a span past the longest accepted", Some("P30D"), MAX_FAULT_SPAN),
        ];
        for (case, value, expected) in rows {
            let span = fault_span(value.map(str::to_string).as_ref());
            assert_eq!(
                span, *expected,
                "case {case:?}: expected {expected:?}, got {span:?}"
            );
        }
    }

    /// RULE: the share affected is the `percentage` the action named, every call where it
    /// named none, and no share at all — so no lease — for a value that is no share.
    #[test]
    fn the_named_percentage_is_the_share_affected() {
        // (case, the parameter value, the share affected)
        let rows: &[(&str, Option<&str>, Option<u8>)] = &[
            ("the whole share", Some("100"), Some(100)),
            ("a tenth", Some("10"), Some(10)),
            ("none of the calls", Some("0"), Some(0)),
            ("a share written with blanks", Some(" 25 "), Some(25)),
            ("no parameter at all", None, Some(EVERY_CALL)),
            ("an empty parameter", Some(""), Some(EVERY_CALL)),
            ("a value that is no percentage", Some("most"), None),
            ("a value past the whole share", Some("400"), None),
            ("a value past what a share could be", Some("4000"), None),
            ("a negative value", Some("-1"), None),
            ("a fractional value", Some("12.5"), None),
        ];
        for (case, value, expected) in rows {
            let share = named_percentage(value.map(str::to_string).as_ref());
            assert_eq!(
                share, *expected,
                "case {case:?}: expected {expected:?}, got {share:?}"
            );
        }
    }
}
