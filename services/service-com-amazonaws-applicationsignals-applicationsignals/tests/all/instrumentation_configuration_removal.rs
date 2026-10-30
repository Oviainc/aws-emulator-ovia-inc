// SPDX-License-Identifier: CC0-1.0

//! The removal of a dynamic instrumentation configuration, asserted over BOTH paths that
//! remove one, over HTTP.
//!
//! The rule under test: taking a configuration away takes the WHOLE record with it — the
//! stored configuration, the status history reported against it and the tags held under its
//! ARN — leaves the location remembered as one a configuration was removed from, records the
//! change on the set the record left, and touches no other configuration. The path that took
//! it away makes no difference: an explicit `DeleteInstrumentationConfiguration` and the lapse
//! the clock owes at a configuration's `ExpiresAt` (materialised by the next operation to run,
//! here `ListInstrumentationConfigurations`) remove the same things.
//!
//! The rows differ along what the rule generalises over: WHICH path removes the record, the
//! instrumentation TYPE of the record removed (and so of the bystander that must survive), how
//! many tags it holds, how many status events it has reported, and whether the requests name
//! its location by hash or in full.
//!
//! A lapse cannot be driven by requests alone: the documented floor on a BREAKPOINT lifetime
//! is 5 minutes, which outlasts any test, so the row that lapses stages the expiry the clock
//! has already passed on the store the handlers share, through
//! `put_instrumentation_configuration` — the very method the create path calls — and then
//! drives the operation that materialises it.

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::state::{
    instrumentation_key, ComAmazonawsApplicationsignalsApplicationsignalsState,
    InstrumentationScope,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::instrumentation_configuration_status::{
    event_times, get_status, nearest_minute, now_minute, report, report_entry, status_request,
    Query, BREAKPOINT, PROBE, SNAPSHOT,
};
use crate::instrumentation_configuration_tags::{
    delete_configuration, get_configuration, router_with_engine,
};
use crate::refusal_wording::CONFIGURATION_DELETED;
use crate::resource_tags::{expected_map, list_tags_raw, reported_tags, tag, tag_list};
use crate::slo_lifecycle::{arn_uri_segment, call};

/// The service and environment every row's configurations live in: one scope per
/// instrumentation type, so a record of one type and the bystander of the other are addressed
/// under scopes of their own.
const SERVICE: &str = "checkout";
const ENVIRONMENT: &str = "eks:cluster-prod/namespace";

/// The `DeletionStatus` a delete answers, the only value the modelled
/// `DynamicInstrumentationDeletionStatus` carries
/// (`smithy_query(query="shape",shape="DynamicInstrumentationDeletionStatus")` ->
/// `["DELETED"]`).
const DELETED: &str = "DELETED";

/// Which of the two paths that remove a configuration a row drives.
#[derive(Clone, Copy, Debug)]
enum Removal {
    /// `DeleteInstrumentationConfiguration`, the explicit removal.
    Delete,
    /// The lapse the clock owes at `ExpiresAt`, materialised by the expiry pass an operation
    /// runs before it answers.
    Lapse,
}

/// One row: the path that removes the record, the record it removes, and how much that record
/// owns when it goes.
#[derive(Clone, Debug)]
struct Case {
    label: &'static str,
    removal: Removal,
    /// The instrumentation type of the record the row removes. The bystander that must survive
    /// is of the other type.
    removed_type: &'static str,
    /// The tags the removed record is created with.
    tags: Vec<(String, String)>,
    /// The statuses reported against the removed record, `ACTIVE` first so the default status
    /// read (which reports `ACTIVE` events of the last hour) has an event to report.
    statuses: Vec<&'static str>,
    /// Whether the requests name the removed record's location by the hash the create answered
    /// or in full: `LocationIdentifier` is "either full code location or a pre-computed hash".
    by_hash: bool,
}

/// The other instrumentation type: the bystander's, given the removed record's.
fn other_type(instrumentation_type: &str) -> &'static str {
    if instrumentation_type == BREAKPOINT {
        PROBE
    } else {
        BREAKPOINT
    }
}

/// The `CodeLocation` of the record a row removes.
fn removed_location() -> Value {
    json!({
        "Language": "Java",
        "ClassName": "com.example.CartService",
        "FilePath": "src/main/java/com/example/CartService.java",
        "LineNumber": 142
    })
}

/// The `CodeLocation` of the bystander, a different location so the two records are stored
/// under different hashes and different ARNs.
fn bystander_location() -> Value {
    json!({
        "Language": "Python",
        "FilePath": "src/pricing/quote.py",
        "LineNumber": 7
    })
}

