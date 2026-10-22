// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Lex runtime conversation slice: `PutSession` creates or
//! modifies a session, `PostText` advances it a turn, `GetSession` reports it and
//! `DeleteSession` removes it, all keyed by (botName, botAlias, userId).
//!
//! Every test drives the real wire path — a `restJson1` HTTP request through the router
//! that mounts the service on one shared engine — so what is asserted is what a client
//! sees, headers and bodies included, not an internal call.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lexruntimeservice_awsdeepsenseruntimeservice::ops::types::{
    DIALOG_ACTION_TYPE_VALUES, DIALOG_STATE_VALUES, FULFILLMENT_STATE_VALUES,
};
use service_com_amazonaws_lexruntimeservice_awsdeepsenseruntimeservice::state::{
    ACCEPTED_ACCEPT_VALUES, MAX_SESSIONS, MAX_SESSION_ATTRIBUTES,
};
use service_com_amazonaws_lexruntimeservice_awsdeepsenseruntimeservice::EmulatedService;
use service_com_amazonaws_lexruntimeservice_awsdeepsenseruntimeservice::ComAmazonawsLexruntimeserviceAwsdeepsenseruntimeserviceState as LexRuntimeState;
use smithy_server_runtime::MultiServiceRouter;

type Response = http::Response<bytes::Bytes>;

/// One emulator with this service mounted on a single shared engine, exactly as the
/// production router mounts it. Request authentication is switched off so a test does not
/// have to sign; routing still goes through the SigV4 credential scope below. The engine
/// is kept so a test can read the store directly where the wire exposes no count.
struct Lex {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Lex {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// How many conversations the store holds.
    fn stored_sessions(&self) -> usize {
        self.engine.state::<LexRuntimeState>().lock().sessions.len()
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        accept: Option<&str>,
        body: Option<&Value>,
    ) -> Response {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://runtime.lex.us-east-1.amazonaws.com{path}"))
            .header("host", "runtime.lex.us-east-1.amazonaws.com")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/lex/aws4_request, \
                 SignedHeaders=host, Signature=0",
            );
        if let Some(accept) = accept {
            builder = builder.header("accept", accept);
        }
        let payload = match body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                bytes::Bytes::from(value.to_string())
            }
            None => bytes::Bytes::new(),
        };
        self.router
            .dispatch(builder.body(payload).expect("build the request"))
            .await
    }

    async fn put_session(&self, user: &str, accept: Option<&str>, body: &Value) -> Response {
        self.send(
            "POST",
            &format!("/bot/OrderFlowers/alias/prod/user/{user}/session"),
            accept,
            Some(body),
        )
        .await
    }

    async fn post_text(&self, user: &str, body: &Value) -> Response {
        self.send(
            "POST",
            &format!("/bot/OrderFlowers/alias/prod/user/{user}/text"),
            None,
            Some(body),
        )
        .await
    }

    async fn get_session(&self, user: &str, filter: Option<&str>) -> Response {
        let query = filter.map_or_else(String::new, |label| {
            format!("?checkpointLabelFilter={label}")
        });
        self.send(
            "GET",
            &format!("/bot/OrderFlowers/alias/prod/user/{user}/session{query}"),
            None,
            None,
        )
        .await
    }

    async fn delete_session(&self, user: &str) -> Response {
        self.send(
            "DELETE",
            &format!("/bot/OrderFlowers/alias/prod/user/{user}/session"),
            None,
            None,
        )
        .await
    }
}

