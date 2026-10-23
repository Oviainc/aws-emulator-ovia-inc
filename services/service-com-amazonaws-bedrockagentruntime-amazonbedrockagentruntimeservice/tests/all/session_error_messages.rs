// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusals of its session slice, asserted end-to-end over
//! the wire.
//!
//! Every case drives a real `restJson1` request through the `MultiServiceRouter` the
//! production emulator serves with (the harness in [`crate::sessions`]), so what is
//! pinned here is the status, the error code and the `message` a Bedrock Agent Runtime
//! client reads — not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than on one
//! example:
//!
//! 1. A session-scoped operation whose `sessionIdentifier` names no stored session
//!    answers one template built from the request — `Session with ID
//!    <sessionIdentifier> not found` — identically for every such operation and for
//!    either identifier form.
//! 2. A WRITE to a session that is no longer `ACTIVE` answers one terminal-state
//!    sentence naming that session's id, identically for every write.
//! 3. `DeleteSession` on a session that is still `ACTIVE` answers one CONSTANT
//!    sentence, interpolating nothing, and deletes nothing.
//! 4. A `@pattern` constraint violation quotes the expression a value has to satisfy
//!    WITHOUT the `^`/`$` a model writes to demand a whole-string match (and with the
//!    alternation of such an expression kept grouped, since dropping the anchors from
//!    `^A|B$` would otherwise change what it matches); every other violation class
//!    keeps the framework's own wording.

use serde_json::{json, Value};

use crate::sessions::{
    call, create_invocation, create_session, error_code, label, put_step, router, ACCOUNT,
};

/// The status and error code of each refusal class, from the model:
/// `smithy_query(query="operation",operation="GetSession,DeleteSession,CreateInvocation")`
/// -> `ResourceNotFoundException` httpError 404, `ConflictException` 409,
/// `ValidationException` 400.
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");
const CONFLICT: (u16, &str) = (409, "ConflictException");
const INVALID: (u16, &str) = (400, "ValidationException");

/// A UUID no test ever creates, used wherever a row must name something absent.
const ABSENT: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// A second absent UUID, so a row can vary the id it addresses.
const ABSENT_TOO: &str = "1a400015-c2b4-45b9-996e-1953e0e39268";

/// A 12-digit account that is NOT the one this emulator presents (`crate::sessions`'
/// `ACCOUNT`, i.e. `engine::EMULATOR_ACCOUNT_ID`): an ARN for another account names
/// nothing here.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// The `@pattern` of `SessionIdentifier` as a refusal must state it: the model's
/// expression with the anchors removed, and the top-level alternation kept grouped so the
/// quoted expression still matches what the anchored one matched. Source:
/// `smithy_query(query="shape",shape="SessionIdentifier")` ->
/// `^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]+:[0-9]{12}:session/[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})|([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})$`.
const SESSION_IDENTIFIER_PATTERN: &str = "((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]+:[0-9]{12}:session/[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})|([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}))";

/// The `@pattern` of `InvocationIdentifier` and of `Uuid` as a refusal must state it: one
/// alternative only, so no group is added. Source:
/// `smithy_query(query="shape",shape="InvocationIdentifier")` ->
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
const UUID_PATTERN: &str = "[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}";

/// The `message` member of an error body, or `""` when there is none.
fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or("")
}

/// RULE 1: the template a session-scoped operation states for an unknown session. It is
/// built from the REQUEST — the identifier as submitted, whichever form it took — and
/// carries no trailing period.
fn session_not_found(identifier: &str) -> String {
    format!("Session with ID {identifier} not found")
}

/// RULE 2: the ONE terminal-state sentence every write to a non-`ACTIVE` session states,
/// naming the id of the session addressed and no particular status.
fn terminal_state(session_id: &str) -> String {
    format!("Session with id {session_id} is in a terminal state and cannot be updated anymore.")
}

/// RULE 3: the constant `DeleteSession` states while the session is still `ACTIVE`.
const ACTIVE_NOT_DELETABLE: &str = "Cannot delete session in ACTIVE state";

/// RULE 4: the one clause a constraint violation contributes, in the framework's own
/// shape, with `requirement` as the constraint half.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// RULE 4: the `@pattern` requirement, quoting `pattern` as the caller must see it.
fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// RULE 4: the single-violation envelope.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// One request shape for a session-scoped operation, as a function of the
/// `sessionIdentifier` it addresses: the path tail after `/sessions/<identifier>` and the
/// body the operation needs to get as far as resolving that identifier.
struct SessionCall {
    operation: &'static str,
    method: &'static str,
    tail: String,
    body: Value,
}

