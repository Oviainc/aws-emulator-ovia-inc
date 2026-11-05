// SPDX-License-Identifier: CC0-1.0

//! How the PartiQL surface WORDS a refusal: `ExecuteStatement`,
//! `BatchExecuteStatement` and `ExecuteTransaction`.
//!
//! One rule, asserted over the classes it generalises over: a statement DynamoDB will
//! not run is refused with the fixed sentence for that defect class, carrying only the
//! parts the request supplied — the parser's phrase for the position a parse stopped
//! at, the function a call names, the position the statement spells a run of text at,
//! and, inside a transaction, the slot of `TransactStatements` the statement was sent
//! in. The rows below span the classes rather than one recorded statement, and every
//! position a row expects is COMPUTED from the statement text, so a row asserts the
//! rule rather than a transcribed number.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row addresses: a partition and a sort key, plus one global
/// secondary index for the rows whose source is an index.
const TABLE: &str = "items";

/// That table's index.
const INDEX: &str = "by-owner";

/// The sentence a parse defect is stated with, over the parser's phrase for the
/// position the parse stopped at.
fn parse_failure(reason: &str) -> String {
    format!("Statement wasn't well formed, can't be processed: {reason}")
}

/// Where `run` sits in `statement`, as a refusal reports a position:
/// `<line>:<column>:<length>`, counted in characters from 1. Every positioned row
/// below derives what it expects from the statement it sends, so the expectation is
/// the rule and not one transcribed figure.
fn position_of(statement: &str, run: &str) -> String {
    let offset = statement
        .find(run)
        .unwrap_or_else(|| panic!("the run {run:?} is not in the statement {statement:?}"));
    let line = 1 + statement[..offset].matches('\n').count();
    let line_start = statement[..offset]
        .rfind('\n')
        .map_or(0, |break_at| break_at + 1);
    let column = 1 + statement[line_start..offset].chars().count();
    format!("{line}:{column}:{}", run.chars().count())
}

/// An emulator holding one table with one index, with three items in it.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    // The index-qualified rows need a table that declares an index, which the shared
    // helper's single-key table does not: this one is created on its own.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "keyed",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "N"},
                {"AttributeName": "owner", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [{
                "IndexName": INDEX,
                "KeySchema": [{"AttributeName": "owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    std::thread::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(200),
    );
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": "a"}, "n": {"N": "1"}}}),
    )
    .await;
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": "keyed",
            "Item": {"pk": {"S": "a"}, "sk": {"N": "1"}, "owner": {"S": "x"}, "note": {"S": "one"}},
        }),
    )
    .await;
    router
}

/// Run one statement that must be refused, and report the message it was refused with.
async fn refused(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, answered) = call(router, "ExecuteStatement", body).await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} with {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ValidationException",
        "case {case}: expected ValidationException, got {answered}"
    );
    message(&answered)
}

/// One row: the case, the statement, the `Parameters` it carries, and the whole
/// sentence the refusal must state.
struct Row {
    case: &'static str,
    statement: String,
    parameters: Value,
    expected: String,
}

