// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the SUCCESS STATUS every operation of this service answers with.
//!
//! The rule under test: an operation whose output structure declares NO members and whose
//! success code the model leaves DEFAULTED (`@http` carries no `code:`, reported as
//! `codeDefaulted: true`) has no content to report, so this service answers it with
//! [`state::NO_CONTENT_SUCCESS_STATUS`] (204 No Content); an operation whose model states
//! its success code explicitly answers THAT code, and one whose output declares members
//! answers the Smithy default 200 with those members.
//!
//! So the assertion is made over a TABLE spanning EVERY operation this service binds —
//! both empty-output operations with a defaulted code, the empty-output operation whose
//! code the model states explicitly, and the nine whose output declares members — and
//! each row carries the two model facts the rule reads (how many members the output
//! declares, and the code the model states, if any) rather than a status written out by
//! hand. The operation the issue cited is ONE row.
//!
//! Source for every row's two model facts: `smithy_query(query="operation",
//! operation="<Name>")` on this crate's `smithy-model/`, whose `output` map and
//! `http: {code, codeDefaulted}` are quoted per row; the row count is checked against
//! that model's `operation_count`.
//!
//! The HTTP harness (`emulator`, `call`, `percent_encode`) and the transition-span
//! constant are the search-job suite's: every suite drives the same emulator the same
//! way, so there is one harness, not several.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator, emulator_with_engine, percent_encode, query_value, recovery_point,
    seed_recovery_points, PAST_THE_CREATING_INSTANT, PAST_THE_DUE_INSTANT,
};

/// The region every call in this suite is signed for. The status does not vary by region
/// (the search-job suite already walks the regions), so one is enough here.
const REGION: &str = "us-west-2";

/// The status Smithy defaults a successful REST operation to when the model states none
/// and there IS content to report.
const DEFAULTED_SUCCESS_STATUS: u16 = 200;

/// How many operations the service binds, from the model's `operation_count`
/// (`smithy_query(query="digest")`). Every one of them is a row below.
const BOUND_OPERATIONS: usize = 12;

/// One operation, a successful request that reaches it, and the two facts the model
/// states about its response.
struct Row {
    /// The operation, as the model binds it.
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// How many members the operation's OUTPUT structure declares (the `output` map of
    /// `smithy_query(query="operation")`).
    output_members: usize,
    /// The success code the model states for the operation, or `None` when the model
    /// leaves it defaulted (`codeDefaulted: true`), i.e. states nothing about it.
    modelled_code: Option<u16>,
}

impl Row {
    /// The status the rule under test gives this operation's success.
    fn expected_status(&self) -> u16 {
        match (self.modelled_code, self.output_members) {
            // The model states the code: that code is the contract.
            (Some(code), _) => code,
            // Nothing stated and nothing to report: no content.
            (None, 0) => state::NO_CONTENT_SUCCESS_STATUS,
            // Nothing stated, but there is a body to parse.
            (None, _) => DEFAULTED_SUCCESS_STATUS,
        }
    }
}

/// Drive one row and assert the status the rule gives it, plus that a no-content success
/// really carries no output members and a reporting success carries some.
async fn check(emulator: &MultiServiceRouter, row: &Row) {
    let Row {
        operation,
        method,
        uri,
        body,
        output_members,
        ..
    } = row;
    let expected = row.expected_status();
    let answer = call(emulator, REGION, method, uri, body.clone()).await;
    assert_eq!(
        answer.status, expected,
        "case {operation:?}: a successful {operation} must answer {expected}, got {} ({:?})",
        answer.status, answer.body
    );
    assert!(
        answer.error_code.is_none(),
        "case {operation:?}: a success must carry no error code, got {:?}",
        answer.error_code
    );

    let members = answer.body.as_object().map_or(0, serde_json::Map::len);
    if *output_members == 0 {
        assert_eq!(
            members, 0,
            "case {operation:?}: an operation whose modelled output has no members must report none, got {:?}",
            answer.body
        );
    } else {
        assert!(
            members > 0,
            "case {operation:?}: an operation whose modelled output declares {output_members} member(s) must report some, got {:?}",
            answer.body
        );
    }
}

/// Start one search job and answer its identifier and its ARN.
async fn started_job(emulator: &MultiServiceRouter, name: &str, tags: Value) -> (String, String) {
    let mut body = json!({
        "Name": name,
        "SearchScope": {"BackupResourceTypes": ["S3"]},
    });
    if tags.as_object().is_some_and(|tags| !tags.is_empty()) {
        body["Tags"] = tags;
    }
    let started = call(emulator, REGION, "PUT", "/search-jobs", Some(body)).await;
    assert_eq!(
        started.status, DEFAULTED_SUCCESS_STATUS,
        "the seed StartSearchJob must succeed, got {} ({:?})",
        started.status, started.body
    );
    (
        started.string("SearchJobIdentifier"),
        started.string("SearchJobArn"),
    )
}

