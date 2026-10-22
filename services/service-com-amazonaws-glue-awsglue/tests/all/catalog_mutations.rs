// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Data Catalog MUTATIONS: `BatchDeleteTable`, `UpdateTable`
//! and `UpdateDatabase`.
//!
//! The rule under test is read-after-write: a mutation that reports success has
//! changed the record the readers consult, so the next `GetTable` / `GetTables` /
//! `GetDatabase` reports the mutated catalog and never the pre-mutation entity.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness (`POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body), dispatched by the same `MultiServiceRouter` the production
//! front door uses.
//!
//! The suites are tables: each row is one input along the axis the rule generalises
//! over (how many tables a delete names, which member set an update submits, how many
//! entries a map carries, the casing of a name), and the recorded case is one row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, error_code, ok, table_names};

/// Create `name` as a database from the submitted `DatabaseInput`.
async fn create_database(router: &MultiServiceRouter, database_input: &Value) {
    ok(router, "CreateDatabase", &json!({ "DatabaseInput": database_input })).await;
}

/// Create a table in `database` from the submitted `TableInput`.
async fn create_table(router: &MultiServiceRouter, database: &str, table_input: &Value) {
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": table_input }),
    )
    .await;
}

/// Every table this database holds, as `GetTables` reports it. `MaxResults` is at the
/// modelled maximum so one page holds every table these suites create.
async fn stored_tables(router: &MultiServiceRouter, database: &str) -> Vec<String> {
    let page = ok(
        router,
        "GetTables",
        &json!({ "DatabaseName": database, "MaxResults": 100 }),
    )
    .await;
    table_names(&page)
}

/// `Vec<String>` from a list of literals, so a table of cases reads as names.
fn names(from: &[&str]) -> Vec<String> {
    from.iter().map(|name| (*name).to_string()).collect()
}

/// The submitted input with its `Name` folded the way the catalog stores it — the
/// model documents the fold for both `DatabaseInput$Name` and `TableInput$Name` ("For
/// Hive compatibility, this is folded to lowercase when it is stored"), so the folded
/// name is what a read reports and what a round-trip assertion compares against.
fn with_folded_name(input: &Value) -> Value {
    let mut folded = input.clone();
    let name = folded["Name"]
        .as_str()
        .expect("the input names the entity")
        .to_ascii_lowercase();
    folded["Name"] = json!(name);
    folded
}

/// The stored `Table` body `GetTable` reports.
async fn get_table(router: &MultiServiceRouter, database: &str, name: &str) -> Value {
    let answer = ok(
        router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": name }),
    )
    .await;
    answer["Table"].clone()
}

/// The stored `Database` body `GetDatabase` reports.
async fn get_database(router: &MultiServiceRouter, name: &str) -> Value {
    let answer = ok(router, "GetDatabase", &json!({ "Name": name })).await;
    answer["Database"].clone()
}

/// Assert that every member `created` submitted and `updated` did NOT is cleared from
/// the read-back entity: an `Input` is the whole client-settable definition, so an
/// update writes what it carries and drops what it omits.
///
/// A member the client cannot set (`Name`, and the server-set members) is not part of
/// this check.
fn assert_omitted_members_are_cleared(case: &str, created: &Value, updated: &Value, got: &Value) {
    for member in created
        .as_object()
        .expect("the created input is a structure")
        .keys()
    {
        if member == "Name" || updated.get(member).is_some() {
            continue;
        }
        let read_back = got.get(member).unwrap_or(&Value::Null);
        assert!(
            read_back.is_null(),
            "case {case}: member {member} is absent from the submitted input, so the update \
             must have cleared it; got {read_back:?}"
        );
    }
}

/// The `VersionId` of a read-back table, as the counter it is.
fn version_id(table: &Value) -> u64 {
    table["VersionId"]
        .as_str()
        .unwrap_or_else(|| panic!("Table.VersionId is a string, got {table}"))
        .parse::<u64>()
        .unwrap_or_else(|_| panic!("Table.VersionId is a decimal counter, got {table}"))
}

