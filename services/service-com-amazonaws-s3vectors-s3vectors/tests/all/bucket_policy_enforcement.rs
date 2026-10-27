// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ENFORCEMENT of a vector bucket policy, served over the real
//! `restJson1` wire through the router production uses. (What a put ACCEPTS and a get
//! reports is `bucket_policy.rs`; this suite is about what the stored document then does.)
//!
//! The rule every row asserts: before an operation acts on a vector bucket or on anything
//! inside it, the bucket's stored resource policy is evaluated for that operation's IAM
//! action (`s3vectors:<OperationName>`) and the resource the call acts on, and a statement
//! that explicitly DENIES it refuses the call with `AccessDeniedException` (HTTP 403) —
//! whatever else the document allows. A policy that merely fails to grant refuses nothing:
//! an explicit deny is the only decision a resource policy takes here, which is why an
//! allow-only document and a deny aimed at another principal both leave every call served.
//!
//! The rows differ along what the rule generalises over: which action element the statement
//! carries (one action, a list, a wildcard, a wildcard prefix, `NotAction`), which resource
//! it names (the index's ARN, the bucket's, a wildcard, none at all), which principal it
//! names (`*`, the caller's account, another account), its `Effect`, and which operation
//! the call then makes — across the data plane, the index and bucket control plane, and
//! the tagging surface.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{
    call, error_code, expected_bucket_arn, expected_index_arn, list_tags, ok, router, tag, untag,
};

/// The bucket, index and vector every row is driven against.
const BUCKET: &str = "policy-enforced-bucket";
const INDEX: &str = "policy-enforced-index";
const PROBE: &str = "probe";

