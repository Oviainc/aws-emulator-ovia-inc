// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Time to Live surface: `UpdateTimeToLive`, the reader it
//! round-trips through (`DescribeTimeToLive`), and the item expiries the stored setting
//! brings about.
//!
//! The rules pinned here:
//!
//! 1. the specification a call carries is STORED on the table and reported back by both
//!    operations ("A successful UpdateTimeToLive call returns the current
//!    TimeToLiveSpecification" — the `UpdateTimeToLive` doc), through the transient
//!    status the asynchronous change is reported in ("It can take up to one hour for the
//!    change to fully process") and into the settled one; a table that has never been
//!    given a specification reports TTL off with no attribute;
//! 2. this API ENABLES OR DISABLES TTL, so a specification asking for the state the
//!    table is already in is refused, and so is one naming an attribute other than the
//!    one TTL is in force for — which is told which attribute that is;
//! 3. a table's TTL setting takes ONE change per modification interval ("Any additional
//!    UpdateTimeToLive calls for the same table during this one hour duration result in a
//!    ValidationException"): a second change is refused while the first is still
//!    processing AND once it has settled, and stores nothing;
//! 4. while TTL is on, an item whose TTL attribute holds an epoch-SECONDS number already
//!    in the past is deleted once the clock makes that deletion due ("If the epoch time
//!    value stored in the attribute is less than the current time, the item is marked as
//!    expired and subsequently deleted"), and nothing else is: not an item whose value is
//!    in the future, not one whose attribute is of another type, not one that carries no
//!    such attribute, and not an item of a table whose TTL is off.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and each rule is asserted over a
//! SET of inputs — the attribute name, the order the toggles come in, the type and the
//! magnitude of the value an item expires by — rather than one example.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::ops::types::TIME_TO_LIVE_STATUS_VALUES;
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    time_to_live_attribute_mismatch, STATUS_TRANSITION, TIME_TO_LIVE_ALREADY_DISABLED_MESSAGE,
    TIME_TO_LIVE_ALREADY_ENABLED_MESSAGE, TIME_TO_LIVE_MODIFIED_TOO_OFTEN_MESSAGE,
    TTL_DELETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};

/// The table's one key attribute, which no row varies.
const PARTITION_KEY: &str = "Part";

/// One row of the repeated-state table: what the row is called, the toggles that put the
/// table into the state under test, the toggle that repeats that state, the sentence the
/// repeat is refused with, and the status still reported afterwards.
type RepeatCase<'a> = (&'a str, &'a [(bool, &'a str)], bool, &'a str, &'a str);

/// One row of the backup table: what the row is called, the toggles applied before the
/// backup is taken, and the status and attribute the backup then records.
type BackupCase<'a> = (&'a str, &'a [(bool, &'a str)], &'a str, Value);

/// Wait out one transient span, so whatever the clock owes a table or its TTL setting is
/// due.
async fn settle() {
    tokio::time::sleep(STATUS_TRANSITION + Duration::from_millis(150)).await;
}

/// Wait until every expiry the clock has made due has been swept.
async fn settle_expiries() {
    tokio::time::sleep(2 * TTL_DELETION_DELAY + Duration::from_millis(150)).await;
}

/// Create one on-demand table and wait for the `CREATING` -> `ACTIVE` transition: TTL is
/// a table setting, so it is configured on a settled table.
async fn create_active_table(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": PARTITION_KEY, "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": PARTITION_KEY, "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    settle().await;
}

/// Send one `UpdateTimeToLive` and return its HTTP status and parsed body.
async fn set_ttl(
    router: &MultiServiceRouter,
    name: &str,
    enabled: bool,
    attribute: &str,
) -> (u16, Value) {
    call(
        router,
        "UpdateTimeToLive",
        json!({
            "TableName": name,
            "TimeToLiveSpecification": {"Enabled": enabled, "AttributeName": attribute},
        }),
    )
    .await
}

