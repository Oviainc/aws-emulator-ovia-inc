// SPDX-License-Identifier: CC0-1.0

//! `GetSyncBlockerSummary`: the blocker summary the blockers this service holds render,
//! and the empty summary a `(SyncType, ResourceName)` pair with no blocker reports.
//!
//! The operation is a pure REPORT, and only the SERVICE mints a sync blocker (when a
//! Git sync it performed is blocked; this emulator performs none), so the rows it
//! reports are seeded into the shared engine's store — the same store every handler of
//! the router reads — the way `git_sync.rs` seeds the rows `UpdateSyncBlocker` resolves.
//! Every request below still goes through the real `MultiServiceRouter` dispatch, so
//! these tests exercise the same path a signed SDK request takes.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codeconnections_codeconnections_20231201::ComAmazonawsCodeconnectionsCodeconnections20231201State as CodeConnectionsState;
use service_com_amazonaws_codeconnections_codeconnections_20231201::ops::types::{
    BLOCKER_STATUS_VALUES, BLOCKER_TYPE_VALUES, SYNC_CONFIGURATION_TYPE_VALUES, SyncBlockerContext,
};
use service_com_amazonaws_codeconnections_codeconnections_20231201::state::StoredSyncBlocker;

use super::connection_lifecycle::{call, router_with_engine};

/// The only value the modeled `SyncConfigurationType` enum declares
/// (`smithy_query(query="shape", shape="SyncConfigurationType")` — its `values` list,
/// also carried by the crate's `SYNC_CONFIGURATION_TYPE_VALUES`).
const CFN_STACK_SYNC: &str = "CFN_STACK_SYNC";

/// A fixed instant the seeded blockers are placed around, so every expected
/// `CreatedAt`/`ResolvedAt` is an exact number rather than "about now".
const BASE_EPOCH_SECS: i64 = 1_700_000_000;

/// The modeled `ResourceName` is `length 1..100`; this is the published maximum, used
/// by the row that pins a name at the bound.
const RESOURCE_NAME_MAX: usize = 100;

/// The two forms of this service's awsJson1_0 RPC target prefix: the FULL service
/// shape id (`com.amazonaws.codeconnections#CodeConnections_20231201` with the `#` as
/// a dot), which is what an SDK built from this service's model sends, and the bare
/// service shape name. Both name the same operation in their last dot-separated
/// segment, so both must route.
const TARGET_PREFIXES: &[&str] = &[
    "com.amazonaws.codeconnections.CodeConnections_20231201",
    "CodeConnections_20231201",
];

