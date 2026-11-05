// SPDX-License-Identifier: CC0-1.0

//! `CreateBackup`: which table a backup is taken OF, and what each call catalogues.
//!
//! The rules pinned here:
//!
//! 1. the member that names the source table accepts EITHER documented form — the bare
//!    table name or the table's Amazon Resource Name ("You can also provide the Amazon
//!    Resource Name (ARN) of the table in this parameter" — `CreateBackup$TableName`) —
//!    and both forms resolve to the SAME stored table, so the record the readers report
//!    carries that table's name, ARN and id whichever form the request was written in;
//! 2. "There is no limit to the number of on-demand backups that can be taken" (the
//!    `CreateBackup` doc), so every call catalogues a record OF ITS OWN under its own
//!    `BackupArn` — repeats under one `BackupName` included — and each is readable;
//! 3. a source table a request does not name is refused by WHY it does not name one: a
//!    value outside the documented table-name form (3..=255 characters of
//!    `[a-zA-Z0-9_.-]`) names no table whose existence could be looked up, so it is a
//!    `ValidationException` stating the member and the constraint; a well-formed name,
//!    in either form, that this endpoint holds no table under is the operation's own
//!    `TableNotFoundException`.
//!
//! Each rule is asserted over a SET of inputs — the source table, the reference form, the
//! repeat count, the way a name fails — and the length bounds on both published edges and
//! one character past each. Every call goes over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the router the emulator's
//! front doors use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};

/// Wait out one transient span, so whatever the clock owes a table or a backup is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// Create one table and wait out its `CREATING` span, so it is a settled table to back up.
async fn active_table(router: &MultiServiceRouter, name: &str, mut body: Value) {
    body["TableName"] = json!(name);
    ok(router, "CreateTable", body).await;
    settle().await;
}

/// An on-demand table with a simple key, for a case that is not about the source table's
/// own shape.
fn simple_table_body() -> Value {
    json!({
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    })
}

