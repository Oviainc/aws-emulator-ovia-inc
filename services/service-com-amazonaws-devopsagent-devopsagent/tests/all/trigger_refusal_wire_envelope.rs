// SPDX-License-Identifier: CC0-1.0

//! The WIRE envelope every refusal of `GetTrigger`
//! (GET `/trigger/agent-space/{agentSpaceId}/triggers/{triggerId}`) carries.
//!
//! `tests/all/trigger_refusal_arms.rs` pins what each arm decides; this suite pins what
//! a restJson1 client actually reads off the socket for that decision, as one triple per
//! row: the HTTP status, the `x-amzn-errortype` header the protocol resolves the error
//! SHAPE from (present on its own, not only as the body's `__type`), and the body's
//! `message`.
//!
//! The rules under test:
//!
//! 1. A refusal is answered with the status its modelled error declares
//!    (`ValidationException` 400, `ResourceNotFoundException` 404,
//!    `AccessDeniedException` 403), names that shape in `x-amzn-errortype`, and repeats
//!    the same shape in the body's `__type`.
//! 2. The message of each arm is the fixed text of its class, with only the variable
//!    parts the class states: the validated member (as a JSON pointer) and the pattern
//!    for the validation arm, the requested identifier for the missing-goal arm, and
//!    nothing at all for the fail-closed arm.
//! 3. A served read carries no error envelope: no `x-amzn-errortype` header and no
//!    `__type`, so the rows above are decisions and not a blanket refusal.
//!
//! The emulator, the fixtures and the class tables are the sibling suite's — `new_router`,
//! `create_space`, `minted_trigger_id`, the pinned refusal texts and
//! `malformed_identifiers`/`absent_goal_identifiers`/`absent_agent_space_ids` are imported
//! from it, so neither the texts nor the tables can drift between the two suites. What is
//! this suite's own is the raw response reader, which has to see the header and the body's
//! `__type` separately where the sibling folds them into one.
//!
//! Each arm is driven over the sibling's table for its class rather than over the one
//! recorded row: the malformed identifiers differ in prefix, separator, character class,
//! group count and group length (including each UUID group one digit short and one long);
//! the absent goal identifiers differ in digit case and value, with the identifier the
//! service itself minted in another space added here; and the absent agent-space
//! identifiers differ in length (both published bounds of `AgentSpaceId`), character class
//! and form.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::trigger_refusal_arms::{
    absent_agent_space_ids, absent_goal_identifiers, create_space, identifier_refusal,
    malformed_identifiers, minted_trigger_id, new_router, AUTHORIZATION, GOAL_ID_PATH,
    GOAL_ID_PATTERN,
};

/// The modelled error shapes of `GetTrigger`, with the status each declares, from
/// `smithy_query(query="operation", operation="GetTrigger")`.
const VALIDATION_EXCEPTION: (&str, u16) = ("ValidationException", 400);
const RESOURCE_NOT_FOUND_EXCEPTION: (&str, u16) = ("ResourceNotFoundException", 404);
const ACCESS_DENIED_EXCEPTION: (&str, u16) = ("AccessDeniedException", 403);

/// One answer exactly as it came off the wire.
///
/// `tests/all/trigger_refusal_arms.rs` folds the `x-amzn-errortype` header into the body's
/// `__type` so one accessor reads either; what this suite is about is the two of them
/// SEPARATELY, so it reads the response itself rather than reusing that reader.
struct Answer {
    status: u16,
    /// The `x-amzn-errortype` header verbatim, `None` when the answer carries none.
    error_type_header: Option<String>,
    body: Value,
}

impl Answer {
    /// The body's `__type` discriminator verbatim, `None` when the answer carries none.
    fn body_type(&self) -> Option<&str> {
        self.body.get("__type").and_then(Value::as_str)
    }

    /// The `message` of an error answer.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
    }
}

/// Send one request and return what came back.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "aidevops.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::new())
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type_header = response
        .headers()
        .get("x-amzn-errortype")
        .map(|value| {
            value
                .to_str()
                .expect("x-amzn-errortype is not valid text")
                .to_string()
        });
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        error_type_header,
        body,
    }
}

/// `GET .../triggers/{trigger_id}` in `space`, read as the raw wire answer.
async fn get_trigger(router: &MultiServiceRouter, space: &str, trigger_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/trigger/agent-space/{space}/triggers/{trigger_id}"),
    )
    .await
}