/// One row of the `BatchDeleteTable` table: the case name, the tables created, the
/// names the batch deletes, and the tables that must remain afterwards.
type DeleteCase = (&'static str, Vec<String>, Vec<String>, Vec<String>);

/// One row of the partial-failure table: a [`DeleteCase`] with the names `Errors`
/// must report inserted before the tables that must remain.
type PartialDeleteCase = (
    &'static str,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
);

// ---------------------------------------------------------------------------
// BatchDeleteTable
// ---------------------------------------------------------------------------

/// `BatchDeleteTable` removes every table it names from the record the read path
/// consults: after a call that reports an empty `Errors` list, each named table is a
/// miss on `GetTable` (HTTP 400 `EntityNotFoundException` with a `Message`) and gone
/// from `GetTables`, exactly as after the single-table `DeleteTable` — and the freed
/// name can be created again.
///
/// The rows differ along what the batch names: one table, every table in the database,
/// a subset (so the tables it does not name must survive), names in a casing other
/// than the stored fold, and both published bounds of `TablesToDelete`
/// (`BatchDeleteTableNameList`, `len 0..100`, per `smithy_query(query="operation",
/// operation="BatchDeleteTable")`): the empty list and 100 names.
#[tokio::test]
async fn batch_delete_table_removes_every_table_it_names() {
    let hundred: Vec<String> = (0..100).map(|index| format!("table_{index:03}")).collect();

    let cases: Vec<DeleteCase> = vec![
        (
            "the only table",
            names(&["events"]),
            names(&["events"]),
            names(&[]),
        ),
        (
            "every table in the database",
            names(&["clicks", "events", "orders"]),
            names(&["events", "orders", "clicks"]),
            names(&[]),
        ),
        (
            "a subset leaves the rest",
            names(&["clicks", "events", "orders"]),
            names(&["events"]),
            names(&["clicks", "orders"]),
        ),
        (
            "names in another casing address the folded tables",
            names(&["clicks", "events", "orders"]),
            names(&["EVENTS", "Clicks"]),
            names(&["orders"]),
        ),
        (
            "the empty list, the list's minimum, deletes nothing",
            names(&["events"]),
            names(&[]),
            names(&["events"]),
        ),
        (
            "100 names, the list's maximum",
            hundred.clone(),
            hundred.clone(),
            names(&[]),
        ),
    ];

    for (case, created, to_delete, remaining) in &cases {
        let router = emulator();
        create_database(&router, &json!({ "Name": "batch_db" })).await;
        for table in created {
            create_table(&router, "batch_db", &json!({ "Name": table })).await;
        }

        let answer = ok(
            &router,
            "BatchDeleteTable",
            &json!({ "DatabaseName": "batch_db", "TablesToDelete": to_delete }),
        )
        .await;
        let errors = answer["Errors"].as_array().cloned().unwrap_or_default();
        assert!(
            errors.is_empty(),
            "case {case}: every named table exists, so Errors must be empty, got {answer}"
        );

        for deleted in to_delete {
            let (status, body) = call(
                &router,
                "GetTable",
                &json!({ "DatabaseName": "batch_db", "Name": deleted }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}: GetTable on the deleted {deleted} must answer 400, got {status} \
                 with {body}"
            );
            assert!(
                error_code(&body).contains("EntityNotFoundException"),
                "case {case}: GetTable on the deleted {deleted} must answer \
                 EntityNotFoundException, got {}",
                error_code(&body)
            );
            assert!(
                body.get("Message").and_then(Value::as_str).is_some(),
                "case {case}: the failure body must carry a Message, got {body}"
            );
        }

        let left = stored_tables(&router, "batch_db").await;
        assert_eq!(
            &left, remaining,
            "case {case}: GetTables must report exactly the tables the batch did not name, \
             got {left:?}"
        );

        // A deleted name is free again — the teardown/recreate flow a client runs on
        // the strength of the empty `Errors` list.
        for deleted in to_delete {
            create_table(&router, "batch_db", &json!({ "Name": deleted })).await;
        }
    }
}

/// A name the database does not hold fails only its own entry: it is reported in
/// `Errors` with `ErrorCode` `EntityNotFoundException`, while every name that does
/// exist is deleted — the batch is not abandoned and does not claim a clean delete.
///
/// The rows differ along how many of the named tables are missing and where they sit
/// in the list: one missing, every name missing, a missing name between two present
/// ones, and the same table named twice (the second occurrence is already gone).
#[tokio::test]
async fn batch_delete_table_reports_the_names_it_could_not_delete() {
    let cases: Vec<PartialDeleteCase> = vec![
        (
            "one name missing",
            names(&["events", "orders"]),
            names(&["events", "absent"]),
            names(&["absent"]),
            names(&["orders"]),
        ),
        (
            "every name missing",
            names(&["events"]),
            names(&["ghost_one", "ghost_two"]),
            names(&["ghost_one", "ghost_two"]),
            names(&["events"]),
        ),
        (
            "a missing name between two present ones",
            names(&["clicks", "events", "orders"]),
            names(&["clicks", "absent", "orders"]),
            names(&["absent"]),
            names(&["events"]),
        ),
        (
            "the same table named twice",
            names(&["events", "orders"]),
            names(&["events", "EVENTS"]),
            names(&["events"]),
            names(&["orders"]),
        ),
    ];

    for (case, created, to_delete, expected_errors, remaining) in &cases {
        let router = emulator();
        create_database(&router, &json!({ "Name": "partial_db" })).await;
        for table in created {
            create_table(&router, "partial_db", &json!({ "Name": table })).await;
        }

        let answer = ok(
            &router,
            "BatchDeleteTable",
            &json!({ "DatabaseName": "partial_db", "TablesToDelete": to_delete }),
        )
        .await;
        let errors = answer["Errors"].as_array().cloned().unwrap_or_default();
        let reported: Vec<String> = errors
            .iter()
            .map(|entry| {
                entry["TableName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: TableError.TableName is a string"))
                    .to_string()
            })
            .collect();
        // `TableError$TableName`: "For Hive compatibility, this must be entirely
        // lowercase" — so a name sent in another casing is reported folded.
        assert_eq!(
            &reported, expected_errors,
            "case {case}: Errors must name exactly the tables the batch could not delete, \
             got {reported:?}"
        );
        for entry in &errors {
            assert_eq!(
                entry["ErrorDetail"]["ErrorCode"].as_str(),
                Some("EntityNotFoundException"),
                "case {case}: a missing table's ErrorCode must be EntityNotFoundException, \
                 got {entry}"
            );
            assert!(
                entry["ErrorDetail"]["ErrorMessage"].as_str().is_some(),
                "case {case}: a reported error must carry an ErrorMessage, got {entry}"
            );
        }

        let left = stored_tables(&router, "partial_db").await;
        assert_eq!(
            &left, remaining,
            "case {case}: the names that did exist must be deleted anyway, got {left:?}"
        );
    }
}

