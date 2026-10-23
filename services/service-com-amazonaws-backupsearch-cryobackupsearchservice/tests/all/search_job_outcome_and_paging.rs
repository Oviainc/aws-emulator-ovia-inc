// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the four rules the search-job plane answers a client with,
//! driven over HTTP exactly as a client drives it. Each test states ONE rule and asserts
//! it over a TABLE whose rows differ in what the rule generalises over, never over the
//! one request a recording happened to make:
//!
//! * the terminal status the clock owes a scan follows from the RESOLVED SCOPE — a scope
//!   that resolved to no recovery point is a terminal `FAILED` the service explains in
//!   `StatusMessage`, and a scope that resolved to some runs to `COMPLETED` with no
//!   message (rows differ in resource type, in which scope filter excludes or admits the
//!   record, and at each bound of the creation window);
//! * `ListSearchJobs` reports its jobs newest-created first and issues a `NextToken`
//!   whenever the page it just served was full — that invariant is asserted over job
//!   counts x page sizes by `paging_returns_every_job_exactly_once_whatever_the_page_size`
//!   in `search_jobs.rs`, which is the one paging harness;
//! * `StopSearchJob` is `@idempotent`: a status that already is, or is on its way to,
//!   the end state it asks for answers the empty 200, and only a terminal status this
//!   operation does not produce conflicts (rows: every value of the model's
//!   `SearchJobState`);
//! * an identifier this service mints is a UUID, and the ARN it is embedded in names it
//!   (rows differ in the start body, and cover both minted identifiers).
//!
//! The HTTP harness (`emulator_with_engine`, `call`, `Answer`) and the seeding helpers
//! are the search-job suite's: every suite drives the same emulator the same way.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator_with_engine, recovery_point, seed_recovery_points, PAST_THE_DUE_INSTANT,
};

/// The region every call in this suite is signed for.
const REGION: &str = "us-west-2";

/// The instant the seeded recovery points were backed up at, as whole seconds since the
/// epoch, so a creation-window filter can be written around it exactly.
const BACKED_UP_AT_SECS: u64 = 1_760_000_000;

/// The instant [`BACKED_UP_AT_SECS`] names.
fn backed_up_at() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(BACKED_UP_AT_SECS)
}

/// One row of the resolved-scope table: the recovery points this account holds, the
/// scope a job is started with, and how many of those points that scope selects.
struct ScopeCase {
    case: &'static str,
    held: Vec<types::SearchJobBackupsResult>,
    scope: Value,
    in_scope: usize,
}

/// A recovery point of `resource_type` backed up at [`backed_up_at`].
fn held(resource_type: &str, backup_arn: &str, source_arn: &str) -> types::SearchJobBackupsResult {
    recovery_point(resource_type, backup_arn, source_arn, backed_up_at())
}