/// A create request for one configuration of `kind` at `location`, carrying `tags`.
fn create_body(kind: &str, location: Value, tags: &[(String, String)]) -> Value {
    json!({
        "Service": SERVICE,
        "Environment": ENVIRONMENT,
        "SignalType": SNAPSHOT,
        "InstrumentationType": kind,
        "Location": { "CodeLocation": location },
        // `CaptureConfiguration`'s required union member, with only its own required member
        // (`smithy_query(query="shape",shape="CodeCaptureConfiguration")`).
        "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } },
        "Tags": tag_list(tags),
    })
}

/// The `LocationIdentifier` naming `location` either by `hash` or in full.
fn identifier(by_hash: bool, hash: &str, location: Value) -> Value {
    if by_hash {
        json!({ "LocationHash": hash })
    } else {
        json!({ "CodeLocation": location })
    }
}

/// A request naming one configuration: what the delete and both reads take.
fn identity(kind: &str, identifier: Value) -> Value {
    json!({
        "Service": SERVICE,
        "Environment": ENVIRONMENT,
        "SignalType": SNAPSHOT,
        "InstrumentationType": kind,
        "LocationIdentifier": identifier,
    })
}

/// `POST /list-instrumentation-configurations` for the scope `kind` names, as of `synced_at`
/// when one is given.
async fn list(router: &MultiServiceRouter, kind: &str, synced_at: Option<f64>) -> (u16, Value) {
    let mut body = json!({
        "Service": SERVICE,
        "Environment": ENVIRONMENT,
        "InstrumentationType": kind,
    });
    if let Some(synced_at) = synced_at {
        body["SyncedAt"] = json!(synced_at);
    }
    call(
        router,
        "POST",
        "/list-instrumentation-configurations",
        Some(body),
    )
    .await
}

/// `POST /create-instrumentation-configuration`, answering the stored record.
async fn create(router: &MultiServiceRouter, label: &str, body: Value) -> Value {
    let (status, created) = call(
        router,
        "POST",
        "/create-instrumentation-configuration",
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the setup create to answer 200, got {status} with {created}"
    );
    created
}

/// The `LocationHash` of a stored record.
fn hash_of(label: &str, created: &Value) -> String {
    created["LocationHash"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: the create answers a location hash: {created}"))
        .to_string()
}

/// The `ARN` of a stored record.
fn arn_of(label: &str, created: &Value) -> String {
    created["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: the create answers an ARN: {created}"))
        .to_string()
}

/// The rows the rule is asserted over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "an explicit delete of a BREAKPOINT configuration, named by hash",
            removal: Removal::Delete,
            removed_type: BREAKPOINT,
            tags: vec![tag("team", "payments")],
            statuses: vec!["ACTIVE"],
            by_hash: true,
        },
        Case {
            label: "an explicit delete of a PROBE configuration, named by its full location",
            removal: Removal::Delete,
            removed_type: PROBE,
            tags: vec![tag("team", "payments"), tag("tier", "gold"), tag("cost", "")],
            statuses: vec!["ACTIVE", "READY", "ERROR"],
            by_hash: false,
        },
        Case {
            label: "the lapse the clock owes a BREAKPOINT configuration, named by hash",
            removal: Removal::Lapse,
            removed_type: BREAKPOINT,
            tags: vec![tag("team", "payments"), tag("tier", "gold")],
            statuses: vec!["ACTIVE", "DISABLED"],
            by_hash: true,
        },
    ]
}

