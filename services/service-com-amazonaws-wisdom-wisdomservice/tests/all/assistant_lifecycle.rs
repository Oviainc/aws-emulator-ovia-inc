// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Wisdom assistant/session slice, driven over HTTP
//! through the same router the emulator serves with.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one
//! recorded request:
//!
//! * a create persists the resource it mints, so every later read of that
//!   resource — by id or by ARN — answers from the stored record;
//! * a `UuidOrArn` path label resolves to the same record in either form;
//! * `ListAssistants` enumerates the persisted assistants exactly once across
//!   its pages, for any page size;
//! * a resource the service does not hold is a `ResourceNotFoundException`,
//!   and an input outside a modelled constraint is a `ValidationException`;
//! * a delete actually removes the record (and the sessions under it);
//! * every caller-driven store is bounded: a create past a ceiling is refused
//!   with a modelled error (and the ceiling is on occupancy, so freeing a
//!   record makes room), while the `clientToken` index is bounded by
//!   oldest-first eviction.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for; the handlers read it
/// out of the SigV4 credential scope to build ARNs.
const REGION: &str = "us-west-2";

/// A fresh emulator: one engine, one mounted Wisdom service, no state.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The routing key this service answers under (`src/service.rs`'s
/// `OWN_REGISTRATION_KEY`): the shared `wisdom` endpoint identity this model and the
/// Amazon Q in Connect model BOTH publish is served by the QConnect crate — the
/// endpoint's current API, whose routes are a superset of this model's — so this legacy
/// model is addressed by its own registration key and contends for nothing.
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// One request, signed the way a client addressed at THIS registered model signs: the
/// credential scope is what routes it to this service and names its region.
fn request(method: &str, path: &str, body: Option<&Value>) -> http::Request<Bytes> {
    let builder = http::Request::builder()
        .method(method)
        .uri(format!("https://wisdom.{REGION}.amazonaws.com{path}"))
        .header("host", format!("wisdom.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{ROUTE_KEY}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified-in-this-suite"
            ),
        );
    match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Bytes::from(json.to_string()))
            .expect("build a request with a JSON body"),
        None => builder.body(Bytes::new()).expect("build a bodyless request"),
    }
}

/// The status, the parsed JSON body (`Value::Null` for an empty one) and the
/// raw body of one call.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value, Bytes) {
    let response = router.dispatch(request(method, path, body)).await;
    let status = response.status().as_u16();
    let bytes = response.body().clone();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed, bytes)
}

/// The wire error code restJson1 carries in the error envelope's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Whether `value` is a lowercase hyphenated v4-shaped uuid, the form the
/// model's `UuidOrArn` pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len && group.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
}

