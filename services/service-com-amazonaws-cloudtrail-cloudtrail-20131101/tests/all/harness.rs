// SPDX-License-Identifier: CC0-1.0

//! Import-registry fixtures: the `ImportRecord` / `ImportFailureRecord` values the
//! `import_failures` suite seeds into the shared store, kept apart from the
//! assertions that read them back over the wire.
//!
//! The emulator-under-test harness itself is `tests/all/emulator.rs`; this module
//! only builds the state a listing is then asserted against, through that harness's
//! `state()` — `state::service_state`, the same seam the handlers read the registry
//! through — so a listing sees exactly what a future `StartImport` (still a scaffold,
//! its own stub issue) would leave.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101 as cloudtrail;

use super::emulator::Emulator;

/// A 36-character import id matching the model's `UUID` shape (len 36..36,
/// `^[a-f0-9\-]+$`), distinct per `index`.
pub fn import_id(index: usize) -> String {
    let id = format!("{index:08x}-0000-4000-8000-{index:012x}");
    assert_eq!(id.len(), 36, "an import id is 36 characters: {id:?}");
    id
}

/// The instant the `index`-th seeded failure was last updated.
pub fn failure_time(index: usize) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_760_000_000 + index as u64)
}

/// The `index`-th failure of a seeded import.
///
/// `Status` cycles through every value the model's `ImportFailureStatus` enum
/// accepts, taken from the catalog this crate generates from the model
/// (`ops::types::IMPORT_FAILURE_STATUS_VALUES` — `FAILED`, `RETRY`, `SUCCEEDED`).
pub fn seeded_failure(index: usize) -> cloudtrail::state::ImportFailureRecord {
    let statuses = cloudtrail::ops::types::IMPORT_FAILURE_STATUS_VALUES;
    cloudtrail::state::ImportFailureRecord {
        location: format!("s3://import-bucket/AWSLogs/000000000000/{index:04}.json.gz"),
        status: statuses[index % statuses.len()].to_string(),
        error_type: "InvalidLogFile".to_string(),
        error_message: format!("The log file at position {index} could not be read"),
        last_updated_time: failure_time(index),
    }
}

/// Seed an import of `region` that recorded `count` failures, and hand back its id.
///
/// It writes the import `StartImport` (still a scaffold) would have left AND the
/// failures it recorded, through the harness's `state()` — `state::service_state`,
/// the same seam the handlers read the registry through.
pub fn seed_import(emulator: &Emulator, region: &str, index: usize, count: usize) -> String {
    let id = import_id(index);
    let created = UNIX_EPOCH + Duration::from_secs(1_759_000_000);
    let record = cloudtrail::state::ImportRecord {
        import_id: id.clone(),
        destinations: vec![format!(
            "arn:aws:cloudtrail:{region}:000000000000:eventdatastore/{id}"
        )],
        import_source: None,
        start_event_time: None,
        end_event_time: None,
        status: cloudtrail::state::IMPORT_STATUS_IN_PROGRESS.to_string(),
        created_timestamp: created,
        updated_timestamp: created,
        // The `INITIALIZING` -> `IN_PROGRESS` move is due at this instant, which is
        // immaterial here: the record is seeded already `IN_PROGRESS`, and
        // `ListImportFailures` reports an import's failures whatever status it carries.
        ingests_at: created,
        statistics: None,
    };
    // `ImportKey` is `(import id, Region)`: an import, and the failures it recorded,
    // are reachable only from the Region it was created in.
    let key = (id.clone(), region.to_string());
    let store = emulator.state();
    let mut state = store.lock();
    state.imports.insert(key.clone(), record);
    state
        .import_failures
        .insert(key, (0..count).map(seeded_failure).collect());
    id
}