/// The same, for a call that must succeed.
async fn set_ttl_ok(
    router: &MultiServiceRouter,
    name: &str,
    enabled: bool,
    attribute: &str,
) -> Value {
    let (status, body) = set_ttl(router, name, enabled, attribute).await;
    assert_eq!(
        status, 200,
        "case set TTL {enabled} on {attribute:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The `TimeToLiveDescription` the reader reports for `name`.
async fn described_ttl(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "DescribeTimeToLive", json!({"TableName": name}))
        .await
        .get("TimeToLiveDescription")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Now, in the unit the TTL attribute counts in: "The epoch time format is the number of
/// seconds elapsed since 12:00:00 AM January 1, 1970 UTC" (the `UpdateTimeToLive` doc).
fn epoch_seconds_now() -> i64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs();
    i64::try_from(since).unwrap_or(i64::MAX)
}

/// Write one item under `key`, carrying `attribute` when the row names a value for it.
async fn put_item(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    attribute: &str,
    value: Option<Value>,
) {
    let mut item = json!({PARTITION_KEY: {"S": key}});
    if let Some(value) = value {
        item[attribute] = value;
    }
    ok(router, "PutItem", json!({"TableName": name, "Item": item})).await;
}

/// Whether `key` is still stored in `name`, read back through `GetItem`.
async fn item_present(router: &MultiServiceRouter, name: &str, key: &str) -> bool {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": name, "Key": {PARTITION_KEY: {"S": key}}}),
    )
    .await;
    read.get("Item").is_some_and(|item| item.is_object())
}

#[tokio::test]
async fn the_specification_a_table_is_given_is_the_one_both_operations_report() {
    let router = emulator();

    // Rows differ in the TTL attribute's name: each published bound of the modelled
    // 1..255 length (`smithy_query(query="shape", shape="TimeToLiveAttributeName")`), a
    // conventional name, and one carrying the punctuation an attribute name can hold.
    let longest = "e".repeat(255);
    let cases: [(&str, &str); 4] = [
        ("the shortest name", "t"),
        ("a conventional name", "expires_at"),
        ("a name with punctuation", "ttl.v2-final"),
        ("the longest name", longest.as_str()),
    ];

    for (case, attribute) in cases {
        let name = format!("Expiring{}", case.replace(' ', ""));
        create_active_table(&router, &name).await;

        // A table that has never been given a TTL specification reports TTL off, with no
        // attribute: `CreateTable` carries no TTL member.
        let fresh = described_ttl(&router, &name).await;
        assert_eq!(
            (
                fresh["TimeToLiveStatus"].clone(),
                fresh["AttributeName"].clone()
            ),
            (json!("DISABLED"), Value::Null),
            "case {case}: a new table has no TTL, got {fresh:?}"
        );

        // ENABLE: the response is the specification stored, and the reader reports it in
        // the transient status until the clock settles it.
        let enabled = set_ttl_ok(&router, &name, true, attribute).await;
        assert_eq!(
            enabled["TimeToLiveSpecification"].clone(),
            json!({"Enabled": true, "AttributeName": attribute}),
            "case {case}: the update returns the specification it stored, got {enabled}"
        );
        let enabling = described_ttl(&router, &name).await;
        assert_eq!(
            (
                enabling["TimeToLiveStatus"].clone(),
                enabling["AttributeName"].clone()
            ),
            (json!("ENABLING"), json!(attribute)),
            "case {case}: the change is still processing, got {enabling:?}"
        );
        settle().await;
        let in_force = described_ttl(&router, &name).await;
        assert_eq!(
            (
                in_force["TimeToLiveStatus"].clone(),
                in_force["AttributeName"].clone()
            ),
            (json!("ENABLED"), json!(attribute)),
            "case {case}: the settled setting is the one stored, got {in_force:?}"
        );
        // A read stores nothing, so reading again reports the same description.
        let again = described_ttl(&router, &name).await;
        assert_eq!(
            again, in_force,
            "case {case}: a second read reports the same description, got {again:?}"
        );

        // The other direction of the round trip — the disable and the `DISABLING` status it
        // leaves — cannot follow on this table: the setting takes one change per
        // modification interval, which `a_second_change_inside_the_modification_interval_is_refused`
        // pins.
    }
}

#[tokio::test]
async fn every_status_a_description_reports_is_one_the_model_declares() {
    let router = emulator();
    create_active_table(&router, "Declared").await;

    // The status member is an enum, so what a description reports must be a value from
    // the model's own table rather than a spelling this emulator invented. Rows span the
    // states one table's setting can be read in within the modification interval: never
    // set, becoming on, and on.
    let mut seen: Vec<String> = Vec::new();
    seen.push(status_of(&described_ttl(&router, "Declared").await));
    set_ttl_ok(&router, "Declared", true, "ttl").await;
    seen.push(status_of(&described_ttl(&router, "Declared").await));
    settle().await;
    seen.push(status_of(&described_ttl(&router, "Declared").await));

    for status in &seen {
        assert!(
            TIME_TO_LIVE_STATUS_VALUES.contains(&status.as_str()),
            "case {status:?}: every reported status is one the model declares, got {seen:?}"
        );
    }
    assert_eq!(
        seen,
        vec![
            "DISABLED".to_string(),
            "ENABLING".to_string(),
            "ENABLED".to_string(),
        ],
        "case the whole walk: each state is reported in turn, got {seen:?}"
    );
}