/// A fresh router holding one bucket, one index in it and one vector in the index, with
/// one tag on the bucket so a removal has something to remove.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": BUCKET }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "dataType": "float32",
            "dimension": 3,
            "distanceMetric": "cosine",
        }),
    )
    .await;
    ok(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "vectors": [{ "key": PROBE, "data": { "float32": [1.0, 0.0, 0.0] } }],
        }),
    )
    .await;
    let (status, body) = tag(
        &router,
        &expected_bucket_arn(BUCKET),
        &json!({ "team": "web" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture's TagResource must succeed, got {status} {body}"
    );
    router
}

/// Store `policy` on the fixture's bucket.
async fn put_policy(router: &MultiServiceRouter, case: &str, policy: &Value) {
    let (status, body) = call(
        router,
        "PutVectorBucketPolicy",
        &json!({ "vectorBucketName": BUCKET, "policy": policy.to_string() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutVectorBucketPolicy must accept the document, got {status} {body}"
    );
}

/// Make one call of `operation` against the fixture, in the form its `@http` binding and
/// required members take, and report its status and body.
///
/// Every operation the policy decision covers is reachable here, so a row can name any of
/// them; the tagging three bind `/tags/{resourceArn}` rather than `/<OperationName>`.
async fn attempt(router: &MultiServiceRouter, operation: &str) -> (u16, Value) {
    let index_target = json!({ "vectorBucketName": BUCKET, "indexName": INDEX });
    match operation {
        "ListVectors" | "GetIndex" | "DeleteIndex" => call(router, operation, &index_target).await,
        "ListIndexes" | "GetVectorBucket" | "DeleteVectorBucket" => {
            call(router, operation, &json!({ "vectorBucketName": BUCKET })).await
        }
        "GetVectors" | "DeleteVectors" => {
            call(
                router,
                operation,
                &json!({
                    "vectorBucketName": BUCKET,
                    "indexName": INDEX,
                    "keys": [PROBE],
                }),
            )
            .await
        }
        "QueryVectors" => {
            call(
                router,
                operation,
                &json!({
                    "vectorBucketName": BUCKET,
                    "indexName": INDEX,
                    "topK": 1,
                    "queryVector": { "float32": [1.0, 0.0, 0.0] },
                }),
            )
            .await
        }
        "PutVectors" => {
            call(
                router,
                operation,
                &json!({
                    "vectorBucketName": BUCKET,
                    "indexName": INDEX,
                    "vectors": [{ "key": "written", "data": { "float32": [0.0, 1.0, 0.0] } }],
                }),
            )
            .await
        }
        "CreateIndex" => {
            call(
                router,
                operation,
                &json!({
                    "vectorBucketName": BUCKET,
                    "indexName": "policy-enforced-second",
                    "dataType": "float32",
                    "dimension": 3,
                    "distanceMetric": "cosine",
                }),
            )
            .await
        }
        "TagResource" => tag(router, &expected_bucket_arn(BUCKET), &json!({ "env": "test" })).await,
        "UntagResource" => {
            untag(
                router,
                &expected_bucket_arn(BUCKET),
                &["team".to_string()],
            )
            .await
        }
        "ListTagsForResource" => list_tags(router, &expected_bucket_arn(BUCKET)).await,
        "TagResourceOnTheIndex" => {
            tag(router, &expected_index_arn(BUCKET, INDEX), &json!({ "env": "test" })).await
        }
        other => panic!("no request form for {other}"),
    }
}

/// A statement's `Principal`, written as the caller's own account root — one of the
/// spellings a resource policy names this account's principals by.
fn account_root() -> String {
    format!("arn:aws:iam::{}:root", engine::EMULATOR_ACCOUNT_ID)
}

/// One policy row: the document stored on the bucket, and what each operation must then
/// answer — `true` for the modelled 403 refusal, `false` for a call the policy leaves
/// served (HTTP 200).
struct PolicyCase {
    case: &'static str,
    policy: Value,
    outcomes: Vec<(&'static str, bool)>,
}

/// Every operation the rule covers, as a row's `outcomes` list.
fn every_operation(refused: bool) -> Vec<(&'static str, bool)> {
    [
        "ListVectors",
        "GetVectors",
        "QueryVectors",
        "PutVectors",
        "DeleteVectors",
        "GetIndex",
        "ListIndexes",
        "GetVectorBucket",
        "CreateIndex",
        "DeleteIndex",
        "TagResource",
        "TagResourceOnTheIndex",
        "UntagResource",
        "ListTagsForResource",
    ]
    .into_iter()
    .map(|operation| (operation, refused))
    .collect()
}

/// The rows.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "a Deny of one action, named by the index's own ARN",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3vectors:ListVectors",
                    "Resource": expected_index_arn(BUCKET, INDEX),
                }],
            }),
            outcomes: vec![
                ("ListVectors", true),
                ("GetVectors", false),
                ("QueryVectors", false),
                ("GetIndex", false),
            ],
        },
        PolicyCase {
            case: "a Deny of one action, named by the bucket's ARN",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3vectors:ListVectors",
                    "Resource": expected_bucket_arn(BUCKET),
                }],
            }),
            outcomes: vec![("ListVectors", true), ("GetIndex", false)],
        },
        PolicyCase {
            case: "a Deny carrying no Resource at all, which covers the bucket it is on",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3vectors:GetVectors",
                }],
            }),
            outcomes: vec![("GetVectors", true), ("ListVectors", false)],
        },
        PolicyCase {
            case: "a Deny of every action of this service, on every resource",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3vectors:*",
                    "Resource": "*",
                }],
            }),
            outcomes: every_operation(true),
        },
        PolicyCase {
            case: "a Deny of an action WILDCARD PREFIX",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3vectors:List*",
                    "Resource": "*",
                }],
            }),
            outcomes: vec![
                ("ListVectors", true),
                ("ListIndexes", true),
                ("ListTagsForResource", true),
                ("GetVectors", false),
                ("GetIndex", false),
            ],
        },
        PolicyCase {
            case: "a Deny naming two actions in a list",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": ["s3vectors:GetVectors", "s3vectors:DeleteVectors"],
                    "Resource": "*",
                }],
            }),
            outcomes: vec![
                ("GetVectors", true),
                ("DeleteVectors", true),
                ("QueryVectors", false),
                ("PutVectors", false),
            ],
        },
        PolicyCase {
            case: "a Deny written as NotAction, which covers every OTHER action",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "NotAction": "s3vectors:GetIndex",
                    "Resource": "*",
                }],
            }),
            outcomes: vec![
                ("GetIndex", false),
                ("ListVectors", true),
                ("GetVectors", true),
                ("TagResource", true),
            ],
        },
        PolicyCase {
            case: "a Deny whose Principal is this account's root",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": { "AWS": account_root() },
                    "Action": "s3vectors:QueryVectors",
                    "Resource": "*",
                }],
            }),
            outcomes: vec![("QueryVectors", true), ("GetVectors", false)],
        },
        PolicyCase {
            case: "a Deny aimed at a principal of ANOTHER account",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": { "AWS": "arn:aws:iam::999999999999:root" },
                    "Action": "s3vectors:*",
                    "Resource": "*",
                }],
            }),
            outcomes: every_operation(false),
        },
        PolicyCase {
            case: "an allow-only document, which states no refusal",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3vectors:GetVectors",
                    "Resource": "*",
                }],
            }),
            outcomes: every_operation(false),
        },
        PolicyCase {
            case: "a document that grants broadly and denies one action beside it",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "AllowEverything",
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "s3vectors:*",
                        "Resource": "*",
                    },
                    {
                        "Sid": "DenyTheWrites",
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "s3vectors:PutVectors",
                        "Resource": "*",
                    },
                ],
            }),
            outcomes: vec![
                ("PutVectors", true),
                ("GetVectors", false),
                ("DeleteVectors", false),
            ],
        },
        PolicyCase {
            case: "a Deny of the bucket's own deletion",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3vectors:DeleteVectorBucket",
                    "Resource": expected_bucket_arn(BUCKET),
                }],
            }),
            outcomes: vec![("DeleteVectorBucket", true), ("GetVectorBucket", false)],
        },
        PolicyCase {
            case: "a Deny conditioned on a key this service does not model, \
                   which is never weakened into permission",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3vectors:ListVectors",
                    "Resource": "*",
                    "Condition": { "StringEquals": { "aws:PrincipalTag/team": "web" } },
                }],
            }),
            outcomes: vec![("ListVectors", true), ("GetVectors", false)],
        },
    ]
}

