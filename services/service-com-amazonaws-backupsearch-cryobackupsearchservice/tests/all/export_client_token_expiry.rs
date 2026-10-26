// SPDX-License-Identifier: CC0-1.0

//! What happens to an EXPORT job's `ClientToken` AT and AFTER the end of the validity
//! window the model publishes on the member, driven over HTTP exactly as a client drives
//! it.
//!
//! The rule under test: a `ClientToken` of this service takes part in idempotency only
//! for the published window, measured from the request that FIRST used it — so the window
//! is a property of that one request and nothing a later caller does moves it. Three
//! consequences this suite pins, each over a TABLE whose rows differ in what the rule
//! generalises over (the token's form, the request body, the signing region, the status
//! the exported search job settled to, and the token's age relative to the published
//! bound):
//!
//! 1. a repeat served from inside the window does not EXTEND it: the stored token still
//!    stops taking part in idempotency the published span after the first call;
//! 2. once the window has closed the token names no earlier export job, so the next call
//!    carrying it mints one and THAT record — with its own fresh window — is what a
//!    further repeat answers with and what a differing body conflicts against, while the
//!    expired call's export job is left untouched;
//! 3. one token's window closing says nothing about another's: a live token still answers
//!    its own repeat.
//!
//! A window measured in hours cannot be waited out, so a row AGES one stored token: it
//! re-mints it as a token carried by a request that completed that long ago, which is the
//! same record the service would hold had the call been made then. Nothing else about the
//! record changes.
//!
//! The HTTP harness is the search-job suite's and the exportable search job is the
//! export-job suite's, so there is one of each rather than one per suite.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_jobs::{passable_role, settled_search_job, stored_export_jobs, Settled};
use crate::search_jobs::{call, emulator_with_engine};

/// The smallest step these rows take either side of an instant, big enough to swamp the
/// milliseconds the real clock advances between ageing a token and asserting on it.
const ONE_SECOND: Duration = Duration::from_secs(1);

/// One row: an export request under a token, and how long before now the request that
/// first used that token completed.
struct Row {
    case: &'static str,
    region: &'static str,
    settled: Settled,
    token: &'static str,
    /// The `ExportSpecification` and the optional members the body carries.
    specification: Value,
    role_arn: Option<String>,
    tags: Option<Value>,
    age: Duration,
}

/// The `StartSearchResultExportJob` body for one row, as a client sends it.
fn body_for(row: &Row, search_job_identifier: &str) -> Value {
    let mut body = json!({
        "ClientToken": row.token,
        "SearchJobIdentifier": search_job_identifier,
        "ExportSpecification": row.specification.clone(),
    });
    if let Some(role_arn) = row.role_arn.clone() {
        body["RoleArn"] = json!(role_arn);
    }
    if let Some(tags) = row.tags.clone() {
        body["Tags"] = tags;
    }
    body
}

/// Rows whose token is still INSIDE the published window: a moment old, a quarter and a
/// half of the way through it, and a minute short of the bound.
fn rows_inside_the_window() -> Vec<Row> {
    vec![
        Row {
            case: "a token used a moment ago, bucket only",
            region: "us-west-2",
            settled: Settled::Completed,
            token: "fresh-export-token",
            specification: json!({"s3ExportSpecification": {"DestinationBucket": "exports"}}),
            role_arn: None,
            tags: None,
            age: Duration::ZERO,
        },
        Row {
            case: "a quarter of the way through the window, bucket and prefix, tagged",
            region: "eu-west-1",
            settled: Settled::Stopped,
            token: "11111111-2222-3333-4444-555555555555",
            specification: json!({
                "s3ExportSpecification": {
                    "DestinationBucket": "quarter.bucket",
                    "DestinationPrefix": "exports/q1/"
                }
            }),
            role_arn: None,
            tags: Some(json!({"team": "data"})),
            age: state::CLIENT_TOKEN_VALIDITY.checked_div(4).unwrap(),
        },
        Row {
            case: "half-way through the window, with a role the service can be passed",
            region: "ap-southeast-2",
            settled: Settled::Completed,
            token: "half.way/token+1",
            specification: json!({"s3ExportSpecification": {"DestinationBucket": "half-bucket"}}),
            role_arn: Some(passable_role("aws", "backup-search-export")),
            tags: Some(json!({"env": "test", "owner": "backup"})),
            age: state::CLIENT_TOKEN_VALIDITY.checked_div(2).unwrap(),
        },
        Row {
            case: "a minute short of the bound, nested prefix",
            region: "us-east-1",
            settled: Settled::Stopped,
            token: "almost-stale-export-token",
            specification: json!({
                "s3ExportSpecification": {
                    "DestinationBucket": "edge-bucket",
                    "DestinationPrefix": "a/b/c/"
                }
            }),
            role_arn: Some(passable_role("aws-us-gov", "service-role/export")),
            tags: None,
            age: state::CLIENT_TOKEN_VALIDITY.saturating_sub(Duration::from_secs(60)),
        },
    ]
}

