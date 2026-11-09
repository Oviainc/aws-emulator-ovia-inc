// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListActors`, the AgentCore Memory actor listing.
//!
//! The rule under test: `ListActors` reports every actor the addressed Memory container
//! holds in this service's state — the actors the stored events and the ingested content
//! imply, each once — ordered by `actorId` ascending and cut into pages of at most
//! `maxResults`, with the `nextToken` it issues resuming after the page it came from. A
//! container the account holds but has stored nothing in is an empty page; a container
//! the account does not hold is a `ResourceNotFoundException`. The listing reports only:
//! a later caller sees exactly what it saw.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so the wire shape (routes, status codes, `restJson1` bodies, modelled error
//! codes) is exercised, not just the handler functions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use super::harness::{create_memory, label};

/// A memory id matching `MemoryId`'s pattern
/// `^(arn:...:memory/)?[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`.
const MEMORY: &str = "actors-memory-0123456789";
/// The same container addressed by its ARN spelling, which that same pattern admits.
const MEMORY_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/actors-memory-0123456789";
/// Another id that pattern admits, for a container the account does NOT hold.
const UNHELD_MEMORY: &str = "absent-memory-9876543210";

/// The emulator, with request authentication off so a test signs nothing.
///
/// The container every case lists is created first through the shared inventory the
/// CONTROL plane's `CreateMemory` writes: this data plane creates no container of its own.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    create_memory(&engine, MEMORY);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an
/// empty body). The SigV4 credential scope is what routes the request to this service.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://bedrock-agentcore.us-east-1.amazonaws.com{path}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

async fn list_actors(router: &MultiServiceRouter, memory: &str, body: Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/actors", label(memory)),
        Some(body),
    )
    .await
}

async fn list_sessions(
    router: &MultiServiceRouter,
    memory: &str,
    actor: &str,
    body: Value,
) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/actor/{}/sessions", label(memory), label(actor)),
        Some(body),
    )
    .await
}

/// Write one short-term event, the call that brings an actor into being in a container.
async fn create_event(
    router: &MultiServiceRouter,
    memory: &str,
    actor: &str,
    session: &str,
) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/events", label(memory)),
        Some(json!({
            "actorId": actor,
            "sessionId": session,
            "eventTimestamp": 1_700_000_000,
            // `Role`'s values are ASSISTANT, USER, TOOL, OTHER (source: smithy_query
            // shape="Role").
            "payload": [{"conversational": {"role": "USER", "content": {"text": "hello"}}}],
        })),
    )
    .await
}

/// Submit content for long-term extraction: the other call that brings an actor into
/// being, without storing any event for it.
async fn ingest_data(
    router: &MultiServiceRouter,
    memory: &str,
    actor: &str,
    session: &str,
) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/memories/{}/ingest", label(memory)),
        Some(json!({
            "actorId": actor,
            "sessionId": session,
            "contentTimestamp": 1_700_000_100,
            "source": {"inline": {"payload": [
                {"conversational": {"role": "USER", "content": {"text": "remember this"}}}
            ]}},
        })),
    )
    .await
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

