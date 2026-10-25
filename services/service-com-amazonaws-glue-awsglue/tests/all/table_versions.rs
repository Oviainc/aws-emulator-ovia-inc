// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetTableVersion`: reading ONE version of a table out of the
//! catalog's version history.
//!
//! The rule under test: a version read answers the definition the table was written
//! with at the version it names — the live definition for the version the table's
//! `VersionId` names (so a table that has never been updated has exactly one version),
//! and the archived definition for every earlier one. A request that names no version
//! addresses the table's current one; a version the table's history does not hold is
//! `EntityNotFoundException`, and a `VersionId` that spells no counter is
//! `InvalidInputException`.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness (`POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body), dispatched by the same `MultiServiceRouter` the production front
//! door uses.
//!
//! The suites are tables: a row is one history (how many writes the table has had and
//! which member set each one submitted) or one malformed request, and the recorded case
//! — a create carrying `Parameters` followed by an update that omits them — is one row.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::TableInput;
use service_com_amazonaws_glue_awsglue::state::{
    ArchivedTableVersion, ComAmazonawsGlueAwsglueState, MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    assert_echoed, call, emulator, emulator_on, error_code, ok, ACCOUNT_ID, CALLER_PRINCIPAL,
};

/// The table every history in these suites writes. One name, so a case is identified by
/// its history and not by a name the rule does not generalise over.
const TABLE: &str = "events";

/// Create `database`, create the table the history's FIRST definition submits, then
/// apply every later definition with `UpdateTable` — so the table ends up with one
/// version per submitted definition, numbered from the first.
async fn write_history(router: &MultiServiceRouter, database: &str, history: &[Value]) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    let (created, updates) = history
        .split_first()
        .expect("a table's history starts with the create");
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": created }),
    )
    .await;
    for definition in updates {
        ok(
            router,
            "UpdateTable",
            &json!({ "DatabaseName": database, "TableInput": definition }),
        )
        .await;
    }
}

/// The `TableVersion` body `GetTableVersion` reports for the version `version_id` names,
/// or for the table's current version when it names none.
async fn get_table_version(
    router: &MultiServiceRouter,
    database: &str,
    version_id: Option<&str>,
) -> Value {
    let mut body = json!({ "DatabaseName": database, "TableName": TABLE });
    if let Some(version_id) = version_id {
        body["VersionId"] = json!(version_id);
    }
    let answer = ok(router, "GetTableVersion", &body).await;
    answer["TableVersion"].clone()
}

/// The stored `Table` body `GetTable` reports — the table as it stands now.
async fn get_table(router: &MultiServiceRouter, database: &str) -> Value {
    let answer = ok(
        router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": TABLE }),
    )
    .await;
    answer["Table"].clone()
}

/// What a `TableInput` member reads back as on a definition that does NOT carry it.
///
/// Absent is absent for every member the model declares nullable, and awsJson omits
/// those. `Retention` is the one member of `com.amazonaws.glue#TableInput` that is a
/// plain (non-nullable) integer (source: `smithy_query shape=TableInput`), so a
/// definition that carries no retention reads back as the zero it defaults to rather
/// than as nothing.
fn absence_reads_as(member: &str) -> Value {
    match member {
        "Retention" => json!(0),
        _ => Value::Null,
    }
}

/// Assert `got` is the definition `submitted` wrote: every member `submitted` carries
/// reappears unchanged, and every member only some OTHER definition of the same history
/// carries is absent. So a version read that served a neighbouring version's definition
/// — the live one for an archived version, or an archived one for the live version —
/// fails here even when the two agree on the members they share.
fn assert_definition_is(case: &str, submitted: &Value, history: &[Value], got: &Value) {
    assert_echoed(case, submitted, got);
    for definition in history {
        for member in definition
            .as_object()
            .expect("a submitted definition is a structure")
            .keys()
        {
            if member == "Name" || submitted.get(member).is_some() {
                continue;
            }
            let read_back = got.get(member).unwrap_or(&Value::Null);
            let absent = absence_reads_as(member);
            assert!(
                read_back.is_null() || read_back == &absent,
                "case {case}: member {member} is absent from the definition this version was \
                 written with, so this version must report {absent}; got {read_back}"
            );
        }
    }
}