/// Every parse-defect class, each with the parser's own phrase for the position the
/// parse stopped at.
fn parse_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a token after a complete statement",
            statement: format!("SELECT * FROM {TABLE} WHERE pk = 'a' junk"),
            parameters: Value::Null,
            expected: parse_failure("Unexpected token after expression"),
        },
        Row {
            case: "a misspelled verb leaves the rest unaccounted for",
            statement: format!("INSRT INTO \"{TABLE}\" VALUE {{'pk':'a'}}"),
            parameters: Value::Null,
            expected: parse_failure("Unexpected token after expression"),
        },
        Row {
            case: "the text reads as an expression and nothing more",
            statement: TABLE.to_string(),
            parameters: Value::Null,
            expected: parse_failure("Expected data manipulation"),
        },
        Row {
            case: "a single-quoted table name is a literal, not a name",
            statement: format!("SELECT * FROM '{TABLE}'"),
            parameters: Value::Null,
            expected: parse_failure("Expected identifier for simple path"),
        },
        Row {
            case: "a single-quoted attribute name in a projection",
            statement: format!("SELECT 'pk' FROM {TABLE}"),
            parameters: Value::Null,
            expected: parse_failure("Expected identifier for simple path"),
        },
        Row {
            case: "a keyword the grammar requires is missing",
            statement: format!("SELECT * {TABLE}"),
            parameters: Value::Null,
            expected: parse_failure("Expected keyword"),
        },
        Row {
            // A function the dialect DOES define, standing where only a name may stand:
            // the statement spells a call there, which is no name — not a call to
            // something undefined.
            case: "a defined function in a projection",
            statement: format!("SELECT attribute_exists(pk) FROM {TABLE}"),
            parameters: Value::Null,
            expected: parse_failure("Expected identifier for simple path"),
        },
        Row {
            case: "a defined function on the left of a SET clause",
            statement: format!("UPDATE {TABLE} SET size(n) = 2 WHERE pk = 'a'"),
            parameters: Value::Null,
            expected: parse_failure("Expected identifier for simple path"),
        },
        Row {
            case: "a malformed numeric literal",
            statement: format!("INSERT INTO {TABLE} VALUE {{'pk': 'z', 'n': 1.2.3}}"),
            parameters: Value::Null,
            expected: parse_failure("Unexpected term"),
        },
        Row {
            case: "a clause that stops before its predicate",
            statement: format!("SELECT * FROM {TABLE} WHERE"),
            parameters: Value::Null,
            expected: parse_failure("Unexpected term"),
        },
    ]
}

/// A call to a function the dialect does not define, in each position a call can sit
/// in: the name and the position are read off the statement.
fn unrecognized_function_rows() -> Vec<Row> {
    // The rows differ in WHERE the call sits and in the name's length, which is what
    // the reported position is made of.
    [
        (
            "an unknown function in a predicate",
            format!("SELECT * FROM \"{TABLE}\" WHERE made_up_function(pk)"),
            "made_up_function",
        ),
        (
            "an unknown function in a projection",
            format!("SELECT other_fn(pk) FROM {TABLE}"),
            "other_fn",
        ),
        (
            "an unknown function on the right of a comparison",
            format!("SELECT * FROM {TABLE} WHERE n = magnitude(n)"),
            "magnitude",
        ),
        (
            "an unknown function in a SET clause",
            format!("UPDATE {TABLE} SET n = twice(n) WHERE pk = 'a'"),
            "twice",
        ),
    ]
    .into_iter()
    .map(|(case, statement, name)| Row {
        case,
        expected: format!(
            "Unrecognized function: {name} at {}",
            position_of(&statement, name)
        ),
        statement,
        parameters: Value::Null,
    })
    .collect()
}

/// A call given something other than the document path it reads, and a call given the
/// wrong number of operands: both name what was called.
fn operand_rows() -> Vec<Row> {
    let requires_path = |function: &str| {
        format!("Operator or function requires a document path; operator or function: {function}")
    };
    let operand_count = |function: &str| {
        format!(
            "Incorrect number of operands for operator or function; operator or function: \
             {function}"
        )
    };
    // The functions are the ones this dialect defines over a document path
    // (`smithy_query(query="doc",shape="ExecuteStatement")` documents the PartiQL
    // surface; the functions are the Developer Guide's `attribute_exists`,
    // `attribute_not_exists`, `attribute_type`, `begins_with`, `contains` and `size`).
    vec![
        Row {
            case: "attribute_exists given a string literal",
            statement: format!("SELECT * FROM {TABLE} WHERE attribute_exists('pk')"),
            parameters: Value::Null,
            expected: requires_path("attribute_exists"),
        },
        Row {
            case: "attribute_not_exists given a number literal",
            statement: format!("SELECT * FROM {TABLE} WHERE attribute_not_exists(1)"),
            parameters: Value::Null,
            expected: requires_path("attribute_not_exists"),
        },
        Row {
            case: "begins_with given a literal first operand",
            statement: format!("SELECT * FROM {TABLE} WHERE begins_with('a', 'a')"),
            parameters: Value::Null,
            expected: requires_path("begins_with"),
        },
        Row {
            case: "size given a literal",
            statement: format!("SELECT * FROM {TABLE} WHERE size('abc') > 1"),
            parameters: Value::Null,
            expected: requires_path("size"),
        },
        Row {
            case: "attribute_exists given two operands",
            statement: format!("SELECT * FROM {TABLE} WHERE attribute_exists(pk, n)"),
            parameters: Value::Null,
            expected: operand_count("attribute_exists"),
        },
        Row {
            case: "begins_with given one operand",
            statement: format!("SELECT * FROM {TABLE} WHERE begins_with(pk)"),
            parameters: Value::Null,
            expected: operand_count("begins_with"),
        },
        Row {
            case: "contains given three operands",
            statement: format!("SELECT * FROM {TABLE} WHERE contains(pk, 'a', 'b')"),
            parameters: Value::Null,
            expected: operand_count("contains"),
        },
        Row {
            case: "size given two operands",
            statement: format!("SELECT * FROM {TABLE} WHERE size(pk, n) > 1"),
            parameters: Value::Null,
            expected: operand_count("size"),
        },
    ]
}