/// Every operation that resolves a `sessionIdentifier`
/// (`smithy_query(query="digest")` -> the nine `/sessions/...` routes), each with a body
/// that satisfies its own required members, so the only thing a row can fail on is the
/// session lookup.
fn session_scoped_calls(invocation_id: &str, step_id: &str) -> Vec<SessionCall> {
    vec![
        SessionCall {
            operation: "GetSession",
            method: "GET",
            tail: "/".to_string(),
            body: Value::Null,
        },
        SessionCall {
            operation: "UpdateSession",
            method: "PUT",
            tail: "/".to_string(),
            body: json!({ "sessionMetadata": { "k": "v" } }),
        },
        SessionCall {
            operation: "EndSession",
            method: "PATCH",
            tail: String::new(),
            body: Value::Null,
        },
        SessionCall {
            operation: "DeleteSession",
            method: "DELETE",
            tail: "/".to_string(),
            body: Value::Null,
        },
        SessionCall {
            operation: "CreateInvocation",
            method: "PUT",
            tail: "/invocations/".to_string(),
            body: json!({}),
        },
        SessionCall {
            operation: "PutInvocationStep",
            method: "PUT",
            tail: "/invocationSteps/".to_string(),
            body: json!({
                "invocationIdentifier": invocation_id,
                "invocationStepTime": 1_783_501_811,
                "payload": { "contentBlocks": [{ "text": "x" }] },
            }),
        },
        SessionCall {
            operation: "GetInvocationStep",
            method: "POST",
            tail: format!("/invocationSteps/{step_id}"),
            body: json!({ "invocationIdentifier": invocation_id }),
        },
        SessionCall {
            operation: "ListInvocationSteps",
            method: "POST",
            tail: "/invocationSteps/".to_string(),
            body: json!({}),
        },
        SessionCall {
            operation: "ListInvocations",
            method: "POST",
            tail: "/invocations/".to_string(),
            body: json!({}),
        },
    ]
}

/// RULE 1, over every operation that resolves a `sessionIdentifier` and every identifier
/// form the member admits: the refusal is 404 `ResourceNotFoundException` stating
/// `Session with ID <sessionIdentifier> not found`, with the identifier exactly as
/// submitted.
#[tokio::test]
async fn an_unknown_session_is_refused_with_one_not_found_template_everywhere() {
    let router = router();
    // A real session, so the invocation and step ids the rows carry are well-formed
    // values the handler could have resolved had the session existed.
    let (session_id, _) = create_session(&router, Value::Null).await;
    let invocation_id = create_invocation(&router, &session_id, json!({})).await;
    let step_id = put_step(
        &router,
        &session_id,
        &invocation_id,
        1_783_501_811,
        &json!({ "contentBlocks": [{ "text": "only" }] }),
    )
    .await;

    // Each row is an identifier naming no stored session: the two forms the member
    // admits (a bare UUID and a session ARN), plus the ARN forms that cannot resolve
    // here — another account's and another service's.
    let identifiers: Vec<(&str, String)> = vec![
        ("a bare UUID", ABSENT.to_string()),
        ("a second bare UUID", ABSENT_TOO.to_string()),
        (
            "this emulator's own ARN for an unknown session",
            format!("arn:aws:bedrock:us-west-2:{ACCOUNT}:session/{ABSENT}"),
        ),
        (
            "a stored session's id under another account's ARN",
            format!("arn:aws:bedrock:us-west-2:{FOREIGN_ACCOUNT}:session/{session_id}"),
        ),
        (
            "an unknown session under a non-default partition's ARN",
            format!("arn:aws-us-gov:bedrock:us-gov-west-1:{ACCOUNT}:session/{ABSENT_TOO}"),
        ),
    ];

    for (form, identifier) in &identifiers {
        for request in session_scoped_calls(&invocation_id, &step_id) {
            let case = format!("{} addressed by {form}", request.operation);
            let path = format!("/sessions/{}{}", label(identifier), request.tail);
            let (status, refused) = call(&router, request.method, &path, request.body.clone()).await;
            assert_eq!(
                (status, error_code(&refused)),
                NOT_FOUND,
                "case {case:?}: expected {NOT_FOUND:?}, got {status} {refused:?}"
            );
            let expected = session_not_found(identifier);
            assert_eq!(
                message(&refused),
                expected,
                "case {case:?}: expected message {expected:?}, got {:?}",
                message(&refused)
            );
        }
    }
}