fn header(response: &Response, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn body_json(response: &Response) -> Value {
    if response.body().is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(response.body()).unwrap_or(Value::Null)
}

/// The wire error code of a failure: `restJson1` carries it in `x-amzn-errortype` and
/// repeats it in the body's `__type`.
fn error_code(response: &Response) -> String {
    let from_header = header(response, "x-amzn-errortype");
    if !from_header.is_empty() {
        return from_header;
    }
    body_json(response)
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A dialog action that is complete for its type, so only the `type`/`fulfillmentState`
/// pair under test decides the resulting `dialogState`.
fn complete_action(action_type: &str, fulfillment_state: Option<&str>) -> Value {
    let mut action = json!({ "type": action_type, "intentName": "OrderFlowers" });
    if action_type == "ElicitSlot" {
        action["slotToElicit"] = json!("FlowerType");
    }
    if let Some(state) = fulfillment_state {
        action["fulfillmentState"] = json!(state);
    }
    action
}

#[tokio::test]
async fn a_session_is_created_advanced_read_and_deleted() {
    let lex = Lex::new();

    let put = lex
        .put_session(
            "user-1",
            None,
            &json!({
                "sessionAttributes": { "cart": "1" },
                "dialogAction": {
                    "type": "ElicitSlot",
                    "intentName": "OrderFlowers",
                    "slotToElicit": "FlowerType",
                    "slots": {}
                },
                "recentIntentSummaryView": [
                    {
                        "intentName": "OrderFlowers",
                        "checkpointLabel": "checkpoint-1",
                        "dialogActionType": "ElicitSlot"
                    },
                    {
                        "intentName": "BookCar",
                        "checkpointLabel": "checkpoint-2",
                        "dialogActionType": "ElicitIntent"
                    }
                ]
            }),
        )
        .await;
    assert_eq!(
        put.status(),
        200,
        "PutSession: expected 200, got {:?} / {:?}",
        put.status(),
        body_json(&put)
    );
    let session_id = header(&put, "x-amz-lex-session-id");
    assert!(
        !session_id.is_empty(),
        "PutSession: expected a session id header, got {:?}",
        put.headers()
    );
    assert_eq!(
        header(&put, "x-amz-lex-dialog-state"),
        "ElicitSlot",
        "PutSession: expected dialogState ElicitSlot, got {:?}",
        header(&put, "x-amz-lex-dialog-state")
    );
    assert_eq!(
        header(&put, "x-amz-lex-intent-name"),
        "OrderFlowers",
        "PutSession: expected intentName OrderFlowers, got {:?}",
        header(&put, "x-amz-lex-intent-name")
    );
    assert_eq!(
        header(&put, "x-amz-lex-slot-to-elicit"),
        "FlowerType",
        "PutSession: expected slotToElicit FlowerType, got {:?}",
        header(&put, "x-amz-lex-slot-to-elicit")
    );
    let put_attributes: Value =
        serde_json::from_str(&header(&put, "x-amz-lex-session-attributes")).unwrap_or(Value::Null);
    assert_eq!(
        put_attributes,
        json!({ "cart": "1" }),
        "PutSession: expected the persisted session attributes, got {put_attributes:?}"
    );
    assert!(
        put.body().is_empty(),
        "PutSession: expected the modeled empty audioStream, got {} bytes",
        put.body().len()
    );

    let text = lex
        .post_text(
            "user-1",
            &json!({ "inputText": "roses", "sessionAttributes": { "promo": "spring" } }),
        )
        .await;
    assert_eq!(
        text.status(),
        200,
        "PostText: expected 200, got {:?} / {:?}",
        text.status(),
        body_json(&text)
    );
    let text_body = body_json(&text);
    assert_eq!(
        text_body["sessionId"],
        json!(session_id.clone()),
        "PostText: expected the session id PutSession returned ({session_id}), got {:?}",
        text_body["sessionId"]
    );
    assert_eq!(
        text_body["sessionAttributes"],
        json!({ "cart": "1", "promo": "spring" }),
        "PostText: expected the merged session attributes, got {:?}",
        text_body["sessionAttributes"]
    );
    assert_eq!(
        text_body["dialogState"],
        json!("ElicitSlot"),
        "PostText: expected the persisted dialogState, got {:?}",
        text_body["dialogState"]
    );
    assert_eq!(
        text_body["intentName"],
        json!("OrderFlowers"),
        "PostText: expected the carried-forward intentName, got {:?}",
        text_body["intentName"]
    );
    assert_eq!(
        text_body["slotToElicit"],
        json!("FlowerType"),
        "PostText: expected the carried-forward slotToElicit, got {:?}",
        text_body["slotToElicit"]
    );
    assert_eq!(
        text_body["botVersion"],
        json!("$LATEST"),
        "PostText: expected botVersion $LATEST, got {:?}",
        text_body["botVersion"]
    );

    let get = lex.get_session("user-1", None).await;
    assert_eq!(
        get.status(),
        200,
        "GetSession: expected 200, got {:?} / {:?}",
        get.status(),
        body_json(&get)
    );
    let get_body = body_json(&get);
    assert_eq!(
        get_body["sessionId"],
        json!(session_id.clone()),
        "GetSession: expected the same session id, got {:?}",
        get_body["sessionId"]
    );
    assert_eq!(
        get_body["sessionAttributes"],
        json!({ "cart": "1", "promo": "spring" }),
        "GetSession: expected the attributes PostText merged to be persisted, got {:?}",
        get_body["sessionAttributes"]
    );
    assert_eq!(
        get_body["dialogAction"]["type"],
        json!("ElicitSlot"),
        "GetSession: expected the dialogAction rebuilt from stored state, got {:?}",
        get_body["dialogAction"]
    );
    assert_eq!(
        get_body["dialogAction"]["slotToElicit"],
        json!("FlowerType"),
        "GetSession: expected the stored slotToElicit, got {:?}",
        get_body["dialogAction"]
    );
    let view = get_body["recentIntentSummaryView"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        view.len(),
        2,
        "GetSession: expected the two stored intent summaries, got {view:?}"
    );
    assert_eq!(
        view[0]["intentName"],
        json!("OrderFlowers"),
        "GetSession: expected the turn's intent first, got {view:?}"
    );
    assert_eq!(
        view[0]["checkpointLabel"],
        json!("checkpoint-1"),
        "GetSession: expected the checkpoint label to survive the turn, got {view:?}"
    );

    let filtered = lex.get_session("user-1", Some("checkpoint-2")).await;
    assert_eq!(
        filtered.status(),
        200,
        "GetSession(filter): expected 200, got {:?}",
        filtered.status()
    );
    let filtered_view = body_json(&filtered)["recentIntentSummaryView"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        filtered_view.len(),
        1,
        "GetSession(checkpointLabelFilter=checkpoint-2): expected one summary, got {filtered_view:?}"
    );
    assert_eq!(
        filtered_view[0]["intentName"],
        json!("BookCar"),
        "GetSession(checkpointLabelFilter=checkpoint-2): expected BookCar, got {filtered_view:?}"
    );

    let deleted = lex.delete_session("user-1").await;
    assert_eq!(
        deleted.status(),
        200,
        "DeleteSession: expected 200, got {:?} / {:?}",
        deleted.status(),
        body_json(&deleted)
    );
    assert_eq!(
        body_json(&deleted),
        json!({
            "botName": "OrderFlowers",
            "botAlias": "prod",
            "userId": "user-1",
            "sessionId": session_id.clone(),
        }),
        "DeleteSession: expected the key and the deleted session id, got {:?}",
        body_json(&deleted)
    );

    let gone = lex.get_session("user-1", None).await;
    assert_eq!(
        gone.status(),
        404,
        "GetSession after DeleteSession: expected 404, got {:?} / {:?}",
        gone.status(),
        body_json(&gone)
    );
    assert_eq!(
        error_code(&gone),
        "NotFoundException",
        "GetSession after DeleteSession: expected NotFoundException, got {:?}",
        error_code(&gone)
    );

    let unknown = lex
        .send(
            "GET",
            "/bot/Nope/alias/prod/user/user-1/session",
            None,
            None,
        )
        .await;
    assert_eq!(
        unknown.status(),
        404,
        "GetSession(unknown bot): expected 404, got {:?} / {:?}",
        unknown.status(),
        body_json(&unknown)
    );
}

#[tokio::test]
async fn post_text_creates_the_session_on_first_contact_and_keeps_its_id() {
    let lex = Lex::new();

    let first = lex.post_text("fresh-user", &json!({ "inputText": "hello" })).await;
    assert_eq!(
        first.status(),
        200,
        "PostText(first contact): expected 200, got {:?} / {:?}",
        first.status(),
        body_json(&first)
    );
    let body = body_json(&first);
    let session_id = body["sessionId"].as_str().unwrap_or_default().to_string();
    assert!(
        !session_id.is_empty(),
        "PostText(first contact): expected a minted session id, got {body:?}"
    );
    assert_eq!(
        body["dialogState"],
        json!("ElicitIntent"),
        "PostText(first contact): a session with no dialog set has no recognised intent, \
         so it must report ElicitIntent; got {:?}",
        body["dialogState"]
    );

    let second = lex.post_text("fresh-user", &json!({ "inputText": "roses" })).await;
    assert_eq!(
        body_json(&second)["sessionId"],
        json!(session_id.clone()),
        "PostText(second turn): expected the same session id {session_id}, got {:?}",
        body_json(&second)["sessionId"]
    );

    let get = lex.get_session("fresh-user", None).await;
    assert_eq!(
        body_json(&get)["sessionId"],
        json!(session_id.clone()),
        "GetSession: expected the id PostText minted, got {:?}",
        body_json(&get)["sessionId"]
    );
}

#[tokio::test]
async fn session_ids_are_distinct_per_session_key() {
    let lex = Lex::new();
    let mut ids = Vec::new();
    for user in ["user-a", "user-b", "user-c"] {
        let put = lex.put_session(user, None, &json!({})).await;
        assert_eq!(
            put.status(),
            200,
            "PutSession({user}): expected 200, got {:?}",
            put.status()
        );
        ids.push(header(&put, "x-amz-lex-session-id"));
    }
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "each session key must get its own session id, got {ids:?}"
    );
}

#[tokio::test]
async fn dialog_state_is_derived_from_the_dialog_action() {
    // The `type` catalog is `DialogActionType` and the `fulfillmentState` catalog is
    // `FulfillmentState`, both read from this service's model (see
    // `src/ops/types/shared_types.rs`); the expectations are the states
    // `DialogState` declares.
    let cases: &[(&str, Option<&str>, &str)] = &[
        ("ElicitIntent", None, "ElicitIntent"),
        ("ElicitSlot", None, "ElicitSlot"),
        ("ConfirmIntent", None, "ConfirmIntent"),
        ("Close", Some("Fulfilled"), "Fulfilled"),
        ("Close", Some("Failed"), "Failed"),
        ("Close", Some("ReadyForFulfillment"), "ReadyForFulfillment"),
    ];

    // The table covers every action type the model declares except `Delegate`, whose
    // next action Amazon Lex picks from the bot's dialog definition; it is exercised
    // separately below.
    for action_type in DIALOG_ACTION_TYPE_VALUES {
        assert!(
            *action_type == "Delegate"
                || cases.iter().any(|(candidate, _, _)| candidate == action_type),
            "the dialogAction.type catalog entry {action_type} is not covered by this table"
        );
    }
    for state in FULFILLMENT_STATE_VALUES {
        assert!(
            cases
                .iter()
                .any(|(_, fulfillment, _)| *fulfillment == Some(*state)),
            "the fulfillmentState catalog entry {state} is not covered by this table"
        );
    }

    let lex = Lex::new();
    for (index, (action_type, fulfillment_state, expected)) in cases.iter().enumerate() {
        let case = format!("type={action_type} fulfillmentState={fulfillment_state:?}");
        let user = format!("dialog-{index}");
        let response = lex
            .put_session(
                &user,
                None,
                &json!({ "dialogAction": complete_action(action_type, *fulfillment_state) }),
            )
            .await;
        assert_eq!(
            response.status(),
            200,
            "case {case}: expected 200, got {:?} / {:?}",
            response.status(),
            body_json(&response)
        );
        let actual = header(&response, "x-amz-lex-dialog-state");
        assert_eq!(
            &actual, expected,
            "case {case}: expected dialogState {expected}, got {actual:?}"
        );
        assert!(
            DIALOG_STATE_VALUES.contains(&actual.as_str()),
            "case {case}: {actual:?} is not a modeled DialogState value"
        );

        // The same state is what a later reader sees, so it was persisted and not
        // computed for this one response.
        let read_back = lex.post_text(&user, &json!({ "inputText": "ok" })).await;
        assert_eq!(
            body_json(&read_back)["dialogState"],
            json!(expected),
            "case {case}: PostText must report the persisted dialogState {expected}, got {:?}",
            body_json(&read_back)["dialogState"]
        );
    }
}

#[tokio::test]
async fn delegate_keeps_the_dialog_the_session_is_already_in() {
    let lex = Lex::new();
    let put = lex
        .put_session(
            "delegate-user",
            None,
            &json!({
                "dialogAction": {
                    "type": "ElicitSlot",
                    "intentName": "OrderFlowers",
                    "slotToElicit": "FlowerType",
                    "slots": { "FlowerType": "roses" }
                }
            }),
        )
        .await;
    assert_eq!(put.status(), 200, "PutSession: expected 200, got {:?}", put.status());

    let delegated = lex
        .put_session(
            "delegate-user",
            None,
            &json!({ "dialogAction": { "type": "Delegate", "slots": { "FlowerType": "tulips" } } }),
        )
        .await;
    assert_eq!(
        delegated.status(),
        200,
        "PutSession(Delegate): expected 200, got {:?} / {:?}",
        delegated.status(),
        body_json(&delegated)
    );
    assert_eq!(
        header(&delegated, "x-amz-lex-dialog-state"),
        "ElicitSlot",
        "PutSession(Delegate): the next action needs the bot's dialog definition, so the \
         session must stay in ElicitSlot; got {:?}",
        header(&delegated, "x-amz-lex-dialog-state")
    );
    let slots: Value =
        serde_json::from_str(&header(&delegated, "x-amz-lex-slots")).unwrap_or(Value::Null);
    assert_eq!(
        slots,
        json!({ "FlowerType": "tulips" }),
        "PutSession(Delegate): expected the slots the action carried, got {slots:?}"
    );
}

#[tokio::test]
async fn an_incomplete_dialog_action_is_refused_and_changes_nothing() {
    // Each row names a `dialogAction` whose type needs a member it does not carry, which
    // Amazon Lex refuses as `BadRequestException`; the last row is the complete form of
    // the same action and must be accepted.
    let cases: &[(&str, Value, u16)] = &[
        (
            "ElicitSlot without slotToElicit",
            json!({ "type": "ElicitSlot", "intentName": "OrderFlowers" }),
            400,
        ),
        (
            "ElicitSlot without intentName",
            json!({ "type": "ElicitSlot", "slotToElicit": "FlowerType" }),
            400,
        ),
        (
            "ConfirmIntent without intentName",
            json!({ "type": "ConfirmIntent" }),
            400,
        ),
        (
            "Close without fulfillmentState",
            json!({ "type": "Close", "intentName": "OrderFlowers" }),
            400,
        ),
        (
            "Close with fulfillmentState",
            json!({ "type": "Close", "intentName": "OrderFlowers", "fulfillmentState": "Fulfilled" }),
            200,
        ),
    ];

    for (case, action, expected) in cases {
        let lex = Lex::new();
        // A session already in a known dialog, so a refusal can be shown to leave it.
        let setup = lex
            .put_session(
                "refusal-user",
                None,
                &json!({ "dialogAction": { "type": "ElicitIntent" } }),
            )
            .await;
        assert_eq!(
            setup.status(),
            200,
            "case {case}: setup PutSession expected 200, got {:?}",
            setup.status()
        );

        let response = lex
            .put_session("refusal-user", None, &json!({ "dialogAction": action }))
            .await;
        assert_eq!(
            response.status().as_u16(),
            *expected,
            "case {case}: expected HTTP {expected}, got {:?} / {:?}",
            response.status(),
            body_json(&response)
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&response),
                "BadRequestException",
                "case {case}: expected BadRequestException, got {:?}",
                error_code(&response)
            );
            let after = lex.get_session("refusal-user", None).await;
            assert_eq!(
                body_json(&after)["dialogAction"]["type"],
                json!("ElicitIntent"),
                "case {case}: a refused PutSession must not change the stored dialog, got {:?}",
                body_json(&after)["dialogAction"]
            );

            // The same refusal on a key that holds NO session must leave it holding none:
            // a refused request creates nothing.
            let fresh = lex
                .put_session("never-put", None, &json!({ "dialogAction": action }))
                .await;
            assert_eq!(
                fresh.status().as_u16(),
                400,
                "case {case} (fresh key): expected HTTP 400, got {:?} / {:?}",
                fresh.status(),
                body_json(&fresh)
            );
            let fresh_after = lex.get_session("never-put", None).await;
            assert_eq!(
                fresh_after.status(),
                404,
                "case {case} (fresh key): a refused PutSession must create no session, \
                 got {:?} / {:?}",
                fresh_after.status(),
                body_json(&fresh_after)
            );
            assert_eq!(
                lex.stored_sessions(),
                1,
                "case {case} (fresh key): the store must still hold only the setup session, \
                 got {} sessions",
                lex.stored_sessions()
            );
        }
    }
}