/// Percent-encode one path label the way an AWS SDK encodes a non-greedy
/// `@httpLabel`: an ARN label carries `:` and `/`, which must not be read as
/// path structure. A bare uuid label passes through unchanged.
fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn str_at(body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row of the create table: what varies between assistants.
#[derive(Debug, Clone)]
struct AssistantCase {
    case: &'static str,
    name: String,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    kms_key_id: Option<&'static str>,
}

impl AssistantCase {
    fn body(&self) -> Value {
        let mut body = json!({ "name": self.name, "type": "AGENT" });
        if let Some(description) = self.description {
            body["description"] = json!(description);
        }
        if !self.tags.is_empty() {
            body["tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), json!(v)))
                    .collect(),
            );
        }
        if let Some(kms_key_id) = self.kms_key_id {
            body["serverSideEncryptionConfiguration"] = json!({ "kmsKeyId": kms_key_id });
        }
        body
    }

    /// The `tags` member the service reports for this case: ALWAYS present, and an
    /// empty object when the row carries no tag (`state::wire_tags`).
    fn expected_tags(&self) -> Option<Value> {
        Some(Value::Object(
            self.tags
                .iter()
                .map(|(k, v)| ((*k).to_string(), json!(v)))
                .collect(),
        ))
    }
}

/// The create table: rows differ in description, tag count, encryption
/// configuration and name length — including both published `Name` length
/// bounds (`len 1..255`).
fn assistant_cases() -> Vec<AssistantCase> {
    vec![
        AssistantCase {
            case: "required members only",
            name: "minimal assistant".to_string(),
            description: None,
            tags: &[],
            kms_key_id: None,
        },
        AssistantCase {
            case: "description and one tag",
            name: "described assistant".to_string(),
            description: Some("bootstrap demo"),
            tags: &[("env", "test")],
            kms_key_id: None,
        },
        AssistantCase {
            case: "three tags",
            name: "tagged.assistant-1".to_string(),
            description: Some("Punctuation , . _ - is inside the Name pattern"),
            tags: &[("env", "prod"), ("team", "support"), ("tier", "1")],
            kms_key_id: None,
        },
        AssistantCase {
            case: "customer managed key",
            name: "encrypted assistant".to_string(),
            description: None,
            tags: &[("env", "test")],
            kms_key_id: Some("arn:aws:kms:us-west-2:000000000000:key/abcd1234"),
        },
        AssistantCase {
            case: "shortest permitted name",
            name: "a".to_string(),
            description: None,
            tags: &[],
            kms_key_id: None,
        },
        AssistantCase {
            case: "longest permitted name",
            name: "n".repeat(255),
            description: None,
            tags: &[],
            kms_key_id: None,
        },
    ]
}

/// Every row of the create table round-trips: what `CreateAssistant` answers is
/// what `GetAssistant` reads back, by id AND by ARN.
#[tokio::test]
async fn a_created_assistant_round_trips_through_get_by_id_and_by_arn() {
    let router = emulator();
    let account = engine::emulator_account_id();

    for case in assistant_cases() {
        let (status, created, _) =
            call(&router, "POST", "/assistants", Some(&case.body())).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from CreateAssistant, got {status} with {created}",
            case.case
        );

        let id = str_at(&created, "/assistant/assistantId");
        let arn = str_at(&created, "/assistant/assistantArn");
        assert!(
            is_uuid(&id),
            "case {:?}: expected a v4-shaped assistantId, got {id:?}",
            case.case
        );
        assert_eq!(
            arn,
            format!("arn:aws:wisdom:{REGION}:{account}:assistant/{id}"),
            "case {:?}: unexpected assistantArn, got {arn:?}",
            case.case
        );
        assert_eq!(
            str_at(&created, "/assistant/status"),
            "ACTIVE",
            "case {:?}: expected status ACTIVE, got {created}",
            case.case
        );
        assert_eq!(
            str_at(&created, "/assistant/type"),
            "AGENT",
            "case {:?}: expected type AGENT, got {created}",
            case.case
        );

        // Both label forms the model's `UuidOrArn` accepts must answer with the
        // identical stored record.
        let (by_id_status, by_id, _) =
            call(&router, "GET", &format!("/assistants/{id}"), None).await;
        let (by_arn_status, by_arn, _) =
            call(&router, "GET", &format!("/assistants/{}", label(&arn)), None).await;
        assert_eq!(
            (by_id_status, by_arn_status),
            (200, 200),
            "case {:?}: expected HTTP 200 from GetAssistant by id and by ARN, got {by_id_status} and {by_arn_status}",
            case.case
        );
        assert_eq!(
            by_id, by_arn,
            "case {:?}: the id-form and ARN-form labels must resolve to one record, got {by_id} and {by_arn}",
            case.case
        );
        assert_eq!(
            by_id.pointer("/assistant"),
            created.pointer("/assistant"),
            "case {:?}: GetAssistant must report what CreateAssistant stored, got {by_id}",
            case.case
        );
        assert_eq!(
            str_at(&by_id, "/assistant/name"),
            case.name,
            "case {:?}: unexpected name, got {by_id}",
            case.case
        );
        assert_eq!(
            by_id
                .pointer("/assistant/description")
                .and_then(Value::as_str),
            case.description,
            "case {:?}: unexpected description, got {by_id}",
            case.case
        );
        assert_eq!(
            by_id.pointer("/assistant/tags").cloned(),
            case.expected_tags(),
            "case {:?}: unexpected tags, got {by_id}",
            case.case
        );
        assert_eq!(
            by_id
                .pointer("/assistant/serverSideEncryptionConfiguration/kmsKeyId")
                .and_then(Value::as_str),
            case.kms_key_id,
            "case {:?}: unexpected serverSideEncryptionConfiguration, got {by_id}",
            case.case
        );
    }
}

/// An input outside a modelled constraint is refused as a `ValidationException`
/// before anything is stored: both `Name` bounds hold, one step past the upper
/// bound and one off-pattern value do not.
#[tokio::test]
async fn an_input_outside_a_modelled_constraint_is_refused() {
    let router = emulator();

    // (case, request body, expected status)
    let rows: Vec<(&str, Value, u16)> = vec![
        (
            "name at the 1-character lower bound",
            json!({ "name": "x", "type": "AGENT" }),
            200,
        ),
        (
            "name at the 255-character upper bound",
            json!({ "name": "y".repeat(255), "type": "AGENT" }),
            200,
        ),
        (
            "name one character past the upper bound",
            json!({ "name": "z".repeat(256), "type": "AGENT" }),
            400,
        ),
        (
            // `@pattern` is a find, not an anchored match, so a name is refused
            // only when NO substring of it satisfies the pattern.
            "name outside the Name pattern",
            json!({ "name": "!!!", "type": "AGENT" }),
            400,
        ),
        (
            "name whose tail is outside the unanchored Name pattern",
            json!({ "name": "no!punctuation?here", "type": "AGENT" }),
            200,
        ),
        (
            "type outside the AssistantType enum",
            json!({ "name": "wrong type", "type": "SUPERVISOR" }),
            400,
        ),
        ("name absent", json!({ "type": "AGENT" }), 400),
    ];

    for (case, body, expected) in rows {
        let (status, parsed, _) = call(&router, "POST", "/assistants", Some(&body)).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} from CreateAssistant, got {status} with {parsed}"
        );
    }

    // A malformed path label never reaches the store either.
    let (status, parsed, _) = call(&router, "GET", "/assistants/not-a-uuid", None).await;
    assert_eq!(
        status, 400,
        "case \"malformed assistantId label\": expected HTTP 400, got {status} with {parsed}"
    );
}

