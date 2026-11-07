// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CloudTrail Lake event data store lifecycle.
//!
//! Every case goes over the wire through the shared [`Emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted here is what a client sees, including the HTTP
//! status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `CreateEventDataStore` mints a store and persists it: the id, its ARN, the home
//!   Region and the timestamps are the service's, every other member is the request's
//!   or the documented default, and `GetEventDataStore` reports the stored values
//!   whether it is addressed by ARN or by the ARN's id suffix;
//! * the retention period a store may carry is the band its `BillingMode` documents
//!   (`EXTENDABLE_RETENTION_PRICING` 7..3653 days, default 366;
//!   `FIXED_RETENTION_PRICING` 7..2557 days, default 2557);
//! * a store is created in the transient `CREATED` status and the CLOCK — not another
//!   call — moves it to the status it was created for;
//! * `DeleteEventDataStore` moves the store into `PENDING_DELETION` with the
//!   documented seven-day wait period, outlasting any transition the clock still
//!   owed the store, and refuses — changing nothing — a store whose termination
//!   protection is on or whose Lake query federation is enabled;
//! * an identifier that names no store of this account and Region is
//!   `EventDataStoreNotFoundException` naming the ARN it addressed, and a value
//!   WRITTEN as an ARN that is not an event data store ARN is
//!   `EventDataStoreARNInvalidException`.
//!
//! Every modelled refusal is asserted at the status CloudTrail really sends, which for
//! `EventDataStoreNotFoundException` is HTTP 400 rather than the 404 its `@httpError`
//! declares (the recorded wire responses this issue was filed from) — see
//! `crate::scaffolded::Scaffolded::ModeledWithStatus`.

use serde_json::{json, Value};

// The emulator-under-test harness is shared with this crate's other suites; see
// `tests/all/emulator.rs`.
use crate::emulator::{Emulator, HOME_REGION};

/// The status the model's `EventDataStoreStatus` enum gives a store that has just been
/// created, before the clock moves it on.
const CREATED: &str = "CREATED";
/// See [`CREATED`].
const ENABLED: &str = "ENABLED";
/// See [`CREATED`].
const PENDING_DELETION: &str = "PENDING_DELETION";
/// See [`CREATED`].
const STOPPED_INGESTION: &str = "STOPPED_INGESTION";

/// The model's `BillingMode` enum values.
const EXTENDABLE: &str = "EXTENDABLE_RETENTION_PRICING";
/// See [`EXTENDABLE`].
const FIXED: &str = "FIXED_RETENTION_PRICING";

/// Long enough for the provisioning span the state module owns to come due.
fn past_provisioning() -> std::time::Duration {
    service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::EVENT_DATA_STORE_PROVISIONING
        + std::time::Duration::from_millis(250)
}

/// Long enough for the ingestion span the state module owns to come due.
fn past_settle() -> std::time::Duration {
    service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::INGESTION_SETTLE_AFTER
        + std::time::Duration::from_millis(250)
}

/// Long enough for the federation span the state module owns to come due.
fn past_federation_settle() -> std::time::Duration {
    service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::FEDERATION_SETTLE_AFTER
        + std::time::Duration::from_millis(250)
}