/// The status one description reports.
fn status_of(description: &Value) -> String {
    description["TimeToLiveStatus"]
        .as_str()
        .unwrap_or("<no status>")
        .to_string()
}

#[tokio::test]
async fn a_specification_asking_for_the_state_already_in_force_is_refused() {
    let router = emulator();

    // Rows differ in how the table reached the state asked for again: TTL off because it
    // has never been set, and on because it was just turned on. Each repeat is refused
    // with the FIXED sentence for that state — the state the table is in is permanent for
    // the request as it stands, so it is answered ahead of the modification interval — and
    // the refusal stores nothing. (A table whose TTL was turned OFF again cannot be reached
    // inside the interval, so it has no row here.)
    let cases: [RepeatCase; 2] = [
        (
            "off because it was never set",
            &[],
            false,
            TIME_TO_LIVE_ALREADY_DISABLED_MESSAGE,
            "DISABLED",
        ),
        (
            "on because it was turned on",
            &[(true, "ttl")],
            true,
            TIME_TO_LIVE_ALREADY_ENABLED_MESSAGE,
            "ENABLED",
        ),
    ];

    for (case, toggles, repeated, expected, reported) in cases {
        let name = format!("Repeated{}", case.replace(' ', ""));
        create_active_table(&router, &name).await;
        for (enabled, attribute) in toggles {
            set_ttl_ok(&router, &name, *enabled, attribute).await;
            settle().await;
        }

        let (status, body) = set_ttl(&router, &name, repeated, "ttl").await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ValidationException".to_string(),
                expected.to_string()
            ),
            "case {case}: asking for the state in force is refused, got {status} with {body}"
        );

        let described = described_ttl(&router, &name).await;
        assert_eq!(
            described["TimeToLiveStatus"].clone(),
            json!(reported),
            "case {case}: the refusal stored nothing, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_specification_naming_another_attribute_while_ttl_is_on_is_refused() {
    let router = emulator();
    create_active_table(&router, "Renamed").await;
    set_ttl_ok(&router, "Renamed", true, "expires_at").await;
    settle().await;

    // Rows differ in the attribute asked for and in whether the call was enabling or
    // disabling TTL: the attribute a table expires items by is not something this API
    // moves, so every one of them is refused and told which attribute is in force.
    let cases: [(&str, bool, &str); 4] = [
        ("an enable naming another attribute", true, "ttl"),
        ("a disable naming another attribute", false, "ttl"),
        (
            "an enable differing only in case",
            true,
            "Expires_At",
        ),
        (
            "an enable naming a longer attribute",
            true,
            "expires_at_utc",
        ),
    ];

    for (case, enabled, attribute) in cases {
        let (status, body) = set_ttl(&router, "Renamed", enabled, attribute).await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ValidationException".to_string(),
                time_to_live_attribute_mismatch("expires_at")
            ),
            "case {case}: the attribute in force is reported back, got {status} with {body}"
        );

        let described = described_ttl(&router, "Renamed").await;
        assert_eq!(
            (
                described["TimeToLiveStatus"].clone(),
                described["AttributeName"].clone()
            ),
            (json!("ENABLED"), json!("expires_at")),
            "case {case}: the refusal left the stored setting alone, got {described:?}"
        );
    }
}