/// A `FROM` path with more components than a table and an index of it, and a write
/// addressed through an index qualifier.
///
/// The arity sentence is about the `FROM` CLAUSE, so it is what a statement that HAS one
/// earns — a `SELECT` or a `DELETE`. An `UPDATE` and an `INSERT` name their target
/// directly, with no `FROM` clause to state a rule about, so any qualifier on that
/// target addresses an index and is refused as an operation asked of an index.
fn source_rows() -> Vec<Row> {
    const PATH_ARITY: &str = "A path may contain at most 2 components in the FROM clause";
    const ON_AN_INDEX: &str = "This operation is not supported on an index";
    let mut rows = vec![
        Row {
            case: "a three-component bare FROM path",
            statement: format!("SELECT * FROM {TABLE}.a.b"),
            parameters: Value::Null,
            expected: PATH_ARITY.to_string(),
        },
        Row {
            case: "a four-component quoted FROM path",
            statement: format!("SELECT * FROM \"keyed\".\"{INDEX}\".\"x\".\"y\""),
            parameters: Value::Null,
            expected: PATH_ARITY.to_string(),
        },
        Row {
            case: "a three-component FROM path in a DELETE",
            statement: format!("DELETE FROM \"keyed\".\"{INDEX}\".\"extra\" WHERE pk = 'a'"),
            parameters: Value::Null,
            expected: PATH_ARITY.to_string(),
        },
    ];
    // Only a `DELETE` states its target through a `FROM` clause, which is what the
    // positioned sentence is about; an `UPDATE` and an `INSERT` name theirs directly.
    rows.push(Row {
        case: "a DELETE through an index qualifier",
        statement: format!("DELETE FROM \"keyed\".\"{INDEX}\" WHERE pk = 'a' AND sk = 1"),
        parameters: Value::Null,
        expected: format!(
            "FROM clause may only contain a single table name in data manipulation statements \
             at {}",
            position_of(
                &format!("DELETE FROM \"keyed\".\"{INDEX}\" WHERE pk = 'a' AND sk = 1"),
                &format!("\"keyed\".\"{INDEX}\"")
            )
        ),
    });
    rows.push(Row {
        case: "a DELETE through a bare index qualifier",
        statement: "DELETE FROM keyed.anyindex WHERE pk = 'a' AND sk = 1".to_string(),
        parameters: Value::Null,
        expected: format!(
            "FROM clause may only contain a single table name in data manipulation statements \
             at {}",
            position_of(
                "DELETE FROM keyed.anyindex WHERE pk = 'a' AND sk = 1",
                "keyed.anyindex"
            )
        ),
    });
    for (case, statement) in [
        (
            "an UPDATE through an index qualifier",
            format!("UPDATE \"keyed\".\"{INDEX}\" SET note = 'two' WHERE pk = 'a' AND sk = 1"),
        ),
        (
            "an INSERT through an index qualifier",
            format!("INSERT INTO \"keyed\".\"{INDEX}\" VALUE {{'pk':'b','sk':2}}"),
        ),
        (
            "an UPDATE through a longer qualifier still addresses an index",
            format!("UPDATE keyed.{INDEX}.extra SET note = 'two' WHERE pk = 'a'"),
        ),
        (
            "an INSERT through a longer qualifier still addresses an index",
            format!("INSERT INTO {TABLE}.a.b VALUE {{'pk':'b'}}"),
        ),
    ] {
        rows.push(Row {
            case,
            statement,
            parameters: Value::Null,
            expected: ON_AN_INDEX.to_string(),
        });
    }
    rows
}

