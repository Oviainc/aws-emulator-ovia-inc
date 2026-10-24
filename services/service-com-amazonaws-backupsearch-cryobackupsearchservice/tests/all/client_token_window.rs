// SPDX-License-Identifier: CC0-1.0

//! The `ClientToken` VALIDITY WINDOW, driven over HTTP exactly as a client drives it.
//!
//! The rule under test: a `ClientToken` of this service takes part in idempotency only
//! for the window the model publishes on the member, measured from the request that first
//! used it; once the clock has closed that window the token names no earlier record, so a
//! call carrying it is a NEW request — it mints a resource, and a body differing from the
//! expired call's is no longer a conflict.
//!
//! Every assertion is made over a TABLE whose rows differ in what the rule generalises
//! over — the operation that takes the token (both of them do), the token's form, the
//! request body, the signing region, and the token's AGE relative to the published bound
//! (fresh, half-way, one second inside the bound, exactly at it, and well past it) —
//! rather than over the one request the issue happened to cite.
//!
//! A window measured in hours cannot be waited out, so a row AGES the stored record: it
//! re-mints the token the store holds as one carried by a request that completed that long
//! ago, which is the same record the service would hold had the call been made then.
//! Nothing else about the record changes.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_jobs::{settled_search_job, stored_export_jobs, Settled};
use crate::search_jobs::{call, emulator_with_engine};

/// One row of the table: a `StartSearchJob` body under a token of some age.
struct Row {
    case: &'static str,
    region: &'static str,
    token: &'static str,
    body: Value,
    /// How long before now the request that first used the token completed.
    age: Duration,
    /// Whether the token still takes part in idempotency at that age.
    open: bool,
}

/// One second, the smallest step these rows take either side of the published bound.
const ONE_SECOND: Duration = Duration::from_secs(1);

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a token used a moment ago, bare S3 scope",
            region: "us-west-2",
            token: "token-fresh",
            body: json!({"SearchScope": {"BackupResourceTypes": ["S3"]}}),
            age: Duration::ZERO,
            open: true,
        },
        Row {
            case: "a token half-way through its window, named and tagged EBS scope",
            region: "eu-west-1",
            token: "11111111-2222-3333-4444-555555555555",
            body: json!({
                "Name": "half-way",
                "Tags": {"env": "test"},
                "SearchScope": {"BackupResourceTypes": ["EBS"]},
                "ItemFilters": {"EBSItemFilters": [{
                    "FilePaths": [{"Value": "/var/log", "Operator": "BEGINS_WITH"}]
                }]}
            }),
            age: state::CLIENT_TOKEN_VALIDITY.checked_div(2).unwrap(),
            open: true,
        },
        Row {
            case: "a token one second inside the published bound, S3 scope with source arns",
            region: "us-east-1",
            token: "token-just-inside",
            body: json!({
                "Name": "just-inside",
                "SearchScope": {
                    "BackupResourceTypes": ["S3"],
                    "SourceResourceArns": ["arn:aws:s3:::alpha", "arn:aws:s3:::beta"]
                }
            }),
            age: state::CLIENT_TOKEN_VALIDITY.saturating_sub(ONE_SECOND),
            open: true,
        },
        Row {
            case: "a token exactly its full validity old, bare S3 scope",
            region: "ap-southeast-2",
            token: "token-at-the-bound",
            body: json!({"SearchScope": {"BackupResourceTypes": ["S3"]}}),
            age: state::CLIENT_TOKEN_VALIDITY,
            open: false,
        },
        Row {
            case: "a token a day old, named EBS scope with an encryption key",
            region: "eu-central-1",
            token: "token-stale",
            body: json!({
                "Name": "stale",
                "EncryptionKeyArn":
                    "arn:aws:kms:eu-central-1:000000000000:key/11111111-2222-3333-4444-555555555555",
                "SearchScope": {"BackupResourceTypes": ["EBS"]}
            }),
            age: Duration::from_secs(24 * 60 * 60),
            open: false,
        },
    ]
}

/// How many search jobs the shared store holds, read off the store itself so the count
/// does not depend on a listing's own filters.
fn stored_search_jobs(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>()
        .lock()
        .search_jobs
        .len()
}

