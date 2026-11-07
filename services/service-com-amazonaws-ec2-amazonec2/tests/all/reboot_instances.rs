// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `RebootInstances`: what a queued reboot records, which instances
//! one can be queued for, and what the operation refuses.
//!
//! The transport is the crate's shared [`super::ec2_query`] harness. Every assertion is
//! driven from a TABLE, because each rule under test holds over a CLASS of inputs — every
//! instance state, every batch size, every id this service cannot resolve — and one
//! recorded request is a single row of it.
//!
//! The rules under test are:
//!
//! 1. a reboot is QUEUED, once for each distinct instance the request names — "this
//!    operation is asynchronous; it only queues a request to reboot the specified
//!    instances" — and the call answers the envelope's success flag;
//! 2. a queued reboot MOVES NOTHING: the instance stays in the state it was in, with the
//!    launch time and the private address it already had, a reboot being no lifecycle
//!    transition;
//! 3. an instance that is up — `running`, or `pending` on its way to it — can be rebooted;
//!    "requests to reboot terminated instances are ignored", so a terminated instance is
//!    skipped rather than refused; an instance in any other state is refused
//!    `IncorrectInstanceState` and nothing is queued for it;
//! 4. an instance id this service cannot resolve is refused — malformed before unknown —
//!    and a request naming one queues nothing for any instance it names;
//! 5. a request that names no instance to reboot is refused before the handler runs, and a
//!    batch above the id ceiling is refused by the handler — the one rule about the request
//!    the model cannot express;
//! 6. `DryRun` answers whether the call WOULD have succeeded and queues nothing.
//!
//! What EC2's envelope puts AROUND the answer — the `<return>` flag, the request id and
//! the media type — is one rule for the whole service and is asserted over every
//! operation in [`super::ec2_query_envelope`]; rule 1 reads the flag back here only
//! because it is this operation's whole output.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_ec2_amazonec2 as ec2;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, InstanceStateName, MAX_RESOURCE_IDS,
    TRANSITION_DELAY,
};

use super::ec2_query;

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here names.
const AMI: &str = "ami-0123456789abcdef0";

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The `InstanceId.N` fields one request carries for `instance_ids`, as ec2Query flattens
/// the list (`RebootInstancesRequest$InstanceIds` is spelled `InstanceId` on the wire).
fn instance_fields(instance_ids: &[String]) -> Vec<(String, String)> {
    instance_ids
        .iter()
        .enumerate()
        .map(|(index, instance_id)| p(&format!("InstanceId.{}", index + 1), instance_id.clone()))
        .collect()
}

/// One row of the parameter table: the case, the form fields the request carries, and the
/// `<Code>` it is refused with.
type Row = (&'static str, Vec<(String, String)>, &'static str);

/// One response, kept as text so each assertion can quote it.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    fn error_code(&self) -> String {
        ec2_query::error_code(&self.body)
    }

    fn error_message(&self) -> String {
        ec2_query::error_message(&self.body)
    }

    /// The envelope's `<return>` flag, as this operation's whole output.
    fn returned(&self) -> Option<String> {
        ec2_query::member(&self.body, "return")
    }
}

/// What `DescribeInstances` reports about one instance, in the members a reboot must
/// leave exactly as they were.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Described {
    state: String,
    launch_time: Option<String>,
    private_ip: Option<String>,
}

/// One EC2 emulator over its own engine. The engine is kept so the queued reboots can be
/// read back out of the store: `RebootInstances` answers `Unit` and no modelled member of
/// `Instance` reports a reboot — the instance does not move — so the store is the only
/// place a queued reboot can be observed.
struct Ec2 {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: ::std::sync::Arc<engine::InMemoryEngine>,
}

impl Ec2 {
    fn new() -> Self {
        let (router, engine) = ec2_query::emulator();
        Self { router, engine }
    }

    async fn call(&self, action: &str, params: Vec<(String, String)>) -> Answer {
        let (status, body) = ec2_query::call(&self.router, REGION, action, &params).await;
        Answer { status, body }
    }

