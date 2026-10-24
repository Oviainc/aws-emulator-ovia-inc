// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of what a `CreateJob` request's `JobEngineVersion` DOES.
//!
//! The rule every case below is one row of: a requested Job engine version is resolved
//! against the account's Job engine version catalogue — the one `ListVersions` reports —
//! and `Job$JobEngineVersionUsed` states each outcome
//! (`smithy_query(query="doc",shape="Job",member="JobEngineVersionUsed",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`):
//! "When you request an expired version, the response for this property will be empty.
//! Requests to create jobs with an expired version result in a regular job, as if no
//! specific Job engine version was requested. When you request an invalid version, the
//! response for this property will be empty. Requests to create jobs with an invalid
//! version result in a 400 error message, and no job is created."
//!
//! So: a label the catalogue does not hold is `BadRequestException` (400) and stores
//! nothing; a label it holds whose expiry has passed creates a job that uses no version;
//! a label it holds that is still available creates a job that uses THAT version. The
//! requested label is echoed either way, because it is what the request stated.
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness, so
//! what is asserted is the bytes a client would see. The only state reached past HTTP is
//! the seeding of the one EXPIRED catalogue entry, because an expired entry is by
//! definition not one `ListVersions` offers.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    JobEngineVersionRecord, JOB_ENGINE_VERSION_LIFETIME,
};
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

use super::job_engine_versions::{labels, list_versions};
use super::job_lifecycle::{settings as job_settings, Emulator, ROLE};

/// The label of the catalogue entry every case needs to be EXPIRED.
///
/// Seeded by [`seed_expired`]: a `YYYY-MM-DD` label (the form `JobEngineVersion$Version`
/// documents) whose entry expired before now, so `ListVersions` does not offer it while
/// the catalogue still holds it.
const EXPIRED_VERSION: &str = "2021-03-01";

/// A label in the documented `YYYY-MM-DD` form that the account's catalogue does not hold
/// at all — a release far before any entry a fresh account is seeded with.
const UNPUBLISHED_PAST_VERSION: &str = "1999-01-01";

/// As [`UNPUBLISHED_PAST_VERSION`], for a date no release has reached yet: well formed,
/// still not a version this account publishes.
const UNPUBLISHED_FUTURE_VERSION: &str = "2099-12-31";

/// Put one entry in the account's catalogue under `label`, available or already past its
/// expiry, keeping every seeded entry beside it.
///
/// Seeded rather than listed, because the two cases a CATALOGUE ENTRY comes in cannot both
/// be read out of `ListVersions`: it reports "all AVAILABLE Job engine versions", so an
/// expired entry is by definition not one it offers.
fn seed_version(emulator: &Emulator, label: &str, expired: bool) {
    let state = emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>();
    let mut state = state.lock();
    let now = SystemTime::now();
    state.job_engine_versions.insert(
        label.to_string(),
        JobEngineVersionRecord {
            version: label.to_string(),
            released_at: now - Duration::from_secs(4 * 365 * 24 * 60 * 60),
            expires_at: if expired {
                now - Duration::from_secs(60)
            } else {
                now + JOB_ENGINE_VERSION_LIFETIME
            },
        },
    );
}

