// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Bedrock Agent Runtime **session** slice: CreateSession,
//! CreateInvocation, PutInvocationStep, GetInvocationStep, ListInvocationSteps,
//! GetSession, UpdateSession, EndSession and DeleteSession.
//!
//! Every test drives real HTTP requests through the framework router, so the wire
//! protocol (restJson1), the URI bindings, the boundary validator and the handlers
//! are all exercised, and every assertion reads a response BODY.
//!
//! The request harness below ([`router`], [`call`], [`create_session`],
//! [`create_invocation`], [`put_step`] and the small readers beside them) is
//! `pub(crate)`, so a sibling suite in `tests/all/` drives the service exactly the
//! way this one does instead of growing a second copy of it (see
//! `tests/all/invocations.rs`).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here signs for. It becomes the region segment of the
/// `sessionArn` the service mints.
pub(crate) const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// A router carrying only this service, over one shared engine — the same object
/// the production router mounts, so state behaves exactly as it does in the
/// emulator.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Send one request and read back `(status, body)`. A `Value::Null` body is sent as
/// no body at all, the way an SDK sends an operation with no body members.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    call_signed_for(router, method, path, body, REGION).await
}

/// [`call`], with the region the request's SigV4 credential scope names — the value a
/// minted `sessionArn` takes its region segment from.
async fn call_signed_for(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Value,
    region: &str,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(serde_json::to_vec(&body).expect("the test body serializes"))
    };
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20261021/{region}/bedrock/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// The wire error code restJson1 puts in `__type`, or `""` on a success body.
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// A `date-time` timestamp member's value as epoch seconds.
///
/// Every `DateTimestamp` member of this service carries `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="InvocationStep")`), so its wire form is an RFC
/// 3339 string: a value that is not one reads as `None` and the assertion naming it
/// fails.
fn seconds(value: &Value) -> Option<f64> {
    let rendered = value.as_str()?;
    ::aws_smithy_types::DateTime::from_str(
        rendered,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()
    .map(|instant| instant.as_secs_f64())
}

/// `epoch_seconds` as the RFC 3339 `date-time` string a client sends for a
/// `@timestampFormat("date-time")` member.
fn date_time(epoch_seconds: i64) -> String {
    ::aws_smithy_types::DateTime::from_secs(epoch_seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an epoch-seconds instant renders as RFC 3339")
}

/// Percent-encode a `sessionIdentifier` for use as a non-greedy `@httpLabel`: a
/// session ARN contains `/`, which would otherwise split the path segment.
pub(crate) fn label(identifier: &str) -> String {
    identifier.replace('%', "%25").replace('/', "%2F")
}

/// Does `value` satisfy the model's `Uuid` pattern,
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`?
/// (`smithy_query(query="operation",operation="CreateSession")` -> `sessionId`.)
pub(crate) fn is_model_uuid(value: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let parts: Vec<&str> = value.split('-').collect();
    parts.len() == GROUPS.len()
        && parts.iter().zip(GROUPS).all(|(part, len)| {
            part.len() == len
                && part.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
}

/// Create a session and return `(sessionId, sessionArn)`.
pub(crate) async fn create_session(
    router: &MultiServiceRouter,
    metadata: Value,
) -> (String, String) {
    let body = if metadata.is_null() {
        json!({})
    } else {
        json!({ "sessionMetadata": metadata })
    };
    let (status, out) = call(router, "PUT", "/sessions/", body).await;
    assert_eq!(status, 201, "CreateSession: expected 201, got {status} {out:?}");
    (
        out["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateSession: no sessionId in {out:?}"))
            .to_string(),
        out["sessionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateSession: no sessionArn in {out:?}"))
            .to_string(),
    )
}

/// RULE: `sessionMetadata` is reported exactly as the writer supplied it, so an
/// absent member and a stored empty map stay the two different answers they are. The
/// rows span the member's presence and its content, through CreateSession and through
/// UpdateSession.
#[tokio::test]
async fn session_metadata_is_reported_exactly_as_it_was_written() {
    // (case, the CreateSession body, the `sessionMetadata` GetSession must report)
    let cases: Vec<(&str, Value, Value)> = vec![
        ("member omitted", json!({}), Value::Null),
        (
            "member present but empty",
            json!({ "sessionMetadata": {} }),
            json!({}),
        ),
        (
            "one pair",
            json!({ "sessionMetadata": { "topic": "demo" } }),
            json!({ "topic": "demo" }),
        ),
        (
            "several pairs, including an empty value",
            json!({ "sessionMetadata": { "a": "1", "b": "", "c": "3" } }),
            json!({ "a": "1", "b": "", "c": "3" }),
        ),
    ];

    for (case, create_body, expected) in &cases {
        let router = router();
        let (status, created) = call(&router, "PUT", "/sessions/", create_body.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateSession expected 201, got {status} {created:?}"
        );
        let session_id = created["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no sessionId in {created:?}"))
            .to_string();

        let (status, session) = call(
            &router,
            "GET",
            &format!("/sessions/{session_id}/"),
            Value::Null,
        )
        .await;
        assert_eq!(
            (status, session.get("sessionMetadata").unwrap_or(&Value::Null)),
            (200, expected),
            "case {case:?}: GetSession must report what CreateSession stored, got {session:?}"
        );

        // The same rule through UpdateSession: a supplied map replaces, an omitted
        // one leaves the stored value alone.
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/sessions/{session_id}/"),
            json!({ "sessionMetadata": {} }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateSession expected 200, got {status} {updated:?}"
        );
        let (status, session) = call(
            &router,
            "GET",
            &format!("/sessions/{session_id}/"),
            Value::Null,
        )
        .await;
        assert_eq!(
            (status, session.get("sessionMetadata").unwrap_or(&Value::Null)),
            (200, &json!({})),
            "case {case:?}: UpdateSession with an empty map must store an empty map, \
             got {session:?}"
        );

        // Replace the stored empty map with a non-empty one, so the no-op below is
        // asserted against a value no later call wrote.
        let (status, replaced) = call(
            &router,
            "PUT",
            &format!("/sessions/{session_id}/"),
            json!({ "sessionMetadata": { "after": "update" } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateSession expected 200, got {status} {replaced:?}"
        );
        let (status, session) = call(
            &router,
            "GET",
            &format!("/sessions/{session_id}/"),
            Value::Null,
        )
        .await;
        assert_eq!(
            (status, session.get("sessionMetadata").unwrap_or(&Value::Null)),
            (200, &json!({ "after": "update" })),
            "case {case:?}: UpdateSession with a map must replace the stored one, got \
             {session:?}"
        );

        let (status, unchanged) = call(
            &router,
            "PUT",
            &format!("/sessions/{session_id}/"),
            json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateSession with no metadata expected 200, got {status} \
             {unchanged:?}"
        );
        let (status, session) = call(
            &router,
            "GET",
            &format!("/sessions/{session_id}/"),
            Value::Null,
        )
        .await;
        assert_eq!(
            (status, session.get("sessionMetadata").unwrap_or(&Value::Null)),
            (200, &json!({ "after": "update" })),
            "case {case:?}: UpdateSession without the member must leave the stored map \
             alone, got {session:?}"
        );
    }
}

/// Create an invocation under `identifier` and return its invocationId.
pub(crate) async fn create_invocation(
    router: &MultiServiceRouter,
    identifier: &str,
    body: Value,
) -> String {
    let path = format!("/sessions/{}/invocations/", label(identifier));
    let (status, out) = call(router, "PUT", &path, body).await;
    assert_eq!(
        status, 201,
        "CreateInvocation({identifier}): expected 201, got {status} {out:?}"
    );
    out["invocationId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateInvocation: no invocationId in {out:?}"))
        .to_string()
}

/// Write one invocation step and return its invocationStepId.
///
/// `step_time` is epoch seconds, sent as the RFC 3339 `date-time` string the member's
/// `@timestampFormat("date-time")` requires.
pub(crate) async fn put_step(
    router: &MultiServiceRouter,
    identifier: &str,
    invocation_id: &str,
    step_time: i64,
    payload: &Value,
) -> String {
    let path = format!("/sessions/{}/invocationSteps/", label(identifier));
    let (status, out) = call(
        router,
        "PUT",
        &path,
        json!({
            "invocationIdentifier": invocation_id,
            "invocationStepTime": date_time(step_time),
            "payload": payload,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "PutInvocationStep({payload:?}): expected 201, got {status} {out:?}"
    );
    out["invocationStepId"]
        .as_str()
        .unwrap_or_else(|| panic!("PutInvocationStep: no invocationStepId in {out:?}"))
        .to_string()
}

/// The whole workflow the bootstrap issue describes, asserting response bodies at
/// every step rather than just the statuses.
#[tokio::test]
async fn the_session_workflow_runs_end_to_end_and_reports_persisted_state() {
    let router = router();

    // 1. CreateSession
    let (session_id, session_arn) = create_session(&router, json!({ "topic": "demo" })).await;
    assert!(
        is_model_uuid(&session_id),
        "CreateSession: sessionId {session_id:?} must match the model's Uuid pattern"
    );
    assert_eq!(
        session_arn,
        format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:session/{session_id}"),
        "CreateSession: sessionArn must be built from the minted sessionId, got {session_arn:?}"
    );

    // 2. CreateInvocation, by bare session UUID.
    let invocation_id =
        create_invocation(&router, &session_id, json!({ "description": "turn-1" })).await;
    assert!(
        is_model_uuid(&invocation_id),
        "CreateInvocation: invocationId {invocation_id:?} must match the model's Uuid pattern"
    );

    // 3. PutInvocationStep
    let payload = json!({ "contentBlocks": [{ "text": "hello" }] });
    let step_time: i64 = 1_792_617_532;
    let step_id = put_step(&router, &session_id, &invocation_id, step_time, &payload).await;
    assert!(
        is_model_uuid(&step_id),
        "PutInvocationStep: invocationStepId {step_id:?} must match the model's Uuid pattern"
    );

    // 4. GetInvocationStep reads the stored record back.
    let (status, got) = call(
        &router,
        "POST",
        &format!(
            "/sessions/{}/invocationSteps/{step_id}",
            label(&session_id)
        ),
        json!({ "invocationIdentifier": invocation_id }),
    )
    .await;
    assert_eq!(status, 200, "GetInvocationStep: expected 200, got {status} {got:?}");
    let step = &got["invocationStep"];
    assert_eq!(
        (
            step["sessionId"].as_str(),
            step["invocationId"].as_str(),
            step["invocationStepId"].as_str(),
            seconds(&step["invocationStepTime"]),
            &step["payload"],
        ),
        (
            Some(session_id.as_str()),
            Some(invocation_id.as_str()),
            Some(step_id.as_str()),
            Some(step_time as f64),
            &payload,
        ),
        "GetInvocationStep: the whole stored step must come back, got {got:?}"
    );

    // 5. ListInvocationSteps, the single step and no token.
    let (status, listed) = call(
        &router,
        "POST",
        &format!(
            "/sessions/{}/invocationSteps/?maxResults=1",
            label(&session_id)
        ),
        json!({ "invocationIdentifier": invocation_id }),
    )
    .await;
    assert_eq!(status, 200, "ListInvocationSteps: expected 200, got {status} {listed:?}");
    let summaries = listed["invocationStepSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("ListInvocationSteps: no summaries in {listed:?}"));
    assert_eq!(
        summaries.len(),
        1,
        "ListInvocationSteps: expected exactly the one stored step, got {listed:?}"
    );
    assert_eq!(
        (
            summaries[0]["sessionId"].as_str(),
            summaries[0]["invocationId"].as_str(),
            summaries[0]["invocationStepId"].as_str(),
            seconds(&summaries[0]["invocationStepTime"]),
        ),
        (
            Some(session_id.as_str()),
            Some(invocation_id.as_str()),
            Some(step_id.as_str()),
            Some(step_time as f64),
        ),
        "ListInvocationSteps: the summary must be built from state, got {listed:?}"
    );
    assert!(
        listed.get("nextToken").is_none_or(Value::is_null),
        "ListInvocationSteps: no nextToken is due when nothing remains, got {listed:?}"
    );

    // 6. GetSession, addressed by ARN, reports the metadata CreateSession stored.
    let (status, session) = call(
        &router,
        "GET",
        &format!("/sessions/{}/", label(&session_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "GetSession: expected 200, got {status} {session:?}");
    assert_eq!(
        session["sessionMetadata"],
        json!({ "topic": "demo" }),
        "GetSession: metadata must be exactly what CreateSession stored, got {session:?}"
    );
    assert_eq!(
        session["sessionStatus"], "ACTIVE",
        "GetSession: a new session is ACTIVE, got {session:?}"
    );
    assert!(
        seconds(&session["lastUpdatedAt"]).is_some(),
        "GetSession: lastUpdatedAt must be present as an RFC 3339 date-time, got {session:?}"
    );

    // 7. DeleteSession is refused while the session is ACTIVE.
    let (status, refused) = call(
        &router,
        "DELETE",
        &format!("/sessions/{}/", label(&session_id)),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (409, "ConflictException"),
        "DeleteSession on an ACTIVE session: expected 409 ConflictException, got {status} {refused:?}"
    );

    // 8. EndSession, then the delete goes through.
    let (status, ended) = call(
        &router,
        "PATCH",
        &format!("/sessions/{}", label(&session_id)),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "EndSession: expected 200, got {status} {ended:?}");
    assert_eq!(
        ended,
        json!({
            "sessionId": session_id,
            "sessionArn": session_arn,
            "sessionStatus": "ENDED",
        }),
        "EndSession: must report the stored session with status ENDED, got {ended:?}"
    );

    let (status, _) = call(
        &router,
        "DELETE",
        &format!("/sessions/{}/", label(&session_id)),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "DeleteSession on an ENDED session: expected 200, got {status}");

    // 9. Both the session and its steps are gone.
    let (status, gone) = call(
        &router,
        "GET",
        &format!("/sessions/{}/", label(&session_id)),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, error_code(&gone)),
        (404, "ResourceNotFoundException"),
        "GetSession after delete: expected 404 ResourceNotFoundException, got {status} {gone:?}"
    );
    let (status, gone) = call(
        &router,
        "POST",
        &format!(
            "/sessions/{}/invocationSteps/{step_id}",
            label(&session_id)
        ),
        json!({ "invocationIdentifier": invocation_id }),
    )
    .await;
    assert_eq!(
        (status, error_code(&gone)),
        (404, "ResourceNotFoundException"),
        "GetInvocationStep after delete: expected 404 ResourceNotFoundException, got {status} {gone:?}"
    );
}

/// RULE: every session-scoped operation resolves its `sessionIdentifier` through the
/// one shared helper, so a bare session UUID and the session's ARN name the same
/// session everywhere. The table rows differ in the FORM of the identifier; each
/// form is driven through every session-scoped operation.
#[tokio::test]
async fn both_identifier_forms_name_the_same_session_in_every_session_scoped_operation() {
    for form in ["bare-uuid", "session-arn"] {
        let router = router();
        let (session_id, session_arn) = create_session(&router, json!({ "form": form })).await;
        let identifier: &str = match form {
            "bare-uuid" => &session_id,
            _ => &session_arn,
        };

        let invocation_id = create_invocation(&router, identifier, json!({})).await;
        let payload = json!({ "contentBlocks": [{ "text": form }] });
        let step_id = put_step(
            &router,
            identifier,
            &invocation_id,
            1_767_322_045,
            &payload,
        )
        .await;

        let (status, got) = call(
            &router,
            "POST",
            &format!(
                "/sessions/{}/invocationSteps/{step_id}",
                label(identifier)
            ),
            json!({ "invocationIdentifier": invocation_id }),
        )
        .await;
        assert_eq!(
            (status, got["invocationStep"]["sessionId"].as_str()),
            (200, Some(session_id.as_str())),
            "case {form:?}: GetInvocationStep must resolve the identifier, got {status} {got:?}"
        );

        let (status, listed) = call(
            &router,
            "POST",
            &format!("/sessions/{}/invocationSteps/", label(identifier)),
            json!({}),
        )
        .await;
        assert_eq!(
            (status, listed["invocationStepSummaries"].as_array().map(Vec::len)),
            (200, Some(1)),
            "case {form:?}: ListInvocationSteps must resolve the identifier, got {status} {listed:?}"
        );

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/sessions/{}/", label(identifier)),
            json!({ "sessionMetadata": { "form": form, "stage": "updated" } }),
        )
        .await;
        assert_eq!(
            (status, updated["sessionId"].as_str()),
            (200, Some(session_id.as_str())),
            "case {form:?}: UpdateSession must resolve the identifier, got {status} {updated:?}"
        );

        let (status, session) = call(
            &router,
            "GET",
            &format!("/sessions/{}/", label(identifier)),
            Value::Null,
        )
        .await;
        assert_eq!(
            (status, &session["sessionMetadata"]),
            (200, &json!({ "form": form, "stage": "updated" })),
            "case {form:?}: GetSession must read back what UpdateSession stored, got {session:?}"
        );

        let (status, ended) = call(
            &router,
            "PATCH",
            &format!("/sessions/{}", label(identifier)),
            Value::Null,
        )
        .await;
        assert_eq!(
            (status, ended["sessionStatus"].as_str()),
            (200, Some("ENDED")),
            "case {form:?}: EndSession must resolve the identifier, got {status} {ended:?}"
        );

        let (status, _) = call(
            &router,
            "DELETE",
            &format!("/sessions/{}/", label(identifier)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {form:?}: DeleteSession must resolve the identifier, got {status}"
        );
    }
}

/// RULE: a step's `payload` union is stored whole and reported back unchanged, for
/// every shape the union can take. The rows span the union's member
/// (`contentBlocks`), the content block variants (`text`, `image`), the image source
/// variants (`bytes`, `s3Location`), the block count and the ordering — the catalog
/// of variants and the `ImageFormat` values come from
/// `smithy_query(query="shape",shape="BedrockSessionContentBlock,ImageSource,ImageFormat")`
/// (`ImageFormat` is png | jpeg | gif | webp), and the cited case (one `text` block)
/// is only the first row.
#[tokio::test]
async fn an_invocation_step_payload_round_trips_unchanged_for_every_union_shape() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "one text block",
            json!({ "contentBlocks": [{ "text": "hello" }] }),
        ),
        (
            "three text blocks keep their order",
            json!({ "contentBlocks": [
                { "text": "first" },
                { "text": "second" },
                { "text": "third" },
            ] }),
        ),
        (
            "text with multi-byte characters and whitespace",
            json!({ "contentBlocks": [{ "text": "  ok \u{3053}\u{3093}\u{306b}\u{3061}\u{306f} \u{1f600}\n" }] }),
        ),
        (
            "image from an s3 location, format webp",
            json!({ "contentBlocks": [{ "image": {
                "format": "webp",
                "source": { "s3Location": { "uri": "s3://example-bucket/checkpoints/frame-1.webp" } },
            } }] }),
        ),
        (
            "image from raw bytes, format gif",
            json!({ "contentBlocks": [{ "image": {
                "format": "gif",
                "source": { "bytes": "R0lGODlhAQABAAAAACw=" },
            } }] }),
        ),
        (
            "mixed text and image blocks",
            json!({ "contentBlocks": [
                { "text": "look at this" },
                { "image": {
                    "format": "png",
                    "source": { "s3Location": { "uri": "s3://example-bucket/a.png" } },
                } },
                { "text": "and that" },
            ] }),
        ),
    ];

    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;
    let invocation_id = create_invocation(&router, &session_id, json!({})).await;

    for (case, payload) in &cases {
        let step_id = put_step(
            &router,
            &session_id,
            &invocation_id,
            1_772_600_767,
            payload,
        )
        .await;
        let (status, got) = call(
            &router,
            "POST",
            &format!(
                "/sessions/{}/invocationSteps/{step_id}",
                label(&session_id)
            ),
            json!({ "invocationIdentifier": invocation_id }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {got:?}");
        assert_eq!(
            &got["invocationStep"]["payload"], payload,
            "case {case:?}: the payload must come back unchanged, got {:?}",
            got["invocationStep"]["payload"]
        );
    }
}

/// RULE: `PutInvocationStep` is `@idempotent`, so a write under an `invocationStepId`
/// that already names a stored step is accepted only as a REPLAY of that step's own
/// content; a write that would change the stored content is refused with the modelled
/// `ConflictException` (409) and the stored step is left as it was.
#[tokio::test]
async fn rewriting_a_stored_step_id_with_other_content_is_refused() {
    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;
    let invocation_id = create_invocation(&router, &session_id, json!({})).await;
    let stored = json!({ "contentBlocks": [{ "text": "draft" }] });
    let stored_time: i64 = 1_777_957_505;
    let step_id = put_step(
        &router,
        &session_id,
        &invocation_id,
        stored_time,
        &stored,
    )
    .await;

    // (case, invocationStepTime, payload, expected status, expected error code)
    let rows: Vec<(&str, i64, Value, u16, &str)> = vec![
        (
            "the same content again: an idempotent replay",
            stored_time,
            stored.clone(),
            201,
            "",
        ),
        (
            "a different payload under the same id",
            stored_time,
            json!({ "contentBlocks": [{ "text": "final" }, { "text": "addendum" }] }),
            409,
            "ConflictException",
        ),
        (
            "one more content block under the same id",
            stored_time,
            json!({ "contentBlocks": [{ "text": "draft" }, { "text": "draft" }] }),
            409,
            "ConflictException",
        ),
        (
            "the same payload at a different instant",
            1_777_961_166,
            stored.clone(),
            409,
            "ConflictException",
        ),
    ];

    for (case, step_time, payload, expected_status, expected_code) in rows {
        let (status, out) = call(
            &router,
            "PUT",
            &format!("/sessions/{}/invocationSteps/", label(&session_id)),
            json!({
                "invocationIdentifier": invocation_id,
                "invocationStepId": step_id,
                "invocationStepTime": date_time(step_time),
                "payload": payload,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&out)),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code:?}, got {status} {out:?}"
        );
        if expected_status == 201 {
            assert_eq!(
                out["invocationStepId"].as_str(),
                Some(step_id.as_str()),
                "case {case:?}: a replay must answer the stored id, got {out:?}"
            );
        }

        // Whatever the answer, the stored step is still the one first written.
        let (status, got) = call(
            &router,
            "POST",
            &format!(
                "/sessions/{}/invocationSteps/{step_id}",
                label(&session_id)
            ),
            json!({ "invocationIdentifier": invocation_id }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: GetInvocationStep expected 200, got {got:?}");
        assert_eq!(
            (
                &got["invocationStep"]["payload"],
                seconds(&got["invocationStep"]["invocationStepTime"])
            ),
            (&stored, Some(stored_time as f64)),
            "case {case:?}: the stored step must be unchanged, got {got:?}"
        );

        let (status, listed) = call(
            &router,
            "POST",
            &format!("/sessions/{}/invocationSteps/", label(&session_id)),
            json!({ "invocationIdentifier": invocation_id }),
        )
        .await;
        assert_eq!(
            (status, listed["invocationStepSummaries"].as_array().map(Vec::len)),
            (200, Some(1)),
            "case {case:?}: neither a replay nor a refusal adds a step, got {status} {listed:?}"
        );
    }
}

/// Walk every page of ListInvocationSteps, returning the ids seen in order and the
/// size of each page.
async fn drain_steps(
    router: &MultiServiceRouter,
    session_id: &str,
    body: Value,
    max_results: usize,
) -> (Vec<String>, Vec<usize>) {
    let mut ids: Vec<String> = Vec::new();
    let mut page_sizes: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut path = format!(
            "/sessions/{}/invocationSteps/?maxResults={max_results}",
            label(session_id)
        );
        if let Some(cursor) = &token {
            path.push_str("&nextToken=");
            path.push_str(cursor);
        }
        let (status, listed) = call(router, "POST", &path, body.clone()).await;
        assert_eq!(
            status, 200,
            "ListInvocationSteps(maxResults={max_results}): expected 200, got {status} {listed:?}"
        );
        let page = listed["invocationStepSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("ListInvocationSteps: no summaries in {listed:?}"));
        page_sizes.push(page.len());
        for summary in page {
            ids.push(
                summary["invocationStepId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("ListInvocationSteps: no id in {summary:?}"))
                    .to_string(),
            );
        }
        token = listed
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return (ids, page_sizes);
        }
        assert!(
            page_sizes.len() <= 64,
            "ListInvocationSteps(maxResults={max_results}): pagination did not terminate after \
             {} pages",
            page_sizes.len()
        );
    }
}

/// RULE: a paginated list returns at most `maxResults` items per page from stored
/// state, in a stable order, emits a `nextToken` only while items remain, and the
/// pages concatenate to every item exactly once. The rows vary the stored item count
/// and the page size, including the published bounds of `maxResults` (1..1000,
/// `smithy_query(query="operation",operation="ListInvocationSteps")`).
#[tokio::test]
async fn list_invocation_steps_pages_stored_state_without_loss_or_repetition() {
    // (stored step count, maxResults, expected page sizes)
    let cases: [(usize, usize, &[usize]); 7] = [
        (0, 1, &[0]),
        (1, 1, &[1]),
        (3, 1, &[1, 1, 1]),
        (5, 2, &[2, 2, 1]),
        (4, 4, &[4]),
        (4, 1000, &[4]),
        (7, 3, &[3, 3, 1]),
    ];

    for (total, max_results, expected_pages) in cases {
        let router = router();
        let (session_id, _) = create_session(&router, Value::Null).await;
        let invocation_id = create_invocation(&router, &session_id, json!({})).await;

        let mut written: Vec<String> = Vec::new();
        for n in 0..total {
            written.push(
                put_step(
                    &router,
                    &session_id,
                    &invocation_id,
                    1_783_501_811 + n as i64,
                    &json!({ "contentBlocks": [{ "text": format!("step-{n}") }] }),
                )
                .await,
            );
        }

        let (seen, page_sizes) = drain_steps(
            &router,
            &session_id,
            json!({ "invocationIdentifier": invocation_id }),
            max_results,
        )
        .await;
        assert_eq!(
            page_sizes, expected_pages,
            "case (total={total}, maxResults={max_results}): expected pages {expected_pages:?}, \
             got {page_sizes:?}"
        );
        assert_eq!(
            seen, written,
            "case (total={total}, maxResults={max_results}): the pages must concatenate to every \
             stored step once, in store order; got {seen:?}"
        );
    }
}

/// RULE: `invocationIdentifier` filters the listing to one invocation; without it the
/// whole session's steps are listed, across invocations, in store order.
#[tokio::test]
async fn list_invocation_steps_filters_by_invocation_only_when_asked() {
    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;

    let first = create_invocation(&router, &session_id, json!({ "description": "turn-1" })).await;
    let second = create_invocation(&router, &session_id, json!({ "description": "turn-2" })).await;

    let mut first_steps = Vec::new();
    let mut second_steps = Vec::new();
    for n in 0..2 {
        first_steps.push(
            put_step(
                &router,
                &session_id,
                &first,
                1_783_501_811,
                &json!({ "contentBlocks": [{ "text": format!("a{n}") }] }),
            )
            .await,
        );
    }
    for n in 0..3 {
        second_steps.push(
            put_step(
                &router,
                &session_id,
                &second,
                1_783_501_812,
                &json!({ "contentBlocks": [{ "text": format!("b{n}") }] }),
            )
            .await,
        );
    }

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "filtered to the first invocation",
            json!({ "invocationIdentifier": first }),
            first_steps.clone(),
        ),
        (
            "filtered to the second invocation",
            json!({ "invocationIdentifier": second }),
            second_steps.clone(),
        ),
        (
            "unfiltered: every step in the session",
            json!({}),
            first_steps
                .iter()
                .chain(second_steps.iter())
                .cloned()
                .collect(),
        ),
    ];

    for (case, body, expected) in cases {
        let (seen, _) = drain_steps(&router, &session_id, body, 2).await;
        assert_eq!(
            seen, expected,
            "case {case:?}: expected {expected:?}, got {seen:?}"
        );
    }
}

/// RULE: a `nextToken` the service did not issue is a client error — the modelled
/// `ValidationException` (400) — not a silent first page and not a 501.
#[tokio::test]
async fn a_pagination_token_the_service_did_not_issue_is_refused() {
    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;
    let invocation_id = create_invocation(&router, &session_id, json!({})).await;
    put_step(
        &router,
        &session_id,
        &invocation_id,
        1_783_501_811,
        &json!({ "contentBlocks": [{ "text": "only" }] }),
    )
    .await;

    for token in ["not-a-token", "zzzz", "6f726465723a", "00"] {
        let (status, refused) = call(
            &router,
            "POST",
            &format!(
                "/sessions/{}/invocationSteps/?nextToken={token}",
                label(&session_id)
            ),
            json!({ "invocationIdentifier": invocation_id }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "ValidationException"),
            "case {token:?}: expected 400 ValidationException, got {status} {refused:?}"
        );
    }
}

/// RULE: an omitted `maxResults` pages at the default the model declares (10), not at
/// the type's zero. The rows are the member's presence: omitted, and set explicitly
/// above and below that default.
#[tokio::test]
async fn an_omitted_max_results_pages_at_the_modelled_default() {
    use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::state::DEFAULT_LIST_PAGE;

    // The literal the model declares for `maxResults`
    // (`smithy_query(query="operation",operation="ListInvocationSteps")` -> `maxResults`
    // has `default: 10`), asserted independently of the implementation's constant so a
    // constant that drifted away from the model fails this test.
    const MODELLED_DEFAULT: usize = 10;
    assert_eq!(
        usize::try_from(DEFAULT_LIST_PAGE).ok(),
        Some(MODELLED_DEFAULT),
        "case \"the service's default page size\": expected the modelled {MODELLED_DEFAULT}, got {DEFAULT_LIST_PAGE:?}"
    );

    let stored = MODELLED_DEFAULT + 2;
    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;
    let invocation_id = create_invocation(&router, &session_id, json!({})).await;
    for n in 0..stored {
        put_step(
            &router,
            &session_id,
            &invocation_id,
            1_783_501_811 + n as i64,
            &json!({ "contentBlocks": [{ "text": format!("step-{n}") }] }),
        )
        .await;
    }

    // (case, the query string, the first page's expected size)
    let cases: [(&str, &str, usize); 3] = [
        ("maxResults omitted", "", MODELLED_DEFAULT),
        ("maxResults below the default", "?maxResults=3", 3),
        ("maxResults above the stored count", "?maxResults=50", stored),
    ];

    for (case, query, expected) in cases {
        let (status, page) = call(
            &router,
            "POST",
            &format!("/sessions/{session_id}/invocationSteps/{query}"),
            json!({ "invocationIdentifier": invocation_id }),
        )
        .await;
        assert_eq!(
            (status, page["invocationStepSummaries"].as_array().map(Vec::len)),
            (200, Some(expected)),
            "case {case:?}: expected a first page of {expected}, got {status} {page:?}"
        );
        let expects_token = expected < stored;
        assert_eq!(
            page.get("nextToken").is_some_and(|t| !t.is_null()),
            expects_token,
            "case {case:?}: nextToken presence must be {expects_token}, got {page:?}"
        );
    }
}

/// RULE: `maxResults` is honoured inside the range the model publishes and a value
/// outside it is refused, never silently reinterpreted. The rows are each published
/// bound and one step past each of them
/// (`smithy_query(query="operation",operation="ListInvocationSteps")` -> `maxResults`
/// has `range {min: 1, max: 1000}`).
#[tokio::test]
async fn max_results_is_honoured_inside_the_published_range_and_refused_outside_it() {
    /// The model's published bounds for `maxResults`.
    const MIN: i64 = 1;
    /// See [`MIN`].
    const MAX: i64 = 1000;

    let stored = 3;
    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;
    let invocation_id = create_invocation(&router, &session_id, json!({})).await;
    for n in 0..stored {
        put_step(
            &router,
            &session_id,
            &invocation_id,
            1_783_501_811 + n as i64,
            &json!({ "contentBlocks": [{ "text": format!("step-{n}") }] }),
        )
        .await;
    }

    // (case, the maxResults query value, the accepted page size or None when the
    // value is outside the published range and must be refused)
    let cases: [(&str, i64, Option<usize>); 5] = [
        ("the published minimum", MIN, Some(1)),
        ("one below the published minimum", MIN - 1, None),
        // Below the minimum and distinct from the integer zero an absent member
        // arrives as, so it exercises the handler's own range check rather than only
        // the boundary validator's.
        ("negative, well below the published minimum", -1, None),
        ("the published maximum", MAX, Some(stored)),
        ("one above the published maximum", MAX + 1, None),
    ];

    for (case, value, accepted) in cases {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/sessions/{session_id}/invocationSteps/?maxResults={value}"),
            json!({ "invocationIdentifier": invocation_id }),
        )
        .await;
        match accepted {
            Some(size) => assert_eq!(
                (status, body["invocationStepSummaries"].as_array().map(Vec::len)),
                (200, Some(size)),
                "case {case:?}: maxResults={value} must page at {size}, got {status} {body:?}"
            ),
            None => assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case {case:?}: maxResults={value} is outside the published range and must be \
                 refused, got {status} {body:?}"
            ),
        }
    }
}

/// RULE: a pagination cursor belongs to the listing that issued it, so a token minted
/// for one session or one `invocationIdentifier` filter is refused on any other
/// listing rather than silently resuming at an unrelated offset. The rows are the two
/// ways a listing's scope can differ: the session, and the invocation filter.
#[tokio::test]
async fn a_pagination_token_is_only_honoured_by_the_listing_that_issued_it() {
    let router = router();

    // Session A: two invocations, two steps each, so every listing pages.
    let (session_a, _) = create_session(&router, Value::Null).await;
    let first = create_invocation(&router, &session_a, json!({})).await;
    let second = create_invocation(&router, &session_a, json!({})).await;
    for invocation in [&first, &second] {
        for n in 0..2 {
            put_step(
                &router,
                &session_a,
                invocation,
                1_783_501_811 + n,
                &json!({ "contentBlocks": [{ "text": format!("a{n}") }] }),
            )
            .await;
        }
    }

    // Session B holds its own steps, so a cursor "resuming" here would skip items.
    let (session_b, _) = create_session(&router, Value::Null).await;
    let b_invocation = create_invocation(&router, &session_b, json!({})).await;
    for n in 0..3 {
        put_step(
            &router,
            &session_b,
            &b_invocation,
            1_783_501_811 + n,
            &json!({ "contentBlocks": [{ "text": format!("b{n}") }] }),
        )
        .await;
    }

    // The token issued by listing session A, filtered to its first invocation.
    let (status, page) = call(
        &router,
        "POST",
        &format!("/sessions/{session_a}/invocationSteps/?maxResults=1"),
        json!({ "invocationIdentifier": first }),
    )
    .await;
    let token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a nextToken, got {status} {page:?}"))
        .to_string();

    // (case, the listing to replay the token on, its body)
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "another session",
            format!("/sessions/{session_b}/invocationSteps/?maxResults=1&nextToken={token}"),
            json!({ "invocationIdentifier": b_invocation }),
        ),
        (
            "another invocation filter in the same session",
            format!("/sessions/{session_a}/invocationSteps/?maxResults=1&nextToken={token}"),
            json!({ "invocationIdentifier": second }),
        ),
        (
            "no invocation filter in the same session",
            format!("/sessions/{session_a}/invocationSteps/?maxResults=1&nextToken={token}"),
            json!({}),
        ),
    ];

    for (case, path, body) in cases {
        let (status, refused) = call(&router, "POST", &path, body).await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "ValidationException"),
            "case {case:?}: a foreign cursor must be refused, got {status} {refused:?}"
        );
    }

    // And the listing that issued it still honours it.
    let (status, resumed) = call(
        &router,
        "POST",
        &format!("/sessions/{session_a}/invocationSteps/?maxResults=1&nextToken={token}"),
        json!({ "invocationIdentifier": first }),
    )
    .await;
    assert_eq!(
        (
            status,
            resumed["invocationStepSummaries"].as_array().map(Vec::len)
        ),
        (200, Some(1)),
        "the issuing listing must still honour its own cursor, got {status} {resumed:?}"
    );
}

/// RULE: an operation naming a session, invocation or step that is not stored answers
/// the modelled `ResourceNotFoundException` (404). The rows span each level of the
/// hierarchy and each operation that resolves one.
#[tokio::test]
async fn an_unknown_session_invocation_or_step_is_a_modelled_404() {
    let router = router();
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
    let absent = "ffffffff-ffff-ffff-ffff-ffffffffffff";

    let cases: Vec<(&str, &str, String, Value)> = vec![
        (
            "GetSession, unknown session",
            "GET",
            format!("/sessions/{absent}/"),
            Value::Null,
        ),
        (
            // Only an ARN this emulator could have minted resolves: another
            // account's ARN names nothing here, as on the real service.
            "GetSession, this session's id under another account's ARN",
            "GET",
            format!(
                "/sessions/{}/",
                label(&format!(
                    "arn:aws:bedrock:us-west-2:999999999999:session/{session_id}"
                ))
            ),
            Value::Null,
        ),
        (
            "GetSession, this session's id under another service's ARN",
            "GET",
            format!(
                "/sessions/{}/",
                label(&format!(
                    "arn:aws:sagemaker:us-west-2:000000000000:session/{session_id}"
                ))
            ),
            Value::Null,
        ),
        (
            "UpdateSession, unknown session",
            "PUT",
            format!("/sessions/{absent}/"),
            json!({ "sessionMetadata": { "k": "v" } }),
        ),
        (
            "EndSession, unknown session",
            "PATCH",
            format!("/sessions/{absent}"),
            Value::Null,
        ),
        (
            "DeleteSession, unknown session",
            "DELETE",
            format!("/sessions/{absent}/"),
            Value::Null,
        ),
        (
            "CreateInvocation, unknown session",
            "PUT",
            format!("/sessions/{absent}/invocations/"),
            json!({}),
        ),
        (
            "PutInvocationStep, unknown session",
            "PUT",
            format!("/sessions/{absent}/invocationSteps/"),
            json!({
                "invocationIdentifier": invocation_id,
                "invocationStepTime": date_time(1_783_501_811),
                "payload": { "contentBlocks": [{ "text": "x" }] },
            }),
        ),
        (
            "PutInvocationStep, unknown invocation",
            "PUT",
            format!("/sessions/{session_id}/invocationSteps/"),
            json!({
                "invocationIdentifier": absent,
                "invocationStepTime": date_time(1_783_501_811),
                "payload": { "contentBlocks": [{ "text": "x" }] },
            }),
        ),
        (
            "GetInvocationStep, unknown invocation",
            "POST",
            format!("/sessions/{session_id}/invocationSteps/{step_id}"),
            json!({ "invocationIdentifier": absent }),
        ),
        (
            "GetInvocationStep, unknown step",
            "POST",
            format!("/sessions/{session_id}/invocationSteps/{absent}"),
            json!({ "invocationIdentifier": invocation_id }),
        ),
        (
            "ListInvocationSteps, unknown session",
            "POST",
            format!("/sessions/{absent}/invocationSteps/"),
            json!({}),
        ),
        (
            "ListInvocationSteps, unknown invocation",
            "POST",
            format!("/sessions/{session_id}/invocationSteps/"),
            json!({ "invocationIdentifier": absent }),
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, refused) = call(&router, method, &path, body).await;
        assert_eq!(
            (status, error_code(&refused)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {refused:?}"
        );
    }
}

/// RULE: an ended session's content stays readable but can no longer be added to or
/// modified (`smithy_query(query="doc",shape="EndSession")`), while an ACTIVE session
/// cannot be deleted (`smithy_query(query="doc",shape="DeleteSession")`). The rows
/// span every operation that writes to a session.
#[tokio::test]
async fn an_ended_session_is_readable_but_no_longer_writable() {
    let router = router();
    let (session_id, _) = create_session(&router, json!({ "topic": "demo" })).await;
    let invocation_id = create_invocation(&router, &session_id, json!({})).await;
    let step_id = put_step(
        &router,
        &session_id,
        &invocation_id,
        1_783_501_811,
        &json!({ "contentBlocks": [{ "text": "before the end" }] }),
    )
    .await;

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

    // The ACTIVE -> ENDED transition is one-way: a second EndSession has no transition
    // to perform and is refused.
    let (status, again) = call(
        &router,
        "PATCH",
        &format!("/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, error_code(&again)),
        (409, "ConflictException"),
        "EndSession on an ENDED session must be refused, got {status} {again:?}"
    );
    assert_eq!(
        ended["sessionStatus"].as_str(),
        Some("ENDED"),
        "the first EndSession is the one that performed the transition, got {ended:?}"
    );

    // Writes are refused.
    let writes: Vec<(&str, &str, String, Value)> = vec![
        (
            "CreateInvocation on an ENDED session",
            "PUT",
            format!("/sessions/{session_id}/invocations/"),
            json!({}),
        ),
        (
            "PutInvocationStep on an ENDED session",
            "PUT",
            format!("/sessions/{session_id}/invocationSteps/"),
            json!({
                "invocationIdentifier": invocation_id,
                "invocationStepTime": date_time(1_783_501_812),
                "payload": { "contentBlocks": [{ "text": "after the end" }] },
            }),
        ),
        (
            "UpdateSession on an ENDED session",
            "PUT",
            format!("/sessions/{session_id}/"),
            json!({ "sessionMetadata": { "topic": "changed" } }),
        ),
    ];
    for (case, method, path, body) in writes {
        let (status, refused) = call(&router, method, &path, body).await;
        assert_eq!(
            (status, error_code(&refused)),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {status} {refused:?}"
        );
    }

    // Reads still work, and report exactly what was stored before the session ended.
    let (status, session) = call(
        &router,
        "GET",
        &format!("/sessions/{session_id}/"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, &session["sessionMetadata"]),
        (200, &json!({ "topic": "demo" })),
        "GetSession on an ENDED session must still report its metadata, got {status} {session:?}"
    );
    let (status, got) = call(
        &router,
        "POST",
        &format!("/sessions/{session_id}/invocationSteps/{step_id}"),
        json!({ "invocationIdentifier": invocation_id }),
    )
    .await;
    assert_eq!(
        (
            status,
            &got["invocationStep"]["payload"]
        ),
        (200, &json!({ "contentBlocks": [{ "text": "before the end" }] })),
        "GetInvocationStep on an ENDED session must still report the step, got {status} {got:?}"
    );
}

/// RULE: `(sessionId, invocationId)` is a key — a create under an `invocationId` the
/// session already holds is refused with the modelled `ConflictException` (409), so
/// exactly one caller claims an id and no second invocation is added under it.
#[tokio::test]
async fn a_second_create_invocation_with_a_known_id_is_refused() {
    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;
    let chosen = "0123abcd-4567-89ef-0123-456789abcdef";

    let path = format!("/sessions/{session_id}/invocations/");
    let (status, first) = call(
        &router,
        "PUT",
        &path,
        json!({ "invocationId": chosen, "description": "turn-1" }),
    )
    .await;
    assert_eq!(
        (status, first["invocationId"].as_str()),
        (201, Some(chosen)),
        "CreateInvocation must use the supplied invocationId, got {status} {first:?}"
    );

    // The rows differ in what the SECOND create carries: the same body, a different
    // description, and no description at all. The id is claimed, so each is refused.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the same body again",
            json!({ "invocationId": chosen, "description": "turn-1" }),
        ),
        (
            "a different description",
            json!({ "invocationId": chosen, "description": "turn-2" }),
        ),
        ("no description at all", json!({ "invocationId": chosen })),
    ];
    for (case, body) in rows {
        let (status, refused) = call(&router, "PUT", &path, body).await;
        assert_eq!(
            (status, error_code(&refused)),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {status} {refused:?}"
        );
    }

    // And the one claimed invocation is still there, holding the step written under it.
    put_step(
        &router,
        &session_id,
        chosen,
        1_783_501_811,
        &json!({ "contentBlocks": [{ "text": "x" }] }),
    )
    .await;
    let (seen, _) = drain_steps(&router, &session_id, json!({}), 10).await;
    assert_eq!(
        seen.len(),
        1,
        "exactly one step must be stored in the session, got {seen:?}"
    );
}

/// RULE: each session is a separate resource — two CreateSession calls mint two
/// distinct ids and ARNs, and neither sees the other's content.
#[tokio::test]
async fn sessions_are_minted_fresh_and_isolated_from_one_another() {
    let router = router();
    let (first_id, first_arn) = create_session(&router, json!({ "n": "1" })).await;
    let (second_id, second_arn) = create_session(&router, json!({ "n": "2" })).await;

    assert_ne!(
        first_id, second_id,
        "two CreateSession calls must mint different sessionIds, got {first_id} twice"
    );
    assert_ne!(
        first_arn, second_arn,
        "two CreateSession calls must mint different sessionArns, got {first_arn} twice"
    );

    let invocation_id = create_invocation(&router, &first_id, json!({})).await;
    put_step(
        &router,
        &first_id,
        &invocation_id,
        1_783_501_811,
        &json!({ "contentBlocks": [{ "text": "in the first session" }] }),
    )
    .await;

    let (seen, _) = drain_steps(&router, &second_id, json!({}), 10).await;
    assert!(
        seen.is_empty(),
        "the second session must hold no steps of its own, got {seen:?}"
    );

    let (status, refused) = call(
        &router,
        "POST",
        &format!("/sessions/{second_id}/invocationSteps/"),
        json!({ "invocationIdentifier": invocation_id }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (404, "ResourceNotFoundException"),
        "an invocation of another session must not resolve, got {status} {refused:?}"
    );
}

/// RULE: every id this service mints for a `Uuid`-shaped member is a version-4 UUID in
/// the canonical lowercase hyphenated form, and no two mints collide. The rows are the
/// three members the service mints (`sessionId`, `invocationId`, `invocationStepId`),
/// each drawn repeatedly so a generator that repeated itself or emitted a value outside
/// the shape would fail.
///
/// The shape is the model's own: `smithy_query(query="operation",operation="CreateSession")`
/// -> `sessionId` targets `Uuid`,
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`. The version and
/// variant positions are RFC 4122 section 4.4: the 15th character is the version and the
/// 20th carries the variant in its top two bits (`8`, `9`, `a` or `b`).
#[tokio::test]
async fn every_minted_id_is_a_distinct_version_4_uuid() {
    /// How many times each minting site is exercised.
    const DRAWS: usize = 12;

    let router = router();
    let mut minted: Vec<(&str, String)> = Vec::with_capacity(DRAWS * 3);
    for n in 0..DRAWS {
        let (session_id, _) = create_session(&router, Value::Null).await;
        let invocation_id = create_invocation(&router, &session_id, json!({})).await;
        let step_id = put_step(
            &router,
            &session_id,
            &invocation_id,
            1_783_501_811 + n as i64,
            &json!({ "contentBlocks": [{ "text": "minted" }] }),
        )
        .await;
        minted.push(("sessionId", session_id));
        minted.push(("invocationId", invocation_id));
        minted.push(("invocationStepId", step_id));
    }

    for (member, id) in &minted {
        assert!(
            is_model_uuid(id),
            "case {member:?}: a minted id must match the model's Uuid pattern, got {id:?}"
        );
        let version = id.chars().nth(14);
        assert_eq!(
            version,
            Some('4'),
            "case {member:?}: a minted id must carry RFC 4122 version 4, got {id:?}"
        );
        let variant = id.chars().nth(19);
        assert!(
            variant.is_some_and(|c| matches!(c, '8' | '9' | 'a' | 'b')),
            "case {member:?}: a minted id must carry the RFC 4122 variant bits, got {id:?}"
        );
    }

    let distinct: std::collections::BTreeSet<&str> =
        minted.iter().map(|(_, id)| id.as_str()).collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "every minted id must be distinct, got {} distinct of {}: {minted:?}",
        distinct.len(),
        minted.len()
    );
}

/// RULE: a minted `sessionArn` always satisfies the pattern the model declares for it
/// (`smithy_query(query="operation",operation="CreateSession")` -> `sessionArn`:
/// `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]+:[0-9]{12}:session/<uuid>$`), so its region
/// segment is the caller's SigV4 credential-scope region only while that value can be
/// one, and this emulator's default region otherwise. The rows span a scope region
/// that is usable, another one, the longest segment the model admits
/// (`AWSResourceARN` bounds the segment to `[a-z0-9-]{1,20}`), one character past it,
/// and a value carrying characters the segment does not admit.
#[tokio::test]
async fn a_minted_session_arn_always_matches_the_modelled_pattern() {
    /// The region this emulator stamps when the credential scope cannot supply one.
    const FALLBACK: &str = "us-east-1";

    let router = router();
    // (case, the region the request signs for, the region segment the ARN must carry)
    let cases: [(&str, &str, &str); 5] = [
        ("the region the caller signed for", "us-west-2", "us-west-2"),
        ("another signed region", "ap-southeast-4", "ap-southeast-4"),
        (
            "the longest segment the model admits",
            "aaaaaaaaaa-aaaaaaaaa",
            "aaaaaaaaaa-aaaaaaaaa",
        ),
        (
            "one character past the modelled segment",
            "aaaaaaaaaa-aaaaaaaaaa",
            FALLBACK,
        ),
        ("characters the segment does not admit", "US_East_1", FALLBACK),
    ];

    for (case, scope_region, expected_segment) in cases {
        let (status, out) =
            call_signed_for(&router, "PUT", "/sessions/", json!({}), scope_region).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateSession must answer 201, got {status} {out:?}"
        );
        let session_id = out["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no sessionId in {out:?}"));
        let arn = out["sessionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no sessionArn in {out:?}"));
        assert_eq!(
            arn,
            format!("arn:aws:bedrock:{expected_segment}:{ACCOUNT}:session/{session_id}"),
            "case {case:?}: the minted ARN must carry the {expected_segment:?} region segment, \
             got {arn:?}"
        );

        // And the segment really is one the declared pattern admits.
        let segment = arn.split(':').nth(3).unwrap_or("<absent>");
        assert!(
            !segment.is_empty()
                && segment.len() <= 20
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "case {case:?}: the ARN's region segment must match [a-z0-9-]{{1,20}}, got \
             {segment:?} in {arn:?}"
        );
    }
}

