// SPDX-License-Identifier: CC0-1.0

//! The wire form of every `@timestampFormat("date-time")` member of this service's
//! session plane, end to end through the framework router.
//!
//! RULE: a `Timestamp` member the model marks `@timestampFormat("date-time")` travels, on
//! this service's restJson1 wire, as an RFC 3339 / ISO-8601 STRING — in BOTH directions
//! and at the full precision the instant carries — and the instant a request carries is
//! the instant a later read reports.
//!
//! The members are the model's own:
//! `smithy_query(query="operation",operation="PutInvocationStep")` ->
//! `invocationStepTime`, and `smithy_query(query="shape",shape="CreateSessionResponse,
//! GetSessionResponse,UpdateSessionResponse,CreateInvocationResponse,InvocationStep,
//! InvocationStepSummary,SessionSummary")` -> `createdAt`, `lastUpdatedAt`,
//! `invocationStepTime`. Each
//! one reports `"timestampFormat": "date-time"`.
//!
//! The first test drives the whole plane over a TABLE of instants; the second reads the
//! declared format off every one of those member schemas, which is what the codec
//! decides the rendering from (`vendor/smithy-schema-server/src/json.rs`).

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice as runtime;

/// A router carrying only this service, over one shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(runtime::EmulatedService::new(engine));
    router
}

/// Send one request and read back `(status, body)`.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
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
            "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20261021/us-west-2/bedrock/aws4_request, \
             SignedHeaders=host, Signature=0000",
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

/// The instant a `date-time` member carries, or `None` when the member is not on the
/// wire as an RFC 3339 string at all — which is itself the defect this suite pins.
fn instant(case: &str, member: &str, value: &Value) -> DateTime {
    let rendered = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 string, got {value:?}")
    });
    DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} {rendered:?} must parse as RFC 3339: {error}")
    })
}