/// Both former homes of the removal rule now go through one path, so a removal takes the same
/// things away whichever operation asks for it.
#[tokio::test]
async fn every_removal_path_takes_the_whole_record_with_it() {
    // An instant inside the default status window (the last hour) and on a whole minute, so
    // the instant a status read reports is derivable from it.
    let reported_at = now_minute() - 60.0;
    // Every change this test makes to a scope's set happens after this instant, so a caller
    // that synced here is told about the removal.
    let before_everything = reported_at - 3600.0;

    for case in cases() {
        let label = case.label;
        let bystander_type = other_type(case.removed_type);
        let (router, engine) = router_with_engine();

        let removed = create(
            &router,
            label,
            create_body(case.removed_type, removed_location(), &case.tags),
        )
        .await;
        let bystander_tags = vec![tag("owner", "sre")];
        let kept = create(
            &router,
            label,
            create_body(bystander_type, bystander_location(), &bystander_tags),
        )
        .await;

        let removed_hash = hash_of(label, &removed);
        let removed_arn = arn_of(label, &removed);
        let kept_hash = hash_of(label, &kept);
        let kept_arn = arn_of(label, &kept);

        let removed_identity = identity(
            case.removed_type,
            identifier(case.by_hash, &removed_hash, removed_location()),
        );
        let kept_identity = identity(bystander_type, json!({ "LocationHash": &kept_hash }));

        // The removed record reports the row's statuses, the bystander reports ACTIVE, so both
        // carry a history for the removal to be seen to take only one of them.
        let mut entries: Vec<Value> = case
            .statuses
            .iter()
            .map(|status| {
                report_entry(
                    case.removed_type,
                    &removed_hash,
                    status,
                    reported_at,
                    (*status == "ERROR").then_some("RUNTIME_ERROR"),
                )
            })
            .collect();
        entries.push(report_entry(
            bystander_type,
            &kept_hash,
            "ACTIVE",
            reported_at,
            None,
        ));
        let (status, body) = report(
            &router,
            &json!({
                "Service": SERVICE,
                "Environment": ENVIRONMENT,
                "Configurations": entries,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the setup report to answer 200, got {status} with {body}"
        );

        // Before the removal the record, its history and its tags are all served.
        let (status, served) = get_configuration(&router, removed_identity.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the configuration to be served before the removal, got \
             {status} with {served}"
        );
        let (status, history) = get_status(
            &router,
            &status_request(
                SERVICE,
                ENVIRONMENT,
                case.removed_type,
                identifier(case.by_hash, &removed_hash, removed_location()),
                &Query::default(),
            ),
        )
        .await;
        assert_eq!(
            (status, event_times(&history)),
            (200, vec![nearest_minute(reported_at)]),
            "case {label:?}: expected the reported event before the removal, got {status} with \
             {history}"
        );
        let (status, tags) = list_tags_raw(&router, &arn_uri_segment(&removed_arn)).await;
        assert_eq!(
            (status, reported_tags(&tags)),
            (200, expected_map(&case.tags)),
            "case {label:?}: expected the created tags before the removal, got {status} with \
             {tags}"
        );

        // Take the record away along this row's path.
        match case.removal {
            Removal::Delete => {
                let (status, answer) = delete_configuration(&router, removed_identity.clone()).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: expected the delete to answer 200, got {status} with {answer}"
                );
                assert_eq!(
                    answer["DeletionStatus"].as_str(),
                    Some(DELETED),
                    "case {label:?}: expected DeletionStatus {DELETED}, got {answer}"
                );
            }
            Removal::Lapse => {
                // Stage what the clock will do: the same record, with an expiry it has already
                // passed.
                let scope = InstrumentationScope::new(case.removed_type, SERVICE, ENVIRONMENT);
                let key = instrumentation_key(SNAPSHOT, &removed_hash);
                let lapsed_at = DateTime::from_secs_f64(
                    DateTime::from(std::time::SystemTime::now()).as_secs_f64() - 1.0,
                );
                {
                    let state =
                        engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
                    let mut store = state.lock();
                    let mut record = store
                        .instrumentation_configuration(&scope, &key)
                        .unwrap_or_else(|| {
                            panic!("case {label:?}: the create stored the configuration")
                        })
                        .clone();
                    record.expires_at = Some(lapsed_at);
                    store.put_instrumentation_configuration(scope, record, lapsed_at);
                }

                // A list runs the pass that materialises the lapse before it answers.
                let (status, answer) = list(&router, case.removed_type, None).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: expected the list that materialises the lapse to answer 200, \
                     got {status} with {answer}"
                );
            }
        }

        // The record is gone, and the location is remembered as one a configuration was
        // removed from — which is the sentence a read of it states.
        let (status, error) = get_configuration(&router, removed_identity.clone()).await;
        assert_eq!(
            (status, error["Message"].as_str()),
            (404, Some(CONFIGURATION_DELETED)),
            "case {label:?}: expected a 404 stating the configuration was removed, got {status} \
             with {error}"
        );

        // Its status history went with it.
        let (status, error) = get_status(
            &router,
            &status_request(
                SERVICE,
                ENVIRONMENT,
                case.removed_type,
                identifier(case.by_hash, &removed_hash, removed_location()),
                &Query::default(),
            ),
        )
        .await;
        assert_eq!(
            (status, error["Message"].as_str()),
            (404, Some(CONFIGURATION_DELETED)),
            "case {label:?}: expected the status read of the removed configuration to be a 404, \
             got {status} with {error}"
        );

        // So did the tags held under its ARN: the ARN names no resource this service holds.
        let (status, error) = list_tags_raw(&router, &arn_uri_segment(&removed_arn)).await;
        assert_eq!(
            (status, error["__type"].as_str()),
            (404, Some("ResourceNotFoundException")),
            "case {label:?}: expected the removed record's ARN to name no resource, got {status} \
             with {error}"
        );

        // The set the record left reports the change, and reports the record no more.
        let (status, answer) = list(&router, case.removed_type, Some(before_everything)).await;
        assert_eq!(
            (status, answer["Changed"].as_bool()),
            (200, Some(true)),
            "case {label:?}: expected the removal to be a change to an earlier sync, got {status} \
             with {answer}"
        );
        assert_eq!(
            answer["LatestConfigurations"].as_array().map(Vec::len),
            Some(0),
            "case {label:?}: expected the removed record to be reported no more, got {:?}",
            answer["LatestConfigurations"]
        );

        // Nothing else was touched: the bystander keeps its record, its history and its tags.
        let (status, still_there) = get_configuration(&router, kept_identity.clone()).await;
        assert_eq!(
            (status, &still_there["Configuration"]["LocationHash"]),
            (200, &json!(&kept_hash)),
            "case {label:?}: expected the other configuration to be served still, got {status} \
             with {still_there}"
        );
        let (status, history) = get_status(
            &router,
            &status_request(
                SERVICE,
                ENVIRONMENT,
                bystander_type,
                json!({ "LocationHash": &kept_hash }),
                &Query::default(),
            ),
        )
        .await;
        assert_eq!(
            (status, event_times(&history)),
            (200, vec![nearest_minute(reported_at)]),
            "case {label:?}: expected the other configuration to keep its history, got {status} \
             with {history}"
        );
        let (status, tags) = list_tags_raw(&router, &arn_uri_segment(&kept_arn)).await;
        assert_eq!(
            (status, reported_tags(&tags)),
            (200, expected_map(&bystander_tags)),
            "case {label:?}: expected the other configuration to keep its tags, got {status} with \
             {tags}"
        );

        // And the store holds no half-removed record behind the wire: the configuration, its
        // history and its tag set are all gone, while the bystander's tag set remains.
        {
            let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
            let store = state.lock();
            let scope = InstrumentationScope::new(case.removed_type, SERVICE, ENVIRONMENT);
            let key = instrumentation_key(SNAPSHOT, &removed_hash);
            assert!(
                store.instrumentation_configuration(&scope, &key).is_none(),
                "case {label:?}: expected the removed configuration to be dropped from the store"
            );
            let held = store
                .instrumentation_status_history(&scope, &key)
                .map(|history| history.len());
            assert!(
                held.is_none(),
                "case {label:?}: expected the removed configuration's history to be dropped, \
                 holding {held:?}"
            );
            assert!(
                !store.tags.contains_key(&removed_arn),
                "case {label:?}: expected the removed configuration's tags to be dropped, got {:?}",
                store.tags.get(&removed_arn)
            );
            assert!(
                store.tags.contains_key(&kept_arn),
                "case {label:?}: expected the other configuration to keep its tag set"
            );
        }
    }
}

