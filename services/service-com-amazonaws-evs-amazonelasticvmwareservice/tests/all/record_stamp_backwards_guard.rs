// SPDX-License-Identifier: CC0-1.0

//! A mutation whose own call MATERIALISES the transition the clock owed the record it
//! changes, over awsJson1_0.
//!
//! The rule under test: `modifiedAt` is "The date and time that the connector was
//! modified"
//! (`smithy_query(query="doc",shape="Connector",member="modifiedAt",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")`,
//! and the same sentence for `Host`, `Vlan` and `Environment`), so a change is dated at
//! the instant it was APPLIED to the store — not the instant the request arrived — and
//! therefore never before a state change the same response reports. A record this service
//! owes a transition is dated at the instant that transition came DUE, so a call that
//! materialises one and then changes the same record must date its change at or after that
//! instant, however long the call waited to be applied.
//!
//! Each row drives the ordering the rule exists for, and drives it deterministically: the
//! test takes this service's store lock itself, issues the call under test while holding
//! it, keeps it until past the instant the record's transition is due, and only then
//! releases it. The call therefore arrives BEFORE its record's transition is due and is
//! applied AFTER it, which is the window in which an instant read on the request's way in
//! would be earlier than the transition the call itself materialises.
//!
//! What the rule generalises over, and so what the table spans: the KIND of record changed
//! (a connector, an ESX host, the environment itself and the VLANs that go away with it),
//! whether that record is a CHILD of the environment or the environment itself, the
//! DIRECTION of the change (a connector reconfigured, a connector removed, a host removed,
//! a tag written, the environment deleted), and the transition waited through (a
//! connector's and a host's own `CREATING` change, and the environment's lifecycle
//! transition). `tests/all/record_stamp_monotonicity.rs` owns the rule where no transition
//! is in flight at all, and the two Elastic IP writers of it.
//!
//! The second test here is the invariant the first rests on, over every kind of pending
//! change this service records: a transition is waited for on the monotonic clock and
//! dates the record from the wall clock, so reaching the instant it is waited for has to
//! mean the instant it is dated at has passed.
//!
//! Every test drives the service the way a client does, through the same
//! `MultiServiceRouter` the production router mounts, and reads each record back through
//! its own reader.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::environment_lifecycle::{call, emulator, error_type, Emulator, PAST_THE_TRANSITION};

/// The connector type every connector here is created with
/// (`smithy_query(query="shape",shape="ConnectorType")` declares it).
const VCENTER: &str = "VCENTER";

/// The credential secret every connector here is created with.
const SECRET_IDENTIFIER: &str = "evs/vcf-appliance-credentials";

/// The appliance a connector reaches before and after it is reconfigured.
const APPLIANCE: &str = "vcenter.evs.example.com";
const MOVED_APPLIANCE: &str = "vcenter-moved.evs.example.com";

/// The instance type every host here runs on, and the key pair it is launched with.
const INSTANCE_TYPE: &str = "i4i.metal";
const KEY_NAME: &str = "evs-key";

/// The tag the tagging row writes.
const TAG_KEY: &str = "team";
const TAG_VALUE: &str = "platform";

/// How long a row holds this service's store lock while the call under test waits for it:
/// past the instant the record's transition is due
/// ([`evs::state::TRANSITION_DELAY`], the span this service owes a transition in),
/// so the transition is the call's to materialise.
pub(crate) fn held_for() -> Duration {
    evs::state::TRANSITION_DELAY + Duration::from_millis(200)
}

/// Which record of an environment a row changes.
#[derive(Debug, Clone)]
enum Record {
    /// The environment record itself.
    Environment,
    /// The ESX host of that `hostName`.
    Host(String),
    /// The connector of that `connectorId`.
    Connector(String),
}

/// The one call under test of a row.
#[derive(Debug, Clone, Copy)]
enum Change {
    /// `UpdateEnvironmentConnector`: point the connector at another appliance.
    ReconfigureConnector,
    /// `DeleteEnvironmentConnector`: remove the connector the setup added.
    RemoveConnector,
    /// `DeleteEnvironmentHost`: remove the ESX host the setup added.
    RemoveHost,
    /// `TagResource`: write a tag the environment does not carry.
    WriteTag,
    /// `DeleteEnvironment`: delete the environment itself.
    DeleteEnvironment,
}