fn scope_cases() -> Vec<ScopeCase> {
    // `ResourceType` accepts exactly S3 and EBS (the model's `ResourceType` enum, as
    // `types::RESOURCE_TYPE_VALUES`), and `SearchScope.BackupResourceTypes` is
    // @length(min: 1, max: 1), so a scope names ONE of them.
    let s3 = held(
        "S3",
        "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-s3",
        "arn:aws:s3:::alpha",
    );
    let ebs = held(
        "EBS",
        "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-ebs",
        "arn:aws:ec2:us-west-2:000000000000:volume/vol-1",
    );
    vec![
        ScopeCase {
            case: "nothing held at all, S3 scope",
            held: Vec::new(),
            scope: json!({"BackupResourceTypes": ["S3"]}),
            in_scope: 0,
        },
        ScopeCase {
            case: "nothing held at all, EBS scope",
            held: Vec::new(),
            scope: json!({"BackupResourceTypes": ["EBS"]}),
            in_scope: 0,
        },
        ScopeCase {
            case: "one S3 recovery point, S3 scope",
            held: vec![s3.clone()],
            scope: json!({"BackupResourceTypes": ["S3"]}),
            in_scope: 1,
        },
        ScopeCase {
            case: "one EBS recovery point, EBS scope",
            held: vec![ebs.clone()],
            scope: json!({"BackupResourceTypes": ["EBS"]}),
            in_scope: 1,
        },
        ScopeCase {
            case: "both held, S3 scope keeps only the S3 one",
            held: vec![s3.clone(), ebs.clone()],
            scope: json!({"BackupResourceTypes": ["S3"]}),
            in_scope: 1,
        },
        ScopeCase {
            case: "only an EBS recovery point held, S3 scope resolves to nothing",
            held: vec![ebs.clone()],
            scope: json!({"BackupResourceTypes": ["S3"]}),
            in_scope: 0,
        },
        ScopeCase {
            case: "a BackupResourceArns filter naming the held recovery point",
            held: vec![s3.clone()],
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceArns": ["arn:aws:backup:us-west-2:000000000000:recovery-point:rp-s3"]
            }),
            in_scope: 1,
        },
        ScopeCase {
            case: "a BackupResourceArns filter naming another recovery point",
            held: vec![s3.clone()],
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceArns": ["arn:aws:backup:us-west-2:000000000000:recovery-point:rp-other"]
            }),
            in_scope: 0,
        },
        ScopeCase {
            case: "a SourceResourceArns filter naming the held source resource",
            held: vec![s3.clone(), ebs.clone()],
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "SourceResourceArns": ["arn:aws:s3:::alpha", "arn:aws:s3:::beta"]
            }),
            in_scope: 1,
        },
        ScopeCase {
            case: "a SourceResourceArns filter naming another source resource",
            held: vec![s3.clone()],
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "SourceResourceArns": ["arn:aws:s3:::beta"]
            }),
            in_scope: 0,
        },
        ScopeCase {
            case: "a creation window whose CreatedAfter bound is the backup's own instant",
            held: vec![s3.clone()],
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {"CreatedAfter": BACKED_UP_AT_SECS}
            }),
            in_scope: 1,
        },
        ScopeCase {
            case: "a creation window starting one second past the backup's instant",
            held: vec![s3.clone()],
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {"CreatedAfter": BACKED_UP_AT_SECS + 1}
            }),
            in_scope: 0,
        },
        ScopeCase {
            case: "a creation window whose CreatedBefore bound is the backup's own instant",
            held: vec![s3.clone()],
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {"CreatedBefore": BACKED_UP_AT_SECS}
            }),
            in_scope: 1,
        },
        ScopeCase {
            case: "a creation window ending one second before the backup's instant",
            held: vec![s3.clone()],
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {"CreatedBefore": BACKED_UP_AT_SECS - 1}
            }),
            in_scope: 0,
        },
        ScopeCase {
            case: "a window spanning the backup's instant from both sides",
            held: vec![s3.clone(), ebs.clone()],
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {
                    "CreatedAfter": BACKED_UP_AT_SECS - 3600,
                    "CreatedBefore": BACKED_UP_AT_SECS + 3600
                }
            }),
            in_scope: 1,
        },
        ScopeCase {
            case: "a tag filter, which no recovery point record can be shown to carry",
            held: vec![s3],
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceTags": {"team": "a"}
            }),
            in_scope: 0,
        },
    ]
}

#[tokio::test]
async fn the_terminal_status_a_scan_settles_to_follows_the_resolved_scope() {
    // One emulator per row, so each row's store holds exactly the recovery points that
    // row seeds. Every job is started BEFORE the single sleep below, so the clock makes
    // every row's transition due at once instead of once per row.
    let mut rows: Vec<(ScopeCase, MultiServiceRouter, String)> = Vec::new();
    for case in scope_cases() {
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, case.held.clone());
        let started = call(
            &emulator,
            REGION,
            "PUT",
            "/search-jobs",
            Some(json!({"Name": "scoped", "SearchScope": case.scope.clone()})),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {:?}: StartSearchJob must accept the scope, got {} ({:?})",
            case.case, started.status, started.body
        );
        let identifier = started.string("SearchJobIdentifier");
        rows.push((case, emulator, identifier));
    }

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;

    for (case, emulator, identifier) in rows {
        let ScopeCase {
            case: name,
            in_scope,
            ..
        } = &case;
        // The rule: nothing in scope is a terminal failure that says so; anything in
        // scope runs to completion and explains nothing.
        let (expected_status, expected_message) = if *in_scope == 0 {
            (state::FAILED, Value::String(state::SCOPE_EMPTY.to_string()))
        } else {
            (state::COMPLETED, Value::Null)
        };

        let fetched = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            fetched.status, 200,
            "case {name:?}: expected 200 from GetSearchJob, got {} ({:?})",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.string("Status"),
            expected_status,
            "case {name:?}: {in_scope} recovery point(s) in scope must settle to {expected_status}, got {:?}",
            fetched.get("Status")
        );
        assert_eq!(
            fetched.get("StatusMessage"),
            &expected_message,
            "case {name:?}: the StatusMessage must be {expected_message:?}, got {:?}",
            fetched.get("StatusMessage")
        );
        assert_eq!(
            fetched.get("SearchScopeSummary")["TotalRecoveryPointsToScanCount"].as_u64(),
            Some(*in_scope as u64),
            "case {name:?}: the scan's scope summary must count the resolved scope, got {:?}",
            fetched.get("SearchScopeSummary")
        );

        // The listing reports the same record, so a client that polls either one sees
        // one answer.
        let listed = call(&emulator, REGION, "GET", "/search-jobs", None).await;
        let summary = listed.get("SearchJobs")[0].clone();
        assert_eq!(
            summary["Status"].as_str(),
            Some(expected_status),
            "case {name:?}: ListSearchJobs must report the same Status, got {summary:?}"
        );
        assert_eq!(
            summary["StatusMessage"], expected_message,
            "case {name:?}: ListSearchJobs must report the same StatusMessage, got {summary:?}"
        );

        // And the recovery points the scan covered are exactly the resolved scope.
        let backups = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}/backups"),
            None,
        )
        .await;
        assert_eq!(
            backups.get("Results").as_array().map(Vec::len),
            Some(*in_scope),
            "case {name:?}: ListSearchJobBackups must report the resolved scope, got {:?}",
            backups.body
        );
    }
}

