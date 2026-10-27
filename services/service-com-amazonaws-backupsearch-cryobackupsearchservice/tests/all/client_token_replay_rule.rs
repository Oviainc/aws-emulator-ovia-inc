// SPDX-License-Identifier: CC0-1.0

//! The ONE `ClientToken` replay rule both `Start` operations of this service answer a
//! repeat by, driven over HTTP exactly as a client drives it and then asserted directly on
//! the shared decision.
//!
//! The rule under test has three steps: find a held record carrying this token still
//! inside its validity window; answer that stored record when the request's fingerprint
//! matches it; refuse with a conflict when it does not. `StartSearchJob` and
//! `StartSearchResultExportJob` both go through it, so each assertion is made over a TABLE
//! whose rows differ in what the rule generalises over — WHICH operation takes the token,
//! the token's form, the request body, how the body differs, the signing region and (for
//! an export) the status the exported search job settled to — rather than over one
//! operation's request.
//!
//! The last test reaches the shared decision itself at BOTH record types, over the records
//! the HTTP calls persisted, and pins the answer of every step including the two that fall
//! through to a new request: a token the store holds nothing live for, and a token whose
//! window the clock has carried past.
//!
//! The HTTP harness is the search-job suite's and the exportable search job is the
//! export-job suite's, so there is one of each rather than one per suite.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state::TokenedRecord;

use crate::export_jobs::{passable_role, settled_search_job, Settled};
use crate::search_jobs::{call, emulator_with_engine};

/// Which operation of this service takes the `ClientToken` a row is about. Both of them
/// do, so the rule is asserted on each rather than on the one a recording happened to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    SearchJob,
    ExportJob,
}

impl Start {
    /// The operation name the refusal reads, which each handler takes from its own
    /// `Operation::NAME`.
    fn operation(self) -> &'static str {
        match self {
            Self::SearchJob => "StartSearchJob",
            Self::ExportJob => "StartSearchResultExportJob",
        }
    }

    /// The URI the operation is bound to (`OPERATIONS.md`, the restJson1 bindings).
    fn uri(self) -> &'static str {
        match self {
            Self::SearchJob => "/search-jobs",
            Self::ExportJob => "/export-search-jobs",
        }
    }

    /// The response member carrying the identifier of the record the call answers with.
    fn identifier_member(self) -> &'static str {
        match self {
            Self::SearchJob => "SearchJobIdentifier",
            Self::ExportJob => "ExportJobIdentifier",
        }
    }

    /// The `resourceType` a conflict about this record names, taken from the record type's
    /// own declaration rather than restated here.
    fn resource_type(self) -> &'static str {
        match self {
            Self::SearchJob => <state::SearchJob as TokenedRecord>::RESOURCE_TYPE,
            Self::ExportJob => <state::ExportJob as TokenedRecord>::RESOURCE_TYPE,
        }
    }
}