/// A storage descriptor, so one row of the table moves a nested structure rather than a
/// scalar or a map.
fn storage_descriptor(location: &str) -> Value {
    json!({
        "Columns": [ { "Name": "id", "Type": "string" } ],
        "Location": location,
        "InputFormat": "org.apache.hadoop.mapred.TextInputFormat",
    })
}

/// A `Parameters` map of `entries` keys, for the row that moves the entry COUNT.
fn parameters(entries: usize) -> Value {
    let map: serde_json::Map<String, Value> = (0..entries)
        .map(|index| (format!("key_{index}"), json!(format!("value_{index}"))))
        .collect();
    Value::Object(map)
}

/// One row: the case name, the database it is written in, and the definitions the table
/// is written with, oldest first.
type HistoryCase = (&'static str, &'static str, Vec<Value>);

/// The histories the suites below read back. The rows differ along what the rule
/// generalises over: how many writes the table has had, which member set each write
/// submits (a map added, a map dropped, a map replaced, a nested structure replaced),
/// and how many entries a map carries.
fn histories() -> Vec<HistoryCase> {
    vec![
        (
            "a table that has never been updated",
            "fresh_db",
            vec![json!({ "Name": TABLE, "Parameters": { "classification": "json" } })],
        ),
        (
            "an update that omits the parameters the create set",
            "cleared_db",
            vec![
                json!({ "Name": TABLE, "Parameters": { "classification": "json" } }),
                json!({ "Name": TABLE }),
            ],
        ),
        (
            "an update that replaces the map rather than merging it",
            "replaced_db",
            vec![
                json!({ "Name": TABLE, "Parameters": { "left": "1", "right": "2" } }),
                json!({ "Name": TABLE, "Parameters": { "only": "3" } }),
            ],
        ),
        (
            "three updates over a growing then shrinking definition",
            "grown_db",
            vec![
                json!({ "Name": TABLE }),
                json!({ "Name": TABLE, "Description": "the second definition" }),
                json!({
                    "Name": TABLE,
                    "Description": "the third definition",
                    "Owner": "analytics",
                    "TableType": "EXTERNAL_TABLE",
                    "Retention": 7,
                    "StorageDescriptor": storage_descriptor("s3://bucket/third"),
                }),
                json!({ "Name": TABLE, "StorageDescriptor": storage_descriptor("s3://bucket/fourth") }),
            ],
        ),
        (
            "a ten-entry map replaced by a bare definition",
            "counted_db",
            vec![
                json!({ "Name": TABLE, "Parameters": parameters(10) }),
                json!({ "Name": TABLE, "Parameters": parameters(1) }),
                json!({ "Name": TABLE }),
            ],
        ),
    ]
}

/// Every version of a table reports the definition that version was written with, under
/// the `VersionId` that names it, beside the members that belong to the table itself.
#[tokio::test]
async fn each_version_reports_the_definition_it_was_written_with() {
    for (case, database, history) in histories() {
        let router = emulator();
        write_history(&router, database, &history).await;
        let live = get_table(&router, database).await;

        for (counter, submitted) in history.iter().enumerate() {
            let version_id = counter.to_string();
            let case = format!("{case} at version {version_id}");
            let version = get_table_version(&router, database, Some(&version_id)).await;
            assert_eq!(
                version["VersionId"].as_str(),
                Some(version_id.as_str()),
                "case {case}: TableVersion.VersionId must name the version read, got {version}"
            );
            let table = &version["Table"];
            assert_definition_is(&case, submitted, &history, table);
            assert_eq!(
                table["VersionId"].as_str(),
                Some(version_id.as_str()),
                "case {case}: the Table carries the version's own id, got {table}"
            );
            // The members that belong to the TABLE, not to one of its versions, are the
            // same on every version of it.
            assert_eq!(
                table["Name"].as_str(),
                Some(TABLE),
                "case {case}: every version is a version of the same table, got {table}"
            );
            assert_eq!(
                table["DatabaseName"].as_str(),
                Some(database),
                "case {case}: every version names the database holding the table, got {table}"
            );
            assert_eq!(
                table["CatalogId"].as_str(),
                Some(ACCOUNT_ID),
                "case {case}: every version names the catalog holding the table, got {table}"
            );
            assert_eq!(
                table["CreatedBy"].as_str(),
                Some(CALLER_PRINCIPAL),
                "case {case}: every version names the principal that created the table, got \
                 {table}"
            );
            assert_eq!(
                table["CreateTime"], live["CreateTime"],
                "case {case}: CreateTime is set once at create, so every version reports it; \
                 expected {}, got {}",
                live["CreateTime"], table["CreateTime"]
            );
        }

        // The newest version is the table itself: the whole live definition, member for
        // member, is what the last write submitted.
        let newest = (history.len() - 1).to_string();
        let version = get_table_version(&router, database, Some(&newest)).await;
        assert_eq!(
            version["Table"], live,
            "case {case}: the newest version is the live table; expected {live}, got {}",
            version["Table"]
        );
    }
}

