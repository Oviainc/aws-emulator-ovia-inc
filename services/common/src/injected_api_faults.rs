// SPDX-License-Identifier: CC0-1.0

//! The faults a RUNNING fault-injection experiment imposes on another service's API, held
//! once for the whole emulator.
//!
//! ## The rule this module carries
//!
//! A fault-injection experiment whose action injects an API error does not merely record a
//! status: while that action is running, the API calls it names FAIL. The action states who
//! is affected (the IAM roles its target resolved to), which service's API, which operations
//! of it, what share of the calls, and for how long — so one request is affected exactly when
//! it was made with one of those roles' credentials, names one of those operations of that
//! service, and arrives inside the action's own window. Outside any of those four it is
//! served as it always was: another principal, another operation, another service and an
//! instant past the window are all unaffected.
//!
//! ## Why it lives here
//!
//! The service that RUNS the experiment and the service whose API is affected are two
//! different crates, and the affected one cannot depend on the fault injector (the injector
//! already reads its store to carry an action's other effects out). So the leases live in a
//! type of this shared crate, held in the ONE engine every service reaches
//! (`engine.state::<InjectedApiFaults>()`), exactly as the account's management-event log is
//! ([`crate::management_events::ManagementEventLog`]): the injector is the only writer and
//! each affected service reads the leases that name it.
//!
//! Nothing here reaches the clock, a socket or a file: a lease carries the window its action
//! owns and every question is asked at an instant its caller passes in.

use std::time::SystemTime;

/// The most leases the registry holds at once.
///
/// A lease is published per ACTION of a running experiment, so this bounds the work one
/// request's lookup does whatever a caller has started. Past it a further lease is not
/// published, which leaves the API unaffected rather than making a request pay for an
/// unbounded walk.
pub const MAX_LEASES: usize = 64;

/// The most operations ONE lease affects, out of the ones its action named.
pub const MAX_LEASE_OPERATIONS: usize = 32;

/// The most principals ONE lease affects, out of the ones its action's target resolved to.
pub const MAX_LEASE_PRINCIPALS: usize = 32;

/// The share of calls a lease affects when its action named none: every one of them.
///
/// The `percentage` parameter of an API-error action is optional, and an action that leaves
/// it out affects the calls it names in full — which is also the only share an emulator can
/// state without having been told one.
pub const EVERY_CALL: u8 = 100;

/// The failure one lease imposes on an affected call: the HTTP status, the error code a
/// client branches on, and the message it shows.
///
/// Stated by the injector, because WHICH failure an action injects is that action's own
/// contract, and rendered by the affected service in its own protocol's envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectedFault {
    /// The HTTP status the affected call answers with.
    pub status: u16,
    /// The error code the affected call answers with.
    pub code: String,
    /// The message the affected call answers with.
    pub message: String,
}

/// One running action's injection: who it affects, what it affects, and for how long.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiFaultLease {
    /// The experiment whose action published this lease, which is what a withdrawal
    /// addresses ([`InjectedApiFaults::publish`]).
    pub experiment_id: String,
    /// The action of that experiment, by the name its template gave it.
    pub action_name: String,
    /// The service whose API is affected, as the action named it — compared against the
    /// affected service's own name without regard to ASCII case.
    pub service: String,
    /// The operations of that service the action named, at most
    /// [`MAX_LEASE_OPERATIONS`] of them.
    pub operations: Vec<String>,
    /// Every spelling of the principals affected: the ARNs of the IAM roles the action's
    /// target resolved to, at most [`MAX_LEASE_PRINCIPALS`] of them.
    pub principals: Vec<String>,
    /// The share of the calls it names that are affected, 0..=100.
    pub percentage: u8,
    /// The failure an affected call answers with.
    pub fault: InjectedFault,
    /// When the action starts, from which instant its calls are affected.
    pub from: SystemTime,
    /// When the action ends, from which instant they are not.
    pub until: SystemTime,
}

impl ApiFaultLease {
    /// Whether this lease is about a call of `operation` on `service` made by one of
    /// `principals`, at `now`.
    ///
    /// Every one of the four has to hold: the service, the operation, the principal and the
    /// window. The service and the operation are compared without regard to ASCII case, the
    /// spelling of an API name being the same name however a client cased it; a principal is
    /// compared exactly, an ARN naming one resource.
    #[must_use]
    pub fn covers(
        &self,
        service: &str,
        operation: &str,
        principals: &[String],
        now: SystemTime,
    ) -> bool {
        if now < self.from || now >= self.until {
            return false;
        }
        if !self.service.eq_ignore_ascii_case(service) {
            return false;
        }
        if !self
            .operations
            .iter()
            .any(|named| named.eq_ignore_ascii_case(operation))
        {
            return false;
        }
        self.principals
            .iter()
            .any(|affected| principals.iter().any(|spelling| spelling == affected))
    }

