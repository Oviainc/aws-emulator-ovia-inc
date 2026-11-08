// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a Data API call gets when the statement, its parameters, its result or
//! its transaction is at fault — driven end to end over the real wire path, so what is
//! asserted is the status, the error code and the message a client sees.
//!
//! The rules under test, each stated above the test that pins it and each asserted over a
//! TABLE of inputs rather than one call:
//!
//! * an error the DATABASE raised while running a statement is the modelled
//!   `DatabaseErrorException`, carrying the engine's own diagnostic, on every path a
//!   statement reaches the engine by ([`an_error_the_database_raised_is_the_database_error_class`]);
//! * a `parameters` list that cannot be bound is a VALIDATION fault, one message per
//!   distinct fault, and the statement does not run
//!   ([`a_binding_fault_is_a_validation_fault_with_its_own_message`]);
//! * a `transactionId` the resource could have issued and no transaction is RUNNING under
//!   is HTTP 404 `TransactionNotFoundException` with the terminated-transaction sentence,
//!   on every operation that takes one, and a transaction a previous call ENDED is
//!   remembered as ended
//!   ([`a_transaction_that_is_not_running_is_refused_with_the_same_sentence`]);
//! * a statement whose RESULT the API cannot represent still RAN: the refusal arrives
//!   after the write, and the row it inserted stays
//!   ([`a_refused_result_keeps_the_write_the_statement_made`]);
//! * a batch is one unit of work: a statement of it that the database refuses refuses the
//!   whole batch and leaves nothing behind
//!   ([`a_batch_a_statement_of_it_fails_lands_nothing`]).
//!
//! These rules are about the data plane, so this suite drives the Data API alone: no
//! control plane is mounted, and the cluster, secret and identity legs of the precondition
//! chain are undecided here (`tests/all/call_preconditions.rs` is the suite that mounts
//! the services those legs ask).

use serde_json::{json, Value};

use crate::sql_data_api::{a_sibling_id, error_code, list, number, parameter, DataApi};

/// The sentence every refusal of a `transactionId` that names no running transaction
/// carries, as this service states it once
/// (`state::TRANSACTION_NOT_RUNNING_MESSAGE`).
fn not_running_message() -> String {
    service_com_amazonaws_rdsdata_rdsdataservice::state::TRANSACTION_NOT_RUNNING_MESSAGE.to_string()
}

/// PostgreSQL's own sentence for a statement that reached a transaction one of its own
/// statements had already failed in, with that condition's SQLSTATE.
fn aborted_transaction_message() -> String {
    "ERROR: current transaction is aborted, commands ignored until end of transaction block; \
     SQLState: 25P02"
        .to_string()
}

/// The table every row below writes into, and the row it starts with.
async fn given_ledger(api: &DataApi) {
    api.run(json!({
        "sql": "CREATE TABLE ledger (id INT PRIMARY KEY, note TEXT NOT NULL, span INTERVAL)",
    }))
    .await;
    api.run(json!({"sql": "INSERT INTO ledger (id, note) VALUES (1, 'first')"}))
        .await;
}

/// The ids the ledger holds now, in the order the engine returns them.
async fn ledger_ids(api: &DataApi) -> Vec<i64> {
    let selected = api
        .run(json!({"sql": "SELECT id FROM ledger ORDER BY id"}))
        .await;
    list(&selected, "records")
        .iter()
        .filter_map(|row| {
            row.as_array()
                .and_then(|row| row.first())
                .and_then(|field| field.get("longValue"))
                .and_then(Value::as_i64)
        })
        .collect()
}

