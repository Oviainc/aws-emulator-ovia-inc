// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `EnableFederation`.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness —
//! an `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the
//! production router serves — so what is asserted here is what a client sees,
//! including the HTTP status and the error code.
//!
//! The rules under test:
//!
//! * enabling Lake query federation on a store whose federation is off persists
//!   the role the request names and moves the store's federation status off
//!   `DISABLED` into the transient `ENABLING`, which the clock settles into
//!   `ENABLED` after
//!   [`service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::FEDERATION_SETTLE_AFTER`];
//!   the call answers with the addressed store's ARN, that status and that role
//!   (model: `EnableFederationResponse`);
//! * the identifier is "The ARN (or ID suffix of the ARN) of the event data store"
//!   (model doc for `EnableFederationRequest$EventDataStore`), so every form
//!   addresses the same store — the ARN pinning the Region it names, a bare id the
//!   Region the call was made in;
//! * a store that cannot be federated is refused with the modelled error for its
//!   condition and left exactly as it was: an unreadable identifier with
//!   `EventDataStoreARNInvalidException`, one naming no store in the Region it
//!   addresses with `EventDataStoreNotFoundException`, a store awaiting deletion
//!   with `InactiveEventDataStoreException`, a federation change still in flight
//!   with `ConcurrentModificationException`, and a store whose federation is
//!   already on with `EventDataStoreFederationEnabledException`;
//! * `FederationRoleArn` is "The ARN of the federation role" that "must exist in
//!   your account", so a value the modelled pattern admits but that is not an IAM
//!   role ARN is refused with `InvalidParameterException`.
//!
//! The registry is seeded through the service's public `state` module because
//! `CreateEventDataStore` is still a scaffold stub (it answers 501), so the stores
//! these cases federate cannot yet be made over the wire. Every federation call,
//! and every read that proves what it persisted, goes through the router.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, EventDataStoreBillingMode, EventDataStoreRecord, EventDataStoreState,
    FederationState, FEDERATION_SETTLE_AFTER,
};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// A Region other than [`HOME_REGION`], for the cases about which Region an
/// identifier addresses a store in.
const OTHER_REGION: &str = "us-west-2";

/// The HTTP status this service answers a not-found refusal with: the model
/// annotates `EventDataStoreNotFoundException` `@httpError(404)`, and the recorded
/// CloudTrail response for a federation call naming an absent store is 400.
const NOT_FOUND_STATUS: u16 = 400;

/// A margin on top of [`FEDERATION_SETTLE_AFTER`], so a case that waits for the
/// clock does not race the instant the handler read.
fn past_settle() -> Duration {
    FEDERATION_SETTLE_AFTER + Duration::from_millis(250)
}

/// A store record under `id` in `region` with federation off — what a store looks
/// like before anybody federates it.
fn record(id: &str, region: &str) -> EventDataStoreRecord {
    EventDataStoreRecord {
        id: id.to_string(),
        name: format!("lake-{id}"),
        region: region.to_string(),
        status: EventDataStoreState::Enabled,
        created_at: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
        retention_period: Some(7),
        billing_mode: EventDataStoreBillingMode::ExtendableRetentionPricing,
        federation_status: FederationState::Disabled,
        ..EventDataStoreRecord::default()
    }
}

/// Persist `record` in the registry every event data store operation reads, as
/// `CreateEventDataStore` will once it is implemented.
fn seed(emulator: &Emulator, record: &EventDataStoreRecord) {
    emulator
        .state()
        .lock()
        .event_data_stores
        .insert(record.id.clone(), record.clone());
}

/// Every stored store, rendered — for the cases that assert a refusal wrote
/// nothing.
fn registry(emulator: &Emulator) -> BTreeMap<String, String> {
    emulator
        .state()
        .lock()
        .event_data_stores
        .iter()
        .map(|(id, record)| (id.clone(), format!("{record:?}")))
        .collect()
}

/// The stored record under `id` — for the assertions about what a call persisted
/// that the wire does not carry (the instant the clock owes the transition at).
fn stored_store(emulator: &Emulator, id: &str) -> EventDataStoreRecord {
    emulator
        .state()
        .lock()
        .event_data_stores
        .get(id)
        .expect("the seeded store is in the registry")
        .clone()
}

