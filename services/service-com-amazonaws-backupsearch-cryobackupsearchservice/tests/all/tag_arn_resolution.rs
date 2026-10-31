// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite pinning that the tag APIs' READ path and WRITE path resolve a
//! `ResourceArn` through ONE resolution step.
//!
//! The rule under test: a tag operation resolves its `ResourceArn` against the held
//! records of every resource this service mints, in one order, so `ListTagsForResource`
//! (the read) and `TagResource`/`UntagResource` (the writes) reach the SAME record for
//! the same ARN and refuse exactly the same ARNs with `ResourceNotFoundException`.
//!
//! Each row drives both paths over one ARN and asserts they agree, so a resolution
//! written out once for the read and again for the write — which could resolve an ARN on
//! one path and refuse it on the other — fails here. The rows span the resource types
//! this service mints (search job and export job, the two the model gives an ARN and a
//! `TagMap`), more than one record of the same type, and the ways an ARN fails to resolve.
//!
//! The HTTP harness (`emulator`, `call`, `percent_encode`, `query_value`, `Answer`) and
//! the transition-span constants are the search-job suite's: every suite drives the same
//! emulator the same way, so there is one harness, not several.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator_with_engine, percent_encode, query_value, settle_a_span_ahead, Answer,
    PAST_THE_CREATING_INSTANT, PAST_THE_DUE_INSTANT,
};

/// `GET /tags/{ResourceArn}` — the read path, whose resolution is the one under test.
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

/// `POST /tags/{ResourceArn}` — a write path over the same resolution.
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

/// `DELETE /tags/{ResourceArn}?tagKeys=…`: `TagKeys` is an `@httpQuery` list, so each key
/// is its own repeated parameter.
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