/// A request that names no `VersionId` addresses the table's current version — the one
/// the live `Table` reports — whatever the table's history holds.
#[tokio::test]
async fn a_request_naming_no_version_serves_the_current_one() {
    for (case, database, history) in histories() {
        let router = emulator();
        write_history(&router, database, &history).await;
        let live = get_table(&router, database).await;

        let version = get_table_version(&router, database, None).await;
        assert_eq!(
            version["VersionId"].as_str(),
            live["VersionId"].as_str(),
            "case {case}: expected the live version {}, got {version}",
            live["VersionId"]
        );
        assert_eq!(
            version["Table"], live,
            "case {case}: expected the live table {live}, got {}",
            version["Table"]
        );
        // One version per write, numbered from the first, so the counter the live table
        // carries is the number of writes minus one.
        assert_eq!(
            live["VersionId"].as_str(),
            Some((history.len() - 1).to_string().as_str()),
            "case {case}: each write makes one version, got {live}"
        );
    }
}

/// A `VersionId` the table's history does not hold is `EntityNotFoundException`, and one
/// that spells no version counter is `InvalidInputException`. The rows span both kinds of
/// malformed id (not a number, not an integer, not a counter) and both kinds of absence
/// (a version past the newest, and any version of a table or database the catalog does
/// not hold).
#[tokio::test]
async fn an_unreadable_version_is_refused() {
    let router = emulator();
    write_history(
        &router,
        "refusing_db",
        &[
            json!({ "Name": TABLE, "Description": "as created" }),
            json!({ "Name": TABLE, "Description": "as updated" }),
        ],
    )
    .await;

    // (case, request, expected error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a version past the newest",
            json!({ "DatabaseName": "refusing_db", "TableName": TABLE, "VersionId": "7" }),
            "EntityNotFoundException",
        ),
        (
            "a version id that names no number",
            json!({ "DatabaseName": "refusing_db", "TableName": TABLE, "VersionId": "newest" }),
            "InvalidInputException",
        ),
        (
            "a version id that names no integer",
            json!({ "DatabaseName": "refusing_db", "TableName": TABLE, "VersionId": "1.5" }),
            "InvalidInputException",
        ),
        (
            "a version id that names no counter",
            json!({ "DatabaseName": "refusing_db", "TableName": TABLE, "VersionId": "-1" }),
            "InvalidInputException",
        ),
        (
            "a table the database does not hold",
            json!({ "DatabaseName": "refusing_db", "TableName": "absent", "VersionId": "0" }),
            "EntityNotFoundException",
        ),
        (
            "a database the catalog does not hold",
            json!({ "DatabaseName": "absent_db", "TableName": TABLE, "VersionId": "0" }),
            "EntityNotFoundException",
        ),
        (
            "a catalog this emulator does not hold",
            json!({
                "CatalogId": "999999999999",
                "DatabaseName": "refusing_db",
                "TableName": TABLE,
                "VersionId": "0",
            }),
            "EntityNotFoundException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, answer) = call(&router, "GetTableVersion", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: a refused version read is HTTP 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).ends_with(expected),
            "case {case}: expected {expected}, got {}",
            error_code(&answer)
        );
    }

    // A version id a table HAS is still served after every refusal, and the padded
    // spelling of a counter names the same version, reported under its canonical id.
    for (case, version_id) in [("the first version", "0"), ("a padded first version", "00")] {
        let version = get_table_version(&router, "refusing_db", Some(version_id)).await;
        assert_eq!(
            version["VersionId"].as_str(),
            Some("0"),
            "case {case}: expected the canonical version id 0, got {version}"
        );
        assert_eq!(
            version["Table"]["Description"].as_str(),
            Some("as created"),
            "case {case}: expected the definition version 0 was written with, got {version}"
        );
    }
}