/// Rows whose token is AT or PAST the published bound, which is where it stops taking
/// part in idempotency.
fn rows_past_the_window() -> Vec<Row> {
    vec![
        Row {
            case: "a token exactly its full validity old, bucket only",
            region: "us-west-2",
            settled: Settled::Completed,
            token: "at-the-bound-export-token",
            specification: json!({"s3ExportSpecification": {"DestinationBucket": "bound-bucket"}}),
            role_arn: None,
            tags: None,
            age: state::CLIENT_TOKEN_VALIDITY,
        },
        Row {
            case: "a token a second past the bound, bucket and prefix, tagged",
            region: "eu-central-1",
            settled: Settled::Stopped,
            token: "22222222-3333-4444-5555-666666666666",
            specification: json!({
                "s3ExportSpecification": {
                    "DestinationBucket": "past.the.bound",
                    "DestinationPrefix": "exports/"
                }
            }),
            role_arn: Some(passable_role("aws", "export")),
            tags: Some(json!({"team": "ops"})),
            age: state::CLIENT_TOKEN_VALIDITY.saturating_add(ONE_SECOND),
        },
        Row {
            case: "a token a week old, with a role and two tags",
            region: "us-east-1",
            settled: Settled::Completed,
            token: "week.old/token",
            specification: json!({"s3ExportSpecification": {"DestinationBucket": "weekly"}}),
            role_arn: Some(passable_role("aws", "weekly-export")),
            tags: Some(json!({"env": "test", "owner": "backup"})),
            age: Duration::from_secs(7 * 24 * 60 * 60),
        },
    ]
}

/// Re-mint the export-job `ClientToken` whose value is `token` as one carried by a request
/// that completed `age` before now, leaving every other record — and every other token —
/// exactly as it was. Answers the instant the re-minted window is measured from.
fn age_export_token(engine: &engine::InMemoryEngine, token: &str, age: Duration) -> SystemTime {
    let completed = SystemTime::now()
        .checked_sub(age)
        .expect("an age the system clock can be moved back by");
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let mut store = store.lock();
    for job in store.export_jobs.values_mut() {
        if job
            .client_token
            .as_ref()
            .is_some_and(|held| held.value() == token)
        {
            job.client_token = Some(state::ClientToken::minted(token.to_string(), completed));
        }
    }
    completed
}

/// Whether the export job `identifier` still holds a token taking part in idempotency at
/// `at`, read off the stored record.
fn token_valid_at(
    engine: &engine::InMemoryEngine,
    identifier: &str,
    at: SystemTime,
) -> Option<bool> {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let store = store.lock();
    store
        .export_jobs
        .get(identifier)
        .map(|job| job.client_token.as_ref().is_some_and(|held| held.is_valid_at(at)))
}

