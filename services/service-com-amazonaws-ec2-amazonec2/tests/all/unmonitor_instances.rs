// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UnmonitorInstances`: the monitoring move it starts, the
//! instances it can be asked for, and what it refuses.
//!
//! The transport is the crate's shared [`super::ec2_query`] harness, and every monitoring
//! state is read back through `DescribeInstances` rather than out of the store, so what is
//! asserted is what a caller sees. Each rule holds over a CLASS of requests — any batch
//! size, an instance whose monitoring was turned on by its launch or by a later call, an
//! instance that is still coming up — so every assertion is driven from a TABLE whose rows
//! differ along what the rule generalises over; one request is a single row of it.
//!
//! The rules under test are:
//!
//! 1. the call turns detailed monitoring OFF for every distinct instance it names: each
//!    one reports the transient `disabling` at once and reaches `disabled` at the instant
//!    the clock owes it, whether or not anybody calls again, and the answer carries one
//!    entry per distinct instance in the order the request first names it;
//! 2. turning monitoring off moves nothing else about the instance: it stays in the
//!    lifecycle state it was in;
//! 3. an instance whose monitoring is already off with nothing owed is left exactly as it
//!    is, so the call reports the settled state rather than re-entering the transient one;
//! 4. only an instance that is up — `running`, or `pending` on its way to it — can have
//!    monitoring turned off; an instance in any other state is refused
//!    `IncorrectInstanceState`;
//! 5. an instance id this service cannot resolve is refused, malformed before unknown, and
//!    a request naming one changes the monitoring of no instance it names;
//! 6. `DryRun` answers whether the call WOULD have succeeded and changes nothing;
//! 7. a batch above the id ceiling this service takes is refused for the ceiling, while a
//!    batch AT it is read as a batch and resolved id by id.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    invalid_id_message, ComAmazonawsEc2Amazonec2State as Ec2State, InstanceStateName,
    MAX_RESOURCE_IDS, MONITORING_STATE_DISABLED, MONITORING_STATE_DISABLING,
    MONITORING_STATE_ENABLED,
    TRANSITION_DELAY,
};

use super::ec2_query::{self, filters};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here names.
const AMI: &str = "ami-0123456789abcdef0";

/// An instance id of the ISSUED form that no instance of this account has.
const UNKNOWN_INSTANCE: &str = "i-0123456789abcdef0";

/// A value that is not an instance id at all, so there is nothing to look up.
const NOT_AN_INSTANCE_ID: &str = "not-an-instance";

/// Wait until every clock-owed move started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The `InstanceId.N` fields one request carries for `instance_ids`, as ec2Query flattens
/// the list (`UnmonitorInstancesRequest$InstanceIds` is spelled `InstanceId` on the wire).
fn instance_fields(instance_ids: &[String]) -> Vec<(String, String)> {
    instance_ids
        .iter()
        .enumerate()
        .map(|(index, instance_id)| p(&format!("InstanceId.{}", index + 1), instance_id.clone()))
        .collect()
}

/// The id numbers the generated ids below count from — far above the ones this service's
/// mint issues, which count from one, so a generated id can never be an instance the
/// suite launched.
const BEYOND_THE_MINT: u64 = 0xffff_ffff;

/// `count` distinct instance ids of the ISSUED long form that name no instance: `i-0`
/// followed by sixteen hexadecimal digits, counted from [`BEYOND_THE_MINT`] so none of
/// them is the all-zero placeholder or an id this account holds.
fn unresolvable_ids(count: usize) -> Vec<String> {
    (1..=count as u64)
        .map(|index| format!("i-0{:016x}", BEYOND_THE_MINT + index))
        .collect()
}

/// One EC2 emulator over its own engine. The engine is kept so an instance can be held in
/// a state no operation leaves it in long enough to call about.
struct Ec2 {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: ::std::sync::Arc<engine::InMemoryEngine>,
}

impl Ec2 {
    fn new() -> Self {
        let (router, engine) = ec2_query::emulator();
        Self { router, engine }
    }

    async fn call(&self, action: &str, params: Vec<(String, String)>) -> (u16, String) {
        ec2_query::call(&self.router, REGION, action, &params).await
    }

    /// One call that must succeed, as its body.
    async fn ok(&self, case: &str, action: &str, params: Vec<(String, String)>) -> String {
        let (status, body) = self.call(action, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
        );
        body
    }