/// Call `EnableFederation` from `region`.
async fn enable_in(emulator: &Emulator, region: &str, identifier: &str, role: &str) -> Reply {
    emulator
        .call_in(
            "EnableFederation",
            region,
            &json!({ "EventDataStore": identifier, "FederationRoleArn": role }),
        )
        .await
}

/// Call `GetEventDataStore` from `region` — the read that proves what a federation
/// call persisted.
async fn get_in(emulator: &Emulator, region: &str, identifier: &str) -> Reply {
    emulator
        .call_in(
            "GetEventDataStore",
            region,
            &json!({ "EventDataStore": identifier }),
        )
        .await
}

/// A string member of a reply body.
fn member(body: &Value, name: &str) -> Option<String> {
    body.get(name).and_then(Value::as_str).map(str::to_string)
}

/// One row of the table a successful federation spans: the store to federate, the
/// identifier form it is addressed by, the Region the call is made in and the role
/// ARN the request names.
struct Row {
    case: &'static str,
    store: EventDataStoreRecord,
    /// Built from the stored store, so the row states the FORM, not a literal.
    identifier: fn(&EventDataStoreRecord) -> String,
    caller_region: &'static str,
    role: &'static str,
}

/// The table of stores a successful federation spans: 6 rows differing along
/// everything the rule generalises over — which identifier form names the store,
/// which Region the call is made in, which live status and which Region the store
/// carries, and the form of the role ARN (a role path, another partition, another
/// account).
fn rows() -> Vec<Row> {
    let mut created = record("22222222-2222-4222-8222-222222222222", HOME_REGION);
    created.status = EventDataStoreState::Created;

    let mut stopped = record("33333333-3333-4333-8333-333333333333", HOME_REGION);
    stopped.status = EventDataStoreState::StoppedIngestion;

    let mut elsewhere = record("44444444-4444-4444-8444-444444444444", OTHER_REGION);
    elsewhere.multi_region_enabled = true;

    let mut already_updated = record("55555555-5555-4555-8555-555555555555", HOME_REGION);
    already_updated.updated_at = Some(UNIX_EPOCH + Duration::from_secs(1_700_086_400));

    let bare_id_elsewhere = record("66666666-6666-4666-8666-666666666666", OTHER_REGION);

    vec![
        Row {
            case: "by bare id, from the store's Region",
            store: record("11111111-1111-4111-8111-111111111111", HOME_REGION),
            identifier: |store| store.id.clone(),
            caller_region: HOME_REGION,
            role: "arn:aws:iam::000000000000:role/lake-federation",
        },
        Row {
            case: "by prefixed id, a CREATED store",
            store: created,
            identifier: |store| format!("eventdatastore/{}", store.id),
            caller_region: HOME_REGION,
            // A role path is part of the role's name.
            role: "arn:aws:iam::000000000000:role/service-role/lake.federation_1",
        },
        Row {
            case: "by ARN, a store that has stopped ingesting",
            store: stopped,
            identifier: EventDataStoreRecord::arn,
            caller_region: HOME_REGION,
            role: "arn:aws:iam::123456789012:role/r",
        },
        Row {
            case: "by ARN of another Region's store",
            store: elsewhere,
            identifier: EventDataStoreRecord::arn,
            caller_region: HOME_REGION,
            // A partition other than `aws`, which the modelled pattern admits.
            role: "arn:aws-us-gov:iam::000000000000:role/lake-federation",
        },
        Row {
            case: "by bare id, a store already updated once",
            store: already_updated,
            identifier: |store| store.id.clone(),
            caller_region: HOME_REGION,
            role: "arn:aws-cn:iam::000000000000:role/Lake_Federation-2.0",
        },
        Row {
            // A bare id names the store in the Region the CALL was made in, so a
            // store living outside the home Region is federated by id from there.
            case: "by bare id, called in the store's own non-home Region",
            store: bare_id_elsewhere,
            identifier: |store| store.id.clone(),
            caller_region: OTHER_REGION,
            role: "arn:aws:iam::000000000000:role/lake-federation",
        },
    ]
}

