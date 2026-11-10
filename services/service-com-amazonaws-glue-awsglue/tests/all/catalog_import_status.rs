// SPDX-License-Identifier: CC0-1.0

//! The Data Catalog's MIGRATION record: `ImportCatalogToGlue` and
//! `GetCatalogImportStatus`.
//!
//! The rule under test: the migration status of a Data Catalog is the SERVICE's own
//! record, not the caller's — "`True` if the migration has completed"
//! (`CatalogImportStatus$ImportCompleted`), "the time that the migration was started"
//! (`$ImportTime`) reported for every catalog whose status is read, and "the name of the
//! person who initiated the migration" (`$ImportedBy`) reported as the service's own
//! marker, because no request this service serves carries such a person and the calling
//! identity is never put in their place. A Data Catalog this account holds is a Glue Data
//! Catalog, so its migration is complete whether an import ran against it or not; an
//! import stamps the instant the status reports; the read reports, so it never changes what
//! a later read sees; and a `CatalogId` this account does not hold is refused by both
//! operations rather than served or created.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: whether
//! the request names its `CatalogId` or lets it default, whether an import has run, how
//! many imports have run, and which catalog id a foreign request names. The recorded cases
//! — reading the status of the account's own catalog, with and without an import before it
//! — are rows among them.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness; the one
//! direct touch of the store seeds an OLDER migration record, so that a later import can be
//! shown to have replaced it without the suite sleeping for a clock tick.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_glue_awsglue::state::{
    service_state, CatalogMigrationEntry, STATUS_SET_BY_DEFAULT,
};

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID,
    CALLER_PRINCIPAL};

/// An instant well before any import this suite performs, as the seeded record's
/// `ImportTime`: a fixed point the clock is past, so a later import's instant is strictly
/// greater.
const SEEDED_IMPORT_EPOCH: Duration = Duration::from_secs(1_500_000_000);

/// The `ImportStatus` a status read reports for the catalog `request` addresses.
async fn import_status(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "GetCatalogImportStatus", request).await["ImportStatus"].clone()
}

/// Migrate the catalog `catalog` names (`None` lets the `CatalogId` default) and assert the
/// import answered 200 with an empty body.
async fn import_catalog(router: &MultiServiceRouter, case: &str, catalog: Option<&str>) {
    let mut request = json!({});
    if let Some(catalog_id) = catalog {
        request["CatalogId"] = json!(catalog_id);
    }
    let answer = ok(router, "ImportCatalogToGlue", &request).await;
    assert_eq!(
        answer,
        json!({}),
        "case {case}: ImportCatalogToGlue reports no members, got {answer}"
    );
}

/// Seconds since the epoch, the scale an instant crosses the wire on.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64()
}

/// Assert of one reported status the three things that hold of EVERY status this service
/// reports, and answer the instant it was stamped at: the migration is complete, the
/// importer is the service's own marker rather than any identity, and an instant is
/// reported.
fn assert_service_authored(case: &str, reported: &Value) -> f64 {
    assert_eq!(
        reported["ImportCompleted"],
        json!(true),
        "case {case}: expected a completed migration, got {reported}"
    );
    assert_eq!(
        reported["ImportedBy"],
        json!(STATUS_SET_BY_DEFAULT),
        "case {case}: expected the service's own marker as the importer, got {reported}"
    );
    assert!(
        !reported.to_string().contains(CALLER_PRINCIPAL),
        "case {case}: the status is the service's own, so no part of it may report the \
         calling identity, got {reported}"
    );
    let stamped_at = reported["ImportTime"].as_f64().unwrap_or_else(|| {
        panic!("case {case}: expected the instant the status was stamped, got {reported}")
    });
    assert!(
        stamped_at > 0.0,
        "case {case}: expected an instant past the epoch, got {stamped_at}"
    );
    stamped_at
}