/// A name is unique among the live assistants: the second claim conflicts, and
/// the name is free again once the first is deleted.
#[tokio::test]
async fn a_duplicate_assistant_name_conflicts_until_the_holder_is_deleted() {
    let router = emulator();
    let body = json!({ "name": "only one of these", "type": "AGENT" });

    let (status, created, _) = call(&router, "POST", "/assistants", Some(&body)).await;
    assert_eq!(status, 200, "case \"first create\": got {status} with {created}");
    let id = str_at(&created, "/assistant/assistantId");

    let (status, conflict, _) = call(&router, "POST", "/assistants", Some(&body)).await;
    assert_eq!(
        status, 409,
        "case \"duplicate name\": expected HTTP 409, got {status} with {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "case \"duplicate name\": unexpected error code in {conflict}"
    );

    let (status, _, _) = call(&router, "DELETE", &format!("/assistants/{id}"), None).await;
    assert_eq!(status, 204, "case \"delete the holder\": got {status}");

    let (status, recreated, _) = call(&router, "POST", "/assistants", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case \"recreate after delete\": expected HTTP 200, got {status} with {recreated}"
    );
    assert_ne!(
        str_at(&recreated, "/assistant/assistantId"),
        id,
        "case \"recreate after delete\": the replacement must be a new resource, got {recreated}"
    );
}

/// `clientToken` makes a create a retry rather than a second resource — for any
/// number of retries, and for each create operation that takes a token.
#[tokio::test]
async fn a_repeated_client_token_replays_the_first_create() {
    let router = emulator();

    for retries in [1usize, 2, 5] {
        let token = format!("token-for-{retries}-retries");
        let body = json!({
            "name": format!("replayed assistant {retries}"),
            "type": "AGENT",
            "clientToken": token,
        });
        let (status, first, _) = call(&router, "POST", "/assistants", Some(&body)).await;
        assert_eq!(
            status, 200,
            "case {retries} retries: expected HTTP 200, got {status} with {first}"
        );

        for retry in 0..retries {
            let (status, replay, _) = call(&router, "POST", "/assistants", Some(&body)).await;
            assert_eq!(
                status, 200,
                "case {retries} retries (retry {retry}): expected HTTP 200, got {status} with {replay}"
            );
            assert_eq!(
                replay.pointer("/assistant"),
                first.pointer("/assistant"),
                "case {retries} retries (retry {retry}): a replayed clientToken must answer with the first resource, got {replay}"
            );
        }

        // The same for a session under that assistant.
        let assistant_id = str_at(&first, "/assistant/assistantId");
        let session_body = json!({ "name": "replayed session", "clientToken": token });
        let path = format!("/assistants/{assistant_id}/sessions");
        let (status, first_session, _) = call(&router, "POST", &path, Some(&session_body)).await;
        assert_eq!(
            status, 200,
            "case {retries} retries: expected HTTP 200 from CreateSession, got {status} with {first_session}"
        );
        let (status, replayed_session, _) = call(&router, "POST", &path, Some(&session_body)).await;
        assert_eq!(
            status, 200,
            "case {retries} retries: expected HTTP 200 from the CreateSession retry, got {status}"
        );
        assert_eq!(
            replayed_session.pointer("/session"),
            first_session.pointer("/session"),
            "case {retries} retries: a replayed clientToken must answer with the first session, got {replayed_session}"
        );
    }
}

/// A `clientToken` only replays the create it was used for: the same token
/// scoped to a different parent is a different create, and a token whose
/// resource was deleted replays as if it had never been used.
#[tokio::test]
async fn a_client_token_replays_only_within_the_create_it_was_used_for() {
    let router = emulator();
    let token = "one-token-two-parents";

    // Two assistants, so the same session token is used under each.
    let mut assistant_ids = Vec::new();
    for index in 0..2 {
        let body = json!({ "name": format!("token host {index}"), "type": "AGENT" });
        let (status, created, _) = call(&router, "POST", "/assistants", Some(&body)).await;
        assert_eq!(status, 200, "creating assistant {index}: got {status} with {created}");
        assistant_ids.push(str_at(&created, "/assistant/assistantId"));
    }

    let session_body = json!({ "name": "scoped session", "clientToken": token });
    let mut sessions = Vec::new();
    for assistant_id in &assistant_ids {
        let path = format!("/assistants/{assistant_id}/sessions");
        let (status, created, _) = call(&router, "POST", &path, Some(&session_body)).await;
        assert_eq!(
            status, 200,
            "case \"session under {assistant_id}\": expected HTTP 200, got {status} with {created}"
        );
        sessions.push(str_at(&created, "/session/sessionId"));
    }
    assert_ne!(
        sessions[0], sessions[1],
        "case \"one token, two parents\": each assistant must get its own session, got {sessions:?}"
    );

    // And each parent still replays ITS session, so the second create did not
    // overwrite the first token entry.
    for (assistant_id, session_id) in assistant_ids.iter().zip(&sessions) {
        let path = format!("/assistants/{assistant_id}/sessions");
        let (status, replayed, _) = call(&router, "POST", &path, Some(&session_body)).await;
        assert_eq!(
            status, 200,
            "case \"replay under {assistant_id}\": expected HTTP 200, got {status} with {replayed}"
        );
        assert_eq!(
            &str_at(&replayed, "/session/sessionId"),
            session_id,
            "case \"replay under {assistant_id}\": expected the session this parent created, got {replayed}"
        );
    }

    // A token whose resource was deleted creates afresh rather than replaying a
    // record that is gone.
    let assistant_body =
        json!({ "name": "deleted token holder", "type": "AGENT", "clientToken": token });
    let (_, first, _) = call(&router, "POST", "/assistants", Some(&assistant_body)).await;
    let first_id = str_at(&first, "/assistant/assistantId");
    let (status, _, _) = call(&router, "DELETE", &format!("/assistants/{first_id}"), None).await;
    assert_eq!(status, 204, "case \"delete the token holder\": got {status}");
    let (status, recreated, _) = call(&router, "POST", "/assistants", Some(&assistant_body)).await;
    assert_eq!(
        status, 200,
        "case \"token reused after delete\": expected HTTP 200, got {status} with {recreated}"
    );
    assert_ne!(
        str_at(&recreated, "/assistant/assistantId"),
        first_id,
        "case \"token reused after delete\": the deleted resource must not be replayed, got {recreated}"
    );
}

