// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the BackupSearch export-job slice, driven over HTTP exactly as a
//! client drives it: `StartSearchResultExportJob` and `GetSearchResultExportJob`.
//!
//! The rule under test is that an export job is a PERSISTED record of one search job's
//! export — minted by `StartSearchResultExportJob`, reported unchanged by
//! `GetSearchResultExportJob`, moved on only by the clock — so every assertion is made
//! over a TABLE of requests differing in the members the rule generalises over
//! (destination bucket, destination prefix, role ARN, tag count, signing region, and how
//! the search job reached a status export permits), never over one request alone.
//!
//! The HTTP harness (`emulator`, `emulator_with_engine`, `call`, `Answer`) and the two
//! transition-span constants are the search-job suite's: both suites drive the same
//! emulator the same way, so there is one harness, not two.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator, emulator_with_engine, Answer, PAST_THE_DUE_INSTANT, WITHIN_THE_DUE_INSTANT,
};

/// How a search job reaches a status an export may be started from. Both of the statuses
/// in `state::EXPORTABLE_SEARCH_JOB_STATES` are reachable, and each row of the table
/// below picks one, so the rule is asserted on every permitted status rather than on the
/// one a recording happened to use.
#[derive(Debug, Clone, Copy)]
enum Settled {
    /// Let the scan finish: `RUNNING` -> `COMPLETED`.
    Completed,
    /// Cancel it: `RUNNING` -> `STOPPING` -> `STOPPED`.
    Stopped,
}

impl Settled {
    fn expected_status(self) -> &'static str {
        match self {
            Self::Completed => state::COMPLETED,
            Self::Stopped => state::STOPPED,
        }
    }
}

/// One row of the start-an-export table: the search job's route to a settled status and
/// the `StartSearchResultExportJob` body, which differ in destination bucket, in whether
/// a destination prefix and a `RoleArn` are present, in tag count and in signing region.
struct ExportCase {
    case: &'static str,
    region: &'static str,
    settled: Settled,
    specification: Value,
    role_arn: Option<&'static str>,
    tags: Option<Value>,
}

fn export_cases() -> Vec<ExportCase> {
    vec![
        ExportCase {
            case: "bucket only, no prefix, completed job, us-west-2",
            region: "us-west-2",
            settled: Settled::Completed,
            specification: json!({"s3ExportSpecification": {"DestinationBucket": "search-exports"}}),
            // Satisfies IamRoleArn's modelled @pattern
            // (^arn:(?:aws|aws-cn|aws-us-gov):iam::[a-z0-9-]+:role/(.+)$).
            role_arn: Some("arn:aws:iam::123456789012:role/backup-search-export"),
            tags: None,
        },
        ExportCase {
            case: "bucket and prefix, stopped job, eu-central-1, one tag, no role",
            region: "eu-central-1",
            settled: Settled::Stopped,
            specification: json!({
                "s3ExportSpecification": {
                    "DestinationBucket": "another-bucket",
                    "DestinationPrefix": "exports/2026"
                }
            }),
            role_arn: None,
            tags: Some(json!({"team": "data"})),
        },
        ExportCase {
            case: "dotted bucket name and a nested prefix, completed job, us-east-1, two tags",
            region: "us-east-1",
            settled: Settled::Completed,
            specification: json!({
                "s3ExportSpecification": {
                    "DestinationBucket": "my.search.exports",
                    "DestinationPrefix": "a/b/c/"
                }
            }),
            role_arn: Some("arn:aws-us-gov:iam::210987654321:role/service-role/export"),
            tags: Some(json!({"owner": "backup", "env": "test"})),
        },
    ]
}

/// The `StartSearchResultExportJob` body for one row.
fn export_body(case: &ExportCase, search_job_identifier: &str) -> Value {
    let mut body = json!({
        "SearchJobIdentifier": search_job_identifier,
        "ExportSpecification": case.specification.clone(),
    });
    if let Some(role_arn) = case.role_arn {
        body["RoleArn"] = json!(role_arn);
    }
    if let Some(tags) = case.tags.clone() {
        body["Tags"] = tags;
    }
    body
}