/// `TablesToDelete` is modelled `len 0..100`: a list one name past the maximum is
/// refused before the handler runs, and the refusal deletes nothing.
#[tokio::test]
async fn batch_delete_table_honours_the_modelled_list_length() {
    let router = emulator();
    create_database(&router, &json!({ "Name": "bounds_db" })).await;
    create_table(&router, "bounds_db", &json!({ "Name": "events" })).await;

    let too_many: Vec<String> = (0..101).map(|index| format!("table_{index:03}")).collect();
    let (status, body) = call(
        &router,
        "BatchDeleteTable",
        &json!({ "DatabaseName": "bounds_db", "TablesToDelete": too_many }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case 101 names: a list past the modelled maximum must be refused, got {status} \
         with {body}"
    );

    let left = stored_tables(&router, "bounds_db").await;
    assert_eq!(
        left,
        names(&["events"]),
        "case 101 names: a refused batch must leave the catalog untouched, got {left:?}"
    );
}

// ---------------------------------------------------------------------------
// UpdateTable
// ---------------------------------------------------------------------------

/// `UpdateTable` writes the submitted `TableInput` into the stored table, so the next
/// `GetTable` reports exactly what was submitted: every member it carries is in
/// effect, every member it omits is cleared, and the table's `VersionId` advances
/// while the server-set `CreateTime` stays as the create left it.
///
/// The rows differ along the member set submitted: a map added where the create set
/// none (the recorded case), a map replaced rather than merged, a fuller definition
/// over a bare one, a bare definition over a fuller one, a map with ten entries (the
/// count axis), and a name in another casing addressing the folded table.
#[tokio::test]
async fn update_table_writes_the_submitted_definition() {
    let many_parameters: serde_json::Map<String, Value> = (0..10)
        .map(|index| (format!("key_{index}"), json!(format!("value_{index}"))))
        .collect();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "parameters added where the create set none",
            json!({ "Name": "events" }),
            json!({ "Name": "events", "Parameters": { "sim2real": "updated" } }),
        ),
        (
            "parameters replaced, not merged",
            json!({ "Name": "events", "Parameters": { "tier": "bronze", "dropped": "yes" } }),
            json!({ "Name": "events", "Parameters": { "tier": "silver" } }),
        ),
        (
            "a fuller definition over a bare one",
            json!({ "Name": "events" }),
            json!({
                "Name": "events",
                "Description": "raw events, revised",
                "Owner": "data-eng",
                "TableType": "EXTERNAL_TABLE",
                "Retention": 7,
                "Parameters": { "classification": "parquet", "compressionType": "snappy" },
                "StorageDescriptor": {
                    "Columns": [
                        { "Name": "id", "Type": "string" },
                        { "Name": "amount", "Type": "double", "Comment": "cents" }
                    ],
                    "Location": "s3://bucket/events/v2/",
                    "InputFormat": "org.apache.hadoop.mapred.TextInputFormat"
                },
                "PartitionKeys": [ { "Name": "dt", "Type": "string" } ]
            }),
        ),
        (
            "a bare definition over a fuller one",
            json!({
                "Name": "events",
                "Description": "raw events",
                "Owner": "data-eng",
                "TableType": "EXTERNAL_TABLE",
                "Parameters": { "classification": "json" },
                "ViewOriginalText": "SELECT 1",
                "StorageDescriptor": { "Location": "s3://bucket/events/" }
            }),
            json!({ "Name": "events" }),
        ),
        (
            "a ten-entry parameters map",
            json!({ "Name": "events", "Parameters": { "only": "one" } }),
            json!({ "Name": "events", "Parameters": Value::Object(many_parameters) }),
        ),
        (
            "a name in another casing addresses the folded table",
            json!({ "Name": "Mixed_Case_Events" }),
            json!({ "Name": "MIXED_case_EVENTS", "Parameters": { "tier": "silver" } }),
        ),
    ];

    for (case, created, submitted) in &cases {
        let router = emulator();
        create_database(&router, &json!({ "Name": "mutable_db" })).await;
        create_table(&router, "mutable_db", created).await;

        let stored_name = created["Name"]
            .as_str()
            .expect("the created input names the table")
            .to_ascii_lowercase();
        let before = get_table(&router, "mutable_db", &stored_name).await;

        let answer = ok(
            &router,
            "UpdateTable",
            &json!({ "DatabaseName": "mutable_db", "TableInput": submitted }),
        )
        .await;
        assert_eq!(
            answer,
            json!({}),
            "case {case}: UpdateTable returns an empty body, got {answer}"
        );

        let after = get_table(&router, "mutable_db", &stored_name).await;
        assert_echoed(case, &with_folded_name(submitted), &after);
        assert_omitted_members_are_cleared(case, created, submitted, &after);
        assert!(
            version_id(&after) > version_id(&before),
            "case {case}: the update makes a new version of the table; expected a VersionId \
             above {}, got {}",
            version_id(&before),
            version_id(&after)
        );
        assert_eq!(
            after["CreateTime"], before["CreateTime"],
            "case {case}: CreateTime is server-set at create and must not move on an update, \
             got {} after {}",
            after["CreateTime"], before["CreateTime"]
        );
        assert_eq!(
            after["DatabaseName"].as_str(),
            Some("mutable_db"),
            "case {case}: the updated table stays in its database, got {after}"
        );

        // The update is in effect for every reader, not just `GetTable`.
        let page = ok(
            &router,
            "GetTables",
            &json!({ "DatabaseName": "mutable_db" }),
        )
        .await;
        let listed = page["TableList"]
            .as_array()
            .expect("TableList is a list")
            .iter()
            .find(|table| table["Name"].as_str() == Some(stored_name.as_str()))
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: GetTables must list {stored_name}"));
        assert_echoed(case, &with_folded_name(submitted), &listed);
    }
}