/// RULE: an error the DATABASE raised while running a statement is reported as the
/// modelled `DatabaseErrorException` (HTTP 400, "There was an error in processing the SQL
/// statement") carrying the diagnostic the DATABASE THIS API ANSWERS FOR words that
/// condition with — PostgreSQL's own sentence and its SQLSTATE, never the backing engine's
/// own wording and never the deprecated `BadRequestException` — on every path a statement
/// reaches the engine by, and the data the statement would have written is not written.
///
/// The rows differ along WHICH error the database raised (a key the primary key already
/// holds, a value a unique constraint over another column already holds, a column that may
/// not be NULL, a table that does not exist, a column that does not exist, a statement it
/// cannot parse) and along the path it reached the engine by (a single statement, a batch,
/// a statement inside a transaction).
#[tokio::test]
async fn an_error_the_database_raised_is_the_database_error_class() {
    let api = DataApi::new();
    given_ledger(&api).await;
    api.run(json!({"sql": "CREATE TABLE tags (id INT PRIMARY KEY, label TEXT UNIQUE)"}))
        .await;
    api.run(json!({"sql": "INSERT INTO tags (id, label) VALUES (1, 'taken')"}))
        .await;

    // (case, the statement, the diagnostic the refusal carries: the whole of it, as the
    // database this API answers for words that condition)
    let rows: &[(&str, &str, &str)] = &[
        (
            "a key the primary key already holds",
            "INSERT INTO ledger (id, note) VALUES (1, 'again')",
            "ERROR: duplicate key value violates unique constraint \"ledger_pkey\"; \
             SQLState: 23505",
        ),
        (
            "a value a unique constraint over another column already holds",
            "INSERT INTO tags (id, label) VALUES (2, 'taken')",
            "ERROR: duplicate key value violates unique constraint \"tags_label_key\"; \
             SQLState: 23505",
        ),
        (
            "a NULL in a column that may not hold one",
            "INSERT INTO ledger (id, note) VALUES (2, NULL)",
            "ERROR: null value in column \"note\" of relation \"ledger\" violates not-null \
             constraint; SQLState: 23502",
        ),
        (
            "a table that does not exist",
            "INSERT INTO absent (id) VALUES (2)",
            "ERROR: relation \"absent\" does not exist; SQLState: 42P01",
        ),
        (
            "a column that does not exist",
            "INSERT INTO ledger (id, absent) VALUES (2, 'x')",
            "ERROR: column \"absent\" of relation \"ledger\" does not exist; SQLState: 42703",
        ),
        (
            "a column that does not exist in a query",
            "SELECT absent FROM ledger",
            "ERROR: column \"absent\" does not exist; SQLState: 42703",
        ),
        (
            "a statement the engine cannot parse",
            "INSERTO INTO ledger VALUES (2)",
            "ERROR: syntax error at or near \"INSERTO\"; Position: 1; SQLState: 42601",
        ),
    ];
    for (case, sql, diagnostic) in rows {
        // The single-statement path.
        let (status, body) = api.execute(json!({"sql": sql})).await;
        assert_eq!(status, 400, "case {case}: expected HTTP 400, got {body}");
        assert_eq!(
            error_code(&body),
            "DatabaseErrorException",
            "case {case}: expected DatabaseErrorException, got {body}"
        );
        let message = body["message"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            message, *diagnostic,
            "case {case}: expected PostgreSQL's own sentence and SQLSTATE, got {message:?}"
        );

        // The batch path, where the same error is the same class.
        let (status, batched) = api
            .batch(json!({"sql": sql, "parameterSets": [[]]}))
            .await;
        assert_eq!(
            status, 400,
            "case {case} as a batch: expected HTTP 400, got {batched}"
        );
        assert_eq!(
            error_code(&batched),
            "DatabaseErrorException",
            "case {case} as a batch: expected DatabaseErrorException, got {batched}"
        );

        // The in-transaction path: the same class, and the statement that failed ABORTS
        // the transaction it ran in — the database ignores every further command of it,
        // and a commit of it discards what it wrote and reports the rollback it
        // performed instead.
        let transaction = api.begin_transaction(None).await;
        let (status, inside) = api
            .execute(json!({"sql": sql, "transactionId": transaction.clone()}))
            .await;
        assert_eq!(
            status, 400,
            "case {case} inside a transaction: expected HTTP 400, got {inside}"
        );
        assert_eq!(
            error_code(&inside),
            "DatabaseErrorException",
            "case {case} inside a transaction: expected DatabaseErrorException, got {inside}"
        );
        let (status, after) = api
            .execute(json!({
                "sql": "INSERT INTO ledger (id, note) VALUES (100, 'after the failure')",
                "transactionId": transaction.clone(),
            }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a statement on the aborted transaction is refused, got {after}"
        );
        assert_eq!(
            error_code(&after),
            "DatabaseErrorException",
            "case {case}: expected the database's own error for an aborted transaction, got \
             {after}"
        );
        assert_eq!(
            after["message"],
            json!(aborted_transaction_message()),
            "case {case}: expected PostgreSQL's aborted-transaction sentence, got {after}"
        );
        let (status, committed) = api.commit(&transaction).await;
        assert_eq!(
            status, 200,
            "case {case}: a commit still ends an aborted transaction, got {committed}"
        );
        assert_eq!(
            committed["transactionStatus"],
            json!("Rollback complete"),
            "case {case}: a commit of an aborted transaction performs a rollback and says so, \
             got {committed}"
        );
    }

    // Not one of the refused statements wrote anything.
    let ids = ledger_ids(&api).await;
    assert_eq!(
        ids,
        vec![1],
        "case after every refusal: expected only the row the working statement wrote, got \
         {ids:?}"
    );
}

/// RULE: a `parameters` list the statement's placeholders cannot be bound from is a fault
/// of the CALL, so it is refused with HTTP 400 `ValidationException` — one message per
/// distinct fault, each naming the parameter it is about — and the statement does not run.
///
/// The rows differ along WHICH binding fault it is: a placeholder no parameter supplies, a
/// parameter name sent twice, a name carrying the ':' that introduces a placeholder in the
/// statement, and an `arrayValue`, which the `Field` union declares but `parameters` says
/// is not supported.
#[tokio::test]
async fn a_binding_fault_is_a_validation_fault_with_its_own_message() {
    let api = DataApi::new();
    given_ledger(&api).await;

    let insert = "INSERT INTO ledger (id, note) VALUES (:id, :note)";
    // (case, the parameters sent, the message AWS words that fault with)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a placeholder no parameter supplies",
            json!([parameter("id", json!({"longValue": 2}))]),
            "Cannot find parameter: note",
        ),
        (
            "a parameter name sent twice",
            json!([
                parameter("id", json!({"longValue": 2})),
                parameter("id", json!({"longValue": 3})),
                parameter("note", json!({"stringValue": "second"})),
            ]),
            "Duplicate parameter id",
        ),
        (
            "a parameter name carrying the placeholder prefix",
            json!([
                parameter(":id", json!({"longValue": 2})),
                parameter("note", json!({"stringValue": "second"})),
            ]),
            "Named parameter syntax is invalid, input: :id",
        ),
        (
            "an array parameter",
            json!([
                parameter("id", json!({"longValue": 2})),
                parameter("note", json!({"arrayValue": {"stringValues": ["a", "b"]}})),
            ]),
            "Array parameters are not supported.",
        ),
    ];
    for (case, parameters, message) in rows {
        for (path, members) in [
            ("ExecuteStatement", json!({"sql": insert, "parameters": parameters})),
            (
                "BatchExecuteStatement",
                json!({"sql": insert, "parameterSets": [parameters]}),
            ),
        ] {
            let (status, body) = if path == "ExecuteStatement" {
                api.execute(members).await
            } else {
                api.batch(members).await
            };
            assert_eq!(
                status, 400,
                "case {case} on {path}: expected HTTP 400, got {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case} on {path}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(message),
                "case {case} on {path}: expected {message:?}, got {body}"
            );
        }
    }

    // A rejected binding writes nothing, and the same statement with every placeholder
    // bound once runs.
    let ids = ledger_ids(&api).await;
    assert_eq!(
        ids,
        vec![1],
        "case after every binding fault: expected no row written, got {ids:?}"
    );

    // The LENGTH of the list is the caller's to choose, so it is capped: a request
    // carrying more values than a statement can bind is refused before the list is
    // scanned, on both operations.
    let over_cap: Vec<Value> = (0..=service_com_amazonaws_rdsdata_rdsdataservice::state::MAX_PARAMETERS_PER_STATEMENT)
        .map(|index| parameter(&format!("p{index}"), json!({"longValue": 1})))
        .collect();
    for (path, members) in [
        (
            "ExecuteStatement",
            json!({"sql": insert, "parameters": over_cap.clone()}),
        ),
        (
            "BatchExecuteStatement",
            json!({"sql": insert, "parameterSets": [over_cap.clone()]}),
        ),
    ] {
        let (status, body) = if path == "ExecuteStatement" {
            api.execute(members).await
        } else {
            api.batch(members).await
        };
        assert_eq!(
            status, 400,
            "case a parameter list past the cap on {path}: expected HTTP 400, got {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case a parameter list past the cap on {path}: expected ValidationException, got \
             {body}"
        );
    }

    let inserted = api
        .run(json!({
            "sql": insert,
            "parameters": [
                parameter("id", json!({"longValue": 2})),
                parameter("note", json!({"stringValue": "second"})),
            ],
        }))
        .await;
    assert_eq!(
        number(&inserted, "numberOfRecordsUpdated"),
        1,
        "case every placeholder bound: expected the row written, got {inserted}"
    );
}