#[tokio::test]
async fn enabling_federation_persists_the_role_and_settles_into_enabled() {
    let emulator = Emulator::new();
    let rows = rows();
    for row in &rows {
        seed(&emulator, &row.store);
    }

    // Every row is federated first, then the clock is given its span ONCE, so the
    // settled state is asserted for every row after a single wait.
    let started = Instant::now();
    let mut federated = Vec::new();
    for row in &rows {
        let case = row.case;
        let identifier = (row.identifier)(&row.store);
        let arn = row.store.arn();

        let called_at = SystemTime::now();
        let reply = enable_in(&emulator, row.caller_region, &identifier, row.role).await;
        let body = reply.expect_ok(case);
        assert_eq!(
            member(body, "EventDataStoreArn").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the addressed store's ARN {arn:?}, got {body:?}"
        );
        // The span is never zero, so the call that starts the transition reports
        // the transient status.
        assert_eq!(
            member(body, "FederationStatus").as_deref(),
            Some(FederationState::Enabling.as_wire()),
            "case {case:?}: expected the transient federation status, got {body:?}"
        );
        assert_eq!(
            member(body, "FederationRoleArn").as_deref(),
            Some(row.role),
            "case {case:?}: expected the role the request named, got {body:?}"
        );

        // The transition is owed to the CLOCK, at an instant strictly after the
        // call: that is what makes the transient status above real for every
        // caller, however fast the machine. Asserted on the stored record, so it
        // does not depend on how long this test took.
        let stored = stored_store(&emulator, &row.store.id);
        assert_eq!(
            stored.federation_status,
            FederationState::Enabling,
            "case {case:?}: the stored status must be the transient one, got {:?}",
            stored.federation_status
        );
        let due = stored
            .federation_settles_at
            .unwrap_or_else(|| panic!("case {case:?}: a transient status owes a settle instant"));
        assert!(
            due > called_at,
            "case {case:?}: the settle instant must fall after the call that started \
             the transition, got {due:?} for a call at {called_at:?}"
        );

        // Read the store back through `GetEventDataStore`: what the call persisted
        // is what a later caller sees.
        let read = get_in(&emulator, &row.store.region, &arn).await;
        let read_body = read.expect_ok(case);
        assert_eq!(
            member(read_body, "FederationRoleArn").as_deref(),
            Some(row.role),
            "case {case:?}: a read must report the persisted role, got {read_body:?}"
        );
        if started.elapsed() < FEDERATION_SETTLE_AFTER {
            assert_eq!(
                member(read_body, "FederationStatus").as_deref(),
                Some(FederationState::Enabling.as_wire()),
                "case {case:?}: within the settle span a read must still report the \
                 transient status, got {read_body:?}"
            );
        }
        // The store changed, so its update time moved to at or after its creation
        // time — and past the time it last carried.
        let updated = read_body.get("UpdatedTimestamp").and_then(Value::as_f64);
        let created = read_body.get("CreatedTimestamp").and_then(Value::as_f64);
        assert!(
            updated > created,
            "case {case:?}: federating the store must move its UpdatedTimestamp past \
             its CreatedTimestamp, got {read_body:?}"
        );

        federated.push((case, row.store.region.clone(), arn, row.role));
    }

    // The clock owes `ENABLING` -> `ENABLED` at the settle instant, whether or not
    // anybody reads the store.
    sleep(past_settle());

    for (case, region, arn, role) in federated {
        let read = get_in(&emulator, &region, &arn).await;
        let body = read.expect_ok(case).clone();
        assert_eq!(
            member(&body, "FederationStatus").as_deref(),
            Some(FederationState::Enabled.as_wire()),
            "case {case:?}: once the settle span has passed a read must report ENABLED, \
             got {body:?}"
        );
        assert_eq!(
            member(&body, "FederationRoleArn").as_deref(),
            Some(role),
            "case {case:?}: settling must keep the persisted role, got {body:?}"
        );
        // A settled status stays settled: the transition is materialised once.
        let again = get_in(&emulator, &region, &arn).await;
        assert_eq!(
            again.expect_ok(case),
            &body,
            "case {case:?}: a later read must answer what the settled read answered, \
             got {:?}",
            again.body
        );
    }
}

