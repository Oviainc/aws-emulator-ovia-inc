// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateEventDataStore`, read back through
//! `GetEventDataStore`.
//!
//! Every case goes over the wire through the shared [`Emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted here is what a client sees, including the HTTP
//! status and the error code. What a CREATE persists is
//! `event_data_store_lifecycle.rs`; this suite is about the operation that changes a
//! stored store.
//!
//! The rules under test, each grounded in this service's Smithy model
//! (`smithy_query(query="operation", operation="UpdateEventDataStore")`):
//!
//! * an update applies exactly the members the request carries, leaves every other
//!   member as it was, and is reported identically by the next read;
//! * "Other parameters are optional, but at least one optional parameter must be
//!   specified, or CloudTrail throws an error" — refused with the one fixed text that
//!   refusal carries, before the identifier is resolved;
//! * the retention period a store may carry is the band its `BillingMode` documents
//!   (`EXTENDABLE_RETENTION_PRICING` 7..3653 days, `FIXED_RETENTION_PRICING`
//!   7..2557), read against the billing mode the SAME call leaves the store in;
//! * "You can't change the billing mode from EXTENDABLE_RETENTION_PRICING to
//!   FIXED_RETENTION_PRICING";
//! * "After you associate an event data store with a KMS key, the KMS key cannot be
//!   removed or changed" — and the accepted forms of one key name one key;
//! * a store name is unique per Region, and a store never conflicts with itself;
//! * "You can configure up to five advanced event selectors for each event data
//!   store";
//! * an identifier is "The ARN (or the ID suffix of the ARN) of the event data store
//!   that you want to update": one naming no store of this account and Region is
//!   `EventDataStoreNotFoundException` over the ARN it addressed, and a value WRITTEN
//!   as an ARN that is not an event data store ARN is
//!   `EventDataStoreARNInvalidException`;
//! * a store awaiting deletion holds no configuration left to change.
//!
//! Every refused update must leave the store exactly as it was, which each case
//! asserts by reading it back.

use serde_json::{json, Map, Value};

use crate::emulator::{Emulator, Reply, HOME_REGION};

/// The model's `BillingMode` enum values.
const EXTENDABLE: &str = "EXTENDABLE_RETENTION_PRICING";
/// See [`EXTENDABLE`].
const FIXED: &str = "FIXED_RETENTION_PRICING";

/// The HTTP status this service answers a not-found refusal with: the model annotates
/// `EventDataStoreNotFoundException` `@httpError(404)`, and the recorded CloudTrail
/// responses for a store that does not exist are 400.
const NOT_FOUND_STATUS: u16 = 400;

/// The members an update's response and a later `GetEventDataStore` both report, so
/// one can be compared with the other member by member.
///
/// `Status` is left out: it is the one member the CLOCK moves on its own.
const SHARED_MEMBERS: [&str; 9] = [
    "EventDataStoreArn",
    "Name",
    "AdvancedEventSelectors",
    "MultiRegionEnabled",
    "OrganizationEnabled",
    "RetentionPeriod",
    "TerminationProtectionEnabled",
    "KmsKeyId",
    "BillingMode",
];

/// Create a store named `name` with `extra` merged into the request, and hand back the
/// `CreateEventDataStore` response body.
async fn create(emulator: &Emulator, name: &str, extra: &Value) -> Value {
    let mut request = Map::new();
    request.insert("Name".to_string(), json!(name));
    for (member, value) in extra.as_object().into_iter().flatten() {
        request.insert(member.clone(), value.clone());
    }
    emulator
        .call("CreateEventDataStore", &Value::Object(request))
        .await
        .expect_ok(&format!("CreateEventDataStore {name} with {extra}"))
        .clone()
}

/// Update the store `identifier` names with `change`, and hand back the reply.
async fn update(emulator: &Emulator, identifier: &str, change: &Value) -> Reply {
    let mut request = change
        .as_object()
        .cloned()
        .expect("an update is an object of members");
    request.insert("EventDataStore".to_string(), json!(identifier));
    emulator
        .call("UpdateEventDataStore", &Value::Object(request))
        .await
}

/// Read the store `identifier` names, and hand back the reply.
async fn get(emulator: &Emulator, identifier: &str) -> Reply {
    emulator
        .call("GetEventDataStore", &json!({"EventDataStore": identifier}))
        .await
}