/// The string member `name` of `body`, or `""`.
fn text(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row of the creation table: the request, and what the created store must
/// therefore report.
struct CreateCase {
    case: &'static str,
    request: Value,
    retention_period: i64,
    billing_mode: &'static str,
    multi_region: bool,
    organization: bool,
    termination_protection: bool,
    /// The status the clock owes the store once its provisioning span elapses.
    settled_status: &'static str,
}

/// The creation table: rows differ along the members `CreateEventDataStore` reads —
/// which are present, the billing mode, the retention period, the flags, whether
/// ingestion starts, and how many tags and selectors the request carries.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "every optional member absent",
            request: json!({"Name": "lake-defaults"}),
            // "The default value is EXTENDABLE_RETENTION_PRICING" with "default
            // retention period ... 366 days".
            retention_period: 366,
            billing_mode: EXTENDABLE,
            multi_region: false,
            organization: false,
            termination_protection: false,
            settled_status: ENABLED,
        },
        CreateCase {
            case: "fixed pricing, its own default retention",
            request: json!({"Name": "lake-fixed", "BillingMode": FIXED}),
            // "FIXED_RETENTION_PRICING ... The default retention period for this
            // billing mode is 2557 days."
            retention_period: 2557,
            billing_mode: FIXED,
            multi_region: false,
            organization: false,
            termination_protection: false,
            settled_status: ENABLED,
        },
        CreateCase {
            case: "shortest retention, ingestion not started",
            request: json!({
                "Name": "lake_min.7",
                "RetentionPeriod": 7,
                "StartIngestion": false,
            }),
            retention_period: 7,
            billing_mode: EXTENDABLE,
            multi_region: false,
            organization: false,
            termination_protection: false,
            settled_status: STOPPED_INGESTION,
        },
        CreateCase {
            case: "longest retention, every flag on, tags and selectors",
            request: json!({
                "Name": "lake-max-3653",
                "RetentionPeriod": 3653,
                "MultiRegionEnabled": true,
                "OrganizationEnabled": true,
                "TerminationProtectionEnabled": true,
                "KmsKeyId": "alias/lake-key",
                "TagsList": [{"Key": "env", "Value": "test"}, {"Key": "owner", "Value": "ct"}],
                "AdvancedEventSelectors": [{
                    "Name": "management events",
                    "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Management"]}],
                }],
            }),
            retention_period: 3653,
            billing_mode: EXTENDABLE,
            multi_region: true,
            organization: true,
            termination_protection: true,
            settled_status: ENABLED,
        },
    ]
}

/// A created store is persisted, reported in `CREATED`, reachable by ARN and by the
/// ARN's id suffix, and reports the request's values back on every read.
#[tokio::test]
async fn created_event_data_stores_are_persisted_and_read_back() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let mut settled: Vec<(&'static str, String, &'static str)> = Vec::new();

    for case in create_cases() {
        let created = emulator
            .call("CreateEventDataStore", &case.request)
            .await
            .expect_ok(case.case)
            .clone();
        let arn = text(&created, "EventDataStoreArn");
        let id = arn.rsplit('/').next().unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{id}"),
            "case {}: expected an event data store ARN in the caller's Region, got {arn:?}",
            case.case
        );
        // A v4 UUID is 36 characters of lowercase hex and four dashes.
        assert_eq!(
            (id.len(), id.matches('-').count()),
            (36, 4),
            "case {}: expected a UUID id suffix, got {id:?}",
            case.case
        );
        assert_eq!(
            text(&created, "Status"),
            CREATED,
            "case {}: expected a new store in {CREATED}, got {created:?}",
            case.case
        );

        // Both identifier forms the model documents ("the ARN (or the ID suffix of
        // the ARN)") name the same store, and both report what was stored.
        for (form, identifier) in [("ARN", arn.clone()), ("id suffix", id.clone())] {
            let read = emulator
                .call(
                    "GetEventDataStore",
                    &json!({"EventDataStore": identifier}),
                )
                .await
                .expect_ok(case.case)
                .clone();
            let label = format!("{} read by {form}", case.case);
            assert_eq!(
                text(&read, "EventDataStoreArn"),
                arn,
                "case {label}: expected the stored ARN, got {read:?}"
            );
            assert_eq!(
                text(&read, "Name"),
                text(&created, "Name"),
                "case {label}: expected the created name, got {read:?}"
            );
            assert_eq!(
                read.get("RetentionPeriod").and_then(Value::as_i64),
                Some(case.retention_period),
                "case {label}: expected RetentionPeriod {}, got {read:?}",
                case.retention_period
            );
            assert_eq!(
                text(&read, "BillingMode"),
                case.billing_mode,
                "case {label}: expected BillingMode {}, got {read:?}",
                case.billing_mode
            );
            assert_eq!(
                read.get("MultiRegionEnabled").and_then(Value::as_bool),
                Some(case.multi_region),
                "case {label}: expected MultiRegionEnabled {}, got {read:?}",
                case.multi_region
            );
            assert_eq!(
                read.get("OrganizationEnabled").and_then(Value::as_bool),
                Some(case.organization),
                "case {label}: expected OrganizationEnabled {}, got {read:?}",
                case.organization
            );
            assert_eq!(
                read.get("TerminationProtectionEnabled").and_then(Value::as_bool),
                Some(case.termination_protection),
                "case {label}: expected TerminationProtectionEnabled {}, got {read:?}",
                case.termination_protection
            );
            // Lake query federation is off until `EnableFederation` turns it on.
            assert_eq!(
                text(&read, "FederationStatus"),
                "DISABLED",
                "case {label}: expected federation DISABLED, got {read:?}"
            );
            assert_eq!(
                read.get("KmsKeyId"),
                case.request.get("KmsKeyId"),
                "case {label}: expected the request's KmsKeyId, got {read:?}"
            );
            // A list the store does not hold stays absent on the wire rather than
            // becoming an empty list.
            assert_eq!(
                read.get("AdvancedEventSelectors"),
                case.request.get("AdvancedEventSelectors"),
                "case {label}: expected the request's selectors, got {read:?}"
            );
            assert!(
                read.get("CreatedTimestamp").is_some(),
                "case {label}: expected a CreatedTimestamp, got {read:?}"
            );
        }
        // `TagsList` is reported by the create response (`GetEventDataStore` models
        // no tag member), and stays absent when the request carried none.
        assert_eq!(
            created.get("TagsList"),
            case.request.get("TagsList"),
            "case {}: expected the request's tags, got {created:?}",
            case.case
        );
        settled.push((case.case, arn, case.settled_status));
    }

    // The CLOCK, not another call, moves a store out of `CREATED`.
    tokio::time::sleep(past_provisioning()).await;
    for (case, arn, expected) in settled {
        let read = emulator
            .call("GetEventDataStore", &json!({"EventDataStore": arn}))
            .await
            .expect_ok(case)
            .clone();
        assert_eq!(
            text(&read, "Status"),
            expected,
            "case {case}: expected {expected} once the provisioning span elapsed, got {read:?}"
        );
    }
}

