// SPDX-License-Identifier: CC0-1.0

//! What dropping a data store MEANS, over both paths that drop one.
//!
//! The rule under test: when this service drops a data store record it drops
//! everything the state holds on that data store's behalf — the registry entry,
//! the idempotency tokens that named it, its export jobs and its import jobs —
//! and nothing belonging to another data store, whichever selection picked the
//! record out: the retention that ran out on a terminal record (reached from
//! every operation that settles the lifecycle, `ListFHIRDatastores` here) or the
//! sweep of terminal records a full store takes before refusing a create
//! (reached from `CreateFHIRDatastore`).
//!
//! The two selections are the rows below, so a per-data-store collection the
//! state gains later cannot be dropped on one path and left behind on the
//! other.
//!
//! `StartFHIRExportJob` mints an export job only for a data store that is
//! ACTIVE and `StartFHIRImportJob` reads an input location out of the emulator's
//! Amazon S3, so the job records
//! are seeded into the SAME store the handlers read, through the shared engine
//! the harness exposes — exactly how `tests/all/fhir_export_job_listing.rs` and
//! `tests/all/fhir_import_job_listing.rs` seed theirs. Every call that TRIGGERS
//! a reap, and every read-back, goes over the real awsJson1_0 wire through
//! `MultiServiceRouter::dispatch`.
//!
//! Every modelled CLIENT error below is asserted at HTTP 400, whatever exact
//! code its shape's `@httpError` trait names: awsJson1_0 binds nothing to the
//! transport, so this service reports an error's FAULT. The rule itself is
//! pinned in `tests/all/error_envelopes.rs`.

use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::ops::types::{
    InputDataConfig, OutputDataConfig, S3Configuration,
};
use service_com_amazonaws_healthlake_healthlake::state::{
    ComAmazonawsHealthlakeHealthlakeState, DatastoreStatus, ExportJob, ImportJob, JobStatus,
    DELETED_RETENTION, MAX_DATASTORES,
};

use crate::fhir_datastore_lifecycle::{error_code, HealthLake};

/// The epoch second a seeded job's submit instant is measured from. A fixed
/// instant, so a seeded job carries a submit time rather than one near the test
/// clock.
const BASE_SECS: i64 = 1_700_000_000;

/// The `DataAccessRoleArn` a seeded job carries, satisfying the modelled
/// `IamRoleArn` pattern.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeReaping";

/// One data store together with everything the state holds on its behalf.
struct Owner {
    id: String,
    /// The `ClientToken` the create carried, recorded against the minted data
    /// store.
    token: &'static str,
    export_job: String,
    import_job: String,
}

/// Create a data store carrying `token` and seed it one export job and one
/// import job, so every per-data-store collection the state owns holds
/// something for it.
async fn owner(service: &HealthLake, name: &str, token: &'static str) -> Owner {
    let (id, _) = service
        .create(&json!({
            "DatastoreTypeVersion": "R4",
            "DatastoreName": name,
            "ClientToken": token,
        }))
        .await;
    let export_job = format!("{token}-export");
    let import_job = format!("{token}-import");
    {
        let store = service
            .engine
            .state::<ComAmazonawsHealthlakeHealthlakeState>();
        let mut store = store.lock();
        store.export_jobs.insert(
            export_job.clone(),
            ExportJob {
                id: export_job.clone(),
                datastore_id: id.clone(),
                name: None,
                status: JobStatus::Completed,
                pending: None,
                submitted_at: ::aws_smithy_types::DateTime::from_secs(BASE_SECS),
                ended_at: None,
                output_data_config: OutputDataConfig::S3Configuration(S3Configuration {
                    s3_uri: format!("s3://healthlake-export-bucket/{export_job}"),
                    kms_key_id: "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(),
                }),
                data_access_role_arn: Some(DATA_ACCESS_ROLE_ARN.to_string()),
                message: None,
                client_token: None,
                request_fingerprint: String::new(),
            },
        );
        store.import_jobs.insert(
            import_job.clone(),
            ImportJob {
                id: import_job.clone(),
                datastore_id: id.clone(),
                name: None,
                status: JobStatus::Completed,
                pending: None,
                submitted_at: ::aws_smithy_types::DateTime::from_secs(BASE_SECS),
                ended_at: None,
                input_data_config: InputDataConfig::S3Uri(format!(
                    "s3://healthlake-import-bucket/{import_job}"
                )),
                output_data_config: None,
                data_access_role_arn: Some(DATA_ACCESS_ROLE_ARN.to_string()),
                message: None,
                progress_report: None,
                validation_level: None,
                // A record this suite seeds was not started through the API, so
                // it carries no `ClientToken` and there is no request to
                // fingerprint.
                client_token: None,
                request_fingerprint: String::new(),
            },
        );
    }
    Owner {
        id,
        token,
        export_job,
        import_job,
    }
}