    /// Launch one instance and answer its id.
    async fn launch(&self, case: &str) -> String {
        let answer = self
            .call(
                "RunInstances",
                vec![p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: RunInstances must succeed, got {} with body {}",
            answer.status, answer.body
        );
        ec2_query::element(&answer.body, "instanceId").unwrap_or_else(|| {
            panic!(
                "case {case:?}: RunInstances must report the instance it minted: {}",
                answer.body
            )
        })
    }

    /// One instance in the `running` state, reached the way a caller reaches it: launch,
    /// then let the clock-owed `pending` -> `running` move fall due.
    async fn running_instance(&self, case: &str) -> String {
        let instance_id = self.launch(case).await;
        past_the_transition().await;
        assert_eq!(
            self.described(&instance_id).await.state,
            InstanceStateName::Running.as_str(),
            "case {case:?}: the launched instance must be running before it is rebooted"
        );
        instance_id
    }

    /// Hold one instance still in `state`.
    ///
    /// The three TRANSIENT states (`pending`, `stopping`, `shutting-down`) cannot be held
    /// across a call through the operations that start them, since the clock moves the
    /// instance on after [`TRANSITION_DELAY`] whether or not anybody calls — so the state
    /// is written into the store the way the sibling suites seed the branches no operation
    /// can reach.
    fn park(&self, case: &str, instance_id: &str, state: InstanceStateName) {
        let store = self.engine.state::<Ec2State>();
        let mut guard = store.lock();
        let instance = guard.instances.get_mut(instance_id).unwrap_or_else(|| {
            panic!("case {case:?}: the store must hold the launched instance {instance_id}")
        });
        instance.state = state;
        instance.pending_transition = None;
    }

    /// What `DescribeInstances` reports about one instance.
    async fn described(&self, instance_id: &str) -> Described {
        let answer = self
            .call("DescribeInstances", vec![p("InstanceId.1", instance_id)])
            .await;
        assert_eq!(
            answer.status, 200,
            "DescribeInstances for {instance_id} must succeed, got {} with body {}",
            answer.status, answer.body
        );
        Described {
            state: ec2_query::described_instance_state(&self.router, REGION, instance_id).await,
            launch_time: ec2_query::element(&answer.body, "launchTime"),
            private_ip: ec2_query::element(&answer.body, "privateIpAddress"),
        }
    }

    /// How many reboots the store holds as queued for one instance.
    fn queued_reboots(&self, instance_id: &str) -> u32 {
        let store = self.engine.state::<Ec2State>();
        let guard = store.lock();
        guard
            .instances
            .get(instance_id)
            .map_or(0, |instance| instance.queued_reboots)
    }

    /// When the store says the most recent reboot of one instance was queued.
    fn last_reboot_queued_at(&self, instance_id: &str) -> Option<SystemTime> {
        let store = self.engine.state::<Ec2State>();
        let guard = store.lock();
        guard
            .instances
            .get(instance_id)
            .and_then(|instance| instance.last_reboot_queued_at)
    }

    /// How many reboots the store holds as queued, across every instance.
    fn queued_reboots_everywhere(&self) -> u32 {
        let store = self.engine.state::<Ec2State>();
        let guard = store.lock();
        guard
            .instances
            .values()
            .map(|instance| instance.queued_reboots)
            .sum()
    }
}

/// Rules 1 and 2: a reboot is queued once for every distinct instance the request names,
/// and moves none of them.
///
/// The rows differ along what the rule generalises over: how many instances one call names,
/// and whether it names one of them more than once.
#[tokio::test]
async fn a_reboot_is_queued_once_for_every_distinct_instance_it_names() {
    // (case, how many ids the request names, how many of them are distinct)
    let rows: [(&str, usize, usize); 4] = [
        ("one instance", 1, 1),
        ("two instances", 2, 2),
        ("three instances", 3, 3),
        ("one instance named three times", 3, 1),
    ];

    for (case, named, distinct) in rows {
        let ec2 = Ec2::new();
        let mut instance_ids = Vec::new();
        for _ in 0..distinct {
            instance_ids.push(ec2.running_instance(case).await);
        }
        // A row that names fewer distinct instances than it names ids repeats the first
        // one, which is what the deduplication rule is about.
        let named_ids: Vec<String> = (0..named)
            .map(|index| instance_ids[index.min(distinct - 1)].clone())
            .collect();

        let mut before = Vec::new();
        for instance_id in &instance_ids {
            before.push(ec2.described(instance_id).await);
        }
        let queued_from = SystemTime::now();

        let answer = ec2
            .call("RebootInstances", instance_fields(&named_ids))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.returned().as_deref(),
            Some("true"),
            "case {case:?}: expected <return>true</return>, got {}",
            answer.body
        );
        assert!(
            ec2_query::member(&answer.body, "requestId")
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: the envelope must state a request id, got {}",
            answer.body
        );

        for (index, instance_id) in instance_ids.iter().enumerate() {
            let queued = ec2.queued_reboots(instance_id);
            assert_eq!(
                queued, 1,
                "case {case:?}: expected exactly one queued reboot for {instance_id}, \
                 got {queued}"
            );
            let at = ec2.last_reboot_queued_at(instance_id);
            assert!(
                at.is_some_and(|at| at >= queued_from),
                "case {case:?}: the queued reboot of {instance_id} must be stamped with \
                 the instant it was queued, got {at:?}"
            );
            // Rule 2: the instance did not move, and nothing about it changed.
            let after = ec2.described(instance_id).await;
            assert_eq!(
                after, before[index],
                "case {case:?}: rebooting {instance_id} must leave it exactly as it was, \
                 expected {:?}, got {after:?}",
                before[index]
            );
            assert_eq!(
                after.state,
                InstanceStateName::Running.as_str(),
                "case {case:?}: a rebooted instance stays running, got {after:?}"
            );
        }
        let everywhere = ec2.queued_reboots_everywhere();
        assert_eq!(
            everywhere,
            u32::try_from(distinct).expect("the row names a handful of instances"),
            "case {case:?}: expected {distinct} queued reboots in all, got {everywhere}"
        );
    }
}

