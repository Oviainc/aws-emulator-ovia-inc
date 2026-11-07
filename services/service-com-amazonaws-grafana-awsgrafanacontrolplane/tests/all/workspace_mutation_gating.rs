// SPDX-License-Identifier: CC0-1.0

//! When a change to a workspace is ACCEPTED and when it is REFUSED as a conflict, end to
//! end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! THE RULES UNDER TEST, both consequences of this service answering `202 Accepted` for a
//! change it has not finished:
//!
//! 1. **A workspace being deleted exists until the removal falls due.** `DeleteWorkspace`
//!    answers 202 with the workspace in `DELETING` and the clock owes the store its
//!    removal, so a poller reads `DELETING`, and the operation being `@idempotent` a
//!    repeated delete of a deletion already in flight is accepted again with the same
//!    record rather than refused. Once the removal falls due the id names nothing and
//!    every workspace operation answers `ResourceNotFoundException`.
//! 2. **A change to a workspace whose previous change is still in flight is a conflict.**
//!    Every operation that mutates a workspace asynchronously (the two the model gives
//!    both a 202 success status and a `ConflictException`) refuses a workspace in one of
//!    the progressive `WorkspaceStatus` values with HTTP 409 `ConflictException`, naming
//!    the status it is in, and stores nothing; once the clock has settled the workspace
//!    the same call is accepted.
//!
//! The tables span what the rules generalise over: the in-flight status the workspace is
//! in (`CREATING`, `UPDATING`, `DELETING`, each reached through the call that leaves it
//! there), the mutating operation (both of them), the settled status a change is accepted
//! in, and — for the delete lifecycle — every workspace operation that reads or changes
//! the record.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_grafana_awsgrafanacontrolplane::state::LIFECYCLE_TRANSITION;
use service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/grafana/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The error code a refused change carries, spelled out here rather than imported so a
/// wrong code in the implementation fails.
const CONFLICT_CODE: &str = "ConflictException";

/// The `resourceType` a workspace refusal names.
const WORKSPACE_TYPE: &str = "Workspace";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error message, or the empty string when the answer carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The `workspace` member a workspace operation answers with.
    fn workspace(&self) -> &Value {
        &self.body["workspace"]
    }

    /// That workspace's `status`.
    fn status_member(&self) -> &str {
        self.workspace()["status"].as_str().unwrap_or_default()
    }

    /// That workspace's id.
    fn id(&self) -> String {
        self.workspace()["id"]
            .as_str()
            .unwrap_or_else(|| panic!("the answer carries no workspace id: {:?}", self.body))
            .to_string()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "grafana.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The three members `CreateWorkspace` requires.
fn minimal_create() -> Value {
    json!({
        "accountAccessType": "CURRENT_ACCOUNT",
        "permissionType": "CUSTOMER_MANAGED",
        "authenticationProviders": ["AWS_SSO"],
    })
}

async fn create(router: &MultiServiceRouter) -> Answer {
    call(router, "POST", "/workspaces", Some(minimal_create())).await
}

async fn describe(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/workspaces/{id}"), None).await
}

async fn delete(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "DELETE", &format!("/workspaces/{id}"), None).await
}

async fn update(router: &MultiServiceRouter, id: &str, body: Value) -> Answer {
    call(router, "PUT", &format!("/workspaces/{id}"), Some(body)).await
}

async fn update_configuration(router: &MultiServiceRouter, id: &str, body: Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("/workspaces/{id}/configuration"),
        Some(body),
    )
    .await
}

async fn describe_configuration(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/workspaces/{id}/configuration"), None).await
}

async fn list(router: &MultiServiceRouter) -> Vec<String> {
    let answer = call(router, "GET", "/workspaces", None).await;
    answer.body["workspaces"]
        .as_array()
        .unwrap_or_else(|| panic!("ListWorkspaces must answer a list: {:?}", answer.body))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Wait out the span the clock owes a workspace its next state in: `ACTIVE` after a create
/// or an update, its REMOVAL after an accepted delete. The transition is owed by the clock,
/// not by a call: nothing is done here but wait.
async fn settled() {
    tokio::time::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(200)).await;
}