    /// Launch one instance, with detailed monitoring asked for at launch or not, and
    /// answer its id.
    async fn launch(&self, case: &str, monitoring_at_launch: bool) -> String {
        let mut params = vec![p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")];
        if monitoring_at_launch {
            params.push(p("Monitoring.Enabled", "true"));
        }
        let body = self.ok(case, "RunInstances", params).await;
        ec2_query::element(&body, "instanceId").unwrap_or_else(|| {
            panic!("case {case:?}: RunInstances must report the instance it minted: {body}")
        })
    }

    /// Hold one instance still in `state`.
    ///
    /// The transient lifecycle states cannot be held across a call through the operations
    /// that start them — the clock moves the instance on after [`TRANSITION_DELAY`]
    /// whether or not anybody calls — so the state is written into the store the way the
    /// sibling suites seed the branches no sequence of calls can hold still. The
    /// monitoring of the instance is left exactly as the calls above set it.
    fn park(&self, case: &str, instance_id: &str, state: InstanceStateName) {
        let store = self.engine.state::<Ec2State>();
        let mut guard = store.lock();
        let instance = guard.instances.get_mut(instance_id).unwrap_or_else(|| {
            panic!("case {case:?}: the store must hold the launched instance {instance_id}")
        });
        instance.state = state;
        instance.pending_transition = None;
    }

    /// The detailed-monitoring state `DescribeInstances` reports for one instance, read
    /// through the document so the instance's own `<monitoring><state>` is never taken for
    /// an identically named member of something nested inside it.
    async fn monitoring_state(&self, case: &str, instance_id: &str) -> String {
        let body = self
            .ok(
                case,
                "DescribeInstances",
                vec![p("InstanceId.1", instance_id)],
            )
            .await;
        let parsed = document(case, &body);
        let instance = parsed
            .descendants()
            .find(|node| {
                node.is_element()
                    && child_text(*node, "instanceId").as_deref() == Some(instance_id)
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: DescribeInstances must report {instance_id}: {body}")
            });
        child(instance, "monitoring")
            .and_then(|monitoring| child_text(monitoring, "state"))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the report of {instance_id} must carry its monitoring \
                     state: {body}"
                )
            })
    }

    /// The lifecycle state `DescribeInstances` reports for one instance.
    async fn instance_state(&self, instance_id: &str) -> String {
        ec2_query::described_instance_state(&self.router, REGION, instance_id).await
    }
}

/// How the row's instances got their detailed monitoring turned ON before the call under
/// test — the two ways a caller can get there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitoringOn {
    /// The launch asked for it.
    AtLaunch,
    /// A later `MonitorInstances` call turned it on, and the clock settled it.
    ByCall,
}

/// One row of the monitoring table: how many instances the call names, how, and in which
/// lifecycle state they are when it is made.
#[derive(Debug)]
struct MonitoringRow {
    case: &'static str,
    /// How many distinct instances the batch names.
    distinct: usize,
    /// How many ids the request carries — above `distinct` when it repeats the first one.
    named: usize,
    on: MonitoringOn,
    /// Call while the instances are still `pending`, instead of letting them come up.
    while_pending: bool,
}

