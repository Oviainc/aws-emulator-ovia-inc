// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the tag APIs' RESOURCE RESOLUTION: `ListTagsForResource`,
//! `TagResource` and `UntagResource` over `/tags/{ResourceArn}`.
//!
//! The rule under test: each of those operations resolves its `ResourceArn` against every
//! resource this service mints and holds — the model documents the member only as "the
//! resource", and each of this service's resources has its own `@arnReference` ARN and is
//! minted by an operation taking the same `TagMap` — so a tag read or write addressed at
//! the ARN of ANY held resource reaches that resource's own stored tag set, and
//! `ResourceNotFoundException` is reserved for an ARN that matches no held record.
//!
//! So every assertion is made over a TABLE spanning the resource types this service
//! mints (search job and export job, the two the model gives an ARN and a `TagMap`) and
//! differing in the members the rule generalises over — tag count, the character classes
//! a key carries, which keys are added and removed, and the signing region — never over
//! the one request the issue happened to cite.
//!
//! The HTTP harness (`emulator`, `call`, `percent_encode`, `Answer`) and the
//! transition-span constant are the search-job suite's: every suite drives the same
//! emulator the same way, so there is one harness, not several.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator, percent_encode, query_value, Answer, PAST_THE_CREATING_INSTANT,
    PAST_THE_DUE_INSTANT,
};

/// Which of this service's taggable resources a row addresses. The set is the resources
/// the model gives both an ARN of this service and a `TagMap` on the operation that mints
/// them: `SearchJobArn`/`StartSearchJob` and `ExportJobArn`/`StartSearchResultExportJob`
/// (smithy_query query="operation" operation="StartSearchJob,StartSearchResultExportJob").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resource {
    SearchJob,
    ExportJob,
}

impl Resource {
    /// The ARN resource-type segment the resource's ARN carries, which is also what a
    /// refusal reports as `resourceType`.
    fn resource_segment(self) -> &'static str {
        match self {
            Self::SearchJob => state::SEARCH_JOB_RESOURCE,
            Self::ExportJob => state::EXPORT_JOB_RESOURCE,
        }
    }
}

/// One row of the tag table: a resource to mint, the tags it is created with, the tags a
/// later `TagResource` adds, the keys a later `UntagResource` removes, and the region the
/// calls are signed for.
struct TagCase {
    case: &'static str,
    resource: Resource,
    region: &'static str,
    created_with: Value,
    added: Value,
    removed: &'static [&'static str],
}

fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "search job created with one tag",
            resource: Resource::SearchJob,
            region: "us-west-2",
            created_with: json!({"env": "prod"}),
            added: json!({"team": "data"}),
            removed: &["env"],
        },
        TagCase {
            case: "export job created with one tag",
            resource: Resource::ExportJob,
            region: "us-west-2",
            created_with: json!({"env": "prod"}),
            added: json!({"team": "data"}),
            removed: &["env"],
        },
        TagCase {
            case: "export job created with no tags at all",
            resource: Resource::ExportJob,
            region: "eu-central-1",
            created_with: json!({}),
            added: json!({"owner": "backup", "tier": "gold"}),
            removed: &["tier"],
        },
        TagCase {
            case: "search job created with no tags at all",
            resource: Resource::SearchJob,
            region: "eu-central-1",
            created_with: json!({}),
            added: json!({"owner": "backup", "tier": "gold"}),
            removed: &["tier"],
        },
        TagCase {
            // The keys exercise the character classes `Tags` documents ("UTF-8 letters,
            // numbers, spaces, and the following characters: + - = . _ : /") beside the
            // plain ones the issue cited.
            case: "export job with several tags, punctuation and spaces in the keys",
            resource: Resource::ExportJob,
            region: "us-east-1",
            created_with: json!({
                "cost:center": "cc-1",
                "app/name+1": "search",
                "two words": "yes",
                "env": "prod"
            }),
            added: json!({"stage.name": "beta", "env": "staging"}),
            removed: &["app/name+1", "never-set"],
        },
        TagCase {
            case: "search job with several tags, punctuation and spaces in the keys",
            resource: Resource::SearchJob,
            region: "us-east-1",
            created_with: json!({
                "cost:center": "cc-1",
                "app/name+1": "search",
                "two words": "yes",
                "env": "prod"
            }),
            added: json!({"stage.name": "beta", "env": "staging"}),
            removed: &["app/name+1", "never-set"],
        },
        TagCase {
            case: "export job whose whole tag set is removed again",
            resource: Resource::ExportJob,
            region: "us-west-2",
            created_with: json!({"only": "one"}),
            added: json!({"second": "two"}),
            removed: &["only", "second"],
        },
    ]
}