/// RULE 2: every write to a session that is no longer `ACTIVE` states the ONE
/// terminal-state sentence, naming the session's id. The rows span each write and both
/// identifier forms.
///
/// `ENDED` is the only terminal status a caller can reach: `EXPIRED` is modelled but no
/// code path mints it (see `state::SessionStatus`), so there is no request that produces
/// it to assert on.
#[tokio::test]
async fn a_write_to_a_terminal_session_states_one_terminal_state_sentence() {
    let router = router();
    let (session_id, session_arn) = create_session(&router, json!({ "topic": "demo" })).await;
    let invocation_id = create_invocation(&router, &session_id, json!({})).await;
    let (status, ended) = call(
        &router,
        "PATCH",
        &format!("/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, ended["sessionStatus"].as_str()),
        (200, Some("ENDED")),
        "EndSession: expected 200 ENDED, got {status} {ended:?}"
    );

    // (operation, method, the path tail, the body)
    let writes: Vec<(&str, &str, &str, Value)> = vec![
        ("CreateInvocation", "PUT", "/invocations/", json!({})),
        (
            "CreateInvocation with an explicit invocationId",
            "PUT",
            "/invocations/",
            json!({ "invocationId": ABSENT }),
        ),
        (
            "PutInvocationStep",
            "PUT",
            "/invocationSteps/",
            json!({
                "invocationIdentifier": invocation_id,
                "invocationStepTime": 1_783_501_812,
                "payload": { "contentBlocks": [{ "text": "after the end" }] },
            }),
        ),
        (
            "UpdateSession",
            "PUT",
            "/",
            json!({ "sessionMetadata": { "topic": "changed" } }),
        ),
        ("UpdateSession with an empty map", "PUT", "/", json!({})),
    ];

    for (form, identifier) in [("its id", &session_id), ("its ARN", &session_arn)] {
        for (operation, method, tail, body) in &writes {
            let case = format!("{operation} on an ENDED session addressed by {form}");
            let path = format!("/sessions/{}{tail}", label(identifier));
            let (status, refused) = call(&router, method, &path, body.clone()).await;
            assert_eq!(
                (status, error_code(&refused)),
                CONFLICT,
                "case {case:?}: expected {CONFLICT:?}, got {status} {refused:?}"
            );
            let expected = terminal_state(&session_id);
            assert_eq!(
                message(&refused),
                expected,
                "case {case:?}: expected message {expected:?}, got {:?}",
                message(&refused)
            );
        }
    }
}

/// RULE 3: `DeleteSession` on a session that is still `ACTIVE` states one constant
/// sentence — the same text whichever session was addressed, however it was addressed and
/// whatever it holds — and the session is still there afterwards.
#[tokio::test]
async fn deleting_an_active_session_states_one_constant_refusal_and_deletes_nothing() {
    // (case, the session's metadata, whether the session holds an invocation and a step,
    // whether the request addresses it by ARN rather than by id)
    let cases: [(&str, Value, bool, bool); 4] = [
        ("an empty session, by id", Value::Null, false, false),
        (
            "a session with metadata, by id",
            json!({ "topic": "demo" }),
            false,
            false,
        ),
        (
            "a session holding an invocation and a step, by id",
            json!({ "topic": "demo" }),
            true,
            false,
        ),
        (
            "a session holding an invocation and a step, by ARN",
            Value::Null,
            true,
            true,
        ),
    ];

    for (case, metadata, populated, by_arn) in cases {
        let router = router();
        let (session_id, session_arn) = create_session(&router, metadata).await;
        if populated {
            let invocation_id = create_invocation(&router, &session_id, json!({})).await;
            put_step(
                &router,
                &session_id,
                &invocation_id,
                1_783_501_811,
                &json!({ "contentBlocks": [{ "text": "held" }] }),
            )
            .await;
        }
        let identifier = if by_arn { &session_arn } else { &session_id };

        let (status, refused) = call(
            &router,
            "DELETE",
            &format!("/sessions/{}/", label(identifier)),
            Value::Null,
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            CONFLICT,
            "case {case:?}: expected {CONFLICT:?}, got {status} {refused:?}"
        );
        assert_eq!(
            message(&refused),
            ACTIVE_NOT_DELETABLE,
            "case {case:?}: expected message {ACTIVE_NOT_DELETABLE:?}, got {:?}",
            message(&refused)
        );

        // A refusal is not a deletion: the session is still readable and still ACTIVE.
        let (status, session) = call(
            &router,
            "GET",
            &format!("/sessions/{session_id}/"),
            Value::Null,
        )
        .await;
        assert_eq!(
            (status, session["sessionStatus"].as_str()),
            (200, Some("ACTIVE")),
            "case {case:?}: the refused session must still be ACTIVE, got {status} {session:?}"
        );
    }
}

/// RULE 4: a value outside a member's `@pattern` is refused with the expression the
/// caller has to satisfy — the model's expression without its anchors. The rows span both
/// patterned members this slice carries, several malformed forms of each, and the
/// operations that bind them.
#[tokio::test]
async fn a_pattern_refusal_quotes_the_expression_without_its_anchors() {
    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;
    let invocation_id = create_invocation(&router, &session_id, json!({})).await;

    // (case, method, path, body, the echoed value, the member path, the pattern quoted)
    let cases: Vec<(String, &str, String, Value, &str, &str, &str)> = vec![
        (
            "GetSession, a sessionIdentifier that is neither a UUID nor an ARN".to_string(),
            "GET",
            "/sessions/not-a-valid-session-id/".to_string(),
            Value::Null,
            "not-a-valid-session-id",
            "sessionIdentifier",
            SESSION_IDENTIFIER_PATTERN,
        ),
        (
            "GetSession, a sessionIdentifier whose hex groups are the wrong length".to_string(),
            "GET",
            "/sessions/1a400015-c2b4-45b9-996e/".to_string(),
            Value::Null,
            "1a400015-c2b4-45b9-996e",
            "sessionIdentifier",
            SESSION_IDENTIFIER_PATTERN,
        ),
        (
            "GetSession, a sessionIdentifier in upper-case hex".to_string(),
            "GET",
            "/sessions/1A400015-C2B4-45B9-996E-1953E0E39268/".to_string(),
            Value::Null,
            "1A400015-C2B4-45B9-996E-1953E0E39268",
            "sessionIdentifier",
            SESSION_IDENTIFIER_PATTERN,
        ),
        (
            "DeleteSession, a malformed sessionIdentifier".to_string(),
            "DELETE",
            "/sessions/session-42/".to_string(),
            Value::Null,
            "session-42",
            "sessionIdentifier",
            SESSION_IDENTIFIER_PATTERN,
        ),
        (
            "EndSession, a malformed sessionIdentifier".to_string(),
            "PATCH",
            "/sessions/session-42".to_string(),
            Value::Null,
            "session-42",
            "sessionIdentifier",
            SESSION_IDENTIFIER_PATTERN,
        ),
        (
            "UpdateSession, a malformed sessionIdentifier".to_string(),
            "PUT",
            "/sessions/session-42/".to_string(),
            json!({ "sessionMetadata": { "k": "v" } }),
            "session-42",
            "sessionIdentifier",
            SESSION_IDENTIFIER_PATTERN,
        ),
        (
            "CreateInvocation, a malformed sessionIdentifier".to_string(),
            "PUT",
            "/sessions/session-42/invocations/".to_string(),
            json!({}),
            "session-42",
            "sessionIdentifier",
            SESSION_IDENTIFIER_PATTERN,
        ),
        (
            "ListInvocations, a malformed sessionIdentifier".to_string(),
            "POST",
            "/sessions/session-42/invocations/".to_string(),
            json!({}),
            "session-42",
            "sessionIdentifier",
            SESSION_IDENTIFIER_PATTERN,
        ),
        (
            "ListInvocationSteps, a malformed invocationIdentifier".to_string(),
            "POST",
            format!("/sessions/{session_id}/invocationSteps/"),
            json!({ "invocationIdentifier": "not-a-valid-uuid" }),
            "not-a-valid-uuid",
            "invocationIdentifier",
            UUID_PATTERN,
        ),
        (
            "ListInvocationSteps, an invocationIdentifier in upper-case hex".to_string(),
            "POST",
            format!("/sessions/{session_id}/invocationSteps/"),
            json!({ "invocationIdentifier": "1A400015-C2B4-45B9-996E-1953E0E39268" }),
            "1A400015-C2B4-45B9-996E-1953E0E39268",
            "invocationIdentifier",
            UUID_PATTERN,
        ),
        (
            "PutInvocationStep, a malformed invocationIdentifier".to_string(),
            "PUT",
            format!("/sessions/{session_id}/invocationSteps/"),
            json!({
                "invocationIdentifier": "invocation-7",
                "invocationStepTime": 1_783_501_811,
                "payload": { "contentBlocks": [{ "text": "x" }] },
            }),
            "invocation-7",
            "invocationIdentifier",
            UUID_PATTERN,
        ),
        (
            "GetInvocationStep, a malformed invocationStepId".to_string(),
            "POST",
            format!("/sessions/{session_id}/invocationSteps/step-7"),
            json!({ "invocationIdentifier": invocation_id }),
            "step-7",
            "invocationStepId",
            UUID_PATTERN,
        ),
    ];

    for (case, method, path, body, value, path_member, pattern) in &cases {
        let (status, refused) = call(&router, method, path, body.clone()).await;
        assert_eq!(
            (status, error_code(&refused)),
            INVALID,
            "case {case:?}: expected {INVALID:?}, got {status} {refused:?}"
        );
        let stated = message(&refused);
        let expected = one_error(&clause(value, path_member, &pattern_requirement(pattern)));
        assert_eq!(
            stated, expected,
            "case {case:?}: expected message {expected:?}, got {stated:?}"
        );
        // The anchors specifically: a quoted expression must neither open with `^` nor
        // close with `$`. (A `^` INSIDE the expression is a negated character class —
        // `[^:]` — and belongs there, so the check is on the two ends only.)
        assert!(
            !stated.contains("pattern: ^") && !stated.ends_with('$'),
            "case {case:?}: the quoted expression must carry no anchors, got {stated:?}"
        );
    }
}

/// RULE 4, for the classes this service does NOT reword: a violation that is not a
/// `@pattern` failure keeps the framework's own clause, in the same envelope.
#[tokio::test]
async fn a_violation_that_is_not_a_pattern_failure_keeps_the_frameworks_wording() {
    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;

    // (case, path, body, the clause the refusal must state). The requirement half is the
    // framework's own wording for that violation class
    // (`aws_smithy_schema_server::constraints::Violation::requirement`), and the bounds
    // are the model's: `smithy_query(query="operation",operation="ListInvocationSteps")`
    // -> `maxResults` range {min: 1, max: 1000}; `nextToken` length 1..2048.
    let cases: Vec<(&str, String, Value, String)> = vec![
        (
            "maxResults below the published minimum",
            format!("/sessions/{session_id}/invocationSteps/?maxResults=0"),
            json!({}),
            clause(
                "0",
                "maxResults",
                "Member must have value greater than or equal to 1",
            ),
        ),
        (
            "maxResults above the published maximum",
            format!("/sessions/{session_id}/invocationSteps/?maxResults=1001"),
            json!({}),
            clause(
                "1001",
                "maxResults",
                "Member must have value less than or equal to 1000",
            ),
        ),
        (
            "an empty nextToken, below the published minimum length",
            format!("/sessions/{session_id}/invocationSteps/"),
            json!({ "nextToken": "" }),
            clause(
                "",
                "nextToken",
                "Member must have length greater than or equal to 1",
            ),
        ),
    ];

    for (case, path, body, expected_clause) in &cases {
        let (status, refused) = call(&router, "POST", path, body.clone()).await;
        assert_eq!(
            (status, error_code(&refused)),
            INVALID,
            "case {case:?}: expected {INVALID:?}, got {status} {refused:?}"
        );
        let expected = one_error(expected_clause);
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?}: expected message {expected:?}, got {:?}",
            message(&refused)
        );
    }
}