/// The environment a row runs against, the record its call under test changes, and when
/// the transition that record is waiting on comes due.
struct Subject {
    environment: String,
    environment_arn: String,
    record: Record,
    /// The epoch milliseconds the record's pending transition is due at: the instant it
    /// was created plus the span this service owes a transition in — the instant the
    /// materialisation inside the call under test dates the record at.
    due_at_ms: i64,
}

/// Create the environment a row runs against and the record its call under test changes,
/// leaving that record with its own transition STILL PENDING.
async fn prepare(router: &Emulator, case: &str, index: usize, change: Change) -> Subject {
    let (status, body) = call(
        router,
        "CreateEnvironment",
        &router.create_request(Some(&format!("waited-{index}"))),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the setup create expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let environment = member(case, &body["environment"], "environmentId");
    let environment_arn = member(case, &body["environment"], "environmentArn");
    let environment_created_ms = wire_ms(case, "the environment", &body["environment"]["createdAt"]);

    let (record, due_at_ms) = match change {
        Change::ReconfigureConnector | Change::RemoveConnector => {
            // A connector is added to an environment that is past initialization, so the
            // environment's own transition is waited out first and the only transition
            // left in flight is the connector's.
            tokio::time::sleep(PAST_THE_TRANSITION).await;
            let (status, body) = call(
                router,
                "CreateEnvironmentConnector",
                &json!({
                    "environmentId": environment,
                    "type": VCENTER,
                    "applianceFqdn": APPLIANCE,
                    "secretIdentifier": SECRET_IDENTIFIER,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the setup connector create expected 200, got {status} ({}) {body}",
                error_type(&body)
            );
            let connector_id = member(case, &body["connector"], "connectorId");
            let created_ms = wire_ms(case, "the connector", &body["connector"]["createdAt"]);
            (Record::Connector(connector_id), due_from(created_ms))
        }
        Change::RemoveHost => {
            tokio::time::sleep(PAST_THE_TRANSITION).await;
            // "DNS hostnames for hosts must be unique across Amazon EVS environments"
            // (`HostInfoForCreate$hostName` doc), so each row names its own.
            let host_name = format!("esx-waited-{index}");
            let (status, body) = call(
                router,
                "CreateEnvironmentHost",
                &json!({
                    "environmentId": environment,
                    "host": {
                        "hostName": host_name,
                        "keyName": KEY_NAME,
                        "instanceType": INSTANCE_TYPE,
                    },
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the setup host create expected 200, got {status} ({}) {body}",
                error_type(&body)
            );
            let created_ms = wire_ms(case, "the host", &body["host"]["createdAt"]);
            (Record::Host(host_name), due_from(created_ms))
        }
        // The record under test is the environment, so the transition in flight is its
        // own `CREATING` one and nothing is waited out.
        Change::WriteTag | Change::DeleteEnvironment => (
            Record::Environment,
            due_from(environment_created_ms),
        ),
    };
    Subject {
        environment,
        environment_arn,
        record,
        due_at_ms,
    }
}

/// When the transition a record created at `created_ms` is owed comes due: the span this
/// service owes a transition in, from the instant the record was created
/// ([`evs::state::TRANSITION_DELAY`]).
pub(crate) fn due_from(created_ms: i64) -> i64 {
    created_ms + evs::state::TRANSITION_DELAY.as_millis() as i64
}

/// One required member of a response, as a string.
pub(crate) fn member(case: &str, record: &Value, name: &str) -> String {
    record[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no {name} in {record}"))
        .to_string()
}

/// The row's one call under test, as the operation it targets and the body it sends.
fn request(subject: &Subject, change: Change) -> (&'static str, Value) {
    let environment = subject.environment.clone();
    match change {
        Change::ReconfigureConnector => (
            "UpdateEnvironmentConnector",
            json!({
                "environmentId": environment,
                "connectorId": connector_of(subject),
                "applianceFqdn": MOVED_APPLIANCE,
            }),
        ),
        Change::RemoveConnector => (
            "DeleteEnvironmentConnector",
            json!({
                "environmentId": environment,
                "connectorId": connector_of(subject),
            }),
        ),
        Change::RemoveHost => (
            "DeleteEnvironmentHost",
            json!({ "environmentId": environment, "hostName": host_of(subject) }),
        ),
        Change::WriteTag => (
            "TagResource",
            json!({
                "resourceArn": subject.environment_arn,
                "tags": { TAG_KEY: TAG_VALUE },
            }),
        ),
        Change::DeleteEnvironment => (
            "DeleteEnvironment",
            json!({ "environmentId": environment }),
        ),
    }
}

/// The connector a connector-addressed row names, as its setup minted it.
fn connector_of(subject: &Subject) -> String {
    match &subject.record {
        Record::Connector(connector_id) => connector_id.clone(),
        other => panic!("the row's record is no connector: {other:?}"),
    }
}

/// The host a host-addressed row names, as its setup created it.
fn host_of(subject: &Subject) -> String {
    match &subject.record {
        Record::Host(host_name) => host_name.clone(),
        other => panic!("the row's record is no host: {other:?}"),
    }
}

/// The `modifiedAt` this service's own store holds for the record under test, in epoch
/// milliseconds.
fn stored_stamp(router: &Emulator, case: &str, subject: &Subject) -> i64 {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    let environment = store
        .environments
        .get(&subject.environment)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {}", subject.environment));
    match &subject.record {
        Record::Environment => environment.modified_at_ms,
        Record::Host(host_name) => environment
            .hosts
            .get(host_name)
            .map(|host| host.modified_at_ms)
            .unwrap_or_else(|| panic!("case {case:?}: the environment holds host {host_name}")),
        Record::Connector(connector_id) => environment
            .connectors
            .get(connector_id)
            .map(|connector| connector.modified_at_ms)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the environment holds connector {connector_id}")
            }),
    }
}

/// The `modifiedAt` the store holds for one environment, in epoch milliseconds.
pub(crate) fn environment_stamp(router: &Emulator, case: &str, environment_id: &str) -> i64 {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    store
        .environments
        .get(environment_id)
        .map(|environment| environment.modified_at_ms)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"))
}

/// The earliest `modifiedAt` the store holds for the VLANs of one environment, in epoch
/// milliseconds.
fn earliest_vlan_stamp(router: &Emulator, case: &str, environment_id: &str) -> i64 {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    let environment = store
        .environments
        .get(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"));
    environment
        .vlans
        .iter()
        .map(|vlan| vlan.modified_at_ms)
        .min()
        .unwrap_or_else(|| panic!("case {case:?}: {environment_id} holds VLANs"))
}

/// The response member the call under test reports the record under test through, and the
/// member carrying its state — `TagResource` reports no record at all.
fn reported(change: Change) -> Option<(&'static str, &'static str, &'static str)> {
    match change {
        Change::ReconfigureConnector => Some(("connector", "state", "UPDATING")),
        Change::RemoveConnector => Some(("connector", "state", "DELETING")),
        Change::RemoveHost => Some(("host", "hostState", "DELETING")),
        Change::DeleteEnvironment => Some(("environment", "environmentState", "DELETING")),
        Change::WriteTag => None,
    }
}

/// The `modifiedAt` the record under test's own reader reports, as the wire carries it.
async fn read_back_stamp(router: &Emulator, case: &str, subject: &Subject) -> Value {
    match &subject.record {
        Record::Environment => {
            let (status, body) = call(
                router,
                "GetEnvironment",
                &json!({ "environmentId": subject.environment }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetEnvironment expected 200, got {status} ({}) {body}",
                error_type(&body)
            );
            body["environment"]["modifiedAt"].clone()
        }
        Record::Host(host_name) => {
            listed_stamp(
                router,
                case,
                &subject.environment,
                "ListEnvironmentHosts",
                "environmentHosts",
                "hostName",
                host_name,
            )
            .await
        }
        Record::Connector(connector_id) => {
            listed_stamp(
                router,
                case,
                &subject.environment,
                "ListEnvironmentConnectors",
                "connectors",
                "connectorId",
                connector_id,
            )
            .await
        }
    }
}

/// The `modifiedAt` one lister reports for the item it identifies by `key`.
async fn listed_stamp(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    operation: &str,
    items: &str,
    key: &str,
    wanted: &str,
) -> Value {
    let (status, body) = call(router, operation, &json!({ "environmentId": environment_id })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body[items]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {items} in {body}"))
        .iter()
        .find(|item| item[key] == json!(wanted))
        .map(|item| item["modifiedAt"].clone())
        .unwrap_or_else(|| panic!("case {case:?}: no {key} {wanted} in {body}"))
}

/// A timestamp the wire carries (epoch seconds) in the epoch milliseconds the store holds
/// it as.
pub(crate) fn wire_ms(case: &str, label: &str, value: &Value) -> i64 {
    let seconds = value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: {label} carries no timestamp, got {value}"));
    (seconds * 1000.0).round() as i64
}

/// How much of the span a transition comes due in is waited out with a timer before the
/// row below spins out the rest: a timer only resolves to about a millisecond, and each
/// check has to land in the first microseconds AFTER its due instant — the earliest a
/// materialisation can run, and the only window in which the two forms of a due instant
/// could disagree.
const SPUN_OUT: Duration = Duration::from_millis(20);

/// How many pending changes of each kind the invariant is checked over.
///
/// The two forms of a due instant are read a fraction of a microsecond apart, so they can
/// only disagree on a sample whose two readings fall either side of a millisecond tick.
/// Taking several thousand samples back to back spreads them over more than one tick, so
/// the window is sampled rather than hoped for — and costs nothing in wall-clock time,
/// because every sample of a row waits out ONE span together.
const DUE_INSTANT_SAMPLES: usize = 8_192;

#[tokio::test]
async fn the_two_forms_of_a_due_instant_denote_one_instant() {
    // Each row is one kind of pending change this service records, with the pair of
    // instants it keeps: the monotonic one the materialisation waits for, and the
    // wall-clock one it dates the record at. The invariant the materialisation rests on is
    // that reaching the first means the second has passed too — so a handler that
    // materialises a transition and then reads the clock to date its own change can never
    // read an instant EARLIER than the stamp the materialisation just wrote.
    #[allow(clippy::type_complexity)]
    let rows: [(&str, fn() -> (std::time::Instant, i64)); 3] = [
        ("an environment's lifecycle transition", || {
            let pending = evs::state::PendingTransition::starting_now(
                evs::state::STATE_CREATED,
                evs::state::STATE_CREATED,
            );
            (pending.due_at, pending.due_at_ms)
        }),
        ("a host's status change", || {
            let pending = evs::state::PendingStatus::starting_now(evs::state::HOST_CREATED);
            (pending.due_at, pending.due_at_ms)
        }),
        ("a connector's state change", || {
            let pending =
                evs::state::PendingConnectorState::starting_now(evs::state::CONNECTOR_ACTIVE);
            (pending.due_at, pending.due_at_ms)
        }),
    ];

    for (case, start) in rows {
        // Back to back, so the samples span several millisecond ticks of the wall clock.
        let samples: Vec<(std::time::Instant, i64)> = (0..DUE_INSTANT_SAMPLES)
            .map(|_| start())
            .collect();
        // One wait for all of them: they were started within a few milliseconds of each
        // other, so their due instants arrive in the order they were taken.
        tokio::time::sleep(evs::state::TRANSITION_DELAY.saturating_sub(SPUN_OUT)).await;
        for (sample, (due_at, due_at_ms)) in samples.into_iter().enumerate() {
            // Spun rather than slept, so the check lands at the due instant and not a
            // timer tick past it.
            while std::time::Instant::now() < due_at {
                std::hint::spin_loop();
            }
            let at_due_ms = evs::state::now_ms();
            assert!(
                at_due_ms >= due_at_ms,
                "case {case:?} sample {sample}: reaching the instant the change is waited \
                 for must mean the instant it is dated at ({due_at_ms}) has passed, got \
                 {at_due_ms}"
            );
        }
    }
}

// Two worker threads: the call under test BLOCKS one of them on this service's store lock
// while the test holds it, so a second is needed to run the test's own task on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_is_dated_after_the_transition_the_same_call_materialised() {
    let rows: [(&str, Change); 5] = [
        ("a connector removed", Change::RemoveConnector),
        (
            "a connector pointed at another appliance",
            Change::ReconfigureConnector,
        ),
        ("an ESX host removed", Change::RemoveHost),
        ("a tag written on the environment", Change::WriteTag),
        ("the environment deleted", Change::DeleteEnvironment),
    ];

    for (index, (case, change)) in rows.into_iter().enumerate() {
        let router = Arc::new(emulator());
        let subject = prepare(&router, case, index, change).await;
        let (operation, body) = request(&subject, change);

        // This service's store lock, taken and HELD past the instant the record's
        // transition is due, on a thread of its own so the call under test — issued below
        // while it is held — cannot be applied until it is released. The thread SIGNALS
        // once it holds the lock, so the call is issued against a lock that is provably
        // held rather than after a span assumed to be long enough. Answers the instant it
        // was released at.
        let (held, taken) = ::tokio::sync::oneshot::channel();
        let holding = tokio::task::spawn_blocking({
            let engine = Arc::clone(&router.engine);
            move || {
                let state = evs::state::service_state(&*engine)
                    .expect("the in-memory engine carries this service's store");
                let store = state.lock();
                let _ = held.send(());
                std::thread::sleep(held_for());
                let released_at_ms = evs::state::now_ms();
                drop(store);
                released_at_ms
            }
        });
        taken.await.expect("the store lock was taken");

        let arrived_at_ms = evs::state::now_ms();
        assert!(
            arrived_at_ms < subject.due_at_ms,
            "case {case:?}: the call under test must arrive before its record's transition \
             is due ({}), got {arrived_at_ms}",
            subject.due_at_ms
        );
        let waiting = tokio::spawn({
            let router = Arc::clone(&router);
            async move { call(&router, operation, &body).await }
        });

        let released_at_ms = holding.await.expect("the lock was held and released");
        let (status, answered) = waiting.await.expect("the call under test ran to completion");
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} expected 200, got {status} ({}) {answered}",
            error_type(&answered)
        );

        // Dated at the instant the change was APPLIED — which cannot be before the lock
        // was released — and so past the transition this very call materialised.
        let after_ms = stored_stamp(&router, case, &subject);
        assert!(
            after_ms >= subject.due_at_ms,
            "case {case:?}: a change must not be dated before the transition the same call \
             materialised ({}), got {after_ms}",
            subject.due_at_ms
        );
        assert!(
            after_ms >= released_at_ms,
            "case {case:?}: a change must be dated at the instant it was applied, which is \
             no earlier than {released_at_ms} (the request arrived at {arrived_at_ms}), got \
             {after_ms}"
        );

        // The record the response reports carries the state change the call made, so its
        // stamp is the one the store now holds and not an earlier one.
        if let Some((reported_member, state_member, expected_state)) = reported(change) {
            let record = answered[reported_member].clone();
            assert_eq!(
                record[state_member],
                json!(expected_state),
                "case {case:?}: the {reported_member} answered must report {expected_state}, \
                 got {}",
                record[state_member]
            );
            let dated_ms = wire_ms(case, reported_member, &record["modifiedAt"]);
            assert_eq!(
                dated_ms, after_ms,
                "case {case:?}: the {reported_member} answered must carry the stamp the \
                 store holds ({after_ms}), got {dated_ms}"
            );
        }

        // And the record is never dated before the environment that HOLDS it: a change to
        // a child is a change to the environment, and both are dated at the one instant
        // the change was applied at.
        let environment_ms = environment_stamp(&router, case, &subject.environment);
        assert!(
            after_ms >= environment_ms,
            "case {case:?}: the record carries the change the environment was re-stamped \
             for, so it cannot be dated before the environment ({environment_ms}), got \
             {after_ms}"
        );

        // Persisted: the record's own reader reports the same stamp.
        let read_back = read_back_stamp(&router, case, &subject).await;
        assert_eq!(
            wire_ms(case, "the stamp read back", &read_back),
            after_ms,
            "case {case:?}: the reader must report the stamp the store holds ({after_ms}), \
             got {read_back}"
        );

        // The environment's own deletion re-stamps the VLANs that go away with it, so
        // they too are dated at the instant the deletion was applied.
        if matches!(change, Change::DeleteEnvironment) {
            let vlan_ms = earliest_vlan_stamp(&router, case, &subject.environment);
            assert_eq!(
                vlan_ms, after_ms,
                "case {case:?}: a VLAN the deletion re-stamps is dated at the \
                 environment's own instant ({after_ms}), got {vlan_ms}"
            );
        }
    }
}