/// A JSON object of string values as a sorted map, so a comparison does not depend on map
/// ordering.
fn as_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /tags/{ResourceArn}`.
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

/// The tags `ListTagsForResource` reports for `arn`, asserting the read itself succeeded.
async fn stored_tags(
    emulator: &MultiServiceRouter,
    region: &str,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let listed = list_tags(emulator, region, arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: reading the tags of {arn:?} must be 200, got {} ({:?})",
        listed.status, listed.body
    );
    as_map(listed.get("Tags"))
}

/// `POST /tags/{ResourceArn}`.
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

/// Start a search job carrying `tags` and bring it to `STOPPED`, which is a status an
/// export may be started from. Answers its identifier and its ARN.
async fn stopped_search_job(
    emulator: &MultiServiceRouter,
    region: &str,
    case: &str,
    name: &str,
    tags: &Value,
) -> (String, String) {
    let mut body = json!({
        "Name": name,
        "SearchScope": {"BackupResourceTypes": ["S3"]},
    });
    if !as_map(tags).is_empty() {
        body["Tags"] = tags.clone();
    }
    let started = call(emulator, region, "PUT", "/search-jobs", Some(body)).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartSearchJob must succeed, got {} ({:?})",
        started.status, started.body
    );
    let identifier = started.string("SearchJobIdentifier");

    // Cancel it once its scan is running, which is the status a stop acts from, then let
    // the clock make STOPPED due: a settled search job is what an export may be started
    // from.
    tokio::time::sleep(PAST_THE_CREATING_INSTANT).await;
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
    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;

    (identifier, started.string("SearchJobArn"))
}

/// Mint the resource a row addresses, carrying exactly `tags`, and answer its ARN.
async fn resource_arn(
    emulator: &MultiServiceRouter,
    region: &str,
    case: &str,
    resource: Resource,
    tags: &Value,
) -> String {
    match resource {
        Resource::SearchJob => {
            stopped_search_job(emulator, region, case, "tagged-search-job", tags)
                .await
                .1
        }
        Resource::ExportJob => {
            // An export job needs a search job in a settled status; that search job is
            // minted WITHOUT tags, so a tag set read back off the export ARN can only
            // have come from the export record.
            let (search_job_identifier, _) =
                stopped_search_job(emulator, region, case, "export-source", &json!({})).await;
            let mut body = json!({
                "SearchJobIdentifier": search_job_identifier,
                "ExportSpecification": {
                    "s3ExportSpecification": {"DestinationBucket": "search-exports"}
                },
            });
            if !as_map(tags).is_empty() {
                body["Tags"] = tags.clone();
            }
            let started = call(emulator, region, "PUT", "/export-search-jobs", Some(body)).await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: StartSearchResultExportJob must succeed, got {} ({:?})",
                started.status, started.body
            );
            started.string("ExportJobArn")
        }
    }
}

#[tokio::test]
async fn the_tags_of_every_resource_this_service_mints_are_read_back_added_to_and_removed() {
    for TagCase {
        case,
        resource,
        region,
        created_with,
        added,
        removed,
    } in tag_cases()
    {
        let emulator = emulator();
        let arn = resource_arn(&emulator, region, case, resource, &created_with).await;
        assert!(
            arn.contains(&format!(":{}/", resource.resource_segment())),
            "case {case:?}: the minted ARN must name the {} resource type, got {arn:?}",
            resource.resource_segment()
        );

        // 1. The tags the resource was created with are what the tag read reports.
        let listed = stored_tags(&emulator, region, case, &arn).await;
        assert_eq!(
            listed,
            as_map(&created_with),
            "case {case:?}: ListTagsForResource must report the tags the resource was created with, got {listed:?}"
        );

        // 2. TagResource merges: a key already present takes the new value, the others
        //    are left as they were.
        let tagged = tag_resource(&emulator, region, &arn, added.clone()).await;
        assert_eq!(
            tagged.status,
            state::NO_CONTENT_SUCCESS_STATUS,
            "case {case:?}: TagResource must answer {}, got {} ({:?})",
            state::NO_CONTENT_SUCCESS_STATUS,
            tagged.status,
            tagged.body
        );
        let mut expected = as_map(&created_with);
        expected.extend(as_map(&added));
        let merged = stored_tags(&emulator, region, case, &arn).await;
        assert_eq!(
            merged, expected,
            "case {case:?}: the resource must report the merged tag set, got {merged:?}"
        );

        // 3. UntagResource removes exactly the keys it names, present or not.
        let untagged = untag_resource(&emulator, region, &arn, removed).await;
        assert_eq!(
            untagged.status,
            state::NO_CONTENT_SUCCESS_STATUS,
            "case {case:?}: UntagResource must answer {}, got {} ({:?})",
            state::NO_CONTENT_SUCCESS_STATUS,
            untagged.status,
            untagged.body
        );
        for key in removed {
            expected.remove(*key);
        }
        let surviving = stored_tags(&emulator, region, case, &arn).await;
        assert_eq!(
            surviving, expected,
            "case {case:?}: the resource must report exactly the surviving tags, got {surviving:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_write_touches_only_the_resource_its_arn_names() {
    let region = "us-west-2";
    let emulator = emulator();

    // One search job and one export job, created with the SAME keys, so a write addressed
    // at one can only be seen on that one if the ARN really selects the record.
    let shared = json!({"env": "prod", "team": "data"});
    let (search_job_identifier, search_job_arn) =
        stopped_search_job(&emulator, region, "isolation", "isolated-search-job", &shared).await;
    let started = call(
        &emulator,
        region,
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "SearchJobIdentifier": search_job_identifier,
            "ExportSpecification": {
                "s3ExportSpecification": {"DestinationBucket": "search-exports"}
            },
            "Tags": shared.clone()
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartSearchResultExportJob must succeed, got {} ({:?})",
        started.status, started.body
    );
    let export_job_arn = started.string("ExportJobArn");
    assert_ne!(
        export_job_arn, search_job_arn,
        "the two resources must have different ARNs, got {export_job_arn:?} twice"
    );

    // Rows: which ARN is written to, and which one must stay as it was.
    let writes = [
        ("write to the export job", &export_job_arn, &search_job_arn),
        ("write to the search job", &search_job_arn, &export_job_arn),
    ];
    for (case, addressed, other) in writes {
        // What the other resource holds BEFORE this row's write, so the claim is that the
        // write did not reach it — rows run against one store, so the earlier row's
        // writes are part of that "before".
        let before = stored_tags(&emulator, region, case, other).await;

        let tagged = tag_resource(&emulator, region, addressed, json!({"written": case})).await;
        assert_eq!(
            tagged.status,
            state::NO_CONTENT_SUCCESS_STATUS,
            "case {case:?}: TagResource must answer {}, got {} ({:?})",
            state::NO_CONTENT_SUCCESS_STATUS,
            tagged.status,
            tagged.body
        );
        let untagged = untag_resource(&emulator, region, addressed, &["env"]).await;
        assert_eq!(
            untagged.status,
            state::NO_CONTENT_SUCCESS_STATUS,
            "case {case:?}: UntagResource must answer {}, got {} ({:?})",
            state::NO_CONTENT_SUCCESS_STATUS,
            untagged.status,
            untagged.body
        );

        let written = stored_tags(&emulator, region, case, addressed).await;
        assert_eq!(
            written.get("written").map(String::as_str),
            Some(case),
            "case {case:?}: the addressed resource must carry the added tag, got {written:?}"
        );
        assert!(
            !written.contains_key("env"),
            "case {case:?}: the addressed resource must have lost the removed key, got {written:?}"
        );

        let untouched = stored_tags(&emulator, region, case, other).await;
        assert_eq!(
            untouched, before,
            "case {case:?}: the other resource's tag set must be exactly what it was before the write, got {untouched:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_no_held_resource_matches_is_not_found_on_every_tag_operation() {
    let region = "us-west-2";
    let emulator = emulator();
    // Both resource types exist in the store, so a miss is a miss on the lookup and not
    // an empty store answering 404 for everything.
    let held_search_job = resource_arn(
        &emulator,
        region,
        "not-found seed",
        Resource::SearchJob,
        &json!({"env": "prod"}),
    )
    .await;
    let held_export_job = resource_arn(
        &emulator,
        region,
        "not-found seed",
        Resource::ExportJob,
        &json!({"env": "prod"}),
    )
    .await;

    let account = engine::emulator_account_id();
    // Rows differ in HOW the ARN fails to resolve: an identifier no record of either
    // resource type holds, another service's ARN, and a string that is not an ARN.
    let unknown: Vec<(&str, String, &str)> = vec![
        (
            "a search-job id no search job holds",
            format!("arn:aws:backup-search:{region}:{account}:search-job/8d2e0e1f-does-not-exist"),
            state::SEARCH_JOB_RESOURCE,
        ),
        (
            "an export-job id no export job holds",
            format!("arn:aws:backup-search:{region}:{account}:export-job/8d2e0e1f-does-not-exist"),
            state::EXPORT_JOB_RESOURCE,
        ),
        (
            "an ARN of this service's region and account with an unknown resource type",
            format!("arn:aws:backup-search:{region}:{account}:search-plan/one"),
            "search-plan",
        ),
        (
            "an ARN whose resource type is colon-separated",
            format!("arn:aws:backup:{region}:{account}:recovery-point:rp-1"),
            "recovery-point",
        ),
        (
            "an ARN naming another service's resource, whose resource field is an id alone",
            "arn:aws:s3:::some-bucket".to_string(),
            "",
        ),
        ("a string that is not an ARN", "not-an-arn".to_string(), ""),
    ];

    for (case, arn, resource_type) in unknown {
        let answers = [
            ("ListTagsForResource", list_tags(&emulator, region, &arn).await),
            (
                "TagResource",
                tag_resource(&emulator, region, &arn, json!({"team": "data"})).await,
            ),
            (
                "UntagResource",
                untag_resource(&emulator, region, &arn, &["env"]).await,
            ),
        ];
        for (operation, answer) in answers {
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{operation}: an unknown resource must be 404, got {} ({:?})",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}/{operation}: the refusal must carry ResourceNotFoundException, got {:?}",
                answer.error_code
            );
            assert_eq!(
                answer.string("resourceId"),
                arn,
                "case {case:?}/{operation}: the refusal must name the submitted ARN, got {:?}",
                answer.get("resourceId")
            );
            assert_eq!(
                answer.string("resourceType"),
                resource_type,
                "case {case:?}/{operation}: the refusal must report the type the submitted ARN claims, got {:?}",
                answer.get("resourceType")
            );
        }
    }

    // And none of those refusals disturbed the records that DO exist.
    for (case, arn) in [
        ("search job", &held_search_job),
        ("export job", &held_export_job),
    ] {
        let untouched = stored_tags(&emulator, region, case, arn).await;
        assert_eq!(
            untouched,
            as_map(&json!({"env": "prod"})),
            "case {case:?}: a refused tag call must leave a held resource as it was, got {untouched:?}"
        );
    }
}
