// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite pinning that the tag APIs answer an unresolvable `ResourceArn` with
//! ONE refusal, whichever of them was called.
//!
//! The rule under test: a tag operation whose `ResourceArn` matches no record this store
//! holds refuses with `ResourceNotFoundException` (HTTP 404) reporting the submitted ARN
//! as the resource that was not found and, as its hypothetical type, the type that ARN
//! claims — so the read `ListTagsForResource` and the writes `TagResource` and
//! `UntagResource` answer the SAME status, code, message, `resourceId` and `resourceType`
//! for the same ARN.
//!
//! Each row drives all three operations over one unresolvable ARN and asserts their
//! refusals agree member for member, so a refusal written out once per handler — which
//! could drift in its wording or in the type it names — fails here. The rows differ in the
//! FORM of the ARN, which is what the reported type is read off: a resource field
//! introduced by `/`, one introduced by `:`, one whose id carries further `/` segments,
//! one that is an id alone, an empty one, and a string that is not an ARN at all.
//!
//! The HTTP harness (`emulator`, `call`, `percent_encode`, `query_value`, `Answer`) is the
//! search-job suite's: every suite drives the same emulator the same way, so there is one
//! harness, not several.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::search_jobs::{call, emulator, percent_encode, query_value, Answer};

/// `GET /tags/{ResourceArn}` — `ListTagsForResource`, the read path.
async fn list_tags(emulator: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    call(
        emulator,
        region,
        "GET",
        &format!("/tags/{}", percent_encode(arn)),
        None,
    )
    .await
}

/// `POST /tags/{ResourceArn}` — `TagResource`, the merging write path.
async fn tag_resource(
    emulator: &MultiServiceRouter,
    region: &str,
    arn: &str,
    tags: Value,
) -> Answer {
    call(
        emulator,
        region,
        "POST",
        &format!("/tags/{}", percent_encode(arn)),
        Some(json!({"Tags": tags})),
    )
    .await
}

/// `DELETE /tags/{ResourceArn}?tagKeys=…` — `UntagResource`, the removing write path.
/// `TagKeys` is an `@httpQuery` list, so each key is its own repeated parameter.
async fn untag_resource(
    emulator: &MultiServiceRouter,
    region: &str,
    arn: &str,
    keys: &[&str],
) -> Answer {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect();
    call(
        emulator,
        region,
        "DELETE",
        &format!("/tags/{}?{}", percent_encode(arn), query.join("&")),
        None,
    )
    .await
}

/// The members of one refusal, so two operations' answers are compared as a whole rather
/// than member by member in three places.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    status: u16,
    error_code: Option<String>,
    message: String,
    resource_id: String,
    resource_type: String,
}

impl Refusal {
    fn of(answer: &Answer) -> Self {
        Self {
            status: answer.status,
            error_code: answer.error_code.clone(),
            message: answer.string("message"),
            resource_id: answer.string("resourceId"),
            resource_type: answer.string("resourceType"),
        }
    }
}

/// Start a search job, so the store HOLDS a taggable record: a miss then comes from the
/// ARN resolving to nothing and not from an empty store refusing everything. Answers the
/// job's minted ARN.
async fn held_search_job_arn(emulator: &MultiServiceRouter, region: &str) -> String {
    let started = call(
        emulator,
        region,
        "PUT",
        "/search-jobs",
        Some(json!({
            "Name": "refusal-seed",
            "SearchScope": {"BackupResourceTypes": ["S3"]},
            "Tags": {"env": "prod"},
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "seed: StartSearchJob must succeed, got {} ({:?})",
        started.status, started.body
    );
    started.string("SearchJobArn")
}

#[tokio::test]
async fn every_tag_operation_refuses_an_unresolvable_arn_the_same_way() {
    let region = "us-west-2";
    let emulator = emulator();
    let held = held_search_job_arn(&emulator, region).await;

    let account = engine::emulator_account_id();
    // Rows: an ARN no record holds, and the type it CLAIMS — the member is documented as
    // the "hypothetical type of the resource affected"
    // (smithy_query query="shape" shape="ResourceNotFoundException"), which for an ARN is
    // the part of its resource field before the first `/` or `:`, and nothing when that
    // field carries no separator at all.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a search-job id no search job holds",
            format!(
                "arn:aws:backup-search:{region}:{account}:search-job/11111111-2222-4333-8444-555555555555"
            ),
            "search-job",
        ),
        (
            "an export-job id no export job holds",
            format!(
                "arn:aws:backup-search:{region}:{account}:export-job/11111111-2222-4333-8444-555555555555"
            ),
            "export-job",
        ),
        (
            "an id carrying further path segments",
            format!("arn:aws:backup-search:{region}:{account}:export-job/2026/10/29/one"),
            "export-job",
        ),
        (
            "a resource field whose type is colon-separated",
            format!("arn:aws:backup:{region}:{account}:recovery-point:rp-1"),
            "recovery-point",
        ),
        (
            "a resource field that is an id alone, so it claims no type",
            "arn:aws:s3:::some-bucket".to_string(),
            "",
        ),
        (
            "an ARN whose resource field is empty",
            format!("arn:aws:backup-search:{region}:{account}:"),
            "",
        ),
        ("a string that is not an ARN", "not-an-arn".to_string(), ""),
    ];

    for (case, arn, claimed_type) in &rows {
        let answers = [
            ("ListTagsForResource", list_tags(&emulator, region, arn).await),
            (
                "TagResource",
                tag_resource(&emulator, region, arn, json!({"team": "data"})).await,
            ),
            (
                "UntagResource",
                untag_resource(&emulator, region, arn, &["env"]).await,
            ),
        ];

        // What the rule says this ARN must be refused with, independent of which
        // operation was called.
        let expected = Refusal {
            status: 404,
            error_code: Some("ResourceNotFoundException".to_string()),
            message: format!("no resource with ARN '{arn}' exists"),
            resource_id: arn.clone(),
            resource_type: (*claimed_type).to_string(),
        };

        // Each operation is held to that same expectation, which is also what makes the
        // three agree with each other — the property a refusal written out once per
        // handler loses first.
        for (operation, answer) in &answers {
            let refusal = Refusal::of(answer);
            assert_eq!(
                refusal, expected,
                "case {case:?}/{operation}: an ARN no held record carries must be refused \
                 with the one not-found refusal, got {refusal:?} ({:?})",
                answer.body
            );
        }
    }

    // A refused call is not a write: the record the store does hold still reports the tag
    // set it was created with.
    let still_held = list_tags(&emulator, region, &held).await;
    assert_eq!(
        still_held.status, 200,
        "the held search job must still resolve, got {} ({:?})",
        still_held.status, still_held.body
    );
    assert_eq!(
        still_held.get("Tags"),
        &json!({"env": "prod"}),
        "the held search job must report the tags it was created with, got {:?}",
        still_held.get("Tags")
    );
}