/// Rules 1 and 2: the call turns monitoring off for every distinct instance it names —
/// `disabling` at once and `disabled` at the instant the clock owes it — and moves nothing
/// else about them.
#[tokio::test]
async fn unmonitor_instances_turns_monitoring_off_for_every_distinct_instance_it_names() {
    let rows = [
        MonitoringRow {
            case: "one instance, monitoring asked for at launch",
            distinct: 1,
            named: 1,
            on: MonitoringOn::AtLaunch,
            while_pending: false,
        },
        MonitoringRow {
            case: "two instances, monitoring turned on by a call",
            distinct: 2,
            named: 2,
            on: MonitoringOn::ByCall,
            while_pending: false,
        },
        MonitoringRow {
            case: "three instances, the first named twice",
            distinct: 3,
            named: 4,
            on: MonitoringOn::AtLaunch,
            while_pending: false,
        },
        MonitoringRow {
            case: "one instance named three times",
            distinct: 1,
            named: 3,
            on: MonitoringOn::ByCall,
            while_pending: false,
        },
        MonitoringRow {
            case: "an instance still coming up",
            distinct: 1,
            named: 1,
            on: MonitoringOn::AtLaunch,
            while_pending: true,
        },
    ];

    for row in &rows {
        let case = row.case;
        let ec2 = Ec2::new();

        let mut instance_ids = Vec::new();
        for _ in 0..row.distinct {
            instance_ids.push(ec2.launch(case, row.on == MonitoringOn::AtLaunch).await);
        }
        if !row.while_pending {
            past_the_transition().await;
        }
        if row.on == MonitoringOn::ByCall {
            ec2.ok(case, "MonitorInstances", instance_fields(&instance_ids))
                .await;
            past_the_transition().await;
        }
        for instance_id in &instance_ids {
            let state = ec2.monitoring_state(case, instance_id).await;
            assert_eq!(
                state, MONITORING_STATE_ENABLED,
                "case {case:?}: {instance_id} must be monitored before the call, got \
                 {state:?}"
            );
        }
        let mut lifecycle_before = Vec::new();
        for instance_id in &instance_ids {
            lifecycle_before.push(ec2.instance_state(instance_id).await);
        }

        // A row that names more ids than it has instances repeats the first one, which is
        // what the one-entry-per-instance rule is about.
        let mut named = instance_ids.clone();
        while named.len() < row.named {
            named.push(instance_ids[0].clone());
        }

        let body = ec2
            .ok(case, "UnmonitorInstances", instance_fields(&named))
            .await;
        let answered_ids = ec2_query::elements(&body, "instanceId");
        assert_eq!(
            answered_ids, instance_ids,
            "case {case:?}: one entry per distinct instance in the order the request first \
             names it, expected {instance_ids:?}, got {answered_ids:?} in {body}"
        );
        let answered_states = ec2_query::elements(&body, "state");
        assert_eq!(
            answered_states,
            vec![MONITORING_STATE_DISABLING.to_string(); instance_ids.len()],
            "case {case:?}: every entry must report {MONITORING_STATE_DISABLING}, got \
             {answered_states:?} in {body}"
        );

        // The transient state is what a caller reading straight afterwards sees, and the
        // instance has not moved.
        for (index, instance_id) in instance_ids.iter().enumerate() {
            let state = ec2.monitoring_state(case, instance_id).await;
            assert_eq!(
                state, MONITORING_STATE_DISABLING,
                "case {case:?}: {instance_id} must report {MONITORING_STATE_DISABLING} at \
                 once, got {state:?}"
            );
            let lifecycle = ec2.instance_state(instance_id).await;
            // A row called while the instance is still coming up is the one place the
            // CLOCK may legitimately have moved it on between the two reads, so there the
            // instance must be in the state it was in or in the one that state was owed —
            // never in a state only a lifecycle operation could have put it in.
            let left_alone = if row.while_pending {
                lifecycle == InstanceStateName::Pending.as_str()
                    || lifecycle == InstanceStateName::Running.as_str()
            } else {
                lifecycle == lifecycle_before[index]
            };
            assert!(
                left_alone,
                "case {case:?}: turning monitoring off must leave {instance_id} in the \
                 state it was in ({:?}) or in the one the clock owed it, got {lifecycle:?}",
                lifecycle_before[index]
            );
        }

        // And at its due instant the clock has it off, for a caller that only reads.
        past_the_transition().await;
        for instance_id in &instance_ids {
            let state = ec2.monitoring_state(case, instance_id).await;
            assert_eq!(
                state, MONITORING_STATE_DISABLED,
                "case {case:?}: {instance_id} must reach {MONITORING_STATE_DISABLED}, got \
                 {state:?}"
            );
        }

        // The documented filter reads the same state the report carries.
        let filtered = ec2
            .ok(
                case,
                "DescribeInstances",
                filters(&[("monitoring-state", &[MONITORING_STATE_DISABLED])]),
            )
            .await;
        let selected = ec2_query::elements(&filtered, "instanceId");
        for instance_id in &instance_ids {
            assert!(
                selected.contains(instance_id),
                "case {case:?}: the monitoring-state filter must select {instance_id}, got \
                 {selected:?}"
            );
        }
    }
}

/// How the row's instance reached `disabled` before the call under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlreadyOff {
    /// Nothing ever turned monitoring on for it.
    NeverMonitored,
    /// It was monitored, then unmonitored, and the clock settled that move.
    Settled,
}