/// Start a search job and bring it to the status `settled` names, the way a client does:
/// let the clock finish the scan, or cancel it and let the clock finish the stop. Answers
/// the job's identifier and ARN.
async fn settled_search_job(
    emulator: &MultiServiceRouter,
    region: &str,
    case: &str,
    settled: Settled,
) -> (String, String) {
    let started = call(
        emulator,
        region,
        "PUT",
        "/search-jobs",
        Some(json!({"Name": "to-export", "SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: expected 200 from StartSearchJob, got {} ({:?})",
        started.status, started.body
    );
    let identifier = started.string("SearchJobIdentifier");

    if matches!(settled, Settled::Stopped) {
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
            "case {case:?}: expected 200 from StopSearchJob, got {} ({:?})",
            stopped.status, stopped.body
        );
    }

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let fetched = call(
        emulator,
        region,
        "GET",
        &format!("/search-jobs/{identifier}"),
        None,
    )
    .await;
    assert_eq!(
        fetched.string("Status"),
        settled.expected_status(),
        "case {case:?}: the search job must reach {} before the export starts, got {:?}",
        settled.expected_status(),
        fetched.get("Status")
    );
    (identifier, fetched.string("SearchJobArn"))
}

/// `GetSearchResultExportJob` for `identifier`.
async fn get_export_job(
    emulator: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> Answer {
    call(
        emulator,
        region,
        "GET",
        &format!("/export-search-jobs/{identifier}"),
        None,
    )
    .await
}

/// How many export jobs the shared store holds — the check that a refused start wrote
/// nothing, which no modelled read can make while `ListSearchResultExportJobs` is still a
/// scaffold stub.
fn stored_export_jobs(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>()
        .lock()
        .export_jobs
        .len()
}

#[tokio::test]
async fn a_started_export_job_is_persisted_and_every_member_is_read_back_out_of_the_store() {
    let mut identifiers: BTreeSet<String> = BTreeSet::new();
    // One emulator for every row, so the identifiers the rows are answered with come out
    // of the SAME store and uniqueness is a claim about that store.
    let emulator = emulator();
    for case in export_cases() {
        let ExportCase {
            case: name,
            region,
            settled,
            ..
        } = case;
        let (search_job_identifier, search_job_arn) =
            settled_search_job(&emulator, region, name, settled).await;
        let body = export_body(&case, &search_job_identifier);

        let started = call(&emulator, region, "PUT", "/export-search-jobs", Some(body.clone())).await;
        assert_eq!(
            started.status, 200,
            "case {name:?}: expected 200 from StartSearchResultExportJob, got {} ({:?})",
            started.status, started.body
        );
        let identifier = started.string("ExportJobIdentifier");
        assert!(
            !identifier.is_empty(),
            "case {name:?}: StartSearchResultExportJob must mint an ExportJobIdentifier, got {:?}",
            started.body
        );
        assert!(
            identifiers.insert(identifier.clone()),
            "case {name:?}: every export job gets its own identifier, but {identifier:?} repeated"
        );
        let expected_arn = format!(
            "arn:aws:backup-search:{region}:{}:export-job/{identifier}",
            engine::emulator_account_id()
        );
        assert_eq!(
            started.string("ExportJobArn"),
            expected_arn,
            "case {name:?}: the ARN must name the signed region, the emulator account and the minted id, got {:?}",
            started.get("ExportJobArn")
        );

        // Straight after the call the export is in flight: the span the clock owes is
        // never zero, so the transient status is observable.
        tokio::time::sleep(WITHIN_THE_DUE_INSTANT).await;
        let running = get_export_job(&emulator, region, &identifier).await;
        assert_eq!(
            running.status, 200,
            "case {name:?}: expected 200 from GetSearchResultExportJob, got {} ({:?})",
            running.status, running.body
        );
        assert_eq!(
            running.string("ExportJobIdentifier"),
            identifier,
            "case {name:?}: GetSearchResultExportJob must report the stored identifier, got {:?}",
            running.body
        );
        assert_eq!(
            running.string("ExportJobArn"),
            expected_arn,
            "case {name:?}: GetSearchResultExportJob must report the stored ARN, got {:?}",
            running.get("ExportJobArn")
        );
        assert_eq!(
            running.string("Status"),
            state::RUNNING,
            "case {name:?}: an export started a moment ago is still RUNNING, got {:?}",
            running.get("Status")
        );
        assert_eq!(
            running.get("ExportSpecification"),
            &body["ExportSpecification"],
            "case {name:?}: the submitted ExportSpecification must round-trip, got {:?}",
            running.get("ExportSpecification")
        );
        assert_eq!(
            running.string("SearchJobArn"),
            search_job_arn,
            "case {name:?}: the export must name the ARN of the search job it exports, got {:?}",
            running.get("SearchJobArn")
        );
        let creation_time = running.get("CreationTime").as_f64();
        assert!(
            creation_time.is_some_and(|seconds| seconds > 1_600_000_000.0),
            "case {name:?}: CreationTime must come from the server clock, got {:?}",
            running.get("CreationTime")
        );
        assert!(
            running.get("CompletionTime").is_null(),
            "case {name:?}: an export still running has no CompletionTime, got {:?}",
            running.get("CompletionTime")
        );

        // Once the clock has made the owed transition due, the same read reports it —
        // this search job matched nothing, so the export had nothing to transmit and
        // completes.
        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let settled_export = get_export_job(&emulator, region, &identifier).await;
        assert_eq!(
            settled_export.string("Status"),
            state::COMPLETED,
            "case {name:?}: an export of an empty result set completes once due, got {:?}",
            settled_export.get("Status")
        );
        let completion_time = settled_export.get("CompletionTime").as_f64();
        assert!(
            completion_time.is_some_and(|seconds| Some(seconds) >= creation_time),
            "case {name:?}: a completed export carries a CompletionTime at or after its CreationTime, got {:?}",
            settled_export.get("CompletionTime")
        );
        assert!(
            settled_export.get("StatusMessage").is_null(),
            "case {name:?}: StatusMessage is the text a FAILED export carries, got {:?}",
            settled_export.get("StatusMessage")
        );
        // Reading it again changes nothing: the record is reported, not advanced.
        let again = get_export_job(&emulator, region, &identifier).await;
        assert_eq!(
            again.body, settled_export.body,
            "case {name:?}: a second read must report the same record, got {:?}",
            again.body
        );
    }
}

#[tokio::test]
async fn an_export_job_identifier_no_export_job_exists_for_is_not_found() {
    let (emulator, _engine) = emulator_with_engine();
    // One real export job exists, so a miss is a miss on the lookup and not an empty
    // store answering 404 for everything.
    let (search_job_identifier, _) =
        settled_search_job(&emulator, "us-west-2", "seed", Settled::Completed).await;
    let started = call(
        &emulator,
        "us-west-2",
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "SearchJobIdentifier": search_job_identifier,
            "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "seed-bucket"}}
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the seeding export must succeed, got {} ({:?})",
        started.status, started.body
    );

    // Identifiers that name no export job: an unknown opaque string, a UUID of the shape
    // the service's own corpus uses, one that differs from the minted id only in its
    // digits, and a SEARCH job's identifier — the two id spaces are separate.
    let minted = started.string("ExportJobIdentifier");
    let cases: Vec<(&str, String)> = vec![
        ("unknown opaque string", "does-not-exist".to_string()),
        (
            "well-formed uuid",
            "48434c42-40a0-4057-b04d-3b484c610e2b".to_string(),
        ),
        ("minted id with other digits", "exportjob-000000009999".to_string()),
        ("a search job's identifier", search_job_identifier.clone()),
    ];
    for (case, identifier) in cases {
        let answer = get_export_job(&emulator, "us-west-2", &identifier).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unknown ExportJobIdentifier must be 404, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the refusal must carry ResourceNotFoundException, got {:?}",
            answer.error_code
        );
        let message = answer.string("message");
        assert_eq!(
            message,
            state::RESOURCE_NOT_FOUND,
            "case {case:?}: the refusal carries this service's fixed not-found text, got {message:?}"
        );
        assert!(
            !message.contains(&identifier),
            "case {case:?}: the fixed not-found text must not echo the requested identifier, got {message:?}"
        );
    }
    // And the one that does exist still resolves, so the lookup is a lookup.
    let found = get_export_job(&emulator, "us-west-2", &minted).await;
    assert_eq!(
        found.status, 200,
        "the minted export job must still resolve, got {} ({:?})",
        found.status, found.body
    );
}

#[tokio::test]
async fn starting_an_export_for_a_search_job_that_does_not_exist_is_not_found_and_writes_nothing() {
    let (emulator, engine) = emulator_with_engine();
    let cases = [
        ("unknown opaque string", "does-not-exist"),
        ("well-formed uuid", "48434c42-40a0-4057-b04d-3b484c610e2b"),
        ("a minted id no job holds", "searchjob-000000004242"),
    ];
    for (case, search_job_identifier) in cases {
        let answer = call(
            &emulator,
            "us-west-2",
            "PUT",
            "/export-search-jobs",
            Some(json!({
                "SearchJobIdentifier": search_job_identifier,
                "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "b"}}
            })),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: exporting a search job that does not exist must be 404, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the refusal must carry ResourceNotFoundException, got {:?}",
            answer.error_code
        );
        assert_eq!(
            answer.string("message"),
            state::RESOURCE_NOT_FOUND,
            "case {case:?}: the refusal carries this service's fixed not-found text, got {:?}",
            answer.get("message")
        );
        assert_eq!(
            stored_export_jobs(&engine),
            0,
            "case {case:?}: a refused start must create no export job, store holds {}",
            stored_export_jobs(&engine)
        );
    }
}

#[tokio::test]
async fn an_export_is_refused_while_the_search_job_has_not_settled() {
    // Every non-terminal status a search job can be observed in, and the one terminal
    // status export does not permit.
    let cases = [
        ("still scanning", state::RUNNING, false),
        ("stop in flight", state::STOPPING, true),
    ];
    for (case, expected_status, cancel) in cases {
        let (emulator, engine) = emulator_with_engine();
        let started = call(
            &emulator,
            "us-west-2",
            "PUT",
            "/search-jobs",
            Some(json!({"SearchScope": {"BackupResourceTypes": ["S3"]}})),
        )
        .await;
        let search_job_identifier = started.string("SearchJobIdentifier");
        if cancel {
            call(
                &emulator,
                "us-west-2",
                "PUT",
                &format!("/search-jobs/{search_job_identifier}/actions/cancel"),
                None,
            )
            .await;
        }
        // Inside the span the clock owes, so the transient status is what the export is
        // judged against.
        tokio::time::sleep(WITHIN_THE_DUE_INSTANT).await;

        let refused = call(
            &emulator,
            "us-west-2",
            "PUT",
            "/export-search-jobs",
            Some(json!({
                "SearchJobIdentifier": search_job_identifier,
                "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "b"}}
            })),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: an export of a search job that has not settled must be 400, got {} ({:?})",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the refusal must carry ValidationException, got {:?}",
            refused.error_code
        );
        let message = refused.string("message");
        assert!(
            message.contains(expected_status),
            "case {case:?}: the refusal must name the status the job is in ({expected_status}), got {message:?}"
        );
        for permitted in state::EXPORTABLE_SEARCH_JOB_STATES {
            assert!(
                message.contains(permitted),
                "case {case:?}: the refusal must name the statuses export permits ({permitted}), got {message:?}"
            );
        }
        assert_eq!(
            stored_export_jobs(&engine),
            0,
            "case {case:?}: a refused start must create no export job, store holds {}",
            stored_export_jobs(&engine)
        );

        // Once the search job settles, the very same request is accepted: the refusal was
        // about the job's status and nothing else.
        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let accepted = call(
            &emulator,
            "us-west-2",
            "PUT",
            "/export-search-jobs",
            Some(json!({
                "SearchJobIdentifier": search_job_identifier,
                "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "b"}}
            })),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the same export must be accepted once the job has settled, got {} ({:?})",
            accepted.status, accepted.body
        );
    }
}

#[tokio::test]
async fn a_client_token_is_idempotent_for_the_same_body_and_conflicts_for_a_different_one() {
    let (emulator, engine) = emulator_with_engine();
    let (first_job, _) =
        settled_search_job(&emulator, "us-west-2", "token", Settled::Completed).await;
    let (second_job, _) =
        settled_search_job(&emulator, "us-west-2", "token", Settled::Stopped).await;

    let token = "11111111-2222-3333-4444-555555555555";
    let body = json!({
        "ClientToken": token,
        "SearchJobIdentifier": first_job,
        "ExportSpecification": {
            "s3ExportSpecification": {"DestinationBucket": "bucket", "DestinationPrefix": "p"}
        },
        "RoleArn": "arn:aws:iam::123456789012:role/export",
        "Tags": {"team": "data"}
    });

    let first = call(&emulator, "us-west-2", "PUT", "/export-search-jobs", Some(body.clone())).await;
    assert_eq!(
        first.status, 200,
        "the first call under a token must succeed, got {} ({:?})",
        first.status, first.body
    );
    let identifier = first.string("ExportJobIdentifier");

    // The same body under the same token, with its members reordered: a repeat, not a
    // second export job.
    let reordered = json!({
        "ExportSpecification": {
            "s3ExportSpecification": {"DestinationPrefix": "p", "DestinationBucket": "bucket"}
        },
        "Tags": {"team": "data"},
        "RoleArn": "arn:aws:iam::123456789012:role/export",
        "SearchJobIdentifier": first_job,
        "ClientToken": token
    });
    let repeat = call(&emulator, "us-west-2", "PUT", "/export-search-jobs", Some(reordered)).await;
    assert_eq!(
        repeat.status, 200,
        "a repeat under the same token must succeed, got {} ({:?})",
        repeat.status, repeat.body
    );
    assert_eq!(
        repeat.string("ExportJobIdentifier"),
        identifier,
        "a repeat must answer with the export job the first call minted, got {:?}",
        repeat.body
    );
    assert_eq!(
        stored_export_jobs(&engine),
        1,
        "a repeat must create no second export job, store holds {}",
        stored_export_jobs(&engine)
    );

    // A body that differs in ANY member the request carries is a conflict under a token
    // that already names an export job.
    let mut different_bucket = body.clone();
    different_bucket["ExportSpecification"]["s3ExportSpecification"]["DestinationBucket"] =
        json!("other-bucket");
    let mut dropped_prefix = body.clone();
    dropped_prefix["ExportSpecification"] =
        json!({"s3ExportSpecification": {"DestinationBucket": "bucket"}});
    let mut other_role = body.clone();
    other_role["RoleArn"] = json!("arn:aws:iam::123456789012:role/other");
    let mut other_tags = body.clone();
    other_tags["Tags"] = json!({"team": "ops"});
    let mut other_job = body.clone();
    other_job["SearchJobIdentifier"] = json!(second_job);

    let cases = [
        ("a different destination bucket", different_bucket),
        ("no destination prefix", dropped_prefix),
        ("a different role", other_role),
        ("different tags", other_tags),
        ("a different search job", other_job),
    ];
    for (case, conflicting) in cases {
        let conflict = call(
            &emulator,
            "us-west-2",
            "PUT",
            "/export-search-jobs",
            Some(conflicting),
        )
        .await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: the same token with a different body must conflict, got {} ({:?})",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error_code.as_deref(),
            Some("ConflictException"),
            "case {case:?}: the refusal must carry ConflictException, got {:?}",
            conflict.error_code
        );
        assert_eq!(
            stored_export_jobs(&engine),
            1,
            "case {case:?}: a conflicting start must create nothing, store holds {}",
            stored_export_jobs(&engine)
        );
    }
}

#[tokio::test]
async fn an_export_that_would_have_to_transmit_items_ends_in_the_modelled_failed_status() {
    let (emulator, engine) = emulator_with_engine();
    let (search_job_identifier, _) =
        settled_search_job(&emulator, "us-west-2", "with results", Settled::Completed).await;

    // Give the settled search job a result set, which nothing in this emulator can scan
    // its way to: an export of it would have to transmit objects into the destination
    // bucket, and that is the transmission the emulator cannot perform. One row per
    // `ResultItem` member of the union, so the outcome does not depend on the item type.
    {
        let state = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
        let mut state = state.lock();
        let job = state
            .job_mut(&search_job_identifier)
            .expect("the settled search job is in the store");
        job.results = vec![
            types::ResultItem::S3ResultItem(types::S3ResultItem {
                object_key: Some("reports/january.csv".to_string()),
                object_size: Some(2_048),
                ..types::S3ResultItem::default()
            }),
            types::ResultItem::EBSResultItem(types::EBSResultItem {
                file_path: Some("/var/log/syslog".to_string()),
                file_size: Some(4_096),
                ..types::EBSResultItem::default()
            }),
        ];
    }

    let started = call(
        &emulator,
        "us-west-2",
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "SearchJobIdentifier": search_job_identifier,
            "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "bucket"}}
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the export of a search job with results is accepted, got {} ({:?})",
        started.status, started.body
    );
    let identifier = started.string("ExportJobIdentifier");

    // While it is RUNNING it carries no StatusMessage: the model documents that member as
    // the text a FAILED export comes with, so the message travels with the status.
    tokio::time::sleep(WITHIN_THE_DUE_INSTANT).await;
    let running = get_export_job(&emulator, "us-west-2", &identifier).await;
    assert_eq!(
        running.string("Status"),
        state::RUNNING,
        "the export is RUNNING inside the span the clock owes, got {:?}",
        running.get("Status")
    );
    assert!(
        running.get("StatusMessage").is_null(),
        "a RUNNING export must not report a failure's StatusMessage, got {:?}",
        running.get("StatusMessage")
    );

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let failed = get_export_job(&emulator, "us-west-2", &identifier).await;
    assert_eq!(
        failed.string("Status"),
        state::FAILED,
        "an export whose items cannot be transmitted ends FAILED, got {:?}",
        failed.get("Status")
    );
    let message = failed.string("StatusMessage");
    assert!(
        message.contains("cannot transmit"),
        "a FAILED export must say why it failed, got {message:?}"
    );
    assert!(
        failed.get("CompletionTime").as_f64().is_some(),
        "a FAILED export carries the instant it ended, got {:?}",
        failed.get("CompletionTime")
    );
}

#[tokio::test]
async fn the_statuses_this_slice_uses_are_the_ones_its_model_declares() {
    // Catalogue source: the `ExportJobStatus` and `SearchJobState` enums, as
    // `types::EXPORT_JOB_STATUS_VALUES` / `types::SEARCH_JOB_STATE_VALUES` render them
    // (smithy_query query="shape" shape="ExportJobStatus").
    for status in [state::RUNNING, state::COMPLETED, state::FAILED] {
        assert!(
            types::EXPORT_JOB_STATUS_VALUES.contains(&status),
            "case {status:?}: an export job's status must be a modelled ExportJobStatus, which is {:?}",
            types::EXPORT_JOB_STATUS_VALUES
        );
    }
    for status in state::EXPORTABLE_SEARCH_JOB_STATES {
        assert!(
            types::SEARCH_JOB_STATE_VALUES.contains(status),
            "case {status:?}: a status export is permitted from must be a modelled SearchJobState, which is {:?}",
            types::SEARCH_JOB_STATE_VALUES
        );
        assert!(
            state::TERMINAL_STATES.contains(status),
            "case {status:?}: export is permitted only from a status the search job never leaves, and {:?} are those",
            state::TERMINAL_STATES
        );
    }
}

#[tokio::test]
async fn starting_an_export_past_the_store_cap_is_refused_with_the_quota_error() {
    let (emulator, engine) = emulator_with_engine();
    let (search_job_identifier, search_job_arn) =
        settled_search_job(&emulator, "us-west-2", "cap", Settled::Completed).await;

    // Seed the store to ONE short of the cap, so the next two calls walk the boundary.
    // Driving `MAX_EXPORT_JOBS` requests would only be a slower way to reach this state.
    {
        let state = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
        let mut state = state.lock();
        for index in 0..state::MAX_EXPORT_JOBS - 1 {
            let identifier = format!("seeded-{index}");
            state.insert_export_job(state::ExportJob {
                identifier: identifier.clone(),
                arn: state::export_job_arn("backup-search", "us-west-2", &identifier),
                search_job_identifier: search_job_identifier.clone(),
                search_job_arn: search_job_arn.clone(),
                status: state::COMPLETED.to_string(),
                creation_time: state::timestamp(std::time::SystemTime::now()),
                completion_time: None,
                status_message: None,
                export_specification: types::ExportSpecification::s3ExportSpecification(
                    types::S3ExportSpecification {
                        destination_bucket: "seeded".to_string(),
                        destination_prefix: None,
                    },
                ),
                role_arn: None,
                tags: std::collections::HashMap::new(),
                client_token: None,
                request_fingerprint: identifier,
                next_status_due: None,
            });
        }
    }

    let body = json!({
        "SearchJobIdentifier": search_job_identifier,
        "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "bucket"}}
    });
    let at_the_cap = call(
        &emulator,
        "us-west-2",
        "PUT",
        "/export-search-jobs",
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        at_the_cap.status, 200,
        "the call that fills the store to its cap must still succeed, got {} ({:?})",
        at_the_cap.status, at_the_cap.body
    );

    let past_the_cap = call(&emulator, "us-west-2", "PUT", "/export-search-jobs", Some(body)).await;
    assert_eq!(
        past_the_cap.status, 402,
        "past the cap StartSearchResultExportJob must refuse with the modelled quota error, got {} ({:?})",
        past_the_cap.status, past_the_cap.body
    );
    assert_eq!(
        past_the_cap.error_code.as_deref(),
        Some("ServiceQuotaExceededException"),
        "the refusal must carry ServiceQuotaExceededException, got {:?}",
        past_the_cap.error_code
    );
}