/// RULE 4, for a request that fails TWO constraints at once: the envelope counts the
/// clauses it carries, the `@pattern` one is still quoted without its anchors, and the
/// other keeps the framework's wording.
#[tokio::test]
async fn an_envelope_over_two_violations_rewords_only_the_pattern_clause() {
    let router = router();
    let (status, refused) = call(
        &router,
        "POST",
        "/sessions/not-a-valid-session-id/invocationSteps/?maxResults=0",
        json!({}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        INVALID,
        "two violations: expected {INVALID:?}, got {status} {refused:?}"
    );
    let stated = message(&refused);
    assert!(
        stated.starts_with("2 validation errors detected: "),
        "two violations: expected the two-error envelope, got {stated:?}"
    );
    let pattern_clause = clause(
        "not-a-valid-session-id",
        "sessionIdentifier",
        &pattern_requirement(SESSION_IDENTIFIER_PATTERN),
    );
    assert!(
        stated.contains(&pattern_clause),
        "two violations: expected the clause {pattern_clause:?}, got {stated:?}"
    );
    let range_clause = clause(
        "0",
        "maxResults",
        "Member must have value greater than or equal to 1",
    );
    assert!(
        stated.contains(&range_clause),
        "two violations: expected the clause {range_clause:?}, got {stated:?}"
    );
}