/// A catalog nothing has migrated reports its migration as complete, under the service's
/// own marker and stamped with an instant, and the read reports rather than writes:
/// repeated reads, with and without the `CatalogId` named, answer the same thing.
#[tokio::test]
async fn an_unmigrated_catalog_reports_the_status_it_carries_by_default() {
    let router = emulator();
    let before = epoch_seconds(SystemTime::now()).ceil();

    // (case, the request body)
    let cases: Vec<(&str, Value)> = vec![
        // The recorded case: the status of the account's own Data Catalog, read with no
        // CatalogId at all.
        ("the catalog id left to default", json!({})),
        (
            "the catalog id named explicitly",
            json!({ "CatalogId": ACCOUNT_ID }),
        ),
    ];

    let mut first_reported: Option<Value> = None;
    for (case, request) in &cases {
        let first = import_status(&router, request).await;
        let stamped_at = assert_service_authored(case, &first);
        assert!(
            stamped_at <= before,
            "case {case}: a catalog no import has migrated carries a stamp taken before the \
             read, at most {before}, got {stamped_at}"
        );
        // Reporting, not mutating: a second read of the same catalog answers the same, and
        // so does a read of the other spelling of the same catalog id.
        let second = import_status(&router, request).await;
        assert_eq!(
            second, first,
            "case {case}: a read must not change what a later read sees; got {first} then \
             {second}"
        );
        let expected = first_reported.get_or_insert_with(|| first.clone());
        assert_eq!(
            &first, expected,
            "case {case}: both spellings of one catalog id address one status; expected \
             {expected}, got {first}"
        );
    }

    let after_reads = import_status(&router, &json!({})).await;
    assert_eq!(
        Some(&after_reads),
        first_reported.as_ref(),
        "case after four reads: the same status must still be reported, got {after_reads}"
    );
}

/// An import is reported by every later status read of the same catalog: the migration is
/// complete, its instant is the one the import was served at, and the importer is still the
/// service's own marker — whichever spelling of the catalog id the two calls used.
#[tokio::test]
async fn an_import_is_reported_by_a_later_status_read() {
    // (case, the CatalogId the import names, the read's request body)
    let cases: Vec<(&str, Option<&str>, Value)> = vec![
        ("imported defaulted, read defaulted", None, json!({})),
        (
            "imported defaulted, read named",
            None,
            json!({ "CatalogId": ACCOUNT_ID }),
        ),
        (
            "imported named, read defaulted",
            Some(ACCOUNT_ID),
            json!({}),
        ),
        (
            "imported named, read named",
            Some(ACCOUNT_ID),
            json!({ "CatalogId": ACCOUNT_ID }),
        ),
    ];

    for (case, imported_catalog, read_request) in &cases {
        let router = emulator();
        let before = epoch_seconds(SystemTime::now()).floor();
        import_catalog(&router, case, *imported_catalog).await;
        let after = epoch_seconds(SystemTime::now()).ceil();

        let reported = import_status(&router, read_request).await;
        let import_time = assert_service_authored(case, &reported);
        assert!(
            (before..=after).contains(&import_time),
            "case {case}: expected the instant the import was served, in {before}..={after}, \
             got {import_time}"
        );

        // Stored, not computed per call: a second read answers the same record.
        let again = import_status(&router, read_request).await;
        assert_eq!(
            again, reported,
            "case {case}: two reads of one catalog must agree; got {reported} then {again}"
        );
    }
}