/// "By default, `UpdateTable` always creates an archived version of the table before
/// updating it. However, if `SkipArchive` is set to true, `UpdateTable` does not create
/// the archived version" — so the version the update replaced is readable afterwards
/// exactly when the update did not ask to skip the archive. The rows span the three
/// states of the flag.
#[tokio::test]
async fn skipping_the_archive_leaves_the_replaced_version_unreadable() {
    // (case, the SkipArchive member of the update, whether version 0 survives it)
    let cases: Vec<(&str, Value, bool)> = vec![
        ("an update that does not mention SkipArchive", Value::Null, true),
        ("an update with SkipArchive false", json!(false), true),
        ("an update with SkipArchive true", json!(true), false),
    ];

    for (case, skip_archive, archived) in cases {
        let router = emulator();
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": "archive_db" } }),
        )
        .await;
        ok(
            &router,
            "CreateTable",
            &json!({
                "DatabaseName": "archive_db",
                "TableInput": { "Name": TABLE, "Description": "as created" },
            }),
        )
        .await;
        let mut update = json!({
            "DatabaseName": "archive_db",
            "TableInput": { "Name": TABLE, "Description": "as updated" },
        });
        if !skip_archive.is_null() {
            update["SkipArchive"] = skip_archive;
        }
        ok(&router, "UpdateTable", &update).await;

        // The update moved the table on either way.
        let current = get_table_version(&router, "archive_db", None).await;
        assert_eq!(
            current["VersionId"].as_str(),
            Some("1"),
            "case {case}: the update makes the table's next version, got {current}"
        );
        assert_eq!(
            current["Table"]["Description"].as_str(),
            Some("as updated"),
            "case {case}: the current version is what the update submitted, got {current}"
        );

        let (status, answer) = call(
            &router,
            "GetTableVersion",
            &json!({ "DatabaseName": "archive_db", "TableName": TABLE, "VersionId": "0" }),
        )
        .await;
        if archived {
            assert_eq!(
                status, 200,
                "case {case}: the replaced definition is archived, so version 0 is readable; \
                 got {status} with {answer}"
            );
            assert_eq!(
                answer["TableVersion"]["Table"]["Description"].as_str(),
                Some("as created"),
                "case {case}: version 0 is the definition the create wrote, got {answer}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: the replaced definition was not archived, so version 0 is gone; \
                 got {status} with {answer}"
            );
            assert!(
                error_code(&answer).ends_with("EntityNotFoundException"),
                "case {case}: expected EntityNotFoundException, got {}",
                error_code(&answer)
            );
        }
    }
}

/// A version read is a READ: it leaves the table, its current version and its history
/// exactly as the writes left them, so a later caller sees what the earlier one did.
#[tokio::test]
async fn reading_a_version_changes_nothing() {
    let router = emulator();
    let history = [
        json!({ "Name": TABLE, "Parameters": { "classification": "json" } }),
        json!({ "Name": TABLE }),
        json!({ "Name": TABLE, "Description": "the newest definition" }),
    ];
    write_history(&router, "stable_db", &history).await;

    let before = get_table(&router, "stable_db").await;
    let versions_before: Vec<Value> = vec![
        get_table_version(&router, "stable_db", Some("0")).await,
        get_table_version(&router, "stable_db", Some("1")).await,
        get_table_version(&router, "stable_db", Some("2")).await,
        get_table_version(&router, "stable_db", None).await,
    ];

    // Read every version again, twice over, including the refused ones.
    for _ in 0..2 {
        for version_id in ["0", "1", "2", "7", "not-a-version"] {
            call(
                &router,
                "GetTableVersion",
                &json!({
                    "DatabaseName": "stable_db",
                    "TableName": TABLE,
                    "VersionId": version_id,
                }),
            )
            .await;
        }
        get_table_version(&router, "stable_db", None).await;
    }

    assert_eq!(
        get_table(&router, "stable_db").await,
        before,
        "case after repeated version reads: the live table must be untouched, expected {before}"
    );
    let versions_after: Vec<Value> = vec![
        get_table_version(&router, "stable_db", Some("0")).await,
        get_table_version(&router, "stable_db", Some("1")).await,
        get_table_version(&router, "stable_db", Some("2")).await,
        get_table_version(&router, "stable_db", None).await,
    ];
    assert_eq!(
        versions_after, versions_before,
        "case after repeated version reads: every version must read back the same, expected \
         {versions_before:?}"
    );
}