/// What the state still holds for one data store: its registry entry, the
/// idempotency tokens naming it, its export jobs and its import jobs — the
/// whole removal set a drop is responsible for.
#[derive(Debug, PartialEq, Eq)]
struct Held {
    datastore: bool,
    client_tokens: usize,
    export_jobs: usize,
    import_jobs: usize,
}

impl Held {
    /// Everything a seeded [`Owner`] holds while its data store is live.
    const SEEDED: Self = Self {
        datastore: true,
        client_tokens: 1,
        export_jobs: 1,
        import_jobs: 1,
    };

    /// What a dropped data store leaves behind: nothing.
    const DROPPED: Self = Self {
        datastore: false,
        client_tokens: 0,
        export_jobs: 0,
        import_jobs: 0,
    };
}

/// How many data store records the state holds — what [`MAX_DATASTORES`]
/// bounds.
fn datastores_held(service: &HealthLake) -> usize {
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let store = store.lock();
    store.datastores.len()
}

fn held_for(service: &HealthLake, datastore_id: &str) -> Held {
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let store = store.lock();
    Held {
        datastore: store.datastores.contains_key(datastore_id),
        client_tokens: store
            .client_tokens
            .values()
            .filter(|record| record.datastore_id == datastore_id)
            .count(),
        export_jobs: store
            .export_jobs
            .values()
            .filter(|job| job.datastore_id == datastore_id)
            .count(),
        import_jobs: store
            .import_jobs
            .values()
            .filter(|job| job.datastore_id == datastore_id)
            .count(),
    }
}

/// Which selection picks the record out — one row per path that drops a data
/// store.
#[derive(Clone, Copy, Debug)]
enum Reaper {
    /// A terminal record whose retention has run out, swept by the lifecycle
    /// settling every operation does.
    RetentionExpired,
    /// A terminal record swept because the store is full and a create is about
    /// to be refused.
    StoreFull,
}

/// Put `target` at its terminal status, with the retention this row needs, and
/// make the call that reaches this row's reaper.
async fn trigger(service: &HealthLake, reaper: Reaper, target: &str) {
    let case = format!("{reaper:?}");
    {
        let store = service
            .engine
            .state::<ComAmazonawsHealthlakeHealthlakeState>();
        let mut store = store.lock();
        let record = store
            .datastores
            .get_mut(target)
            .unwrap_or_else(|| panic!("case {case:?}: the created data store is held"));
        record.status = DatastoreStatus::Deleted;
        record.pending = None;
        record.reap_at = Some(match reaper {
            // The end of its own retention: what a delete leaves behind once
            // the retention has run out.
            Reaper::RetentionExpired => Instant::now(),
            // Retention still running, so only the full-store sweep can pick
            // this record out.
            Reaper::StoreFull => Instant::now() + DELETED_RETENTION,
        });
    }

    match reaper {
        Reaper::RetentionExpired => {
            let listed = service.list_all(&Value::Null).await;
            assert!(
                !listed
                    .iter()
                    .any(|datastore| datastore["DatastoreId"] == json!(target)),
                "case {case:?}: a reaped data store must not be listed, got {listed:?}"
            );
        }
        Reaper::StoreFull => {
            for index in datastores_held(service)..MAX_DATASTORES {
                service
                    .create(&json!({
                        "DatastoreTypeVersion": "R4",
                        "DatastoreName": format!("filler-{index}"),
                    }))
                    .await;
            }
            // The store is full, so this create sweeps the terminal record
            // before deciding, and succeeds in the slot it frees.
            let full = datastores_held(service);
            assert_eq!(
                full, MAX_DATASTORES,
                "case {case:?}: the store must be full before the create, {full} held"
            );
            let (status, payload) = service
                .call(
                    "CreateFHIRDatastore",
                    &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "reclaiming" }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a create must reclaim a terminal record's slot, got {status}: {payload}"
            );
        }
    }
}