/// Each `UpdateTable` makes a new version of the table, so the `VersionId` the read
/// path reports advances by one per update and never repeats.
#[tokio::test]
async fn repeated_updates_advance_the_table_version() {
    let router = emulator();
    create_database(&router, &json!({ "Name": "versioned_db" })).await;
    create_table(&router, "versioned_db", &json!({ "Name": "events" })).await;

    let mut seen = vec![version_id(&get_table(&router, "versioned_db", "events").await)];
    for round in 1..=4u64 {
        ok(
            &router,
            "UpdateTable",
            &json!({
                "DatabaseName": "versioned_db",
                "TableInput": { "Name": "events", "Parameters": { "round": round.to_string() } }
            }),
        )
        .await;
        let table = get_table(&router, "versioned_db", "events").await;
        assert_eq!(
            table["Parameters"]["round"].as_str(),
            Some(round.to_string().as_str()),
            "case round {round}: the read path must serve the newest definition, got {table}"
        );
        let version = version_id(&table);
        let previous = *seen.last().expect("the first version was recorded");
        assert_eq!(
            version,
            previous + 1,
            "case round {round}: expected version {}, got {version}",
            previous + 1
        );
        seen.push(version);
    }
}

// ---------------------------------------------------------------------------
// UpdateDatabase
// ---------------------------------------------------------------------------