#[tokio::test]
async fn stopping_a_job_is_idempotent_and_only_an_unreachable_end_state_conflicts() {
    // One row per value of the model's `SearchJobState` (`types::SEARCH_JOB_STATE_VALUES`):
    // the status the job is seeded in, what a StopSearchJob against it must answer, and
    // the status the job holds afterwards. A status that already is the end state this
    // `@idempotent` operation asks for, or is on its way to it, is a no-op success; only
    // a terminal status this operation does not produce is a conflict.
    let cases: Vec<(&str, u16, &str)> = vec![
        (state::RUNNING, 200, state::STOPPING),
        (state::STOPPING, 200, state::STOPPING),
        (state::STOPPED, 200, state::STOPPED),
        (state::COMPLETED, 409, state::COMPLETED),
        (state::FAILED, 409, state::FAILED),
    ];
    assert_eq!(
        cases.len(),
        types::SEARCH_JOB_STATE_VALUES.len(),
        "every status the model declares must have a row, got {:?}",
        types::SEARCH_JOB_STATE_VALUES
    );

    for (seeded, expected_status, expected_after) in cases {
        assert!(
            types::SEARCH_JOB_STATE_VALUES.contains(&seeded),
            "case {seeded:?}: the seeded status must be one the model declares"
        );
        let (emulator, engine) = emulator_with_engine();
        let identifier = seed_job(&engine, seeded);
        let uri = format!("/search-jobs/{identifier}/actions/cancel");

        let stopped = call(&emulator, REGION, "PUT", &uri, None).await;
        assert_eq!(
            stopped.status, expected_status,
            "case {seeded:?}: StopSearchJob must answer {expected_status}, got {} ({:?})",
            stopped.status, stopped.body
        );
        if expected_status == 200 {
            assert_eq!(
                stopped.body,
                json!({}),
                "case {seeded:?}: the modelled output is empty, got {:?}",
                stopped.body
            );
        } else {
            assert_eq!(
                stopped.error_code.as_deref(),
                Some("ConflictException"),
                "case {seeded:?}: the refusal must carry ConflictException, got {:?}",
                stopped.error_code
            );
        }

        let fetched = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            fetched.string("Status"),
            expected_after,
            "case {seeded:?}: the job must be {expected_after} afterwards, got {:?}",
            fetched.get("Status")
        );

        // A second call answers exactly as the first did: the operation is @idempotent,
        // so a retry is never a new outcome.
        let again = call(&emulator, REGION, "PUT", &uri, None).await;
        assert_eq!(
            again.status, stopped.status,
            "case {seeded:?}: a retry must answer as the first call did, got {} ({:?})",
            again.status, again.body
        );
    }
}

/// Seed one search job in `status` straight into the store, which is how a row reaches a
/// status no sequence of calls can hold still at (`STOPPING` is transient, and
/// `COMPLETED` needs a recovery point in scope). The job owes no transition, so the row
/// is deterministic and sleep-free.
fn seed_job(engine: &engine::InMemoryEngine, status: &str) -> String {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let mut store = store.lock();
    let identifier = store
        .mint_identifier()
        .expect("the store mints a fresh identifier");
    store.insert_job(state::SearchJob {
        identifier: identifier.clone(),
        arn: state::search_job_arn("backup-search", REGION, &identifier),
        name: None,
        status: status.to_string(),
        creation_time: state::timestamp(SystemTime::now()),
        completion_time: None,
        status_message: None,
        encryption_key_arn: None,
        search_scope: types::SearchScope::default(),
        item_filters: types::ItemFilters::default(),
        search_scope_summary: types::SearchScopeSummary::default(),
        current_search_progress: types::CurrentSearchProgress::default(),
        results: Vec::new(),
        backups: Vec::new(),
        tags: std::collections::HashMap::new(),
        client_token: None,
        request_fingerprint: identifier.clone(),
        next_status_due: None,
    });
    identifier
}

