// SPDX-License-Identifier: CC0-1.0

//! Which error SHAPE each layer of this service names when it refuses a request, end to
//! end over the real router.
//!
//! restJson1 resolves the error shape a client branches on from the `x-amzn-errortype`
//! response header (and this service renders the same name in the body's `__type`). The
//! protocol admits three spellings of one shape — `Shape`, `namespace#Shape` and
//! `Shape:<namespace URI>` — and a client resolves the name out of any of them, so what
//! a refusal must get right is the SHAPE, not a spelling.
//!
//! The rule under test: the shape a refusal names is the shape of the LAYER that refused
//! the request.
//!
//! * A request naming a resource ARN of ANOTHER VENDOR names a resource this service's
//!   API cannot address at all. It is refused ahead of this service's own input
//!   validation, and the shape it names is the front door's generic bad-request shape,
//!   which no operation of this service declares.
//! * Everything this service refuses about a value of its OWN surface names the modelled
//!   shape it declares for that refusal: `ValidationException` (HTTP 400) for a value it
//!   will not accept, `ResourceNotFoundException` (HTTP 404) for a resource that does not
//!   exist.
//! * A success names no shape at all: a client must not resolve an error out of a 200.
//!
//! Every case runs on all three operations that take a `resourceArn` (`TagResource`,
//! `UntagResource` and `ListTagsForResource` all bind it to `/tags/{resourceArn}`),
//! because the rule belongs to the layer and not to one handler. The tagging BEHAVIOUR
//! those operations implement is the subject of `resource_tags.rs`; this suite asserts
//! only the identity each refusal carries.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// Shared with the lifecycle suite rather than written twice; `urlencode` percent-encodes
// one path segment, so an ARN carrying `:` and `/` stays the single `{resourceArn}` label
// of `/tags/{resourceArn}`.
use super::application_lifecycle::{
    call, emulator, get_application, started_application, urlencode, Answer,
};

/// The region the shared harness signs its requests for, and the region every minted ARN
/// names.
const REGION: &str = "eu-west-1";

/// The account id every emulator ARN carries (`engine::emulator_account_id`).
const ACCOUNT_ID: &str = "000000000000";

/// The front door's generic bad-request shape: the identity a resource ARN of another
/// vendor is refused with. No operation of this service declares it — the request never
/// reaches one.
const BAD_REQUEST: &str = "BadRequestException";

/// The client-input shape every operation of this service declares for a value of its own
/// surface it will not accept (`smithy_query(query="operation",…)` lists it with
/// `httpError` 400).
const VALIDATION: &str = "ValidationException";

/// The shape every one of these operations declares for a resource that does not exist
/// (`httpError` 404).
const NOT_FOUND: &str = "ResourceNotFoundException";

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/tags/{}", urlencode(resource_arn)),
        None,
    )
    .await
}

async fn tag_resource(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", urlencode(resource_arn)),
        Some(json!({ "tags": { "owner": "team-a" } })),
    )
    .await
}

async fn untag_resource(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/tags/{}?tagKeys=owner", urlencode(resource_arn)),
        None,
    )
    .await
}

/// The same `resourceArn` sent to each of the three operations that bind it.
async fn answers(router: &MultiServiceRouter, resource_arn: &str) -> Vec<(&'static str, Answer)> {
    vec![
        ("ListTagsForResource", list_tags(router, resource_arn).await),
        ("TagResource", tag_resource(router, resource_arn).await),
        ("UntagResource", untag_resource(router, resource_arn).await),
    ]
}

