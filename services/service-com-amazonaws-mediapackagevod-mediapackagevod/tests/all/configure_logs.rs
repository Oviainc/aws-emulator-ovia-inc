// SPDX-License-Identifier: CC0-1.0

//! `ConfigureLogs` end-to-end: the egress-access-log subscription of a PackagingGroup.
//!
//! Every case drives real HTTP through the router the production binary builds, so what
//! is asserted is the wire behaviour a `mediapackage-vod` client sees: the restJson1
//! route `PUT /packaging_groups/{Id}/configure_logs`, its status and its JSON body.
//!
//! The rules under test, asserted over a TABLE of subscriptions rather than one
//! recorded call:
//!
//! * the call REPLACES the addressed group's log-subscription member with the one the
//!   request carries — an absent member leaves the group unsubscribed — and persists
//!   it, so a following `DescribePackagingGroup` reports the same value;
//! * it answers the group's full current representation, and changes nothing else
//!   about it: `Arn`, `CreatedAt`, `DomainName`, `Authorization` and its tags are the
//!   ones the group was created with, however often the subscription is rewritten;
//! * it addresses ONE group: configuring one group's logs leaves every other group's
//!   subscription where it was;
//! * an `Id` this service holds no group under is refused with the modelled
//!   `NotFoundException` (HTTP 404), and nothing is created for it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; it only has to be a legal credential scope.
const REGION: &str = "us-west-2";

/// The wire error code of a missing addressed resource, and its modelled `httpError`
/// (`smithy_query(query="shape",shape="NotFoundException")`: `errorKind client`,
/// `httpError 404`).
const NOT_FOUND: (&str, u16) = ("NotFoundException", 404);

/// One answer: the HTTP status, the error shape named by `x-amzn-errortype` (how
/// restJson1 resolves an error shape, `"<unset>"` on a success) and the parsed body.
struct Answer {
    status: u16,
    errortype: String,
    body: Value,
}

impl Answer {
    /// A member of the body, or `Value::Null` when the body carries none — the
    /// distinction between a reported subscription and no subscription at all.
    fn member(&self, name: &str) -> Value {
        self.body.get(name).cloned().unwrap_or(Value::Null)
    }

    /// The `message` member the modelled error shapes spell `Message` with
    /// `jsonName: message`.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<unset>")
            .to_string()
    }
}

/// A router carrying exactly this service over one shared engine — the same assembly
/// `core/router` performs, minus the other services.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request and report what came back.
///
/// The `Authorization` header carries only a SigV4 credential SCOPE: that is what the
/// router routes on and what the handlers read the region from. Signature verification
/// is off for this router, so the signature itself is never checked.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let host = format!("mediapackage-vod.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<unset>")
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        errortype,
        body,
    }
}

/// Create a packaging group carrying CDN authorization and tags, asserting it
/// succeeded. The extra members exist so the test can prove `ConfigureLogs` leaves
/// them alone.
async fn create_group(router: &MultiServiceRouter, id: &str, body: Value) -> Value {
    let mut request = json!({ "id": id });
    let Value::Object(members) = body else {
        panic!("the extra create members must be a JSON object");
    };
    for (key, value) in members {
        request[key] = value;
    }
    let answer = call(router, "POST", "/packaging_groups", Some(request)).await;
    assert_eq!(
        answer.status, 200,
        "CreatePackagingGroup {id}: body {}",
        answer.body
    );
    answer.body
}

/// `ConfigureLogs` on `id`, carrying `egress_access_logs` when it is `Some` and no
/// `egressAccessLogs` member at all when it is `None`.
async fn configure_logs(
    router: &MultiServiceRouter,
    id: &str,
    egress_access_logs: Option<Value>,
) -> Answer {
    let body = match egress_access_logs {
        Some(logs) => json!({ "egressAccessLogs": logs }),
        None => json!({}),
    };
    call(
        router,
        "PUT",
        &format!("/packaging_groups/{id}/configure_logs"),
        Some(body),
    )
    .await
}

/// `DescribePackagingGroup` on `id`.
async fn describe_group(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/packaging_groups/{id}"), None).await
}

/// One row of the subscription table: a case name and the `egressAccessLogs` member a
/// request carries (`None` = the member is absent from the request body).
struct Row {
    case: &'static str,
    logs: Option<Value>,
}

/// The subscriptions the replacement rule is asserted over, applied in order to one
/// group so each row also replaces the row before it.
///
/// They differ along what the rule generalises over: a log group name's value, a
/// subscription that names none (`EgressAccessLogs$LogGroupName` is optional —
/// "Customize the log group name."), the member being absent altogether, and a
/// resubscribe after a clear.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a custom log group name",
            logs: Some(json!({ "logGroupName": "/aws/MediaPackage/vod-custom" })),
        },
        Row {
            case: "a different custom log group name replaces the first",
            logs: Some(json!({ "logGroupName": "my-vod-egress-logs" })),
        },
        Row {
            case: "a subscription naming no log group",
            logs: Some(json!({})),
        },
        Row {
            case: "an absent member leaves the group unsubscribed",
            logs: None,
        },
        Row {
            case: "resubscribed after a clear",
            logs: Some(json!({ "logGroupName": "/aws/MediaPackage/EgressAccessLogs" })),
        },
        Row {
            case: "a log group name at the long end of what a client may send",
            logs: Some(json!({ "logGroupName": format!("/aws/MediaPackage/{}", "g".repeat(200)) })),
        },
    ]
}

/// The `egressAccessLogs` a response must report for a request that carried `logs`:
/// exactly what was sent, and nothing when the member was absent.
fn expected(logs: &Option<Value>) -> Value {
    logs.clone().unwrap_or(Value::Null)
}

