// SPDX-License-Identifier: CC0-1.0

//! Which refusal `RestoreGraphFromSnapshot` answers when a request breaks BOTH of its
//! conditions at once, driven over the wire (`restJson1`) through the same router the
//! emulator serves from and set up through PUBLIC CALLS ONLY — so every state a row is
//! about is one this service's own accepted calls produce, rather than one a test wrote
//! into the store.
//!
//! The rule every assertion here is about: an operation that mints a record decides the
//! refusal that depends only on what the store ALREADY HOLDS — the identity the minted
//! record would reserve being taken — BEFORE any precondition on the status of the record
//! it mints from, so a request that breaks both at once is answered by the taken-identity
//! conflict. Nothing the source record goes on to do frees a reserved identity, and the
//! two refusals ask the caller for different things (pick another name, versus wait), so
//! which one comes back is part of the answer rather than an accident of the order the
//! checks happen to be written in.
//!
//! The rows span what the rule generalises over, in the three dimensions this operation
//! has:
//!
//! * the status of the SOURCE SNAPSHOT the restore reads — both of the non-terminal
//!   statuses the modelled `SnapshotStatus` carries that this service's own calls reach
//!   (`CREATING` while a capture is outstanding, `DELETING` once a deletion is accepted;
//!   `smithy_query(query="shape",shape="SnapshotStatus")` lists `CREATING`, `AVAILABLE`,
//!   `DELETING`, `FAILED`, and `AVAILABLE` is the one the gate accepts);
//! * which operation RESERVED the colliding `graphName` — every operation of this service
//!   that mints a graph (`CreateGraph`, `CreateGraphUsingImportTask` and
//!   `RestoreGraphFromSnapshot` itself), since the uniqueness is keyed on one index all
//!   three write;
//! * what the graph holding that name is doing — in service, still coming up in the
//!   transient status its minting call reported, or being deleted (a graph holds its name
//!   until the deletion completes).
//!
//! Every row also asserts the CONTROL: the identical restore, sent immediately after and
//! differing only in asking for a FREE name, is refused by the source snapshot's status
//! gate, naming the status the row put the snapshot in. A snapshot only ever moves
//! forward (`CREATING` to `AVAILABLE`, `DELETING` to gone), so a control taken after the
//! row's own call proves the snapshot was in that status for the call too — which is what
//! makes each row a measurement of precedence rather than of one refusal having swallowed
//! the other.
//!
//! The store-level half of the same rule for the other minting operations of this
//! service, and the full-store refusal, are covered by `store_refusal_precedence.rs`
//! against parked records; the snapshot namespace's wire-driven row is
//! `duplicate_name_conflicts.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`ProvisionedMemory` is
/// `@range(16..24576)`), which is all a graph needs to be here.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`, for the import
/// form of a graph create.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneImportRole";

/// An operation of this service that mints a graph, and so reserves a `graphName`.
#[derive(Clone, Copy, Debug)]
enum Reserver {
    CreateGraph,
    CreateGraphUsingImportTask,
    RestoreGraphFromSnapshot,
}

/// What the graph holding the colliding name is doing when the restore collides with it.
#[derive(Clone, Copy, Debug)]
enum HolderState {
    /// Left in the transient status its minting call reported (`CREATING`, or `IMPORTING`
    /// for the import form), with nothing waited out.
    JustMinted,
    /// Waited into service, so it reports `AVAILABLE`.
    InService,
    /// Its deletion accepted, so it reports `DELETING` and still holds its name.
    Deleting,
}

/// The status the source snapshot is in when the restore reads it — the two non-terminal
/// statuses an accepted call of this service puts a snapshot into.
#[derive(Clone, Copy, Debug)]
enum SourceState {
    /// A capture that has just been started and is still outstanding.
    Creating,
    /// A complete snapshot whose deletion has been accepted.
    Deleting,
}

impl SourceState {
    /// The status the refusal names, as the modelled enum spells it.
    fn wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Deleting => "DELETING",
        }
    }
}

