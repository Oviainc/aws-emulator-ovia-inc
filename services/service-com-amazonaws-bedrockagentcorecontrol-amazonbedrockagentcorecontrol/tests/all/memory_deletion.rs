// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteMemory`.
//!
//! The rule under test: a memory is addressed by the `memoryId` its create minted, so a
//! well-formed id this account holds no memory under is refused HTTP 404
//! `ResourceNotFoundException` — the one answer delete-if-exists cleanup keys on — and
//! nothing is mutated for it, so an unrelated memory is untouched and still deletable. An
//! id the account DOES hold is accepted (HTTP 202) and the memory reports `DELETING` until
//! the clock owes its removal, after which the id names nothing either.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request in this suite signs for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::emulator_account_id`), which the
/// refusal of a missing memory names.
const ACCOUNT: &str = "000000000000";

/// The settle span `src/state.rs` gives a memory's clock-owed transitions, plus slack, so
/// a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-agentcore-control.amazonaws.com")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        );
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `CreateMemory`: `POST /memories/create`.
async fn create_memory(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "POST",
        "/memories/create",
        Some(&json!({"name": name, "eventExpiryDuration": 30})),
    )
    .await;
    assert_eq!(
        status, 202,
        "the memory {name} is created, got {status} ({created})"
    );
    created["memory"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateMemory reported no id in {created}"))
        .to_string()
}

/// `DeleteMemory`: `DELETE /memories/{memoryId}/delete`.
async fn delete_memory(router: &MultiServiceRouter, memory_id: &str) -> (u16, Value) {
    send(
        router,
        "DELETE",
        &format!("/memories/{memory_id}/delete"),
        None,
    )
    .await
}

/// `GetMemory`: `GET /memories/{memoryId}/details`.
async fn get_memory(router: &MultiServiceRouter, memory_id: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/memories/{memory_id}/details"),
        None,
    )
    .await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A well-formed id no memory of this account was created under is refused, and the
/// refusal names the id the request carried and the account the lookup ran in.
///
/// The rows differ along the shape of the well-formed id — the modelled minimum length, a
/// name carrying the separators the pattern admits, one that only differs from a stored
/// memory's id by its suffix, and a long name — because the rule is about the id naming
/// no memory, not about its spelling. Every id satisfies `MemoryId`'s pattern
/// `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$` with length 12.., so each refusal is
/// the handler's and not the boundary's.
#[tokio::test]
async fn a_well_formed_id_no_memory_exists_under_is_refused() {
    let router = router();
    // A memory really held, so the refusals below are about the id and not about the
    // account being empty.
    let present = create_memory(&router, "Untouched").await;

    let stored_name = present
        .rsplit_once('-')
        .map(|(name, _)| name.to_string())
        .unwrap_or_default();
    let absent: [(&str, String); 4] = [
        ("an id nothing was created under", "Absent-0123456789".to_string()),
        (
            "the shortest id the pattern admits",
            "a-0123456789".to_string(),
        ),
        (
            "a stored memory's name with another suffix",
            format!("{stored_name}-zzzzzzzzzz"),
        ),
        (
            "a name carrying the separators the pattern admits",
            "a_long-memory_name-0123456789".to_string(),
        ),
    ];

    for (case, memory_id) in absent {
        let (status, refused) = delete_memory(&router, &memory_id).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} ({refused})"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            format!(
                "Resource not found during DeleteMemory: Failed to retrieve memory with ID: \
                 {memory_id} for account: {ACCOUNT}"
            ),
            "case {case:?}: unexpected refusal message, got {refused}"
        );
    }

    // Nothing was mutated for any of them: the memory this account does hold is still
    // there, and still deletable.
    let (status, read) = get_memory(&router, &present).await;
    assert_eq!(
        (status, read["memory"]["id"].clone()),
        (200, json!(present)),
        "the unrelated memory is untouched by the refused deletes, got {status} ({read})"
    );
    let (status, deleted) = delete_memory(&router, &present).await;
    assert_eq!(
        (status, deleted["memoryId"].clone()),
        (202, json!(present)),
        "the unrelated memory is still deletable, got {status} ({deleted})"
    );
}

/// An accepted delete reports `DELETING` and the clock owes the removal: the memory is
/// readable while the span runs and names nothing afterwards, and the repeat inside the
/// span reports the same memory rather than re-arming the clock.
#[tokio::test]
async fn an_accepted_delete_reports_deleting_until_the_clock_removes_the_memory() {
    let router = router();
    let memory_id = create_memory(&router, "Doomed").await;

    let (status, accepted) = delete_memory(&router, &memory_id).await;
    assert_eq!(
        (
            status,
            accepted["memoryId"].clone(),
            accepted["status"].clone()
        ),
        (202, json!(memory_id.clone()), json!("DELETING")),
        "the delete is accepted and the memory reports DELETING, got {status} ({accepted})"
    );

    // The record is still there while the span the clock owes runs, so a caller polling
    // it sees DELETING rather than a resource that vanished mid-call.
    let (status, read) = get_memory(&router, &memory_id).await;
    assert_eq!(
        (status, read["memory"]["status"].clone()),
        (200, json!("DELETING")),
        "the memory reports DELETING while the removal is owed, got {status} ({read})"
    );

    // `DeleteMemory` is `@idempotent`: the repeat answers with the same memory.
    let (status, repeated) = delete_memory(&router, &memory_id).await;
    assert_eq!(
        (status, repeated["status"].clone()),
        (202, json!("DELETING")),
        "the repeated delete reports the same memory, got {status} ({repeated})"
    );

    // Nothing sleeps for the removal in the service, so the suite waits the clock out.
    tokio::time::sleep(PAST_SETTLE).await;
    for (operation, (status, answer)) in [
        ("GetMemory", get_memory(&router, &memory_id).await),
        ("DeleteMemory", delete_memory(&router, &memory_id).await),
    ] {
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (404, "ResourceNotFoundException"),
            "{operation} after the removal is the modelled 404, got {status} ({answer})"
        );
    }

    // The name is free again, because the memory that held it is gone.
    let reused = create_memory(&router, "Doomed").await;
    assert_ne!(
        reused, memory_id,
        "the re-created memory gets an id of its own, got {reused}"
    );
}

/// A memory whose deletion has been accepted takes no update: the configuration is on its
/// way out, so the request conflicts with the operation already running on it.
#[tokio::test]
async fn a_deleting_memory_takes_no_update() {
    let router = router();
    let memory_id = create_memory(&router, "Closing").await;
    // The create's own CREATING span has to pass first, or the update would conflict with
    // that instead.
    tokio::time::sleep(PAST_SETTLE).await;

    let (status, accepted) = delete_memory(&router, &memory_id).await;
    assert_eq!(status, 202, "the delete is accepted, got {status} ({accepted})");

    let (status, refused) = send(
        &router,
        "PUT",
        &format!("/memories/{memory_id}/update"),
        Some(&json!({"description": "edited while deleting"})),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (409, "ConflictException"),
        "an update of a deleting memory conflicts, got {status} ({refused})"
    );
}
