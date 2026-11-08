// SPDX-License-Identifier: CC0-1.0

//! `ExecuteFastReset`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! One rule is under test: fast reset is a two-step command, and only the token the
//! engine itself minted on the latest `initiateDatabaseReset` lets a
//! `performDatabaseReset` empty the database — a request carrying no token is refused for
//! the missing parameter, any other token value is refused as invalid, and a token that
//! did perform a reset is consumed, so it cannot perform a second one.
//!
//! The table rows differ along what the rule generalises over: whether a token is sent at
//! all, its form (absent, empty, blank, free text, a well-formed UUID the engine never
//! minted), and its age (a superseded token, the outstanding one, a consumed one).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_neptunedata_amazonneptunedataplane::state::{
    ComAmazonawsNeptunedataAmazonneptunedataplaneState, GremlinQueryRecord, QueryState,
};
use service_com_amazonaws_neptunedata_amazonneptunedataplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service. `neptune-db` is neptunedata's sigv4
/// signing name, and it is what routes a request to this service.
const SIGNED_FOR_NEPTUNE_DB: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/neptune-db/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The two values the model's `Action` enum carries, spelled out here rather than
/// imported so a wrong spelling in the handler fails this test.
/// Source: smithy_query(query="shape", shape="ExecuteFastResetInput") — member `action`.
const INITIATE: &str = "initiateDatabaseReset";
const PERFORM: &str = "performDatabaseReset";

/// One emulator: a router carrying only this service, over one shared engine. The engine
/// is handed back so a test can seed the Gremlin registry and read the stored fast-reset
/// record the way another request would.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`) without its shape namespace, or the empty string on
    /// a success.
    fn error_code(&self) -> &str {
        self.body["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
    }

    fn detailed_message(&self) -> &str {
        self.body["detailedMessage"].as_str().unwrap_or_default()
    }

    fn request_id(&self) -> String {
        self.body["requestId"].as_str().unwrap_or_default().to_string()
    }

    /// `payload.token`, or the empty string when the answer carries no payload.
    fn payload_token(&self) -> &str {
        self.body["payload"]["token"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "neptune-db.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_NEPTUNE_DB);
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

/// `ExecuteFastReset` is `POST /system`, with `action` and `token` in the JSON body.
/// Source: smithy_query(query="operation", operation="ExecuteFastReset") — http binding.
async fn fast_reset(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/system", Some(body)).await
}

async fn initiate(router: &MultiServiceRouter) -> Answer {
    fast_reset(router, json!({ "action": INITIATE })).await
}

/// Perform a reset, with `token` sent only when the case supplies one — an absent member
/// is a different request from one carrying an empty string.
async fn perform(router: &MultiServiceRouter, token: Option<&str>) -> Answer {
    let body = match token {
        Some(token) => json!({ "action": PERFORM, "token": token }),
        None => json!({ "action": PERFORM }),
    };
    fast_reset(router, body).await
}

/// Whether `value` is a version-4 UUID in the hyphenated form Neptune reports a
/// fast-reset token as (`FastResetToken$token`: "A UUID generated by the database").
/// Source of the form: 8-4-4-4-12 lowercase hex, with the version nibble `4` and the
/// variant nibble one of 8/9/a/b.
fn is_v4_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    if groups.iter().map(|group| group.len()).ne([8, 4, 4, 4, 12]) {
        return false;
    }
    if !value
        .chars()
        .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return false;
    }
    let version = groups.get(2).and_then(|group| group.chars().next());
    let variant = groups.get(3).and_then(|group| group.chars().next());
    version == Some('4') && variant.is_some_and(|c| matches!(c, '8' | '9' | 'a' | 'b'))
}

/// The token the store currently holds as the outstanding one.
fn pending_token(engine: &engine::InMemoryEngine) -> Option<String> {
    engine
        .state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>()
        .lock()
        .fast_reset
        .pending_token
        .clone()
}

/// How many resets the store records.
fn resets_performed(engine: &engine::InMemoryEngine) -> u64 {
    engine
        .state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>()
        .lock()
        .fast_reset
        .resets_performed
}

/// When the store says the outstanding token was minted, and when the latest reset ran.
fn reset_instants(
    engine: &engine::InMemoryEngine,
) -> (Option<SystemTime>, Option<SystemTime>) {
    let state = engine.state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>();
    let store = state.lock();
    (
        store.fast_reset.token_issued_at,
        store.fast_reset.last_reset_at,
    )
}

/// Assert a refusal: the status, the modelled error code, and the two members every
/// Neptune error carries (`code` naming the HTTP status, `requestId` naming the response).
fn assert_refused(case: &str, answer: &Answer, expected_code: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400, got {} with body {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        expected_code,
        "case {case}: expected the modelled {expected_code}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["code"].as_str(),
        Some("400"),
        "case {case}: the error must carry its HTTP status in `code`, got {:?}",
        answer.body["code"]
    );
    assert!(
        !answer.request_id().is_empty(),
        "case {case}: the error must name its request, got {:?}",
        answer.body["requestId"]
    );
}

#[tokio::test]
async fn initiate_mints_a_fresh_uuid_token_and_stores_it_as_the_outstanding_one() {
    let Emulator { router, engine } = emulator();

    assert_eq!(
        pending_token(&engine),
        None,
        "case fresh engine: nothing may be outstanding before a reset is initiated"
    );

    // Several initiates in a row: each mints its own token, and the newest is the one the
    // store holds — one outstanding token, never a pile of them.
    let mut minted: Vec<String> = Vec::new();
    for round in 1..=3u32 {
        let answer = initiate(&router).await;
        let case = format!("initiate #{round}");
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {:?}",
            answer.status, answer.body
        );
        let token = answer.payload_token().to_string();
        assert!(
            is_v4_uuid(&token),
            "case {case}: expected a hyphenated v4 UUID token, got {token:?}"
        );
        assert!(
            !minted.contains(&token),
            "case {case}: token {token:?} was already minted by an earlier initiate"
        );
        assert_eq!(
            answer.body["status"].as_str(),
            Some("200 OK"),
            "case {case}: expected the accepted status, got {:?}",
            answer.body["status"]
        );
        assert_eq!(
            pending_token(&engine).as_deref(),
            Some(token.as_str()),
            "case {case}: the minted token must be the one the store holds"
        );
        minted.push(token);
    }

    let (issued_at, reset_at) = reset_instants(&engine);
    assert!(
        issued_at.is_some(),
        "case initiate only: the store must record when the outstanding token was minted"
    );
    assert_eq!(
        reset_at, None,
        "case initiate only: minting a token must record no reset, got {reset_at:?}"
    );
    assert_eq!(
        resets_performed(&engine),
        0,
        "case initiate only: minting a token must not reset anything"
    );
}