/// `ListAssistants` enumerates every persisted assistant exactly once, in
/// creation order, for any page size — and only names a `nextToken` while
/// assistants remain.
#[tokio::test]
async fn listing_pages_over_every_persisted_assistant_for_any_page_size() {
    let router = emulator();

    let mut created_in_order = Vec::new();
    for index in 0..5 {
        let body = json!({ "name": format!("listed assistant {index}"), "type": "AGENT" });
        let (status, created, _) = call(&router, "POST", "/assistants", Some(&body)).await;
        assert_eq!(status, 200, "creating assistant {index}: got {status} with {created}");
        created_in_order.push(str_at(&created, "/assistant/assistantId"));
    }

    for page_size in [1usize, 2, 3, 5, 100] {
        let mut seen = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut path = format!("/assistants?maxResults={page_size}");
            if let Some(token) = &next_token {
                path.push_str(&format!("&nextToken={token}"));
            }
            let (status, page, _) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected HTTP 200, got {status} with {page}"
            );
            let summaries = page
                .pointer("/assistantSummaries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                !summaries.is_empty() && summaries.len() <= page_size,
                "case page size {page_size}: a page must hold 1..={page_size} summaries, got {}",
                summaries.len()
            );
            for summary in &summaries {
                assert!(
                    is_uuid(summary["assistantId"].as_str().unwrap_or_default()),
                    "case page size {page_size}: expected a v4-shaped assistantId, got {summary}"
                );
                assert_eq!(
                    summary["assistantArn"].as_str().unwrap_or_default(),
                    format!(
                        "arn:aws:wisdom:{REGION}:{}:assistant/{}",
                        engine::emulator_account_id(),
                        summary["assistantId"].as_str().unwrap_or_default()
                    ),
                    "case page size {page_size}: unexpected assistantArn in {summary}"
                );
                assert_eq!(
                    summary["status"].as_str().unwrap_or_default(),
                    "ACTIVE",
                    "case page size {page_size}: unexpected status in {summary}"
                );
                seen.push(summary["assistantId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            next_token = page
                .pointer("/nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= created_in_order.len(),
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            seen, created_in_order,
            "case page size {page_size}: every assistant must be listed exactly once, in creation order, got {seen:?}"
        );
        let expected_pages = created_in_order.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case page size {page_size}: expected {expected_pages} pages, got {pages}"
        );
    }

    // A token that is not a token of this listing at all is a refusal, not an
    // empty page.
    let (status, refused, _) = call(
        &router,
        "GET",
        "/assistants?nextToken=00000000-0000-0000-0000-000000000000",
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"unknown nextToken\": expected HTTP 400, got {status} with {refused}"
    );
}

/// A page token issued before a delete still continues the listing: it names
/// the cursor the next page opens at, so the continuation resumes at the first
/// surviving assistant at or after it instead of refusing.
#[tokio::test]
async fn a_page_token_survives_deletes_between_pages() {
    let router = emulator();

    let mut ids = Vec::new();
    for index in 0..4 {
        let body = json!({ "name": format!("paged assistant {index}"), "type": "AGENT" });
        let (status, created, _) = call(&router, "POST", "/assistants", Some(&body)).await;
        assert_eq!(status, 200, "creating assistant {index}: got {status} with {created}");
        ids.push(str_at(&created, "/assistant/assistantId"));
    }

    // First page of one: the token opens the page that would start at ids[1].
    let (status, page, _) = call(&router, "GET", "/assistants?maxResults=1", None).await;
    assert_eq!(status, 200, "case \"first page\": got {status} with {page}");
    let token = page
        .pointer("/nextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "case \"first page\": expected a nextToken while assistants remain, got {page}"
    );

    // Delete exactly the assistant that token would have named.
    let (status, _, _) = call(&router, "DELETE", &format!("/assistants/{}", ids[1]), None).await;
    assert_eq!(status, 204, "case \"delete the token's assistant\": got {status}");

    let (status, resumed, _) = call(
        &router,
        "GET",
        &format!("/assistants?maxResults=10&nextToken={token}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"resume after a delete\": expected HTTP 200, got {status} with {resumed}"
    );
    let resumed_ids: Vec<String> = resumed
        .pointer("/assistantSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["assistantId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        resumed_ids,
        vec![ids[2].clone(), ids[3].clone()],
        "case \"resume after a delete\": the continuation must skip the deleted entry and list the rest, got {resumed_ids:?}"
    );

    // And when everything after the token is gone, the continuation is an empty
    // last page rather than a refusal.
    for id in [&ids[2], &ids[3]] {
        let (status, _, _) = call(&router, "DELETE", &format!("/assistants/{id}"), None).await;
        assert_eq!(status, 204, "case \"delete the tail\": got {status} for {id}");
    }
    let (status, empty, _) = call(
        &router,
        "GET",
        &format!("/assistants?nextToken={token}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"resume past the end\": expected HTTP 200, got {status} with {empty}"
    );
    assert_eq!(
        empty.pointer("/assistantSummaries").and_then(Value::as_array),
        Some(&vec![]),
        "case \"resume past the end\": expected an empty page, got {empty}"
    );
    assert!(
        empty.pointer("/nextToken").is_none(),
        "case \"resume past the end\": an empty last page carries no nextToken, got {empty}"
    );
}

/// A `nextToken` the listing never issued is refused as a `ValidationException`
/// rather than answered with an empty page a client would read as the end —
/// whatever form the bogus token takes.
#[tokio::test]
async fn a_next_token_this_listing_never_issued_is_refused() {
    let router = emulator();
    for index in 0..3 {
        let body = json!({ "name": format!("token-checked assistant {index}"), "type": "AGENT" });
        let (status, created, _) = call(&router, "POST", "/assistants", Some(&body)).await;
        assert_eq!(status, 200, "creating assistant {index}: got {status} with {created}");
    }

    // (case, token, expected status): a cursor this listing issued continues it;
    // anything it did not issue — a cursor past the newest one, or a token that
    // is not a cursor at all — is refused.
    let rows: Vec<(&str, String, u16)> = vec![
        ("the opening cursor", "0".to_string(), 200),
        ("a cursor inside the listing", "2".to_string(), 200),
        ("the cursor the next create would take", "3".to_string(), 400),
        ("one cursor past that", "4".to_string(), 400),
        ("a far-future cursor", "999999999".to_string(), 400),
        ("a non-numeric token", "not-a-cursor".to_string(), 400),
        ("an uuid-shaped token", "a".repeat(32), 400),
    ];

    for (case, token, expected) in rows {
        let (status, parsed, _) =
            call(&router, "GET", &format!("/assistants?nextToken={token}"), None).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for nextToken {token:?}, got {status} with {parsed}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case:?}: unexpected error code in {parsed}"
            );
        }
    }
}

