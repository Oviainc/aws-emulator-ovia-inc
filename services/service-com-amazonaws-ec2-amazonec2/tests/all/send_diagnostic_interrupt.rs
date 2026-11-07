// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `SendDiagnosticInterrupt`: what a sent interrupt records, which
//! instances one can be sent to, and what the operation refuses.
//!
//! The transport is the crate's shared [`super::ec2_query`] harness. Every assertion is
//! driven from a TABLE, because each rule under test holds over a CLASS of inputs — every
//! instance state, every id this service cannot resolve, every request a probe can carry —
//! and one request is a single row of it.
//!
//! The rules under test are:
//!
//! 1. one interrupt is DELIVERED per call and recorded on the instance it was sent to, and
//!    the call answers the envelope's success flag; a second call is a second interrupt, so
//!    the operation can be asked any number of times;
//! 2. a sent interrupt MOVES NOTHING: "the operating system crashes and reboots when a
//!    kernel panic or stop error is triggered", which is a crash of the instance's
//!    operating system and no lifecycle move, so the instance stays in the state it was in
//!    with the launch time and the private address it already had;
//! 3. an instance that is up — `running`, or `pending` on its way to it — takes an
//!    interrupt; an instance in any other state has no operating system running to crash,
//!    so it is refused `IncorrectInstanceState` and nothing is recorded for it;
//! 4. an instance id this service cannot resolve is refused — malformed before unknown —
//!    and records nothing, for that instance or any other;
//! 5. a request that names no instance at all is refused before the handler runs;
//! 6. `DryRun` asks "whether you have the required permissions for the operation, without
//!    actually making the request", so it is answered `DryRunOperation` before the store is
//!    read — whatever the instance the request names, and whether or not that id resolves —
//!    and records nothing.
//!
//! What EC2's envelope puts AROUND the answer — the `<return>` flag, the request id and the
//! media type — is one rule for the whole service and is asserted over every operation in
//! [`super::ec2_query_envelope`]; rule 1 reads the flag back here only because it is this
//! operation's whole output.

use std::time::{Duration, SystemTime};

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, InstanceStateName, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;

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

/// The fields one `SendDiagnosticInterrupt` request carries for `instance_id`, which
/// ec2Query spells as the member's own name.
fn instance_fields(instance_id: &str) -> Vec<(String, String)> {
    vec![p("InstanceId", instance_id)]
}

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

/// What `DescribeInstances` reports about one instance, in the members an interrupt must
/// leave exactly as they were.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Described {
    state: String,
    launch_time: Option<String>,
    private_ip: Option<String>,
}

/// One EC2 emulator over its own engine. The engine is kept so the sent interrupts can be
/// read back out of the store: `SendDiagnosticInterrupt` answers `Unit` and no modelled
/// member of `Instance` reports an interrupt — the instance does not move — so the store is
/// the only place a sent interrupt can be observed.
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
            "case {case:?}: the launched instance must be running before it is interrupted"
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

    /// How many diagnostic interrupts the store holds as sent to one instance.
    fn interrupts(&self, instance_id: &str) -> u32 {
        let store = self.engine.state::<Ec2State>();
        let guard = store.lock();
        guard
            .instances
            .get(instance_id)
            .map_or(0, |instance| instance.diagnostic_interrupts)
    }

    /// When the store says the most recent interrupt of one instance was sent.
    fn last_interrupt_at(&self, instance_id: &str) -> Option<SystemTime> {
        let store = self.engine.state::<Ec2State>();
        let guard = store.lock();
        guard
            .instances
            .get(instance_id)
            .and_then(|instance| instance.last_diagnostic_interrupt_at)
    }

    /// How many interrupts the store holds as sent, across every instance.
    fn interrupts_everywhere(&self) -> u32 {
        let store = self.engine.state::<Ec2State>();
        let guard = store.lock();
        guard
            .instances
            .values()
            .map(|instance| instance.diagnostic_interrupts)
            .sum()
    }
}

