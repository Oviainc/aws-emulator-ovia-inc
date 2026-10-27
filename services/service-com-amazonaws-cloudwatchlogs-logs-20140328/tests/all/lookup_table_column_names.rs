// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two rules every lookup-table write and read of this service is
//! held to: which CSV header rows name a table, and how a table the account does not hold is
//! refused.
//!
//! The rules under test:
//!
//! 1. A `tableBody`'s first row "must be a header row with column names", and those names are
//!    the columns the table is read by — so a header row that declares the same column name
//!    more than once names no column set and the write is refused, with
//!    `ValidationException` and the fixed sentence AWS answers for that error class, before
//!    anything is stored. A header row whose names are all DISTINCT is accepted, whatever the
//!    names are and however many columns or data rows the body carries.
//! 2. A refusal that the lookup table a request identifies is not one the account holds
//!    states WHICH table and WHICH account was looked in, both taken from the identifier the
//!    request carried.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables span what the rules generalise over rather than the one body or the one table
//! that motivated them: duplicate header rows differing in where the repeat sits (adjacent,
//! apart, at the end), in how often the name repeats, in column count, in quoting, in data-row
//! count, in line terminator, and in whether the data rows agree with the header's column
//! count at all; distinct header rows differing in case, in surrounding whitespace, in
//! quoting, in column count and in data-row count; and missing tables reached through a name
//! nothing created, a table this account deleted, another Region's ARN and another account's
//! ARN, through each of the three operations that identify a table by its ARN.

use serde_json::{json, Value};

use crate::harness::{Logs, REGION};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::lookup_table_arn;

/// The operations under test: the two writes that read a CSV body, and the three operations
/// that identify a table by its ARN, plus the enumeration a refused create must stay out of.
const CREATE: &str = "CreateLookupTable";
const UPDATE: &str = "UpdateLookupTable";
const GET: &str = "GetLookupTable";
const DELETE: &str = "DeleteLookupTable";
const DESCRIBE: &str = "DescribeLookupTables";

/// The code answered for a body whose header row names no column set — a rule about the
/// header row as a whole, which no single member's constraint can express.
const VALIDATION: &str = "ValidationException";

/// The code answered when the table a request identifies is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// AWS's wording for a header row that declares one column name twice. The sentence is fixed
/// text for that error class: it names neither the repeated column, nor the member, nor a
/// position in the body.
const DUPLICATE_FIELDS: &str = "CSV header contains duplicate field names";

/// One CSV body whose header row declares a column name more than once.
struct Duplicate {
    /// What this body varies, for the assertion messages.
    case: &'static str,
    /// The name the refused create would have stored the table under.
    name: &'static str,
    table_body: &'static str,
}

/// Every form a repeated header name takes. The last row also carries data rows that disagree
/// with the header's column count, so the header rule is shown to be decided before the rows
/// after it are read.
fn duplicates() -> Vec<Duplicate> {
    vec![
        Duplicate {
            case: "two columns, the same name twice",
            name: "dup_adjacent",
            table_body: "key,key\nuser_id,active\n",
        },
        Duplicate {
            case: "three columns, the repeat apart from the first",
            name: "dup_apart",
            table_body: "id,name,id\n1,alice,2\n3,bob,4\n",
        },
        Duplicate {
            case: "the repeat at the end of a wide header",
            name: "dup_at_the_end",
            table_body: "ip_address,hostname,owner,ip_address\n10.0.1.5,web-01,team-a,10.0.1.6\n",
        },
        Duplicate {
            case: "the same name three times",
            name: "dup_three_times",
            table_body: "a,a,a\n1,2,3\n",
        },
        Duplicate {
            case: "a quoted name repeating an unquoted one",
            name: "dup_quoted",
            table_body: "\"key\",key\nuser_id,active\n",
        },
        Duplicate {
            case: "a repeat in a header row with no data rows after it",
            name: "dup_no_rows",
            table_body: "col,col\n",
        },
        Duplicate {
            case: "a repeat in a CRLF-terminated body",
            name: "dup_crlf",
            table_body: "ip,ip\r\n10.0.0.1,10.0.0.2\r\n",
        },
        Duplicate {
            case: "a repeat whose data row carries fewer fields than the header",
            name: "dup_short_row",
            table_body: "key,key\nuser_id\n",
        },
    ]
}