/// One row of the session table: what varies between sessions, including which
/// of the two `UuidOrArn` forms names the parent assistant.
#[derive(Debug, Clone)]
struct SessionCase<'a> {
    case: &'static str,
    parent: &'a str,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// A session round-trips under its parent assistant, by id and by ARN, and
/// carries an ARN naming both ids.
#[tokio::test]
async fn a_created_session_round_trips_under_its_assistant() {
    let router = emulator();
    let account = engine::emulator_account_id();

    let (_, assistant, _) = call(
        &router,
        "POST",
        "/assistants",
        Some(&json!({ "name": "session host", "type": "AGENT" })),
    )
    .await;
    let assistant_id = str_at(&assistant, "/assistant/assistantId");
    let assistant_arn = str_at(&assistant, "/assistant/assistantArn");

    // Rows differ in description and tag count; the parent label alternates
    // between the two forms `UuidOrArn` accepts.
    let rows = vec![
        SessionCase {
            case: "minimal, parent by id",
            parent: &assistant_id,
            description: None,
            tags: &[],
        },
        SessionCase {
            case: "described, parent by ARN",
            parent: &assistant_arn,
            description: Some("a demo session"),
            tags: &[("env", "test")],
        },
        SessionCase {
            case: "tagged, parent by id",
            parent: &assistant_id,
            description: Some("another session"),
            tags: &[("env", "prod"), ("team", "support")],
        },
    ];

    let mut session_ids = BTreeSet::new();
    for SessionCase {
        case,
        parent,
        description,
        tags,
    } in rows
    {
        let mut body = json!({ "name": format!("session for {case}") });
        if let Some(description) = description {
            body["description"] = json!(description);
        }
        if !tags.is_empty() {
            body["tags"] = Value::Object(
                tags.iter()
                    .map(|(k, v)| ((*k).to_string(), json!(v)))
                    .collect(),
            );
        }

        let (status, created, _) = call(
            &router,
            "POST",
            &format!("/assistants/{}/sessions", label(parent)),
            Some(&body),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateSession, got {status} with {created}"
        );

        let session_id = str_at(&created, "/session/sessionId");
        let session_arn = str_at(&created, "/session/sessionArn");
        assert!(
            is_uuid(&session_id),
            "case {case:?}: expected a v4-shaped sessionId, got {session_id:?}"
        );
        assert_eq!(
            session_arn,
            format!("arn:aws:wisdom:{REGION}:{account}:session/{assistant_id}/{session_id}"),
            "case {case:?}: unexpected sessionArn, got {session_arn:?}"
        );
        assert!(
            session_ids.insert(session_id.clone()),
            "case {case:?}: every session must get its own id, saw {session_id} twice"
        );

        for (label_form, session_label) in [("id", &session_id), ("ARN", &session_arn)] {
            let (status, read, _) = call(
                &router,
                "GET",
                &format!(
                    "/assistants/{}/sessions/{}",
                    label(&assistant_id),
                    label(session_label)
                ),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} by {label_form}: expected HTTP 200 from GetSession, got {status} with {read}"
            );
            assert_eq!(
                read.pointer("/session"),
                created.pointer("/session"),
                "case {case:?} by {label_form}: GetSession must report what CreateSession stored, got {read}"
            );
        }
    }
}