/// How many jobs the store holds, read back through `ListJobs` — how every refusal row
/// checks that nothing was stored, through the API rather than the state struct.
async fn stored_jobs(emulator: &Emulator) -> usize {
    let listed = emulator.send("GET", "/2017-08-29/jobs", None).await;
    assert_eq!(
        listed.status, 200,
        "ListJobs must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    listed.at("/jobs").as_array().map_or(usize::MAX, Vec::len)
}

/// One row: the `JobEngineVersion` a `CreateJob` request carries, and what the service
/// answers for it.
struct Row {
    case: String,
    /// The member's value, or `None` for a request that carries the member not at all.
    requested: Option<String>,
    /// The HTTP status the create must answer.
    status: u16,
    /// The `jobEngineVersionUsed` the created job must report; `None` means the member
    /// must be absent.
    used: Option<String>,
}

/// A requested Job engine version decides the job's `JobEngineVersionUsed`, or is refused:
/// the catalogue is the authority, not the string.
///
/// The rows span the class the rule generalises over — no version requested at all, the
/// oldest and the newest label the account publishes (neither of them the one the issue
/// cited), a label the catalogue holds whose expiry has passed, and labels the catalogue
/// does not hold: one that is not a date, an empty one, a well-formed past date and a
/// well-formed future date.
#[tokio::test]
async fn a_requested_job_engine_version_is_resolved_against_the_accounts_catalogue() {
    let emulator = Emulator::new();
    seed_version(&emulator, EXPIRED_VERSION, true);

    // The labels this account publishes, read through `ListVersions` itself — the
    // catalogue a requested version is resolved against — so no row names a version this
    // test assumed.
    let available = labels(&list_versions(&emulator, "").await);
    assert!(
        available.len() >= 2,
        "the account's catalogue must publish at least two versions for this table, got \
         {available:?}"
    );
    assert!(
        !available.contains(&EXPIRED_VERSION.to_string()),
        "the seeded expired entry must not be an AVAILABLE version, got {available:?}"
    );
    let oldest = available.first().cloned().unwrap_or_default();
    let newest = available.last().cloned().unwrap_or_default();

    let mut rows = vec![
        Row {
            case: "no version requested at all".to_string(),
            requested: None,
            status: 201,
            used: None,
        },
        Row {
            case: format!("the newest published version {newest:?}"),
            requested: Some(newest.clone()),
            status: 201,
            used: Some(newest.clone()),
        },
        Row {
            case: format!("the oldest published version {oldest:?}"),
            requested: Some(oldest.clone()),
            status: 201,
            used: Some(oldest.clone()),
        },
        Row {
            case: "a published version that has expired".to_string(),
            requested: Some(EXPIRED_VERSION.to_string()),
            status: 201,
            used: None,
        },
    ];
    for unpublished in [
        "not-a-version",
        "",
        UNPUBLISHED_PAST_VERSION,
        UNPUBLISHED_FUTURE_VERSION,
    ] {
        rows.push(Row {
            case: format!("the unpublished version {unpublished:?}"),
            requested: Some(unpublished.to_string()),
            status: 400,
            used: None,
        });
    }

    for (index, row) in rows.iter().enumerate() {
        let case = &row.case;
        let mut request = json!({
            "role": ROLE,
            "settings": job_settings(&format!("s3://example-bucket/in/{index}.mp4"), "_v"),
        });
        if let Some(requested) = &row.requested {
            request["jobEngineVersion"] = json!(requested);
        }

        let before = stored_jobs(&emulator).await;
        let created = emulator.create_job(request).await;
        assert_eq!(
            created.status, row.status,
            "case {case:?}: expected HTTP {}, got {} with body {}",
            row.status, created.status, created.body
        );

        if row.status != 201 {
            assert_eq!(
                created.error_type.as_deref(),
                Some("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {:?}",
                created.error_type
            );
            assert_eq!(
                stored_jobs(&emulator).await,
                before,
                "case {case:?}: a refused create must store no job"
            );
            continue;
        }

        let want_requested = row
            .requested
            .as_ref()
            .map_or(Value::Null, |requested| json!(requested));
        let want_used = row.used.as_ref().map_or(Value::Null, |used| json!(used));
        let id = created.string("/job/id");
        let fetched = emulator.get_job(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetJob HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );
        // The create response and the read-back must agree: the resolution is persisted,
        // not recomputed per reader.
        for (where_from, reply) in [("create", &created), ("get", &fetched)] {
            assert_eq!(
                reply.at("/job/jobEngineVersionRequested"),
                &want_requested,
                "case {case:?} on {where_from}: expected jobEngineVersionRequested \
                 {want_requested}, got {}",
                reply.body
            );
            assert_eq!(
                reply.at("/job/jobEngineVersionUsed"),
                &want_used,
                "case {case:?} on {where_from}: expected jobEngineVersionUsed {want_used}, \
                 got {}",
                reply.body
            );
        }
    }
}

/// A version that expires stops being the version a later job uses: with nothing about the
/// request changed, the job submitted while the entry was available uses it and the job
/// submitted once it has expired is a regular job, "as if no specific Job engine version
/// was requested".
///
/// The two rows differ only in the entry's expiry — the dimension the rule turns on —
/// which is moved on the stored record rather than waited out, so the case pins the rule
/// instead of racing a real clock.
#[tokio::test]
async fn a_version_that_expires_is_no_longer_the_version_a_later_job_uses() {
    let emulator = Emulator::new();
    let label = "2025-06-01";

    for (case, expired, want_used) in [
        ("while the version is available", false, json!(label)),
        ("once the version has expired", true, Value::Null),
    ] {
        seed_version(&emulator, label, expired);
        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "jobEngineVersion": label,
                "settings": job_settings("s3://example-bucket/in/expiring.mp4", "_e"),
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: a version the catalogue holds must be accepted, got {} with \
             body {}",
            created.status, created.body
        );
        assert_eq!(
            created.at("/job/jobEngineVersionRequested"),
            &json!(label),
            "case {case:?}: the requested version is echoed either way, got {}",
            created.body
        );
        assert_eq!(
            created.at("/job/jobEngineVersionUsed"),
            &want_used,
            "case {case:?}: expected jobEngineVersionUsed {want_used}, got {}",
            created.body
        );
    }
}