/// The ARN's id suffix, which is the other identifier form every operation accepts.
fn store_id(body: &Value) -> String {
    body.get("EventDataStoreArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert `read` reports `before` for every shared member except those `changed`
/// names, and the changed ones as `changed` states them.
fn assert_members(case: &str, before: &Value, read: &Value, changed: &Value) {
    for member in SHARED_MEMBERS {
        let expected = changed.get(member).or_else(|| before.get(member));
        let actual = read.get(member);
        assert_eq!(
            actual,
            expected,
            "case {case}: expected {member} {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: an update applies exactly the members the request carries and leaves every
/// other member as it was; the next read reports the same values.
///
/// The rows differ along WHICH member the request carries and what it carries — a
/// name, a selector list, each boolean flag, a retention period, a KMS key, a billing
/// mode, and several at once.
#[tokio::test]
async fn an_update_applies_only_the_members_it_carries() {
    // (case, the update, the members it must change)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a rename",
            json!({"Name": "renamed"}),
            json!({"Name": "renamed"}),
        ),
        (
            "a selector list",
            json!({"AdvancedEventSelectors": [{
                "Name": "data events",
                "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Data"]}],
            }]}),
            json!({"AdvancedEventSelectors": [{
                "Name": "data events",
                "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Data"]}],
            }]}),
        ),
        (
            "multi-Region on",
            json!({"MultiRegionEnabled": true}),
            json!({"MultiRegionEnabled": true}),
        ),
        (
            "organization on",
            json!({"OrganizationEnabled": true}),
            json!({"OrganizationEnabled": true}),
        ),
        (
            "the shortest retention period",
            json!({"RetentionPeriod": 7}),
            json!({"RetentionPeriod": 7}),
        ),
        (
            "the longest retention period",
            json!({"RetentionPeriod": 3653}),
            json!({"RetentionPeriod": 3653}),
        ),
        (
            "termination protection on",
            json!({"TerminationProtectionEnabled": true}),
            json!({"TerminationProtectionEnabled": true}),
        ),
        (
            "a KMS key",
            json!({"KmsKeyId": "alias/lake-key"}),
            json!({"KmsKeyId": "alias/lake-key"}),
        ),
        (
            "the billing mode it already carries",
            json!({"BillingMode": EXTENDABLE}),
            json!({"BillingMode": EXTENDABLE}),
        ),
        (
            "every member at once",
            json!({
                "Name": "everything",
                "MultiRegionEnabled": true,
                "OrganizationEnabled": true,
                "RetentionPeriod": 90,
                "TerminationProtectionEnabled": true,
                "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/00000000-0000-4000-8000-000000000001",
                "BillingMode": EXTENDABLE,
            }),
            json!({
                "Name": "everything",
                "MultiRegionEnabled": true,
                "OrganizationEnabled": true,
                "RetentionPeriod": 90,
                "TerminationProtectionEnabled": true,
                "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/00000000-0000-4000-8000-000000000001",
                "BillingMode": EXTENDABLE,
            }),
        ),
    ];

    for (index, (case, change, changed)) in rows.into_iter().enumerate() {
        // One store per row, so a row is independent of the rows before it. Each one
        // starts with a selector list and a retention period the row does not name,
        // so "left as it was" is something to observe.
        let emulator = Emulator::new();
        let created = create(
            &emulator,
            &format!("store-{index}"),
            &json!({
                "RetentionPeriod": 30,
                "AdvancedEventSelectors": [{
                    "Name": "management events",
                    "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Management"]}],
                }],
            }),
        )
        .await;
        let id = store_id(&created);

        // Both identifier forms address the same store.
        let identifier = if index % 2 == 0 {
            id.clone()
        } else {
            created
                .get("EventDataStoreArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let reply = update(&emulator, &identifier, &change).await;
        let updated = reply.expect_ok(case).clone();
        assert_members(case, &created, &updated, &changed);

        // The response and the next read are the same values: the update persisted.
        let read = get(&emulator, &id).await;
        let read = read.expect_ok(case).clone();
        assert_members(&format!("{case}, read back"), &created, &read, &changed);

        // "UpdatedTimestamp is always either the same or newer than the time shown in
        // CreatedTimestamp."
        let created_at = updated.get("CreatedTimestamp").cloned();
        let updated_at = updated.get("UpdatedTimestamp").cloned();
        assert_eq!(
            created_at,
            created.get("CreatedTimestamp").cloned(),
            "case {case}: an update must not move CreatedTimestamp, got {updated:?}"
        );
        assert!(
            updated_at.is_some()
                && updated_at.as_ref().and_then(Value::as_f64)
                    >= created_at.as_ref().and_then(Value::as_f64),
            "case {case}: expected UpdatedTimestamp at or after CreatedTimestamp, got {updated:?}"
        );
    }
}