#[tokio::test]
async fn a_repeat_served_from_inside_the_window_does_not_extend_it() {
    for row in rows_inside_the_window() {
        let (emulator, engine) = emulator_with_engine();
        let (search_job, _) =
            settled_search_job(&emulator, &engine, row.region, row.case, row.settled).await;
        let body = body_for(&row, &search_job);

        let first = call(
            &emulator,
            row.region,
            "PUT",
            "/export-search-jobs",
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first StartSearchResultExportJob must be accepted, got {} ({:?})",
            row.case, first.status, first.body
        );
        let identifier = first.string("ExportJobIdentifier");

        let completed = age_export_token(&engine, row.token, row.age);

        let repeat = call(
            &emulator,
            row.region,
            "PUT",
            "/export-search-jobs",
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            repeat.status, 200,
            "case {:?}: inside the window a repeat must be accepted, got {} ({:?})",
            row.case, repeat.status, repeat.body
        );
        assert_eq!(
            repeat.string("ExportJobIdentifier"),
            identifier,
            "case {:?}: inside the window a repeat answers with the first export job, got {:?}",
            row.case,
            repeat.body
        );
        assert_eq!(
            stored_export_jobs(&engine),
            1,
            "case {:?}: inside the window a repeat creates no second export job, store holds {}",
            row.case,
            stored_export_jobs(&engine)
        );

        // The window is still the one the FIRST call opened: it ends the published span
        // after that request completed, whatever the repeat did.
        let closes_at = completed + state::CLIENT_TOKEN_VALIDITY;
        let just_inside = closes_at - ONE_SECOND;
        let just_past = closes_at + ONE_SECOND;
        assert_eq!(
            token_valid_at(&engine, &identifier, just_inside),
            Some(true),
            "case {:?}: a second short of the published bound the stored token is still live, got {:?}",
            row.case,
            token_valid_at(&engine, &identifier, just_inside)
        );
        assert_eq!(
            token_valid_at(&engine, &identifier, just_past),
            Some(false),
            "case {:?}: the repeat must not carry the window past the published bound, got {:?}",
            row.case,
            token_valid_at(&engine, &identifier, just_past)
        );
    }
}

#[tokio::test]
async fn past_the_window_the_export_job_the_token_mints_takes_over_idempotency() {
    for row in rows_past_the_window() {
        let (emulator, engine) = emulator_with_engine();
        let (search_job, _) =
            settled_search_job(&emulator, &engine, row.region, row.case, row.settled).await;
        let body = body_for(&row, &search_job);

        let first = call(
            &emulator,
            row.region,
            "PUT",
            "/export-search-jobs",
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first StartSearchResultExportJob must be accepted, got {} ({:?})",
            row.case, first.status, first.body
        );
        let first_identifier = first.string("ExportJobIdentifier");

        age_export_token(&engine, row.token, row.age);

        // Past the window the token names no earlier export job, so this is a new
        // request and it mints one.
        let second = call(
            &emulator,
            row.region,
            "PUT",
            "/export-search-jobs",
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            second.status, 200,
            "case {:?}: past the window the call under the token must be accepted, got {} ({:?})",
            row.case, second.status, second.body
        );
        let second_identifier = second.string("ExportJobIdentifier");
        assert_ne!(
            second_identifier, first_identifier,
            "case {:?}: past the window the call mints its own export job, got {:?}",
            row.case, second.body
        );
        assert_eq!(
            stored_export_jobs(&engine),
            2,
            "case {:?}: past the window the store holds both export jobs, got {}",
            row.case,
            stored_export_jobs(&engine)
        );

        // The record that call minted carries the token's new window, so a further repeat
        // answers with THAT export job rather than the expired call's.
        let repeat = call(
            &emulator,
            row.region,
            "PUT",
            "/export-search-jobs",
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            repeat.string("ExportJobIdentifier"),
            second_identifier,
            "case {:?}: inside the new window a repeat answers with the export job the expired token minted, got {:?}",
            row.case,
            repeat.body
        );
        assert_eq!(
            stored_export_jobs(&engine),
            2,
            "case {:?}: the repeat creates no third export job, store holds {}",
            row.case,
            stored_export_jobs(&engine)
        );

        // And a differing body under the token now conflicts with that record, not with
        // the one the expired call minted.
        let mut different = body.clone();
        different["ExportSpecification"]["s3ExportSpecification"]["DestinationPrefix"] =
            json!("a-differing-prefix/");
        let conflict = call(
            &emulator,
            row.region,
            "PUT",
            "/export-search-jobs",
            Some(different),
        )
        .await;
        assert_eq!(
            conflict.status, 409,
            "case {:?}: inside the new window a differing body under the token conflicts, got {} ({:?})",
            row.case, conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error_code.as_deref(),
            Some("ConflictException"),
            "case {:?}: the refusal carries ConflictException, got {:?}",
            row.case,
            conflict.error_code
        );
        assert_eq!(
            conflict.string("resourceId"),
            second_identifier,
            "case {:?}: the conflict names the export job the live token holds, got {:?}",
            row.case,
            conflict.get("resourceId")
        );
        assert_eq!(
            conflict.string("resourceType"),
            state::EXPORT_JOB_RESOURCE,
            "case {:?}: the conflict names the export-job resource type, got {:?}",
            row.case,
            conflict.get("resourceType")
        );

        // The expired call's export job is untouched by any of this: it still reads back
        // under its own identifier.
        let earlier = call(
            &emulator,
            row.region,
            "GET",
            &format!("/export-search-jobs/{first_identifier}"),
            None,
        )
        .await;
        assert_eq!(
            earlier.status, 200,
            "case {:?}: GetSearchResultExportJob must still report the first export job, got {} ({:?})",
            row.case, earlier.status, earlier.body
        );
        assert_eq!(
            earlier.string("ExportJobIdentifier"),
            first_identifier,
            "case {:?}: the first export job reports its own identifier, got {:?}",
            row.case,
            earlier.body
        );
        assert_eq!(
            earlier.string("ExportJobArn"),
            first.string("ExportJobArn"),
            "case {:?}: the first export job keeps the ARN it was minted with, got {:?}",
            row.case,
            earlier.get("ExportJobArn")
        );
    }
}