#[tokio::test]
async fn the_accept_header_selects_the_response_content_type() {
    // The accepted values are the ones the `accept` member documents, exposed as
    // `crate::state::ACCEPTED_ACCEPT_VALUES`. `audio/*` resolves to the `audio/mpeg` its
    // documentation names as that range's default.
    let lex = Lex::new();
    for (index, accept) in ACCEPTED_ACCEPT_VALUES.iter().enumerate() {
        let user = format!("accept-{index}");
        let expected = if *accept == "audio/*" {
            "audio/mpeg"
        } else {
            accept
        };
        let response = lex.put_session(&user, Some(accept), &json!({})).await;
        assert_eq!(
            response.status(),
            200,
            "case accept={accept}: expected 200, got {:?} / {:?}",
            response.status(),
            body_json(&response)
        );
        assert_eq!(
            header(&response, "content-type"),
            expected,
            "case accept={accept}: expected the Content-Type to report {expected}, got {:?}",
            header(&response, "content-type")
        );
    }

    // However the header is spelled, and whichever of its media ranges names a form this
    // service serves, that form is what is reported.
    let spellings: &[(&str, &str)] = &[
        ("TEXT/PLAIN;CHARSET=UTF-8", "text/plain; charset=utf-8"),
        ("text/plain;charset=utf-8", "text/plain; charset=utf-8"),
        (
            "text/plain; charset=utf-8;q=0.9",
            "text/plain; charset=utf-8",
        ),
        ("audio/mpeg; q=0.8", "audio/mpeg"),
        ("*/*, text/plain; charset=utf-8", "text/plain; charset=utf-8"),
        ("application/json, audio/ogg;q=0.5", "audio/ogg"),
    ];
    for (index, (accept, expected)) in spellings.iter().enumerate() {
        let user = format!("spelling-{index}");
        let response = lex.put_session(&user, Some(accept), &json!({})).await;
        assert_eq!(
            response.status(),
            200,
            "case accept={accept}: expected 200, got {:?} / {:?}",
            response.status(),
            body_json(&response)
        );
        assert_eq!(
            header(&response, "content-type"),
            *expected,
            "case accept={accept}: expected Content-Type {expected}, got {:?}",
            header(&response, "content-type")
        );
    }

    // A header naming no form this service serves is the modeled 406, and it creates no
    // session.
    let unacceptable = lex
        .put_session("accept-bad", Some("application/xml, image/png"), &json!({}))
        .await;
    assert_eq!(
        unacceptable.status(),
        406,
        "case accept=application/xml: expected 406, got {:?} / {:?}",
        unacceptable.status(),
        body_json(&unacceptable)
    );
    assert_eq!(
        error_code(&unacceptable),
        "NotAcceptableException",
        "case accept=application/xml: expected NotAcceptableException, got {:?}",
        error_code(&unacceptable)
    );
    let after = lex.get_session("accept-bad", None).await;
    assert_eq!(
        after.status(),
        404,
        "case accept=application/xml: a refused PutSession must create no session, got {:?}",
        after.status()
    );

    // Speech of a message would need Amazon Polly, which this emulator has no dependency
    // on, so it is reported as the modeled dependency failure rather than faked — and that
    // refusal creates no session either.
    let spoken = lex
        .put_session(
            "accept-audio",
            Some("audio/mpeg"),
            &json!({
                "dialogAction": {
                    "type": "Close",
                    "intentName": "OrderFlowers",
                    "fulfillmentState": "Fulfilled",
                    "message": "Your order has been placed"
                }
            }),
        )
        .await;
    assert_eq!(
        spoken.status(),
        424,
        "case accept=audio/mpeg with a message: expected 424, got {:?} / {:?}",
        spoken.status(),
        body_json(&spoken)
    );
    assert_eq!(
        error_code(&spoken),
        "DependencyFailedException",
        "case accept=audio/mpeg with a message: expected DependencyFailedException, got {:?}",
        error_code(&spoken)
    );
    let after_audio = lex.get_session("accept-audio", None).await;
    assert_eq!(
        after_audio.status(),
        404,
        "case accept=audio/mpeg with a message: a refused PutSession must create no \
         session, got {:?}",
        after_audio.status()
    );
}