#[tokio::test]
async fn every_path_that_drops_a_data_store_drops_everything_it_owns() {
    for reaper in [Reaper::RetentionExpired, Reaper::StoreFull] {
        let case = format!("{reaper:?}");
        let service = HealthLake::new();
        let target = owner(&service, "reaped store", "token-target").await;
        let bystander = owner(&service, "kept store", "token-bystander").await;

        for owned in [&target, &bystander] {
            let held = held_for(&service, &owned.id);
            assert_eq!(
                held,
                Held::SEEDED,
                "case {case:?}: data store {:?} must hold its token, its export job and its \
                 import job before anything is reaped, got {held:?}",
                owned.token
            );
        }

        trigger(&service, reaper, &target.id).await;

        let dropped = held_for(&service, &target.id);
        assert_eq!(
            dropped,
            Held::DROPPED,
            "case {case:?}: dropping a data store must drop its registry entry, the tokens that \
             named it, its export jobs and its import jobs together, got {dropped:?}"
        );
        let kept = held_for(&service, &bystander.id);
        assert_eq!(
            kept,
            Held::SEEDED,
            "case {case:?}: another data store's records must survive the reap, got {kept:?}"
        );

        // Over the wire: the reaped data store reads as gone (a DELETED record
        // still HELD describes), and nothing of it is reachable.
        let (status, payload) = service
            .call(
                "DescribeFHIRDatastore",
                &json!({ "DatastoreId": &target.id }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a reaped data store must be refused, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: a reaped data store must answer ResourceNotFoundException, got {payload}"
        );
        for operation in ["ListFHIRExportJobs", "ListFHIRImportJobs"] {
            let (status, payload) = service
                .call(operation, &json!({ "DatastoreId": &target.id }))
                .await;
            assert_eq!(
                error_code(&payload),
                "ResourceNotFoundException",
                "case {case:?}: {operation} for a reaped data store must answer \
                 ResourceNotFoundException at 400, got {status}: {payload}"
            );
        }

        // And the data store that was NOT selected still serves everything it
        // holds.
        let described = service.describe(&bystander.id).await;
        assert_eq!(
            described["DatastoreId"],
            json!(bystander.id),
            "case {case:?}: the data store that was not selected must still describe, got {described}"
        );
        for (operation, member, expected) in [
            (
                "ListFHIRExportJobs",
                "ExportJobPropertiesList",
                &bystander.export_job,
            ),
            (
                "ListFHIRImportJobs",
                "ImportJobPropertiesList",
                &bystander.import_job,
            ),
        ] {
            let payload = service
                .ok(operation, &json!({ "DatastoreId": &bystander.id }))
                .await;
            let listed: Vec<String> = payload[member]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: {member} must be a list, got {payload}"))
                .iter()
                .map(|job| job["JobId"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                listed,
                vec![expected.clone()],
                "case {case:?}: {operation} must still report the job of the data store that was \
                 not selected, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_dropped_data_stores_token_mints_a_new_data_store() {
    // The tokens a drop forgets are the ones that NAMED the dropped record, so
    // a caller reusing one gets a fresh data store rather than a replay of a
    // record the store no longer holds.
    let service = HealthLake::new();
    let target = owner(&service, "reaped store", "token-target").await;

    trigger(&service, Reaper::RetentionExpired, &target.id).await;

    let (minted, _) = service
        .create(&json!({
            "DatastoreTypeVersion": "R4",
            "DatastoreName": "reaped store",
            "ClientToken": target.token,
        }))
        .await;
    assert_ne!(
        minted, target.id,
        "a token whose data store was reaped must mint a new one, got {minted:?}"
    );
    let held = held_for(&service, &minted);
    assert_eq!(
        held,
        Held {
            datastore: true,
            client_tokens: 1,
            export_jobs: 0,
            import_jobs: 0,
        },
        "the new data store must hold its own token and none of the reaped store's jobs, got \
         {held:?}"
    );
}