/// Which token a perform presents. The rows span the forms a request can carry: no token
/// at all, one that is present but carries nothing, free text, a well-formed UUID the
/// engine never minted, a token a later initiate superseded, and the outstanding one.
#[derive(Debug, Clone, Copy)]
enum Presented {
    Absent,
    Literal(&'static str),
    /// A token a second initiate superseded.
    Superseded,
    /// The token the latest initiate minted.
    Outstanding,
}

/// The error code a presented token must be refused with, or `None` when it must work.
/// `MissingParameterException` is the modelled "a required parameter is missing";
/// `InvalidParameterException` is the modelled "a parameter value is not valid".
const PRESENTED_CASES: [(Presented, Option<&str>); 8] = [
    (Presented::Absent, Some("MissingParameterException")),
    (Presented::Literal(""), Some("MissingParameterException")),
    (Presented::Literal("   "), Some("MissingParameterException")),
    (Presented::Literal("not-a-token"), Some("InvalidParameterException")),
    // A well-formed v4 UUID this engine never minted: the FORM is right and it still
    // cannot authorise a reset.
    (
        Presented::Literal("3f2504e0-4f89-41d3-9a0c-0305e82c3301"),
        Some("InvalidParameterException"),
    ),
    // The token with a character appended — a near miss must not match either.
    (
        Presented::Literal("near-miss"),
        Some("InvalidParameterException"),
    ),
    (Presented::Superseded, Some("InvalidParameterException")),
    (Presented::Outstanding, None),
];

#[tokio::test]
async fn perform_resets_only_for_the_outstanding_token() {
    for (presented, expected_error) in PRESENTED_CASES {
        let Emulator { router, engine } = emulator();
        let case = format!("{presented:?}");

        // Every row starts from the same place: one outstanding token.
        let first = initiate(&router).await;
        let first_token = first.payload_token().to_string();

        let token = match presented {
            Presented::Absent => None,
            Presented::Literal("near-miss") => Some(format!("{first_token}x")),
            Presented::Literal(literal) => Some(literal.to_string()),
            Presented::Superseded => {
                // A second initiate replaces the token, so the first one no longer
                // authorises anything.
                let second = initiate(&router).await;
                assert_ne!(
                    second.payload_token(),
                    first_token,
                    "case {case}: a second initiate must mint a different token"
                );
                Some(first_token.clone())
            }
            Presented::Outstanding => Some(first_token.clone()),
        };

        let answer = perform(&router, token.as_deref()).await;

        match expected_error {
            Some(code) => {
                assert_refused(&case, &answer, code);
                if code == "MissingParameterException" {
                    assert!(
                        answer.detailed_message().contains("token"),
                        "case {case}: the message must name the missing member, got {:?}",
                        answer.detailed_message()
                    );
                }
                assert_eq!(
                    resets_performed(&engine),
                    0,
                    "case {case}: a refused perform must not reset anything"
                );
                assert!(
                    pending_token(&engine).is_some(),
                    "case {case}: a refused perform must leave the outstanding token in place"
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200, got {} with body {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.body["status"].as_str(),
                    Some("200 OK"),
                    "case {case}: expected the accepted status, got {:?}",
                    answer.body["status"]
                );
                // "The payload is only returned by the initiateDatabaseReset action".
                assert!(
                    answer.body["payload"].is_null(),
                    "case {case}: a performed reset carries no payload, got {:?}",
                    answer.body["payload"]
                );
                assert_eq!(
                    resets_performed(&engine),
                    1,
                    "case {case}: the performed reset must be recorded"
                );
                assert_eq!(
                    pending_token(&engine),
                    None,
                    "case {case}: the token must be consumed by the reset it authorised"
                );
                let (issued_at, reset_at) = reset_instants(&engine);
                assert_eq!(
                    issued_at, None,
                    "case {case}: the consumed token's mint instant must go with it, got {issued_at:?}"
                );
                assert!(
                    reset_at.is_some(),
                    "case {case}: the store must record when the reset ran"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_consumed_token_cannot_reset_the_database_again() {
    let Emulator { router, engine } = emulator();

    let token = initiate(&router).await.payload_token().to_string();
    let performed = perform(&router, Some(&token)).await;
    assert_eq!(
        performed.status, 200,
        "case first perform: expected 200, got {} with body {:?}",
        performed.status, performed.body
    );

    // Replaying the same request is not idempotent in the sense of "does it again": the
    // token is consumed, so the second attempt cannot empty the database.
    let replayed = perform(&router, Some(&token)).await;
    assert_refused("replayed token", &replayed, "InvalidParameterException");
    assert_ne!(
        performed.request_id(),
        replayed.request_id(),
        "case replayed token: two responses must not share a requestId"
    );
    assert_eq!(
        resets_performed(&engine),
        1,
        "case replayed token: the database must have been reset exactly once"
    );

    // And a fresh two-step command works again from scratch, so the refusal above is
    // about the token rather than about the engine refusing to reset twice.
    let second_token = initiate(&router).await.payload_token().to_string();
    assert_ne!(
        second_token, token,
        "case second command: a new initiate must mint a new token"
    );
    let second = perform(&router, Some(&second_token)).await;
    assert_eq!(
        second.status, 200,
        "case second command: expected 200, got {} with body {:?}",
        second.status, second.body
    );
    assert_eq!(
        resets_performed(&engine),
        2,
        "case second command: both resets must be recorded"
    );
}

/// One query in the registry when the reset happens, and what must be true of it
/// afterwards. The rows differ in how far the query had got — the only thing the rule
/// keys on.
#[derive(Debug, Clone, Copy)]
struct QueryCase {
    id: &'static str,
    query: &'static str,
    state: QueryState,
    /// Whether the reset must leave the record cancelled. A query the engine still owed
    /// an answer for was asked about data the reset removes; one that had already
    /// finished keeps the outcome it finished with.
    cancelled_by_reset: bool,
}

const QUERY_CASES: [QueryCase; 4] = [
    QueryCase {
        id: "0f0c1a1e-0000-4000-8000-0000000000a1",
        query: "g.V().limit(1)",
        state: QueryState::Waiting,
        cancelled_by_reset: true,
    },
    QueryCase {
        id: "0f0c1a1e-0000-4000-8000-0000000000a2",
        query: "g.V().hasLabel('person').out('knows').values('name')",
        state: QueryState::Running,
        cancelled_by_reset: true,
    },
    QueryCase {
        id: "0f0c1a1e-0000-4000-8000-0000000000a3",
        query: "g.V().count()",
        state: QueryState::Done,
        cancelled_by_reset: false,
    },
    QueryCase {
        id: "0f0c1a1e-0000-4000-8000-0000000000a4",
        query: "g.addV('person').property('name','Ärger: a \"quoted\" ünïcode name')",
        state: QueryState::Cancelled,
        cancelled_by_reset: true,
    },
];

#[tokio::test]
async fn the_reset_leaves_no_query_the_engine_still_owes_an_answer_for() {
    let Emulator { router, engine } = emulator();

    // Seed the registry the way whatever admits a query would.
    let now = SystemTime::now();
    {
        let state = engine.state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>();
        let mut store = state.lock();
        for case in &QUERY_CASES {
            let mut record =
                GremlinQueryRecord::accepted(case.id, case.query, now - Duration::from_secs(5));
            record.state = case.state;
            if case.state != QueryState::Waiting {
                record.started_at = Some(now - Duration::from_secs(4));
            }
            if case.state.is_terminal() {
                record.finished_at = Some(now - Duration::from_secs(3));
                record.cancelled = case.state == QueryState::Cancelled;
            }
            store.gremlin_queries.insert(record.query_id.clone(), record);
        }
    }

    let token = initiate(&router).await.payload_token().to_string();
    let performed = perform(&router, Some(&token)).await;
    assert_eq!(
        performed.status, 200,
        "case perform: expected 200, got {} with body {:?}",
        performed.status, performed.body
    );

    // Read every seeded id back through the implemented status reader.
    for case in &QUERY_CASES {
        let answer = call(&router, "GET", &format!("/gremlin/status/{}", case.id), None).await;
        assert_eq!(
            answer.status, 200,
            "case {}: the record must still be reported after the reset, got {} with body {:?}",
            case.id, answer.status, answer.body
        );
        assert_eq!(
            answer.body["queryString"].as_str(),
            Some(case.query),
            "case {}: the stored query string must survive the reset, got {:?}",
            case.id,
            answer.body["queryString"]
        );
        assert_eq!(
            answer.body["queryEvalStats"]["cancelled"].as_bool(),
            Some(case.cancelled_by_reset),
            "case {}: expected cancelled={}, got {:?}",
            case.id,
            case.cancelled_by_reset,
            answer.body["queryEvalStats"]
        );
    }

    // Nothing is accepted or running any more: the list reports no current query, with or
    // without the queued ones.
    for query in ["", "?includeWaiting=true"] {
        let listed = call(&router, "GET", &format!("/gremlin/status{query}"), None).await;
        assert_eq!(
            listed.body["acceptedQueryCount"].as_i64(),
            Some(0),
            "case list{query}: no query may be outstanding after a reset, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.body["runningQueryCount"].as_i64(),
            Some(0),
            "case list{query}: no query may be running after a reset, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.body["queries"].as_array().map(Vec::len),
            Some(0),
            "case list{query}: the current-query list must be empty, got {:?}",
            listed.body["queries"]
        );
    }
}

/// An `action` outside the modelled `Action` enum is refused before the handler runs, and
/// a refused command must leave the store exactly as it was.
#[tokio::test]
async fn an_action_the_model_does_not_declare_is_refused_and_resets_nothing() {
    let Emulator { router, engine } = emulator();

    for action in ["", "initiate", "INITIATEDATABASERESET", "dropDatabase"] {
        let answer = fast_reset(&router, json!({ "action": action })).await;
        assert_eq!(
            answer.status, 400,
            "case {action:?}: an unmodelled action must be refused, got {} with body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            pending_token(&engine),
            None,
            "case {action:?}: a refused command must mint no token"
        );
        assert_eq!(
            resets_performed(&engine),
            0,
            "case {action:?}: a refused command must reset nothing"
        );
    }
}