/// A second store of the same name in the same Region is a conflict; the same name in
/// another Region is a different store.
#[tokio::test]
async fn an_event_data_store_name_is_unique_per_region() {
    let emulator = Emulator::new();
    let first = emulator
        .call("CreateEventDataStore", &json!({"Name": "shared-name"}))
        .await
        .expect_ok("CreateEventDataStore")
        .clone();
    emulator
        .call("CreateEventDataStore", &json!({"Name": "shared-name"}))
        .await
        .expect_error(
            "CreateEventDataStore twice in one Region",
            400,
            "EventDataStoreAlreadyExistsException",
        );
    let elsewhere = emulator
        .call_in(
            "CreateEventDataStore",
            "eu-west-1",
            &json!({"Name": "shared-name"}),
        )
        .await
        .expect_ok("CreateEventDataStore in another Region")
        .clone();
    assert_ne!(
        text(&first, "EventDataStoreArn"),
        text(&elsewhere, "EventDataStoreArn"),
        "case the same name in two Regions: expected two stores, got one ARN twice"
    );

    // A store is reached only from the Region it was created in: the other Region's
    // store is addressed by its own ARN.
    let home_arn = text(&first, "EventDataStoreArn");
    emulator
        .call_in(
            "GetEventDataStore",
            "eu-west-1",
            &json!({"EventDataStore": home_arn.rsplit('/').next().unwrap_or_default()}),
        )
        .await
        .expect_error(
            "GetEventDataStore by id suffix from another Region",
            400,
            "EventDataStoreNotFoundException",
        );
    emulator
        .call_in(
            "GetEventDataStore",
            "eu-west-1",
            &json!({"EventDataStore": home_arn}),
        )
        .await
        .expect_ok("GetEventDataStore by ARN from another Region");
}