/// Rule 3: an instance whose monitoring is already off with nothing owed is left exactly
/// as it is — the call reports the settled state rather than starting the move again — and
/// an instance the move is still owed for keeps reaching `disabled`.
#[tokio::test]
async fn unmonitoring_an_instance_whose_monitoring_is_already_off_leaves_it_as_it_is() {
    let rows = [
        ("an instance nothing ever monitored", AlreadyOff::NeverMonitored),
        ("an instance already unmonitored once", AlreadyOff::Settled),
    ];

    for (case, how) in rows {
        let ec2 = Ec2::new();
        let instance_id = ec2.launch(case, how == AlreadyOff::Settled).await;
        past_the_transition().await;
        if how == AlreadyOff::Settled {
            ec2.ok(
                case,
                "UnmonitorInstances",
                instance_fields(std::slice::from_ref(&instance_id)),
            )
            .await;
            past_the_transition().await;
        }
        let state = ec2.monitoring_state(case, &instance_id).await;
        assert_eq!(
            state, MONITORING_STATE_DISABLED,
            "case {case:?}: {instance_id} must be unmonitored before the call, got {state:?}"
        );

        let body = ec2
            .ok(
                case,
                "UnmonitorInstances",
                instance_fields(std::slice::from_ref(&instance_id)),
            )
            .await;
        let answered = ec2_query::elements(&body, "state");
        assert_eq!(
            answered,
            vec![MONITORING_STATE_DISABLED.to_string()],
            "case {case:?}: a call that has nothing to turn off reports \
             {MONITORING_STATE_DISABLED}, got {answered:?} in {body}"
        );
        let state = ec2.monitoring_state(case, &instance_id).await;
        assert_eq!(
            state, MONITORING_STATE_DISABLED,
            "case {case:?}: {instance_id} must still report {MONITORING_STATE_DISABLED} at \
             once, got {state:?}"
        );
        past_the_transition().await;
        let state = ec2.monitoring_state(case, &instance_id).await;
        assert_eq!(
            state, MONITORING_STATE_DISABLED,
            "case {case:?}: {instance_id} must stay at {MONITORING_STATE_DISABLED}, got \
             {state:?}"
        );
    }
}

/// How one refusal row names the instances it asks about.
#[derive(Debug, Clone, Copy)]
enum Names {
    /// The row's own instance, held in a lifecycle state with nothing running to monitor.
    Parked(InstanceStateName),
    /// A well-formed instance id this account holds no instance for.
    Unknown,
    /// A value that is not an instance id at all.
    Malformed,
    /// The monitored instance, beside an id that resolves to nothing.
    MonitoredBesideUnknown,
    /// The monitored instance, under `DryRun`.
    MonitoredUnderDryRun,
    /// No instance at all.
    NoneAtAll,
    /// `count` distinct ids this account holds no instance for.
    UnresolvableBatch(usize),
}

/// One row of the refusal table: how the request names its instances, and the answer it
/// must be refused with.
#[derive(Debug)]
struct RefusalRow {
    case: &'static str,
    names: Names,
    status: u16,
    code: &'static str,
}