/// Rule 1, on repetition: a second request to reboot one instance is a second reboot, so
/// each one is queued — the operation queues a request and consumes nothing, so it can be
/// asked any number of times.
#[tokio::test]
async fn every_reboot_one_instance_is_asked_for_is_queued() {
    let ec2 = Ec2::new();
    let case = "three reboots of one instance";
    let instance_id = ec2.running_instance(case).await;

    let mut previous: Option<SystemTime> = None;
    for round in 1..=3u32 {
        let answer = ec2
            .call(
                "RebootInstances",
                instance_fields(std::slice::from_ref(&instance_id)),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: reboot {round} must succeed, got {} with body {}",
            answer.status, answer.body
        );
        let queued = ec2.queued_reboots(&instance_id);
        assert_eq!(
            queued, round,
            "case {case:?}: after {round} reboots the store must hold {round} queued, \
             got {queued}"
        );
        let at = ec2
            .last_reboot_queued_at(&instance_id)
            .unwrap_or_else(|| panic!("case {case:?}: reboot {round} must be stamped"));
        assert!(
            previous.is_none_or(|earlier| at >= earlier),
            "case {case:?}: reboot {round} must be stamped no earlier than the one \
             before it, got {at:?} after {previous:?}"
        );
        previous = Some(at);
    }
}

/// Rule 3: which states a reboot can be queued from. The table is the WHOLE
/// `InstanceStateName` catalog, so every state the service can hold an instance in is a
/// row: an instance that is up is rebooted, a terminated one is IGNORED — the request
/// still succeeds and queues nothing for it — and every other state is refused
/// `IncorrectInstanceState` with nothing queued.
#[tokio::test]
async fn only_an_instance_that_is_up_can_be_rebooted() {
    // (case, the state the instance is held in, the expected status, the reboots queued)
    let rows: [(&str, InstanceStateName, u16, u32); 6] = [
        ("a pending instance", InstanceStateName::Pending, 200, 1),
        ("a running instance", InstanceStateName::Running, 200, 1),
        (
            "a terminated instance, whose reboot is ignored",
            InstanceStateName::Terminated,
            200,
            0,
        ),
        (
            "an instance shutting down",
            InstanceStateName::ShuttingDown,
            400,
            0,
        ),
        ("a stopping instance", InstanceStateName::Stopping, 400, 0),
        ("a stopped instance", InstanceStateName::Stopped, 400, 0),
    ];

    for (case, state, expected, expected_queued) in rows {
        let ec2 = Ec2::new();
        let instance_id = ec2.running_instance(case).await;
        if state != InstanceStateName::Running {
            ec2.park(case, &instance_id, state);
        }

        let answer = ec2
            .call(
                "RebootInstances",
                instance_fields(std::slice::from_ref(&instance_id)),
            )
            .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );
        let queued = ec2.queued_reboots(&instance_id);
        assert_eq!(
            queued, expected_queued,
            "case {case:?}: expected {expected_queued} queued reboots, got {queued}"
        );
        // Whatever the answer, the call is no lifecycle move: the instance is still in the
        // state the row put it in.
        let described = ec2.described(&instance_id).await;
        assert_eq!(
            described.state,
            state.as_str(),
            "case {case:?}: the call must leave the instance in {}, got {described:?}",
            state.as_str()
        );
        if expected == 200 {
            continue;
        }
        assert_eq!(
            answer.error_code(),
            "IncorrectInstanceState",
            "case {case:?}: expected IncorrectInstanceState, got {}",
            answer.body
        );
        assert_eq!(
            answer.error_message(),
            format!("The instance '{instance_id}' is not in a state from which it can be rebooted."),
            "case {case:?}: the refusal must name the instance and the action its state \
             does not admit, got {:?}",
            answer.error_message()
        );
    }
}