/// `UpdateDatabase` writes the submitted `DatabaseInput` into the stored database, so
/// the next `GetDatabase` reports exactly what was submitted: every member it carries
/// is in effect, every member it omits is cleared, and the server-set `CreateTime`
/// stays as the create left it.
///
/// The rows differ along the member set submitted: a new description with a changed
/// parameters entry (the recorded case), a replaced location, a fuller definition over
/// a bare one, a bare definition over a fuller one, a three-entry parameters map (the
/// count axis), and a name in another casing addressing the folded database.
#[tokio::test]
async fn update_database_writes_the_submitted_definition() {
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "description and a parameters entry changed",
            json!({
                "Name": "probe_db",
                "Description": "sim2real probe database v1",
                "Parameters": { "tier": "bronze" }
            }),
            json!({
                "Name": "probe_db",
                "Description": "sim2real probe database v2",
                "Parameters": { "tier": "silver" }
            }),
        ),
        (
            "location replaced",
            json!({ "Name": "probe_db", "LocationUri": "s3://bucket/old/" }),
            json!({ "Name": "probe_db", "LocationUri": "s3://bucket/new/" }),
        ),
        (
            "a fuller definition over a bare one",
            json!({ "Name": "probe_db" }),
            json!({
                "Name": "probe_db",
                "Description": "fully described",
                "LocationUri": "s3://bucket/warehouse/",
                "Parameters": { "tier": "gold", "owner": "data-eng" },
                "CreateTableDefaultPermissions": [
                    {
                        "Principal": { "DataLakePrincipalIdentifier": "arn:aws:iam::000000000000:root" },
                        "Permissions": [ "ALL" ]
                    }
                ]
            }),
        ),
        (
            "a bare definition over a fuller one",
            json!({
                "Name": "probe_db",
                "Description": "described at create",
                "LocationUri": "s3://bucket/db/",
                "Parameters": { "tier": "bronze" }
            }),
            json!({ "Name": "probe_db" }),
        ),
        (
            "a three-entry parameters map",
            json!({ "Name": "probe_db", "Parameters": { "tier": "bronze" } }),
            json!({
                "Name": "probe_db",
                "Parameters": { "tier": "silver", "owner": "data-eng", "retention": "30" }
            }),
        ),
        (
            "a name in another casing addresses the folded database",
            json!({ "Name": "Probe_DB", "Description": "before" }),
            json!({ "Name": "PROBE_db", "Description": "after" }),
        ),
    ];

    for (case, created, submitted) in &cases {
        let router = emulator();
        create_database(&router, created).await;

        let stored_name = created["Name"]
            .as_str()
            .expect("the created input names the database")
            .to_ascii_lowercase();
        let before = get_database(&router, &stored_name).await;

        let answer = ok(
            &router,
            "UpdateDatabase",
            &json!({ "Name": stored_name, "DatabaseInput": submitted }),
        )
        .await;
        assert_eq!(
            answer,
            json!({}),
            "case {case}: UpdateDatabase returns an empty body, got {answer}"
        );

        let after = get_database(&router, &stored_name).await;
        assert_echoed(case, &with_folded_name(submitted), &after);
        assert_omitted_members_are_cleared(case, created, submitted, &after);
        assert_eq!(
            after["CreateTime"], before["CreateTime"],
            "case {case}: CreateTime is server-set at create and must not move on an update, \
             got {} after {}",
            after["CreateTime"], before["CreateTime"]
        );
    }
}