/// Re-mint every `ClientToken` the store holds — on a search job or on an export job — as
/// one carried by a request that completed `age` before now, so the clock stands `age`
/// into the window without a test waiting for it.
fn age_held_client_tokens(engine: &engine::InMemoryEngine, age: Duration) {
    let completed = SystemTime::now()
        .checked_sub(age)
        .expect("an age the system clock can be moved back by");
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let mut store = store.lock();
    for job in store.search_jobs.values_mut() {
        if let Some(held) = job.client_token.as_ref() {
            job.client_token = Some(state::ClientToken::minted(
                held.value().to_string(),
                completed,
            ));
        }
    }
    for job in store.export_jobs.values_mut() {
        if let Some(held) = job.client_token.as_ref() {
            job.client_token = Some(state::ClientToken::minted(
                held.value().to_string(),
                completed,
            ));
        }
    }
}

/// The body with its `ClientToken` set, as a client sends it.
fn under_token(body: &Value, token: &str) -> Value {
    let mut body = body.clone();
    body["ClientToken"] = json!(token);
    body
}

#[tokio::test]
async fn the_window_a_client_token_is_valid_for_is_the_one_the_model_publishes() {
    // The member's own documentation on both operations that take a `ClientToken` states
    // the window: valid for 8 hours after the first request that uses it is completed
    // (`smithy_query(query="doc", shape="StartSearchJobInput", member="ClientToken")`).
    assert_eq!(
        state::CLIENT_TOKEN_VALIDITY,
        Duration::from_secs(8 * 60 * 60),
        "the validity window is the modelled 8 hours, got {:?}",
        state::CLIENT_TOKEN_VALIDITY
    );

    // The bound itself and one step either side of it, on the value the records hold: the
    // window is open up to but not including the instant it closes.
    let completed = SystemTime::now();
    let token = state::ClientToken::minted("bound-probe".to_string(), completed);
    let probes = [
        ("the instant the request completed", Duration::ZERO, true),
        (
            "one second inside the bound",
            state::CLIENT_TOKEN_VALIDITY.saturating_sub(ONE_SECOND),
            true,
        ),
        ("the bound itself", state::CLIENT_TOKEN_VALIDITY, false),
        (
            "one second past the bound",
            state::CLIENT_TOKEN_VALIDITY.saturating_add(ONE_SECOND),
            false,
        ),
    ];
    for (case, elapsed, valid) in probes {
        let at = completed + elapsed;
        assert_eq!(
            token.is_valid_at(at),
            valid,
            "case {case:?}: a token {elapsed:?} after the request that used it completed \
             must be valid={valid}, got {}",
            token.is_valid_at(at)
        );
        assert_eq!(
            token.is_live("bound-probe", at),
            valid,
            "case {case:?}: the token matches its own value only inside the window, got {}",
            token.is_live("bound-probe", at)
        );
    }
    assert!(
        !token.is_live("another-token", completed),
        "a token never matches a different value, got true"
    );
}