/// Rule 3: a table's TTL setting takes ONE change per modification interval. "It can take
/// up to one hour for the change to fully process. Any additional UpdateTimeToLive calls
/// for the same table during this one hour duration result in a ValidationException" (the
/// `UpdateTimeToLive` doc), so the second change is refused both while the first is still
/// processing and once its status has settled — the interval runs from the accepted change,
/// not from the status it left behind
/// (`service_com_amazonaws_dynamodb_dynamodb_20120810::state::TTL_MODIFICATION_INTERVAL`).
#[tokio::test]
async fn a_second_change_inside_the_modification_interval_is_refused() {
    let router = emulator();

    // Rows differ in the change that arrives inside the interval: the opposite toggle, one
    // that re-states what the first asked for, and one naming another attribute. The
    // interval refuses a change the setting's own state would otherwise take; the rows
    // whose second call is refused on its own merits instead (asking for the state in
    // force, or naming another attribute while TTL is on) carry that sentence, because
    // those refusals are permanent for the request as it stands.
    let cases: [(&str, bool, &str, Option<String>); 3] = [
        ("a disable right after an enable", false, "ttl", None),
        (
            "an enable right after an enable",
            true,
            "ttl",
            Some(TIME_TO_LIVE_ALREADY_ENABLED_MESSAGE.to_string()),
        ),
        (
            "a rename right after an enable",
            true,
            "other",
            Some(time_to_live_attribute_mismatch("ttl")),
        ),
    ];

    for (case, second, attribute, settled_merits) in cases {
        let name = format!("Interval{}", case.replace(' ', ""));
        create_active_table(&router, &name).await;
        set_ttl_ok(&router, &name, true, "ttl").await;

        // While the first change is still processing, every one of them is refused on the
        // interval: the setting is in no state the second call could be refused for yet.
        let (status, body) = set_ttl(&router, &name, second, attribute).await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ValidationException".to_string(),
                TIME_TO_LIVE_MODIFIED_TOO_OFTEN_MESSAGE.to_string()
            ),
            "case {case}: a change inside the interval is refused, got {status} with {body}"
        );
        let during = described_ttl(&router, &name).await;
        assert_eq!(
            during["TimeToLiveStatus"].clone(),
            json!("ENABLING"),
            "case {case}: the refusal stored nothing, got {during:?}"
        );

        // The status settles long before the interval is out, and the second change is
        // STILL refused: a change the status no longer refuses is refused on the interval.
        settle().await;
        let settled = described_ttl(&router, &name).await;
        assert_eq!(
            settled["TimeToLiveStatus"].clone(),
            json!("ENABLED"),
            "case {case}: the first change settles on its own clock, got {settled:?}"
        );
        let (status, body) = set_ttl(&router, &name, second, attribute).await;
        let expected = settled_merits
            .unwrap_or_else(|| TIME_TO_LIVE_MODIFIED_TOO_OFTEN_MESSAGE.to_string());
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (400, "ValidationException".to_string(), expected.clone()),
            "case {case}: once the status settled the change is still refused with \
             {expected:?}, got {status} with {body}"
        );
        let after = described_ttl(&router, &name).await;
        assert_eq!(
            (
                after["TimeToLiveStatus"].clone(),
                after["AttributeName"].clone()
            ),
            (json!("ENABLED"), json!("ttl")),
            "case {case}: the second refusal stored nothing either, got {after:?}"
        );
    }
}

#[tokio::test]
async fn an_item_is_deleted_only_once_its_own_epoch_second_has_made_it_due() {
    let router = emulator();
    create_active_table(&router, "Sweeping").await;
    set_ttl_ok(&router, "Sweeping", true, "ttl").await;
    settle().await;

    let now = epoch_seconds_now();
    // Rows differ in the value the TTL attribute holds — its magnitude, its sign, its
    // precision and its TYPE — and in whether it is there at all. Only an epoch-SECONDS
    // NUMBER already in the past expires an item: "TTL compares the current time in epoch
    // time format to the time stored in the TTL attribute of an item" (the
    // `UpdateTimeToLive` doc), and no other type states such a time.
    let cases: [(&str, Option<Value>, bool); 10] = [
        ("an hour in the past", Some(json!({"N": (now - 3600).to_string()})), false),
        ("a few seconds in the past", Some(json!({"N": (now - 5).to_string()})), false),
        ("the first second of the epoch", Some(json!({"N": "1"})), false),
        ("before the epoch", Some(json!({"N": format!("-{}", now)})), false),
        (
            "a fractional second in the past",
            Some(json!({"N": format!("{}.25", now - 10)})),
            false,
        ),
        ("an hour in the future", Some(json!({"N": (now + 3600).to_string()})), true),
        (
            "this instant in milliseconds, so far in the future",
            Some(json!({"N": (now * 1000).to_string()})),
            true,
        ),
        ("a past instant as a string", Some(json!({"S": (now - 3600).to_string()})), true),
        (
            "a past instant inside a number set",
            Some(json!({"NS": [(now - 3600).to_string()]})),
            true,
        ),
        ("no TTL attribute at all", None, true),
    ];

    for (case, value, _) in &cases {
        let key = case.replace(' ', "");
        put_item(&router, "Sweeping", &key, "ttl", value.clone()).await;
        // Nothing is deleted the moment it is written: an item that has expired but whose
        // deletion is not due yet "will still show up in reads, queries, and scans".
        assert!(
            item_present(&router, "Sweeping", &key).await,
            "case {case:?}: a freshly written item is served, expected present, got absent"
        );
    }

    settle_expiries().await;

    for (case, _, survives) in &cases {
        let key = case.replace(' ', "");
        let present = item_present(&router, "Sweeping", &key).await;
        assert_eq!(
            present, *survives,
            "case {case:?}: expected present={survives}, got present={present}"
        );
    }
}