/// One row: a request under a `ClientToken`, and the member a differing body differs in.
struct Row {
    case: &'static str,
    start: Start,
    region: &'static str,
    token: &'static str,
    /// The body, without the `ClientToken` and (for an export) without the
    /// `SearchJobIdentifier`, which the row fills in.
    body: Value,
    /// For an export row: the status the exported search job is brought to first.
    settled: Settled,
    /// How a differing body under the same token differs from `body`.
    differ: fn(&mut Value),
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a search job under a plain token, bare S3 scope, differing in Name",
            start: Start::SearchJob,
            region: "us-west-2",
            token: "replay-token",
            body: json!({"SearchScope": {"BackupResourceTypes": ["S3"]}}),
            settled: Settled::Completed,
            differ: |body| body["Name"] = json!("a-different-name"),
        },
        Row {
            case: "a search job under a uuid token, named and filtered EBS scope, differing in a tag",
            start: Start::SearchJob,
            region: "eu-west-1",
            token: "11111111-2222-3333-4444-555555555555",
            body: json!({
                "Name": "replayed",
                "Tags": {"env": "test"},
                "SearchScope": {"BackupResourceTypes": ["EBS"]},
                "ItemFilters": {"EBSItemFilters": [{
                    "FilePaths": [{"Value": "/var/log", "Operator": "BEGINS_WITH"}]
                }]}
            }),
            settled: Settled::Completed,
            differ: |body| body["Tags"]["env"] = json!("prod"),
        },
        Row {
            case: "a search job under a punctuated token, scope with source arns, differing in the scope",
            start: Start::SearchJob,
            region: "ap-southeast-2",
            token: "replay.token/one+two",
            body: json!({
                "SearchScope": {
                    "BackupResourceTypes": ["S3"],
                    "SourceResourceArns": ["arn:aws:s3:::alpha", "arn:aws:s3:::beta"]
                }
            }),
            settled: Settled::Completed,
            differ: |body| body["SearchScope"]["SourceResourceArns"] = json!(["arn:aws:s3:::alpha"]),
        },
        Row {
            case: "an export of a completed job, bucket only, differing in the destination bucket",
            start: Start::ExportJob,
            region: "us-west-2",
            token: "export-replay-token",
            body: json!({
                "ExportSpecification": {
                    "s3ExportSpecification": {"DestinationBucket": "replay-exports"}
                }
            }),
            settled: Settled::Completed,
            differ: |body| {
                body["ExportSpecification"]["s3ExportSpecification"]["DestinationBucket"] =
                    json!("another-bucket");
            },
        },
        Row {
            case: "an export of a stopped job, bucket and prefix with a role, differing in the prefix",
            start: Start::ExportJob,
            region: "eu-central-1",
            token: "22222222-3333-4444-5555-666666666666",
            body: json!({
                "ExportSpecification": {
                    "s3ExportSpecification": {
                        "DestinationBucket": "replay.exports",
                        "DestinationPrefix": "exports/2026/"
                    }
                }
            }),
            settled: Settled::Stopped,
            differ: |body| {
                body["ExportSpecification"]["s3ExportSpecification"]["DestinationPrefix"] =
                    json!("exports/2027/");
            },
        },
        Row {
            case: "an export of a completed job with tags, differing in the tag set",
            start: Start::ExportJob,
            region: "us-east-1",
            token: "export.replay/token+3",
            body: json!({
                "ExportSpecification": {
                    "s3ExportSpecification": {"DestinationBucket": "tagged-exports"}
                },
                "Tags": {"team": "data"}
            }),
            settled: Settled::Completed,
            differ: |body| body["Tags"] = json!({"team": "ops", "env": "test"}),
        },
    ]
}

/// How many records of the kind `start` mints the shared store holds, read off the store
/// itself so the count does not depend on a listing's own filters.
fn stored(engine: &engine::InMemoryEngine, start: Start) -> usize {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let store = store.lock();
    match start {
        Start::SearchJob => store.search_jobs.len(),
        Start::ExportJob => store.export_jobs.len(),
    }
}

/// An emulator ready for one row, and the body that row's calls send: for an export row
/// that means a search job already brought to a status an export may be started from.
async fn prepared(row: &Row) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Value) {
    let (emulator, engine) = emulator_with_engine();
    let mut body = row.body.clone();
    body["ClientToken"] = json!(row.token);
    if row.start == Start::ExportJob {
        let (search_job, _) =
            settled_search_job(&emulator, &engine, row.region, row.case, row.settled).await;
        body["SearchJobIdentifier"] = json!(search_job);
        // A role is part of an export request a client sends; one in this emulator's own
        // account is a role the service can be passed.
        body["RoleArn"] = json!(passable_role("aws", "backup-search-export"));
    }
    (emulator, engine, body)
}