/// The members CloudTrail's "no updatable member" refusal enumerates, in the order it
/// states them. Each is spelled as the input member is spelled
/// (`smithy_query(query="operation", operation="UpdateEventDataStore")`: the optional
/// members are Name, AdvancedEventSelectors, MultiRegionEnabled, OrganizationEnabled,
/// RetentionPeriod, TerminationProtectionEnabled, KmsKeyId and BillingMode).
const MEMBERS_THE_REFUSAL_NAMES: [&str; 7] = [
    "AdvancedEventSelectors",
    "Name",
    "RetentionPeriod",
    "MultiRegionEnabled",
    "TerminationProtectionEnabled",
    "OrganizationEnabled",
    "KmsKeyId",
];

/// The whole refusal text, which is FIXED: no part of it is derived from the request
/// or from the registry.
fn no_updatable_member_message() -> String {
    format!(
        "At least one of the following must be provided: {}",
        MEMBERS_THE_REFUSAL_NAMES.join(", ")
    )
}

/// RULE: "at least one optional parameter must be specified, or CloudTrail throws an
/// error" — the refusal is `InvalidParameterException` carrying that one fixed text,
/// whatever store the identifier addresses and whether it addresses one at all (the
/// precondition is read before the store is resolved), and the refused call changes
/// nothing.
///
/// The rows differ along the identifier the call carries — the two accepted forms, a
/// store that exists and ARNs and ids that name none, in the caller's Region and in
/// another — because none of that may reach the message.
#[tokio::test]
async fn an_update_naming_no_optional_member_is_refused() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let absent = "11111111-1111-1111-1111-111111111111";
    let created = create(&emulator, "untouched", &json!({"RetentionPeriod": 30})).await;
    let id = store_id(&created);
    let arn = created
        .get("EventDataStoreArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let expected = no_updatable_member_message();
    // (case, the identifier the call carries)
    let rows: Vec<(&str, String)> = vec![
        ("an existing store, by its id suffix", id.clone()),
        ("an existing store, by its ARN", arn.clone()),
        ("an id that names no store", absent.to_string()),
        (
            "an ARN that names no store in the caller's Region",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{absent}"),
        ),
        (
            "an ARN that names no store in another Region",
            format!("arn:aws:cloudtrail:us-west-2:{account}:eventdatastore/{absent}"),
        ),
        (
            // A value that is not even a readable identifier: the precondition is
            // still what answers, because it is read first.
            "a value no identifier form accepts",
            "not-an-arn".to_string(),
        ),
    ];

    for (case, identifier) in &rows {
        let reply = update(&emulator, identifier, &json!({})).await;
        reply.expect_error(case, 400, "InvalidParameterException");
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: expected the fixed refusal text {expected:?}, got {:?}",
            reply.message()
        );
    }

    // Every member the refusal names counts as a change: supplying just that one gets
    // the call PAST this precondition, so the message's list is the list the handler
    // reads. The store is one that does not exist, so what proves the precondition
    // passed is the next refusal — the store lookup's.
    let unknown = format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{absent}");
    for member in MEMBERS_THE_REFUSAL_NAMES {
        let value = match member {
            "AdvancedEventSelectors" => json!([{
                "Name": "management events",
                "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Management"]}],
            }]),
            "Name" => json!("a-new-name"),
            "RetentionPeriod" => json!(90),
            "KmsKeyId" => json!("alias/lake-key"),
            // The three remaining members are booleans; `false` is as much a supplied
            // member as `true`, so the rows carry the value that is NOT the default.
            _ => json!(false),
        };
        let reply = update(&emulator, &unknown, &json!({member: value})).await;
        reply.expect_error(member, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
    }

    // The refused calls left the store exactly as it was.
    let case = "the store the refusals addressed";
    let read = get(&emulator, &id).await;
    let read = read.expect_ok(case).clone();
    assert_members(case, &created, &read, &json!({}));
}