/// A second import REPLACES the record rather than adding to it: the status reports the
/// migration last performed, so an older record is reported until an import supersedes it
/// and never afterwards.
#[tokio::test]
async fn a_later_import_replaces_the_recorded_migration() {
    // (case, how many seconds before the seeded epoch the record was written)
    let cases: Vec<(&str, u64)> = vec![
        ("a record written at the seeded instant", 0),
        ("a record written a day earlier", 86_400),
    ];

    for (case, seeded_offset) in &cases {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));

        let seeded_at = SystemTime::UNIX_EPOCH + SEEDED_IMPORT_EPOCH
            - Duration::from_secs(*seeded_offset);
        service_state(&*engine)
            .expect("the Glue store is reachable")
            .lock()
            .catalog_mut_for_create(ACCOUNT_ID)
            .expect("the account's own catalog is admitted")
            .migration = Some(CatalogMigrationEntry {
            import_time: seeded_at,
        });

        let seeded = import_status(&router, &json!({})).await;
        assert_service_authored(case, &seeded);
        assert_eq!(
            seeded["ImportTime"].as_f64(),
            Some(epoch_seconds(seeded_at)),
            "case {case}: expected the stored instant reported as it was written, got {seeded}"
        );

        import_catalog(&router, case, None).await;
        let replaced = import_status(&router, &json!({})).await;
        let import_time = assert_service_authored(case, &replaced);
        assert!(
            import_time > epoch_seconds(seeded_at),
            "case {case}: expected the instant of the migration last performed, after \
             {}, got {import_time}",
            epoch_seconds(seeded_at)
        );
    }
}

/// A `CatalogId` this emulator holds no Data Catalog under belongs to another account, so
/// both operations refuse it with HTTP 400 `AccessDeniedException` naming the catalog the
/// request addressed — and the refused import writes nothing, so this account's own catalog
/// reports no migration of its own.
#[tokio::test]
async fn a_catalog_id_this_account_does_not_hold_is_refused_by_both_operations() {
    // Ids that are not this emulator's account: another twelve-digit account, a shorter
    // one, and a name-shaped id.
    let foreign_ids = ["210987654321", "1234", "other-catalog"];

    for foreign in foreign_ids {
        let router = emulator();
        let stamped_before = assert_service_authored(
            &format!("{foreign:?} before the refusals"),
            &import_status(&router, &json!({})).await,
        );

        let requests: Vec<(&str, Value)> = vec![
            ("GetCatalogImportStatus", json!({ "CatalogId": foreign })),
            ("ImportCatalogToGlue", json!({ "CatalogId": foreign })),
        ];

        for (operation, request) in &requests {
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {foreign:?} / {operation}: expected 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("AccessDeniedException"),
                "case {foreign:?} / {operation}: expected AccessDeniedException, got {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(foreign),
                "case {foreign:?} / {operation}: the refusal is built from the request, so it \
                 must name the CatalogId it refused, got {message:?}"
            );
        }

        // The refused import stored nothing: this account's catalog still carries the
        // status it had before, stamp and all.
        let reported = import_status(&router, &json!({})).await;
        let stamped_after = assert_service_authored(&format!("{foreign:?}"), &reported);
        assert_eq!(
            stamped_after, stamped_before,
            "case {foreign:?}: a refused import must leave this account's catalog alone; \
             expected the stamp {stamped_before}, got {reported}"
        );
    }
}

/// The migration record is the catalog's own: importing leaves the catalog's databases and
/// tables readable, and creating resources afterwards does not disturb the record the import
/// stored.
#[tokio::test]
async fn the_migration_record_lives_beside_the_catalog_resources() {
    let router = emulator();
    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "import_status_db" } }),
    )
    .await;

    import_catalog(&router, "a catalog holding a database", None).await;
    let migrated = import_status(&router, &json!({})).await;

    let database = ok(
        &router,
        "GetDatabase",
        &json!({ "Name": "import_status_db" }),
    )
    .await;
    assert_eq!(
        database["Database"]["Name"],
        json!("import_status_db"),
        "a database of a migrated catalog must still read back, got {database}"
    );

    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "import_status_db",
            "TableInput": { "Name": "import_status_tbl" }
        }),
    )
    .await;
    let after_create = import_status(&router, &json!({})).await;
    assert_eq!(
        after_create, migrated,
        "creating catalog resources must not change the migration record; expected \
         {migrated}, got {after_create}"
    );
}