/// `DatabaseInput` carries the database's own name, so an update that submits a
/// different one moves the database to that name: the new name reads the updated
/// definition and the tables it owns (which report the new `DatabaseName`), the old
/// name is a miss, and a move onto a name already taken is `AlreadyExistsException`
/// with nothing moved.
#[tokio::test]
async fn update_database_moves_the_database_when_the_submitted_name_differs() {
    let router = emulator();
    create_database(
        &router,
        &json!({ "Name": "old_db", "Description": "before the move" }),
    )
    .await;
    create_table(&router, "old_db", &json!({ "Name": "events" })).await;
    create_database(&router, &json!({ "Name": "taken_db" })).await;

    ok(
        &router,
        "UpdateDatabase",
        &json!({
            "Name": "old_db",
            "DatabaseInput": { "Name": "new_db", "Description": "after the move" }
        }),
    )
    .await;

    let moved = get_database(&router, "new_db").await;
    assert_eq!(
        moved["Description"].as_str(),
        Some("after the move"),
        "case moved database: the new name must read the updated definition, got {moved}"
    );
    let table = get_table(&router, "new_db", "events").await;
    assert_eq!(
        table["DatabaseName"].as_str(),
        Some("new_db"),
        "case moved database: a table moves with its database, got {table}"
    );
    assert_eq!(
        stored_tables(&router, "new_db").await,
        names(&["events"]),
        "case moved database: the tables must be readable under the new name"
    );

    let (status, body) = call(&router, "GetDatabase", &json!({ "Name": "old_db" })).await;
    assert_eq!(
        status, 400,
        "case old name: a moved database must be unreachable by its old name, got {status} \
         with {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case old name: expected EntityNotFoundException, got {}",
        error_code(&body)
    );

    let (status, body) = call(
        &router,
        "UpdateDatabase",
        &json!({ "Name": "new_db", "DatabaseInput": { "Name": "taken_db" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case taken destination: a move onto an existing name must be refused, got {status} \
         with {body}"
    );
    assert!(
        error_code(&body).contains("AlreadyExistsException"),
        "case taken destination: expected AlreadyExistsException, got {}",
        error_code(&body)
    );
    let unchanged = get_database(&router, "new_db").await;
    assert_eq!(
        unchanged["Description"].as_str(),
        Some("after the move"),
        "case taken destination: the refused move must leave the database where it was, \
         got {unchanged}"
    );
    let destination = get_database(&router, "taken_db").await;
    assert!(
        destination.get("Description").is_none(),
        "case taken destination: the refused move must not overwrite the destination, \
         got {destination}"
    );
}

// ---------------------------------------------------------------------------
// Modelled failures and refusals
// ---------------------------------------------------------------------------

/// A mutation that addresses a resource the catalog does not hold is
/// `EntityNotFoundException` (HTTP 400) and writes nothing. The rows span both axes of
/// absence (the database and the table) and every mutation of this slice.
#[tokio::test]
async fn mutations_on_absent_resources_answer_the_modelled_error() {
    let router = emulator();
    create_database(&router, &json!({ "Name": "present_db" })).await;
    create_table(&router, "present_db", &json!({ "Name": "events" })).await;
    let version_before = version_id(&get_table(&router, "present_db", "events").await);

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "BatchDeleteTable in an absent database",
            "BatchDeleteTable",
            json!({ "DatabaseName": "absent_db", "TablesToDelete": [ "events" ] }),
        ),
        (
            "BatchDeleteTable in an unknown catalog",
            "BatchDeleteTable",
            json!({
                "CatalogId": "111111111111",
                "DatabaseName": "present_db",
                "TablesToDelete": [ "events" ]
            }),
        ),
        (
            "UpdateTable on an absent table",
            "UpdateTable",
            json!({ "DatabaseName": "present_db", "TableInput": { "Name": "absent" } }),
        ),
        (
            "UpdateTable in an absent database",
            "UpdateTable",
            json!({ "DatabaseName": "absent_db", "TableInput": { "Name": "events" } }),
        ),
        (
            "UpdateTable in an unknown catalog",
            "UpdateTable",
            json!({
                "CatalogId": "111111111111",
                "DatabaseName": "present_db",
                "TableInput": { "Name": "events" }
            }),
        ),
        (
            "UpdateDatabase on an absent database",
            "UpdateDatabase",
            json!({ "Name": "absent_db", "DatabaseInput": { "Name": "absent_db" } }),
        ),
        (
            "UpdateDatabase in an unknown catalog",
            "UpdateDatabase",
            json!({
                "CatalogId": "111111111111",
                "Name": "present_db",
                "DatabaseInput": { "Name": "present_db" }
            }),
        ),
    ];

    for (case, operation, body) in &cases {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {}",
            error_code(&answer)
        );
    }

    // None of the misses wrote anything.
    let table = get_table(&router, "present_db", "events").await;
    assert_eq!(
        version_id(&table),
        version_before,
        "case after every miss: a refused mutation must not version the table, got {table}"
    );
    assert_eq!(
        stored_tables(&router, "present_db").await,
        names(&["events"]),
        "case after every miss: the catalog must be untouched"
    );
}