/// RULE: a `transactionId` the resource the call names COULD have issued and no transaction
/// of that resource is RUNNING under is refused with HTTP 404
/// `TransactionNotFoundException` and the fixed sentence AWS words that class with — on
/// every operation that takes one, and however the transaction stopped running: a commit
/// ended it, a rollback ended it, a statement of its own that failed ended it, or no
/// transaction was ever opened under that id. Refusing the call changes nothing: the
/// committed row stays readable and the rolled-back write stays absent.
/// (A value that could NOT be an id of that resource is a fault of the call's own shape
/// instead — `sql_data_api` pins that class.)
#[tokio::test]
async fn a_transaction_that_is_not_running_is_refused_with_the_same_sentence() {
    let api = DataApi::new();
    given_ledger(&api).await;

    // A transaction that COMMITTED: its first commit reports the status, and the id is
    // then no longer running.
    let committed = api.begin_transaction(None).await;
    api.run(json!({
        "sql": "INSERT INTO ledger (id, note) VALUES (2, 'committed')",
        "transactionId": committed.clone(),
    }))
    .await;
    let (status, answered) = api.commit(&committed).await;
    assert_eq!(status, 200, "case the first commit: got {answered}");
    assert_eq!(
        answered["transactionStatus"],
        json!("Transaction Committed"),
        "case the first commit: expected the committed status, got {answered}"
    );

    // A transaction that ROLLED BACK, whose write never lands.
    let rolled_back = api.begin_transaction(None).await;
    api.run(json!({
        "sql": "INSERT INTO ledger (id, note) VALUES (3, 'discarded')",
        "transactionId": rolled_back.clone(),
    }))
    .await;
    let (status, answered) = api.rollback(&rolled_back).await;
    assert_eq!(status, 200, "case the first rollback: got {answered}");
    assert_eq!(
        answered["transactionStatus"],
        json!("Rollback complete"),
        "case the first rollback: expected the rollback status, got {answered}"
    );

    // A transaction a statement of its own ABORTED, and that a rollback then ENDED: an
    // aborted transaction is still there until a commit or a rollback ends it.
    let terminated = api.begin_transaction(None).await;
    let (status, failed) = api
        .execute(json!({
            "sql": "INSERT INTO ledger (id, note) VALUES (1, 'duplicate')",
            "transactionId": terminated.clone(),
        }))
        .await;
    assert_eq!(status, 400, "case the failing statement: got {failed}");
    let (status, answered) = api.rollback(&terminated).await;
    assert_eq!(
        status, 200,
        "case the rollback of an aborted transaction: got {answered}"
    );

    // An id this cluster COULD have issued and never did: the resource binding a minted id
    // carries is this cluster's, and the rest of it names no transaction that was ever
    // opened.
    let never_issued = a_sibling_id(&committed);

    // Every id that is not running, on every operation that takes one.
    let ids = [
        ("a committed transaction", committed.as_str()),
        ("a rolled-back transaction", rolled_back.as_str()),
        ("a transaction its own statement aborted and a rollback ended", terminated.as_str()),
        ("an id this cluster could have issued and never did", never_issued.as_str()),
    ];
    for (case, transaction_id) in ids {
        let calls = vec![
            (
                "ExecuteStatement",
                api.execute(json!({
                    "sql": "INSERT INTO ledger (id, note) VALUES (9, 'late')",
                    "transactionId": transaction_id,
                }))
                .await,
            ),
            (
                "BatchExecuteStatement",
                api.batch(json!({
                    "sql": "INSERT INTO ledger (id, note) VALUES (9, 'late')",
                    "parameterSets": [[]],
                    "transactionId": transaction_id,
                }))
                .await,
            ),
            (
                "CommitTransaction",
                api.commit(transaction_id).await,
            ),
            ("RollbackTransaction", api.rollback(transaction_id).await),
        ];
        for (operation, (status, body)) in calls {
            assert_eq!(
                status, 404,
                "case {case} on {operation}: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "TransactionNotFoundException",
                "case {case} on {operation}: expected TransactionNotFoundException, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(not_running_message()),
                "case {case} on {operation}: expected the terminated-transaction sentence, got \
                 {body}"
            );
        }
    }

    // None of those refusals changed the data: the committed row is there, and neither
    // the rolled-back write nor any of the refused statements is.
    let ids = ledger_ids(&api).await;
    assert_eq!(
        ids,
        vec![1, 2],
        "case after every refusal: expected the committed rows only, got {ids:?}"
    );
}