#[tokio::test]
async fn configure_logs_replaces_the_subscription_and_persists_it() {
    let router = router();
    let group_id = "vod-group-logs";
    let created = create_group(
        &router,
        group_id,
        json!({
            "authorization": {
                "cdnIdentifierSecret": "arn:aws:secretsmanager:us-west-2:111122223333:secret:cdn",
                "secretsRoleArn": "arn:aws:iam::111122223333:role/MediaPackageVodSecrets",
            },
            "tags": { "stage": "test" },
        }),
    )
    .await;

    for row in rows() {
        let answer = configure_logs(&router, group_id, row.logs.clone()).await;
        let case = row.case;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.member("egressAccessLogs"),
            expected(&row.logs),
            "case {case:?}: the response must report the subscription that was sent, got {}",
            answer.body
        );

        // Everything else about the group is the group that was created: this call
        // configures logs and rewrites nothing else.
        for member in ["id", "arn", "createdAt", "domainName", "authorization", "tags"] {
            assert_eq!(
                answer.member(member),
                created.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: {member} must be unchanged by ConfigureLogs, got {}",
                answer.member(member)
            );
        }

        // Persisted, not merely echoed: the read-back reports the same subscription.
        let described = describe_group(&router, group_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribePackagingGroup must succeed, got {} with body {}",
            described.status, described.body
        );
        assert_eq!(
            described.member("egressAccessLogs"),
            expected(&row.logs),
            "case {case:?}: the stored subscription must be the one configured, got {}",
            described.body
        );
        assert_eq!(
            described.member("authorization"),
            created.get("authorization").cloned().unwrap_or(Value::Null),
            "case {case:?}: ConfigureLogs must not disturb the stored Authorization, got {}",
            described.body
        );
        assert_eq!(
            described.member("tags"),
            created.get("tags").cloned().unwrap_or(Value::Null),
            "case {case:?}: ConfigureLogs must not disturb the stored tags, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn configure_logs_addresses_only_the_group_it_names() {
    let router = router();
    let ids = ["vod-group-a", "vod-group-b", "vod-group-c"];
    for id in ids {
        create_group(&router, id, json!({})).await;
    }

    // Each group gets its own subscription, one call at a time; after every call every
    // group must report its own.
    let mut configured: Vec<(&str, Value)> = Vec::new();
    for id in ids {
        let logs = json!({ "logGroupName": format!("/aws/MediaPackage/{id}") });
        let answer = configure_logs(&router, id, Some(logs.clone())).await;
        assert_eq!(
            answer.status, 200,
            "case {id:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        configured.push((id, logs));

        for (seen, expected_logs) in &configured {
            let described = describe_group(&router, seen).await;
            assert_eq!(
                described.member("egressAccessLogs"),
                *expected_logs,
                "case {id:?}: {seen} must still report its own subscription, got {}",
                described.body
            );
        }
    }

    // A group nobody configured still reports none, so the writes above did not leak.
    let untouched = create_group(&router, "vod-group-unconfigured", json!({})).await;
    assert_eq!(
        untouched.get("egressAccessLogs").cloned().unwrap_or(Value::Null),
        Value::Null,
        "a group created after the writes above must carry no subscription, got {untouched}"
    );
}

#[tokio::test]
async fn configure_logs_refuses_an_id_no_group_is_stored_under() {
    let router = router();
    create_group(&router, "vod-group-present", json!({})).await;

    // The ids differ in form — one never created, one created and then deleted, one a
    // suffixed near-miss of a stored one, two differing from it only in case or
    // separator — because the rule is about what the store holds, not about the id's
    // shape.
    create_group(&router, "vod-group-deleted", json!({})).await;
    let deleted = call(&router, "DELETE", "/packaging_groups/vod-group-deleted", None).await;
    // `DeletePackagingGroup` answers the recorded 204 No Content, not the 202 the
    // model declares — see `src/ops/delete_packaging_group.rs`.
    assert_eq!(
        deleted.status, 204,
        "DeletePackagingGroup must succeed, got {} with body {}",
        deleted.status, deleted.body
    );

    for id in [
        "vod-group-absent",
        "vod-group-deleted",
        "vod-group-present-",
        "VOD-GROUP-PRESENT",
        "vod_group_present",
    ] {
        let answer = configure_logs(
            &router,
            id,
            Some(json!({ "logGroupName": "/aws/MediaPackage/nope" })),
        )
        .await;
        assert_eq!(
            answer.status, NOT_FOUND.1,
            "case {id:?}: expected HTTP {}, got {} with body {}",
            NOT_FOUND.1, answer.status, answer.body
        );
        assert_eq!(
            answer.errortype, NOT_FOUND.0,
            "case {id:?}: expected error type {}, got {}",
            NOT_FOUND.0, answer.errortype
        );
        assert_eq!(
            answer.message(),
            format!("PackagingGroup with id={id} not found"),
            "case {id:?}: the refusal must bind the id the request asked for, got {}",
            answer.message()
        );

        // The refusal created nothing: the id is still unknown to the describe.
        let described = describe_group(&router, id).await;
        assert_eq!(
            described.status, NOT_FOUND.1,
            "case {id:?}: a refused ConfigureLogs must not create a group, got {} with body {}",
            described.status, described.body
        );
    }

    // And the group that does exist is untouched by those refusals.
    let described = describe_group(&router, "vod-group-present").await;
    assert_eq!(
        described.status, 200,
        "the stored group must still be readable, got {} with body {}",
        described.status, described.body
    );
    assert_eq!(
        described.member("egressAccessLogs"),
        Value::Null,
        "the stored group must carry no subscription, got {}",
        described.body
    );
}
