// SPDX-License-Identifier: CC0-1.0

//! Which TABLE STATE the backup path acts on, and how it refuses the rest.
//!
//! The rule pinned here, over the operations that ACT on a table through that path —
//! `CreateBackup`, which catalogues a copy of it, and `UpdateContinuousBackups`, which
//! changes the continuous-backups settings held on it:
//!
//! > a table is one the backup path can act on only once its creation has FINISHED and
//! > while its deletion has not started; in any other state the endpoint has no table to
//! > act on under that name, so the call is refused with the `TableNotFoundException`
//! > both operations model — HTTP 400, `Table not found: <table name>` — and nothing is
//! > stored. A table whose configuration is being UPDATED is a table, and is acted on.
//!
//! The refusal is the terminal "the resource is not there" a client retries by creating,
//! and not an in-use conflict it would retry in place — which is the whole point of
//! matching on the error type.
//!
//! Rules are asserted over a SET of inputs: every state a table can be in when the call
//! arrives (never created, still being created, being deleted, deleted, active, being
//! updated), each driven through BOTH operations, and the table addressed in either
//! documented form (the bare name and the table's ARN). Every call goes over the wire
//! (`POST /` with `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};

/// Wait out one transient span, so whatever the clock owes a table is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// The two operations this rule spans: both act on a table through the backup path, and
/// both model `TableNotFoundException`.
const OPERATIONS: [&str; 2] = ["CreateBackup", "UpdateContinuousBackups"];

/// How many times a row whose state is TRANSIENT is driven before the suite calls the
/// race against the clock lost: the span a table spends in one is non-zero but short, so
/// a loaded machine can settle it between the preparation and the call under test.
const ATTEMPTS: usize = 5;

/// The state a table is in when the call under test arrives.
#[derive(Clone, Copy, Debug)]
enum State {
    /// No table was ever created under the name.
    NeverCreated,
    /// A `CreateTable` was accepted and its span has not elapsed.
    Creating,
    /// A `DeleteTable` was accepted and its span has not elapsed.
    Deleting,
    /// That deletion has settled, so the table is gone.
    Deleted,
    /// The table's creation has settled and nothing else is in flight.
    Active,
    /// An `UpdateTable` was accepted and its span has not elapsed.
    Updating,
}

/// Which documented form of the table member a call addresses the table in.
#[derive(Clone, Copy, Debug)]
enum Form {
    /// The bare table name.
    Name,
    /// The table's own Amazon Resource Name, as the control plane reported it.
    Arn,
}

/// The body of `operation` addressed to `table`, with the members the model requires of
/// it.
fn request(operation: &str, table: &str) -> Value {
    match operation {
        "CreateBackup" => json!({"TableName": table, "BackupName": "Attempted"}),
        _ => json!({
            "TableName": table,
            "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true},
        }),
    }
}

/// A table body with the capacity mode `state` needs: a provisioned pair for the state an
/// `UpdateTable` moves, and on-demand for every other.
fn table_body(name: &str, state: State) -> Value {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
    });
    match state {
        State::Updating => {
            body["ProvisionedThroughput"] =
                json!({"ReadCapacityUnits": 5, "WriteCapacityUnits": 5});
        }
        _ => body["BillingMode"] = json!("PAY_PER_REQUEST"),
    }
    body
}

/// Leave the table `name` in `state`, and answer the table description the control plane
/// reported for it last — the status it is in and the ARN it is addressed by — together
/// with the instant the span of a TRANSIENT state started, so a row can tell whether the
/// clock had settled that state by the time the call under test was answered.
async fn prepare(
    router: &MultiServiceRouter,
    state: State,
    name: &str,
) -> (Value, std::time::Instant) {
    if matches!(state, State::NeverCreated) {
        return (Value::Null, std::time::Instant::now());
    }
    let created = ok(router, "CreateTable", table_body(name, state)).await["TableDescription"]
        .clone();
    let started = std::time::Instant::now();
    match state {
        State::NeverCreated | State::Creating => (created, started),
        State::Active => {
            settle().await;
            (
                ok(router, "DescribeTable", json!({"TableName": name})).await["Table"].clone(),
                std::time::Instant::now(),
            )
        }
        State::Deleting => {
            settle().await;
            let deleted = ok(router, "DeleteTable", json!({"TableName": name})).await
                ["TableDescription"]
                .clone();
            (deleted, std::time::Instant::now())
        }
        State::Deleted => {
            settle().await;
            ok(router, "DeleteTable", json!({"TableName": name})).await;
            settle().await;
            (Value::Null, std::time::Instant::now())
        }
        State::Updating => {
            settle().await;
            let updated = ok(
                router,
                "UpdateTable",
                json!({
                    "TableName": name,
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": 11,
                        "WriteCapacityUnits": 11,
                    },
                }),
            )
            .await["TableDescription"]
                .clone();
            (updated, std::time::Instant::now())
        }
    }
}