/// Rules 1 and 2: one call delivers one interrupt to the instance it names, records it
/// there and nowhere else, and moves nothing.
///
/// The rows differ along what the rule generalises over: how many instances the account
/// holds beside the one the request names, so the interrupt is pinned to the instance the
/// id resolved to rather than to "the instance there is".
#[tokio::test]
async fn one_call_interrupts_the_one_instance_it_names() {
    // (case, how many instances the account holds, which of them the request names)
    let rows: [(&str, usize, usize); 4] = [
        ("the only instance", 1, 0),
        ("the first of two", 2, 0),
        ("the second of two", 2, 1),
        ("the last of three", 3, 2),
    ];

    for (case, held, named) in rows {
        let ec2 = Ec2::new();
        let mut instance_ids = Vec::new();
        for _ in 0..held {
            instance_ids.push(ec2.running_instance(case).await);
        }
        let target = instance_ids[named].clone();

        let mut before = Vec::new();
        for instance_id in &instance_ids {
            before.push(ec2.described(instance_id).await);
        }
        let sent_from = SystemTime::now();

        let answer = ec2
            .call("SendDiagnosticInterrupt", instance_fields(&target))
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

        let sent = ec2.interrupts(&target);
        assert_eq!(
            sent, 1,
            "case {case:?}: expected exactly one interrupt recorded for {target}, got {sent}"
        );
        let at = ec2.last_interrupt_at(&target);
        assert!(
            at.is_some_and(|at| at >= sent_from),
            "case {case:?}: the interrupt sent to {target} must be stamped with the instant \
             it was sent, got {at:?}"
        );
        let everywhere = ec2.interrupts_everywhere();
        assert_eq!(
            everywhere, 1,
            "case {case:?}: the interrupt must be recorded for the instance the request \
             named and for no other, got {everywhere} in all"
        );

        // Rule 2: no instance moved, and nothing about any of them changed.
        for (index, instance_id) in instance_ids.iter().enumerate() {
            let after = ec2.described(instance_id).await;
            assert_eq!(
                after, before[index],
                "case {case:?}: interrupting {target} must leave {instance_id} exactly as it \
                 was, expected {:?}, got {after:?}",
                before[index]
            );
            assert_eq!(
                after.state,
                InstanceStateName::Running.as_str(),
                "case {case:?}: an interrupted instance stays running, got {after:?}"
            );
            if instance_id != &target {
                let other = ec2.interrupts(instance_id);
                assert_eq!(
                    other, 0,
                    "case {case:?}: no interrupt may be recorded for {instance_id}, got {other}"
                );
            }
        }
    }
}

/// Rule 1, on repetition: a second interrupt sent to one instance is a second interrupt,
/// so each one is recorded — the call consumes nothing, so it can be asked any number of
/// times. The rows are the rounds: the count climbs by one each time and the stamp never
/// goes backwards.
#[tokio::test]
async fn every_interrupt_one_instance_is_sent_is_recorded() {
    let ec2 = Ec2::new();
    let case = "three interrupts of one instance";
    let instance_id = ec2.running_instance(case).await;

    let mut previous: Option<SystemTime> = None;
    for round in 1..=3u32 {
        let answer = ec2
            .call("SendDiagnosticInterrupt", instance_fields(&instance_id))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: round {round} expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        let sent = ec2.interrupts(&instance_id);
        assert_eq!(
            sent, round,
            "case {case:?}: after {round} call(s) expected {round} interrupts, got {sent}"
        );
        let at = ec2
            .last_interrupt_at(&instance_id)
            .unwrap_or_else(|| panic!("case {case:?}: round {round} must stamp the interrupt"));
        assert!(
            previous.is_none_or(|earlier| at >= earlier),
            "case {case:?}: round {round} must stamp the interrupt no earlier than the one \
             before it, got {at:?} after {previous:?}"
        );
        previous = Some(at);
        let described = ec2.described(&instance_id).await;
        assert_eq!(
            described.state,
            InstanceStateName::Running.as_str(),
            "case {case:?}: round {round} must leave the instance running, got {described:?}"
        );
    }
}

/// Rule 3: which states an interrupt can be sent from. The table is the WHOLE
/// `InstanceStateName` catalog, so every state the service can hold an instance in is a
/// row: an instance that is up takes the interrupt, and every other state is refused
/// `IncorrectInstanceState` with nothing recorded.
#[tokio::test]
async fn only_an_instance_that_is_up_can_be_interrupted() {
    // (case, the state the instance is held in, the expected status, the interrupts recorded)
    let rows: [(&str, InstanceStateName, u16, u32); 6] = [
        ("a pending instance", InstanceStateName::Pending, 200, 1),
        ("a running instance", InstanceStateName::Running, 200, 1),
        (
            "an instance shutting down",
            InstanceStateName::ShuttingDown,
            400,
            0,
        ),
        (
            "a terminated instance",
            InstanceStateName::Terminated,
            400,
            0,
        ),
        ("a stopping instance", InstanceStateName::Stopping, 400, 0),
        ("a stopped instance", InstanceStateName::Stopped, 400, 0),
    ];

    for (case, state, expected, expected_sent) in rows {
        let ec2 = Ec2::new();
        let instance_id = ec2.running_instance(case).await;
        if state != InstanceStateName::Running {
            ec2.park(case, &instance_id, state);
        }

        let answer = ec2
            .call("SendDiagnosticInterrupt", instance_fields(&instance_id))
            .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );
        let sent = ec2.interrupts(&instance_id);
        assert_eq!(
            sent, expected_sent,
            "case {case:?}: expected {expected_sent} interrupts recorded, got {sent}"
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
        let message = answer.error_message();
        assert_eq!(
            message,
            format!(
                "The instance '{instance_id}' is not in a state from which it can be sent a \
                 diagnostic interrupt."
            ),
            "case {case:?}: the refusal must name the instance and the action its state does \
             not admit, got {message:?}"
        );
        assert!(
            message.contains(&instance_id),
            "case {case:?}: the refusal must name the instance it is about, got {message:?}"
        );
    }
}