/// The location a removal frees is free for a new configuration, whichever path freed it: a
/// create at it succeeds and mints a record of its own, and the read that reported the removal
/// reports that NEW record.
#[tokio::test]
async fn a_location_either_removal_frees_takes_a_new_configuration() {
    for case in cases() {
        let label = case.label;
        let (router, engine) = router_with_engine();
        let body = create_body(case.removed_type, removed_location(), &case.tags);
        let first = create(&router, label, body.clone()).await;
        let hash = hash_of(label, &first);
        let identity = identity(
            case.removed_type,
            identifier(case.by_hash, &hash, removed_location()),
        );

        match case.removal {
            Removal::Delete => {
                let (status, answer) = delete_configuration(&router, identity.clone()).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: expected the delete to answer 200, got {status} with {answer}"
                );
            }
            Removal::Lapse => {
                let scope = InstrumentationScope::new(case.removed_type, SERVICE, ENVIRONMENT);
                let key = instrumentation_key(SNAPSHOT, &hash);
                let lapsed_at = DateTime::from_secs_f64(
                    DateTime::from(std::time::SystemTime::now()).as_secs_f64() - 1.0,
                );
                {
                    let state =
                        engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
                    let mut store = state.lock();
                    let mut record = store
                        .instrumentation_configuration(&scope, &key)
                        .unwrap_or_else(|| {
                            panic!("case {label:?}: the create stored the configuration")
                        })
                        .clone();
                    record.expires_at = Some(lapsed_at);
                    store.put_instrumentation_configuration(scope, record, lapsed_at);
                }
                let (status, answer) = list(&router, case.removed_type, None).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: expected the list that materialises the lapse to answer 200, \
                     got {status} with {answer}"
                );
            }
        }

        let recreated = create(&router, label, body).await;
        assert_eq!(
            hash_of(label, &recreated),
            hash,
            "case {label:?}: expected the same location to hash the same, got {recreated}"
        );
        let (status, served) = get_configuration(&router, identity).await;
        assert_eq!(
            (status, &served["Configuration"]["CreatedAt"]),
            (200, &recreated["CreatedAt"]),
            "case {label:?}: expected the read to report the NEW record, got {status} with {served}"
        );
    }
}