/// The `TableArn` and `TableId` a table reports for itself — what a backup of it records.
async fn table_identity(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let described = ok(router, "DescribeTable", json!({"TableName": name})).await;
    (
        described["Table"]["TableArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        described["Table"]["TableId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// Take one backup and return the `BackupDetails` the create reported.
async fn take_backup(router: &MultiServiceRouter, reference: &str, name: &str) -> Value {
    ok(
        router,
        "CreateBackup",
        json!({"TableName": reference, "BackupName": name}),
    )
    .await["BackupDetails"]
        .clone()
}

/// The `BackupArn` of a `BackupDetails`.
fn arn_of(details: &Value) -> String {
    details["BackupArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn either_documented_form_of_the_source_table_records_the_same_table() {
    let router = two_source_tables().await;

    // Rows differ in the SOURCE TABLE — its capacity mode and its key schema — and each
    // is backed up twice: once naming the table, once naming it by its own ARN.
    for (table_case, table) in [
        ("an on-demand table with a simple key", "Plain"),
        ("a provisioned table with a composite key", "Composite"),
    ] {
        let (table_arn, table_id) = table_identity(&router, table).await;
        let mut minted: Vec<String> = Vec::new();

        for (form_case, reference) in [
            ("the bare table name", table.to_string()),
            ("the table's ARN", table_arn.clone()),
        ] {
            let case = format!("{table_case} addressed by {form_case}");
            let details = take_backup(&router, &reference, "Addressed").await;
            let arn = arn_of(&details);
            assert!(
                arn.starts_with(&format!("{table_arn}/backup/")),
                "case {case:?}: the minted ARN is a backup of the table named, expected it \
                 under {table_arn:?}/backup/, got {details}"
            );
            assert_eq!(
                (
                    details["BackupName"].clone(),
                    details["BackupStatus"].clone(),
                    details["BackupType"].clone(),
                ),
                (json!("Addressed"), json!("CREATING"), json!("USER")),
                "case {case:?}: the create reports the record it wrote, got {details}"
            );
            minted.push(arn.clone());

            // The record is read back through `DescribeBackup`: the source table it
            // reports is the one the request named, in whichever form.
            settle().await;
            let described = ok(&router, "DescribeBackup", json!({"BackupArn": arn})).await;
            assert_eq!(
                (
                    described["BackupDescription"]["BackupDetails"]["BackupArn"].clone(),
                    described["BackupDescription"]["BackupDetails"]["BackupStatus"].clone(),
                    described["BackupDescription"]["SourceTableDetails"]["TableName"].clone(),
                    described["BackupDescription"]["SourceTableDetails"]["TableArn"].clone(),
                    described["BackupDescription"]["SourceTableDetails"]["TableId"].clone(),
                ),
                (
                    json!(arn),
                    json!("AVAILABLE"),
                    json!(table),
                    json!(table_arn),
                    json!(table_id),
                ),
                "case {case:?}: the record names the stored table, got {described}"
            );
        }

        assert_eq!(
            minted.iter().collect::<std::collections::BTreeSet<_>>().len(),
            minted.len(),
            "case {table_case}: the two forms catalogue two records, got {minted:?}"
        );
    }
}

/// The emulator the case above drives, holding the two source tables its rows name.
async fn two_source_tables() -> MultiServiceRouter {
    let router = emulator();
    active_table(&router, "Plain", simple_table_body()).await;
    active_table(
        &router,
        "Composite",
        json!({
            "KeySchema": [
                {"AttributeName": "Part", "KeyType": "HASH"},
                {"AttributeName": "Sort", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "Part", "AttributeType": "S"},
                {"AttributeName": "Sort", "AttributeType": "N"},
            ],
            "ProvisionedThroughput": {"ReadCapacityUnits": 4, "WriteCapacityUnits": 7},
        }),
    )
    .await;
    router
}

#[tokio::test]
async fn every_call_catalogues_a_record_of_its_own() {
    let router = emulator();
    active_table(&router, "Repeatedly", simple_table_body()).await;
    let (table_arn, _) = table_identity(&router, "Repeatedly").await;

    // Rows differ in the REQUEST NAME the repeats carry: the same name every time, and a
    // name of its own each time. Neither is a limit on how many records the catalogue
    // holds, so each call adds one.
    for (case, names) in [
        ("four calls under one backup name", vec!["Nightly"; 4]),
        (
            "four calls under names of their own",
            vec!["First", "Second", "Third", "Fourth"],
        ),
    ] {
        let listed_before = listed_arns(&router, "Repeatedly").await.len();
        let mut minted: Vec<String> = Vec::new();
        for name in &names {
            minted.push(arn_of(&take_backup(&router, "Repeatedly", name).await));
        }
        assert_eq!(
            minted.iter().collect::<std::collections::BTreeSet<_>>().len(),
            names.len(),
            "case {case:?}: each call mints a record of its own, got {minted:?}"
        );

        settle().await;
        for (name, arn) in names.iter().zip(&minted) {
            let described = ok(&router, "DescribeBackup", json!({"BackupArn": arn})).await;
            assert_eq!(
                (
                    described["BackupDescription"]["BackupDetails"]["BackupName"].clone(),
                    described["BackupDescription"]["SourceTableDetails"]["TableArn"].clone(),
                ),
                (json!(name), json!(table_arn)),
                "case {case:?}: the record under {arn:?} is readable and names its table, got \
                 {described}"
            );
        }

        let listed = listed_arns(&router, "Repeatedly").await;
        assert_eq!(
            listed.len(),
            listed_before + names.len(),
            "case {case:?}: the catalogue grew by one record per call, got {listed:?}"
        );
        for arn in &minted {
            assert!(
                listed.contains(arn),
                "case {case:?}: the list reports the record under {arn:?}, got {listed:?}"
            );
        }
    }
}

/// The `BackupArn`s the catalogue holds for `table`, in the order the list reports them.
async fn listed_arns(router: &MultiServiceRouter, table: &str) -> Vec<String> {
    ok(router, "ListBackups", json!({"TableName": table})).await["BackupSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["BackupArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_source_table_the_request_does_not_name_is_refused() {
    let router = emulator();
    // The documented table-name bounds, from `CreateTable$TableName` (3..=255 characters
    // of `[a-zA-Z0-9_.-]`): a name AT each edge is a table that can be created and backed
    // up, so the refusals below are the bound rather than an off-by-one.
    let shortest = "abc".to_string();
    let longest = "T".repeat(255);
    for name in [shortest.clone(), longest.clone()] {
        active_table(&router, &name, simple_table_body()).await;
        let details = take_backup(&router, &name, "AtTheBound").await;
        let (table_arn, _) = table_identity(&router, &name).await;
        assert!(
            arn_of(&details).starts_with(&format!("{table_arn}/backup/")),
            "case a name of {} character(s), at a published bound: the table is backed up, got \
             {details}",
            name.chars().count()
        );
    }

    // Rows differ in HOW the value fails to name a table of this endpoint. One outside the
    // documented form names no table to look up, so it is stated as the constraint it
    // broke; a well-formed name — bare or inside a table ARN — that this endpoint holds no
    // table under is the miss this operation models.
    let absent_arn = table_identity(&router, &shortest)
        .await
        .0
        .replace(&format!("table/{shortest}"), "table/Vanished");
    let cases: [(&str, String, &str, String); 6] = [
        (
            "one character below the documented minimum length",
            "ab".to_string(),
            "ValidationException",
            "1 validation error detected: Value 'ab' at 'tableName' failed to satisfy \
             constraint: Member must have length greater than or equal to 3"
                .to_string(),
        ),
        (
            "one character past the documented maximum length",
            "T".repeat(256),
            "ValidationException",
            format!(
                "1 validation error detected: Value '{}' at 'tableName' failed to satisfy \
                 constraint: Member must have length less than or equal to 255",
                "T".repeat(256)
            ),
        ),
        (
            "a character the documented pattern excludes",
            "bad name!".to_string(),
            "ValidationException",
            "1 validation error detected: Value 'bad name!' at 'tableName' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
        (
            "an ARN carrying no table resource at all",
            "arn:aws:dynamodb:us-east-1:000000000000:backup/Nope".to_string(),
            "ValidationException",
            "1 validation error detected: Invalid resource type in \
             'arn:aws:dynamodb:us-east-1:000000000000:backup/Nope'"
                .to_string(),
        ),
        (
            "a well-formed name this endpoint holds no table under",
            "Vanished".to_string(),
            "TableNotFoundException",
            "Table not found: Vanished".to_string(),
        ),
        (
            "that same table named by ARN",
            absent_arn,
            "TableNotFoundException",
            "Table not found: Vanished".to_string(),
        ),
    ];

    for (case, reference, expected_code, expected_message) in cases {
        let (status, body) = call(
            &router,
            "CreateBackup",
            json!({"TableName": reference, "BackupName": "Attempted"}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (400, expected_code.to_string(), expected_message.clone()),
            "case {case:?}: expected HTTP 400 {expected_code} {expected_message:?}, got {status} \
             with {body}"
        );
    }
}