#[tokio::test]
async fn a_store_that_cannot_be_federated_is_refused_and_left_alone() {
    let emulator = Emulator::new();
    let role = "arn:aws:iam::000000000000:role/lake-federation";
    let future = SystemTime::now() + Duration::from_secs(60);

    let live = record("11111111-1111-4111-8111-111111111111", HOME_REGION);
    let mut pending_deletion = record("22222222-2222-4222-8222-222222222222", HOME_REGION);
    pending_deletion.status = EventDataStoreState::PendingDeletion;
    let mut already_federated = record("33333333-3333-4333-8333-333333333333", HOME_REGION);
    already_federated.federation_status = FederationState::Enabled;
    already_federated.federation_role_arn =
        Some("arn:aws:iam::000000000000:role/other-federation".to_string());
    let mut enabling = record("44444444-4444-4444-8444-444444444444", HOME_REGION);
    enabling.federation_status = FederationState::Enabling;
    enabling.federation_settles_at = Some(future);
    enabling.federation_role_arn = Some(role.to_string());
    let mut disabling = record("55555555-5555-4555-8555-555555555555", HOME_REGION);
    disabling.federation_status = FederationState::Disabling;
    disabling.federation_settles_at = Some(future);
    let elsewhere = record("66666666-6666-4666-8666-666666666666", OTHER_REGION);

    for store in [
        &live,
        &pending_deletion,
        &already_federated,
        &enabling,
        &disabling,
        &elsewhere,
    ] {
        seed(&emulator, store);
    }
    let before = registry(&emulator);

    let absent = "77777777-7777-4777-8777-777777777777";
    // Each row names a condition the store cannot be federated under, and the
    // modelled error it is refused with. The unreadable identifiers are values the
    // modelled `EventDataStoreArn` pattern and length bounds admit, so the
    // boundary validator hands them to the handler.
    let cases: Vec<(&str, &str, String, u16, &str)> = vec![
        (
            "an ARN of another resource type",
            HOME_REGION,
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/lake"),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "an ARN of another service",
            HOME_REGION,
            format!("arn:aws:logs:{HOME_REGION}:000000000000:eventdatastore/{absent}"),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "an ARN with no Region",
            HOME_REGION,
            format!("arn:aws:cloudtrail::000000000000:eventdatastore/{absent}"),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "the resource prefix alone",
            HOME_REGION,
            "eventdatastore/".to_string(),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "an absent id",
            HOME_REGION,
            absent.to_string(),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "an absent ARN",
            HOME_REGION,
            event_data_store_arn(HOME_REGION, absent),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "the bare id of a store living elsewhere",
            HOME_REGION,
            elsewhere.id.clone(),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "the ARN of this Region's store, in another Region",
            OTHER_REGION,
            event_data_store_arn(OTHER_REGION, &live.id),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "a store awaiting deletion",
            HOME_REGION,
            pending_deletion.id.clone(),
            400,
            "InactiveEventDataStoreException",
        ),
        (
            "a store whose federation is already on",
            HOME_REGION,
            already_federated.id.clone(),
            400,
            "EventDataStoreFederationEnabledException",
        ),
        (
            "a store still turning federation on",
            HOME_REGION,
            enabling.id.clone(),
            400,
            "ConcurrentModificationException",
        ),
        (
            "a store still turning federation off",
            HOME_REGION,
            disabling.id.clone(),
            400,
            "ConcurrentModificationException",
        ),
    ];

    for (case, region, identifier, status, code) in cases {
        let reply = enable_in(&emulator, region, &identifier, role).await;
        reply.expect_error(case, status, code);
        // The refusal says which store it was about: the value itself when it
        // names no store at all, the ARN addressed otherwise.
        let named = if code == "EventDataStoreARNInvalidException" || identifier.contains(':') {
            identifier.clone()
        } else {
            event_data_store_arn(region, &identifier)
        };
        assert!(
            reply.message().contains(&named),
            "case {case:?}: the refusal must name {named:?}, got {:?}",
            reply.message()
        );
    }

    assert_eq!(
        registry(&emulator),
        before,
        "a refused federation must leave every stored store exactly as it was, got {:?}",
        registry(&emulator)
    );
}