/// One CSV body whose header row names a DISTINCT set of columns: the neighbouring rule, which
/// the duplicate check must not swallow.
struct Distinct {
    case: &'static str,
    name: &'static str,
    table_body: &'static str,
    /// The columns CSV reads from the header row, in order.
    fields: &'static [&'static str],
    /// The data rows after the header row.
    records: i64,
}

/// Header rows that name distinct columns, including names that differ only in case or in
/// surrounding whitespace — CSV compares a field's bytes, so neither is a repeat.
fn distincts() -> Vec<Distinct> {
    vec![
        Distinct {
            case: "two distinct names",
            name: "ok_two_names",
            table_body: "key,value\nuser_id,active\n",
            fields: &["key", "value"],
            records: 1,
        },
        Distinct {
            case: "one column only",
            name: "ok_one_column",
            table_body: "key\nuser_id\nsession_id\n",
            fields: &["key"],
            records: 2,
        },
        Distinct {
            case: "names differing only in case",
            name: "ok_case_differs",
            table_body: "Key,key\nA,b\n",
            fields: &["Key", "key"],
            records: 1,
        },
        Distinct {
            case: "names differing only in surrounding whitespace",
            name: "ok_whitespace_differs",
            table_body: "key, key\nA,b\n",
            fields: &["key", " key"],
            records: 1,
        },
        Distinct {
            case: "a quoted name carrying the separator",
            name: "ok_quoted_separator",
            table_body: "\"first,name\",first\nalice,a\n",
            fields: &["first,name", "first"],
            records: 1,
        },
        Distinct {
            case: "a wide header with no data rows",
            name: "ok_wide_no_rows",
            table_body: "ip_address,hostname,owner,team\n",
            fields: &["ip_address", "hostname", "owner", "team"],
            records: 0,
        },
    ]
}