/// RULE: a statement whose RESULT the API cannot represent still RAN. The result is shaped
/// after the statement has been applied, so the call is refused with
/// `UnsupportedResultException` AND the row the statement inserted stays — where a
/// statement refused for its own shape (one the engine cannot parse, a parameter that
/// cannot be bound) never runs at all and writes nothing.
///
/// The rows differ along WHICH result the API cannot represent: a multidimensional array
/// and a data type the `Field` union has no member for, each named by a different part of
/// the statement (a constructor, a cast).
#[tokio::test]
async fn a_refused_result_keeps_the_write_the_statement_made() {
    let api = DataApi::new();
    given_ledger(&api).await;

    // (case, the statement, the id it inserts, the message the refusal carries)
    let rows: &[(&str, &str, i64, &str)] = &[
        (
            "a multidimensional array returned by an insert",
            "INSERT INTO ledger (id, note) VALUES (2, 'returned') \
             RETURNING ARRAY[[1, 2], [3, 4]] AS grid",
            2,
            "The result contains a multidimensional array.",
        ),
        (
            "an unsupported data type returned by an insert",
            "INSERT INTO ledger (id, note) VALUES (3, 'returned') \
             RETURNING '1 day'::interval AS iv",
            3,
            "The result contains the unsupported data type INTERVAL.",
        ),
        (
            "a braced literal of arrays returned by an insert",
            "INSERT INTO ledger (id, note) VALUES (4, 'returned') \
             RETURNING '{{1,2},{3,4}}'::int4[] AS grid",
            4,
            "The result contains a multidimensional array.",
        ),
        (
            "an unsupported data type of a column the table declares",
            "INSERT INTO ledger (id, note, span) VALUES (5, 'returned', '1 day') \
             RETURNING span",
            5,
            "The result contains the unsupported data type INTERVAL.",
        ),
    ];
    let mut landed = vec![1];
    for (case, sql, id, message) in rows {
        let (status, body) = api.execute(json!({"sql": sql})).await;
        assert_eq!(status, 400, "case {case}: expected HTTP 400, got {body}");
        assert_eq!(
            error_code(&body),
            "UnsupportedResultException",
            "case {case}: expected UnsupportedResultException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(message),
            "case {case}: expected {message:?}, got {body}"
        );
        landed.push(*id);
        let ids = ledger_ids(&api).await;
        assert_eq!(
            ids, landed,
            "case {case}: the refusal shaped the result AFTER the row was written, so the \
             row must be present: expected {landed:?}, got {ids:?}"
        );
    }

    // A QUERY of the rows those refusals left behind is served, so what was refused is
    // the RESULT of each write and not the rows it wrote.
    let read_back = api
        .run(json!({"sql": "SELECT count(*) AS rows FROM ledger"}))
        .await;
    assert_eq!(
        read_back["records"],
        json!([[{"longValue": landed.len() as i64}]]),
        "case reading the rows back: expected every landed row, got {read_back}"
    );

    // By contrast, a statement refused for its own shape never ran, so it wrote nothing.
    let (status, refused) = api
        .execute(json!({"sql": "INSERTO INTO ledger (id) VALUES (8)"}))
        .await;
    assert_eq!(status, 400, "case a statement the engine cannot parse: got {refused}");
    let ids = ledger_ids(&api).await;
    assert_eq!(
        ids, landed,
        "case a statement the engine cannot parse: expected nothing written, got {ids:?}"
    );
}