#[tokio::test]
async fn a_federation_role_that_is_not_an_iam_role_arn_is_refused() {
    let emulator = Emulator::new();
    let store = record("11111111-1111-4111-8111-111111111111", HOME_REGION);
    seed(&emulator, &store);
    // A malformed role is answered before the store's own state, so the same
    // refusal is due whatever state the store it names is in.
    let mut pending_deletion = record("22222222-2222-4222-8222-222222222222", HOME_REGION);
    pending_deletion.status = EventDataStoreState::PendingDeletion;
    seed(&emulator, &pending_deletion);
    let mut already_federated = record("33333333-3333-4333-8333-333333333333", HOME_REGION);
    already_federated.federation_status = FederationState::Enabled;
    already_federated.federation_role_arn =
        Some("arn:aws:iam::000000000000:role/other-federation".to_string());
    seed(&emulator, &already_federated);
    let before = registry(&emulator);

    // Every row satisfies the modelled `FederationRoleArn` constraints (3..125
    // characters of `^[a-zA-Z0-9._/\-:@=\+,\.]+$`), so the boundary validator hands
    // it to the handler, and none of them is an IAM role ARN.
    let cases = [
        ("a bare role name", "lake-federation"),
        (
            "an IAM ARN of another resource type",
            "arn:aws:iam::000000000000:user/lake-federation",
        ),
        (
            "a role ARN of another service",
            "arn:aws:sts::000000000000:role/lake-federation",
        ),
        (
            "an IAM ARN carrying a Region",
            "arn:aws:iam:us-east-1:000000000000:role/lake-federation",
        ),
        (
            "an IAM ARN with no partition",
            "arn::iam::000000000000:role/lake-federation",
        ),
        (
            "an IAM ARN with no account",
            "arn:aws:iam:::role/lake-federation",
        ),
        ("an IAM ARN with no role name", "arn:aws:iam::000000000000:role/"),
        (
            "an IAM ARN writing the role with a colon",
            "arn:aws:iam::000000000000:role:lake-federation",
        ),
        (
            "an IAM ARN with too many fields",
            "arn:aws:iam::000000000000:role/lake:1",
        ),
    ];

    for (case, role) in cases {
        for (store_case, store_id) in [
            ("a live store", &store.id),
            ("a store awaiting deletion", &pending_deletion.id),
            ("a store already federated", &already_federated.id),
        ] {
            let case = format!("{case}, against {store_case}");
            let reply = enable_in(&emulator, HOME_REGION, store_id, role).await;
            reply.expect_error(&case, 400, "InvalidParameterException");
            assert!(
                reply.message().contains("FederationRoleArn") && reply.message().contains(role),
                "case {case:?}: the refusal must name the member and the value it refused, \
                 got {:?}",
                reply.message()
            );
        }
    }

    assert_eq!(
        registry(&emulator),
        before,
        "a refused federation must leave every stored store exactly as it was, got {:?}",
        registry(&emulator)
    );

    // A store that is NOT one of the three above takes a role that IS an IAM role
    // ARN, so the refusals above are about the value, not about the store. The
    // second row carries the whole character set IAM documents for a role name
    // (`[\w+=,.@-]`) and a role path, so the refusals are not refusing those
    // characters either.
    for (case, id, role) in [
        (
            "a plain role ARN",
            "44444444-4444-4444-8444-444444444444",
            "arn:aws:iam::000000000000:role/lake-federation",
        ),
        (
            "a role ARN with a path and the documented name characters",
            "55555555-5555-4555-8555-555555555555",
            "arn:aws-us-gov:iam::123456789012:role/service-role/lake+fed=1,x@2.y_3-4",
        ),
    ] {
        let takes_a_role = record(id, HOME_REGION);
        seed(&emulator, &takes_a_role);
        let accepted = enable_in(&emulator, HOME_REGION, &takes_a_role.id, role).await;
        assert_eq!(
            member(accepted.expect_ok(case), "FederationRoleArn").as_deref(),
            Some(role),
            "case {case:?}: expected the role the request named, got {:?}",
            accepted.body
        );
        assert_eq!(
            member(&accepted.body, "FederationStatus").as_deref(),
            Some(FederationState::Enabling.as_wire()),
            "case {case:?}: expected the transient federation status, got {:?}",
            accepted.body
        );
    }
}