/// The refusals a statement that parses earns: a `Parameters` list that does not
/// correspond to the statement's markers, a number past the stored domain, and a
/// singleton write whose `WHERE` clause pins no whole key.
fn semantic_rows() -> Vec<Row> {
    const PARAMETERS: &str = "Number of parameters in request and statement don't match.";
    const KEY_EQUALITY: &str =
        "Where clause does not contain a mandatory equality on all key attributes";
    // 39 digits: one past the documented 38 significant digits an N carries.
    const THIRTY_NINE_DIGITS: &str = "123456789012345678901234567890123456789";
    vec![
        Row {
            case: "more parameters than markers",
            statement: format!("SELECT * FROM {TABLE} WHERE pk = ?"),
            parameters: json!([{"S": "a"}, {"S": "b"}]),
            expected: PARAMETERS.to_string(),
        },
        Row {
            case: "fewer parameters than markers",
            statement: format!("SELECT * FROM {TABLE} WHERE pk = ? AND n = ?"),
            parameters: json!([{"S": "a"}]),
            expected: PARAMETERS.to_string(),
        },
        Row {
            case: "no parameters at all for one marker",
            statement: format!("DELETE FROM {TABLE} WHERE pk = ?"),
            parameters: Value::Null,
            expected: PARAMETERS.to_string(),
        },
        Row {
            case: "a number literal past the stored precision, in a VALUE",
            statement: format!("INSERT INTO {TABLE} VALUE {{'pk': 'z', 'n': {THIRTY_NINE_DIGITS}}}"),
            parameters: Value::Null,
            expected: "Attempting to store more than 38 significant digits in a Number"
                .to_string(),
        },
        Row {
            case: "a number literal past the stored precision, in a SET clause",
            statement: format!("UPDATE {TABLE} SET n = {THIRTY_NINE_DIGITS} WHERE pk = 'a'"),
            parameters: Value::Null,
            expected: "Attempting to store more than 38 significant digits in a Number"
                .to_string(),
        },
        Row {
            case: "an UPDATE that states no WHERE clause",
            statement: format!("UPDATE {TABLE} SET n = 2"),
            parameters: Value::Null,
            expected: KEY_EQUALITY.to_string(),
        },
        Row {
            case: "a DELETE that pins only part of the key",
            statement: "DELETE FROM keyed WHERE pk = 'a'".to_string(),
            parameters: Value::Null,
            expected: KEY_EQUALITY.to_string(),
        },
        Row {
            case: "an UPDATE whose sort key is pinned by an inequality",
            statement: "UPDATE keyed SET note = 'two' WHERE pk = 'a' AND sk > 0".to_string(),
            parameters: Value::Null,
            expected: KEY_EQUALITY.to_string(),
        },
    ]
}

/// Every row of every class, run through `ExecuteStatement`.
fn every_row() -> Vec<Row> {
    let mut rows = parse_rows();
    rows.extend(unrecognized_function_rows());
    rows.extend(operand_rows());
    rows.extend(source_rows());
    rows.extend(semantic_rows());
    rows
}

#[tokio::test]
async fn each_statement_defect_class_states_its_own_sentence() {
    let router = seeded().await;

    for row in every_row() {
        let mut body = json!({"Statement": row.statement});
        if !row.parameters.is_null() {
            body["Parameters"] = row.parameters.clone();
        }
        let stated = refused(&router, row.case, body).await;
        assert_eq!(
            stated, row.expected,
            "case {}: statement {:?} expected the message {:?}, got {stated:?}",
            row.case, row.statement, row.expected
        );
    }

    // None of those refusals wrote anything: the seeded item stands as it was.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "a"}}}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"pk": {"S": "a"}, "n": {"N": "1"}}),
        "case a refused statement leaves the store alone: got {read}"
    );
}