#[tokio::test]
async fn a_repeat_under_a_live_token_answers_the_stored_record_for_both_start_operations() {
    for row in rows() {
        let (emulator, engine, body) = prepared(&row).await;

        let first = call(&emulator, row.region, "PUT", row.start.uri(), Some(body.clone())).await;
        assert_eq!(
            first.status,
            200,
            "case {:?}: the first {} must be accepted, got {} ({:?})",
            row.case,
            row.start.operation(),
            first.status,
            first.body
        );
        let identifier = first.string(row.start.identifier_member());
        let held = stored(&engine, row.start);

        let repeat = call(&emulator, row.region, "PUT", row.start.uri(), Some(body)).await;
        assert_eq!(
            repeat.status,
            200,
            "case {:?}: a repeat of the same body under the token must be accepted, got {} ({:?})",
            row.case,
            repeat.status,
            repeat.body
        );
        assert_eq!(
            repeat.string(row.start.identifier_member()),
            identifier,
            "case {:?}: the repeat answers with the record the first call minted, got {:?}",
            row.case,
            repeat.body
        );
        assert_eq!(
            stored(&engine, row.start),
            held,
            "case {:?}: the repeat creates nothing, store holds {} where it held {held}",
            row.case,
            stored(&engine, row.start)
        );
    }
}

#[tokio::test]
async fn a_differing_body_under_a_live_token_is_the_same_conflict_for_both_start_operations() {
    for row in rows() {
        let (emulator, engine, body) = prepared(&row).await;

        let first = call(&emulator, row.region, "PUT", row.start.uri(), Some(body.clone())).await;
        assert_eq!(
            first.status,
            200,
            "case {:?}: the first {} must be accepted, got {} ({:?})",
            row.case,
            row.start.operation(),
            first.status,
            first.body
        );
        let identifier = first.string(row.start.identifier_member());
        let held = stored(&engine, row.start);

        let mut different = body;
        (row.differ)(&mut different);
        let answer = call(&emulator, row.region, "PUT", row.start.uri(), Some(different)).await;

        assert_eq!(
            answer.status,
            409,
            "case {:?}: a differing body under the live token conflicts, got {} ({:?})",
            row.case,
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ConflictException"),
            "case {:?}: the refusal carries ConflictException, got {:?}",
            row.case,
            answer.error_code
        );
        assert_eq!(
            answer.string("message"),
            format!(
                "ClientToken '{}' was already used for a different {} request",
                row.token,
                row.start.operation()
            ),
            "case {:?}: the refusal names the token and the operation called, got {:?}",
            row.case,
            answer.get("message")
        );
        assert_eq!(
            answer.string("resourceId"),
            identifier,
            "case {:?}: the refusal names the record the live token holds, got {:?}",
            row.case,
            answer.get("resourceId")
        );
        assert_eq!(
            answer.string("resourceType"),
            row.start.resource_type(),
            "case {:?}: the refusal names that record's resource type, got {:?}",
            row.case,
            answer.get("resourceType")
        );
        assert_eq!(
            stored(&engine, row.start),
            held,
            "case {:?}: a refused start writes nothing, store holds {} where it held {held}",
            row.case,
            stored(&engine, row.start)
        );
    }
}

#[tokio::test]
async fn without_a_client_token_every_call_mints_its_own_record_for_both_start_operations() {
    for row in rows() {
        let (emulator, engine, mut body) = prepared(&row).await;
        body.as_object_mut()
            .expect("a request body that is a JSON object")
            .remove("ClientToken");

        let first = call(&emulator, row.region, "PUT", row.start.uri(), Some(body.clone())).await;
        let second = call(&emulator, row.region, "PUT", row.start.uri(), Some(body)).await;
        assert_eq!(
            (first.status, second.status),
            (200, 200),
            "case {:?}: both untokened {} calls must be accepted, got {:?} and {:?}",
            row.case,
            row.start.operation(),
            first.body,
            second.body
        );
        assert_ne!(
            second.string(row.start.identifier_member()),
            first.string(row.start.identifier_member()),
            "case {:?}: with no token the second call mints its own record, got {:?}",
            row.case,
            second.body
        );
        assert_eq!(
            stored(&engine, row.start),
            2,
            "case {:?}: the store holds both records, got {}",
            row.case,
            stored(&engine, row.start)
        );
    }
}