/// RULE: a reporting operation must not change what a later caller sees. GetSession,
/// GetInvocationStep and ListInvocationSteps are `@readonly`, so repeating them
/// leaves every response identical.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = router();
    let (session_id, _) = create_session(&router, json!({ "topic": "demo" })).await;
    let invocation_id = create_invocation(&router, &session_id, json!({})).await;
    let step_id = put_step(
        &router,
        &session_id,
        &invocation_id,
        1_783_501_811,
        &json!({ "contentBlocks": [{ "text": "stable" }] }),
    )
    .await;

    let reads: Vec<(&str, &str, String, Value)> = vec![
        (
            "GetSession",
            "GET",
            format!("/sessions/{session_id}/"),
            Value::Null,
        ),
        (
            "GetInvocationStep",
            "POST",
            format!("/sessions/{session_id}/invocationSteps/{step_id}"),
            json!({ "invocationIdentifier": invocation_id }),
        ),
        (
            "ListInvocationSteps",
            "POST",
            format!("/sessions/{session_id}/invocationSteps/"),
            json!({}),
        ),
    ];

    for (case, method, path, body) in reads {
        let (first_status, first) = call(&router, method, &path, body.clone()).await;
        let (second_status, second) = call(&router, method, &path, body.clone()).await;
        assert_eq!(
            (first_status, &first),
            (second_status, &second),
            "case {case:?}: a repeated read must answer identically, got {first:?} then {second:?}"
        );
        // And the session is still ACTIVE: a read must not move a status.
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
            "case {case:?}: reading must leave the session ACTIVE, got {status} {session:?}"
        );
    }
}