/// The retention band a billing mode documents, at each published bound and one past
/// it. The model's own `@range` (7..3653) is refused by the boundary validator as
/// `ValidationException`; the narrower per-billing-mode maximum is the service's own
/// `InvalidParameterException`.
#[tokio::test]
async fn retention_period_is_bounded_by_the_billing_mode() {
    let emulator = Emulator::new();
    // (case, billing mode, retention period, expected error code or "" for a success)
    let rows: [(&str, &str, i64, &str); 8] = [
        ("extendable, shortest", EXTENDABLE, 7, ""),
        ("extendable, one shorter", EXTENDABLE, 6, "ValidationException"),
        ("extendable, longest", EXTENDABLE, 3653, ""),
        (
            "extendable, one longer",
            EXTENDABLE,
            3654,
            "ValidationException",
        ),
        ("fixed, shortest", FIXED, 7, ""),
        ("fixed, one shorter", FIXED, 6, "ValidationException"),
        ("fixed, longest", FIXED, 2557, ""),
        (
            "fixed, one longer",
            FIXED,
            2558,
            "InvalidParameterException",
        ),
    ];

    // How many rows have been accepted so far: the registry holds exactly these, so a
    // refused row must leave the count where it was.
    let mut created_rows = 0usize;
    for (index, (case, billing_mode, retention, expected)) in rows.iter().enumerate() {
        let request = json!({
            "Name": format!("retention-{index}"),
            "BillingMode": billing_mode,
            "RetentionPeriod": retention,
        });
        let reply = emulator.call("CreateEventDataStore", &request).await;
        if expected.is_empty() {
            let created = reply.expect_ok(case);
            assert_eq!(
                created.get("RetentionPeriod").and_then(Value::as_i64),
                Some(*retention),
                "case {case}: expected RetentionPeriod {retention}, got {created:?}"
            );
            created_rows += 1;
            continue;
        }
        reply.expect_error(case, 400, expected);
        // A refused create persists nothing.
        let stores = emulator.state().lock().event_data_stores.len();
        assert_eq!(
            stores, created_rows,
            "case {case}: a refused CreateEventDataStore must persist nothing, the \
             registry holds {stores} where {created_rows} row(s) were accepted"
        );
    }
}

/// The caller-chosen lists a created store would retain are bounded by what the model
/// documents, and a refused create persists nothing.
///
/// The bounds are the model's own prose, not this emulator's invention: "You can
/// configure up to five advanced event selectors for each event data store"
/// (`smithy_query(query="doc", shape="CreateEventDataStoreRequest",
/// member="AdvancedEventSelectors")`), "up to a maximum of 500 values for all conditions
/// and selectors" (`PutEventSelectors`) and "the limit is 50" tags per resource
/// (`TagsLimitExceededException`). Rows differ along WHICH list grows and whether it is
/// at its bound or one past it.
#[tokio::test]
async fn a_created_store_bounds_the_lists_it_retains() {
    let emulator = Emulator::new();
    let selector_limit = service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::
        ADVANCED_SELECTORS_PER_EVENT_DATA_STORE_LIMIT;
    let value_limit = service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::
        ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT;
    let tag_limit =
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::TAGS_PER_RESOURCE_LIMIT;

    /// `selectors` selectors, each carrying `values_each` condition values.
    ///
    /// The mandatory `eventCategory` field selector is one of them — every advanced
    /// event selector states which category of events it selects — so the rest of the
    /// count is spread over a `resources.ARN` condition.
    fn selectors(selectors: usize, values_each: usize) -> Value {
        Value::Array(
            (0..selectors)
                .map(|index| {
                    let values: Vec<String> = (0..values_each.saturating_sub(1))
                        .map(|v| format!("arn:aws:s3:::b{v}/"))
                        .collect();
                    let mut field_selectors =
                        vec![json!({"Field": "eventCategory", "Equals": ["Data"]})];
                    if !values.is_empty() {
                        field_selectors
                            .push(json!({"Field": "resources.ARN", "StartsWith": values}));
                    }
                    json!({
                        "Name": format!("selector-{index}"),
                        "FieldSelectors": field_selectors,
                    })
                })
                .collect(),
        )
    }

    /// `count` distinct tags.
    fn tags(count: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({"Key": format!("k{index}"), "Value": "v"}))
                .collect(),
        )
    }

    // (case, request, the error code or "" for a success)
    let rows: Vec<(String, Value, &str)> = vec![
        (
            format!("{selector_limit} selectors: the documented bound"),
            json!({"Name": "bound-selectors", "AdvancedEventSelectors": selectors(selector_limit, 1)}),
            "",
        ),
        (
            format!("{} selectors: one past the bound", selector_limit + 1),
            json!({
                "Name": "too-many-selectors",
                "AdvancedEventSelectors": selectors(selector_limit + 1, 1),
            }),
            "InvalidEventSelectorsException",
        ),
        (
            format!("{value_limit} condition values: the documented bound"),
            json!({
                "Name": "bound-values",
                "AdvancedEventSelectors": selectors(1, value_limit),
            }),
            "",
        ),
        (
            format!("{} condition values: one past the bound", value_limit + 1),
            json!({
                "Name": "too-many-values",
                "AdvancedEventSelectors": selectors(1, value_limit + 1),
            }),
            "InvalidEventSelectorsException",
        ),
        (
            "a field selector stating no condition at all".to_string(),
            json!({
                "Name": "conditionless",
                "AdvancedEventSelectors": [{
                    "Name": "nothing",
                    "FieldSelectors": [{"Field": "eventCategory"}],
                }],
            }),
            "InvalidEventSelectorsException",
        ),
        (
            format!("{tag_limit} tags: the documented bound"),
            json!({"Name": "bound-tags", "TagsList": tags(tag_limit)}),
            "",
        ),
        (
            format!("{} tags: one past the bound", tag_limit + 1),
            json!({"Name": "too-many-tags", "TagsList": tags(tag_limit + 1)}),
            "InvalidTagParameterException",
        ),
    ];

    let mut accepted = 0usize;
    for (case, request, expected) in rows {
        let reply = emulator.call("CreateEventDataStore", &request).await;
        if expected.is_empty() {
            reply.expect_ok(&case);
            accepted += 1;
        } else {
            reply.expect_error(&case, 400, expected);
        }
        // Whatever the answer, the registry holds exactly the accepted rows: a refused
        // create retains nothing of the request.
        let stored = emulator.state().lock().event_data_stores.len();
        assert_eq!(
            stored, accepted,
            "case {case}: expected {accepted} stored store(s), got {stored}"
        );
    }
}