/// Create a workspace and hand it over settled, so the case decides what state it is in.
async fn active_workspace(router: &MultiServiceRouter) -> String {
    let created = create(router).await;
    assert_eq!(
        created.status, 202,
        "the fixture create must be accepted, got {} with {:?}",
        created.status, created.body
    );
    let id = created.id();
    settled().await;
    assert_eq!(
        describe(router, &id).await.status_member(),
        "ACTIVE",
        "the fixture workspace must settle before the case runs"
    );
    id
}

/// The operations that change a workspace asynchronously: the ones the model gives both a
/// 202 success status and a `ConflictException`
/// (`smithy_query(query="digest")` — `UpdateWorkspace` and `UpdateWorkspaceConfiguration`).
/// Each row carries a request the operation would otherwise accept, and a reader of the
/// member it would have written.
struct Mutation {
    operation: &'static str,
    request: fn() -> Value,
}

/// The two mutating operations under the conflict rule.
fn mutations() -> Vec<Mutation> {
    vec![
        Mutation {
            operation: "UpdateWorkspace",
            request: || json!({"workspaceDescription": "written-by-the-case"}),
        },
        Mutation {
            operation: "UpdateWorkspaceConfiguration",
            request: || json!({"configuration": "{\"written\":\"by-the-case\"}"}),
        },
    ]
}

/// Drive one mutation against `id`.
async fn mutate(router: &MultiServiceRouter, mutation: &Mutation, id: &str) -> Answer {
    let request = (mutation.request)();
    match mutation.operation {
        "UpdateWorkspace" => update(router, id, request).await,
        "UpdateWorkspaceConfiguration" => update_configuration(router, id, request).await,
        other => panic!("this suite drives no operation named {other:?}"),
    }
}