#[tokio::test]
async fn a_message_is_returned_as_text_and_base64() {
    let lex = Lex::new();
    let response = lex
        .put_session(
            "message-user",
            None,
            &json!({
                "dialogAction": {
                    "type": "Close",
                    "intentName": "OrderFlowers",
                    "fulfillmentState": "Fulfilled",
                    "message": "Your order has been placed"
                }
            }),
        )
        .await;
    assert_eq!(
        response.status(),
        200,
        "PutSession: expected 200, got {:?} / {:?}",
        response.status(),
        body_json(&response)
    );
    assert_eq!(
        header(&response, "x-amz-lex-message"),
        "Your order has been placed",
        "expected the message that was set, got {:?}",
        header(&response, "x-amz-lex-message")
    );
    assert_eq!(
        header(&response, "x-amz-lex-encoded-message"),
        "WW91ciBvcmRlciBoYXMgYmVlbiBwbGFjZWQ=",
        "expected the base-64 encoding of that message, got {:?}",
        header(&response, "x-amz-lex-encoded-message")
    );
    assert_eq!(
        header(&response, "x-amz-lex-message-format"),
        "PlainText",
        "a message with no messageFormat is plain UTF-8 text, got {:?}",
        header(&response, "x-amz-lex-message-format")
    );
}