/// `DeleteEventDataStore` moves the store into `PENDING_DELETION`, keeps it readable
/// for the documented seven-day wait period, and is unchanged by a repeat.
#[tokio::test]
async fn delete_moves_the_store_into_pending_deletion_for_seven_days() {
    let emulator = Emulator::new();
    let wait =
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::EVENT_DATA_STORE_DELETION_WAIT;

    // Both identifier forms drive the delete.
    for (form, by_arn) in [("ARN", true), ("id suffix", false)] {
        let created = emulator
            .call(
                "CreateEventDataStore",
                &json!({"Name": format!("delete-by-{}", form.replace(' ', "-"))}),
            )
            .await
            .expect_ok(form)
            .clone();
        let arn = text(&created, "EventDataStoreArn");
        let id = arn.rsplit('/').next().unwrap_or_default().to_string();
        let identifier = if by_arn { arn.clone() } else { id.clone() };

        let before = std::time::SystemTime::now();
        emulator
            .call(
                "DeleteEventDataStore",
                &json!({"EventDataStore": identifier.clone()}),
            )
            .await
            .expect_ok(&format!("DeleteEventDataStore by {form}"));

        let read = emulator
            .call("GetEventDataStore", &json!({"EventDataStore": arn.clone()}))
            .await
            .expect_ok(&format!("GetEventDataStore after the delete by {form}"))
            .clone();
        assert_eq!(
            text(&read, "Status"),
            PENDING_DELETION,
            "case delete by {form}: expected {PENDING_DELETION}, got {read:?}"
        );

        // The store is removed only once the seven-day wait has elapsed, which is the
        // instant the record carries.
        let due = emulator
            .state()
            .lock()
            .event_data_stores
            .get(&id)
            .and_then(|record| record.deleted_at)
            .expect("a deleted store carries the instant its wait period ends");
        assert!(
            due >= before + wait && due <= std::time::SystemTime::now() + wait,
            "case delete by {form}: expected the wait period to end seven days out, got {due:?}"
        );

        // Repeating the call leaves the store in the state it asked for, wait period
        // and all.
        emulator
            .call("DeleteEventDataStore", &json!({"EventDataStore": identifier}))
            .await
            .expect_ok(&format!("DeleteEventDataStore again by {form}"));
        let after = emulator
            .state()
            .lock()
            .event_data_stores
            .get(&id)
            .and_then(|record| record.deleted_at)
            .expect("the store is still pending deletion");
        assert_eq!(
            after, due,
            "case delete by {form}: a repeat must not restart the wait period"
        );
    }
}

