// SPDX-License-Identifier: CC0-1.0

//! What `BatchDeleteImage` reports for the entries it could not process, driven end to end
//! over the wire.
//!
//! The rule under test: a batch answers with one `failures` entry per request entry it
//! could not process, carrying the `imageId` members the caller supplied, the
//! `ImageFailureCode` for what went wrong and the reason fixed for that code — and the
//! entries the REQUEST itself got wrong (an identifier naming neither a tag nor a digest,
//! which addresses no image whatever the registry holds) are reported before every entry a
//! lookup against the repository could not match, each group in request order. Source for
//! the shape: `smithy_query(query="operation", operation="BatchDeleteImage")` — `failures`
//! -> `ImageFailureList` of `ImageFailure{imageId, failureCode, failureReason}`, with
//! `imageIds` required and `@length(1..100)`.
//!
//! The rows differ along what the rule generalises over: which failure codes the batch
//! carries, the position of each offending entry in the request, how many entries fail, the
//! members each entry names, and whether entries that succeed sit between them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_image_lifecycle::{digest_of, ok, service};

/// The repository every row is driven against.
const REPOSITORY: &str = "batch/deletes";

/// A tag the seeded repository carries, so a row can name an entry that resolves.
const PRESENT_TAG: &str = "present";

/// A second tag of the seeded repository, used by the row whose digest and tag disagree.
const OTHER_TAG: &str = "other";

/// A well-formed `sha256` digest no push ever produced.
fn absent_digest() -> String {
    format!("sha256:{}", "0".repeat(64))
}

/// The reason reported beside each `ImageFailureCode`: fixed per code, with no part of it
/// taken from the request.
fn reason_for(code: &str) -> &'static str {
    match code {
        "MissingDigestAndTag" => "Invalid request parameters: both tag and digest cannot be null",
        "ImageNotFound" => "Requested image not found",
        "ImageTagDoesNotMatchDigest" => {
            "Requested image tag does not match the specified image digest"
        }
        other => panic!("no reason is stated for the failure code {other:?}"),
    }
}

/// One row: the `imageIds` a request carries, and the failures it must answer with, in
/// order — each as the code and the identifier the failure has to echo.
struct BatchCase {
    case: &'static str,
    image_ids: Vec<Value>,
    expected: Vec<(&'static str, Value)>,
}

/// Seed a repository holding one manifest under two tags, and answer the manifest's digest.
async fn seeded(router: &MultiServiceRouter) -> String {
    ok(
        router,
        "CreateRepository",
        json!({"repositoryName": REPOSITORY}),
        "seed",
    )
    .await;
    let manifest = json!({"schemaVersion": 2, "name": "batch-probe"}).to_string();
    for tag in [PRESENT_TAG, OTHER_TAG] {
        ok(
            router,
            "PutImage",
            json!({
                "repositoryName": REPOSITORY,
                "imageManifest": manifest,
                "imageTag": tag,
            }),
            "seed",
        )
        .await;
    }
    digest_of(&manifest)
}

fn batch_cases(digest: &str) -> Vec<BatchCase> {
    let absent = absent_digest();
    vec![
        BatchCase {
            // The recorded instance: a digest that names nothing, then an entry naming
            // neither member. The unusable identifier is reported first even though it was
            // submitted second.
            case: "a digest that names nothing, then an entry naming neither",
            image_ids: vec![json!({"imageDigest": absent.clone()}), json!({})],
            expected: vec![
                ("MissingDigestAndTag", json!({})),
                ("ImageNotFound", json!({"imageDigest": absent.clone()})),
            ],
        },
        BatchCase {
            // The same two entries the other way round: the answer's order does not depend
            // on which of them the caller listed first.
            case: "an entry naming neither, then a digest that names nothing",
            image_ids: vec![json!({}), json!({"imageDigest": absent.clone()})],
            expected: vec![
                ("MissingDigestAndTag", json!({})),
                ("ImageNotFound", json!({"imageDigest": absent.clone()})),
            ],
        },
        BatchCase {
            case: "two entries naming neither, around two that name nothing",
            image_ids: vec![
                json!({"imageTag": "ghost"}),
                json!({}),
                json!({"imageDigest": absent.clone()}),
                json!({}),
            ],
            expected: vec![
                ("MissingDigestAndTag", json!({})),
                ("MissingDigestAndTag", json!({})),
                ("ImageNotFound", json!({"imageTag": "ghost"})),
                ("ImageNotFound", json!({"imageDigest": absent.clone()})),
            ],
        },
        BatchCase {
            case: "a tag that names nothing, which echoes the tag it was given",
            image_ids: vec![json!({"imageTag": "ghost"})],
            expected: vec![("ImageNotFound", json!({"imageTag": "ghost"}))],
        },
        BatchCase {
            case: "a digest and a tag that disagree, which echoes both",
            image_ids: vec![json!({"imageDigest": absent.clone(), "imageTag": OTHER_TAG})],
            expected: vec![(
                "ImageTagDoesNotMatchDigest",
                json!({"imageDigest": absent.clone(), "imageTag": OTHER_TAG}),
            )],
        },
        BatchCase {
            // An entry that resolves sits between the two that do not: a success is not a
            // failure, so it moves neither of them.
            case: "an entry that resolves between two that do not",
            image_ids: vec![
                json!({"imageDigest": absent.clone()}),
                json!({"imageTag": PRESENT_TAG}),
                json!({}),
            ],
            expected: vec![
                ("MissingDigestAndTag", json!({})),
                ("ImageNotFound", json!({"imageDigest": absent.clone()})),
            ],
        },
        BatchCase {
            case: "one entry that resolves and nothing else",
            image_ids: vec![json!({"imageDigest": digest.to_string()})],
            expected: vec![],
        },
    ]
}

#[tokio::test]
async fn a_batch_reports_its_failures_unusable_identifiers_first_each_with_its_own_reason() {
    for batch_case in batch_cases("placeholder") {
        // One repository per row, so a row that deletes an image does not decide the next.
        let router = service();
        let digest = seeded(&router).await;
        let case = batch_case.case;
        // The digest-bearing row is the only one that names the stored manifest, and the
        // digest is only known once the seed has pushed it.
        let image_ids: Vec<Value> = batch_case
            .image_ids
            .iter()
            .map(|id| {
                if id["imageDigest"] == json!("placeholder") {
                    json!({"imageDigest": digest.clone()})
                } else {
                    id.clone()
                }
            })
            .collect();

        let reply = ok(
            &router,
            "BatchDeleteImage",
            json!({"repositoryName": REPOSITORY, "imageIds": image_ids}),
            case,
        )
        .await;
        let failures = reply["failures"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a failures array, got {reply}"))
            .clone();

        assert_eq!(
            failures.len(),
            batch_case.expected.len(),
            "case {case:?}: expected {} failures, got {reply}",
            batch_case.expected.len()
        );
        for (position, (code, identifier)) in batch_case.expected.iter().enumerate() {
            let failure = &failures[position];
            assert_eq!(
                failure["failureCode"],
                json!(code),
                "case {case:?}: expected {code} at failures[{position}], got {failure}"
            );
            assert_eq!(
                failure["failureReason"],
                json!(reason_for(code)),
                "case {case:?}: expected the reason fixed for {code} at \
                 failures[{position}], got {failure}"
            );
            assert_eq!(
                failure["imageId"],
                identifier.clone(),
                "case {case:?}: expected the submitted identifier at failures[{position}], \
                 got {failure}"
            );
        }
    }
}