#[tokio::test]
async fn active_contexts_live_for_the_turns_their_time_to_live_asks_for() {
    // `turnsToLive` is "the number of conversation turns that the context should be
    // active. A conversation turn is one PostContent or PostText request and the
    // corresponding response", so a context set with N turns survives exactly N turns.
    for turns in [1_i64, 2, 3] {
        let lex = Lex::new();
        let put = lex
            .put_session(
                "context-user",
                None,
                &json!({
                    "activeContexts": [{
                        "name": "Ordering",
                        "timeToLive": { "turnsToLive": turns },
                        "parameters": { "flower": "roses" }
                    }]
                }),
            )
            .await;
        assert_eq!(
            put.status(),
            200,
            "case turnsToLive={turns}: PutSession expected 200, got {:?} / {:?}",
            put.status(),
            body_json(&put)
        );

        for turn in 1..=turns {
            let text = lex.post_text("context-user", &json!({ "inputText": "roses" })).await;
            let contexts = body_json(&text)["activeContexts"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                contexts.len(),
                1,
                "case turnsToLive={turns}: the context must still be active on turn {turn}, got {contexts:?}"
            );
            assert_eq!(
                contexts[0]["timeToLive"]["turnsToLive"],
                json!(turns),
                "case turnsToLive={turns}: the configured turnsToLive must be reported unchanged, got {contexts:?}"
            );
            assert_eq!(
                contexts[0]["parameters"],
                json!({ "flower": "roses" }),
                "case turnsToLive={turns}: expected the stored parameters, got {contexts:?}"
            );
        }

        let after = lex.get_session("context-user", None).await;
        let remaining = body_json(&after)["activeContexts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            remaining.is_empty(),
            "case turnsToLive={turns}: the context must be gone after {turns} turns, got {remaining:?}"
        );
    }
}