/// A resource the service does not hold is a `ResourceNotFoundException`, for
/// every label that can name one.
#[tokio::test]
async fn an_unknown_resource_is_not_found() {
    let router = emulator();
    let account = engine::emulator_account_id();
    let absent = "11111111-2222-3333-4444-555555555555";

    let (_, assistant, _) = call(
        &router,
        "POST",
        "/assistants",
        Some(&json!({ "name": "the only assistant", "type": "AGENT" })),
    )
    .await;
    let assistant_id = str_at(&assistant, "/assistant/assistantId");
    let (_, session, _) = call(
        &router,
        "POST",
        &format!("/assistants/{assistant_id}/sessions"),
        Some(&json!({ "name": "the only session" })),
    )
    .await;
    let session_id = str_at(&session, "/session/sessionId");

    // (case, method, path)
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "GetAssistant on an absent id",
            "GET",
            format!("/assistants/{absent}"),
        ),
        (
            "GetAssistant on an absent ARN",
            "GET",
            format!(
                "/assistants/{}",
                label(&format!(
                    "arn:aws:wisdom:{REGION}:{account}:assistant/{absent}"
                ))
            ),
        ),
        (
            "DeleteAssistant on an absent id",
            "DELETE",
            format!("/assistants/{absent}"),
        ),
        (
            "CreateSession under an absent assistant",
            "POST",
            format!("/assistants/{absent}/sessions"),
        ),
        (
            "GetSession on an absent session",
            "GET",
            format!("/assistants/{assistant_id}/sessions/{absent}"),
        ),
        (
            "GetSession under an absent assistant",
            "GET",
            format!("/assistants/{absent}/sessions/{session_id}"),
        ),
    ];

    for (case, method, path) in rows {
        let body = json!({ "name": "unreachable" });
        let request_body = (method == "POST").then_some(&body);
        let (status, parsed, _) = call(&router, method, &path, request_body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {parsed}"
        );
    }
}

/// `DeleteAssistant` answers 204 with an empty body and really removes the
/// record: the assistant, its sessions and its listing entry all go.
#[tokio::test]
async fn deleting_an_assistant_removes_it_and_its_sessions_from_state() {
    let router = emulator();

    // Two assistants, so the listing after the delete is a positive check too.
    let mut ids = Vec::new();
    for index in 0..2 {
        let body = json!({ "name": format!("deletable assistant {index}"), "type": "AGENT" });
        let (_, created, _) = call(&router, "POST", "/assistants", Some(&body)).await;
        ids.push(str_at(&created, "/assistant/assistantId"));
    }
    let (doomed, survivor) = (ids[0].clone(), ids[1].clone());

    let (_, session, _) = call(
        &router,
        "POST",
        &format!("/assistants/{doomed}/sessions"),
        Some(&json!({ "name": "doomed session" })),
    )
    .await;
    let session_id = str_at(&session, "/session/sessionId");

    let (status, _, body) = call(&router, "DELETE", &format!("/assistants/{doomed}"), None).await;
    assert_eq!(
        status, 204,
        "case \"first delete\": expected HTTP 204, got {status}"
    );
    assert!(
        // DeleteAssistantResponse models no members, so the body carries none.
        // The framework's restJson1 serializer always writes a JSON object, so
        // an empty one is the no-member body it can render.
        body.is_empty() || body.as_ref() == b"{}",
        "case \"first delete\": expected a body with no members, got {body:?}"
    );

    for (case, path) in [
        ("GetAssistant after delete", format!("/assistants/{doomed}")),
        (
            "GetSession after delete",
            format!("/assistants/{doomed}/sessions/{session_id}"),
        ),
    ] {
        let (status, parsed, _) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {parsed}"
        );
    }

    let (status, listed, _) = call(&router, "GET", "/assistants", None).await;
    assert_eq!(status, 200, "case \"list after delete\": got {status} with {listed}");
    let remaining: Vec<String> = listed
        .pointer("/assistantSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["assistantId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        remaining,
        vec![survivor],
        "case \"list after delete\": the deleted assistant must be gone, got {remaining:?}"
    );

    // The model marks DeleteAssistant @idempotent, but an id the service no
    // longer knows is still a 404 — the first delete mutated state.
    let (status, parsed, _) = call(&router, "DELETE", &format!("/assistants/{doomed}"), None).await;
    assert_eq!(
        status, 404,
        "case \"second delete\": expected HTTP 404, got {status} with {parsed}"
    );
}