#[tokio::test]
async fn one_tokens_window_closing_leaves_every_other_token_live() {
    // Two export jobs of the same settled search job under two tokens, then only the
    // first token aged: a window belongs to the request that opened it, so the second
    // token still answers its own repeat.
    let region = "us-west-2";
    let aged_rows = [
        ("the bound itself", state::CLIENT_TOKEN_VALIDITY),
        (
            "a second past the bound",
            state::CLIENT_TOKEN_VALIDITY.saturating_add(ONE_SECOND),
        ),
    ];

    for (case, age) in aged_rows {
        let (emulator, engine) = emulator_with_engine();
        let (search_job, _) =
            settled_search_job(&emulator, &engine, region, case, Settled::Completed).await;

        let stale_token = "stale-token";
        let live_token = "live-token";
        let stale_body = json!({
            "ClientToken": stale_token,
            "SearchJobIdentifier": search_job,
            "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "stale-bucket"}}
        });
        let live_body = json!({
            "ClientToken": live_token,
            "SearchJobIdentifier": search_job,
            "ExportSpecification": {
                "s3ExportSpecification": {
                    "DestinationBucket": "live-bucket",
                    "DestinationPrefix": "exports/"
                }
            },
            "Tags": {"team": "data"}
        });

        let stale = call(
            &emulator,
            region,
            "PUT",
            "/export-search-jobs",
            Some(stale_body.clone()),
        )
        .await;
        let live = call(
            &emulator,
            region,
            "PUT",
            "/export-search-jobs",
            Some(live_body.clone()),
        )
        .await;
        assert_eq!(
            (stale.status, live.status),
            (200, 200),
            "case {case:?}: both StartSearchResultExportJob calls must be accepted, got {:?} and {:?}",
            stale.body,
            live.body
        );
        let stale_identifier = stale.string("ExportJobIdentifier");
        let live_identifier = live.string("ExportJobIdentifier");

        age_export_token(&engine, stale_token, age);

        let under_stale = call(
            &emulator,
            region,
            "PUT",
            "/export-search-jobs",
            Some(stale_body),
        )
        .await;
        assert_eq!(
            under_stale.status, 200,
            "case {case:?}: the call under the expired token must be accepted, got {} ({:?})",
            under_stale.status, under_stale.body
        );
        assert_ne!(
            under_stale.string("ExportJobIdentifier"),
            stale_identifier,
            "case {case:?}: the expired token mints a new export job, got {:?}",
            under_stale.body
        );

        let under_live = call(
            &emulator,
            region,
            "PUT",
            "/export-search-jobs",
            Some(live_body),
        )
        .await;
        assert_eq!(
            under_live.string("ExportJobIdentifier"),
            live_identifier,
            "case {case:?}: the token still inside its window answers with its own export job, got {:?}",
            under_live.body
        );
        assert_eq!(
            stored_export_jobs(&engine),
            3,
            "case {case:?}: the store holds the two original export jobs and the one the expired token minted, got {}",
            stored_export_jobs(&engine)
        );
    }
}