/// RULE: one `BatchExecuteStatement` is ONE unit of work. A statement of it the database
/// refuses refuses the whole batch — the modelled `DatabaseErrorException` — and nothing
/// the batch had already written stays, so a caller retrying it cannot land a row twice.
///
/// The rows differ along WHERE in the batch the refused set sits (first, in the middle,
/// last) and along how many sets the batch carries.
#[tokio::test]
async fn a_batch_a_statement_of_it_fails_lands_nothing() {
    let api = DataApi::new();
    given_ledger(&api).await;

    let insert = "INSERT INTO ledger (id, note) VALUES (:id, :note)";
    let set = |id: i64| {
        json!([
            parameter("id", json!({"longValue": id})),
            parameter("note", json!({"stringValue": format!("row {id}")})),
        ])
    };
    // The key 1 is already taken, so a set carrying it is the one the database refuses.
    // (case, the sets the batch carries)
    let rows: Vec<(&str, Value)> = vec![
        ("the first set of three", json!([set(1), set(20), set(21)])),
        ("the middle set of three", json!([set(20), set(1), set(21)])),
        ("the last set of three", json!([set(20), set(21), set(1)])),
        ("the only set", json!([set(1)])),
        (
            "one set among ten",
            json!([
                set(30), set(31), set(32), set(33), set(1), set(34), set(35), set(36), set(37),
                set(38),
            ]),
        ),
    ];
    for (case, sets) in rows {
        let (status, body) = api
            .batch(json!({"sql": insert, "parameterSets": sets}))
            .await;
        assert_eq!(status, 400, "case {case}: expected HTTP 400, got {body}");
        assert_eq!(
            error_code(&body),
            "DatabaseErrorException",
            "case {case}: expected DatabaseErrorException, got {body}"
        );
        let ids = ledger_ids(&api).await;
        assert_eq!(
            ids,
            vec![1],
            "case {case}: a refused batch leaves nothing behind, got {ids:?}"
        );
    }

    // A batch every statement of which runs lands every row, which is what makes the
    // refusals above the batch being refused and not the statements failing one by one.
    let (status, served) = api
        .batch(json!({"sql": insert, "parameterSets": [set(40), set(41)]}))
        .await;
    assert_eq!(status, 200, "case a batch that runs: got {served}");
    assert_eq!(
        list(&served, "updateResults").len(),
        2,
        "case a batch that runs: expected one result per set, got {served}"
    );
    let ids = ledger_ids(&api).await;
    assert_eq!(
        ids,
        vec![1, 40, 41],
        "case a batch that runs: expected every row landed, got {ids:?}"
    );
}