/// Rule 4: an instance id this service cannot resolve is refused — malformed before
/// unknown — and records nothing. The rows span the forms a bad id takes: not an id at
/// all, the null long id, a long id outside the form EC2 issues, an id of another resource
/// kind, and a well-formed id of either form naming no instance of the account.
#[tokio::test]
async fn an_instance_id_this_account_cannot_resolve_is_refused() {
    let ec2 = Ec2::new();
    // Launched so the refusal comes from the id in the request and not from an account
    // with nothing in it.
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
            .call("SendDiagnosticInterrupt", instance_fields(instance_id))
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
        let everywhere = ec2.interrupts_everywhere();
        assert_eq!(
            everywhere, 0,
            "case {case:?}: a refused request must record nothing, not even for the instance \
             the account does hold ({running}), got {everywhere}"
        );
    }
}

/// Rule 5: a request that names no instance at all is refused, and records nothing.
///
/// `InstanceId` is `@required` but `clientOptional`, so where that refusal comes from is
/// worth pinning rather than assuming: over ec2Query the boundary validator refuses the
/// request against the model's own `@required` marking, in the service-wide refusal
/// envelope, before the handler runs. The rows are the two ways a request can carry no
/// instance id: the field missing, and the field present but empty.
#[tokio::test]
async fn a_request_that_names_no_instance_is_refused() {
    let ec2 = Ec2::new();
    let running = ec2.running_instance("seed").await;

    // (case, the form fields the request carries)
    let rows: [(&str, Vec<(String, String)>); 3] = [
        ("no InstanceId field at all", Vec::new()),
        ("an empty InstanceId", instance_fields("")),
        (
            "a DryRun with no InstanceId",
            vec![p("DryRun", "false")],
        ),
    ];

    for (case, params) in rows {
        let answer = ec2.call("SendDiagnosticInterrupt", params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            !answer.error_code().is_empty(),
            "case {case:?}: the refusal must state an error code, got {}",
            answer.body
        );
        let sent = ec2.interrupts(&running);
        assert_eq!(
            sent, 0,
            "case {case:?}: a refused request must record nothing, got {sent}"
        );
    }
}

/// Rule 6: `DryRun` asks whether the caller could have made the request, so it is answered
/// `DryRunOperation` before the store is read and records nothing. The rows span what the
/// request can name beside the flag: an instance that would have taken the interrupt, one
/// in a state that cannot, an id that is not an id, and a well-formed id naming nothing —
/// the probe answers the same for all of them, because none of them is a question about
/// the caller's permissions.
#[tokio::test]
async fn a_dry_run_is_refused_and_records_nothing() {
    // (case, the state to hold the launched instance in, the id to name instead of it)
    let rows: [(&str, Option<InstanceStateName>, Option<&str>); 5] = [
        (
            "a dry run naming a running instance",
            Some(InstanceStateName::Running),
            None,
        ),
        (
            "a dry run naming a pending instance",
            Some(InstanceStateName::Pending),
            None,
        ),
        (
            "a dry run naming a stopped instance",
            Some(InstanceStateName::Stopped),
            None,
        ),
        (
            "a dry run naming a malformed id",
            None,
            Some("i-1234567890abcdef0"),
        ),
        (
            "a dry run naming an unknown id",
            None,
            Some("i-0fedcba9876543210"),
        ),
    ];

    for (case, state, other_id) in rows {
        let ec2 = Ec2::new();
        let instance_id = ec2.running_instance(case).await;
        if let Some(state) = state {
            if state != InstanceStateName::Running {
                ec2.park(case, &instance_id, state);
            }
        }
        let named = other_id.unwrap_or(instance_id.as_str());

        let mut params = instance_fields(named);
        params.push(p("DryRun", "true"));
        let answer = ec2.call("SendDiagnosticInterrupt", params).await;
        assert_eq!(
            answer.status, 412,
            "case {case:?}: expected HTTP 412, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {}",
            answer.body
        );
        let everywhere = ec2.interrupts_everywhere();
        assert_eq!(
            everywhere, 0,
            "case {case:?}: a dry run must record nothing, got {everywhere}"
        );
    }
}

/// Rule 6, on the other value of the flag: `DryRun=false` is no probe, so the request is
/// served exactly as one carrying no flag at all. The rows are the two ways to say it.
#[tokio::test]
async fn a_request_that_is_not_a_dry_run_is_served() {
    // (case, the fields to add to the request)
    let rows: [(&str, Vec<(String, String)>); 2] = [
        ("no DryRun field", Vec::new()),
        ("DryRun=false", vec![p("DryRun", "false")]),
    ];

    for (case, extra) in rows {
        let ec2 = Ec2::new();
        let instance_id = ec2.running_instance(case).await;
        let mut params = instance_fields(&instance_id);
        params.extend(extra);

        let answer = ec2.call("SendDiagnosticInterrupt", params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        let sent = ec2.interrupts(&instance_id);
        assert_eq!(
            sent, 1,
            "case {case:?}: expected one interrupt recorded, got {sent}"
        );
    }
}