#[tokio::test]
async fn a_batch_slot_states_the_same_sentence_its_own_statement_earns() {
    let router = seeded().await;

    // Three malformed statements, one per class, each sent beside a well-formed INSERT
    // that must still run: a batch reports a statement's defect in ITS OWN slot.
    let rows = [
        (
            "a misspelled verb",
            format!("INSRT INTO \"{TABLE}\" VALUE {{'pk':'q'}}"),
            parse_failure("Unexpected token after expression"),
        ),
        (
            "an unknown function",
            format!("SELECT * FROM {TABLE} WHERE made_up_function(pk) AND pk = 'a'"),
            format!(
                "Unrecognized function: made_up_function at {}",
                position_of(
                    &format!("SELECT * FROM {TABLE} WHERE made_up_function(pk) AND pk = 'a'"),
                    "made_up_function"
                )
            ),
        ),
        (
            "a write addressed through an index qualifier",
            format!("INSERT INTO {TABLE}.a.b VALUE {{'pk':'q'}}"),
            "This operation is not supported on an index".to_string(),
        ),
    ];

    for (at, (case, statement, expected)) in rows.iter().enumerate() {
        let key = format!("sibling-{at}");
        let (status, answered) = call(
            &router,
            "BatchExecuteStatement",
            json!({"Statements": [
                {"Statement": statement},
                {"Statement": format!("INSERT INTO {TABLE} VALUE {{'pk': '{key}'}}")},
            ]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a batch answers 200 whatever its slots say, got {status} with {answered}"
        );
        let slot = &answered["Responses"][0];
        assert_eq!(
            slot["Error"]["Code"].as_str(),
            Some("ValidationError"),
            "case {case}: expected the per-slot ValidationError code, got {slot}"
        );
        assert_eq!(
            slot["Error"]["Message"].as_str(),
            Some(expected.as_str()),
            "case {case}: expected the slot message {expected:?}, got {slot}"
        );
        // The slot beside it ran, and its write is readable.
        let sibling = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE, "Key": {"pk": {"S": key}}}),
        )
        .await;
        assert_eq!(
            sibling["Item"],
            json!({"pk": {"S": key}}),
            "case {case}: the well-formed sibling statement ran, got {sibling}"
        );
    }
}