fn error_message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The `actorId`s on one page, in the order the page reported them.
fn listed(page: &Value) -> Vec<String> {
    page["actorSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["actorId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the table below: the writes to perform, and the actors they imply.
type Row = (&'static str, Vec<(Write, &'static str, &'static str)>, Vec<&'static str>);

/// How a write brings an actor into being: both ways a caller can, since an actor is
/// never created on its own.
#[derive(Debug, Clone, Copy)]
enum Write {
    Event,
    Ingest,
}

/// Perform `write` for `(actor, session)` and assert the service accepted it.
async fn perform(router: &MultiServiceRouter, case: &str, write: Write, actor: &str, session: &str) {
    let (status, body) = match write {
        Write::Event => create_event(router, MEMORY, actor, session).await,
        Write::Ingest => ingest_data(router, MEMORY, actor, session).await,
    };
    let expected = match write {
        Write::Event => 201,
        Write::Ingest => 202,
    };
    assert_eq!(
        status, expected,
        "case {case:?}: expected HTTP {expected} from the {write:?} write for actor \
         {actor:?}, got {status} with {body:?}"
    );
}

/// `ListActors` reports exactly the actors the stored writes imply, each once, sorted by
/// `actorId` — whichever spelling of the container the request uses.
#[tokio::test]
async fn reports_the_actors_the_stored_writes_imply_sorted_by_actor_id() {
    // Rows differ along the number of actors, the order they were written in, the form of
    // the `actorId` (`ActorId`'s pattern
    // `^[a-zA-Z0-9][a-zA-Z0-9-_/]*(?::[a-zA-Z0-9-_/]+)*[a-zA-Z0-9-_/]*$`, source:
    // smithy_query shape="ActorId"), which call brought the actor into being, and whether
    // one actor holds several sessions.
    let cases: Vec<Row> = vec![
        ("nothing-stored-in-the-container", vec![], vec![]),
        (
            "one-actor-known-through-an-event",
            vec![(Write::Event, "zeta", "s1")],
            vec!["zeta"],
        ),
        (
            "one-actor-known-only-through-ingested-content",
            vec![(Write::Ingest, "ingest-only", "s1")],
            vec!["ingest-only"],
        ),
        (
            "three-actors-written-newest-first",
            vec![
                (Write::Event, "c-actor", "s1"),
                (Write::Event, "b-actor", "s1"),
                (Write::Event, "a-actor", "s1"),
            ],
            vec!["a-actor", "b-actor", "c-actor"],
        ),
        (
            "actor-id-forms-the-pattern-admits",
            vec![
                (Write::Event, "team/alpha", "s1"),
                (Write::Ingest, "tenant:beta/7", "s1"),
                (Write::Event, "9-numeric-start", "s1"),
                (Write::Event, "under_score", "s1"),
            ],
            vec![
                "9-numeric-start",
                "team/alpha",
                "tenant:beta/7",
                "under_score",
            ],
        ),
        (
            "one-actor-in-several-sessions-is-one-summary",
            vec![
                (Write::Event, "repeat-actor", "s1"),
                (Write::Event, "repeat-actor", "s2"),
                (Write::Ingest, "repeat-actor", "s3"),
                (Write::Event, "other-actor", "s1"),
            ],
            vec!["other-actor", "repeat-actor"],
        ),
    ];

    for (case, writes, expected) in cases {
        let router = emulator();
        for (write, actor, session) in &writes {
            perform(&router, case, *write, actor, session).await;
        }

        // Both spellings of `MemoryId` address the one container, so both report the
        // same actors.
        for memory in [MEMORY, MEMORY_ARN] {
            let (status, page) = list_actors(&router, memory, json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?} addressed as {memory:?}: expected HTTP 200 from ListActors, \
                 got {status} with {page:?}"
            );
            assert_eq!(
                listed(&page),
                expected,
                "case {case:?} addressed as {memory:?}: expected {expected:?}, got {:?}",
                listed(&page)
            );
            assert_eq!(
                page["nextToken"],
                Value::Null,
                "case {case:?} addressed as {memory:?}: expected no nextToken on a single \
                 complete page, got {:?}",
                page["nextToken"]
            );
        }

        // Reporting only: a second listing sees exactly what the first did, and so does
        // the write path's own view of the container.
        let (status, again) = list_actors(&router, MEMORY, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated ListActors to answer, got {status} with \
             {again:?}"
        );
        assert_eq!(
            listed(&again),
            expected,
            "case {case:?}: expected the repeated listing to report {expected:?}, got {:?}",
            listed(&again)
        );
    }
}

/// The actors a listing reports are the ones whose sessions `ListSessions` reports: one
/// store, two views of it.
#[tokio::test]
async fn every_reported_actor_has_the_sessions_it_was_written_with() {
    let router = emulator();
    let written: Vec<(&str, Vec<&str>)> = vec![
        ("actor-one", vec!["s1"]),
        ("actor-two", vec!["s1", "s2"]),
        ("actor-three", vec!["s1", "s2", "s3"]),
    ];
    for (actor, sessions) in &written {
        for session in sessions {
            perform(&router, actor, Write::Event, actor, session).await;
        }
    }

    let (status, page) = list_actors(&router, MEMORY, json!({})).await;
    assert_eq!(
        status, 200,
        "case the written actors: expected HTTP 200 from ListActors, got {status} with {page:?}"
    );
    let reported = listed(&page);
    let mut expected: Vec<String> = written
        .iter()
        .map(|(actor, _)| (*actor).to_string())
        .collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "case the written actors: expected {expected:?}, got {reported:?}"
    );

    for (actor, sessions) in &written {
        let (status, listing) = list_sessions(&router, MEMORY, actor, json!({})).await;
        assert_eq!(
            status, 200,
            "case {actor:?}: expected HTTP 200 from ListSessions, got {status} with {listing:?}"
        );
        let count = listing["sessionSummaries"]
            .as_array()
            .map_or(0, Vec::len);
        assert_eq!(
            count,
            sessions.len(),
            "case {actor:?}: expected the reported actor to hold {} session(s), got \
             {count} in {listing:?}",
            sessions.len()
        );
    }
}