/// A member these mutations cannot serve faithfully is REFUSED, never accepted and
/// dropped: the transactional and versioned write paths and the view/open-table-format
/// updates answer 501 `NotImplemented` ("nobody has written it yet"), while a request
/// that names no table to update, or names two different ones, is the modelled
/// `InvalidInputException`.
#[tokio::test]
async fn unsupported_update_members_are_refused_not_silently_dropped() {
    let router = emulator();
    create_database(&router, &json!({ "Name": "guarded_db" })).await;
    create_table(
        &router,
        "guarded_db",
        &json!({ "Name": "events", "Description": "as created" }),
    )
    .await;
    let version_before = version_id(&get_table(&router, "guarded_db", "events").await);

    let unimplemented: Vec<(&str, &str, Value)> = vec![
        (
            "BatchDeleteTable with TransactionId",
            "BatchDeleteTable",
            json!({
                "DatabaseName": "guarded_db",
                "TablesToDelete": [ "events" ],
                "TransactionId": "txn-1"
            }),
        ),
        (
            "UpdateTable with TransactionId",
            "UpdateTable",
            json!({
                "DatabaseName": "guarded_db",
                "TableInput": { "Name": "events" },
                "TransactionId": "txn-1"
            }),
        ),
        (
            "UpdateTable with VersionId",
            "UpdateTable",
            json!({
                "DatabaseName": "guarded_db",
                "TableInput": { "Name": "events" },
                "VersionId": "1"
            }),
        ),
        (
            "UpdateTable with ViewUpdateAction",
            "UpdateTable",
            json!({
                "DatabaseName": "guarded_db",
                "TableInput": { "Name": "events" },
                "ViewUpdateAction": "REPLACE"
            }),
        ),
        (
            "UpdateTable with TableInput.ViewDefinition",
            "UpdateTable",
            json!({
                "DatabaseName": "guarded_db",
                "TableInput": { "Name": "events", "ViewDefinition": { "IsProtected": true } }
            }),
        ),
        (
            "UpdateTable with UpdateOpenTableFormatInput",
            "UpdateTable",
            json!({
                "DatabaseName": "guarded_db",
                "Name": "events",
                "UpdateOpenTableFormatInput": {
                    "UpdateIcebergInput": { "UpdateIcebergTableInput": { "Updates": [] } }
                }
            }),
        ),
    ];

    for (case, operation, body) in &unimplemented {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 501,
            "case {case}: an unserved member must answer 501 NotImplemented, got {status} \
             with {answer}"
        );
        assert!(
            error_code(&answer).contains("NotImplemented"),
            "case {case}: expected NotImplemented, got {}",
            error_code(&answer)
        );
    }

    let invalid: Vec<(&str, &str, Value)> = vec![
        (
            "UpdateTable with no TableInput at all",
            "UpdateTable",
            json!({ "DatabaseName": "guarded_db", "Name": "events" }),
        ),
        (
            "UpdateTable whose Name and TableInput.Name disagree",
            "UpdateTable",
            json!({
                "DatabaseName": "guarded_db",
                "Name": "events",
                "TableInput": { "Name": "renamed_events" }
            }),
        ),
    ];

    for (case, operation, body) in &invalid {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {}",
            error_code(&answer)
        );
    }

    // Every refusal left the catalog exactly as the create made it.
    let table = get_table(&router, "guarded_db", "events").await;
    assert_eq!(
        table["Description"].as_str(),
        Some("as created"),
        "case after every refusal: a refused request must leave the table untouched, got {table}"
    );
    assert_eq!(
        version_id(&table),
        version_before,
        "case after every refusal: a refused request must not version the table, got {table}"
    );
    assert_eq!(
        stored_tables(&router, "guarded_db").await,
        names(&["events"]),
        "case after every refusal: a refused batch must delete nothing"
    );
}