/// One request that is wrong in BOTH ways at once.
#[derive(Clone, Copy, Debug)]
struct Row {
    case: &'static str,
    reserver: Reserver,
    holder_state: HolderState,
    source: SourceState,
    /// The `graphName` the Region already holds when the restore asks for it.
    taken: &'static str,
    /// A `graphName` nothing holds, for the row's control call.
    free: &'static str,
}

/// An emulator holding only this service, on its own engine, so each row starts from an
/// empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this test
/// does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, case: &str, path: &str) {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from {path}, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {case:?}: {path} never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// `POST /graphs` — reserve `name` through `CreateGraph`, returning the minted graph id.
async fn create_graph(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        Some(json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph for {name:?}, got {status}: {created}"
    );
    graph_id(case, &created, "id")
}

/// `POST /importtasks` — reserve `name` through `CreateGraphUsingImportTask`, returning
/// the minted graph id.
async fn create_graph_using_import(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/importtasks",
        Some(json!({
            "graphName": name,
            "roleArn": ROLE_ARN,
            "source": "s3://import-bucket/seed",
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraphUsingImportTask for {name:?}, \
         got {status}: {created}"
    );
    graph_id(case, &created, "graphId")
}

/// `POST /snapshots/{snapshotIdentifier}/restore` — the call under test, which is also
/// how one row reserves the colliding name.
async fn restore(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    graph_name: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/snapshots/{snapshot_id}/restore"),
        Some(json!({"graphName": graph_name})),
    )
    .await
}

/// `POST /snapshots` — take a complete snapshot off `graph_id`: the source graph goes
/// through `SNAPSHOTTING` and the snapshot through `CREATING`, so both are waited back
/// into service before the caller's next call.
async fn settled_snapshot(
    router: &MultiServiceRouter,
    case: &str,
    graph_id: &str,
    name: &str,
) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": graph_id, "snapshotName": name})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraphSnapshot for {name:?}, \
         got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no snapshot id in {created}"))
        .to_string();
    wait_until_available(router, case, &format!("/graphs/{graph_id}")).await;
    wait_until_available(router, case, &format!("/snapshots/{id}")).await;
    id
}

/// The graph id a minting response reports under `member`.
fn graph_id(case: &str, body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {body}"))
        .to_string()
}

/// The modelled `GraphStatus` the graph holding the colliding name reports while the row
/// runs: the transient status its minting operation leaves it in (`IMPORTING` for the
/// import form of a create, `CREATING` for the others), the status it rests in once
/// waited out, or the one an accepted deletion puts it in
/// (`smithy_query(query="shape",shape="GraphStatus")`).
fn holder_status(row: Row) -> &'static str {
    match (row.holder_state, row.reserver) {
        (HolderState::JustMinted, Reserver::CreateGraphUsingImportTask) => "IMPORTING",
        (HolderState::JustMinted, _) => "CREATING",
        (HolderState::InService, _) => "AVAILABLE",
        (HolderState::Deleting, _) => "DELETING",
    }
}

/// Assert one refusal of this operation: the status, the modelled error code and the
/// whole message, so a row cannot pass on a refusal that merely has the right class.
fn assert_refusal(case: &str, which: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 409,
        "case {case:?}: expected 409 from the {which} call, got {status}: {body}"
    );
    let code = error_code(body);
    assert_eq!(
        code, "ConflictException",
        "case {case:?}: expected ConflictException from the {which} call, got {code:?}: {body}"
    );
    let actual = message(body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the {which} call to answer {expected:?}, got {actual:?}"
    );
}