#[tokio::test]
async fn an_empty_active_context_list_clears_the_session_contexts() {
    let lex = Lex::new();
    let set = lex
        .put_session(
            "clear-user",
            None,
            &json!({
                "activeContexts": [{
                    "name": "Ordering",
                    "timeToLive": { "timeToLiveInSeconds": 300 },
                    "parameters": {}
                }]
            }),
        )
        .await;
    assert_eq!(set.status(), 200, "PutSession: expected 200, got {:?}", set.status());

    // "If you don't specify a list of contexts, Amazon Lex will use the current list of
    // contexts for the session."
    let kept = lex.put_session("clear-user", None, &json!({})).await;
    let kept_contexts: Value =
        serde_json::from_str(&header(&kept, "x-amz-lex-active-contexts")).unwrap_or(Value::Null);
    assert_eq!(
        kept_contexts.as_array().map(Vec::len),
        Some(1),
        "an absent activeContexts must leave the session's contexts alone, got {kept_contexts:?}"
    );

    // "If you specify an empty list, all contexts for the session are cleared."
    let cleared = lex
        .put_session("clear-user", None, &json!({ "activeContexts": [] }))
        .await;
    let cleared_contexts: Value =
        serde_json::from_str(&header(&cleared, "x-amz-lex-active-contexts")).unwrap_or(Value::Null);
    assert_eq!(
        cleared_contexts,
        json!([]),
        "an empty activeContexts must clear the session's contexts, got {cleared_contexts:?}"
    );
}