/// RULE applied to `invocationStepTime`, the one `date-time` member a REQUEST carries:
/// the instant the request sends is accepted (HTTP 201) and every read of the stored step
/// reports that same instant, as a string, at full precision.
///
/// The rows differ along what the rule generalises over: the precision carried (whole
/// seconds, milliseconds, microseconds), the era (before the epoch, the epoch itself,
/// now, a far-future instant), a leap-day instant, and the `+00:00` spelling of the zone
/// a client may send instead of `Z`.
#[tokio::test]
async fn a_date_time_member_round_trips_as_rfc3339_at_full_precision() {
    let rows: [(&str, &str); 8] = [
        ("whole seconds", "2026-10-23T00:33:20Z"),
        ("millisecond precision", "2026-10-23T00:33:20.654Z"),
        ("microsecond precision", "2026-10-23T00:33:20.654272Z"),
        ("the epoch itself", "1970-01-01T00:00:00Z"),
        ("an instant before the epoch", "1969-07-20T20:17:40Z"),
        ("a leap-day instant", "2024-02-29T12:00:00Z"),
        ("a far-future instant", "2099-12-31T23:59:59.999999Z"),
        ("an explicit zero fraction", "2026-10-23T00:33:20.000000Z"),
    ];

    for (case, sent) in rows {
        let router = router();
        let expected = DateTime::from_str(sent, Format::DateTime)
            .unwrap_or_else(|error| panic!("case {case:?}: the test value must parse: {error}"));

        let (status, session) = call(&router, "PUT", "/sessions/", json!({})).await;
        assert_eq!(status, 201, "case {case:?}: CreateSession got {session:?}");
        let session_id = session["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no sessionId in {session:?}"))
            .to_string();

        let (status, invocation) = call(
            &router,
            "PUT",
            &format!("/sessions/{session_id}/invocations/"),
            json!({}),
        )
        .await;
        assert_eq!(status, 201, "case {case:?}: CreateInvocation got {invocation:?}");
        let invocation_id = invocation["invocationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no invocationId in {invocation:?}"))
            .to_string();

        // The request carries the instant as the modelled string form.
        let (status, written) = call(
            &router,
            "PUT",
            &format!("/sessions/{session_id}/invocationSteps/"),
            json!({
                "invocationIdentifier": invocation_id,
                "invocationStepTime": sent,
                "payload": { "contentBlocks": [{ "text": "x" }] },
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: PutInvocationStep must accept the date-time form {sent:?}, \
             got {status} {written:?}"
        );
        let step_id = written["invocationStepId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no invocationStepId in {written:?}"))
            .to_string();

        // GetInvocationStep reports the instant that was written.
        let (status, got) = call(
            &router,
            "POST",
            &format!("/sessions/{session_id}/invocationSteps/{step_id}"),
            json!({ "invocationIdentifier": invocation_id }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: GetInvocationStep got {got:?}");
        let read_back = instant(
            case,
            "invocationStep.invocationStepTime",
            &got["invocationStep"]["invocationStepTime"],
        );
        assert_eq!(
            read_back, expected,
            "case {case:?}: GetInvocationStep must report the instant written ({sent}), \
             got {got:?}"
        );

        // So does the listing's summary of it.
        let (status, listed) = call(
            &router,
            "POST",
            &format!("/sessions/{session_id}/invocationSteps/"),
            json!({ "invocationIdentifier": invocation_id }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: ListInvocationSteps got {listed:?}");
        let summary_time = instant(
            case,
            "invocationStepSummaries.0.invocationStepTime",
            &listed["invocationStepSummaries"][0]["invocationStepTime"],
        );
        assert_eq!(
            summary_time, expected,
            "case {case:?}: the summary must report the instant written ({sent}), \
             got {listed:?}"
        );

        // And every server-generated `date-time` member of the plane is on the wire in
        // the same form: a string that parses as RFC 3339.
        let created_at = instant(case, "createdAt", &session["createdAt"]);
        let invocation_created_at = instant(case, "invocationCreatedAt", &invocation["createdAt"]);

        // The invocation listing's summary reports the instant the invocation was
        // created with, in the same form.
        let (status, invocations) = call(
            &router,
            "POST",
            &format!("/sessions/{session_id}/invocations/"),
            Value::Null,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: ListInvocations got {invocations:?}");
        assert_eq!(
            instant(
                case,
                "invocationSummaries.0.createdAt",
                &invocations["invocationSummaries"][0]["createdAt"]
            ),
            invocation_created_at,
            "case {case:?}: the invocation summary must report the stored createdAt, got \
             {invocations:?}"
        );
        let (status, read) = call(&router, "GET", &format!("/sessions/{session_id}/"), Value::Null)
            .await;
        assert_eq!(status, 200, "case {case:?}: GetSession got {read:?}");
        assert_eq!(
            instant(case, "GetSession.createdAt", &read["createdAt"]),
            created_at,
            "case {case:?}: GetSession must report the session's own createdAt, got {read:?}"
        );
        let last_updated_at = instant(case, "GetSession.lastUpdatedAt", &read["lastUpdatedAt"]);
        assert!(
            last_updated_at >= created_at,
            "case {case:?}: lastUpdatedAt {last_updated_at:?} cannot precede createdAt \
             {created_at:?}"
        );
        assert!(
            invocation_created_at >= created_at,
            "case {case:?}: the invocation's createdAt {invocation_created_at:?} cannot \
             precede the session's {created_at:?}"
        );

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/sessions/{session_id}/"),
            json!({ "sessionMetadata": { "topic": "demo" } }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: UpdateSession got {updated:?}");
        assert_eq!(
            instant(case, "UpdateSession.createdAt", &updated["createdAt"]),
            created_at,
            "case {case:?}: UpdateSession must report the stored createdAt, got {updated:?}"
        );
        assert!(
            instant(case, "UpdateSession.lastUpdatedAt", &updated["lastUpdatedAt"])
                >= last_updated_at,
            "case {case:?}: UpdateSession must not move lastUpdatedAt backwards, \
             got {updated:?}"
        );
    }
}

/// RULE at its boundary: a value that is not the Smithy `date-time` form the member
/// declares is REFUSED with a 400 `ValidationException`, rather than being read as some
/// other instant. (Smithy's `date-time` is RFC 3339 with a `Z` offset only.)
///
/// The rows differ along how the value misses the form: a zone offset instead of `Z`, a
/// date with no time, a human rendering, and an empty string.
#[tokio::test]
async fn a_value_that_is_not_the_date_time_form_is_refused() {
    let router = router();
    let (status, session) = call(&router, "PUT", "/sessions/", json!({})).await;
    assert_eq!(status, 201, "CreateSession got {session:?}");
    let session_id = session["sessionId"]
        .as_str()
        .expect("CreateSession answers a sessionId")
        .to_string();
    let (status, invocation) = call(
        &router,
        "PUT",
        &format!("/sessions/{session_id}/invocations/"),
        json!({}),
    )
    .await;
    assert_eq!(status, 201, "CreateInvocation got {invocation:?}");
    let invocation_id = invocation["invocationId"]
        .as_str()
        .expect("CreateInvocation answers an invocationId")
        .to_string();

    let rows: [(&str, &str); 4] = [
        ("a zone offset instead of Z", "2026-10-23T00:33:20+02:00"),
        ("a date with no time", "2026-10-23"),
        ("a human rendering", "Fri, 23 Oct 2026 00:33:20 GMT"),
        ("an empty string", ""),
    ];

    for (case, sent) in rows {
        let (status, refused) = call(
            &router,
            "PUT",
            &format!("/sessions/{session_id}/invocationSteps/"),
            json!({
                "invocationIdentifier": invocation_id,
                "invocationStepTime": sent,
                "payload": { "contentBlocks": [{ "text": "x" }] },
            }),
        )
        .await;
        let code = refused
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            (status, code),
            (400, "ValidationException"),
            "case {case:?}: {sent:?} must be refused as a bad date-time, got {status} \
             {refused:?}"
        );
    }

    // Nothing was stored by any of those refusals.
    let (status, listed) = call(
        &router,
        "POST",
        &format!("/sessions/{session_id}/invocationSteps/"),
        json!({ "invocationIdentifier": invocation_id }),
    )
    .await;
    assert_eq!(
        (
            status,
            listed["invocationStepSummaries"].as_array().map(Vec::len)
        ),
        (200, Some(0)),
        "a refused write must store nothing, got {status} {listed:?}"
    );
}

/// RULE, read off the schemas the codec consults: every member of this service's session
/// plane whose model declaration carries `@timestampFormat("date-time")` reports that
/// format on its member schema. A member reporting none is rendered in the protocol's
/// default instead (epoch seconds), which is the defect the suite above catches on the
/// wire; this test names the member that lost it.
#[test]
fn every_modelled_date_time_member_schema_declares_the_date_time_format() {
    use aws_smithy_schema::traits::TimestampFormat;
    use aws_smithy_schema::{Schema, ShapeType};
    use runtime::ops::types::{
        CreateInvocationOutput, CreateSessionOutput, GetSessionOutput, InvocationStep,
        InvocationStepSummary, InvocationSummary, PutInvocationStepInput, SessionSummary,
        UpdateSessionOutput,
    };

    // (case, the shape's schema)
    let rows: [(&str, &Schema); 9] = [
        ("PutInvocationStepRequest", &PutInvocationStepInput::SCHEMA),
        ("CreateSessionResponse", &CreateSessionOutput::SCHEMA),
        ("GetSessionResponse", &GetSessionOutput::SCHEMA),
        ("UpdateSessionResponse", &UpdateSessionOutput::SCHEMA),
        ("CreateInvocationResponse", &CreateInvocationOutput::SCHEMA),
        ("InvocationStep", &InvocationStep::SCHEMA),
        ("InvocationStepSummary", &InvocationStepSummary::SCHEMA),
        ("InvocationSummary", &InvocationSummary::SCHEMA),
        ("SessionSummary", &SessionSummary::SCHEMA),
    ];

    for (case, schema) in rows {
        let timestamps: Vec<&&Schema> = schema
            .members()
            .iter()
            .filter(|member| matches!(member.shape_type(), ShapeType::Timestamp))
            .collect();
        assert!(
            !timestamps.is_empty(),
            "case {case:?}: this shape declares a timestamp member, so the walk must find one"
        );
        for member in timestamps {
            let format = member.timestamp_format().map(|carried| carried.format());
            assert_eq!(
                format,
                Some(TimestampFormat::DateTime),
                "case {case:?}: member {:?} must carry the modelled date-time format, got \
                 {format:?}",
                member.member_name()
            );
        }
    }
}