/// RULE: the retention period an update may set is the band the store's billing mode
/// documents — read against the mode the SAME call leaves the store in — and the
/// model's own `@range` (7..3653) is refused by the boundary validator before the
/// handler runs.
///
/// The rows walk each band's published bounds and one value past each, with and
/// without a billing mode in the same request.
#[tokio::test]
async fn retention_period_is_bounded_by_the_billing_mode_the_call_leaves() {
    // (case, the billing mode the store is created with, the update, expected code or
    // "" for a success)
    let rows: [(&str, &str, Value, &str); 10] = [
        (
            "extendable, the shortest",
            EXTENDABLE,
            json!({"RetentionPeriod": 7}),
            "",
        ),
        (
            "extendable, one shorter than the model allows",
            EXTENDABLE,
            json!({"RetentionPeriod": 6}),
            "ValidationException",
        ),
        (
            "extendable, the longest",
            EXTENDABLE,
            json!({"RetentionPeriod": 3653}),
            "",
        ),
        (
            "extendable, one longer than the model allows",
            EXTENDABLE,
            json!({"RetentionPeriod": 3654}),
            "ValidationException",
        ),
        (
            "fixed, the shortest",
            FIXED,
            json!({"RetentionPeriod": 7}),
            "",
        ),
        (
            "fixed, its longest",
            FIXED,
            json!({"RetentionPeriod": 2557}),
            "",
        ),
        (
            "fixed, one longer than its billing mode allows",
            FIXED,
            json!({"RetentionPeriod": 2558}),
            "InvalidParameterException",
        ),
        (
            "fixed, restated in the same call, at its longest",
            FIXED,
            json!({"BillingMode": FIXED, "RetentionPeriod": 2557}),
            "",
        ),
        (
            "fixed, restated in the same call, one longer",
            FIXED,
            json!({"BillingMode": FIXED, "RetentionPeriod": 2558}),
            "InvalidParameterException",
        ),
        (
            "fixed moving to extendable, past the fixed maximum",
            FIXED,
            json!({"BillingMode": EXTENDABLE, "RetentionPeriod": 2558}),
            "",
        ),
    ];

    for (index, (case, billing_mode, change, expected)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let created = create(
            &emulator,
            &format!("retention-{index}"),
            &json!({"BillingMode": billing_mode, "RetentionPeriod": 30}),
        )
        .await;
        let id = store_id(&created);
        let reply = update(&emulator, &id, change).await;
        if expected.is_empty() {
            let updated = reply.expect_ok(case);
            assert_eq!(
                updated.get("RetentionPeriod"),
                change.get("RetentionPeriod"),
                "case {case}: expected the requested RetentionPeriod, got {updated:?}"
            );
            continue;
        }
        reply.expect_error(case, 400, expected);
        // A refused update leaves the store as it was.
        let read = get(&emulator, &id).await;
        let read = read.expect_ok(case).clone();
        assert_members(case, &created, &read, &json!({}));
    }
}

/// RULE: "You can't change the billing mode from EXTENDABLE_RETENTION_PRICING to
/// FIXED_RETENTION_PRICING"; every other move between the modelled values is allowed,
/// and a value outside the enum is refused.
#[tokio::test]
async fn billing_mode_cannot_move_from_extendable_to_fixed() {
    // (case, the mode the store carries, the mode asked for, expected code or "")
    let rows: [(&str, &str, &str, &str); 4] = [
        (
            "extendable to fixed",
            EXTENDABLE,
            FIXED,
            "InvalidParameterException",
        ),
        ("extendable to extendable", EXTENDABLE, EXTENDABLE, ""),
        ("fixed to extendable", FIXED, EXTENDABLE, ""),
        ("fixed to fixed", FIXED, FIXED, ""),
    ];

    for (index, (case, from, to, expected)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        // A retention period inside BOTH bands, so only the mode decides the answer.
        let created = create(
            &emulator,
            &format!("billing-{index}"),
            &json!({"BillingMode": from, "RetentionPeriod": 30}),
        )
        .await;
        let id = store_id(&created);
        let reply = update(&emulator, &id, &json!({"BillingMode": to})).await;
        if expected.is_empty() {
            let updated = reply.expect_ok(case);
            assert_eq!(
                updated.get("BillingMode").and_then(Value::as_str),
                Some(*to),
                "case {case}: expected BillingMode {to}, got {updated:?}"
            );
            continue;
        }
        reply.expect_error(case, 400, expected);
        let read = get(&emulator, &id).await;
        let read = read.expect_ok(case).clone();
        assert_members(case, &created, &read, &json!({}));
    }
}