/// The `Tags` of a successful read, as a sorted map so a comparison does not depend on
/// map ordering.
fn tags_of(answer: &Answer) -> BTreeMap<String, String> {
    answer
        .get("Tags")
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Start a search job and bring it to `STOPPED` — a settled status an export job may be
/// started from. Answers its identifier and its minted ARN.
async fn stopped_search_job(
    emulator: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    region: &str,
    case: &str,
    name: &str,
) -> (String, String) {
    let started = call(
        emulator,
        region,
        "PUT",
        "/search-jobs",
        Some(json!({"Name": name, "SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartSearchJob must succeed, got {} ({:?})",
        started.status, started.body
    );
    let identifier = started.string("SearchJobIdentifier");

    settle_a_span_ahead(engine, PAST_THE_CREATING_INSTANT);
    let stopped = call(
        emulator,
        region,
        "PUT",
        &format!("/search-jobs/{identifier}/actions/cancel"),
        None,
    )
    .await;
    assert_eq!(
        stopped.status, 200,
        "case {case:?}: StopSearchJob must succeed, got {} ({:?})",
        stopped.status, stopped.body
    );
    // The stop's end state is owed at a stored instant, so the clock is carried to it
    // rather than waited out.
    settle_a_span_ahead(engine, PAST_THE_DUE_INSTANT);

    (identifier, started.string("SearchJobArn"))
}

/// `StartSearchResultExportJob` over `search_job_identifier`, answering the export job's
/// minted ARN.
async fn export_job_arn(
    emulator: &MultiServiceRouter,
    region: &str,
    case: &str,
    search_job_identifier: &str,
) -> String {
    let started = call(
        emulator,
        region,
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "SearchJobIdentifier": search_job_identifier,
            "ExportSpecification": {
                "s3ExportSpecification": {"DestinationBucket": "search-exports"}
            },
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartSearchResultExportJob must succeed, got {} ({:?})",
        started.status, started.body
    );
    started.string("ExportJobArn")
}

#[tokio::test]
async fn the_tag_read_path_and_the_tag_write_path_resolve_the_same_arns() {
    let region = "us-west-2";
    let (emulator, engine) = emulator_with_engine();

    // Two search jobs and two export jobs, so a row can address the second record of a
    // type: the resolution has to find a record wherever it sits in the store, not only
    // the single one a one-record store leaves.
    let (first_search_job, first_search_job_arn) =
        stopped_search_job(&emulator, &engine, region, "first search job", "resolved-one").await;
    let (second_search_job, second_search_job_arn) =
        stopped_search_job(&emulator, &engine, region, "second search job", "resolved-two").await;
    let first_export_job_arn =
        export_job_arn(&emulator, region, "first export job", &first_search_job).await;
    let second_export_job_arn =
        export_job_arn(&emulator, region, "second export job", &second_search_job).await;

    let account = engine::emulator_account_id();
    // Rows: an ARN, and whether a held record of this store carries it. The resolving
    // rows span both taggable resource types and two records of each; the refusing rows
    // span the ways an ARN names nothing held — an identifier of each resource type that
    // no record holds, an unknown resource type, another service's ARN, and a string that
    // is not an ARN at all.
    let rows: Vec<(&str, String, bool)> = vec![
        ("the first search job", first_search_job_arn.clone(), true),
        ("the second search job", second_search_job_arn.clone(), true),
        ("the first export job", first_export_job_arn.clone(), true),
        ("the second export job", second_export_job_arn.clone(), true),
        (
            "a search-job id no search job holds",
            format!("arn:aws:backup-search:{region}:{account}:search-job/00000000-0000-4000-8000-000000000000"),
            false,
        ),
        (
            "an export-job id no export job holds",
            format!("arn:aws:backup-search:{region}:{account}:export-job/00000000-0000-4000-8000-000000000000"),
            false,
        ),
        (
            "an ARN of this service naming a resource type it does not mint",
            format!("arn:aws:backup-search:{region}:{account}:search-plan/one"),
            false,
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::some-bucket".to_string(),
            false,
        ),
        ("a string that is not an ARN", "not-an-arn".to_string(), false),
    ];

    for (case, arn, resolves) in &rows {
        let read = list_tags(&emulator, region, arn).await;
        let written = tag_resource(&emulator, region, arn, json!({"resolved-by": case})).await;

        if *resolves {
            // Both paths resolved it, and the write landed on the record the read reports:
            // one resolution, so the read sees exactly what the write put there.
            assert_eq!(
                read.status, 200,
                "case {case:?}: ListTagsForResource must resolve a held ARN, got {} ({:?})",
                read.status, read.body
            );
            assert_eq!(
                written.status,
                state::NO_CONTENT_SUCCESS_STATUS,
                "case {case:?}: TagResource must resolve the same held ARN the read did, got {} ({:?})",
                written.status,
                written.body
            );
            let after_write = list_tags(&emulator, region, arn).await;
            assert_eq!(
                after_write.status, 200,
                "case {case:?}: reading back the tags must be 200, got {} ({:?})",
                after_write.status, after_write.body
            );
            let read_back = tags_of(&after_write);
            assert_eq!(
                read_back.get("resolved-by").map(String::as_str),
                Some(*case),
                "case {case:?}: the read path must report the tag the write path added, got {read_back:?}"
            );

            let removed = untag_resource(&emulator, region, arn, &["resolved-by"]).await;
            assert_eq!(
                removed.status,
                state::NO_CONTENT_SUCCESS_STATUS,
                "case {case:?}: UntagResource must resolve the same held ARN, got {} ({:?})",
                removed.status,
                removed.body
            );
            let after_removal = list_tags(&emulator, region, arn).await;
            let surviving = tags_of(&after_removal);
            assert!(
                !surviving.contains_key("resolved-by"),
                "case {case:?}: the read path must report the key the write path removed as gone, got {surviving:?}"
            );
        } else {
            let removed = untag_resource(&emulator, region, arn, &["resolved-by"]).await;
            for (operation, answer) in [
                ("ListTagsForResource", &read),
                ("TagResource", &written),
                ("UntagResource", &removed),
            ] {
                assert_eq!(
                    answer.status, 404,
                    "case {case:?}/{operation}: an ARN no held record carries must be refused on every tag path, got {} ({:?})",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code.as_deref(),
                    Some("NotFoundException"),
                    "case {case:?}/{operation}: the refusal must carry NotFoundException, got {:?}",
                    answer.error_code
                );
            }
        }
    }

    // And each resolving row reached its OWN record: every held ARN still reports a tag
    // set of its own, with no key left over from another row's write.
    for arn in [
        &first_search_job_arn,
        &second_search_job_arn,
        &first_export_job_arn,
        &second_export_job_arn,
    ] {
        let final_read = list_tags(&emulator, region, arn).await;
        let tags = tags_of(&final_read);
        assert!(
            tags.is_empty(),
            "ARN {arn:?}: every row removed the key it added, so the record must report no tags, got {tags:?}"
        );
    }
}