/// A top-level string member of a response body.
fn member(body: &Value, key: &str) -> Option<String> {
    body.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// Every lookup table the account enumerates.
async fn listed(logs: &Logs) -> Vec<Value> {
    let page = logs.ok(DESCRIBE, &json!({})).await;
    page.get("lookupTables")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{DESCRIBE} reports lookupTables, got {page}"))
        .clone()
}

/// A create whose header row declares a column name twice is refused with the service's
/// sentence for that header, and stores nothing: the name is absent from the enumeration and
/// from the single-table read.
#[tokio::test]
async fn a_create_whose_header_row_repeats_a_column_name_is_refused() {
    let logs = Logs::new();

    for row in duplicates() {
        let (code, message) = logs
            .refusal(
                CREATE,
                &json!({"lookupTableName": row.name, "tableBody": row.table_body}),
            )
            .await;
        assert_eq!(
            code, VALIDATION,
            "case {:?}: a header row naming one column twice is a validation failure, \
             expected {VALIDATION}, got {code} {message:?}",
            row.case
        );
        assert_eq!(
            message, DUPLICATE_FIELDS,
            "case {:?}: expected the message {DUPLICATE_FIELDS:?}, got {message:?}",
            row.case
        );

        let names: Vec<String> = listed(&logs)
            .await
            .iter()
            .filter_map(|table| member(table, "lookupTableName"))
            .collect();
        assert!(
            !names.iter().any(|name| name == row.name),
            "case {:?}: a refused create stores no table, got the enumeration {names:?}",
            row.case
        );

        let arn = lookup_table_arn(REGION, row.name);
        let (code, message) = logs.refusal(GET, &json!({"lookupTableArn": arn})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {:?}: the refused create left nothing to read, expected {NOT_FOUND}, got \
             {code} {message:?}",
            row.case
        );
    }
}

/// A header row naming distinct columns is accepted whatever those names are: the table is
/// stored, its columns are the header's names in order, its records are the rows after it, and
/// its body comes back verbatim.
#[tokio::test]
async fn a_create_whose_header_row_names_distinct_columns_is_accepted() {
    let logs = Logs::new();

    for row in distincts() {
        logs.ok(
            CREATE,
            &json!({"lookupTableName": row.name, "tableBody": row.table_body}),
        )
        .await;

        let reported = listed(&logs)
            .await
            .into_iter()
            .find(|table| member(table, "lookupTableName").as_deref() == Some(row.name))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the accepted create is enumerated under {:?}",
                    row.case, row.name
                )
            });
        let fields: Vec<String> = reported
            .get("tableFields")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a reported table carries tableFields, got {reported}",
                    row.case
                )
            })
            .iter()
            .filter_map(|field| field.as_str().map(ToString::to_string))
            .collect();
        assert_eq!(
            fields, row.fields,
            "case {:?}: the header row names the table's columns, expected {:?}, got {fields:?}",
            row.case, row.fields
        );
        let records = reported.get("recordsCount").and_then(Value::as_i64);
        assert_eq!(
            records,
            Some(row.records),
            "case {:?}: the rows after the header are the table's records, expected {}, got \
             {records:?}",
            row.case,
            row.records
        );

        let read = logs
            .ok(
                GET,
                &json!({"lookupTableArn": lookup_table_arn(REGION, row.name)}),
            )
            .await;
        assert_eq!(
            member(&read, "tableBody").as_deref(),
            Some(row.table_body),
            "case {:?}: the stored body comes back verbatim, got {read}",
            row.case
        );
    }
}

/// An update is held to the same header rule, and a refused update replaces nothing: the table
/// keeps the content and the last-updated instant it had.
#[tokio::test]
async fn an_update_whose_header_row_repeats_a_column_name_is_refused() {
    let logs = Logs::new();
    const NAME: &str = "update_header_rules";
    const BODY: &str = "ip_address,hostname,owner\n10.0.1.5,web-01,team-a\n";

    logs.ok(CREATE, &json!({"lookupTableName": NAME, "tableBody": BODY}))
        .await;
    let arn = lookup_table_arn(REGION, NAME);
    let before = logs.ok(GET, &json!({"lookupTableArn": arn})).await;
    let updated_before = before.get("lastUpdatedTime").and_then(Value::as_i64);

    for row in duplicates() {
        let (code, message) = logs
            .refusal(
                UPDATE,
                &json!({"lookupTableArn": arn, "tableBody": row.table_body}),
            )
            .await;
        assert_eq!(
            code, VALIDATION,
            "case {:?}: an update's header row is held to the same rule, expected \
             {VALIDATION}, got {code} {message:?}",
            row.case
        );
        assert_eq!(
            message, DUPLICATE_FIELDS,
            "case {:?}: expected the message {DUPLICATE_FIELDS:?}, got {message:?}",
            row.case
        );

        let read = logs.ok(GET, &json!({"lookupTableArn": arn})).await;
        assert_eq!(
            member(&read, "tableBody").as_deref(),
            Some(BODY),
            "case {:?}: a refused update replaces nothing, got {read}",
            row.case
        );
        assert_eq!(
            read.get("lastUpdatedTime").and_then(Value::as_i64),
            updated_before,
            "case {:?}: a refused update does not move the last-updated instant, got {read}",
            row.case
        );
    }

    // The neighbouring rule holds for an update too: a replacement whose header names distinct
    // columns is accepted and observable through the read.
    const REPLACEMENT: &str = "ip_address,team\n10.0.1.5,team-b\n";
    logs.ok(
        UPDATE,
        &json!({"lookupTableArn": arn, "tableBody": REPLACEMENT}),
    )
    .await;
    let read = logs.ok(GET, &json!({"lookupTableArn": arn})).await;
    assert_eq!(
        member(&read, "tableBody").as_deref(),
        Some(REPLACEMENT),
        "case \"a distinct header replaces the content\": got {read}"
    );
}