/// A delete issued while the CLOCK still owes the store a transition sticks: the store
/// stays in `PENDING_DELETION` once that span elapses.
///
/// Rows differ along which transition is in flight when the delete lands — the
/// provisioning span a created store carries, and the ingestion span a stop starts.
#[tokio::test]
async fn delete_outlasts_a_transition_the_clock_still_owed() {
    // (case, whether to wait out provisioning first, whether to stop ingestion first)
    let rows = [
        ("while still provisioning", false, false),
        ("once provisioned", true, false),
        ("while ingestion is stopping", true, true),
    ];

    for (case, settle_first, stop_ingestion) in rows {
        let emulator = Emulator::new();
        // `StopEventDataStoreIngestion` is documented for a store whose
        // `eventCategory` is one of Management, Data, NetworkActivity or
        // ConfigurationItem, so the store collects management events.
        let created = emulator
            .call(
                "CreateEventDataStore",
                &json!({
                    "Name": "transitioning",
                    "AdvancedEventSelectors": [{
                        "Name": "management events",
                        "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Management"]}],
                    }],
                }),
            )
            .await
            .expect_ok(case)
            .clone();
        let arn = text(&created, "EventDataStoreArn");
        if settle_first {
            tokio::time::sleep(past_provisioning()).await;
        }
        if stop_ingestion {
            emulator
                .call(
                    "StopEventDataStoreIngestion",
                    &json!({"EventDataStore": arn.clone()}),
                )
                .await
                .expect_ok(case);
        }

        emulator
            .call(
                "DeleteEventDataStore",
                &json!({"EventDataStore": arn.clone()}),
            )
            .await
            .expect_ok(case);

        // Both spans the registry owns are shorter than this wait, so anything the
        // clock still owed has come due by the read below.
        tokio::time::sleep(past_provisioning() + past_settle()).await;
        let read = emulator
            .call("GetEventDataStore", &json!({"EventDataStore": arn}))
            .await
            .expect_ok(case)
            .clone();
        assert_eq!(
            text(&read, "Status"),
            PENDING_DELETION,
            "case {case}: expected {PENDING_DELETION} to outlast the owed transition, got {read:?}"
        );
    }
}