/// A read never changes what a later caller sees: `GetAssistant`,
/// `ListAssistants` and `GetSession` leave the store exactly as they found it.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = emulator();

    let (_, assistant, _) = call(
        &router,
        "POST",
        "/assistants",
        Some(&json!({ "name": "read me twice", "type": "AGENT", "tags": { "env": "test" } })),
    )
    .await;
    let assistant_id = str_at(&assistant, "/assistant/assistantId");
    let (_, session, _) = call(
        &router,
        "POST",
        &format!("/assistants/{assistant_id}/sessions"),
        Some(&json!({ "name": "read me twice too" })),
    )
    .await;
    let session_id = str_at(&session, "/session/sessionId");

    let reads: Vec<(&str, String)> = vec![
        ("GetAssistant", format!("/assistants/{assistant_id}")),
        ("ListAssistants", "/assistants".to_string()),
        (
            "GetSession",
            format!("/assistants/{assistant_id}/sessions/{session_id}"),
        ),
    ];

    for (case, path) in reads {
        let (first_status, first, _) = call(&router, "GET", &path, None).await;
        let (second_status, second, _) = call(&router, "GET", &path, None).await;
        assert_eq!(
            (first_status, second_status),
            (200, 200),
            "case {case:?}: both reads must answer HTTP 200, got {first_status} and {second_status}"
        );
        assert_eq!(
            first, second,
            "case {case:?}: a read must not change what the next caller sees, got {first} then {second}"
        );
    }
}