#[tokio::test]
async fn a_repeat_under_a_search_job_client_token_is_the_same_job_only_inside_the_window() {
    for row in rows() {
        let (emulator, engine) = emulator_with_engine();
        let body = under_token(&row.body, row.token);

        let first = call(&emulator, row.region, "PUT", "/search-jobs", Some(body.clone())).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first StartSearchJob must be accepted, got {} ({:?})",
            row.case, first.status, first.body
        );
        let first_identifier = first.string("SearchJobIdentifier");
        let first_creation = first.get("CreationTime").as_f64();

        // Reading the job does not consume or expire its token: the window is driven by
        // the clock alone.
        let job_uri = format!("/search-jobs/{first_identifier}");
        let fetched = call(&emulator, row.region, "GET", &job_uri, None).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: GetSearchJob must report the minted job, got {} ({:?})",
            row.case, fetched.status, fetched.body
        );

        age_held_client_tokens(&engine, row.age);

        let repeat = call(&emulator, row.region, "PUT", "/search-jobs", Some(body.clone())).await;
        assert_eq!(
            repeat.status, 200,
            "case {:?}: a repeat under the token must be accepted, got {} ({:?})",
            row.case, repeat.status, repeat.body
        );
        let repeat_identifier = repeat.string("SearchJobIdentifier");

        if row.open {
            assert_eq!(
                repeat_identifier, first_identifier,
                "case {:?}: inside the window a repeat answers with the first job, got {:?}",
                row.case, repeat.body
            );
            assert_eq!(
                repeat.get("CreationTime").as_f64(),
                first_creation,
                "case {:?}: inside the window a repeat answers with the first job's CreationTime, got {:?}",
                row.case,
                repeat.get("CreationTime")
            );
            assert_eq!(
                stored_search_jobs(&engine),
                1,
                "case {:?}: inside the window a repeat creates no second job, store holds {}",
                row.case,
                stored_search_jobs(&engine)
            );
        } else {
            assert_ne!(
                repeat_identifier, first_identifier,
                "case {:?}: past the window the token is a new request and must mint another job, got {:?}",
                row.case, repeat.body
            );
            assert_eq!(
                stored_search_jobs(&engine),
                2,
                "case {:?}: past the window the store holds both jobs, got {}",
                row.case,
                stored_search_jobs(&engine)
            );

            // Both records read back out of the store, each with its own identifier and
            // creation time: the second call minted a job rather than re-answering one.
            let minted = call(
                &emulator,
                row.region,
                "GET",
                &format!("/search-jobs/{repeat_identifier}"),
                None,
            )
            .await;
            assert_eq!(
                minted.status, 200,
                "case {:?}: GetSearchJob must report the newly minted job, got {} ({:?})",
                row.case, minted.status, minted.body
            );
            assert_eq!(
                minted.string("SearchJobIdentifier"),
                repeat_identifier,
                "case {:?}: the new job reports its own identifier, got {:?}",
                row.case,
                minted.body
            );
            assert_eq!(
                minted.get("SearchScope"),
                &row.body["SearchScope"],
                "case {:?}: the new job reports the submitted SearchScope, got {:?}",
                row.case,
                minted.get("SearchScope")
            );
            let earlier = call(&emulator, row.region, "GET", &job_uri, None).await;
            assert_eq!(
                earlier.get("CreationTime").as_f64(),
                first_creation,
                "case {:?}: the expired call's job is untouched, got {:?}",
                row.case,
                earlier.get("CreationTime")
            );
        }
    }
}

#[tokio::test]
async fn a_different_body_under_a_search_job_client_token_conflicts_only_inside_the_window() {
    for row in rows() {
        let (emulator, engine) = emulator_with_engine();
        let body = under_token(&row.body, row.token);

        let first = call(&emulator, row.region, "PUT", "/search-jobs", Some(body.clone())).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first StartSearchJob must be accepted, got {} ({:?})",
            row.case, first.status, first.body
        );
        let first_identifier = first.string("SearchJobIdentifier");

        age_held_client_tokens(&engine, row.age);

        // The same token over a body that differs in a member the request carries.
        let mut different = body.clone();
        different["Name"] = json!("a-different-name");
        let answer = call(&emulator, row.region, "PUT", "/search-jobs", Some(different)).await;

        if row.open {
            assert_eq!(
                answer.status, 409,
                "case {:?}: inside the window a different body under the token conflicts, got {} ({:?})",
                row.case, answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ConflictException"),
                "case {:?}: the refusal carries ConflictException, got {:?}",
                row.case,
                answer.error_code
            );
            assert_eq!(
                stored_search_jobs(&engine),
                1,
                "case {:?}: a refused start writes nothing, store holds {}",
                row.case,
                stored_search_jobs(&engine)
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {:?}: past the window a different body under the token is accepted, got {} ({:?})",
                row.case, answer.status, answer.body
            );
            assert_ne!(
                answer.string("SearchJobIdentifier"),
                first_identifier,
                "case {:?}: past the window the accepted call mints its own job, got {:?}",
                row.case,
                answer.body
            );
            assert_eq!(
                stored_search_jobs(&engine),
                2,
                "case {:?}: past the window the store holds both jobs, got {}",
                row.case,
                stored_search_jobs(&engine)
            );
        }
    }
}