/// A delete this operation's documented preconditions forbid is refused, and the store
/// it names is left exactly as it was.
#[tokio::test]
async fn delete_refuses_a_protected_or_federated_store_and_changes_nothing() {
    let emulator = Emulator::new();

    // "TerminationProtectionEnabled must be set to False on the event data store".
    let protected = emulator
        .call(
            "CreateEventDataStore",
            &json!({"Name": "protected", "TerminationProtectionEnabled": true}),
        )
        .await
        .expect_ok("CreateEventDataStore with termination protection")
        .clone();
    let protected_arn = text(&protected, "EventDataStoreArn");
    emulator
        .call(
            "DeleteEventDataStore",
            &json!({"EventDataStore": protected_arn.clone()}),
        )
        .await
        .expect_error(
            "DeleteEventDataStore on a protected store",
            400,
            "EventDataStoreTerminationProtectedException",
        );
    let read = emulator
        .call(
            "GetEventDataStore",
            &json!({"EventDataStore": protected_arn}),
        )
        .await
        .expect_ok("GetEventDataStore after the refusal")
        .clone();
    assert_ne!(
        text(&read, "Status"),
        PENDING_DELETION,
        "case protected store: the refused delete must change nothing, got {read:?}"
    );

    // "the FederationStatus must be DISABLED". The two TRANSIENT values (`ENABLING`,
    // `DISABLING`) exist only inside a span the clock owns, so the status is seeded in
    // the store the handlers read rather than raced through `EnableFederation`; every
    // value other than DISABLED — the transient pair included — is refused.
    for status in [
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::FederationState::Enabled,
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::FederationState::Enabling,
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::FederationState::Disabling,
    ] {
        let federated = emulator
            .call(
                "CreateEventDataStore",
                &json!({"Name": format!("federated-{}", status.as_wire())}),
            )
            .await
            .expect_ok("CreateEventDataStore")
            .clone();
        let arn = text(&federated, "EventDataStoreArn");
        let id = arn.rsplit('/').next().unwrap_or_default().to_string();
        {
            let state = emulator.state();
            let mut store = state.lock();
            let record = store
                .event_data_stores
                .get_mut(&id)
                .expect("the created store is in the registry");
            record.federation_status = status;
        }
        emulator
            .call("DeleteEventDataStore", &json!({"EventDataStore": arn.clone()}))
            .await
            .expect_error(
                &format!("DeleteEventDataStore with federation {}", status.as_wire()),
                400,
                "EventDataStoreFederationEnabledException",
            );
        let read = emulator
            .call("GetEventDataStore", &json!({"EventDataStore": arn}))
            .await
            .expect_ok("GetEventDataStore after the refusal")
            .clone();
        assert_ne!(
            text(&read, "Status"),
            PENDING_DELETION,
            "case federation {}: the refused delete must change nothing, got {read:?}",
            status.as_wire()
        );
    }

    // The same refusal, with the federation turned on OVER THE WIRE rather than
    // seeded: `EnableFederation` leaves the store `ENABLING` and the clock settles it
    // into `ENABLED`, and a delete is refused in both of those states.
    let account = engine::emulator_account_id();
    let federated = emulator
        .call("CreateEventDataStore", &json!({"Name": "federated-by-call"}))
        .await
        .expect_ok("CreateEventDataStore")
        .clone();
    let arn = text(&federated, "EventDataStoreArn");
    tokio::time::sleep(past_provisioning()).await;
    emulator
        .call(
            "EnableFederation",
            &json!({
                "EventDataStore": arn.clone(),
                "FederationRoleArn": format!("arn:aws:iam::{account}:role/lake-federation"),
            }),
        )
        .await
        .expect_ok("EnableFederation");
    for case in ["while federation is enabling", "once federation is enabled"] {
        emulator
            .call(
                "DeleteEventDataStore",
                &json!({"EventDataStore": arn.clone()}),
            )
            .await
            .expect_error(case, 400, "EventDataStoreFederationEnabledException");
        let read = emulator
            .call("GetEventDataStore", &json!({"EventDataStore": arn.clone()}))
            .await
            .expect_ok(case)
            .clone();
        assert_ne!(
            text(&read, "Status"),
            PENDING_DELETION,
            "case {case}: the refused delete must change nothing, got {read:?}"
        );
        // Let the clock settle `ENABLING` into `ENABLED` for the second row.
        tokio::time::sleep(past_federation_settle()).await;
    }
}

/// Once the wait period a delete started has elapsed, the store is really gone — "is
/// automatically deleted after a wait period of seven days" — and so are the Lake
/// queries that ran on it, which are reachable only through it.
///
/// Seven days cannot be waited out in a test, so the instant the delete recorded is
/// moved into the past in the registry the handlers read; the removal itself is the
/// service's, and it is observed over the wire.
#[tokio::test]
async fn a_store_whose_wait_period_elapsed_is_gone_with_its_queries() {
    let emulator = Emulator::new();
    let created = emulator
        .call("CreateEventDataStore", &json!({"Name": "expiring"}))
        .await
        .expect_ok("CreateEventDataStore")
        .clone();
    let arn = text(&created, "EventDataStoreArn");
    let id = arn.rsplit('/').next().unwrap_or_default().to_string();
    emulator
        .call(
            "DeleteEventDataStore",
            &json!({"EventDataStore": arn.clone()}),
        )
        .await
        .expect_ok("DeleteEventDataStore");

    // A query that ran on this store, and one that ran on another: only the first is
    // reachable through the store being removed.
    {
        let state = emulator.state();
        let mut store = state.lock();
        for (query_id, store_id) in [("on-the-store", id.as_str()), ("elsewhere", "other-store")] {
            store.queries.insert(
                query_id.to_string(),
                service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::QueryRecord {
                    query_id: query_id.to_string(),
                    event_data_store_id: store_id.to_string(),
                    region: HOME_REGION.to_string(),
                    ..Default::default()
                },
            );
        }
        let record = store
            .event_data_stores
            .get_mut(&id)
            .expect("the deleted store is still pending deletion");
        record.deleted_at = Some(std::time::SystemTime::now() - std::time::Duration::from_secs(1));
    }

    let reply = emulator
        .call("GetEventDataStore", &json!({"EventDataStore": arn}))
        .await;
    reply.expect_error(
        "a store whose wait period elapsed",
        400,
        "EventDataStoreNotFoundException",
    );
    let store = emulator.state();
    let store = store.lock();
    assert!(
        !store.event_data_stores.contains_key(&id),
        "case a store whose wait period elapsed: expected it gone from the registry, \
         {} store(s) remain",
        store.event_data_stores.len()
    );
    assert_eq!(
        store.queries.keys().cloned().collect::<Vec<String>>(),
        vec!["elsewhere".to_string()],
        "case a store whose wait period elapsed: expected only its own queries to go, got {:?}",
        store.queries.keys().collect::<Vec<&String>>()
    );
}