/// Assert the whole error envelope of one row: the status, the `x-amzn-errortype`
/// header, the body's `__type`, and the message.
fn assert_envelope(case: &str, answer: &Answer, shape: (&str, u16), message: &str) {
    let (name, status) = shape;
    assert_eq!(
        answer.status, status,
        "case {case}: expected HTTP {status}, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type_header.as_deref(),
        Some(name),
        "case {case}: expected the x-amzn-errortype header to name {name}, got {:?}",
        answer.error_type_header
    );
    assert_eq!(
        answer.body_type(),
        Some(name),
        "case {case}: expected the body's __type to name {name}, got {}",
        answer.body
    );
    assert_eq!(
        answer.message(),
        message,
        "case {case}: expected the message {message:?}, got {:?}",
        answer.message()
    );
}

#[tokio::test]
async fn a_malformed_identifier_answers_the_validation_envelope() {
    let router = new_router();
    let space = create_space(&router, "trigger-wire-validation").await;

    for case in malformed_identifiers() {
        let answer = get_trigger(&router, &space, &case).await;
        assert_envelope(
            &format!("GetTrigger {case:?}"),
            &answer,
            VALIDATION_EXCEPTION,
            &identifier_refusal(),
        );
        // The class states the member and the pattern only, so the rejected value is
        // never part of what goes back. (A row that is itself a substring of the pattern
        // text cannot be told apart from the pattern, so it is exempt — the equality
        // above already pins the whole message for it.)
        if !GOAL_ID_PATTERN.contains(case.as_str()) {
            assert!(
                !answer.message().contains(case.as_str()),
                "case {case:?}: the refusal must not echo the rejected value, got {:?}",
                answer.message()
            );
        }
        assert_eq!(
            answer.body["fieldList"][0]["path"].as_str(),
            Some(GOAL_ID_PATH),
            "case {case:?}: expected the field path {GOAL_ID_PATH}, got {}",
            answer.body
        );
    }
}

#[tokio::test]
async fn an_unknown_goal_identifier_answers_the_not_found_envelope() {
    let router = new_router();
    let space = create_space(&router, "trigger-wire-not-found").await;
    let other_space = create_space(&router, "trigger-wire-not-found-other").await;

    // A Trigger stored in ANOTHER space, so this arm is decided from what the addressed
    // space holds. Its identifier is the one the service minted, a value no literal
    // here supplied.
    let elsewhere = minted_trigger_id(&router, &other_space).await;

    let mut rows = absent_goal_identifiers();
    rows.push(elsewhere.clone());
    for case in rows {
        let answer = get_trigger(&router, &space, &case).await;
        assert_envelope(
            &format!("GetTrigger {case:?}"),
            &answer,
            RESOURCE_NOT_FOUND_EXCEPTION,
            &format!("Goal not found: {case}"),
        );
    }

    // The same identifier is served in the space that owns it, with no error envelope
    // at all.
    let served = get_trigger(&router, &other_space, &elsewhere).await;
    assert_eq!(
        served.status, 200,
        "case {elsewhere:?} in its own space: expected 200, got {} with {}",
        served.status, served.body
    );
    assert_eq!(
        served.error_type_header, None,
        "case {elsewhere:?} in its own space: a served read carries no error type, got {:?}",
        served.error_type_header
    );
    assert_eq!(
        served.body_type(),
        None,
        "case {elsewhere:?} in its own space: a served read carries no __type, got {}",
        served.body
    );
}

#[tokio::test]
async fn an_unresolvable_agent_space_answers_the_access_denied_envelope() {
    let router = new_router();
    let present = create_space(&router, "trigger-wire-fail-closed").await;
    let stored = minted_trigger_id(&router, &present).await;

    for space_id in absent_agent_space_ids() {
        assert_ne!(
            space_id, present,
            "the table must not name the stored AgentSpace"
        );
        // Both a Trigger that exists elsewhere and one that exists nowhere: inside an
        // unresolvable space the answer is the same, so it discloses nothing about what
        // the space holds or whether it exists.
        for trigger_id in [
            stored.as_str(),
            "goal-00000000-0000-0000-0000-000000000000",
        ] {
            let case = format!("GetTrigger {trigger_id:?} in absent space {space_id:?}");
            let answer = get_trigger(&router, &space_id, trigger_id).await;
            // The message is wholly fixed text, so pinning it IS the no-disclosure check:
            // an identifier in it could not satisfy this equality.
            assert_envelope(&case, &answer, ACCESS_DENIED_EXCEPTION, "Access denied");
        }
    }

    // The space that does exist is still served, so the arm is decided from the store.
    let served = get_trigger(&router, &present, &stored).await;
    assert_eq!(
        served.status, 200,
        "case {stored:?} in {present:?}: expected 200, got {} with {}",
        served.status, served.body
    );
}