    /// Whether a call this lease covers is affected on the draw `draw`, a value in
    /// `0..`[`EVERY_CALL`] standing for where that call fell in the share.
    ///
    /// The share is the lease's `percentage`: with 100 every covered call is affected, with
    /// 0 none is, and in between exactly the draws below the percentage are. The draw is
    /// made by the caller, one per call, so nothing about which calls are affected is
    /// remembered between them.
    #[must_use]
    pub fn affects(&self, draw: u8) -> bool {
        draw < self.percentage
    }
}

/// Every lease the emulator holds right now.
///
/// Reached through the shared engine (`engine.state::<InjectedApiFaults>()`) so the service
/// that runs the experiment and the service whose API it affects hold ONE registry and not
/// two copies of it.
#[derive(Debug, Default)]
pub struct InjectedApiFaults {
    /// The leases, in the order they were published, at most [`MAX_LEASES`] of them.
    leases: Vec<ApiFaultLease>,
}

impl InjectedApiFaults {
    /// Replace every lease of `experiment_id` with `leases`.
    ///
    /// The injector publishes an experiment's WHOLE live set each time it settles that
    /// experiment, so publishing is how a lease is installed, renewed and — with an empty
    /// set — withdrawn: an action the run has left behind stops affecting anything as the
    /// settle that ended it is reported, rather than waiting its window out.
    ///
    /// Past [`MAX_LEASES`] a further lease is not held, which leaves the API unaffected
    /// rather than growing the walk every request pays for.
    pub fn publish(&mut self, experiment_id: &str, leases: Vec<ApiFaultLease>) {
        self.leases
            .retain(|lease| lease.experiment_id != experiment_id);
        for lease in leases {
            if self.leases.len() >= MAX_LEASES {
                break;
            }
            self.leases.push(lease);
        }
    }

    /// Drop every lease whose window closed before `now`.
    ///
    /// Called by the injector beside [`Self::publish`]: a lease nothing renews — the
    /// experiment that published it having been forgotten — affects no call once its window
    /// is past, and holding it would only cost the next reader a comparison.
    pub fn forget_elapsed(&mut self, now: SystemTime) {
        self.leases.retain(|lease| now < lease.until);
    }

    /// Every lease held, in the order they were published.
    #[must_use]
    pub fn leases(&self) -> &[ApiFaultLease] {
        &self.leases
    }

    /// The fault a call of `operation` on `service` made by one of `principals` is affected
    /// by at `now`, or `None` when no lease covers it or no covering lease affects the draw
    /// `draw`.
    ///
    /// Every covering lease is asked, in the order they were published, and the first one
    /// whose own share the draw falls in decides: two experiments injecting into one
    /// operation are two faults, a call can only be answered with one, and a lease that
    /// affects a SHARE of the calls leaves the calls outside its share to whatever else
    /// covers them rather than shielding them from it.
    #[must_use]
    pub fn fault_for(
        &self,
        service: &str,
        operation: &str,
        principals: &[String],
        now: SystemTime,
        draw: u8,
    ) -> Option<&InjectedFault> {
        self.leases
            .iter()
            .filter(|lease| lease.covers(service, operation, principals, now))
            .find(|lease| lease.affects(draw))
            .map(|lease| &lease.fault)
    }
}