/// Seed `archived` archived versions straight into the stored table, each one a
/// definition carrying its own counter, so the per-table ceiling is reachable without
/// driving a thousand updates over the wire (which would exercise the transport rather
/// than the ceiling).
fn seed_archived_versions(engine: &Arc<engine::InMemoryEngine>, database: &str, archived: u64) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let table = store
        .database_mut(ACCOUNT_ID, database)
        .expect("the seeded database was created")
        .tables
        .get_mut(TABLE)
        .expect("the seeded table was created");
    for counter in 0..archived {
        table.archived_versions.insert(
            counter,
            ArchivedTableVersion {
                definition: TableInput {
                    name: TABLE.to_string(),
                    description: Some(format!("seeded version {counter}")),
                    ..TableInput::default()
                },
                update_time: SystemTime::now(),
            },
        );
    }
    table.version_id = archived.to_string();
}

/// The store is resident memory and an update is reachable by any caller, so the number
/// of definitions one table keeps archived is capped: the update that fills the ceiling
/// is accepted, the one past it is refused with the modelled
/// `ResourceNumberLimitExceededException` and changes nothing — and an update that asks
/// not to archive is still accepted, because it adds no archived version.
#[tokio::test]
async fn an_update_archiving_past_the_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    write_history(
        &router,
        "filled_db",
        &[json!({ "Name": TABLE, "Description": "as created" })],
    )
    .await;
    let archived = MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE as u64 - 1;
    seed_archived_versions(&engine, "filled_db", archived);

    // The last update under the ceiling archives the definition it replaces.
    ok(
        &router,
        "UpdateTable",
        &json!({
            "DatabaseName": "filled_db",
            "TableInput": { "Name": TABLE, "Description": "the last archiving update" },
        }),
    )
    .await;

    let (status, answer) = call(
        &router,
        "UpdateTable",
        &json!({
            "DatabaseName": "filled_db",
            "TableInput": { "Name": TABLE, "Description": "one archived version too many" },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case archived version {} of {MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE}: expected 400, got \
         {status} with {answer}",
        MAX_ARCHIVED_TABLE_VERSIONS_PER_TABLE + 1
    );
    assert!(
        error_code(&answer).ends_with("ResourceNumberLimitExceededException"),
        "case one archived version too many: expected ResourceNumberLimitExceededException, got \
         {}",
        error_code(&answer)
    );

    // The refusal wrote nothing: the table is still what the accepted update left.
    let current = get_table_version(&router, "filled_db", None).await;
    assert_eq!(
        current["Table"]["Description"].as_str(),
        Some("the last archiving update"),
        "case after the refusal: a refused update must not write, got {current}"
    );

    // An update that archives nothing is still served at the ceiling.
    ok(
        &router,
        "UpdateTable",
        &json!({
            "DatabaseName": "filled_db",
            "TableInput": { "Name": TABLE, "Description": "an update that skips the archive" },
            "SkipArchive": true,
        }),
    )
    .await;
    let current = get_table_version(&router, "filled_db", None).await;
    assert_eq!(
        current["Table"]["Description"].as_str(),
        Some("an update that skips the archive"),
        "case at the ceiling with SkipArchive: an update that archives nothing is served, got \
         {current}"
    );
}