/// The `TableStatus` a table in `state` is reported in, and `Value::Null` for a state in
/// which there is no table to report.
fn expected_status(state: State) -> Value {
    match state {
        State::NeverCreated | State::Deleted => Value::Null,
        State::Creating => json!("CREATING"),
        State::Deleting => json!("DELETING"),
        State::Active => json!("ACTIVE"),
        State::Updating => json!("UPDATING"),
    }
}

/// Whether the state a row named was STILL the table's when the call under test was
/// answered.
///
/// A transient state lasts exactly the span the clock owes the move out of it
/// (`crate::state::STATUS_TRANSITION`, counted from the call that started it), so a run
/// whose whole sequence finished inside that span was answered about that state, and one
/// that overran it was answered about the settled table and is driven again. It is read
/// off the clock rather than through `DescribeTable`, which waits a creation out.
fn held(state: State, span_started: std::time::Instant) -> bool {
    match state {
        State::NeverCreated | State::Deleted | State::Active => true,
        _ => {
            span_started.elapsed()
                < service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
        }
    }
}

/// Drive `operation` at a table left in `state` and addressed in `form`, on an emulator
/// of its own, and answer the response with the table's name.
///
/// The table's state is CONFIRMED to still be the row's once the call has been answered,
/// so a run that lost the race against the clock is driven again rather than asserted on.
async fn answered_in(state: State, operation: &str, form: Form) -> (u16, Value, String) {
    for attempt in 1..=ATTEMPTS {
        let router = emulator();
        let name = format!("Guarded{attempt}");
        let (described, span_started) = prepare(&router, state, &name).await;
        assert_eq!(
            described["TableStatus"].clone(),
            expected_status(state),
            "case {state:?} on {operation}: the preparation must leave the table in the \
             state the row names, got {described}"
        );
        let sent = match form {
            Form::Name => name.clone(),
            Form::Arn => described["TableArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        };
        let (status, body) = call(&router, operation, request(operation, &sent)).await;
        if held(state, span_started) {
            return (status, body, name);
        }
    }
    panic!(
        "case {state:?} on {operation} addressed by {form:?}: the state never held for the \
         call in {ATTEMPTS} attempts"
    )
}

#[tokio::test]
async fn a_table_the_backup_path_cannot_act_on_is_refused_as_absent() {
    // Rows differ in WHY the endpoint has no table to act on: one was never created, one
    // has not finished being created, one has started being deleted, and one is gone.
    let rows: [(&str, State); 4] = [
        ("a table that was never created", State::NeverCreated),
        ("a table that is still being created", State::Creating),
        ("a table that is being deleted", State::Deleting),
        ("a table that was deleted", State::Deleted),
    ];

    for (case, state) in rows {
        for operation in OPERATIONS {
            let (status, body, name) = answered_in(state, operation, Form::Name).await;
            assert_eq!(
                (status, error_code(&body), message(&body)),
                (
                    400,
                    "TableNotFoundException".to_string(),
                    format!("Table not found: {name}"),
                ),
                "case {case} on {operation}: expected HTTP 400 TableNotFoundException \
                 \"Table not found: {name}\", got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn either_documented_form_of_the_table_is_refused_the_same_way() {
    // "You can also provide the Amazon Resource Name (ARN) of the table in this
    // parameter" (`CreateBackup$TableName`, repeated on
    // `UpdateContinuousBackups$TableName`), so the refusal is about the table the value
    // resolves to and not about the form it was sent in — and the message names that
    // table, whichever form named it.
    for operation in OPERATIONS {
        for (case, form) in [
            ("the bare table name", Form::Name),
            ("the table's ARN", Form::Arn),
        ] {
            let (status, body, name) = answered_in(State::Creating, operation, form).await;
            assert_eq!(
                (status, error_code(&body), message(&body)),
                (
                    400,
                    "TableNotFoundException".to_string(),
                    format!("Table not found: {name}"),
                ),
                "case a table still being created addressed by {case} on {operation}: \
                 expected HTTP 400 TableNotFoundException naming {name:?}, got {status} \
                 with {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_refusal_on_a_table_that_is_still_being_created_stores_nothing() {
    // The refused calls are refused BEFORE anything is written, so once the table the
    // request named has settled it holds no backup and its recovery is still off — which
    // is what a client that reads it back after the refusal sees. As above, a run whose
    // span settled around the calls is driven again rather than asserted on.
    for attempt in 1..=ATTEMPTS {
        let router = emulator();
        let name = format!("Untouched{attempt}");
        let (_, span_started) = prepare(&router, State::Creating, &name).await;
        let mut answers: Vec<(&str, u16, Value)> = Vec::new();
        for operation in OPERATIONS {
            let (status, body) = call(&router, operation, request(operation, &name)).await;
            answers.push((operation, status, body));
        }
        if !held(State::Creating, span_started) {
            continue;
        }

        for (operation, status, body) in &answers {
            assert_eq!(
                *status, 400,
                "case {operation} while the table is still being created: expected HTTP \
                 400, got {status} with {body}"
            );
        }

        settle().await;
        let listed = ok(&router, "ListBackups", json!({"TableName": &name})).await;
        assert_eq!(
            listed["BackupSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
            Vec::<Value>::new(),
            "case after the refusals: the catalogue holds nothing for the table, got {listed}"
        );
        let described = ok(
            &router,
            "DescribeContinuousBackups",
            json!({"TableName": &name}),
        )
        .await;
        assert_eq!(
            described["ContinuousBackupsDescription"]["PointInTimeRecoveryDescription"]
                ["PointInTimeRecoveryStatus"],
            json!("DISABLED"),
            "case after the refusals: the enable was not applied, got {described}"
        );
        return;
    }
    panic!(
        "case a table still being created: the creating span never held for both calls in \
         {ATTEMPTS} attempts"
    )
}

#[tokio::test]
async fn a_table_the_backup_path_acts_on_is_served_and_the_result_reads_back() {
    // Rows differ in the state a table the backup path DOES act on can be in: settled,
    // and with a configuration change of its own in flight ("While it's UPDATING, you
    // can't issue another UpdateTable request" — the `UpdateTable` doc — which bars
    // another table change, not a backup).
    let rows: [(&str, State); 2] = [
        ("a settled table", State::Active),
        ("a table with a capacity change in flight", State::Updating),
    ];

    for (case, state) in rows {
        let router = emulator();
        let name = format!(
            "Served{}",
            expected_status(state).as_str().unwrap_or_default()
        );
        let (described, _) = prepare(&router, state, &name).await;
        assert_eq!(
            described["TableStatus"].clone(),
            expected_status(state),
            "case {case}: the table must be in the state the row names, got {described}"
        );

        // The backup is catalogued and the enable is applied, and both are read back:
        // neither call merely answered 200.
        let details = ok(&router, "CreateBackup", request("CreateBackup", &name)).await
            ["BackupDetails"]
            .clone();
        let arn = details["BackupArn"].as_str().unwrap_or_default().to_string();
        let recovery = ok(
            &router,
            "UpdateContinuousBackups",
            request("UpdateContinuousBackups", &name),
        )
        .await;
        assert_eq!(
            recovery["ContinuousBackupsDescription"]["PointInTimeRecoveryDescription"]
                ["PointInTimeRecoveryStatus"],
            json!("ENABLED"),
            "case {case}: the enable is applied and reported, got {recovery}"
        );

        settle().await;
        let backup = ok(&router, "DescribeBackup", json!({"BackupArn": &arn})).await;
        assert_eq!(
            (
                backup["BackupDescription"]["BackupDetails"]["BackupArn"].clone(),
                backup["BackupDescription"]["SourceTableDetails"]["TableName"].clone(),
            ),
            (json!(arn), json!(name)),
            "case {case}: the catalogued record names the table it was taken from, got {backup}"
        );
        let settled = ok(
            &router,
            "DescribeContinuousBackups",
            json!({"TableName": &name}),
        )
        .await;
        assert_eq!(
            settled["ContinuousBackupsDescription"]["PointInTimeRecoveryDescription"]
                ["PointInTimeRecoveryStatus"],
            json!("ENABLED"),
            "case {case}: the stored settings are what a later read reports, got {settled}"
        );
    }
}
