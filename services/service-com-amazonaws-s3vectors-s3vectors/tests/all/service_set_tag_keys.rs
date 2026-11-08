// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the OTHER half of the reserved tag-key rule: the tags AWS sets
//! from inside itself.
//!
//! RULE: a tag key in the reserved `aws:` namespace is AWS's own — so a request a CLIENT
//! makes may neither write nor remove one (`reserved_tag_keys.rs` pins that half), while
//! the service that owns such a tag sets it through no public API at all. In this emulator
//! one service reaches another only over the in-process callback, so a request that arrives
//! on that path may carry a reserved key, and every later read of the resource reports it
//! beside the client's own tags: `ListTagsForResource` on the handle the owner published
//! answers with the whole set.
//!
//! The rows differ along what the rule generalises over: the reserved NAMESPACE the key
//! sits in (the stack provenance a CloudFormation deployment stamps, and another service's
//! own namespace the issue did not cite), the case the prefix is spelled in, a key that
//! merely CONTAINS the prefix without being in the namespace, the operation the key arrives
//! on (`CreateVectorBucket$tags`, `CreateIndex$tags`, `TagResource`, `UntagResource`) and
//! the KIND of resource tagged (vector bucket, vector index).
//!
//! Every case drives real HTTP through the production front door
//! (`MultiServiceRouter::dispatch`), so what is asserted is what a client sees; the
//! in-process path is the one the framework's callback transport stamps
//! (`smithy_server_runtime::CallbackRecursionDepth`), which is a request EXTENSION and
//! therefore cannot arrive from a socket.

use serde_json::{json, Value};
use smithy_server_runtime::{CallbackRecursionDepth, MultiServiceRouter};

use service_com_amazonaws_s3vectors_s3vectors::state::SYSTEM_TAG_REFUSED;

use crate::vector_lifecycle::{
    encode_label, error_code, expected_bucket_arn, expected_index_arn, pairs, reported_tags,
    router, tag_object, AUTHORIZATION, REGION,
};

/// Which path a request arrives on, which is what decides whether a reserved tag key is
/// admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    /// A client's own request, arriving from a socket: it carries no in-process stamp.
    Caller,
    /// A request another emulated service made in-process, which the framework's callback
    /// transport stamps.
    InProcess,
}

/// Dispatch one request on `path`, by the method and URI the operation's `@http` binding
/// declares.
///
/// The same request either way but for the stamp: an in-process call carries the
/// [`CallbackRecursionDepth`] extension the callback transport puts on it, which is the
/// one thing about a request a client cannot choose for itself.
async fn dispatch(
    router: &MultiServiceRouter,
    path: Path,
    method: &str,
    path_and_query: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = match body {
        Some(body) => {
            bytes::Bytes::from(serde_json::to_vec(body).expect("the request body serializes"))
        }
        None => bytes::Bytes::new(),
    };
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://s3vectors.{REGION}.amazonaws.com{path_and_query}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    if path == Path::InProcess {
        request = request.extension(CallbackRecursionDepth(1));
    }
    let response = router
        .dispatch(request.body(payload).expect("a well-formed request"))
        .await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `POST /<Operation>` on `path`.
async fn operation(
    router: &MultiServiceRouter,
    path: Path,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    dispatch(router, path, "POST", &format!("/{operation}"), Some(body)).await
}