/// RULE: the in-memory store has a declared maximum at every level it grows, and a
/// write that would pass it is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 400) rather than allocating. The rows are
/// the three growth sites, each driven to its own published constant.
#[tokio::test]
async fn a_write_past_a_declared_store_maximum_is_refused_not_allocated() {
    use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::state::{
        MAX_INVOCATIONS_PER_SESSION, MAX_SESSIONS, MAX_STEPS_PER_INVOCATION,
    };

    // Sessions.
    let sessions_router = router();
    for n in 0..MAX_SESSIONS {
        let (status, out) = call(&sessions_router, "PUT", "/sessions/", json!({})).await;
        assert_eq!(
            status, 201,
            "case sessions: CreateSession {n} of {MAX_SESSIONS} must succeed, got {status} {out:?}"
        );
    }
    let (status, refused) = call(&sessions_router, "PUT", "/sessions/", json!({})).await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "ServiceQuotaExceededException"),
        "case sessions: CreateSession past MAX_SESSIONS={MAX_SESSIONS} must be refused, \
         got {status} {refused:?}"
    );

    // Invocations in one session.
    let invocations_router = router();
    let (session_id, _) = create_session(&invocations_router, Value::Null).await;
    let invocations_path = format!("/sessions/{session_id}/invocations/");
    for n in 0..MAX_INVOCATIONS_PER_SESSION {
        let (status, out) = call(&invocations_router, "PUT", &invocations_path, json!({})).await;
        assert_eq!(
            status, 201,
            "case invocations: CreateInvocation {n} of {MAX_INVOCATIONS_PER_SESSION} must \
             succeed, got {status} {out:?}"
        );
    }
    let (status, refused) = call(&invocations_router, "PUT", &invocations_path, json!({})).await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "ServiceQuotaExceededException"),
        "case invocations: CreateInvocation past MAX_INVOCATIONS_PER_SESSION=\
         {MAX_INVOCATIONS_PER_SESSION} must be refused, got {status} {refused:?}"
    );

    // Steps in one invocation.
    let steps_router = router();
    let (session_id, _) = create_session(&steps_router, Value::Null).await;
    let invocation_id = create_invocation(&steps_router, &session_id, json!({})).await;
    for n in 0..MAX_STEPS_PER_INVOCATION {
        put_step(
            &steps_router,
            &session_id,
            &invocation_id,
            1_783_501_811,
            &json!({ "contentBlocks": [{ "text": format!("step-{n}") }] }),
        )
        .await;
    }
    let (status, refused) = call(
        &steps_router,
        "PUT",
        &format!("/sessions/{session_id}/invocationSteps/"),
        json!({
            "invocationIdentifier": invocation_id,
            "invocationStepTime": date_time(1_783_501_811),
            "payload": { "contentBlocks": [{ "text": "one too many" }] },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "ServiceQuotaExceededException"),
        "case steps: PutInvocationStep past MAX_STEPS_PER_INVOCATION=\
         {MAX_STEPS_PER_INVOCATION} must be refused, got {status} {refused:?}"
    );

    // A REPLAY of a stored step is not growth, so it is still accepted at the cap.
    let (_, listed) = call(
        &steps_router,
        "POST",
        &format!("/sessions/{session_id}/invocationSteps/?maxResults=1"),
        json!({ "invocationIdentifier": invocation_id }),
    )
    .await;
    let summary = &listed["invocationStepSummaries"][0];
    let existing = summary["invocationStepId"]
        .as_str()
        .unwrap_or_else(|| panic!("ListInvocationSteps: no step to replay in {listed:?}"))
        .to_string();
    let (status, out) = call(
        &steps_router,
        "PUT",
        &format!("/sessions/{session_id}/invocationSteps/"),
        json!({
            "invocationIdentifier": invocation_id,
            "invocationStepId": existing,
            "invocationStepTime": date_time(1_783_501_811),
            "payload": { "contentBlocks": [{ "text": "step-0" }] },
        }),
    )
    .await;
    assert_eq!(
        (status, out["invocationStepId"].as_str()),
        (201, Some(existing.as_str())),
        "case steps: replaying a stored step adds nothing, so it must still be accepted at \
         the cap, got {status} {out:?}"
    );
}