#[tokio::test]
async fn an_export_job_client_token_follows_the_same_window() {
    // The model states the window on BOTH `ClientToken` members, so the rule is the
    // service's and not one operation's: the ages either side of the published bound are
    // asserted on `StartSearchResultExportJob` too.
    let export_rows = [
        (
            "one second inside the bound",
            state::CLIENT_TOKEN_VALIDITY.saturating_sub(ONE_SECOND),
            true,
        ),
        ("the bound itself", state::CLIENT_TOKEN_VALIDITY, false),
        (
            "a day past the bound",
            Duration::from_secs(24 * 60 * 60),
            false,
        ),
    ];

    for (case, age, open) in export_rows {
        let (emulator, engine) = emulator_with_engine();
        let (search_job, _) =
            settled_search_job(&emulator, &engine, "us-west-2", case, Settled::Completed).await;
        let token = "export-window-token";
        let body = json!({
            "ClientToken": token,
            "SearchJobIdentifier": search_job,
            "ExportSpecification": {
                "s3ExportSpecification": {"DestinationBucket": "window-bucket"}
            },
            "Tags": {"team": "data"}
        });

        let first = call(
            &emulator,
            "us-west-2",
            "PUT",
            "/export-search-jobs",
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first StartSearchResultExportJob must be accepted, got {} ({:?})",
            first.status, first.body
        );
        let first_identifier = first.string("ExportJobIdentifier");

        age_held_client_tokens(&engine, age);

        let repeat = call(
            &emulator,
            "us-west-2",
            "PUT",
            "/export-search-jobs",
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            repeat.status, 200,
            "case {case:?}: a repeat under the token must be accepted, got {} ({:?})",
            repeat.status, repeat.body
        );
        let repeat_identifier = repeat.string("ExportJobIdentifier");

        if open {
            assert_eq!(
                repeat_identifier, first_identifier,
                "case {case:?}: inside the window a repeat answers with the first export job, got {:?}",
                repeat.body
            );
            assert_eq!(
                stored_export_jobs(&engine),
                1,
                "case {case:?}: inside the window a repeat creates no second export job, store holds {}",
                stored_export_jobs(&engine)
            );
        } else {
            assert_ne!(
                repeat_identifier, first_identifier,
                "case {case:?}: past the window the token is a new request and must mint another export job, got {:?}",
                repeat.body
            );
            assert_eq!(
                stored_export_jobs(&engine),
                2,
                "case {case:?}: past the window the store holds both export jobs, got {}",
                stored_export_jobs(&engine)
            );

            // The minted record reads back out of the store under its own identifier.
            let minted = call(
                &emulator,
                "us-west-2",
                "GET",
                &format!("/export-search-jobs/{repeat_identifier}"),
                None,
            )
            .await;
            assert_eq!(
                minted.status, 200,
                "case {case:?}: GetSearchResultExportJob must report the newly minted export job, got {} ({:?})",
                minted.status, minted.body
            );
            assert_eq!(
                minted.string("ExportJobIdentifier"),
                repeat_identifier,
                "case {case:?}: the new export job reports its own identifier, got {:?}",
                minted.body
            );
        }

        // A body differing under the SAME token: a conflict only while the token still
        // names the earlier export job.
        let (emulator, engine) = emulator_with_engine();
        let (search_job, _) =
            settled_search_job(&emulator, &engine, "us-west-2", case, Settled::Completed).await;
        let body = json!({
            "ClientToken": token,
            "SearchJobIdentifier": search_job,
            "ExportSpecification": {
                "s3ExportSpecification": {"DestinationBucket": "window-bucket"}
            }
        });
        let first = call(
            &emulator,
            "us-west-2",
            "PUT",
            "/export-search-jobs",
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first StartSearchResultExportJob must be accepted, got {} ({:?})",
            first.status, first.body
        );

        age_held_client_tokens(&engine, age);

        let mut different = body.clone();
        different["ExportSpecification"]["s3ExportSpecification"]["DestinationBucket"] =
            json!("other-bucket");
        let answer = call(
            &emulator,
            "us-west-2",
            "PUT",
            "/export-search-jobs",
            Some(different),
        )
        .await;
        if open {
            assert_eq!(
                answer.status, 409,
                "case {case:?}: inside the window a different body under the token conflicts, got {} ({:?})",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ConflictException"),
                "case {case:?}: the refusal carries ConflictException, got {:?}",
                answer.error_code
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: past the window a different body under the token is accepted, got {} ({:?})",
                answer.status, answer.body
            );
            assert_eq!(
                stored_export_jobs(&engine),
                2,
                "case {case:?}: past the window the store holds both export jobs, got {}",
                stored_export_jobs(&engine)
            );
        }
    }
}
