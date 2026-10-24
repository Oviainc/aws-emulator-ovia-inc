// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateSyncBlocker`.
//!
//! Every call goes over the wire: a request is dispatched through the
//! [`MultiServiceRouter`] exactly as the emulator's front door does it (`POST /` +
//! `X-Amz-Target: CodeStar_connections_20191201.<Operation>`, awsJson1_0 body), so what
//! is asserted here is the service's HTTP behaviour and not a handler called directly.
//!
//! A sync blocker is `AUTOMATED` — the service's Git-sync component raises it and no
//! modeled operation creates one — so the blockers these tests resolve are put in the
//! store directly, the way `host_lifecycle.rs` reads `Host$StatusMessage` out of it for
//! a member whose wire reader is still a scaffold.
//!
//! The rules pinned, each over a TABLE rather than one request:
//!
//! * resolving a sync blocker moves the STORED blocker to `RESOLVED`, records the reason
//!   the caller gave and the instant it was resolved, and leaves everything the blocker
//!   was raised with untouched — over the published `Id` (`len 1..50`), `ResolvedReason`
//!   (`len 1..250`) and `ResourceName` (`len 1..100`) bounds, every modeled
//!   `SyncConfigurationType`, with and without a parent resource, and over context lists
//!   of zero, one and three entries;
//! * the (`Id`, `ResourceName`, `SyncType`) triple is what addresses a blocker: a triple
//!   that resolves to no stored blocker is reported ABSENT
//!   (`SyncBlockerDoesNotExistException`, at the status the recorded service answers,
//!   with a message naming the id asked for) and nothing is written;
//! * a blocker that is already `RESOLVED` cannot be resolved again: the request is
//!   refused (`InvalidInputException`) and the stored resolution is left as it was;
//! * a request outside a modeled constraint is refused and changes nothing.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

use codestar::ops::types::SyncBlockerContext;
use codestar::state::{
    ComAmazonawsCodestarconnectionsCodestarConnections20191201State as ServiceState,
    StoredSyncBlocker, BLOCKER_STATUS_ACTIVE, BLOCKER_STATUS_RESOLVED, BLOCKER_TYPE_AUTOMATED,
};

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// awsJson1_0 target prefix, from `OPERATIONS.md` ("Wire protocol").
const TARGET_PREFIX: &str = "CodeStar_connections_20191201";

/// Every value the model's `SyncConfigurationType` enum accepts.
///
/// Referenced, not copied: this is the generated catalog
/// (`src/ops/types/shared_types.rs::SYNC_CONFIGURATION_TYPE_VALUES`, from
/// `smithy_query(query="shape", shape="SyncConfigurationType")`), so the tables below
/// widen with the model instead of drifting from it.
const SYNC_TYPES: &[&str] = codestar::ops::types::shared_types::SYNC_CONFIGURATION_TYPE_VALUES;