/// Rule 3, on a BATCH: a terminated instance named beside one that is up is ignored while
/// the other is rebooted, and an instance in a state the operation refuses takes the whole
/// batch down with it — the batch is checked before the first reboot is queued.
#[tokio::test]
async fn a_batch_ignores_a_terminated_instance_and_is_refused_for_a_bad_state() {
    // (case, the state the second instance is held in, the expected status, the reboots
    //  queued for the FIRST instance — the one that is up)
    let rows: [(&str, InstanceStateName, u16, u32); 2] = [
        (
            "a terminated instance beside a running one",
            InstanceStateName::Terminated,
            200,
            1,
        ),
        (
            "a stopped instance beside a running one",
            InstanceStateName::Stopped,
            400,
            0,
        ),
    ];

    for (case, state, expected, expected_queued) in rows {
        let ec2 = Ec2::new();
        let running = ec2.running_instance(case).await;
        let other = ec2.running_instance(case).await;
        ec2.park(case, &other, state);

        let answer = ec2
            .call(
                "RebootInstances",
                instance_fields(&[running.clone(), other.clone()]),
            )
            .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );
        let queued = ec2.queued_reboots(&running);
        assert_eq!(
            queued, expected_queued,
            "case {case:?}: expected {expected_queued} queued reboots for the running \
             instance, got {queued}"
        );
        let skipped = ec2.queued_reboots(&other);
        assert_eq!(
            skipped, 0,
            "case {case:?}: no reboot may be queued for the instance in {}, got {skipped}",
            state.as_str()
        );
    }
}

/// Rule 4: an instance id this service cannot resolve is refused — malformed before
/// unknown — and the refusal is the answer however else the request is formed. The rows
/// span the forms a bad id takes: not an id at all, the null long id, a long id outside the
/// form EC2 issues, an id of another resource kind, and a well-formed id of either form
/// naming no instance.
#[tokio::test]
async fn an_instance_id_this_account_cannot_resolve_is_refused() {
    let ec2 = Ec2::new();
    // Launched so the refusal comes from the id in the request and not from an account
    // with nothing in it. It is also the SECOND id of each row below, so a refusal for one
    // instance of a batch queues nothing for the instance that was fine.
    let running = ec2.running_instance("seed").await;

    // (case, the id the request names, the expected <Code>)
    let rows: [(&str, &str, &str); 6] = [
        (
            "not an id at all",
            "not-an-instance",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a short-form id nothing was launched under",
            "i-12345678",
            "InvalidInstanceID.NotFound",
        ),
        (
            "a long id whose leading digit is above zero",
            "i-1234567890abcdef0",
            "InvalidInstanceID.Malformed",
        ),
        (
            "the null long id",
            "i-00000000000000000",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a volume id",
            "vol-0123456789abcdef0",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a well-formed id nothing was launched under",
            "i-0fedcba9876543210",
            "InvalidInstanceID.NotFound",
        ),
    ];

    for (case, instance_id, expected) in rows {
        let answer = ec2
            .call(
                "RebootInstances",
                instance_fields(&[instance_id.to_string(), running.clone()]),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected,
            "case {case:?}: expected {expected}, got {}",
            answer.body
        );
        let everywhere = ec2.queued_reboots_everywhere();
        assert_eq!(
            everywhere, 0,
            "case {case:?}: a refused request must queue nothing, not even for the \
             instance it named that does resolve, got {everywhere}"
        );
    }
}