#[tokio::test]
async fn every_batch_slot_echoes_the_table_its_statement_addressed() {
    let router = seeded().await;

    // A write that reports an item, a write that reports none, a read, and a statement
    // that fails on its own merits: each slot names the table it addressed, so a batch
    // spread over two tables can be attributed slot by slot.
    let (status, answered) = call(
        &router,
        "BatchExecuteStatement",
        json!({"Statements": [
            {"Statement": "UPDATE keyed SET note = 'two' WHERE pk = 'a' AND sk = 1 RETURNING ALL OLD *"},
            {"Statement": format!("UPDATE {TABLE} SET n = 2 WHERE pk = 'a'")},
            {"Statement": "UPDATE keyed SET note = 'three' WHERE pk = 'zz' AND sk = 9"},
        ]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a batch over two tables: expected HTTP 200, got {status} with {answered}"
    );

    for (at, (case, table, item, code)) in [
        (
            "an UPDATE reporting the item it replaced",
            "keyed",
            Some(json!({
                "pk": {"S": "a"},
                "sk": {"N": "1"},
                "owner": {"S": "x"},
                "note": {"S": "one"},
            })),
            None,
        ),
        ("an UPDATE that reports no item", TABLE, None, None),
        (
            "a statement whose condition was not met",
            "keyed",
            None,
            Some("ConditionalCheckFailed"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let slot = &answered["Responses"][at];
        assert_eq!(
            slot["TableName"].as_str(),
            Some(table),
            "case {case}: expected TableName {table:?}, got {slot}"
        );
        assert_eq!(
            slot.get("Item").cloned(),
            item,
            "case {case}: expected Item {item:?}, got {slot}"
        );
        // Which KIND of slot it is: one that ran carries no error, and one that failed
        // on its own merits carries its code beside the table it addressed.
        assert_eq!(
            slot["Error"]["Code"].as_str(),
            code,
            "case {case}: expected Error.Code {code:?}, got {slot}"
        );
    }

    // The slots that ran wrote what they said they did.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "keyed", "Key": {"pk": {"S": "a"}, "sk": {"N": "1"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["note"],
        json!({"S": "two"}),
        "case the reported UPDATE landed: got {read}"
    );
}

#[tokio::test]
async fn a_transaction_states_the_slot_a_statement_was_sent_in() {
    let router = seeded().await;

    // The same defects, each sent at a different position of `TransactStatements`: the
    // sentence is the statement's own, behind the slot the request stated it at.
    let defects = [
        (
            "a misspelled verb",
            format!("INSRT INTO \"{TABLE}\" VALUE {{'pk':'q'}}"),
            parse_failure("Unexpected token after expression"),
        ),
        (
            "a single-quoted table name",
            format!("SELECT * FROM '{TABLE}' WHERE pk = 'a'"),
            parse_failure("Expected identifier for simple path"),
        ),
        (
            "an unknown function",
            format!("UPDATE {TABLE} SET n = twice(n) WHERE pk = 'a'"),
            format!(
                "Unrecognized function: twice at {}",
                position_of(
                    &format!("UPDATE {TABLE} SET n = twice(n) WHERE pk = 'a'"),
                    "twice"
                )
            ),
        ),
    ];
    let well_formed = format!("UPDATE {TABLE} SET n = 2 WHERE pk = 'a'");

    for (slot, (case, statement, sentence)) in defects.iter().enumerate() {
        // The malformed statement is sent at its own index, after that many well-formed
        // ones, so the index the message states is a fact of the request.
        let mut statements: Vec<Value> = (0..slot)
            .map(|_| json!({"Statement": well_formed}))
            .collect();
        statements.push(json!({"Statement": statement}));
        let (status, answered) = call(
            &router,
            "ExecuteTransaction",
            json!({"TransactStatements": statements}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case} at slot {slot}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case} at slot {slot}: expected ValidationException, got {answered}"
        );
        let expected = format!("Validation failed in TransactStatements[{slot}]: {sentence}");
        assert_eq!(
            message(&answered),
            expected,
            "case {case} at slot {slot}: expected the message {expected:?}, got {answered}"
        );
    }

    // A refused transaction applied nothing, so the well-formed statements beside the
    // malformed one did not write either.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "a"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["n"],
        json!({"N": "1"}),
        "case a refused transaction writes nothing: got {read}"
    );
}

#[tokio::test]
async fn a_transaction_read_that_addresses_no_one_item_states_that_rule() {
    let router = seeded().await;

    // The ways a read statement can fail to address ONE item of the table: no clause at
    // all, part of the key, a key pinned by something other than an equality, and a
    // source that is an index (which is not keyed by the table's primary key).
    let rows = [
        ("no WHERE clause at all", "SELECT * FROM keyed"),
        ("only the partition key", "SELECT * FROM keyed WHERE pk = 'a'"),
        (
            "the sort key pinned by an inequality",
            "SELECT * FROM keyed WHERE pk = 'a' AND sk > 0",
        ),
        (
            "a source that is an index",
            "SELECT * FROM \"keyed\".\"by-owner\" WHERE owner = 'x'",
        ),
    ];
    let expected = "Select statements within ExecuteTransaction must specify the primary key in \
                    the where clause.";

    for (case, statement) in rows {
        let (status, answered) = call(
            &router,
            "ExecuteTransaction",
            json!({"TransactStatements": [{"Statement": statement}]}),
        )
        .await;
        // "top-level validation": the call is refused as a whole rather than reported as
        // a cancellation of a transaction that was attempted.
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case}: expected ValidationException, got {answered}"
        );
        assert_eq!(
            message(&answered),
            expected,
            "case {case}: expected the message {expected:?}, got {answered}"
        );
    }
}