#[tokio::test]
async fn modeled_constraints_are_refused_as_bad_request() {
    // Each row is a value at, or one step past, a bound the model declares; a refusal is
    // HTTP 400 `BadRequestException`.
    struct Case {
        name: &'static str,
        method: &'static str,
        path: String,
        body: Option<Value>,
        expected: u16,
    }

    let two = "ab"; // UserId len 2..100: the published minimum
    let hundred = "u".repeat(100); // the published maximum
    let one = "a"; // one past the minimum
    let hundred_one = "u".repeat(101); // one past the maximum
    let longest_text = "x".repeat(1024); // Text len 1..1024: the published maximum
    let too_long_text = "x".repeat(1025);
    let longest_label = "a".repeat(255); // IntentSummaryCheckpointLabel len 1..255
    let too_long_label = "a".repeat(256);

    let summary = json!({ "intentName": "OrderFlowers", "dialogActionType": "ElicitIntent" });
    let context = json!({
        "name": "Ordering",
        "timeToLive": { "timeToLiveInSeconds": 300 },
        "parameters": {}
    });

    let cases: Vec<Case> = vec![
        Case {
            name: "userId at the minimum length",
            method: "POST",
            path: format!("/bot/OrderFlowers/alias/prod/user/{two}/session"),
            body: Some(json!({})),
            expected: 200,
        },
        Case {
            name: "userId at the maximum length",
            method: "POST",
            path: format!("/bot/OrderFlowers/alias/prod/user/{hundred}/session"),
            body: Some(json!({})),
            expected: 200,
        },
        Case {
            name: "userId one below the minimum length",
            method: "POST",
            path: format!("/bot/OrderFlowers/alias/prod/user/{one}/session"),
            body: Some(json!({})),
            expected: 400,
        },
        Case {
            name: "userId one above the maximum length",
            method: "POST",
            path: format!("/bot/OrderFlowers/alias/prod/user/{hundred_one}/session"),
            body: Some(json!({})),
            expected: 400,
        },
        Case {
            name: "userId outside its pattern",
            method: "POST",
            path: "/bot/OrderFlowers/alias/prod/user/bad!user/session".to_string(),
            body: Some(json!({})),
            expected: 400,
        },
        Case {
            name: "inputText at the maximum length",
            method: "POST",
            path: "/bot/OrderFlowers/alias/prod/user/bounds/text".to_string(),
            body: Some(json!({ "inputText": longest_text })),
            expected: 200,
        },
        Case {
            name: "inputText one above the maximum length",
            method: "POST",
            path: "/bot/OrderFlowers/alias/prod/user/bounds/text".to_string(),
            body: Some(json!({ "inputText": too_long_text })),
            expected: 400,
        },
        Case {
            name: "inputText below the minimum length",
            method: "POST",
            path: "/bot/OrderFlowers/alias/prod/user/bounds/text".to_string(),
            body: Some(json!({ "inputText": "" })),
            expected: 400,
        },
        Case {
            name: "inputText absent",
            method: "POST",
            path: "/bot/OrderFlowers/alias/prod/user/bounds/text".to_string(),
            body: Some(json!({})),
            expected: 400,
        },
        Case {
            name: "recentIntentSummaryView at the maximum length",
            method: "POST",
            path: "/bot/OrderFlowers/alias/prod/user/bounds/session".to_string(),
            body: Some(json!({ "recentIntentSummaryView": [summary, summary, summary] })),
            expected: 200,
        },
        Case {
            name: "recentIntentSummaryView one above the maximum length",
            method: "POST",
            path: "/bot/OrderFlowers/alias/prod/user/bounds/session".to_string(),
            body: Some(json!({ "recentIntentSummaryView": [summary, summary, summary, summary] })),
            expected: 400,
        },
        Case {
            name: "activeContexts one above the maximum length",
            method: "POST",
            path: "/bot/OrderFlowers/alias/prod/user/bounds/session".to_string(),
            body: Some(json!({ "activeContexts": vec![context.clone(); 21] })),
            expected: 400,
        },
        Case {
            name: "timeToLiveInSeconds at the published minimum",
            method: "POST",
            path: "/bot/OrderFlowers/alias/prod/user/bounds/session".to_string(),
            body: Some(json!({ "activeContexts": [{
                "name": "Ordering",
                "timeToLive": { "timeToLiveInSeconds": 5 },
                "parameters": {}
            }] })),
            expected: 200,
        },
        Case {
            name: "timeToLiveInSeconds one below the published minimum",
            method: "POST",
            path: "/bot/OrderFlowers/alias/prod/user/bounds/session".to_string(),
            body: Some(json!({ "activeContexts": [{
                "name": "Ordering",
                "timeToLive": { "timeToLiveInSeconds": 4 },
                "parameters": {}
            }] })),
            expected: 400,
        },
        Case {
            name: "timeToLiveInSeconds one above the published maximum",
            method: "POST",
            path: "/bot/OrderFlowers/alias/prod/user/bounds/session".to_string(),
            body: Some(json!({ "activeContexts": [{
                "name": "Ordering",
                "timeToLive": { "timeToLiveInSeconds": 86_401 },
                "parameters": {}
            }] })),
            expected: 400,
        },
        Case {
            name: "checkpointLabelFilter at the maximum length",
            method: "GET",
            path: format!(
                "/bot/OrderFlowers/alias/prod/user/bounds/session?checkpointLabelFilter={longest_label}"
            ),
            body: None,
            expected: 200,
        },
        Case {
            name: "checkpointLabelFilter one above the maximum length",
            method: "GET",
            path: format!(
                "/bot/OrderFlowers/alias/prod/user/bounds/session?checkpointLabelFilter={too_long_label}"
            ),
            body: None,
            expected: 400,
        },
        Case {
            name: "checkpointLabelFilter outside its pattern",
            method: "GET",
            path: "/bot/OrderFlowers/alias/prod/user/bounds/session?checkpointLabelFilter=bad_label"
                .to_string(),
            body: None,
            expected: 400,
        },
    ];

    let lex = Lex::new();
    // The `bounds` user has a session, so a 400 on its rows cannot be a 404 in disguise.
    let setup = lex.put_session("bounds", None, &json!({})).await;
    assert_eq!(
        setup.status(),
        200,
        "setup PutSession expected 200, got {:?}",
        setup.status()
    );

    for case in &cases {
        let response = lex
            .send(case.method, &case.path, None, case.body.as_ref())
            .await;
        assert_eq!(
            response.status().as_u16(),
            case.expected,
            "case {}: expected HTTP {}, got {:?} / {:?}",
            case.name,
            case.expected,
            response.status(),
            body_json(&response)
        );
        if case.expected == 400 {
            assert_eq!(
                error_code(&response),
                "BadRequestException",
                "case {}: expected BadRequestException, got {:?}",
                case.name,
                error_code(&response)
            );
        }
    }
}