/// Each policy document refuses exactly the calls its statements deny, and leaves every
/// other call served.
#[tokio::test]
async fn a_stored_bucket_policy_refuses_exactly_the_calls_its_deny_covers() {
    for row in policy_cases() {
        let PolicyCase {
            case,
            policy,
            outcomes,
        } = row;
        for (operation, refused) in outcomes {
            // A fresh fixture per call, so a row's mutating operations cannot change what
            // the next one is answered.
            let router = fixture().await;
            put_policy(&router, case, &policy).await;

            let (status, body) = attempt(&router, operation).await;
            if refused {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (403, "AccessDeniedException"),
                    "case {case:?}: {operation} must be refused, got {status} {body}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: {operation} must stay served, got {status} {body}"
                );
            }
        }
    }
}

/// A refusal says who was refused, which action was asked for and which resource it was
/// asked for — the resource the call acts on, which is the index for an index-level
/// action and the bucket for a bucket-level one.
#[tokio::test]
async fn a_refusal_names_the_caller_the_action_and_the_resource_acted_on() {
    let router = fixture().await;
    put_policy(
        &router,
        "deny everything",
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "s3vectors:*",
                "Resource": "*",
            }],
        }),
    )
    .await;

    // (operation, the action the message must name, the resource it must name)
    let cases = [
        (
            "GetVectors",
            "s3vectors:GetVectors",
            expected_index_arn(BUCKET, INDEX),
        ),
        (
            "ListVectors",
            "s3vectors:ListVectors",
            expected_index_arn(BUCKET, INDEX),
        ),
        (
            "GetVectorBucket",
            "s3vectors:GetVectorBucket",
            expected_bucket_arn(BUCKET),
        ),
        (
            "ListIndexes",
            "s3vectors:ListIndexes",
            expected_bucket_arn(BUCKET),
        ),
        (
            "TagResourceOnTheIndex",
            "s3vectors:TagResource",
            expected_index_arn(BUCKET, INDEX),
        ),
    ];

    for (operation, action, resource) in cases {
        let (status, body) = attempt(&router, operation).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (403, "AccessDeniedException"),
            "case {operation:?}: must be refused, got {status} {body}"
        );
        let message = body
            .get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let expected = format!(
            "is not authorized to perform: {action} on resource: {resource} \
             with an explicit deny in a resource-based policy"
        );
        assert!(
            message.starts_with("User: arn:aws:iam::") && message.ends_with(&expected),
            "case {operation:?}: the refusal must name the caller, {action} and {resource}, \
             got {message:?}"
        );
    }
}