/// Every step of the shared replay rule, asserted on the decision itself over the records
/// one store holds: a live token with the stored body, a live token with another body, a
/// token nothing holds, no token at all, and a token the clock has carried past its
/// window.
///
/// Generic over the record type, so the ONE rule is exercised at BOTH of its
/// instantiations through the same assertions rather than through a copy per record.
fn assert_every_step<T: TokenedRecord>(
    case: &str,
    records: &HashMap<String, T>,
    token: &str,
    fingerprint: &str,
    identifier: &str,
    operation: &str,
    now: SystemTime,
) {
    let answered = |token: Option<&str>, fingerprint: &str, at: SystemTime| {
        match state::client_token_replay(records, token, fingerprint, at, operation) {
            Ok(found) => Ok(found.map(|record| record.record_identifier().to_string())),
            Err(conflict) => Err(conflict),
        }
    };

    assert_eq!(
        answered(Some(token), fingerprint, now).map_err(|conflict| conflict.message),
        Ok(Some(identifier.to_string())),
        "case {case:?}: the same body under the live token answers with the stored record"
    );

    let conflict = answered(Some(token), "a-body-this-record-was-not-created-from", now)
        .err()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a differing body under the live token must be a conflict")
        });
    assert_eq!(
        conflict.message,
        format!("ClientToken '{token}' was already used for a different {operation} request"),
        "case {case:?}: the conflict names the token and the operation, got {:?}",
        conflict.message
    );
    assert_eq!(
        (
            conflict.resource_id.as_str(),
            conflict.resource_type.as_str()
        ),
        (identifier, T::RESOURCE_TYPE),
        "case {case:?}: the conflict names the held record and its resource type, got {:?}",
        (&conflict.resource_id, &conflict.resource_type)
    );

    for (step, carried) in [
        ("a token the store holds nothing live for", Some("no-such-token")),
        ("no token at all", None),
    ] {
        assert_eq!(
            answered(carried, fingerprint, now).map_err(|conflict| conflict.message),
            Ok(None),
            "case {case:?}: {step} is a new request"
        );
    }

    // The window is open up to but not including the instant it closes, so at exactly the
    // published span after the request that used the token the rule falls through too.
    let past_the_window = now + state::CLIENT_TOKEN_VALIDITY;
    assert_eq!(
        answered(Some(token), fingerprint, past_the_window).map_err(|conflict| conflict.message),
        Ok(None),
        "case {case:?}: past the published window the token names no earlier record"
    );
}

#[tokio::test]
async fn the_shared_replay_rule_answers_every_step_at_both_record_types() {
    let region = "us-west-2";
    let (emulator, engine) = emulator_with_engine();
    let (search_job, _) =
        settled_search_job(&emulator, &engine, region, "both record types", Settled::Completed)
            .await;

    let search_token = "search-job-rule-token";
    let started = call(
        &emulator,
        region,
        "PUT",
        "/search-jobs",
        Some(json!({
            "ClientToken": search_token,
            "Name": "under-the-rule",
            "SearchScope": {"BackupResourceTypes": ["S3"]}
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartSearchJob must be accepted, got {} ({:?})",
        started.status, started.body
    );
    let search_identifier = started.string("SearchJobIdentifier");

    let export_token = "export-job-rule-token";
    let exported = call(
        &emulator,
        region,
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "ClientToken": export_token,
            "SearchJobIdentifier": search_job,
            "ExportSpecification": {
                "s3ExportSpecification": {"DestinationBucket": "rule-exports"}
            }
        })),
    )
    .await;
    assert_eq!(
        exported.status, 200,
        "StartSearchResultExportJob must be accepted, got {} ({:?})",
        exported.status, exported.body
    );
    let export_identifier = exported.string("ExportJobIdentifier");

    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let store = store.lock();
    let now = SystemTime::now();

    let search_fingerprint = store
        .search_jobs
        .get(&search_identifier)
        .map(|job| job.request_fingerprint.clone())
        .expect("the store holds the search job the call answered with");
    assert_every_step(
        "a search job",
        &store.search_jobs,
        search_token,
        &search_fingerprint,
        &search_identifier,
        "StartSearchJob",
        now,
    );

    let export_fingerprint = store
        .export_jobs
        .get(&export_identifier)
        .map(|job| job.request_fingerprint.clone())
        .expect("the store holds the export job the call answered with");
    assert_every_step(
        "an export job",
        &store.export_jobs,
        export_token,
        &export_fingerprint,
        &export_identifier,
        "StartSearchResultExportJob",
        now,
    );
}