#[tokio::test]
async fn the_session_attribute_bound_is_enforced_on_both_writers() {
    // `sessionAttributes` carries no modeled bound, so this store declares one
    // (`MAX_SESSION_ATTRIBUTES`) and both writers enforce it before changing anything.
    let at_bound: Value = (0..MAX_SESSION_ATTRIBUTES)
        .map(|index| (format!("key-{index}"), json!("v")))
        .collect::<serde_json::Map<_, _>>()
        .into();
    let past_bound: Value = (0..=MAX_SESSION_ATTRIBUTES)
        .map(|index| (format!("key-{index}"), json!("v")))
        .collect::<serde_json::Map<_, _>>()
        .into();

    for (case, attributes, expected) in [
        ("at the bound", at_bound.clone(), 200_u16),
        ("one past the bound", past_bound.clone(), 429),
    ] {
        let lex = Lex::new();
        let put = lex
            .put_session(
                "attribute-user",
                None,
                &json!({ "sessionAttributes": attributes }),
            )
            .await;
        assert_eq!(
            put.status().as_u16(),
            expected,
            "case PutSession {case}: expected HTTP {expected}, got {:?} / {:?}",
            put.status(),
            body_json(&put)
        );
        if expected == 429 {
            assert_eq!(
                error_code(&put),
                "LimitExceededException",
                "case PutSession {case}: expected LimitExceededException, got {:?}",
                error_code(&put)
            );
            assert_eq!(
                lex.stored_sessions(),
                0,
                "case PutSession {case}: a refused PutSession must create no session, got {} \
                 sessions",
                lex.stored_sessions()
            );
        }
    }

    // The merged map is what is bounded, so a turn that would push a session past the
    // bound is refused and the stored map is left as it was.
    let lex = Lex::new();
    let put = lex
        .put_session(
            "attribute-user",
            None,
            &json!({ "sessionAttributes": at_bound }),
        )
        .await;
    assert_eq!(
        put.status(),
        200,
        "setup PutSession at the bound: expected 200, got {:?} / {:?}",
        put.status(),
        body_json(&put)
    );

    let overflow = lex
        .post_text(
            "attribute-user",
            &json!({ "inputText": "roses", "sessionAttributes": { "one-too-many": "v" } }),
        )
        .await;
    assert_eq!(
        overflow.status(),
        429,
        "case PostText past the bound: expected 429, got {:?} / {:?}",
        overflow.status(),
        body_json(&overflow)
    );
    assert_eq!(
        error_code(&overflow),
        "LimitExceededException",
        "case PostText past the bound: expected LimitExceededException, got {:?}",
        error_code(&overflow)
    );
    let after = lex.get_session("attribute-user", None).await;
    assert_eq!(
        body_json(&after)["sessionAttributes"]
            .as_object()
            .map(serde_json::Map::len),
        Some(MAX_SESSION_ATTRIBUTES),
        "case PostText past the bound: the stored attributes must be unchanged, got {:?}",
        body_json(&after)["sessionAttributes"]
    );

    // A turn that replaces an existing key stays within the bound and is served.
    let replaced = lex
        .post_text(
            "attribute-user",
            &json!({ "inputText": "roses", "sessionAttributes": { "key-0": "changed" } }),
        )
        .await;
    assert_eq!(
        replaced.status(),
        200,
        "case PostText replacing a key: expected 200, got {:?} / {:?}",
        replaced.status(),
        body_json(&replaced)
    );
    assert_eq!(
        body_json(&replaced)["sessionAttributes"]["key-0"],
        json!("changed"),
        "case PostText replacing a key: expected the new value, got {:?}",
        body_json(&replaced)["sessionAttributes"]["key-0"]
    );
}

#[tokio::test]
async fn the_session_store_never_grows_past_its_declared_cap() {
    // Sessions are created by request, keyed by values the caller chooses, so the store
    // declares a ceiling (`MAX_SESSIONS`) and makes room by giving up the least recently
    // updated conversation rather than growing without bound.
    let lex = Lex::new();
    let overshoot = 8;
    for index in 0..MAX_SESSIONS + overshoot {
        let user = format!("cap-user-{index}");
        let response = lex.put_session(&user, None, &json!({})).await;
        assert_eq!(
            response.status(),
            200,
            "case session {index}: expected 200, got {:?} / {:?}",
            response.status(),
            body_json(&response)
        );
        assert!(
            lex.stored_sessions() <= MAX_SESSIONS,
            "case session {index}: the store holds {} sessions, past the {MAX_SESSIONS} cap",
            lex.stored_sessions()
        );
    }
    assert_eq!(
        lex.stored_sessions(),
        MAX_SESSIONS,
        "after {} creations the store must sit at its cap, got {} sessions",
        MAX_SESSIONS + overshoot,
        lex.stored_sessions()
    );

    // The conversations given up are the stalest ones; the most recent are still there.
    let newest = lex
        .get_session(&format!("cap-user-{}", MAX_SESSIONS + overshoot - 1), None)
        .await;
    assert_eq!(
        newest.status(),
        200,
        "the most recently updated session must survive the cap, got {:?}",
        newest.status()
    );
    let stalest = lex.get_session("cap-user-0", None).await;
    assert_eq!(
        stalest.status(),
        404,
        "the least recently updated session must have been given up, got {:?}",
        stalest.status()
    );
}

#[tokio::test]
async fn deleting_an_absent_session_is_not_found() {
    let lex = Lex::new();
    let response = lex.delete_session("never-seen").await;
    assert_eq!(
        response.status(),
        404,
        "DeleteSession(absent): expected 404, got {:?} / {:?}",
        response.status(),
        body_json(&response)
    );
    assert_eq!(
        error_code(&response),
        "NotFoundException",
        "DeleteSession(absent): expected NotFoundException, got {:?}",
        error_code(&response)
    );
}