/// Rules 4 to 7: the call acts on every instance it names or on none of them, and every
/// refusal leaves the monitoring of every instance in the store exactly as it was.
#[tokio::test]
async fn unmonitor_instances_refuses_what_it_cannot_act_on_and_changes_nothing() {
    let rows = [
        RefusalRow {
            case: "a stopped instance",
            names: Names::Parked(InstanceStateName::Stopped),
            status: 400,
            code: "IncorrectInstanceState",
        },
        RefusalRow {
            case: "an instance on its way to stopped",
            names: Names::Parked(InstanceStateName::Stopping),
            status: 400,
            code: "IncorrectInstanceState",
        },
        RefusalRow {
            case: "an instance on its way to terminated",
            names: Names::Parked(InstanceStateName::ShuttingDown),
            status: 400,
            code: "IncorrectInstanceState",
        },
        RefusalRow {
            case: "a terminated instance",
            names: Names::Parked(InstanceStateName::Terminated),
            status: 400,
            code: "IncorrectInstanceState",
        },
        RefusalRow {
            case: "an id of no instance",
            names: Names::Unknown,
            status: 400,
            code: "InvalidInstanceID.NotFound",
        },
        RefusalRow {
            case: "an id that is not an instance id at all",
            names: Names::Malformed,
            status: 400,
            code: "InvalidInstanceID.Malformed",
        },
        RefusalRow {
            case: "one resolvable instance beside one that is not",
            names: Names::MonitoredBesideUnknown,
            status: 400,
            code: "InvalidInstanceID.NotFound",
        },
        RefusalRow {
            case: "a request that would have succeeded, under DryRun",
            names: Names::MonitoredUnderDryRun,
            status: 412,
            code: "DryRunOperation",
        },
        RefusalRow {
            // `InstanceIds` is `@required`, so the boundary validator refuses a request
            // that omits the list before the handler runs.
            case: "no instance at all",
            names: Names::NoneAtAll,
            status: 400,
            code: "MalformedInput",
        },
        RefusalRow {
            case: "as many ids as the service takes, none of them an instance",
            names: Names::UnresolvableBatch(MAX_RESOURCE_IDS),
            status: 400,
            code: "InvalidInstanceID.NotFound",
        },
        RefusalRow {
            case: "one id more than the service takes",
            names: Names::UnresolvableBatch(MAX_RESOURCE_IDS + 1),
            status: 400,
            code: "InvalidParameterValue",
        },
    ];

    let ec2 = Ec2::new();

    // One monitored instance every row can name, and one per lifecycle state a row holds
    // an instance in. They are launched and monitored together, so the suite waits out the
    // clock-owed moves once rather than once per row.
    let monitored = ec2.launch("the monitored instance", false).await;
    let mut parked: Vec<(InstanceStateName, String)> = Vec::new();
    for row in &rows {
        if let Names::Parked(state) = row.names {
            parked.push((state, ec2.launch(row.case, false).await));
        }
    }
    let mut every_instance = vec![monitored.clone()];
    every_instance.extend(parked.iter().map(|(_, instance_id)| instance_id.clone()));

    past_the_transition().await;
    ec2.ok(
        "the monitored instances",
        "MonitorInstances",
        instance_fields(&every_instance),
    )
    .await;
    past_the_transition().await;
    for instance_id in &every_instance {
        let state = ec2.monitoring_state("the monitored instances", instance_id).await;
        assert_eq!(
            state, MONITORING_STATE_ENABLED,
            "{instance_id} must be monitored before any refusal, got {state:?}"
        );
    }
    for (state, instance_id) in &parked {
        ec2.park("the parked instances", instance_id, *state);
    }

    for row in &rows {
        let RefusalRow {
            case,
            names,
            status: expected_status,
            code: expected_code,
        } = row;

        // What the refusal must say, built from the request and the store the way the
        // service words each class.
        let (params, expected_message) = match *names {
            Names::Parked(state) => {
                let instance_id = parked
                    .iter()
                    .find(|(parked_state, _)| *parked_state == state)
                    .map(|(_, instance_id)| instance_id.clone())
                    .unwrap_or_else(|| panic!("case {case:?}: the row must have an instance"));
                let message = format!(
                    "The instance '{instance_id}' is in state '{}', which is not a state \
                     from which it can be unmonitored",
                    state.as_str()
                );
                (
                    instance_fields(std::slice::from_ref(&instance_id)),
                    Some(message),
                )
            }
            Names::Unknown => (
                vec![p("InstanceId.1", UNKNOWN_INSTANCE)],
                Some(format!(
                    "The instance ID '{UNKNOWN_INSTANCE}' does not exist"
                )),
            ),
            Names::Malformed => (
                vec![p("InstanceId.1", NOT_AN_INSTANCE_ID)],
                Some(invalid_id_message(NOT_AN_INSTANCE_ID)),
            ),
            Names::MonitoredBesideUnknown => (
                vec![
                    p("InstanceId.1", monitored.clone()),
                    p("InstanceId.2", UNKNOWN_INSTANCE),
                ],
                Some(format!(
                    "The instance ID '{UNKNOWN_INSTANCE}' does not exist"
                )),
            ),
            Names::MonitoredUnderDryRun => (
                vec![p("InstanceId.1", monitored.clone()), p("DryRun", "true")],
                Some("Request would have succeeded, but DryRun flag is set.".to_string()),
            ),
            // The boundary validator words this one, so only the code is this
            // operation's to answer.
            Names::NoneAtAll => (Vec::new(), None),
            Names::UnresolvableBatch(count) => {
                let ids = unresolvable_ids(count);
                let message = if count > MAX_RESOURCE_IDS {
                    format!(
                        "InstanceId carries {count} ids, above the maximum of \
                         {MAX_RESOURCE_IDS}"
                    )
                } else {
                    // A batch at the ceiling is read as a batch: the first id it names is
                    // the one the lookup refuses.
                    format!("The instance ID '{}' does not exist", ids[0])
                };
                (instance_fields(&ids), Some(message))
            }
        };

        let mut monitoring_before = Vec::new();
        for instance_id in &every_instance {
            monitoring_before.push(ec2.monitoring_state(case, instance_id).await);
        }

        let (status, body) = ec2.call("UnmonitorInstances", params).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = ec2_query::error_code(&body);
        assert_eq!(
            code, *expected_code,
            "case {case:?}: expected the error code {expected_code}, got {code:?} with \
             body {body}"
        );
        if let Some(expected_message) = &expected_message {
            let message = ec2_query::error_message(&body);
            assert_eq!(
                &message, expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {message:?} \
                 with body {body}"
            );
        }

        for (index, instance_id) in every_instance.iter().enumerate() {
            let state = ec2.monitoring_state(case, instance_id).await;
            assert_eq!(
                state, monitoring_before[index],
                "case {case:?}: a refused call must leave the monitoring of {instance_id} \
                 as it was, expected {:?}, got {state:?}",
                monitoring_before[index]
            );
        }
    }
}