/// Every graph name the store currently reports, with the id holding it.
async fn graph_names(router: &MultiServiceRouter, case: &str) -> Vec<(String, String)> {
    let (status, listed) = call(router, "GET", "/graphs", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListGraphs, got {status}: {listed}"
    );
    listed["graphs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no graphs list in {listed}"))
        .iter()
        .map(|graph| {
            (
                graph["name"].as_str().unwrap_or_default().to_string(),
                graph["id"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A restore whose `graphName` the Region already holds is answered by that name
/// conflict even when the snapshot it restores from is in a status the restore's own
/// state gate refuses — and the identical restore asking for a free name, sent
/// immediately after, is answered by that state gate.
#[tokio::test]
async fn a_taken_graph_name_outranks_the_source_snapshots_status() {
    let rows = [
        Row {
            case: "a snapshot still being created and a name a graph in service holds",
            reserver: Reserver::CreateGraph,
            holder_state: HolderState::InService,
            source: SourceState::Creating,
            taken: "restore-target",
            free: "restore-free",
        },
        Row {
            case: "a snapshot still being created and a name an import-task create has \
                   just minted",
            reserver: Reserver::CreateGraphUsingImportTask,
            holder_state: HolderState::JustMinted,
            source: SourceState::Creating,
            taken: "imported-target-7",
            free: "imported-free",
        },
        Row {
            case: "a snapshot still being created and a name an earlier restore's graph \
                   is being deleted under",
            reserver: Reserver::RestoreGraphFromSnapshot,
            holder_state: HolderState::Deleting,
            source: SourceState::Creating,
            taken: "restored-earlier-target",
            free: "restored-earlier-free",
        },
        Row {
            case: "a snapshot being deleted and a name CreateGraph has just minted",
            reserver: Reserver::CreateGraph,
            holder_state: HolderState::JustMinted,
            source: SourceState::Deleting,
            taken: "a",
            free: "deleting-source-free",
        },
        Row {
            case: "a snapshot being deleted and a name an import-task create holds in \
                   service",
            reserver: Reserver::CreateGraphUsingImportTask,
            holder_state: HolderState::InService,
            source: SourceState::Deleting,
            taken: "imported-in-service-2",
            free: "imported-in-service-free",
        },
        Row {
            case: "a snapshot being deleted and a name an earlier restore's graph is \
                   being deleted under",
            reserver: Reserver::RestoreGraphFromSnapshot,
            holder_state: HolderState::Deleting,
            source: SourceState::Deleting,
            taken: "restored-then-deleted",
            free: "restored-then-deleted-free",
        },
    ];

    for row in rows {
        let case = row.case;
        let router = emulator();

        // A graph in service to take snapshots off. Its own name is none of the names a
        // row collides on.
        let feeder = create_graph(&router, case, "precedence-feeder").await;
        wait_until_available(&router, case, &format!("/graphs/{feeder}")).await;

        // The snapshot the row that reserves its name through a restore mints that graph
        // out of: complete, so that restore is accepted.
        let reserved_from = match row.reserver {
            Reserver::RestoreGraphFromSnapshot => {
                Some(settled_snapshot(&router, case, &feeder, "precedence-capture").await)
            }
            _ => None,
        };

        // The snapshot this row's restore reads. A row whose source is being DELETED
        // needs it complete first, so it is taken (and waited out) here; a row whose
        // source is still being CREATED takes it below, with nothing waited.
        let source = match row.source {
            SourceState::Deleting => {
                Some(settled_snapshot(&router, case, &feeder, "precedence-source").await)
            }
            SourceState::Creating => None,
        };

        // The graph that RESERVES the colliding name, through each operation that mints
        // one.
        let holder = match (row.reserver, reserved_from.as_deref()) {
            (Reserver::CreateGraph, _) => create_graph(&router, case, row.taken).await,
            (Reserver::CreateGraphUsingImportTask, _) => {
                create_graph_using_import(&router, case, row.taken).await
            }
            (Reserver::RestoreGraphFromSnapshot, Some(snapshot)) => {
                let (status, restored) = restore(&router, snapshot, row.taken).await;
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected 201 from the restore that reserves \
                     {:?}, got {status}: {restored}",
                    row.taken
                );
                graph_id(case, &restored, "id")
            }
            (Reserver::RestoreGraphFromSnapshot, None) => {
                panic!("case {case:?}: a restore row needs a settled snapshot to mint from")
            }
        };
        if !matches!(row.holder_state, HolderState::JustMinted) {
            wait_until_available(&router, case, &format!("/graphs/{holder}")).await;
        }

        // From here nothing waits: each transient status below is owed back by the clock
        // a fixed span after the call that starts it, and the row is about the window
        // while both are still outstanding.
        if matches!(row.holder_state, HolderState::Deleting) {
            let (status, deleted) = call(
                &router,
                "DELETE",
                &format!("/graphs/{holder}?skipSnapshot=true"),
                None,
            )
            .await;
            assert_eq!(
                (status, &deleted["status"]),
                (200, &json!("DELETING")),
                "case {case:?}: expected 200 and DELETING from DeleteGraph on the name \
                 holder, got {status}: {deleted}"
            );
        }

        let source = match (row.source, source) {
            (SourceState::Creating, _) => {
                let (status, created) = call(
                    &router,
                    "POST",
                    "/snapshots",
                    Some(json!({
                        "graphIdentifier": feeder,
                        "snapshotName": "precedence-source",
                    })),
                )
                .await;
                assert_eq!(
                    (status, &created["status"]),
                    (201, &json!("CREATING")),
                    "case {case:?}: expected 201 and CREATING from CreateGraphSnapshot, \
                     got {status}: {created}"
                );
                created["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no snapshot id in {created}"))
                    .to_string()
            }
            (SourceState::Deleting, Some(id)) => {
                let (status, deleted) =
                    call(&router, "DELETE", &format!("/snapshots/{id}"), None).await;
                assert_eq!(
                    (status, &deleted["status"]),
                    (200, &json!("DELETING")),
                    "case {case:?}: expected 200 and DELETING from DeleteGraphSnapshot, \
                     got {status}: {deleted}"
                );
                id
            }
            (SourceState::Deleting, None) => {
                panic!("case {case:?}: a deleting-source row needs a settled snapshot")
            }
        };

        // The request that is wrong in BOTH ways: a name the Region holds, off a snapshot
        // the restore's own state gate refuses.
        let (status, refused) = restore(&router, &source, row.taken).await;
        assert_refusal(
            case,
            "taken-name",
            status,
            &refused,
            &format!("Graph with name {} already exists!", row.taken),
        );

        // The CONTROL, sent after it and differing only in the name: the state gate, so
        // the source snapshot really was in the status this row put it in — for this call
        // and therefore for the one above it, since a snapshot only moves forward.
        let (status, control) = restore(&router, &source, row.free).await;
        assert_refusal(
            case,
            "free-name",
            status,
            &control,
            &format!(
                "Unable to restore the graph snapshot because it is in the {} state.",
                row.source.wire()
            ),
        );

        // The graph holding the name was still in the state this row is about AFTER both
        // calls, so it was in it for both: a graph's status moves forward too, and one
        // whose deletion had completed would have freed the name instead.
        let (status, holding) = call(&router, "GET", &format!("/graphs/{holder}"), None).await;
        let expected_status = holder_status(row);
        assert_eq!(
            (status, &holding["status"]),
            (200, &json!(expected_status)),
            "case {case:?}: expected the name holder to report {expected_status} across \
             both calls, got {status}: {holding}"
        );

        // Neither refusal minted anything or disturbed what the store holds: the taken
        // name is still the one graph that reserved it, and the control's name is held by
        // no graph at all.
        let held = graph_names(&router, case).await;
        let holding_taken: Vec<&String> = held
            .iter()
            .filter(|(name, _)| name == row.taken)
            .map(|(_, id)| id)
            .collect();
        assert_eq!(
            holding_taken,
            vec![&holder],
            "case {case:?}: expected {:?} to be held by {holder} alone, got {held:?}",
            row.taken
        );
        assert!(
            !held.iter().any(|(name, _)| name == row.free),
            "case {case:?}: the refused control must mint no graph named {:?}, got {held:?}",
            row.free
        );
    }
}