/// RULE: "After you associate an event data store with a KMS key, the KMS key cannot
/// be removed or changed" — and the forms the request accepts ("an alias name
/// prefixed by alias/, a fully specified ARN to an alias, a fully specified ARN to a
/// key, or a globally unique identifier") are spellings of one key, so re-stating the
/// same key in another form changes nothing.
#[tokio::test]
async fn a_kms_key_cannot_be_changed_once_associated() {
    let account = engine::emulator_account_id();
    // (case, the key the store is given, the key asked for next, expected code or "")
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "the same globally unique identifier again",
            "00000000-0000-4000-8000-000000000001".to_string(),
            "00000000-0000-4000-8000-000000000001".to_string(),
            "",
        ),
        (
            "the same key, spelled as its ARN",
            "00000000-0000-4000-8000-000000000001".to_string(),
            format!(
                "arn:aws:kms:{HOME_REGION}:{account}:key/00000000-0000-4000-8000-000000000001"
            ),
            "",
        ),
        (
            "the same alias again",
            "alias/lake-key".to_string(),
            "alias/lake-key".to_string(),
            "",
        ),
        (
            "the same alias, spelled as its ARN",
            "alias/lake-key".to_string(),
            format!("arn:aws:kms:{HOME_REGION}:{account}:alias/lake-key"),
            "",
        ),
        (
            "another key id",
            "00000000-0000-4000-8000-000000000001".to_string(),
            "00000000-0000-4000-8000-000000000002".to_string(),
            "InvalidKmsKeyIdException",
        ),
        (
            "another alias",
            "alias/lake-key".to_string(),
            "alias/other-key".to_string(),
            "InvalidKmsKeyIdException",
        ),
        (
            "another key ARN",
            format!(
                "arn:aws:kms:{HOME_REGION}:{account}:key/00000000-0000-4000-8000-000000000001"
            ),
            format!(
                "arn:aws:kms:{HOME_REGION}:{account}:key/00000000-0000-4000-8000-000000000002"
            ),
            "InvalidKmsKeyIdException",
        ),
    ];

    for (index, (case, given, requested, expected)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        // The key is associated by the UPDATE, so a store with no key can take one.
        let created = create(&emulator, &format!("kms-{index}"), &json!({})).await;
        let id = store_id(&created);
        let associated = update(&emulator, &id, &json!({"KmsKeyId": given}))
            .await
            .expect_ok(&format!("{case}: associating the key"))
            .clone();
        assert_eq!(
            associated.get("KmsKeyId").and_then(Value::as_str),
            Some(given.as_str()),
            "case {case}: expected the stored KmsKeyId to be the one given, got {associated:?}"
        );

        let reply = update(&emulator, &id, &json!({"KmsKeyId": requested})).await;
        if expected.is_empty() {
            reply.expect_ok(case);
        } else {
            reply.expect_error(case, 400, expected);
        }
        // Either way the store still carries the key it was associated with: the key
        // cannot be changed, and re-stating it changes nothing.
        let read = get(&emulator, &id).await;
        let read = read.expect_ok(case).clone();
        assert_eq!(
            read.get("KmsKeyId").and_then(Value::as_str),
            Some(given.as_str()),
            "case {case}: expected the associated key to stand, got {read:?}"
        );
    }
}