/// Everything a mutation could have written, read back through the operations that report
/// it: the workspace record and its configuration document.
async fn stored(router: &MultiServiceRouter, id: &str) -> (Value, String) {
    let described = describe(router, id).await;
    let configuration = describe_configuration(router, id).await;
    (
        described.workspace().clone(),
        configuration.body["configuration"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// How a case puts a workspace into the in-flight status it is about.
#[derive(Debug, Clone, Copy)]
enum StartedBy {
    /// The create itself, which leaves the workspace `CREATING`.
    Create,
    /// An accepted update, which leaves it `UPDATING`.
    Update,
    /// An accepted delete, which leaves it `DELETING`.
    Delete,
}

/// THE RULE: a workspace whose previous accepted change is still in flight refuses another
/// one with HTTP 409 `ConflictException` naming the status it is in, and stores nothing.
#[tokio::test]
async fn a_change_to_a_workspace_with_one_in_flight_is_a_conflict() {
    // (case, how the workspace got there, the status it reports)
    let in_flight: [(&str, StartedBy, &str); 3] = [
        ("a workspace whose create is in flight", StartedBy::Create, "CREATING"),
        ("a workspace whose update is in flight", StartedBy::Update, "UPDATING"),
        ("a workspace whose delete is in flight", StartedBy::Delete, "DELETING"),
    ];

    for (case, started_by, status) in in_flight {
        for mutation in mutations() {
            let case = format!("{} refused for {case:?}", mutation.operation);
            // A fresh store per row: no row can pass on another's write.
            let router = emulator();
            let id = match started_by {
                StartedBy::Create => create(&router).await.id(),
                StartedBy::Update => {
                    let id = active_workspace(&router).await;
                    let accepted = update(&router, &id, json!({"workspaceName": "first"})).await;
                    assert_eq!(
                        accepted.status, 202,
                        "case {case:?}: the first update must be accepted, got {} with {:?}",
                        accepted.status, accepted.body
                    );
                    id
                }
                StartedBy::Delete => {
                    let id = active_workspace(&router).await;
                    let accepted = delete(&router, &id).await;
                    assert_eq!(
                        accepted.status, 202,
                        "case {case:?}: the delete must be accepted, got {} with {:?}",
                        accepted.status, accepted.body
                    );
                    id
                }
            };
            assert_eq!(
                describe(&router, &id).await.status_member(),
                status,
                "case {case:?}: the workspace must report {status} before the refusal"
            );
            let before = stored(&router, &id).await;

            let refused = mutate(&router, &mutation, &id).await;
            assert_eq!(
                refused.status, 409,
                "case {case:?}: expected HTTP 409, got {} with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                CONFLICT_CODE,
                "case {case:?}: expected {CONFLICT_CODE}, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.message(),
                format!("Workspace operation failed. Workspace is currently in the {status} state."),
                "case {case:?}: expected the conflict to name the status, got {:?}",
                refused.message()
            );
            assert_eq!(
                (
                    refused.body["resourceId"].as_str().unwrap_or_default(),
                    refused.body["resourceType"].as_str().unwrap_or_default(),
                ),
                (id.as_str(), WORKSPACE_TYPE),
                "case {case:?}: the conflict must name the workspace in the way, got {:?}",
                refused.body
            );

            let after = stored(&router, &id).await;
            assert_eq!(
                before, after,
                "case {case:?}: a refused change must leave the stored record untouched"
            );
        }
    }
}

/// THE SAME RULE from the other side: a workspace the clock has settled accepts the very
/// call it refused while the change was in flight, and the change is then readable.
#[tokio::test]
async fn the_same_change_is_accepted_once_the_workspace_has_settled() {
    for mutation in mutations() {
        let router = emulator();
        let id = active_workspace(&router).await;

        // One accepted change, so the next one meets the conflict.
        let first = mutate(&router, &mutation, &id).await;
        assert_eq!(
            first.status, 202,
            "case {:?}: a change to a settled workspace must be accepted, got {} with {:?}",
            mutation.operation, first.status, first.body
        );
        let refused = mutate(&router, &mutation, &id).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (409, CONFLICT_CODE),
            "case {:?}: the change issued while the first is in flight must be refused, \
             got {} with {:?}",
            mutation.operation,
            refused.status,
            refused.body
        );

        settled().await;
        assert_eq!(
            describe(&router, &id).await.status_member(),
            "ACTIVE",
            "case {:?}: the clock must settle the workspace back to ACTIVE",
            mutation.operation
        );
        let again = mutate(&router, &mutation, &id).await;
        assert_eq!(
            again.status, 202,
            "case {:?}: the same call must be accepted once the workspace has settled, \
             got {} with {:?}",
            mutation.operation,
            again.status,
            again.body
        );
    }
}

/// THE RULE: `DeleteWorkspace` is `@idempotent`, so a delete repeated while the deletion is
/// still in flight is accepted again — the same workspace, still `DELETING` — however many
/// times it is sent, and the workspace is removed only once, when the clock makes the
/// removal due.
#[tokio::test]
async fn a_repeated_delete_of_a_deletion_in_flight_is_accepted() {
    // The counts span "once more" and "several times more": at-least-once callers retry.
    for repeats in [1, 2, 5] {
        let router = emulator();
        let id = active_workspace(&router).await;
        let other = active_workspace(&router).await;

        let first = delete(&router, &id).await;
        assert_eq!(
            (first.status, first.id(), first.status_member().to_string()),
            (202, id.clone(), "DELETING".to_string()),
            "case {repeats}: the first delete must be accepted with the workspace DELETING, \
             got {} with {:?}",
            first.status,
            first.body
        );

        for attempt in 1..=repeats {
            let again = delete(&router, &id).await;
            assert_eq!(
                (again.status, again.id(), again.status_member().to_string()),
                (202, id.clone(), "DELETING".to_string()),
                "case {repeats}: delete #{attempt} while the deletion is in flight must be \
                 accepted with the same workspace, got {} with {:?}",
                again.status,
                again.body
            );
            let polled = describe(&router, &id).await;
            assert_eq!(
                (polled.status, polled.status_member()),
                (200, "DELETING"),
                "case {repeats}: a poller must read DELETING after delete #{attempt}, got {} \
                 with {:?}",
                polled.status,
                polled.body
            );
        }
        assert!(
            list(&router).await.contains(&id),
            "case {repeats}: a workspace being deleted is still listed"
        );

        // The removal is owed by the CLOCK, not by the number of deletes: once it falls
        // due the id names nothing, and nothing but that workspace went with it.
        settled().await;
        let gone = describe(&router, &id).await;
        assert_eq!(
            (gone.status, gone.error_code()),
            (404, "ResourceNotFoundException"),
            "case {repeats}: the removed workspace must answer 404, got {} with {:?}",
            gone.status,
            gone.body
        );
        assert_eq!(
            list(&router).await,
            vec![other],
            "case {repeats}: only the deleted workspace may be removed"
        );
    }
}

/// THE RULE from the reading side: while a deletion is in flight the record is still
/// there, so every operation that READS the workspace answers about it; once the removal
/// falls due every one of them answers `ResourceNotFoundException`.
#[tokio::test]
async fn every_workspace_read_follows_the_deleting_then_removed_lifecycle() {
    // (case, method, the URI under the workspace)
    let reads: [(&str, &str, &str); 4] = [
        ("DescribeWorkspace", "GET", ""),
        ("DescribeWorkspaceConfiguration", "GET", "/configuration"),
        ("DescribeWorkspaceAuthentication", "GET", "/authentication"),
        ("ListWorkspaceServiceAccounts", "GET", "/serviceaccounts"),
    ];

    for (case, method, suffix) in reads {
        // A fresh store per row, and ONE read in it: each row has to decide existence
        // from the clock itself rather than inherit a release another call performed.
        let router = emulator();
        let id = active_workspace(&router).await;
        assert_eq!(
            delete(&router, &id).await.status,
            202,
            "case {case:?}: the delete this row needs must be accepted"
        );

        let reading = call(&router, method, &format!("/workspaces/{id}{suffix}"), None).await;
        assert_eq!(
            reading.status, 200,
            "case {case:?}: a workspace being deleted is still readable, got {} with {:?}",
            reading.status, reading.body
        );

        settled().await;
        let gone = call(&router, method, &format!("/workspaces/{id}{suffix}"), None).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: a removed workspace answers 404, got {} with {:?}",
            gone.status, gone.body
        );
        assert_eq!(
            gone.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            gone.body
        );
    }
}

/// How a row of [`every_other_read_of_a_workspace_also_follows_the_removal`] addresses the
/// workspace: the account id and the workspace id, into the request URI.
type WorkspaceUri = fn(&str, &str) -> String;

/// The URI `ListTagsForResource` addresses one workspace at.
///
/// `resourceArn` is bound to a single NON-greedy `{resourceArn}` path segment, so its `:`
/// and `/` are percent-encoded in the path exactly as an AWS SDK sends them.
fn tags_uri(account: &str, id: &str) -> String {
    let arn = format!("arn:aws:grafana:eu-west-1:{account}:/workspaces/{id}");
    let encoded: String = arn
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect();
    format!("/tags/{encoded}")
}

/// The URI `ListVersions` names one workspace at: its own `workspace-id` query parameter.
fn versions_uri(_account: &str, id: &str) -> String {
    format!("/versions?workspace-id={id}")
}

/// THE SAME RULE for the reads that resolve a workspace by something other than the
/// `workspaceId` label — a resource ARN, and the version catalogue's query parameter:
/// each decides existence from the clock on its own.
#[tokio::test]
async fn every_other_read_of_a_workspace_also_follows_the_removal() {
    let account = ::engine::emulator_account_id();
    // (case, method, the URI naming the workspace some other way)
    let reads: [(&str, &str, WorkspaceUri); 2] = [
        ("ListTagsForResource", "GET", tags_uri),
        ("ListVersions", "GET", versions_uri),
    ];

    for (case, method, uri) in reads {
        let router = emulator();
        let id = active_workspace(&router).await;
        assert_eq!(
            delete(&router, &id).await.status,
            202,
            "case {case:?}: the delete this row needs must be accepted"
        );

        let reading = call(&router, method, &uri(account, &id), None).await;
        assert_eq!(
            reading.status, 200,
            "case {case:?}: a workspace being deleted is still readable, got {} with {:?}",
            reading.status, reading.body
        );

        settled().await;
        let gone = call(&router, method, &uri(account, &id), None).await;
        assert_eq!(
            (gone.status, gone.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a removed workspace answers 404 ResourceNotFoundException, \
             got {} with {:?}",
            gone.status,
            gone.body
        );
    }
}