/// The rule: every refusal carries the error-type identity of the layer that refused the
/// request, and a success carries none.
///
/// The rows span the refusal classes a request carrying a resource ARN can take, each on
/// all three operations:
///
/// * ARNs of other vendors — several vendors, two partitions, with and without a region,
///   over and under `ResourceArn`'s `length 60..1024` — all refused by the vendor check,
///   so the class is asserted over the ARN shapes it generalises over rather than one;
/// * values of this service's own surface the member does not admit: this vendor with a
///   resource type `ResourceArn`'s pattern does not name, a value that is not an ARN at
///   all, and one under the published length minimum;
/// * an ARN that satisfies the member and names no stored resource.
#[tokio::test]
async fn every_refusal_names_the_error_shape_of_the_layer_that_refused_it() {
    let router = emulator();
    let application_id = started_application(&router, "tok-identity").await;
    // The ARN the service minted and STORED, read back through the resource's own
    // report rather than spelled out here.
    let stored = get_application(&router, &application_id).await;
    let stored_application_arn = stored
        .at("/application/arn")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, resource ARN, expected status, expected resolved shape)
    let mut rows: Vec<(String, String, u16, &str)> = Vec::new();

    // Another vendor: the front door's refusal, whatever the ARN's other segments say.
    for (case, arn) in [
        (
            "an S3 bucket ARN (no region, no account)",
            "arn:aws:s3:::emr-serverless-artifacts-bucket-000000000000000".to_string(),
        ),
        (
            "an IAM role ARN",
            "arn:aws:iam::000000000000:role/EMRServerlessExecutionRoleForTests".to_string(),
        ),
        (
            // `emr` is a different vendor from `emr-serverless`, prefix or not.
            "an EMR cluster ARN",
            format!("arn:aws:emr:{REGION}:{ACCOUNT_ID}:cluster/j-000000000000000000001"),
        ),
        (
            "a Lambda ARN in another partition",
            "arn:aws-cn:lambda:cn-north-1:000000000000:function:emr-serverless-trigger".to_string(),
        ),
        (
            // Under `ResourceArn`'s length minimum: the vendor is still readable, and
            // the vendor check is ahead of the member's own constraints.
            "an S3 ARN under the member's minimum length",
            "arn:aws:s3:::b".to_string(),
        ),
    ] {
        rows.push((case.to_string(), arn, 400, BAD_REQUEST));
    }

    // This service's own surface: the member's constraints, refused with the modelled
    // client-input shape.
    for (case, arn) in [
        (
            "this vendor with a resource type the member's pattern does not name",
            format!("arn:aws:emr-serverless:{REGION}:{ACCOUNT_ID}:/clusters/0000000000000001"),
        ),
        (
            "a value that is not an ARN at all",
            "this-is-not-an-arn-but-it-is-long-enough-to-pass-the-length-check".to_string(),
        ),
        (
            // The region segment is `.+`, so a one-character region keeps the ARN well
            // formed while taking it under the published `length 60..1024` minimum.
            "this vendor under the member's minimum length",
            "arn:aws:emr-serverless:x:000000000000:/applications/a".to_string(),
        ),
    ] {
        rows.push((case.to_string(), arn, 400, VALIDATION));
    }

    // The member is satisfied and the resource does not exist: the handler's own 404.
    rows.push((
        "an ARN naming no stored resource".to_string(),
        format!(
            "arn:aws:emr-serverless:{REGION}:{ACCOUNT_ID}:/applications/\
             00fakeidnonexistent0000000000000"
        ),
        404,
        NOT_FOUND,
    ));

    for (case, resource_arn, status, shape) in rows {
        for (operation, answer) in answers(&router, &resource_arn).await {
            let case = format!("{operation}({case})");
            assert_eq!(
                answer.status, status,
                "case {case:?}: expected {status}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.resolved_error_shape(),
                Some(shape),
                "case {case:?}: expected an identity resolving to {shape}, got {:?}",
                answer.error_type
            );
            // The body names the same shape, so a client reading either one branches the
            // same way.
            assert_eq!(
                answer.error_code(),
                shape,
                "case {case:?}: expected the body to name {shape}, got {:?}",
                answer.body
            );
        }
    }

    // A served request names no error shape at all.
    let listed = list_tags(&router, &stored_application_arn).await;
    assert_eq!(
        listed.status, 200,
        "case 'a stored application': expected 200, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.resolved_error_shape(),
        None,
        "case 'a stored application': a success carries no error-type identity, got {:?}",
        listed.error_type
    );
    assert_eq!(
        listed.at("/tags"),
        Value::Object(serde_json::Map::new()),
        "case 'a stored application': expected an empty tag map, got {:?}",
        listed.body
    );
}