/// Paging covers every actor exactly once, in `actorId` order, for any page size the
/// model admits: each page holds at most `maxResults`, the token resumes after it, and
/// the last page carries none.
#[tokio::test]
async fn pages_cover_every_actor_exactly_once() {
    let router = emulator();
    // Written in an order that is neither ascending nor descending, so the paging order
    // cannot be the insertion order.
    let actors = [
        "actor-d", "actor-a", "actor-g", "actor-c", "actor-f", "actor-b", "actor-e",
    ];
    for actor in actors {
        perform(&router, "seven actors", Write::Event, actor, "s1").await;
    }
    let mut expected: Vec<String> = actors.iter().map(|a| (*a).to_string()).collect();
    expected.sort();

    // `maxResults` is `@range(min: 1, max: 100)` (source: smithy_query operation
    // ListActors): both published bounds, the sizes that divide and do not divide the
    // seven actors, and the exact count.
    for max_results in [1, 2, 3, 6, 7, 8, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": max_results});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (status, page) = list_actors(&router, MEMORY, request).await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: expected HTTP 200 from ListActors, got \
                 {status} with {page:?}"
            );
            let ids = listed(&page);
            assert!(
                ids.len() <= max_results,
                "case maxResults={max_results}: expected at most {max_results} summaries \
                 on a page, got {ids:?}"
            );
            collected.extend(ids);
            pages += 1;
            assert!(
                pages <= actors.len() + 1,
                "case maxResults={max_results}: expected paging to end, got {pages} pages \
                 and {collected:?}"
            );
            match page["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case maxResults={max_results}: expected the pages to cover {expected:?} in \
             order, got {collected:?}"
        );
    }
}

/// A `maxResults` outside the published range is refused, and each published bound is
/// accepted.
#[tokio::test]
async fn max_results_is_held_to_the_published_range() {
    let router = emulator();
    perform(&router, "one actor", Write::Event, "actor-a", "s1").await;

    // `maxResults` is `@range(min: 1, max: 100)` (source: smithy_query operation
    // ListActors): each bound, and one step past each.
    let cases: [(i64, u16); 4] = [(0, 400), (1, 200), (100, 200), (101, 400)];
    for (max_results, expected) in cases {
        let (status, body) = list_actors(&router, MEMORY, json!({"maxResults": max_results})).await;
        assert_eq!(
            status, expected,
            "case maxResults={max_results}: expected HTTP {expected}, got {status} with {body:?}"
        );
        if expected == 200 {
            assert_eq!(
                listed(&body),
                vec!["actor-a".to_string()],
                "case maxResults={max_results}: expected the one stored actor, got {body:?}"
            );
        }
    }
}

/// A `nextToken` this listing did not issue is refused as invalid input rather than
/// silently resuming somewhere: a token belongs to the list it came from.
#[tokio::test]
async fn a_token_from_elsewhere_is_refused() {
    let router = emulator();
    for actor in ["actor-a", "actor-b"] {
        perform(&router, "two actors", Write::Event, actor, "s1").await;
        perform(&router, "two actors", Write::Event, actor, "s2").await;
    }

    // A token this listing DID issue resumes it.
    let (status, first) = list_actors(&router, MEMORY, json!({"maxResults": 1})).await;
    assert_eq!(
        status, 200,
        "case the first page: expected HTTP 200, got {status} with {first:?}"
    );
    let own = first["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !own.is_empty(),
        "case the first page: expected a nextToken for the second page, got {first:?}"
    );
    let (status, resumed) = list_actors(&router, MEMORY, json!({"nextToken": own})).await;
    assert_eq!(
        status, 200,
        "case its own token: expected HTTP 200, got {status} with {resumed:?}"
    );
    assert_eq!(
        listed(&resumed),
        vec!["actor-b".to_string()],
        "case its own token: expected the rest of the list, got {resumed:?}"
    );

    // A token another paginated listing issued belongs to that list, not this one.
    let (status, sessions) =
        list_sessions(&router, MEMORY, "actor-a", json!({"maxResults": 1})).await;
    assert_eq!(
        status, 200,
        "case the sessions page: expected HTTP 200, got {status} with {sessions:?}"
    );
    let foreign = sessions["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !foreign.is_empty(),
        "case the sessions page: expected a nextToken to borrow, got {sessions:?}"
    );

    for (case, token) in [
        ("a token from the sessions listing", foreign.as_str()),
        ("a token that decodes to nothing", "not-a-token"),
        ("an empty token", ""),
    ] {
        let (status, body) = list_actors(&router, MEMORY, json!({"nextToken": token})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }
}

/// A container the account does not hold is a `ResourceNotFoundException` naming it,
/// whichever spelling the request used; one it holds answers instead.
#[tokio::test]
async fn a_container_the_account_does_not_hold_is_refused() {
    let router = emulator();
    perform(&router, "one actor", Write::Event, "actor-a", "s1").await;

    let unheld_arn =
        format!("arn:aws:bedrock-agentcore:us-east-1:123456789012:memory/{UNHELD_MEMORY}");
    for (case, memory) in [
        ("the bare id of an unheld container", UNHELD_MEMORY),
        ("the ARN of an unheld container", unheld_arn.as_str()),
    ] {
        let (status, body) = list_actors(&router, memory, json!({})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            format!("Memory not found: {memory}"),
            "case {case:?}: expected the message to name the container as the request \
             spelled it, got {:?}",
            error_message(&body)
        );
    }

    let (status, page) = list_actors(&router, MEMORY, json!({})).await;
    assert_eq!(
        status, 200,
        "case the held container: expected HTTP 200, got {status} with {page:?}"
    );
    assert_eq!(
        listed(&page),
        vec!["actor-a".to_string()],
        "case the held container: expected the one stored actor, got {page:?}"
    );
}