/// RULE: a store name is unique per Region; a store never conflicts with itself, and
/// the same name in another Region is another store.
#[tokio::test]
async fn a_rename_onto_a_name_another_store_holds_is_refused() {
    let emulator = Emulator::new();
    let first = create(&emulator, "shared-name", &json!({})).await;
    let second = create(&emulator, "other-name", &json!({})).await;

    let case = "a rename onto a name this Region holds";
    update(&emulator, &store_id(&second), &json!({"Name": "shared-name"}))
        .await
        .expect_error(case, 400, "EventDataStoreAlreadyExistsException");
    let read = get(&emulator, &store_id(&second)).await;
    let read = read.expect_ok(case).clone();
    assert_members(case, &second, &read, &json!({}));

    // A store never conflicts with itself.
    let case = "a rename onto its own name";
    let reply = update(&emulator, &store_id(&first), &json!({"Name": "shared-name"})).await;
    let updated = reply.expect_ok(case);
    assert_eq!(
        updated.get("Name").and_then(Value::as_str),
        Some("shared-name"),
        "case {case}: expected the name to stand, got {updated:?}"
    );

    // A name is scoped to a Region: a store in another Region may take the name this
    // Region's store carries.
    let elsewhere = emulator
        .call_in(
            "CreateEventDataStore",
            "eu-west-1",
            &json!({"Name": "elsewhere"}),
        )
        .await
        .expect_ok("CreateEventDataStore in another Region")
        .clone();
    let case = "a rename in another Region onto this Region's name";
    let mut request = Map::new();
    request.insert(
        "EventDataStore".to_string(),
        json!(store_id(&elsewhere)),
    );
    request.insert("Name".to_string(), json!("shared-name"));
    emulator
        .call_in(
            "UpdateEventDataStore",
            "eu-west-1",
            &Value::Object(request),
        )
        .await
        .expect_ok(case);
}

/// RULE: "You can configure up to five advanced event selectors for each event data
/// store", and a selector list an update states has to satisfy the same per-selector
/// rules `PutEventSelectors` applies.
#[tokio::test]
async fn an_update_bounds_the_selector_list_it_stores() {
    let limit = service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::
        ADVANCED_SELECTORS_PER_EVENT_DATA_STORE_LIMIT;

    /// `count` selectors, each selecting one event category by equality and scoping it
    /// to one bucket.
    fn selectors(count: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|index| {
                    json!({
                        "Name": format!("selector-{index}"),
                        "FieldSelectors": [
                            {"Field": "eventCategory", "Equals": ["Data"]},
                            {
                                "Field": "resources.ARN",
                                "StartsWith": [format!("arn:aws:s3:::bucket-{index}/")],
                            },
                        ],
                    })
                })
                .collect(),
        )
    }

    // (case, the update, expected code or "" for a success)
    let rows: Vec<(String, Value, &str)> = vec![
        (
            format!("{limit} selectors: the documented bound"),
            json!({"AdvancedEventSelectors": selectors(limit)}),
            "",
        ),
        (
            format!("{} selectors: one past the bound", limit + 1),
            json!({"AdvancedEventSelectors": selectors(limit + 1)}),
            "InvalidEventSelectorsException",
        ),
        (
            "a field selector stating no condition at all".to_string(),
            json!({"AdvancedEventSelectors": [{
                "Name": "nothing",
                "FieldSelectors": [{"Field": "eventCategory"}],
            }]}),
            "InvalidEventSelectorsException",
        ),
    ];

    for (index, (case, change, expected)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let created = create(
            &emulator,
            &format!("selectors-{index}"),
            &json!({"AdvancedEventSelectors": selectors(1)}),
        )
        .await;
        let id = store_id(&created);
        let reply = update(&emulator, &id, change).await;
        if expected.is_empty() {
            reply.expect_ok(case);
            let read = get(&emulator, &id).await;
            let read = read.expect_ok(case).clone();
            assert_eq!(
                read.get("AdvancedEventSelectors"),
                change.get("AdvancedEventSelectors"),
                "case {case}: expected the stored selectors to be the request's, got {read:?}"
            );
            continue;
        }
        reply.expect_error(case, 400, expected);
        // The refused update left the selectors the store already carried.
        let read = get(&emulator, &id).await;
        let read = read.expect_ok(case).clone();
        assert_members(case, &created, &read, &json!({}));
    }
}