#[tokio::test]
async fn every_identifier_this_service_mints_is_a_uuid_the_arn_names() {
    // Rows differ in the start body — the members a caller varies cannot reach the
    // minted id, so none of them may change its shape.
    let bodies = vec![
        ("plain S3 scope", json!({"SearchScope": {"BackupResourceTypes": ["S3"]}})),
        (
            "named, tagged EBS scope",
            json!({
                "Name": "minted",
                "SearchScope": {"BackupResourceTypes": ["EBS"]},
                "Tags": {"env": "test"}
            }),
        ),
        (
            "a scope with filters and a client token",
            json!({
                "ClientToken": "token-1",
                "SearchScope": {
                    "BackupResourceTypes": ["S3"],
                    "SourceResourceArns": ["arn:aws:s3:::alpha"]
                }
            }),
        ),
    ];

    let (emulator, engine) = emulator_with_engine();
    seed_recovery_points(
        &engine,
        vec![
            held(
                "S3",
                "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-s3",
                "arn:aws:s3:::alpha",
            ),
            held(
                "EBS",
                "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-ebs",
                "arn:aws:ec2:us-west-2:000000000000:volume/vol-1",
            ),
        ],
    );

    let mut minted: Vec<String> = Vec::new();
    let mut last_search_job = String::new();
    for (case, body) in bodies {
        let started = call(&emulator, REGION, "PUT", "/search-jobs", Some(body)).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartSearchJob must succeed, got {} ({:?})",
            started.status, started.body
        );
        let identifier = started.string("SearchJobIdentifier");
        assert_uuid(case, "SearchJobIdentifier", &identifier);
        assert_eq!(
            started.string("SearchJobArn"),
            format!(
                "arn:aws:backup-search:{REGION}:{}:search-job/{identifier}",
                engine::emulator_account_id()
            ),
            "case {case:?}: the ARN's resource segment must be the minted id, got {:?}",
            started.get("SearchJobArn")
        );
        minted.push(identifier.clone());
        last_search_job = identifier;
    }

    // The same rule holds for the other identifier this service mints: an export job's.
    // Its search job has a recovery point in scope, so it reaches an exportable status.
    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let exported = call(
        &emulator,
        REGION,
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "SearchJobIdentifier": last_search_job,
            "ExportSpecification": {"s3ExportSpecification": {"DestinationBucket": "exports"}}
        })),
    )
    .await;
    assert_eq!(
        exported.status, 200,
        "StartSearchResultExportJob must succeed for a settled search job, got {} ({:?})",
        exported.status, exported.body
    );
    let export_identifier = exported.string("ExportJobIdentifier");
    assert_uuid("an export job", "ExportJobIdentifier", &export_identifier);
    assert!(
        exported
            .string("ExportJobArn")
            .ends_with(&format!("export-job/{export_identifier}")),
        "the export ARN's resource segment must be the minted id, got {:?}",
        exported.get("ExportJobArn")
    );
    minted.push(export_identifier);

    let unique: std::collections::BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        unique.len(),
        minted.len(),
        "every minted identifier must be distinct, got {minted:?}"
    );
}

/// Pin the SHAPE of a minted identifier: the canonical lowercase hyphenated rendering of
/// a version-4 UUID, which is what this service's recordings show it mints. The fields
/// and the two fixed nibbles are RFC 4122 §4.4's: `8-4-4-4-12` lowercase hex, version
/// nibble `4`, and a variant nibble in `8`/`9`/`a`/`b`.
fn assert_uuid(case: &str, member: &str, value: &str) {
    let fields: Vec<&str> = value.split('-').collect();
    assert_eq!(
        fields.iter().map(|field| field.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: {member} must be a UUID's 8-4-4-4-12 fields, got {value:?}"
    );
    assert!(
        fields
            .iter()
            .all(|field| field.chars().all(|digit| digit.is_ascii_hexdigit()
                && !digit.is_ascii_uppercase())),
        "case {case:?}: {member} must be lowercase hex, got {value:?}"
    );
    assert_eq!(
        fields[2].chars().next(),
        Some('4'),
        "case {case:?}: {member} must carry the version-4 nibble, got {value:?}"
    );
    assert!(
        fields[3]
            .chars()
            .next()
            .is_some_and(|variant| "89ab".contains(variant)),
        "case {case:?}: {member} must carry an RFC 4122 variant nibble, got {value:?}"
    );
}