/// Rule 5: a request that names no instance to reboot, and one above the id ceiling, are
/// refused and queue nothing.
///
/// `InstanceIds` is `@required` but `clientOptional`, so where the refusal of a request that
/// names NO instance comes from is worth pinning rather than assuming: over ec2Query the
/// boundary validator refuses it against the model's own `@required` marking, in the
/// service-wide refusal envelope (`MalformedInput`), so the handler's own empty-list guard
/// is a safety net behind it and never the wire answer. The batch size is the handler's
/// own, the model publishing no length for the list, so the other rows are the ceiling at
/// the maximum and one past it.
#[tokio::test]
async fn a_request_outside_the_operations_parameters_is_refused() {
    let ec2 = Ec2::new();
    let running = ec2.running_instance("seed").await;
    // Ids of the form EC2 issues, so a row about the CEILING is refused for how many ids
    // it names rather than for an id inside it.
    let many = |count: usize| -> Vec<(String, String)> {
        let ids: Vec<String> = (0..count).map(|index| format!("i-{index:017x}")).collect();
        instance_fields(&ids)
    };

    // (case, the form fields, the expected <Code>)
    let rows: Vec<Row> = vec![
        (
            // No `InstanceId.N` field at all: the modelled `@required` list is absent, so
            // the boundary refuses the request before the handler runs — never a 200, and
            // never a reboot queued for anybody.
            "no instance named at all",
            Vec::new(),
            "MalformedInput",
        ),
        (
            "one id past the batch ceiling",
            many(MAX_RESOURCE_IDS + 1),
            "InvalidParameterValue",
        ),
        (
            // At the ceiling the batch itself is accepted, so the refusal is about the
            // ids it names rather than about how many there are.
            "the batch ceiling exactly",
            many(MAX_RESOURCE_IDS),
            "InvalidInstanceID.Malformed",
        ),
    ];

    for (case, params, expected) in rows {
        let answer = ec2.call("RebootInstances", params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected,
            "case {case:?}: expected {expected}, got {}",
            answer.body
        );
        let queued = ec2.queued_reboots(&running);
        assert_eq!(
            queued, 0,
            "case {case:?}: a refused request must queue nothing, got {queued}"
        );
    }
}

/// Rule 6: `DryRun` asks whether the call WOULD have succeeded. A request that would have
/// been served is refused `DryRunOperation` and queues nothing; one that would have been
/// refused is refused with ITS reason instead. The rows span both.
#[tokio::test]
async fn a_dry_run_is_refused_and_queues_nothing() {
    // (case, the state the instance is held in, the expected status and code)
    let rows: [(&str, InstanceStateName, u16, &str); 3] = [
        (
            "a dry run that would have been served",
            InstanceStateName::Running,
            412,
            "DryRunOperation",
        ),
        (
            "a dry run that would have been refused for the state",
            InstanceStateName::Stopped,
            400,
            "IncorrectInstanceState",
        ),
        (
            // A reboot of a terminated instance is ignored, so the request WOULD have
            // succeeded — the dry run answers that and not the skipped instance.
            "a dry run naming a terminated instance",
            InstanceStateName::Terminated,
            412,
            "DryRunOperation",
        ),
    ];

    for (case, state, expected_status, expected_code) in rows {
        let ec2 = Ec2::new();
        let instance_id = ec2.running_instance(case).await;
        if state != InstanceStateName::Running {
            ec2.park(case, &instance_id, state);
        }

        let mut params = instance_fields(std::slice::from_ref(&instance_id));
        params.push(p("DryRun", "true"));
        let answer = ec2.call("RebootInstances", params).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected_code,
            "case {case:?}: expected {expected_code}, got {}",
            answer.body
        );
        let queued = ec2.queued_reboots(&instance_id);
        assert_eq!(
            queued, 0,
            "case {case:?}: a dry run must queue nothing, got {queued}"
        );
    }
}

/// Rule 4 again, on an id this service cannot resolve under `DryRun`: the dry run reports
/// the reason the request would have been refused for, never `DryRunOperation`.
#[tokio::test]
async fn a_dry_run_naming_an_unresolvable_instance_reports_that_reason() {
    let ec2 = Ec2::new();
    // (case, the id, the expected code)
    let rows: [(&str, &str, &str); 2] = [
        (
            "a malformed id under DryRun",
            "i-1234567890abcdef0",
            "InvalidInstanceID.Malformed",
        ),
        (
            "an unknown id under DryRun",
            "i-0fedcba9876543210",
            "InvalidInstanceID.NotFound",
        ),
    ];

    for (case, instance_id, expected) in rows {
        let mut params = instance_fields(&[instance_id.to_string()]);
        params.push(p("DryRun", "true"));
        let answer = ec2.call("RebootInstances", params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected,
            "case {case:?}: expected {expected}, got {}",
            answer.body
        );
    }
}