/// RULE: an identifier is an ARN or the ID suffix of one; one naming no store of this
/// account and Region is `EventDataStoreNotFoundException` over the ARN it addressed,
/// and a value written as an ARN that is not an event data store ARN is
/// `EventDataStoreARNInvalidException`.
///
/// The recorded unknown-store ARN this issue was filed on is one row of the first
/// table.
#[tokio::test]
async fn an_identifier_naming_no_store_is_refused_over_the_arn_it_addressed() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let absent = "11111111-1111-1111-1111-111111111111";
    // A store that DOES exist, so a Region-mismatch row is about the Region.
    let created = create(&emulator, "present", &json!({})).await;
    let present_id = store_id(&created);

    // (case, identifier, the ARN the refusal must name)
    let rows: Vec<(&str, String, String)> = vec![
        (
            // `cloudtrail-event-data-store-notfound-crud-error-semantics#task#2`
            // addresses this ARN in us-west-2.
            "the recorded unknown-store ARN",
            format!("arn:aws:cloudtrail:us-west-2:{account}:eventdatastore/{absent}"),
            format!("arn:aws:cloudtrail:us-west-2:{account}:eventdatastore/{absent}"),
        ),
        (
            "an unused id suffix",
            absent.to_string(),
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{absent}"),
        ),
        (
            "an unused ARN in the caller's Region",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{absent}"),
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{absent}"),
        ),
        (
            "an existing store addressed in another Region",
            format!("arn:aws:cloudtrail:ap-south-1:{account}:eventdatastore/{present_id}"),
            format!("arn:aws:cloudtrail:ap-south-1:{account}:eventdatastore/{present_id}"),
        ),
    ];

    for (case, identifier, addressed) in &rows {
        // An update needs one optional member to get as far as the identifier.
        let reply = update(&emulator, identifier, &json!({"RetentionPeriod": 30})).await;
        reply.expect_error(case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
        assert_eq!(
            reply.message(),
            format!("The event data store {addressed} was not found."),
            "case {case}: the refusal must name the ARN it addressed, got {:?}",
            reply.message()
        );
    }

    // A value WRITTEN as an ARN has to BE an event data store ARN, and a bare value
    // has to be an id this service could have minted: the recorded answer to this
    // operation naming `not-an-arn` is `EventDataStoreARNInvalidException` over the
    // ARN that value completes to
    // (`cloudtrail-lake-read-surface-and-error-contracts#task#7`). The rows differ in
    // which part of the form is broken.
    let malformed: Vec<(&str, String)> = vec![
        ("a bare value that is no minted id", "not-an-arn".to_string()),
        (
            "a bare value one group short of a minted id",
            "11111111-1111-1111-111111111111".to_string(),
        ),
        (
            "another resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:trail/demo"),
        ),
        (
            "another service",
            format!("arn:aws:s3:{HOME_REGION}:{account}:eventdatastore/{absent}"),
        ),
        (
            "too few fields",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}"),
        ),
        (
            "no Region",
            format!("arn:aws:cloudtrail::{account}:eventdatastore/{absent}"),
        ),
        (
            "no account",
            format!("arn:aws:cloudtrail:{HOME_REGION}::eventdatastore/{absent}"),
        ),
        (
            "no id suffix",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/"),
        ),
    ];
    for (case, identifier) in &malformed {
        let reply = update(&emulator, identifier, &json!({"RetentionPeriod": 30})).await;
        reply.expect_error(case, 400, "EventDataStoreARNInvalidException");
        // The refusal names the ARN the value addressed: an ARN as given, and a bare
        // value completed into the ARN it names in the Region the call was made in.
        let addressed = if identifier.starts_with("arn:") {
            identifier.clone()
        } else {
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{identifier}")
        };
        assert_eq!(
            reply.message(),
            format!("The event data store ARN {addressed} format is not valid."),
            "case {case}: expected the refusal to name the ARN it addressed, got {:?}",
            reply.message()
        );
    }

    // The control: the store those identifiers failed to name is still updatable by
    // the identifier that does name it.
    update(&emulator, &present_id, &json!({"RetentionPeriod": 30}))
        .await
        .expect_ok("the control");
}

/// RULE: a store awaiting deletion holds no configuration left to change, which the
/// modelled `InactiveEventDataStoreException` ("The event data store is inactive")
/// reports — and the refused update changes nothing.
#[tokio::test]
async fn a_store_awaiting_deletion_cannot_be_updated() {
    let emulator = Emulator::new();
    let created = create(&emulator, "pending", &json!({"RetentionPeriod": 30})).await;
    let id = store_id(&created);
    emulator
        .call("DeleteEventDataStore", &json!({"EventDataStore": id.clone()}))
        .await
        .expect_ok("DeleteEventDataStore");

    let case = "UpdateEventDataStore on a store awaiting deletion";
    update(&emulator, &id, &json!({"RetentionPeriod": 90}))
        .await
        .expect_error(case, 400, "InactiveEventDataStoreException");

    let read = get(&emulator, &id).await;
    let read = read.expect_ok(case).clone();
    assert_members(case, &created, &read, &json!({}));
}