#[tokio::test]
async fn every_operations_success_status_is_the_one_its_response_shape_implies() {
    let (emulator, engine) = emulator_with_engine();
    // One recovery point this account holds, so the seeded job's scope resolves to
    // something and the clock owes it `COMPLETED` — the settled status an export may be
    // started from. Nothing in THIS service creates a recovery point; the Backup data
    // plane does, so a test that needs a scope to resolve seeds them.
    seed_recovery_points(
        &engine,
        vec![recovery_point(
            "S3",
            "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-s3",
            "arn:aws:s3:::alpha",
            std::time::SystemTime::now(),
        )],
    );

    // The search job the search-job reads and the tag calls are addressed at, started
    // before the table so its identifier and ARN can be spelled into the rows' URIs.
    let (identifier, arn) =
        started_job(&emulator, "success-status-seed", json!({"env": "test"})).await;
    let tags_uri = format!("/tags/{}", percent_encode(&arn));

    let reads_and_tag_writes = vec![
        Row {
            // output {SearchJobArn, CreationTime, SearchJobIdentifier}; http code 200,
            // stated explicitly by the model.
            operation: "StartSearchJob",
            method: "PUT",
            uri: "/search-jobs".to_string(),
            body: Some(json!({
                "Name": "success-status-second",
                "SearchScope": {"BackupResourceTypes": ["EBS"]}
            })),
            output_members: 3,
            modelled_code: Some(200),
        },
        Row {
            // output of 12 members; http code 200, stated explicitly.
            operation: "GetSearchJob",
            method: "GET",
            uri: format!("/search-jobs/{identifier}"),
            body: None,
            output_members: 12,
            modelled_code: Some(200),
        },
        Row {
            // output {SearchJobs, NextToken}; http code 200, stated explicitly.
            operation: "ListSearchJobs",
            method: "GET",
            uri: "/search-jobs".to_string(),
            body: None,
            output_members: 2,
            modelled_code: Some(200),
        },
        Row {
            // output {Results, NextToken}; http code 200, stated explicitly.
            operation: "ListSearchJobResults",
            method: "GET",
            uri: format!("/search-jobs/{identifier}/search-results"),
            body: None,
            output_members: 2,
            modelled_code: Some(200),
        },
        Row {
            // output {Results, NextToken}; http code 200, stated explicitly.
            operation: "ListSearchJobBackups",
            method: "GET",
            uri: format!("/search-jobs/{identifier}/backups"),
            body: None,
            output_members: 2,
            modelled_code: Some(200),
        },
        Row {
            // output {} (TagResourceResponse declares no member) and
            // http {code: 200, codeDefaulted: true}: the rule's own case.
            operation: "TagResource",
            method: "POST",
            uri: tags_uri.clone(),
            body: Some(json!({"Tags": {"team": "a"}})),
            output_members: 0,
            modelled_code: None,
        },
        Row {
            // output {Tags}; http {code: 200, codeDefaulted: true} — a defaulted code
            // WITH content to report, so the Smithy default stands.
            operation: "ListTagsForResource",
            method: "GET",
            uri: tags_uri.clone(),
            body: None,
            output_members: 1,
            modelled_code: None,
        },
        Row {
            // output {} (UntagResourceResponse declares no member) and
            // http {code: 200, codeDefaulted: true}: the operation the issue cited.
            operation: "UntagResource",
            method: "DELETE",
            uri: format!("{tags_uri}?tagKeys=team&tagKeys=env"),
            body: None,
            output_members: 0,
            modelled_code: None,
        },
    ];
    for row in &reads_and_tag_writes {
        check(&emulator, row).await;
    }

    // The no-content successes are still the writes they claim to be: the key the 204
    // TagResource added and the one the job was created with are both gone after the 204
    // UntagResource named them, and nothing else was invented.
    let listed = call(&emulator, REGION, "GET", &tags_uri, None).await;
    assert_eq!(
        listed.get("Tags"),
        &json!({}),
        "a 204 tag write must still have written: both removed keys must be gone, got {:?}",
        listed.get("Tags")
    );

    // `StopSearchJob` is the empty-output operation whose success code the model STATES,
    // so it is the row that pins the other side of the rule. It is addressed at a job
    // whose scan is running, which is the status the operation acts from.
    let (to_stop, _) = started_job(&emulator, "success-status-stopped", json!({})).await;
    tokio::time::sleep(PAST_THE_CREATING_INSTANT).await;
    let stop = vec![Row {
        // output {} just like the two tag writes, but http {code: 200} stated EXPLICITLY
        // (no codeDefaulted), so the model's own code is the contract.
        operation: "StopSearchJob",
        method: "PUT",
        uri: format!("/search-jobs/{to_stop}/actions/cancel"),
        body: None,
        output_members: 0,
        modelled_code: Some(200),
    }];
    for row in &stop {
        check(&emulator, row).await;
    }

    // The export operations need a search job whose results have settled: the clock makes
    // the seeded job's owed terminal status due.
    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let export = call(
        &emulator,
        REGION,
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "SearchJobIdentifier": identifier,
            "ExportSpecification": {
                "s3ExportSpecification": {"DestinationBucket": "search-exports"}
            }
        })),
    )
    .await;
    assert_eq!(
        export.status, DEFAULTED_SUCCESS_STATUS,
        "the seed StartSearchResultExportJob must succeed, got {} ({:?})",
        export.status, export.body
    );
    let export_identifier = export.string("ExportJobIdentifier");

    let exports = vec![
        Row {
            // output {ExportJobArn, ExportJobIdentifier}; http code 200, stated
            // explicitly.
            operation: "StartSearchResultExportJob",
            method: "PUT",
            uri: "/export-search-jobs".to_string(),
            body: Some(json!({
                "SearchJobIdentifier": identifier,
                "ExportSpecification": {
                    "s3ExportSpecification": {"DestinationBucket": "search-exports-2"}
                }
            })),
            output_members: 2,
            modelled_code: Some(200),
        },
        Row {
            // output of 8 members; http code 200, stated explicitly.
            operation: "GetSearchResultExportJob",
            method: "GET",
            uri: format!("/export-search-jobs/{export_identifier}"),
            body: None,
            output_members: 8,
            modelled_code: Some(200),
        },
        Row {
            // output {ExportJobs, NextToken}; http code 200, stated explicitly.
            operation: "ListSearchResultExportJobs",
            method: "GET",
            uri: "/export-search-jobs".to_string(),
            body: None,
            output_members: 2,
            modelled_code: Some(200),
        },
    ];
    for row in &exports {
        check(&emulator, row).await;
    }

    // And the table really is the whole surface: one row per operation the model binds,
    // none of them twice.
    let mut covered: Vec<&str> = reads_and_tag_writes
        .iter()
        .chain(&stop)
        .chain(&exports)
        .map(|row| row.operation)
        .collect();
    assert_eq!(
        covered.len(),
        BOUND_OPERATIONS,
        "every operation the model binds must have a row, got {covered:?}"
    );
    covered.sort_unstable();
    covered.dedup();
    assert_eq!(
        covered.len(),
        BOUND_OPERATIONS,
        "no operation may be covered twice while another has no row, got {covered:?}"
    );
}