/// `POST /tags/{resourceArn}` on `path` — `TagResource`.
async fn tag_on(
    router: &MultiServiceRouter,
    path: Path,
    arn: &str,
    tags: &Value,
) -> (u16, Value) {
    dispatch(
        router,
        path,
        "POST",
        &format!("/tags/{}", encode_label(arn)),
        Some(&json!({ "tags": tags })),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` on `path` — `UntagResource`.
async fn untag_on(
    router: &MultiServiceRouter,
    path: Path,
    arn: &str,
    tag_keys: &[&str],
) -> (u16, Value) {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect::<Vec<String>>()
        .join("&");
    dispatch(
        router,
        path,
        "DELETE",
        &format!("/tags/{}?{query}", encode_label(arn)),
        None,
    )
    .await
}

/// One row: a tag key, and whether it is in the namespace AWS keeps for itself.
struct Row {
    case: &'static str,
    key: &'static str,
    /// Whether the key is in the reserved namespace, so a client's request may not name
    /// it and an in-process one may.
    reserved: bool,
}

/// The rows the rule generalises over.
///
/// The three stack-provenance keys are the ones CloudFormation records on every resource
/// it provisions (`aws:cloudformation:stack-name`, `aws:cloudformation:stack-id` and
/// `aws:cloudformation:logical-id`, this emulator's
/// `service-com-amazonaws-cloudformation-cloudformation`
/// `state::provisioning::{STACK_NAME_TAG, STACK_ID_TAG, LOGICAL_ID_TAG}`); the Service
/// Catalog key is another service's own namespace, which the issue did not cite. The last
/// two rows are the boundary: the prefix in another case is still the namespace, and a key
/// that merely contains `aws:` is not in it at all.
const ROWS: &[Row] = &[
    Row {
        case: "the stack a resource belongs to",
        key: "aws:cloudformation:stack-name",
        reserved: true,
    },
    Row {
        case: "that stack's id",
        key: "aws:cloudformation:stack-id",
        reserved: true,
    },
    Row {
        case: "the resource's logical id in the template",
        key: "aws:cloudformation:logical-id",
        reserved: true,
    },
    Row {
        case: "another service's own namespace",
        key: "aws:servicecatalog:provisioningPrincipalArn",
        reserved: true,
    },
    Row {
        case: "the bare reserved prefix as the whole key",
        key: "aws:",
        reserved: true,
    },
    Row {
        case: "the reserved prefix in mixed case",
        key: "Aws:Cloudformation:Stack-Name",
        reserved: true,
    },
    Row {
        case: "a key that merely contains the prefix",
        key: "not-aws:prefixed",
        reserved: false,
    },
    Row {
        case: "a client's own key",
        key: "env",
        reserved: false,
    },
];

/// The value every row writes. A tag VALUE is a value either way: the rule is about the
/// KEY, so one value serves every row.
const VALUE: &str = "probe-value";

/// A key a client owns, written beside the row's key so each case also shows the other
/// half of the request travelled.
const CLIENT_KEY: &str = "team";

/// `CreateVectorBucket$tags` and `CreateIndex$tags`: a reserved key is refused on a
/// client's create and stored on an in-process one, and the create that was refused
/// created nothing.
#[tokio::test]
async fn a_create_takes_a_reserved_tag_key_only_on_the_in_process_path() {
    for row in ROWS {
        let case = row.case;
        for path in [Path::Caller, Path::InProcess] {
            let admitted = !row.reserved || path == Path::InProcess;
            let expected = tag_object(&pairs(&[(row.key, VALUE), (CLIENT_KEY, "search")]));
            let router = router();

            // The vector bucket, created with the row's tags on the path under test.
            let (status, body) = operation(
                &router,
                path,
                "CreateVectorBucket",
                &json!({ "vectorBucketName": "provenance-bucket", "tags": expected }),
            )
            .await;
            if admitted {
                assert_eq!(
                    status, 200,
                    "case {case:?} on {path:?}: CreateVectorBucket must store the key, got \
                     {status} {body}"
                );
                let reported =
                    reported_tags(&router, case, &expected_bucket_arn("provenance-bucket")).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?} on {path:?}: the bucket's tag read must report the whole \
                     set, got {reported}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?} on {path:?}: CreateVectorBucket must refuse the key, got \
                     {status} {body}"
                );
                let (status, gone) = operation(
                    &router,
                    path,
                    "GetVectorBucket",
                    &json!({ "vectorBucketName": "provenance-bucket" }),
                )
                .await;
                assert_eq!(
                    (status, error_code(&gone).as_str()),
                    (404, "NotFoundException"),
                    "case {case:?} on {path:?}: a refused create must create nothing, got \
                     {status} {gone}"
                );
                // The index half needs a bucket, so one is created without the row's tags.
                let (status, body) = operation(
                    &router,
                    path,
                    "CreateVectorBucket",
                    &json!({ "vectorBucketName": "provenance-bucket" }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?} on {path:?}: an untagged create must succeed, got {status} \
                     {body}"
                );
            }

            // The index in it, created the same way: the rule is the member's, not one
            // operation's.
            let create_index = json!({
                "vectorBucketName": "provenance-bucket",
                "indexName": "provenance-index",
                "dataType": "float32",
                "dimension": 2,
                "distanceMetric": "cosine",
                "tags": expected,
            });
            let (status, body) = operation(&router, path, "CreateIndex", &create_index).await;
            if admitted {
                assert_eq!(
                    status, 200,
                    "case {case:?} on {path:?}: CreateIndex must store the key, got {status} \
                     {body}"
                );
                let reported = reported_tags(
                    &router,
                    case,
                    &expected_index_arn("provenance-bucket", "provenance-index"),
                )
                .await;
                assert_eq!(
                    reported, expected,
                    "case {case:?} on {path:?}: the index's tag read must report the whole \
                     set, got {reported}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?} on {path:?}: CreateIndex must refuse the key, got {status} \
                     {body}"
                );
                assert_eq!(
                    body.get("message").and_then(Value::as_str),
                    Some(SYSTEM_TAG_REFUSED),
                    "case {case:?} on {path:?}: the refusal must be the service's own \
                     sentence, got {body}"
                );
            }
        }
    }
}

/// `TagResource` and `UntagResource`: the same line, on a resource that already exists.
/// A reserved key is refused to a client and written — then removed — on the in-process
/// path, with the client's own tags left exactly as they were either way.
#[tokio::test]
async fn the_tagging_operations_take_a_reserved_key_only_on_the_in_process_path() {
    for row in ROWS {
        let case = row.case;
        for path in [Path::Caller, Path::InProcess] {
            let admitted = !row.reserved || path == Path::InProcess;
            let router = router();
            let established = pairs(&[(CLIENT_KEY, "search")]);
            let created = tag_object(&established);

            // A vector bucket holding a vector index, both carrying the client's own tag.
            let (status, body) = operation(
                &router,
                Path::Caller,
                "CreateVectorBucket",
                &json!({ "vectorBucketName": "provenance-bucket", "tags": created }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} on {path:?}: the fixture bucket must be created, got {status} \
                 {body}"
            );
            let (status, body) = operation(
                &router,
                Path::Caller,
                "CreateIndex",
                &json!({
                    "vectorBucketName": "provenance-bucket",
                    "indexName": "provenance-index",
                    "dataType": "float32",
                    "dimension": 2,
                    "distanceMetric": "cosine",
                    "tags": created,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} on {path:?}: the fixture index must be created, got {status} \
                 {body}"
            );

            for arn in [
                expected_bucket_arn("provenance-bucket"),
                expected_index_arn("provenance-bucket", "provenance-index"),
            ] {
                let written = tag_object(&pairs(&[(row.key, VALUE)]));
                let (status, body) = tag_on(&router, path, &arn, &written).await;
                let mut expected = established.clone();
                if admitted {
                    assert_eq!(
                        status, 200,
                        "case {case:?} on {path:?}: TagResource({arn}) must store the key, \
                         got {status} {body}"
                    );
                    expected.push((row.key.to_string(), VALUE.to_string()));
                } else {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (400, "ValidationException"),
                        "case {case:?} on {path:?}: TagResource({arn}) must refuse the key, \
                         got {status} {body}"
                    );
                }
                let reported = reported_tags(&router, case, &arn).await;
                assert_eq!(
                    reported,
                    tag_object(&expected),
                    "case {case:?} on {path:?}: {arn} must carry exactly the set the write \
                     left, got {reported}"
                );

                // And the removal half: the key the write left is removed on the path that
                // may name it, and the client's own tag is untouched either way.
                let (status, body) = untag_on(&router, path, &arn, &[row.key]).await;
                if admitted {
                    assert_eq!(
                        status, 200,
                        "case {case:?} on {path:?}: UntagResource({arn}) must remove the key, \
                         got {status} {body}"
                    );
                } else {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (400, "ValidationException"),
                        "case {case:?} on {path:?}: UntagResource({arn}) must refuse the key, \
                         got {status} {body}"
                    );
                }
                let reported = reported_tags(&router, case, &arn).await;
                assert_eq!(
                    reported,
                    tag_object(&established),
                    "case {case:?} on {path:?}: {arn} must be left with the client's own tags, \
                     got {reported}"
                );
            }
        }
    }
}