/// Every caller-driven store is bounded: a create that would take the store
/// past a ceiling is refused with a modelled error instead of growing the heap,
/// and the ceiling is on OCCUPANCY — freeing a record makes room again.
#[tokio::test]
async fn a_create_past_a_store_ceiling_is_refused_instead_of_growing_the_store() {
    let router = emulator();

    // Fill the assistant store exactly to its ceiling.
    let mut ids = Vec::new();
    for index in 0..state::MAX_ASSISTANTS {
        let (status, parsed, _) = call(
            &router,
            "POST",
            "/assistants",
            Some(&json!({ "name": format!("bounded assistant {index}"), "type": "AGENT" })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"assistant {index} of {}\": expected HTTP 200, got {status} with {parsed}",
            state::MAX_ASSISTANTS
        );
        ids.push(str_at(&parsed, "/assistant/assistantId"));
    }

    // One past it is the error the model declares for a create that cannot be
    // served because the service holds as many of the resource as it keeps.
    let (status, refused, _) = call(
        &router,
        "POST",
        "/assistants",
        Some(&json!({ "name": "one assistant too many", "type": "AGENT" })),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"one past MAX_ASSISTANTS\": expected HTTP 402, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case \"one past MAX_ASSISTANTS\": unexpected error code in {refused}"
    );

    // The listing still enumerates exactly the stored assistants: the refusal
    // stored nothing.
    let mut listed = 0usize;
    let mut next: Option<String> = None;
    loop {
        let path = match &next {
            Some(token) => format!("/assistants?maxResults=100&nextToken={token}"),
            None => "/assistants?maxResults=100".to_string(),
        };
        let (status, page, _) = call(&router, "GET", &path, None).await;
        assert_eq!(status, 200, "case \"listing the full store\": got {status} with {page}");
        listed += page
            .pointer("/assistantSummaries")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        next = page
            .pointer("/nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next.is_none() {
            break;
        }
    }
    assert_eq!(
        listed,
        state::MAX_ASSISTANTS,
        "case \"listing the full store\": expected {} assistants, got {listed}",
        state::MAX_ASSISTANTS
    );

    // The ceiling is occupancy, not a lifetime counter: deleting one frees a slot.
    let (status, _, _) = call(&router, "DELETE", &format!("/assistants/{}", ids[0]), None).await;
    assert_eq!(status, 204, "case \"free one slot\": expected HTTP 204, got {status}");
    let (status, parsed, _) = call(
        &router,
        "POST",
        "/assistants",
        Some(&json!({ "name": "reusing the freed slot", "type": "AGENT" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create into the freed slot\": expected HTTP 200, got {status} with {parsed}"
    );
}

/// The same bound one level down: an assistant holds at most
/// `MAX_SESSIONS_PER_ASSISTANT` sessions, and a create past that is refused
/// with the error `CreateSession` models for a conflicting parent state.
#[tokio::test]
async fn a_session_create_past_the_per_assistant_ceiling_is_refused() {
    let router = emulator();
    let (_, assistant, _) = call(
        &router,
        "POST",
        "/assistants",
        Some(&json!({ "name": "session ceiling holder", "type": "AGENT" })),
    )
    .await;
    let assistant_id = str_at(&assistant, "/assistant/assistantId");
    let sessions_path = format!("/assistants/{assistant_id}/sessions");

    for index in 0..state::MAX_SESSIONS_PER_ASSISTANT {
        let (status, parsed, _) = call(
            &router,
            "POST",
            &sessions_path,
            Some(&json!({ "name": format!("bounded session {index}") })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"session {index} of {}\": expected HTTP 200, got {status} with {parsed}",
            state::MAX_SESSIONS_PER_ASSISTANT
        );
    }

    let (status, refused, _) = call(
        &router,
        "POST",
        &sessions_path,
        Some(&json!({ "name": "one session too many" })),
    )
    .await;
    assert_eq!(
        status, 409,
        "case \"one past MAX_SESSIONS_PER_ASSISTANT\": expected HTTP 409, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "case \"one past MAX_SESSIONS_PER_ASSISTANT\": unexpected error code in {refused}"
    );
}

/// A resource's tag map is bounded too: AWS allows at most 50 tags per
/// resource, so a create naming more is refused, for every create that takes
/// tags.
#[tokio::test]
async fn a_create_naming_more_tags_than_a_resource_holds_is_refused() {
    let router = emulator();
    let (_, assistant, _) = call(
        &router,
        "POST",
        "/assistants",
        Some(&json!({ "name": "tag ceiling parent", "type": "AGENT" })),
    )
    .await;
    let parent = str_at(&assistant, "/assistant/assistantId");
    let sessions_path = format!("/assistants/{parent}/sessions");

    /// `tag_count` tags, each a distinct key.
    fn tags(tag_count: usize) -> Value {
        Value::Object(
            (0..tag_count)
                .map(|index| (format!("key{index}"), json!(format!("value{index}"))))
                .collect(),
        )
    }

    // (tag count, expected status): 0, 1 and the published 50-tag maximum are
    // accepted; one past it is not.
    for (tag_count, expected) in [(0usize, 200u16), (1, 200), (50, 200), (51, 400)] {
        let (status, parsed, _) = call(
            &router,
            "POST",
            "/assistants",
            Some(&json!({
                "name": format!("assistant with {tag_count} tags"),
                "type": "AGENT",
                "tags": tags(tag_count),
            })),
        )
        .await;
        assert_eq!(
            status, expected,
            "case \"CreateAssistant with {tag_count} tags\": expected HTTP {expected}, got {status} with {parsed}"
        );

        let (status, parsed, _) = call(
            &router,
            "POST",
            &sessions_path,
            Some(&json!({
                "name": format!("session with {tag_count} tags"),
                "tags": tags(tag_count),
            })),
        )
        .await;
        assert_eq!(
            status, expected,
            "case \"CreateSession with {tag_count} tags\": expected HTTP {expected}, got {status} with {parsed}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case \"CreateSession with {tag_count} tags\": unexpected error code in {parsed}"
            );
        }
    }
}

/// The `clientToken` index is bounded by eviction rather than by refusal: it
/// never holds more than `MAX_REMEMBERED_CLIENT_TOKENS` entries, and the entry
/// it drops is always the one remembered longest ago.
#[test]
fn the_client_token_index_never_grows_past_its_ceiling() {
    let mut store = state::ComAmazonawsWisdomWisdomserviceState::default();
    let overflow = 25usize;

    for index in 0..state::MAX_REMEMBERED_CLIENT_TOKENS + overflow {
        store.remember_client_token(
            "CreateAssistant",
            "",
            &format!("token-{index}"),
            &format!("resource-{index}"),
        );
        assert!(
            store.idempotency.len() <= state::MAX_REMEMBERED_CLIENT_TOKENS,
            "case \"after {} tokens\": the index must hold at most {} entries, got {}",
            index + 1,
            state::MAX_REMEMBERED_CLIENT_TOKENS,
            store.idempotency.len()
        );
        assert_eq!(
            store.idempotency_order.len(),
            store.idempotency.len(),
            "case \"after {} tokens\": the order queue must mirror the index, got {} and {}",
            index + 1,
            store.idempotency_order.len(),
            store.idempotency.len()
        );
    }

    // The oldest `overflow` tokens were the ones evicted; every newer token
    // still replays its own resource.
    for index in 0..overflow {
        let token = format!("token-{index}");
        assert!(
            store
                .replayed_resource_id("CreateAssistant", "", &token)
                .is_none(),
            "case {token:?}: the oldest tokens must be the evicted ones, but it is still remembered"
        );
    }
    for index in overflow..state::MAX_REMEMBERED_CLIENT_TOKENS + overflow {
        let token = format!("token-{index}");
        let expected = format!("resource-{index}");
        assert_eq!(
            store.replayed_resource_id("CreateAssistant", "", &token),
            Some(&expected),
            "case {token:?}: expected {expected:?}, got {:?}",
            store.replayed_resource_id("CreateAssistant", "", &token)
        );
    }
}