#[tokio::test]
async fn items_of_a_table_whose_ttl_is_off_never_expire() {
    let router = emulator();

    // Rows differ in why the table expires nothing: TTL never set, and TTL enabled over
    // ANOTHER attribute than the one the item carries. The item's own value is the same
    // long-past epoch second in each, so what differs is the table's setting and not the
    // item.
    let now = epoch_seconds_now();
    let cases: [(&str, &[(bool, &str)]); 2] = [
        ("TTL was never set", &[]),
        ("TTL is on for another attribute", &[(true, "other")]),
    ];

    for (case, toggles) in cases {
        let name = format!("Kept{}", case.replace(' ', ""));
        create_active_table(&router, &name).await;
        for (enabled, attribute) in toggles {
            set_ttl_ok(&router, &name, *enabled, attribute).await;
            settle().await;
        }
        put_item(
            &router,
            &name,
            "only",
            "ttl",
            Some(json!({"N": (now - 3600).to_string()})),
        )
        .await;

        settle_expiries().await;
        assert!(
            item_present(&router, &name, "only").await,
            "case {case:?}: nothing expires it, expected present, got absent"
        );
    }
}

#[tokio::test]
async fn a_backup_records_the_ttl_setting_its_table_had_when_it_was_taken() {
    let router = emulator();

    // `SourceTableFeatureDetails$TimeToLiveDescription` is "the description of the Time
    // to Live (TTL) status on the specified table" as the backup recorded it, so rows
    // differ in the setting in force when the backup was taken: none ever set, and on.
    let cases: [BackupCase; 2] = [
        ("TTL was never set", &[], "DISABLED", Value::Null),
        ("TTL is on", &[(true, "expires_at")], "ENABLED", json!("expires_at")),
    ];

    for (case, toggles, status, attribute) in cases {
        let name = format!("BackedUp{}", case.replace(' ', ""));
        create_active_table(&router, &name).await;
        for (enabled, named) in toggles {
            set_ttl_ok(&router, &name, *enabled, named).await;
            settle().await;
        }

        let details = ok(
            &router,
            "CreateBackup",
            json!({"TableName": name, "BackupName": format!("{name}Backup")}),
        )
        .await;
        let arn = details["BackupDetails"]["BackupArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        settle().await;

        let described = ok(&router, "DescribeBackup", json!({"BackupArn": arn})).await;
        let recorded =
            &described["BackupDescription"]["SourceTableFeatureDetails"]["TimeToLiveDescription"];
        assert_eq!(
            (
                recorded["TimeToLiveStatus"].clone(),
                recorded["AttributeName"].clone()
            ),
            (json!(status), attribute),
            "case {case:?}: the backup records the setting the table had, got {described}"
        );
    }
}

#[tokio::test]
async fn a_table_that_is_not_there_is_refused_by_both_operations() {
    let router = emulator();
    create_active_table(&router, "Present").await;

    // Rows differ in why the table is not there: never created, and created then deleted.
    // Both operations model `ResourceNotFoundException` — "The operation tried to access a
    // nonexistent table or index".
    create_active_table(&router, "Removed").await;
    ok(&router, "DeleteTable", json!({"TableName": "Removed"})).await;
    settle().await;

    for case in ["Absent", "Removed"] {
        for operation in ["DescribeTimeToLive", "UpdateTimeToLive"] {
            let mut request = json!({"TableName": case});
            if operation == "UpdateTimeToLive" {
                request["TimeToLiveSpecification"] =
                    json!({"Enabled": true, "AttributeName": "ttl"});
            }
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                (status, error_code(&body), message(&body)),
                (
                    400,
                    "ResourceNotFoundException".to_string(),
                    format!("Requested resource not found: Table: {case} not found")
                ),
                "case {case} on {operation}: a table that is not there is refused, got {status} \
                 with {body}"
            );
        }
    }

    // The table that IS there is still served, so the refusals above are about the table
    // named and not about the operations.
    let served = set_ttl_ok(&router, "Present", true, "ttl").await;
    assert_eq!(
        served["TimeToLiveSpecification"]["AttributeName"].clone(),
        json!("ttl"),
        "case the present table: it is served, got {served}"
    );
}