/// One sync blocker to seed against a resource.
struct BlockerSeed {
    /// The modeled `Id` (`length 1..50`), and the key the blocker store holds it under.
    id: String,
    /// A value of the modeled `BlockerType` enum.
    blocker_type: &'static str,
    /// A value of the modeled `BlockerStatus` enum.
    status: &'static str,
    created_reason: &'static str,
    /// Seconds after [`BASE_EPOCH_SECS`] at which the service raised the blocker.
    created_offset: i64,
    /// `(Key, Value)` context pairs, in the order the store holds them.
    contexts: &'static [(&'static str, &'static str)],
    /// `(ResolvedReason, seconds after BASE_EPOCH_SECS)` once it was resolved.
    resolved: Option<(&'static str, i64)>,
}

/// One resource's seeded blockers, and what its summary must report.
struct SummaryCase {
    /// What a failing assertion names this row by.
    label: &'static str,
    resource_name: String,
    parent_resource_name: Option<&'static str>,
    /// The blockers to seed, in the order they are written to the store — deliberately
    /// NOT the order the summary must report.
    blockers: Vec<BlockerSeed>,
    /// The blocker ids the summary must report, in order: most recent first, the
    /// blocker id breaking a tie on `CreatedAt`.
    expected_order: Vec<String>,
}

fn at(offset: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs(BASE_EPOCH_SECS + offset)
}

/// Seed one resource's blockers into the engine every handler of `router` reads, the
/// way the service would record them: one row per blocker, keyed by its id.
fn seed(engine: &Arc<engine::InMemoryEngine>, case: &SummaryCase) {
    let state = engine.state::<CodeConnectionsState>();
    let mut store = state.lock();
    for blocker in &case.blockers {
        store.sync_blockers.insert(
            blocker.id.clone(),
            StoredSyncBlocker {
                id: blocker.id.clone(),
                sync_type: CFN_STACK_SYNC.to_string(),
                resource_name: case.resource_name.clone(),
                parent_resource_name: case.parent_resource_name.map(str::to_string),
                blocker_type: blocker.blocker_type.to_string(),
                status: blocker.status.to_string(),
                created_reason: blocker.created_reason.to_string(),
                created_at: at(blocker.created_offset),
                contexts: blocker
                    .contexts
                    .iter()
                    .map(|(key, value)| SyncBlockerContext {
                        key: (*key).to_string(),
                        value: (*value).to_string(),
                    })
                    .collect(),
                resolved_reason: blocker.resolved.map(|(reason, _)| reason.to_string()),
                resolved_at: blocker.resolved.map(|(_, offset)| at(offset)),
            },
        );
    }
}

/// How many blocker rows the store holds.
fn stored_blockers(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<CodeConnectionsState>()
        .lock()
        .sync_blockers
        .len()
}

async fn get_summary(
    router: &MultiServiceRouter,
    sync_type: &str,
    resource_name: &str,
) -> (u16, serde_json::Value) {
    call(
        router,
        "GetSyncBlockerSummary",
        serde_json::json!({ "SyncType": sync_type, "ResourceName": resource_name }),
    )
    .await
}

/// The ids a row expects, as owned strings.
fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

/// The table: resources differing in how many blockers they carry, the order the store
/// holds them in, whether a parent resource is recorded, how many context pairs a
/// blocker has, whether it is resolved, and the form and length of the resource name
/// and of the blocker id (each published bound of `Id` 1..50).
fn summary_cases() -> Vec<SummaryCase> {
    // Fifty characters: the published maximum of the modeled `Id` (`length 1..50`).
    let id_at_max_length = "x".repeat(50);
    vec![
        SummaryCase {
            label: "a resource with no blocker at all",
            resource_name: "StackWithoutBlockers".to_string(),
            parent_resource_name: None,
            blockers: Vec::new(),
            expected_order: Vec::new(),
        },
        SummaryCase {
            label: "one active blocker, no context, id at the published minimum",
            resource_name: "A".to_string(),
            parent_resource_name: None,
            blockers: vec![BlockerSeed {
                id: "b".to_string(),
                blocker_type: "AUTOMATED",
                status: "ACTIVE",
                created_reason: "Stack update failed: template parse error",
                created_offset: 0,
                contexts: &[],
                resolved: None,
            }],
            expected_order: ids(&["b"]),
        },
        SummaryCase {
            label: "three blockers seeded oldest last, reported newest first",
            resource_name: "my_stack-sync".to_string(),
            parent_resource_name: Some("ParentStack"),
            blockers: vec![
                BlockerSeed {
                    id: "middle".to_string(),
                    blocker_type: "AUTOMATED",
                    status: "ACTIVE",
                    created_reason: "second failure",
                    created_offset: 60,
                    contexts: &[("Branch", "main")],
                    resolved: None,
                },
                BlockerSeed {
                    id: "newest".to_string(),
                    blocker_type: "AUTOMATED",
                    status: "ACTIVE",
                    created_reason: "third failure",
                    created_offset: 120,
                    contexts: &[("Branch", "main"), ("CommitId", "0f1e2d3")],
                    resolved: None,
                },
                BlockerSeed {
                    id: "oldest".to_string(),
                    blocker_type: "AUTOMATED",
                    status: "RESOLVED",
                    created_reason: "first failure",
                    created_offset: 0,
                    contexts: &[],
                    resolved: Some(("retried the latest commit", 30)),
                },
            ],
            expected_order: ids(&["newest", "middle", "oldest"]),
        },
        SummaryCase {
            label: "two blockers raised at the same instant, ordered by id",
            resource_name: "TieBreak".to_string(),
            parent_resource_name: None,
            blockers: vec![
                BlockerSeed {
                    id: "zz-second".to_string(),
                    blocker_type: "AUTOMATED",
                    status: "ACTIVE",
                    created_reason: "one of a pair",
                    created_offset: 5,
                    contexts: &[],
                    resolved: None,
                },
                BlockerSeed {
                    id: "aa-first".to_string(),
                    blocker_type: "AUTOMATED",
                    status: "ACTIVE",
                    created_reason: "the other of the pair",
                    created_offset: 5,
                    contexts: &[],
                    resolved: None,
                },
            ],
            expected_order: ids(&["aa-first", "zz-second"]),
        },
        SummaryCase {
            label: "a resource name and a blocker id at their published maxima",
            resource_name: "N".repeat(RESOURCE_NAME_MAX),
            parent_resource_name: Some("P"),
            blockers: vec![BlockerSeed {
                id: id_at_max_length.clone(),
                blocker_type: "AUTOMATED",
                status: "RESOLVED",
                created_reason: "resolved blocker on a long-named resource",
                created_offset: 9,
                contexts: &[("Reason", "manual")],
                resolved: Some(("resolved by the operator", 11)),
            }],
            expected_order: vec![id_at_max_length],
        },
    ]
}

/// Assert one reported blocker renders every member of the seeded row.
fn assert_blocker(case: &str, actual: &serde_json::Value, seed: &BlockerSeed) {
    assert_eq!(
        actual["Id"].as_str(),
        Some(seed.id.as_str()),
        "case {case:?}: expected blocker id {:?}, got {actual:?}",
        seed.id
    );
    assert_eq!(
        actual["Type"].as_str(),
        Some(seed.blocker_type),
        "case {case:?}: expected blocker type {:?}, got {actual:?}",
        seed.blocker_type
    );
    assert_eq!(
        actual["Status"].as_str(),
        Some(seed.status),
        "case {case:?}: expected blocker status {:?}, got {actual:?}",
        seed.status
    );
    assert_eq!(
        actual["CreatedReason"].as_str(),
        Some(seed.created_reason),
        "case {case:?}: expected created reason {:?}, got {actual:?}",
        seed.created_reason
    );
    let expected_created = (BASE_EPOCH_SECS + seed.created_offset) as f64;
    assert_eq!(
        actual["CreatedAt"].as_f64(),
        Some(expected_created),
        "case {case:?}: expected CreatedAt {expected_created}, got {actual:?}"
    );

    if seed.contexts.is_empty() {
        assert!(
            actual.get("Contexts").is_none(),
            "case {case:?}: a blocker with no context must report none, got {actual:?}"
        );
    } else {
        let reported: Vec<(&str, &str)> = actual["Contexts"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Contexts list, got {actual:?}"))
            .iter()
            .map(|context| {
                (
                    context["Key"].as_str().unwrap_or_default(),
                    context["Value"].as_str().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            reported,
            seed.contexts.to_vec(),
            "case {case:?}: expected contexts {:?}, got {actual:?}",
            seed.contexts
        );
    }

    match seed.resolved {
        None => {
            assert!(
                actual.get("ResolvedReason").is_none() && actual.get("ResolvedAt").is_none(),
                "case {case:?}: an unresolved blocker must report neither ResolvedReason nor ResolvedAt, got {actual:?}"
            );
        }
        Some((reason, offset)) => {
            assert_eq!(
                actual["ResolvedReason"].as_str(),
                Some(reason),
                "case {case:?}: expected resolved reason {reason:?}, got {actual:?}"
            );
            let expected_resolved = (BASE_EPOCH_SECS + offset) as f64;
            assert_eq!(
                actual["ResolvedAt"].as_f64(),
                Some(expected_resolved),
                "case {case:?}: expected ResolvedAt {expected_resolved}, got {actual:?}"
            );
        }
    }
}

/// Every seeded resource's summary is computed from its stored blockers: the resource
/// name it was asked about, the parent those blockers carry, and the blockers rendered
/// most recent first. One store holds every row, so a summary also reports ONLY the
/// blockers of the pair it was asked about.
#[tokio::test]
async fn the_summary_renders_the_stored_blockers() {
    let (router, engine) = router_with_engine();
    let cases = summary_cases();
    for case in &cases {
        seed(&engine, case);
    }

    for case in &cases {
        let label = case.label;
        let (status, body) = get_summary(&router, CFN_STACK_SYNC, &case.resource_name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetSyncBlockerSummary, got {status} {body}"
        );

        let summary = &body["SyncBlockerSummary"];
        assert_eq!(
            summary["ResourceName"].as_str(),
            Some(case.resource_name.as_str()),
            "case {label:?}: expected the requested resource name, got {summary}"
        );
        match case.parent_resource_name {
            None => assert!(
                summary.get("ParentResourceName").is_none(),
                "case {label:?}: a resource whose blockers carry no parent must report none, got {summary}"
            ),
            Some(parent) => assert_eq!(
                summary["ParentResourceName"].as_str(),
                Some(parent),
                "case {label:?}: expected parent {parent:?}, got {summary}"
            ),
        }

        let blockers = summary["LatestBlockers"].as_array().unwrap_or_else(|| {
            panic!("case {label:?}: expected a LatestBlockers list, got {summary}")
        });
        let reported: Vec<&str> = blockers
            .iter()
            .map(|blocker| blocker["Id"].as_str().unwrap_or_default())
            .collect();
        let expected: Vec<&str> = case.expected_order.iter().map(String::as_str).collect();
        assert_eq!(
            reported, expected,
            "case {label:?}: expected blockers {expected:?} most recent first, got {summary}"
        );

        for (position, id) in case.expected_order.iter().enumerate() {
            let seed = case
                .blockers
                .iter()
                .find(|seed| &seed.id == id)
                .unwrap_or_else(|| panic!("case {label:?}: row names an unseeded blocker {id:?}"));
            assert_blocker(&format!("{label} / {id}"), &blockers[position], seed);
        }
    }
}

/// Every value the modeled `BlockerType` and `BlockerStatus` enums declare stores and
/// reports as itself — one row per declared value, from the crate's
/// `BLOCKER_TYPE_VALUES` / `BLOCKER_STATUS_VALUES` (the enums' `values` lists), not
/// just the pair the issue cited.
#[tokio::test]
async fn every_modeled_blocker_type_and_status_round_trips() {
    let (router, engine) = router_with_engine();

    for (type_index, blocker_type) in BLOCKER_TYPE_VALUES.iter().enumerate() {
        for (status_index, status) in BLOCKER_STATUS_VALUES.iter().enumerate() {
            let resource_name = format!("Enum{type_index}x{status_index}");
            let blocker_id = format!("enum-{type_index}-{status_index}");
            let resolved = (*status == "RESOLVED").then_some(("resolved in the console", 7));
            let case = SummaryCase {
                label: "enum round trip",
                resource_name: resource_name.clone(),
                parent_resource_name: None,
                blockers: vec![BlockerSeed {
                    id: blocker_id.clone(),
                    blocker_type,
                    status,
                    created_reason: "an automated blocker",
                    created_offset: 3,
                    contexts: &[("Key", "Value")],
                    resolved,
                }],
                expected_order: vec![blocker_id],
            };
            seed(&engine, &case);

            let (http_status, body) = get_summary(&router, CFN_STACK_SYNC, &resource_name).await;
            assert_eq!(
                http_status, 200,
                "case {blocker_type}/{status}: expected 200, got {http_status} {body}"
            );
            let blockers = body["SyncBlockerSummary"]["LatestBlockers"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {blocker_type}/{status}: expected a LatestBlockers list, got {body}")
                });
            assert_eq!(
                blockers.len(),
                1,
                "case {blocker_type}/{status}: expected exactly one blocker, got {body}"
            );
            assert_blocker(
                &format!("{blocker_type}/{status}"),
                &blockers[0],
                &case.blockers[0],
            );
        }
    }
}

/// A pair the service holds no blocker for reports the EMPTY summary at 200, echoing
/// the requested resource name — including a name that differs from a stored one only
/// in case or by a suffix — and stores nothing for it.
#[tokio::test]
async fn a_resource_without_stored_blockers_reports_an_empty_summary() {
    let (router, engine) = router_with_engine();
    let held = SummaryCase {
        label: "the one resource this store holds a blocker for",
        resource_name: "HeldStack".to_string(),
        parent_resource_name: Some("HeldParent"),
        blockers: vec![BlockerSeed {
            id: "held-blocker".to_string(),
            blocker_type: "AUTOMATED",
            status: "ACTIVE",
            created_reason: "the held resource's blocker",
            created_offset: 1,
            contexts: &[],
            resolved: None,
        }],
        expected_order: ids(&["held-blocker"]),
    };
    seed(&engine, &held);
    let rows_before = stored_blockers(&engine);

    // Rows: names near the stored one and far from it. Resource names are
    // case-sensitive, so a case variant is a DIFFERENT resource.
    for resource_name in [
        "heldstack",
        "HELDSTACK",
        "HeldStack2",
        "Held",
        "AnotherStack",
        &"Z".repeat(RESOURCE_NAME_MAX),
    ] {
        let (status, body) = get_summary(&router, CFN_STACK_SYNC, resource_name).await;
        assert_eq!(
            status, 200,
            "case {resource_name:?}: expected 200 for a resource with no stored blocker, got {status} {body}"
        );
        let summary = &body["SyncBlockerSummary"];
        assert_eq!(
            summary["ResourceName"].as_str(),
            Some(resource_name),
            "case {resource_name:?}: expected the requested resource name, got {summary}"
        );
        assert_eq!(
            summary["LatestBlockers"].as_array().map(Vec::len),
            Some(0),
            "case {resource_name:?}: expected no blockers, got {summary}"
        );
        assert!(
            summary.get("ParentResourceName").is_none(),
            "case {resource_name:?}: expected no parent resource, got {summary}"
        );
    }

    // Reporting an empty summary stores nothing: the pair stays blocker-free.
    assert_eq!(
        stored_blockers(&engine),
        rows_before,
        "reporting an empty summary must not record a blocker"
    );

    // And the resource that DOES hold a blocker still reports it, so the empty summary
    // is "this pair has no blocker", not "this operation reports nothing".
    let (status, body) = get_summary(&router, CFN_STACK_SYNC, &held.resource_name).await;
    assert_eq!(
        status, 200,
        "case held: expected 200 for the stored resource, got {status} {body}"
    );
    let reported: Vec<&str> = body["SyncBlockerSummary"]["LatestBlockers"]
        .as_array()
        .unwrap_or_else(|| panic!("case held: expected a LatestBlockers list, got {body}"))
        .iter()
        .map(|blocker| blocker["Id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        reported,
        vec!["held-blocker"],
        "case held: expected the stored blocker, got {body}"
    );
}

/// A `SyncType` the model does not declare is refused as a client error (400) rather
/// than read as a sync type: the modeled enum admits exactly the values the crate's
/// `SYNC_CONFIGURATION_TYPE_VALUES` lists.
#[tokio::test]
async fn a_sync_type_outside_the_modeled_enum_is_refused() {
    let (router, engine) = router_with_engine();
    let case = SummaryCase {
        label: "a resource whose blocker is stored under the modeled sync type",
        resource_name: "TypedStack".to_string(),
        parent_resource_name: None,
        blockers: vec![BlockerSeed {
            id: "typed-blocker".to_string(),
            blocker_type: "AUTOMATED",
            status: "ACTIVE",
            created_reason: "a blocker under the modeled sync type",
            created_offset: 2,
            contexts: &[],
            resolved: None,
        }],
        expected_order: ids(&["typed-blocker"]),
    };
    seed(&engine, &case);

    assert_eq!(
        SYNC_CONFIGURATION_TYPE_VALUES.to_vec(),
        vec![CFN_STACK_SYNC],
        "the modeled SyncConfigurationType values moved; update this suite"
    );

    // Rows: a lowercased value, a near-miss, an unrelated word and the empty string.
    for sync_type in ["cfn_stack_sync", "CFN_STACK_SYNCS", "GIT_SYNC", ""] {
        let (status, body) = get_summary(&router, sync_type, &case.resource_name).await;
        assert_eq!(
            status, 400,
            "case {sync_type:?}: expected 400 for a sync type the model does not declare, got {status} {body}"
        );
    }

    // The declared value still reports the same resource's blocker.
    let (status, body) = get_summary(&router, CFN_STACK_SYNC, &case.resource_name).await;
    assert_eq!(
        status, 200,
        "case {CFN_STACK_SYNC:?}: expected 200, got {status} {body}"
    );
    assert_eq!(
        body["SyncBlockerSummary"]["LatestBlockers"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "case {CFN_STACK_SYNC:?}: expected the stored blocker, got {body}"
    );
}

/// Reporting a summary changes nothing a later caller sees: two reads of the same
/// resource answer identically, and neither adds, removes or edits a stored blocker.
#[tokio::test]
async fn reporting_a_summary_does_not_change_the_store() {
    let (router, engine) = router_with_engine();
    let case = SummaryCase {
        label: "a resource with one active blocker",
        resource_name: "StableStack".to_string(),
        parent_resource_name: Some("StableParent"),
        blockers: vec![BlockerSeed {
            id: "stable-blocker".to_string(),
            blocker_type: "AUTOMATED",
            status: "ACTIVE",
            created_reason: "stack update failed",
            created_offset: 0,
            contexts: &[("Branch", "release")],
            resolved: None,
        }],
        expected_order: ids(&["stable-blocker"]),
    };
    seed(&engine, &case);

    let rows_before = stored_blockers(&engine);

    let (first_status, first) = get_summary(&router, CFN_STACK_SYNC, &case.resource_name).await;
    let (second_status, second) = get_summary(&router, CFN_STACK_SYNC, &case.resource_name).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "case repeated read: expected 200 twice, got {first_status} {first} and {second_status} {second}"
    );
    assert_eq!(
        first, second,
        "case repeated read: expected an identical second answer, got {first} then {second}"
    );

    let (rows_after, blocker_status, resolved_at) = {
        let state = engine.state::<CodeConnectionsState>();
        let store = state.lock();
        let row = store
            .sync_blockers
            .get("stable-blocker")
            .unwrap_or_else(|| panic!("case repeated read: the stored blocker disappeared"));
        (
            store.sync_blockers.len(),
            row.status.clone(),
            row.resolved_at,
        )
    };
    assert_eq!(
        rows_after, rows_before,
        "case repeated read: expected {rows_before} stored blockers after two reads, got {rows_after}"
    );
    assert_eq!(
        blocker_status, "ACTIVE",
        "case repeated read: expected the blocker to stay ACTIVE, got {blocker_status:?}"
    );
    assert!(
        resolved_at.is_none(),
        "case repeated read: expected the blocker to stay unresolved, got {resolved_at:?}"
    );

    // Reporting an empty summary for a pair with no blocker records nothing either.
    let (status, body) = get_summary(&router, CFN_STACK_SYNC, "NeverStored").await;
    assert_eq!(
        status, 200,
        "case missing read: expected 200 with an empty summary, got {status} {body}"
    );
    assert_eq!(
        stored_blockers(&engine),
        rows_before,
        "case missing read: a read of a blocker-free pair must store nothing"
    );
}

/// Dispatch one request with the `x-amz-target` header set verbatim, so a test can
/// drive a target prefix of its own choosing.
async fn call_with_target(
    router: &MultiServiceRouter,
    target: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", target)
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDTESTCODECONN/20261021/eu-west-1/codeconnections/aws4_request, \
             SignedHeaders=content-type;host;x-amz-target, Signature=0123456789abcdef",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "target {target}: body is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// An RPC target names its operation in the segment after the LAST dot, so every form
/// of this service's target prefix reaches the same operation — including the dotted,
/// fully-qualified prefix an SDK built from this service's model sends. The rule is
/// service-wide, so it is asserted on a second operation as well, and a target whose
/// final segment names no modeled operation is still refused.
#[tokio::test]
async fn every_target_prefix_form_reaches_the_operation() {
    let (router, engine) = router_with_engine();
    let case = SummaryCase {
        label: "a resource reachable under either target prefix",
        resource_name: "RoutedStack".to_string(),
        parent_resource_name: None,
        blockers: vec![BlockerSeed {
            id: "routed-blocker".to_string(),
            blocker_type: "AUTOMATED",
            status: "ACTIVE",
            created_reason: "a blocker read under both target prefixes",
            created_offset: 4,
            contexts: &[],
            resolved: None,
        }],
        expected_order: ids(&["routed-blocker"]),
    };
    seed(&engine, &case);

    for prefix in TARGET_PREFIXES {
        let (status, body) = call_with_target(
            &router,
            &format!("{prefix}.GetSyncBlockerSummary"),
            serde_json::json!({
                "SyncType": CFN_STACK_SYNC,
                "ResourceName": case.resource_name,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {prefix:?}: expected 200 from GetSyncBlockerSummary, got {status} {body}"
        );
        assert_eq!(
            body["SyncBlockerSummary"]["ResourceName"].as_str(),
            Some(case.resource_name.as_str()),
            "case {prefix:?}: expected the requested resource name, got {body}"
        );
        assert_eq!(
            body["SyncBlockerSummary"]["LatestBlockers"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "case {prefix:?}: expected the stored blocker, got {body}"
        );

        // A second operation, so the rule is not one operation's special case: an ARN
        // naming no connection is this service's modeled 404 either way.
        let (status, body) = call_with_target(
            &router,
            &format!("{prefix}.GetConnection"),
            serde_json::json!({
                "ConnectionArn": format!(
                    "arn:aws:codeconnections:eu-west-1:{}:connection/11111111-2222-3333-4444-555555555555",
                    engine::emulator_account_id()
                ),
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {prefix:?}/GetConnection: expected 404 for an unknown ARN, got {status} {body}"
        );

        // And a target whose last segment names no modeled operation stays refused.
        let (status, body) = call_with_target(
            &router,
            &format!("{prefix}.NoSuchOperation"),
            serde_json::json!({}),
        )
        .await;
        assert!(
            status >= 400,
            "case {prefix:?}/NoSuchOperation: expected a refusal, got {status} {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|wire_type| wire_type.ends_with("NotImplemented")),
            "case {prefix:?}/NoSuchOperation: expected NotImplemented, got {status} {body}"
        );
    }
}