/// One emulator: the router the production front door builds, with this service mounted
/// on a single shared engine, plus a handle on that engine so a test can seed the
/// blockers the Git-sync side raises and observe what a call persisted.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests sign nothing, so no authenticator may be registered.
        router.set_request_authentication(false);
        router.register_service(codestar::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Dispatch one awsJson1_0 call and return `(status, body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("https://codestar-connections.us-west-2.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}\
                     /codestar-connections/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(::bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("the request builds");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Put one sync blocker in the store, as the Git-sync side does.
    fn seed(&self, blocker: StoredSyncBlocker) {
        self.engine
            .state::<ServiceState>()
            .lock()
            .sync_blockers
            .insert(blocker.id.clone(), blocker);
    }

    /// The stored blocker under `id`.
    fn stored(&self, id: &str) -> StoredSyncBlocker {
        let state = self.engine.state::<ServiceState>();
        let store = state.lock();
        store
            .sync_blockers
            .get(id)
            .unwrap_or_else(|| panic!("the seeded blocker {id} is in the store"))
            .clone()
    }

    /// Every stored blocker as `(id, status, resolved_reason)`, in id order — the
    /// snapshot a refused call must not change.
    fn snapshot(&self) -> Vec<(String, String, Option<String>)> {
        let state = self.engine.state::<ServiceState>();
        let store = state.lock();
        store
            .sync_blockers
            .values()
            .map(|blocker| {
                (
                    blocker.id.clone(),
                    blocker.status.clone(),
                    blocker.resolved_reason.clone(),
                )
            })
            .collect()
    }
}

/// The error code an awsJson error envelope carries (`__type`), reduced to the shape
/// NAME: the awsJson protocols allow the code to be written with or without its
/// namespace, and a client matches on the name.
fn error_code(response: &Value) -> String {
    let raw = response["__type"].as_str().unwrap_or_default();
    raw.rsplit_once('#')
        .map_or(raw, |(_, name)| name)
        .to_string()
}

/// The `Message` an awsJson error envelope carries.
fn error_message(response: &Value) -> String {
    response["Message"]
        .as_str()
        .or_else(|| response["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// A modeled `Timestamp` as the awsJson1_0 envelope carries it: epoch seconds.
fn epoch_seconds(case: &str, value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected an epoch-seconds timestamp, got {value:?}"))
}

/// One row of the resolution table: the blocker the Git-sync side raised, and the
/// `ResolvedReason` the caller resolves it with.
struct BlockerCase {
    case: String,
    id: String,
    resource_name: String,
    parent_resource_name: Option<String>,
    sync_type: &'static str,
    created_reason: String,
    contexts: Vec<(String, String)>,
    resolved_reason: String,
}

impl BlockerCase {
    /// This row as the stored blocker the call will address, raised a minute ago so a
    /// resolution's instant is strictly later than the creation's.
    fn stored(&self) -> StoredSyncBlocker {
        StoredSyncBlocker {
            id: self.id.clone(),
            resource_name: self.resource_name.clone(),
            parent_resource_name: self.parent_resource_name.clone(),
            sync_type: self.sync_type.to_string(),
            blocker_type: BLOCKER_TYPE_AUTOMATED.to_string(),
            status: BLOCKER_STATUS_ACTIVE.to_string(),
            created_reason: self.created_reason.clone(),
            created_at: SystemTime::now() - Duration::from_secs(60),
            contexts: self
                .contexts
                .iter()
                .map(|(key, value)| SyncBlockerContext {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
            resolved_reason: None,
            resolved_at: None,
        }
    }

    /// The `UpdateSyncBlocker` request body for this row.
    fn request(&self) -> Value {
        json!({
            "Id": self.id,
            "ResourceName": self.resource_name,
            "SyncType": self.sync_type,
            "ResolvedReason": self.resolved_reason,
        })
    }
}

/// The table: one row per modeled `SyncConfigurationType`, the published `Id`
/// (`len 1..50`), `ResourceName` (`len 1..100`) and `ResolvedReason` (`len 1..250`)
/// bounds at both ends, a blocker with and without a parent resource, and context lists
/// of zero, one and three entries.
fn blocker_cases() -> Vec<BlockerCase> {
    let mut cases: Vec<BlockerCase> = SYNC_TYPES
        .iter()
        .enumerate()
        .map(|(index, sync_type)| BlockerCase {
            case: format!("sync type {sync_type}"),
            id: format!("blocker-{index}"),
            resource_name: format!("stack-{index}"),
            parent_resource_name: None,
            sync_type,
            created_reason: "The latest sync failed".to_string(),
            contexts: Vec::new(),
            resolved_reason: "Fixed the template".to_string(),
        })
        .collect();

    let sync_type = SYNC_TYPES[0];
    cases.push(BlockerCase {
        case: "shortest modeled id, resource name and resolved reason".to_string(),
        id: "b".to_string(),
        resource_name: "r".to_string(),
        parent_resource_name: None,
        sync_type,
        created_reason: "x".to_string(),
        contexts: Vec::new(),
        resolved_reason: "y".to_string(),
    });
    cases.push(BlockerCase {
        case: "longest modeled id, resource name and resolved reason".to_string(),
        // `Id` is `len 1..50`, `ResourceName` `len 1..100` (pattern
        // `^[0-9A-Za-z]+[0-9A-Za-z_\-]*$`) and `ResolvedReason` `len 1..250`: each of
        // these is exactly the published maximum.
        id: "i".repeat(50),
        resource_name: format!("S{}", "n".repeat(99)),
        parent_resource_name: Some(format!("P{}", "p".repeat(99))),
        sync_type,
        created_reason: "c".repeat(600),
        contexts: Vec::new(),
        resolved_reason: "r".repeat(250),
    });
    cases.push(BlockerCase {
        case: "a parent resource and one context".to_string(),
        id: "blocker-with-parent".to_string(),
        resource_name: "child_stack-1".to_string(),
        parent_resource_name: Some("parent_stack-1".to_string()),
        sync_type,
        created_reason: "Drift detected on the parent stack".to_string(),
        contexts: vec![("Reason".to_string(), "DRIFT".to_string())],
        resolved_reason: "Reverted the manual change".to_string(),
    });
    cases.push(BlockerCase {
        case: "three contexts".to_string(),
        id: "blocker-three-contexts".to_string(),
        resource_name: "stack-three-contexts".to_string(),
        parent_resource_name: None,
        sync_type,
        created_reason: "The latest commit could not be applied".to_string(),
        contexts: vec![
            ("Commit".to_string(), "abc123".to_string()),
            ("Branch".to_string(), "main".to_string()),
            ("File".to_string(), "template.yml".to_string()),
        ],
        resolved_reason: "Pushed a fixed commit".to_string(),
    });
    cases
}

/// RULE: `UpdateSyncBlocker` resolves the stored sync blocker its (`Id`, `ResourceName`,
/// `SyncType`) triple addresses — the stored status moves to `RESOLVED` and records the
/// caller's reason and the instant of the resolution, while everything the blocker was
/// raised with stays as raised — and reports that stored blocker back.
#[tokio::test]
async fn resolving_a_sync_blocker_persists_the_resolution_and_reports_the_stored_blocker() {
    let emulator = Emulator::new();

    // Every row is seeded up front, so each resolution runs against a store holding
    // other blockers too and has to find the one its triple names.
    let cases = blocker_cases();
    for case in &cases {
        emulator.seed(case.stored());
    }

    for blocker in &cases {
        let case = blocker.case.as_str();
        let raised = emulator.stored(&blocker.id);
        let (status, response) = emulator.call("UpdateSyncBlocker", blocker.request()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateSyncBlocker to answer 200, got {status} {response:?}"
        );

        assert_eq!(
            response["ResourceName"].as_str(),
            Some(blocker.resource_name.as_str()),
            "case {case:?}: wrong ResourceName in {response:?}"
        );
        assert_eq!(
            response["ParentResourceName"].as_str(),
            blocker.parent_resource_name.as_deref(),
            "case {case:?}: wrong ParentResourceName in {response:?}"
        );

        let reported = &response["SyncBlocker"];
        assert_eq!(
            reported["Id"].as_str(),
            Some(blocker.id.as_str()),
            "case {case:?}: wrong SyncBlocker.Id in {response:?}"
        );
        assert_eq!(
            reported["Status"].as_str(),
            Some(BLOCKER_STATUS_RESOLVED),
            "case {case:?}: a resolved blocker reports RESOLVED, got {response:?}"
        );
        assert_eq!(
            reported["ResolvedReason"].as_str(),
            Some(blocker.resolved_reason.as_str()),
            "case {case:?}: wrong SyncBlocker.ResolvedReason in {response:?}"
        );
        // Raised as raised: type, reason, creation time and contexts are reported from
        // the stored row and are not touched by the resolution.
        assert_eq!(
            reported["Type"].as_str(),
            Some(BLOCKER_TYPE_AUTOMATED),
            "case {case:?}: wrong SyncBlocker.Type in {response:?}"
        );
        assert_eq!(
            reported["CreatedReason"].as_str(),
            Some(blocker.created_reason.as_str()),
            "case {case:?}: wrong SyncBlocker.CreatedReason in {response:?}"
        );
        if blocker.contexts.is_empty() {
            assert!(
                reported["Contexts"].is_null(),
                "case {case:?}: a blocker raised with no contexts must report none, got \
                 {response:?}"
            );
        } else {
            let expected: Vec<Value> = blocker
                .contexts
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect();
            assert_eq!(
                reported["Contexts"],
                Value::Array(expected),
                "case {case:?}: wrong SyncBlocker.Contexts in {response:?}"
            );
        }

        let created_at = epoch_seconds(case, &reported["CreatedAt"]);
        let resolved_at = epoch_seconds(case, &reported["ResolvedAt"]);
        assert!(
            resolved_at > created_at,
            "case {case:?}: a blocker is resolved after it was raised, got \
             ResolvedAt {resolved_at} and CreatedAt {created_at}"
        );

        // What was PERSISTED, which is what any later reader of this blocker answers from.
        let stored = emulator.stored(&blocker.id);
        assert_eq!(
            stored.status, BLOCKER_STATUS_RESOLVED,
            "case {case:?}: the stored blocker was not moved to RESOLVED, got {stored:?}"
        );
        assert_eq!(
            stored.resolved_reason.as_deref(),
            Some(blocker.resolved_reason.as_str()),
            "case {case:?}: the stored blocker holds the wrong ResolvedReason, got {stored:?}"
        );
        assert!(
            stored
                .resolved_at
                .is_some_and(|at| at >= raised.created_at),
            "case {case:?}: the stored blocker holds no resolution instant after its \
             creation, got {stored:?}"
        );
        assert_eq!(
            stored.created_at, raised.created_at,
            "case {case:?}: the resolution moved the blocker's creation time, got {stored:?}"
        );
        assert_eq!(
            stored.created_reason, raised.created_reason,
            "case {case:?}: the resolution rewrote the blocker's creation reason, got {stored:?}"
        );
        assert_eq!(
            stored.contexts.len(),
            raised.contexts.len(),
            "case {case:?}: the resolution changed the blocker's contexts, got {stored:?}"
        );

        // The resolution is persisted, read back over the WIRE: the same call again now
        // addresses a blocker that is no longer active, which only a stored resolution
        // can produce.
        let (status, again) = emulator.call("UpdateSyncBlocker", blocker.request()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a second resolution to be refused, got {status} {again:?}"
        );
        assert_eq!(
            error_code(&again),
            "InvalidInputException",
            "case {case:?}: wrong error code for an already-resolved blocker in {again:?}"
        );
    }
}

/// RULE: a sync blocker is addressed by its `Id` together with the `ResourceName` and
/// `SyncType` it was raised for, so a triple that resolves to no stored blocker is
/// reported ABSENT — `SyncBlockerDoesNotExistException`, with a message naming the id
/// that was asked for — and the failed call writes nothing.
#[tokio::test]
async fn a_triple_naming_no_sync_blocker_is_reported_absent() {
    let emulator = Emulator::new();

    // Blockers DO exist, so a refusal comes from the lookup and not from an empty store.
    let present = blocker_cases();
    for case in &present {
        emulator.seed(case.stored());
    }
    let held = &present[0];

    // (case, Id, ResourceName) — each pair satisfies every modeled constraint and
    // addresses no stored blocker.
    let unknown: Vec<(&str, String, String)> = vec![
        (
            "an id never raised",
            "nonexistent-blocker-id".to_string(),
            held.resource_name.clone(),
        ),
        (
            "the longest modeled id, never raised",
            "z".repeat(50),
            held.resource_name.clone(),
        ),
        (
            "an id raised for another resource",
            held.id.clone(),
            "some-other-stack".to_string(),
        ),
        (
            "an id differing only in case",
            held.id.to_uppercase(),
            held.resource_name.clone(),
        ),
        (
            "an id that is a prefix of one raised",
            held.id.chars().take(3).collect(),
            held.resource_name.clone(),
        ),
    ];

    let before = emulator.snapshot();
    for (case, id, resource_name) in &unknown {
        let (status, response) = emulator
            .call(
                "UpdateSyncBlocker",
                json!({
                    "Id": id,
                    "ResourceName": resource_name,
                    "SyncType": SYNC_TYPES[0],
                    "ResolvedReason": "nothing to resolve",
                }),
            )
            .await;
        // The model puts `@httpError 404` on `SyncBlockerDoesNotExistException`, but the
        // recorded wire response for a blocker id that addresses nothing is HTTP 400 with
        // that same shape and message — the status a client of the real service branches
        // on; see `SYNC_BLOCKER_DOES_NOT_EXIST_STATUS` in `src/ops/update_sync_blocker.rs`.
        assert_eq!(
            status, 400,
            "case {case:?}: expected UpdateSyncBlocker to refuse an absent blocker, got \
             {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "SyncBlockerDoesNotExistException",
            "case {case:?}: wrong error code in {response:?}"
        );
        assert_eq!(
            error_message(&response),
            format!("Sync blocker with id {id} does not exist "),
            "case {case:?}: the message must name the id that was asked for, got {:?}",
            error_message(&response)
        );
    }
    assert_eq!(
        emulator.snapshot(),
        before,
        "a refused UpdateSyncBlocker changed the store"
    );

    // The blocker that IS held still resolves.
    let (status, response) = emulator.call("UpdateSyncBlocker", held.request()).await;
    assert_eq!(
        status, 200,
        "the held blocker must still resolve, got {status} {response:?}"
    );
}

/// RULE: resolving is defined on a blocker that is still blocking, so a blocker already
/// `RESOLVED` is refused (`InvalidInputException`) and its stored resolution is left
/// exactly as it was.
#[tokio::test]
async fn an_already_resolved_sync_blocker_cannot_be_resolved_again() {
    let emulator = Emulator::new();

    // (case, the reason the blocker was resolved with, the reason the second call offers)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "resolved once",
            "Fixed the template".to_string(),
            "Fixed it again".to_string(),
        ),
        (
            "resolved with the shortest modeled reason",
            "y".to_string(),
            "z".to_string(),
        ),
        (
            "resolved with the longest modeled reason",
            "r".repeat(250),
            "s".repeat(250),
        ),
        (
            "re-offered the very same reason",
            "Reverted the manual change".to_string(),
            "Reverted the manual change".to_string(),
        ),
    ];

    for (index, (case, first_reason, second_reason)) in cases.iter().enumerate() {
        let id = format!("resolved-blocker-{index}");
        let resource_name = format!("stack-{index}");
        let blocker = BlockerCase {
            case: (*case).to_string(),
            id: id.clone(),
            resource_name: resource_name.clone(),
            parent_resource_name: None,
            sync_type: SYNC_TYPES[0],
            created_reason: "The latest sync failed".to_string(),
            contexts: Vec::new(),
            resolved_reason: first_reason.clone(),
        };
        emulator.seed(blocker.stored());

        let (status, first) = emulator.call("UpdateSyncBlocker", blocker.request()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first resolution to answer 200, got {status} {first:?}"
        );
        let resolved = emulator.stored(&id);

        let (status, second) = emulator
            .call(
                "UpdateSyncBlocker",
                json!({
                    "Id": id,
                    "ResourceName": resource_name,
                    "SyncType": SYNC_TYPES[0],
                    "ResolvedReason": second_reason,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected an already-resolved blocker to be refused, got \
             {status} {second:?}"
        );
        assert_eq!(
            error_code(&second),
            "InvalidInputException",
            "case {case:?}: wrong error code in {second:?}"
        );
        assert!(
            error_message(&second).contains(id.as_str()),
            "case {case:?}: the message must name the id, got {:?}",
            error_message(&second)
        );

        let after = emulator.stored(&id);
        assert_eq!(
            after.resolved_reason, resolved.resolved_reason,
            "case {case:?}: the refused call rewrote the stored ResolvedReason, got {after:?}"
        );
        assert_eq!(
            after.resolved_at, resolved.resolved_at,
            "case {case:?}: the refused call rewrote the stored resolution instant, got {after:?}"
        );
    }
}

/// RULE: an `UpdateSyncBlocker` request outside a modeled constraint is refused before
/// the blocker is touched — `Id` is `len 1..50`, `ResolvedReason` is `len 1..250`,
/// `ResourceName` is `len 1..100` with pattern `^[0-9A-Za-z]+[0-9A-Za-z_\-]*$`,
/// `SyncType` is a closed enum, and all four members are required — so one past each
/// published bound is refused and nothing changes.
#[tokio::test]
async fn a_request_outside_the_modeled_constraints_changes_nothing() {
    let emulator = Emulator::new();

    // One blocker at the published maxima, so every refusal below comes from the
    // constraint rather than from an absence.
    let held = BlockerCase {
        case: "held".to_string(),
        id: "i".repeat(50),
        resource_name: format!("S{}", "n".repeat(99)),
        parent_resource_name: None,
        sync_type: SYNC_TYPES[0],
        created_reason: "The latest sync failed".to_string(),
        contexts: Vec::new(),
        resolved_reason: "r".repeat(250),
    };
    emulator.seed(held.stored());

    let one_past_max_id = "i".repeat(51);
    let one_past_max_reason = "r".repeat(251);
    let one_past_max_resource = format!("S{}", "n".repeat(100));
    let cases: Vec<(&str, Value)> = vec![
        (
            "Id one past the published maximum",
            json!({
                "Id": one_past_max_id,
                "ResourceName": held.resource_name,
                "SyncType": SYNC_TYPES[0],
                "ResolvedReason": held.resolved_reason,
            }),
        ),
        (
            "an empty Id",
            json!({
                "Id": "",
                "ResourceName": held.resource_name,
                "SyncType": SYNC_TYPES[0],
                "ResolvedReason": held.resolved_reason,
            }),
        ),
        (
            "ResolvedReason one past the published maximum",
            json!({
                "Id": held.id,
                "ResourceName": held.resource_name,
                "SyncType": SYNC_TYPES[0],
                "ResolvedReason": one_past_max_reason,
            }),
        ),
        (
            "an empty ResolvedReason",
            json!({
                "Id": held.id,
                "ResourceName": held.resource_name,
                "SyncType": SYNC_TYPES[0],
                "ResolvedReason": "",
            }),
        ),
        (
            "ResourceName one past the published maximum",
            json!({
                "Id": held.id,
                "ResourceName": one_past_max_resource,
                "SyncType": SYNC_TYPES[0],
                "ResolvedReason": held.resolved_reason,
            }),
        ),
        (
            "a ResourceName outside the published pattern",
            json!({
                "Id": held.id,
                "ResourceName": "_leading-underscore",
                "SyncType": SYNC_TYPES[0],
                "ResolvedReason": held.resolved_reason,
            }),
        ),
        (
            "a SyncType the enum does not declare",
            json!({
                "Id": held.id,
                "ResourceName": held.resource_name,
                "SyncType": "NOT_A_SYNC_TYPE",
                "ResolvedReason": held.resolved_reason,
            }),
        ),
        (
            "no ResolvedReason at all",
            json!({
                "Id": held.id,
                "ResourceName": held.resource_name,
                "SyncType": SYNC_TYPES[0],
            }),
        ),
        (
            "no Id at all",
            json!({
                "ResourceName": held.resource_name,
                "SyncType": SYNC_TYPES[0],
                "ResolvedReason": held.resolved_reason,
            }),
        ),
    ];

    let before = emulator.snapshot();
    for (case, body) in cases {
        let (status, response) = emulator.call("UpdateSyncBlocker", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside a modeled constraint was accepted: \
             {status} {response:?}"
        );
        assert_eq!(
            emulator.snapshot(),
            before,
            "case {case:?}: a refused request changed the store"
        );
    }

    // The request that satisfies every constraint is the one that is accepted.
    let (status, response) = emulator.call("UpdateSyncBlocker", held.request()).await;
    assert_eq!(
        status, 200,
        "a request inside every modeled constraint was refused: {status} {response:?}"
    );
    assert_eq!(
        emulator.stored(&held.id).status,
        BLOCKER_STATUS_RESOLVED,
        "the accepted request did not resolve the blocker"
    );
}