/// One table a request identifies that the account does not hold: the ARN it names, and the
/// table name and account id the refusal must state, both of which that ARN carries.
struct Missing {
    case: &'static str,
    arn: String,
    name: String,
    account_id: String,
}

/// A refusal that the identified lookup table is not there names the table and the account the
/// identifier belongs to — through every operation that identifies a table by its ARN, and for
/// a table that never existed as well as one this account deleted.
#[tokio::test]
async fn a_missing_lookup_table_is_refused_by_name_and_account() {
    let logs = Logs::new();
    let account = engine::emulator_account_id().to_string();

    // Two tables created and then deleted, so the ARNs below name tables this account HELD —
    // and one table left in place, so none of the refusals can be a reader that fails for
    // every ARN.
    const DELETED: [&str; 2] = ["gone_probe", "second_gone_probe"];
    const HELD: &str = "still_held_probe";
    for name in DELETED.iter().chain(std::iter::once(&HELD)) {
        logs.ok(
            CREATE,
            &json!({
                "lookupTableName": name,
                "tableBody": "ip_address,hostname,owner\n10.0.1.5,web-01,team-a\n",
            }),
        )
        .await;
    }
    for name in DELETED {
        logs.ok(
            DELETE,
            &json!({"lookupTableArn": lookup_table_arn(REGION, name)}),
        )
        .await;
    }

    let mut cases: Vec<Missing> = DELETED
        .iter()
        .map(|name| Missing {
            case: "a table this account deleted",
            arn: lookup_table_arn(REGION, name),
            name: (*name).to_string(),
            account_id: account.clone(),
        })
        .collect();
    cases.push(Missing {
        case: "a name nothing created",
        arn: lookup_table_arn(REGION, "never_created_probe"),
        name: "never_created_probe".to_string(),
        account_id: account.clone(),
    });
    cases.push(Missing {
        case: "a held table's name in ANOTHER Region",
        arn: lookup_table_arn("us-east-1", HELD),
        name: HELD.to_string(),
        account_id: account.clone(),
    });
    cases.push(Missing {
        case: "a held table's name in ANOTHER account",
        arn: format!("arn:aws:logs:{REGION}:111122223333:lookup-table:{HELD}"),
        name: HELD.to_string(),
        account_id: "111122223333".to_string(),
    });

    for case in cases {
        let expected = format!(
            "Lookup table '{}' not found in account '{}'",
            case.name, case.account_id
        );
        // Each operation that identifies the table by its ARN, with the members its request
        // also requires: an update replaces content, so it carries a body whose header names
        // distinct columns and is refused for the table rather than for the body.
        let requests = [
            (GET, json!({"lookupTableArn": case.arn})),
            (DELETE, json!({"lookupTableArn": case.arn})),
            (
                UPDATE,
                json!({"lookupTableArn": case.arn, "tableBody": "key,value\nuser_id,active\n"}),
            ),
        ];
        for (operation, request) in requests {
            let (code, message) = logs.refusal(operation, &request).await;
            assert_eq!(
                code, NOT_FOUND,
                "case {:?} ({operation}): expected {NOT_FOUND}, got {code} {message:?}",
                case.case
            );
            assert_eq!(
                message, expected,
                "case {:?} ({operation}): expected the message {expected:?}, got {message:?}",
                case.case
            );
        }
    }

    // The table the account DOES hold still reads, so none of the refusals above is a reader
    // that answers not-found for everything.
    let read = logs
        .ok(
            GET,
            &json!({"lookupTableArn": lookup_table_arn(REGION, HELD)}),
        )
        .await;
    assert_eq!(
        member(&read, "lookupTableName").as_deref(),
        Some(HELD),
        "the table the account holds is still reported, got {read}"
    );
}