/// A draw for one call: where it falls in the share a lease affects, in
/// `0..`[`EVERY_CALL`].
///
/// Taken per CALL from the thread's own generator, so what a lease affects is a share of the
/// calls rather than a function of anything a request carries, and nothing about which call
/// was affected is stored. At a percentage of 100 every draw is below the share, so a full
/// injection is deterministic.
#[must_use]
pub fn call_draw() -> u8 {
    ::rand::Rng::random_range(&mut ::rand::rng(), 0..EVERY_CALL)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A lease of `percentage` over one operation and one principal, open for a minute from
    /// `from`.
    fn lease(from: SystemTime, percentage: u8) -> ApiFaultLease {
        ApiFaultLease {
            experiment_id: "EXP000000000001".to_string(),
            action_name: "fault".to_string(),
            service: "ec2".to_string(),
            operations: vec!["DescribeAvailabilityZones".to_string()],
            principals: vec!["arn:aws:iam::000000000000:role/target".to_string()],
            percentage,
            fault: InjectedFault {
                status: 500,
                code: "InternalError".to_string(),
                message: "An internal error has occurred".to_string(),
            },
            from,
            until: from + Duration::from_secs(60),
        }
    }

    /// One call put to a lease: who made it, what it named, when it arrived, and whether the
    /// lease is about it.
    struct Call<'a> {
        case: &'a str,
        service: &'a str,
        operation: &'a str,
        principals: &'a [String],
        instant: SystemTime,
        affected: bool,
    }

    /// RULE: a call is affected exactly when the service, the operation, the principal and
    /// the instant are all the lease's — one row per way a call falls outside it.
    #[test]
    fn a_lease_covers_only_the_calls_its_action_names() {
        let from = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let held = lease(from, EVERY_CALL);
        let inside = from + Duration::from_secs(1);
        let targeted = vec![
            "arn:aws:sts::000000000000:assumed-role/target/session".to_string(),
            "arn:aws:iam::000000000000:role/target".to_string(),
        ];
        let other = vec!["arn:aws:iam::000000000000:role/control".to_string()];
        let nobody: Vec<String> = Vec::new();
        let rows = [
            Call {
                case: "the named operation by a targeted role inside the window",
                service: "ec2",
                operation: "DescribeAvailabilityZones",
                principals: &targeted,
                instant: inside,
                affected: true,
            },
            Call {
                case: "the named operation cased as a client wrote it",
                service: "EC2",
                operation: "describeavailabilityzones",
                principals: &targeted,
                instant: inside,
                affected: true,
            },
            Call {
                case: "another operation of the same service",
                service: "ec2",
                operation: "DescribeRegions",
                principals: &targeted,
                instant: inside,
                affected: false,
            },
            Call {
                case: "the named operation of another service",
                service: "ssm",
                operation: "DescribeAvailabilityZones",
                principals: &targeted,
                instant: inside,
                affected: false,
            },
            Call {
                case: "another principal",
                service: "ec2",
                operation: "DescribeAvailabilityZones",
                principals: &other,
                instant: inside,
                affected: false,
            },
            Call {
                case: "a call with no principal at all",
                service: "ec2",
                operation: "DescribeAvailabilityZones",
                principals: &nobody,
                instant: inside,
                affected: false,
            },
            Call {
                case: "an instant before the action starts",
                service: "ec2",
                operation: "DescribeAvailabilityZones",
                principals: &targeted,
                instant: from - Duration::from_secs(1),
                affected: false,
            },
            Call {
                case: "the instant the action starts",
                service: "ec2",
                operation: "DescribeAvailabilityZones",
                principals: &targeted,
                instant: from,
                affected: true,
            },
            Call {
                case: "the instant the action ends",
                service: "ec2",
                operation: "DescribeAvailabilityZones",
                principals: &targeted,
                instant: from + Duration::from_secs(60),
                affected: false,
            },
        ];
        for row in &rows {
            let covered = held.covers(row.service, row.operation, row.principals, row.instant);
            assert_eq!(
                covered, row.affected,
                "case {:?}: expected covers {}, got {covered}",
                row.case, row.affected
            );
        }
    }

    /// RULE: the share a lease affects is its `percentage` — every call at 100, none at 0,
    /// and exactly the draws below it in between.
    #[test]
    fn the_percentage_decides_the_share_of_affected_calls() {
        let from = SystemTime::UNIX_EPOCH;
        // (case, percentage, draw, whether the call is affected)
        let rows: &[(&str, u8, u8, bool)] = &[
            ("the whole share, the lowest draw", EVERY_CALL, 0, true),
            ("the whole share, the highest draw", EVERY_CALL, 99, true),
            ("no share at all", 0, 0, false),
            ("half the share, below it", 50, 49, true),
            ("half the share, at it", 50, 50, false),
            ("half the share, above it", 50, 99, false),
            ("a tenth of the share, below it", 10, 9, true),
            ("a tenth of the share, at it", 10, 10, false),
        ];
        for (case, percentage, draw, expected) in rows {
            let affected = lease(from, *percentage).affects(*draw);
            assert_eq!(
                affected, *expected,
                "case {case:?}: expected affects {expected}, got {affected}"
            );
        }
    }

    /// RULE: publishing an experiment's set REPLACES what it published before, so an empty
    /// set withdraws its faults; another experiment's leases are untouched.
    #[test]
    fn publishing_replaces_one_experiments_leases_and_leaves_the_others() {
        let from = SystemTime::UNIX_EPOCH;
        let now = from + Duration::from_secs(1);
        let targeted = vec!["arn:aws:iam::000000000000:role/target".to_string()];
        let mut registry = InjectedApiFaults::default();
        let mut other = lease(from, EVERY_CALL);
        other.experiment_id = "EXP000000000002".to_string();
        registry.publish("EXP000000000001", vec![lease(from, EVERY_CALL)]);
        registry.publish("EXP000000000002", vec![other]);
        assert_eq!(
            registry.leases().len(),
            2,
            "case \"two experiments injecting\": expected both leases, got {:?}",
            registry.leases()
        );
        registry.publish("EXP000000000001", Vec::new());
        let left: Vec<&str> = registry
            .leases()
            .iter()
            .map(|lease| lease.experiment_id.as_str())
            .collect();
        assert_eq!(
            left,
            vec!["EXP000000000002"],
            "case \"one experiment withdrawn\": expected the other's lease alone, got {left:?}"
        );
        let answered = registry.fault_for(
            "ec2",
            "DescribeAvailabilityZones",
            &targeted,
            now,
            0,
        );
        assert_eq!(
            answered.map(|fault| fault.code.as_str()),
            Some("InternalError"),
            "case \"the remaining lease\": expected it to still affect the call, got \
             {answered:?}"
        );
        registry.publish("EXP000000000002", Vec::new());
        let answered = registry.fault_for(
            "ec2",
            "DescribeAvailabilityZones",
            &targeted,
            now,
            0,
        );
        assert!(
            answered.is_none(),
            "case \"every lease withdrawn\": expected no fault, got {answered:?}"
        );
    }

    /// RULE: every covering lease is asked, so a lease whose share this call falls outside
    /// does not shield the call from another lease that does affect it — two experiments
    /// injecting into one operation are two faults.
    #[test]
    fn a_lease_that_does_not_affect_the_call_does_not_shield_it_from_one_that_does() {
        let from = SystemTime::UNIX_EPOCH;
        let now = from + Duration::from_secs(1);
        let targeted = vec!["arn:aws:iam::000000000000:role/target".to_string()];
        let mut registry = InjectedApiFaults::default();
        // Published FIRST, and affecting none of the calls it covers.
        registry.publish("EXP000000000001", vec![lease(from, 0)]);
        let mut throttle = lease(from, EVERY_CALL);
        throttle.experiment_id = "EXP000000000002".to_string();
        throttle.fault = InjectedFault {
            status: 503,
            code: "RequestLimitExceeded".to_string(),
            message: "Request limit exceeded.".to_string(),
        };
        registry.publish("EXP000000000002", vec![throttle]);
        let answered = registry.fault_for("ec2", "DescribeAvailabilityZones", &targeted, now, 0);
        assert_eq!(
            answered.map(|fault| (fault.status, fault.code.as_str())),
            Some((503, "RequestLimitExceeded")),
            "case \"a share of none published ahead of a whole share\": expected the second \
             experiment's fault, got {answered:?}"
        );
    }

    /// RULE: no more than [`MAX_LEASES`] leases are held, so one caller cannot grow the
    /// walk a request pays for.
    #[test]
    fn the_registry_holds_no_more_than_its_ceiling() {
        let from = SystemTime::UNIX_EPOCH;
        let mut registry = InjectedApiFaults::default();
        let asked = MAX_LEASES * 2;
        registry.publish(
            "EXP000000000001",
            (0..asked).map(|_| lease(from, EVERY_CALL)).collect(),
        );
        assert_eq!(
            registry.leases().len(),
            MAX_LEASES,
            "case \"more leases than the ceiling\": expected {MAX_LEASES}, got {}",
            registry.leases().len()
        );
    }

    /// RULE: a lease whose window has closed affects nothing and is forgotten.
    #[test]
    fn an_elapsed_lease_is_forgotten() {
        let from = SystemTime::UNIX_EPOCH;
        let mut registry = InjectedApiFaults::default();
        registry.publish("EXP000000000001", vec![lease(from, EVERY_CALL)]);
        registry.forget_elapsed(from + Duration::from_secs(30));
        assert_eq!(
            registry.leases().len(),
            1,
            "case \"a window still open\": expected the lease to be held, got {:?}",
            registry.leases()
        );
        registry.forget_elapsed(from + Duration::from_secs(60));
        assert!(
            registry.leases().is_empty(),
            "case \"a window that closed\": expected no lease, got {:?}",
            registry.leases()
        );
    }

    /// RULE: a draw is one of the shares a percentage can cut, so every draw it makes is
    /// below [`EVERY_CALL`] — which is what makes a full injection deterministic.
    #[test]
    fn every_draw_falls_inside_the_share() {
        for attempt in 0..256 {
            let draw = call_draw();
            assert!(
                draw < EVERY_CALL,
                "case \"draw {attempt}\": expected a draw below {EVERY_CALL}, got {draw}"
            );
        }
    }
}