/// An identifier that names no store of this account and Region is refused with
/// `EventDataStoreNotFoundException`, naming the ARN it addressed — by every operation
/// that resolves one.
#[tokio::test]
async fn an_identifier_naming_no_store_is_not_found() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let absent_id = "08d7dd45-103e-41b9-bc6a-6e8c427e9513";

    // A store that DOES exist, so the Region-mismatch row is about the Region and not
    // about the id.
    let created = emulator
        .call("CreateEventDataStore", &json!({"Name": "present"}))
        .await
        .expect_ok("CreateEventDataStore")
        .clone();
    let present_id = text(&created, "EventDataStoreArn")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    // (case, identifier, the ARN the refusal must name)
    let rows = [
        (
            "an unused id suffix",
            absent_id.to_string(),
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{absent_id}"),
        ),
        (
            "an unused ARN in the caller's Region",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{absent_id}"),
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{absent_id}"),
        ),
        (
            "an ARN in another Region",
            format!("arn:aws:cloudtrail:us-west-2:{account}:eventdatastore/{absent_id}"),
            format!("arn:aws:cloudtrail:us-west-2:{account}:eventdatastore/{absent_id}"),
        ),
        (
            "an existing store addressed in another Region",
            format!("arn:aws:cloudtrail:ap-south-1:{account}:eventdatastore/{present_id}"),
            format!("arn:aws:cloudtrail:ap-south-1:{account}:eventdatastore/{present_id}"),
        ),
    ];

    for (case, identifier, addressed) in rows {
        for operation in ["DeleteEventDataStore", "GetEventDataStore"] {
            let reply = emulator
                .call(operation, &json!({"EventDataStore": identifier.clone()}))
                .await;
            let label = format!("{operation} with {case}");
            reply.expect_error(&label, 400, "EventDataStoreNotFoundException");
            assert_eq!(
                text(&reply.body, "Message"),
                format!("The event data store {addressed} was not found."),
                "case {label}: the refusal must name the ARN it addressed, got {:?}",
                reply.body
            );
        }
    }
}

/// A value WRITTEN as an ARN that is not a CloudTrail event data store ARN is refused
/// as a bad ARN, not reported as a missing store — and so is a bare value that is no
/// id this service mints, which the recorded `UpdateEventDataStore` answer to
/// `not-an-arn` refuses as `EventDataStoreARNInvalidException` over the ARN it
/// completes to.
#[tokio::test]
async fn a_value_written_as_an_arn_must_be_an_event_data_store_arn() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let rows = [
        (
            "a bare value that is no id this service mints",
            "no-such-event-data-store".to_string(),
        ),
        (
            "another resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:trail/demo"),
        ),
        (
            "another service",
            format!("arn:aws:s3:{HOME_REGION}:{account}:eventdatastore/demo"),
        ),
        (
            "too few fields",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}"),
        ),
        (
            "no Region",
            format!("arn:aws:cloudtrail::{account}:eventdatastore/demo"),
        ),
        (
            "no account",
            format!("arn:aws:cloudtrail:{HOME_REGION}::eventdatastore/demo"),
        ),
        (
            "no id suffix",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/"),
        ),
    ];

    for (case, identifier) in rows {
        for operation in ["DeleteEventDataStore", "GetEventDataStore"] {
            emulator
                .call(operation, &json!({"EventDataStore": identifier.clone()}))
                .await
                .expect_error(
                    &format!("{operation} with {case}"),
                    400,
                    "EventDataStoreARNInvalidException",
                );
        }
    }
}