#[tokio::test]
async fn the_tag_writes_answer_no_content_whatever_the_request_names() {
    // The rule is about the operation's response SHAPE, so what the request names cannot
    // change the status. Rows differ in how many tags are written, how many keys are
    // removed, whether those keys are present, and the character classes they carry.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "one tag written, the same key removed",
            json!({"k": "v"}),
            vec!["k"],
        ),
        (
            "three tags written, two removed",
            json!({"env": "prod", "team": "a", "tier": "gold"}),
            vec!["team", "tier"],
        ),
        (
            "a key carrying the punctuation tags allow, and an absent key removed",
            json!({"cost:center": "cc-1", "app/name+1": "search"}),
            vec!["never-set"],
        ),
        (
            "a tag whose value is empty, removed by a key with a space in it",
            json!({"two words": ""}),
            vec!["two words"],
        ),
    ];

    for (case, tags, removed) in cases {
        let emulator = emulator();
        let (_, arn) = started_job(&emulator, "no-content-tag-writes", json!({})).await;
        let tags_uri = format!("/tags/{}", percent_encode(&arn));

        let tagged = call(
            &emulator,
            REGION,
            "POST",
            &tags_uri,
            Some(json!({"Tags": tags})),
        )
        .await;
        assert_eq!(
            tagged.status,
            state::NO_CONTENT_SUCCESS_STATUS,
            "case {case:?}: TagResource must answer {}, got {} ({:?})",
            state::NO_CONTENT_SUCCESS_STATUS,
            tagged.status,
            tagged.body
        );

        let query: Vec<String> = removed
            .iter()
            .map(|key| format!("tagKeys={}", query_value(key)))
            .collect();
        let untagged = call(
            &emulator,
            REGION,
            "DELETE",
            &format!("{tags_uri}?{}", query.join("&")),
            None,
        )
        .await;
        assert_eq!(
            untagged.status,
            state::NO_CONTENT_SUCCESS_STATUS,
            "case {case:?}: UntagResource must answer {}, got {} ({:?})",
            state::NO_CONTENT_SUCCESS_STATUS,
            untagged.status,
            untagged.body
        );

        // The read beside them keeps the defaulted 200: it HAS content to report, and it
        // reports the removal the no-content successes performed.
        let listed = call(&emulator, REGION, "GET", &tags_uri, None).await;
        assert_eq!(
            listed.status, DEFAULTED_SUCCESS_STATUS,
            "case {case:?}: ListTagsForResource must answer {DEFAULTED_SUCCESS_STATUS}, got {} ({:?})",
            listed.status, listed.body
        );
        for key in &removed {
            assert!(
                listed.get("Tags").get(key).is_none(),
                "case {case:?}: key {key:?} must be gone after the 204 removal, got {:?}",
                listed.get("Tags")
            );
        }
    }
}