/// The policy operations themselves are the bucket owner's: a document that denies
/// everything does not lock its own bucket's policy away, so replacing it with an
/// allow-only document restores access.
#[tokio::test]
async fn a_deny_of_everything_still_leaves_the_policy_itself_readable_and_replaceable() {
    let router = fixture().await;
    let deny_everything = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": "*",
            "Action": "s3vectors:*",
            "Resource": "*",
        }],
    });
    put_policy(&router, "deny everything", &deny_everything).await;

    let (status, body) = attempt(&router, "ListVectors").await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "AccessDeniedException"),
        "the deny must be in force, got {status} {body}"
    );

    // The document is still readable…
    let reported = ok(
        &router,
        "GetVectorBucketPolicy",
        &json!({ "vectorBucketName": BUCKET }),
    )
    .await;
    assert_eq!(
        reported["policy"].as_str(),
        Some(deny_everything.to_string().as_str()),
        "the stored policy must still be readable, got {reported}"
    );

    // …and replaceable, which restores access.
    put_policy(
        &router,
        "allow only",
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": "*",
                "Action": "s3vectors:*",
                "Resource": "*",
            }],
        }),
    )
    .await;
    let (status, body) = attempt(&router, "ListVectors").await;
    assert_eq!(
        status, 200,
        "replacing the policy must restore access, got {status} {body}"
    );

    // Deleting the policy outright leaves nothing to refuse either.
    put_policy(&router, "deny everything again", &deny_everything).await;
    ok(
        &router,
        "DeleteVectorBucketPolicy",
        &json!({ "vectorBucketName": BUCKET }),
    )
    .await;
    let (status, body) = attempt(&router, "ListVectors").await;
    assert_eq!(
        status, 200,
        "a bucket with no policy refuses nothing, got {status} {body}"
    );
}

/// A policy decides calls against ITS bucket and nothing else: a deny on one bucket
/// leaves another bucket's identical calls served.
#[tokio::test]
async fn a_policy_decides_only_the_bucket_it_is_attached_to() {
    let router = fixture().await;
    let sibling = "policy-enforced-sibling";
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": sibling }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": sibling,
            "indexName": INDEX,
            "dataType": "float32",
            "dimension": 3,
            "distanceMetric": "cosine",
        }),
    )
    .await;

    put_policy(
        &router,
        "deny the listing of vectors",
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "s3vectors:ListVectors",
                "Resource": "*",
            }],
        }),
    )
    .await;

    let (status, body) = attempt(&router, "ListVectors").await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "AccessDeniedException"),
        "the policy's own bucket must be refused, got {status} {body}"
    );

    let (status, body) = call(
        &router,
        "ListVectors",
        &json!({ "vectorBucketName": sibling, "indexName": INDEX }),
    )
    .await;
    assert_eq!(
        status, 200,
        "another bucket carries no policy and is served, got {status} {body}"
    );
}

/// ListVectors asking for vector data or metadata stands on `s3vectors:GetVectors` as
/// well as on its own action, so a deny of GetVectors alone refuses exactly those
/// requests and leaves a key-only listing served.
#[tokio::test]
async fn listing_vector_data_stands_on_the_get_vectors_action_too() {
    let router = fixture().await;
    put_policy(
        &router,
        "deny GetVectors",
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "s3vectors:GetVectors",
                "Resource": "*",
            }],
        }),
    )
    .await;

    // (case, the request's flags, refused)
    let cases = [
        ("neither flag", json!({}), false),
        ("returnData", json!({ "returnData": true }), true),
        ("returnMetadata", json!({ "returnMetadata": true }), true),
        (
            "both flags",
            json!({ "returnData": true, "returnMetadata": true }),
            true,
        ),
        (
            "both flags set to false",
            json!({ "returnData": false, "returnMetadata": false }),
            false,
        ),
    ];

    for (case, flags, refused) in cases {
        let mut request = json!({ "vectorBucketName": BUCKET, "indexName": INDEX });
        for (member, value) in flags.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        let (status, body) = call(&router, "ListVectors", &request).await;
        if refused {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: a listing of vector data must be refused, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a listing of keys alone must stay served, got {status} {body}"
            );
        }
    }
}
