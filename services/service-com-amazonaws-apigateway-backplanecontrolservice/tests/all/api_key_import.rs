// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ImportApiKeys`: the API key file a request carries becomes one
//! stored `ApiKey` per row the import can carry over, read back through `GetApiKey`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`] and friends are this crate's one
//! wire-test harness, reused from `rest_api_lifecycle` and `api_key_lifecycle`.
//!
//! The RULE under test, asserted over SETS of inputs rather than one recorded call:
//! `ImportApiKeys` reads its payload as a CSV API key file, creates one API key per data
//! row from that row's columns, reports the created ids in row order and one warning per
//! row it could not carry over, and — when `failOnWarnings` is set — refuses the whole
//! import with the modelled 400 instead, leaving no record created.
//!
//! The tables' rows differ along what the rule generalises over:
//!
//! * the COLUMN SET and its ORDER, including a column the format does not declare;
//! * the header's CASE and surrounding space;
//! * the ROW COUNT (none, one, several) and the line ending between rows;
//! * the key value's LENGTH, at each published bound and one past each;
//! * the FORM of a cell: quoted and carrying a comma, a doubled quote or a line ending,
//!   empty, absent because the row stops short of the header's width;
//! * the WORD an `Enabled` cell carries, in every spelling of each, plus one that is
//!   neither;
//! * the document's shape: blank lines between rows, a byte order mark before the header;
//! * WHICH check a row fails, and whether the rows around it are still imported;
//! * how many of a `usageplanIds` cell's ids name a stored usage plan (all, none, some,
//!   one named twice), and whether the key ends up metered by it.

use serde_json::Value;

use crate::api_key_lifecycle::{create_key, get_key};
use crate::rest_api_lifecycle::{assert_failure, Gateway};
use crate::usage_plan_key_lifecycle::get_usage_plan_key;
use crate::usage_plan_lifecycle::create_usage_plan;

/// AWS's published length range for an API key value: 20 to 128 characters. The same
/// range the sibling `api_key_lifecycle` suite holds `CreateApiKey$value` to.
///
/// Written out here from the published value rather than imported from the handler's own
/// constant ON PURPOSE, as in that sibling suite: a bound test that reads the bound from
/// the code it is testing cannot fail.
const MIN_VALUE_CHARS: usize = 20;

/// The upper end of that range; see [`MIN_VALUE_CHARS`].
const MAX_VALUE_CHARS: usize = 128;

/// The error code every refusal in this suite carries.
const BAD_REQUEST: &str = "BadRequestException";

/// `ImportApiKeys` with `document` as its payload, sent verbatim on the operation's own
/// route (`POST /apikeys?mode=import`, `format` and `failonwarnings` bound to the query
/// string).
async fn import(
    gateway: &Gateway,
    document: &str,
    fail_on_warnings: bool,
) -> crate::rest_api_lifecycle::Answer {
    let uri = format!("/apikeys?mode=import&format=csv&failonwarnings={fail_on_warnings}");
    gateway.call_raw("POST", &uri, document).await
}

/// A distinct, well-formed key value for row `row` of case `case`: 22 characters, so it
/// sits inside the published range and no two rows of this suite collide.
fn row_value(case: usize, row: usize) -> String {
    format!("imported-api-key-{case:02}-{row:02}")
}

/// The `ids` a successful import answered with.
fn imported_ids(body: &Value, case: &str) -> Vec<String> {
    body["ids"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an ids array, got {body:?}"))
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string id, got {body:?}"))
                .to_owned()
        })
        .collect()
}

/// The `warnings` an import answered with, or an empty list when it reported none.
fn import_warnings(body: &Value, case: &str) -> Vec<String> {
    match &body["warnings"] {
        Value::Null => Vec::new(),
        Value::Array(warnings) => warnings
            .iter()
            .map(|warning| {
                warning
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: expected a string warning, got {body:?}")
                    })
                    .to_owned()
            })
            .collect(),
        other => panic!("case {case}: expected warnings to be a list, got {other:?}"),
    }
}

/// One row of an API key file, and the record it must become.
#[derive(Debug, Clone)]
struct Expected {
    value: String,
    name: String,
    description: Option<String>,
    enabled: bool,
}

/// Assert the key `id` names was stored exactly as `expected` describes it.
async fn assert_stored(gateway: &Gateway, id: &str, expected: &Expected, case: &str) {
    let answer = get_key(gateway, id, true).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected GetApiKey({id}) to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["value"].as_str(),
        Some(expected.value.as_str()),
        "case {case}: expected the imported value {:?}, got {:?}",
        expected.value,
        answer.body
    );
    assert_eq!(
        answer.body["name"].as_str(),
        Some(expected.name.as_str()),
        "case {case}: expected the imported name {:?}, got {:?}",
        expected.name,
        answer.body
    );
    assert_eq!(
        answer.body["description"].as_str(),
        expected.description.as_deref(),
        "case {case}: expected the imported description {:?}, got {:?}",
        expected.description,
        answer.body
    );
    assert_eq!(
        answer.body["enabled"].as_bool(),
        Some(expected.enabled),
        "case {case}: expected enabled {:?}, got {:?}",
        expected.enabled,
        answer.body
    );
    assert_eq!(
        answer.body["stageKeys"].as_array().map(Vec::len),
        Some(0),
        "case {case}: an imported key is associated with no stage, got {:?}",
        answer.body
    );
}

/// An expected record, with the optional members left unset.
fn expected(value: &str, name: &str) -> Expected {
    Expected {
        value: value.to_owned(),
        name: name.to_owned(),
        description: None,
        enabled: false,
    }
}

#[tokio::test]
async fn an_imported_api_key_file_creates_one_key_per_row_read_back_through_get_api_key() {
    let gateway = Gateway::new();

    // (case, document, the records the rows must become, in row order)
    let mut cases: Vec<(String, String, Vec<Expected>)> = Vec::new();

    // The two required columns alone.
    let value = row_value(0, 1);
    cases.push((
        "the required columns alone".to_string(),
        format!("key,name\n{value},minimal\n"),
        vec![expected(&value, "minimal")],
    ));

    // Every declared column, in an order the example file does not use: the format
    // states the column order is free, so what selects a cell is its header name.
    let value = row_value(1, 1);
    cases.push((
        "every column, in another order".to_string(),
        format!("description,name,Enabled,key\nfully described,described,TRUE,{value}\n"),
        vec![Expected {
            value: value.clone(),
            name: "described".to_string(),
            description: Some("fully described".to_string()),
            enabled: true,
        }],
    ));

    // Several rows at once, each with its own `Enabled` spelling, and a quoted cell
    // carrying a comma — one value, not two columns.
    let first = row_value(2, 1);
    let second = row_value(2, 2);
    let third = row_value(2, 3);
    cases.push((
        "three rows, mixed enablement and a quoted comma".to_string(),
        format!(
            "key,name,description,Enabled\n\
             {first},first,\"one, with a comma\",true\n\
             {second},second,,FALSE\n\
             {third},third,,\n"
        ),
        vec![
            Expected {
                value: first.clone(),
                name: "first".to_string(),
                description: Some("one, with a comma".to_string()),
                enabled: true,
            },
            expected(&second, "second"),
            expected(&third, "third"),
        ],
    ));

    // A header whose names are differently cased and padded: the names are matched
    // without regard to either.
    let value = row_value(3, 1);
    cases.push((
        "a differently cased and padded header".to_string(),
        format!(" KEY , Name , ENABLED \n {value} , cased , true \n"),
        vec![Expected {
            value: value.clone(),
            name: "cased".to_string(),
            description: None,
            enabled: true,
        }],
    ));

    // Both words the `Enabled` column carries, in each spelling the format's example
    // file and a spreadsheet export write them with: the cell is read without regard to
    // case, and an EMPTY cell is the modelled default (`enabled` is `default false`).
    let mut document = String::from("key,name,Enabled\n");
    let mut rows = Vec::new();
    for (index, (word, enabled)) in [
        ("TRUE", true),
        ("true", true),
        ("True", true),
        ("FALSE", false),
        ("false", false),
        ("False", false),
        ("", false),
    ]
    .into_iter()
    .enumerate()
    {
        let value = row_value(14, index + 1);
        document.push_str(&format!("{value},enabled-{index},{word}\n"));
        rows.push(Expected {
            value,
            name: format!("enabled-{index}"),
            description: None,
            enabled,
        });
    }
    cases.push((
        "every spelling of the Enabled column's two words".to_string(),
        document,
        rows,
    ));

    // A column the format does not declare carries nothing this service stores, so it
    // is ignored rather than refused.
    let value = row_value(4, 1);
    cases.push((
        "a column the format does not declare".to_string(),
        format!("key,name,customerId\n{value},extra-column,marketplace-1\n"),
        vec![expected(&value, "extra-column")],
    ));

    // Rows separated by CRLF, as a spreadsheet export writes them.
    let first = row_value(5, 1);
    let second = row_value(5, 2);
    cases.push((
        "CRLF line endings".to_string(),
        format!("key,name\r\n{first},crlf-one\r\n{second},crlf-two\r\n"),
        vec![expected(&first, "crlf-one"), expected(&second, "crlf-two")],
    ));

    // The published length range, at each bound: both are inside it.
    let shortest = "a".repeat(MIN_VALUE_CHARS);
    let longest = "b".repeat(MAX_VALUE_CHARS);
    cases.push((
        "key values at each published length bound".to_string(),
        format!("key,name\n{shortest},shortest\n{longest},longest\n"),
        vec![
            expected(&shortest, "shortest"),
            expected(&longest, "longest"),
        ],
    ));

    // A header line and no data rows: nothing to import is not a failure.
    cases.push((
        "a header line and no rows".to_string(),
        "key,name,description,Enabled,usageplanIds\n".to_string(),
        Vec::new(),
    ));

    // A doubled quote inside a quoted cell is one quote character in the value.
    let value = row_value(6, 1);
    cases.push((
        "a doubled quote inside a quoted cell".to_string(),
        format!("key,name,description\n{value},quoted,\"the \"\"only\"\" one\"\n"),
        vec![Expected {
            value: value.clone(),
            name: "quoted".to_string(),
            description: Some("the \"only\" one".to_string()),
            enabled: false,
        }],
    ));

    // A line ending inside a quoted cell is part of the value, not the end of the row.
    let value = row_value(7, 1);
    cases.push((
        "a line ending inside a quoted cell".to_string(),
        format!("key,name,description\n{value},multiline,\"first line\nsecond line\"\n"),
        vec![Expected {
            value: value.clone(),
            name: "multiline".to_string(),
            description: Some("first line\nsecond line".to_string()),
            enabled: false,
        }],
    ));

    // A blank line between rows is not a row.
    let first = row_value(8, 1);
    let second = row_value(8, 2);
    cases.push((
        "blank lines between rows".to_string(),
        format!("key,name\n\n{first},spaced-one\n\n{second},spaced-two\n\n"),
        vec![
            expected(&first, "spaced-one"),
            expected(&second, "spaced-two"),
        ],
    ));

    // A document carrying a byte order mark, as a spreadsheet export writes it: the mark
    // belongs to the encoding, not to the first column's name.
    let value = row_value(9, 1);
    cases.push((
        "a byte order mark before the header".to_string(),
        format!("\u{feff}key,name\n{value},marked\n"),
        vec![expected(&value, "marked")],
    ));

    for (case, document, rows) in cases {
        let answer = import(&gateway, &document, false).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        let warnings = import_warnings(&answer.body, &case);
        assert!(
            warnings.is_empty(),
            "case {case}: expected no warnings, got {warnings:?}"
        );
        let ids = imported_ids(&answer.body, &case);
        assert_eq!(
            ids.len(),
            rows.len(),
            "case {case}: expected {} ids, got {ids:?}",
            rows.len()
        );
        for (id, row) in ids.iter().zip(&rows) {
            assert_stored(&gateway, id, row, &case).await;
        }
    }
}

#[tokio::test]
async fn a_row_the_import_cannot_carry_over_is_a_warning_naming_it_and_the_rest_are_imported() {
    let gateway = Gateway::new();

    // A key value this account already holds, so a row declaring it collides with the
    // stored record rather than with another row.
    let stored = "s".repeat(MIN_VALUE_CHARS);
    create_key(
        &gateway,
        "a key the import will collide with",
        serde_json::json!({ "name": "already-stored", "value": stored }),
    )
    .await;

    let good = row_value(10, 1);
    let too_short = "t".repeat(MIN_VALUE_CHARS - 1);
    let too_long = "u".repeat(MAX_VALUE_CHARS + 1);
    let duplicated = row_value(11, 1);
    let planned = row_value(12, 1);
    let partial = row_value(13, 1);
    let mistyped = row_value(15, 1);

    // (case, document, the values the import must create, the fragments each warning
    // must carry)
    let cases: Vec<(String, String, Vec<String>, Vec<String>)> = vec![
        (
            "a row with no key cell".to_string(),
            "key,name\n,nameless-key\n".to_string(),
            Vec::new(),
            vec!["row 1".to_string(), "key is required".to_string()],
        ),
        (
            "a row with no name cell".to_string(),
            format!("key,name\n{good},\n"),
            Vec::new(),
            vec!["row 1".to_string(), "name is required".to_string()],
        ),
        (
            "a row that stops short of the header's width".to_string(),
            format!("key,name\n{good}\n"),
            Vec::new(),
            vec!["row 1".to_string(), "name is required".to_string()],
        ),
        (
            "a key value one character under the published bound".to_string(),
            format!("key,name\n{too_short},too-short\n"),
            Vec::new(),
            vec![
                "row 1".to_string(),
                format!("between {MIN_VALUE_CHARS} and {MAX_VALUE_CHARS}"),
                format!("got {}", MIN_VALUE_CHARS - 1),
            ],
        ),
        (
            "a key value one character over the published bound".to_string(),
            format!("key,name\n{too_long},too-long\n"),
            Vec::new(),
            vec![
                "row 1".to_string(),
                format!("got {}", MAX_VALUE_CHARS + 1),
            ],
        ),
        (
            "two rows declaring one key value".to_string(),
            format!("key,name\n{duplicated},first\n{duplicated},second\n"),
            vec![duplicated.clone()],
            vec!["row 2".to_string(), "already exists".to_string()],
        ),
        (
            "a row declaring a key value this account already holds".to_string(),
            format!("key,name\n{stored},collides\n"),
            Vec::new(),
            vec!["row 1".to_string(), "already exists".to_string()],
        ),
        (
            "a row naming a usage plan that does not exist".to_string(),
            format!("key,name,usageplanIds\n{planned},planned,absent-plan\n"),
            vec![planned.clone()],
            vec![
                "row 1".to_string(),
                "no usage plan absent-plan exists".to_string(),
            ],
        ),
        (
            "one good row beside one bad row".to_string(),
            format!("key,name\n{partial},imported\n{too_short},too-short\n"),
            vec![partial.clone()],
            vec!["row 2".to_string()],
        ),
        (
            // The one malformed CELL that does not stop the row being imported: the key
            // keeps the modelled default and the caller is told the cell was not read.
            "an Enabled cell that is neither of the format's words".to_string(),
            format!("key,name,Enabled\n{mistyped},mistyped,yes\n"),
            vec![mistyped.clone()],
            vec![
                "row 1".to_string(),
                "enabled must be one of TRUE, FALSE".to_string(),
                "left disabled".to_string(),
            ],
        ),
    ];

    for (case, document, created, fragments) in cases {
        let answer = import(&gateway, &document, false).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        let ids = imported_ids(&answer.body, &case);
        assert_eq!(
            ids.len(),
            created.len(),
            "case {case}: expected {} created key(s), got {ids:?}",
            created.len()
        );
        // The rows the import DID carry over are stored, whatever the warning said
        // about the others.
        for (id, value) in ids.iter().zip(&created) {
            let stored = get_key(&gateway, id, true).await;
            assert_eq!(
                stored.body["value"].as_str(),
                Some(value.as_str()),
                "case {case}: expected the imported value {value:?}, got {:?}",
                stored.body
            );
        }
        let warnings = import_warnings(&answer.body, &case);
        assert_eq!(
            warnings.len(),
            1,
            "case {case}: expected exactly one warning, got {warnings:?}"
        );
        for fragment in &fragments {
            assert!(
                warnings[0].contains(fragment.as_str()),
                "case {case}: expected the warning to carry {fragment:?}, got {warnings:?}"
            );
        }
    }
}

/// The most warnings this service reports for a caller-sized list
/// (`state::MAX_COLLECTION_ENTRIES`), which is where the reported list is cut.
///
/// Written out rather than imported for the reason given on [`MIN_VALUE_CHARS`]: the
/// bound this suite asserts must be stated independently of the code under test.
const MAX_WARNINGS: usize = 50;

#[tokio::test]
async fn a_warning_list_past_the_reported_bound_is_cut_and_says_how_many_it_left_out() {
    let gateway = Gateway::new();

    // Rows differ along how many of them warn: one short of the bound, exactly at it,
    // and past it, where the last entry accounts for what was left out.
    for extra in [-1i64, 0, 5] {
        let rows = usize::try_from(i64::try_from(MAX_WARNINGS).unwrap_or_default() + extra)
            .unwrap_or_default();
        let case = format!("{rows} warning rows");
        // Every row carries a key value one character short of the published bound, so
        // every row warns and none is imported.
        let mut document = String::from("key,name\n");
        for row in 0..rows {
            document.push_str(&format!("{},short-{row}\n", "w".repeat(MIN_VALUE_CHARS - 1)));
        }
        let answer = import(&gateway, &document, false).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            imported_ids(&answer.body, &case).is_empty(),
            "case {case}: expected no key to be created, got {:?}",
            answer.body
        );
        let warnings = import_warnings(&answer.body, &case);
        assert_eq!(
            warnings.len(),
            rows.min(MAX_WARNINGS),
            "case {case}: expected the reported warnings to be bounded at {MAX_WARNINGS}, \
             got {} of them",
            warnings.len()
        );
        let last = warnings.last().cloned().unwrap_or_default();
        if rows > MAX_WARNINGS {
            let omitted = rows - MAX_WARNINGS + 1;
            assert_eq!(
                last,
                format!("{omitted} further row(s) warned"),
                "case {case}: expected the cut list to account for the rows it left out, \
                 got {last:?}"
            );
        } else {
            assert!(
                last.starts_with("row "),
                "case {case}: a complete list carries only per-row warnings, got {last:?}"
            );
        }
    }
}

#[tokio::test]
async fn fail_on_warnings_refuses_the_whole_import_and_creates_nothing() {
    let gateway = Gateway::new();

    // Each document carries one row that would be imported and one the import cannot
    // carry over, so the rollback is observable: the good row's value is still free
    // afterwards.
    let rows: Vec<(String, String, String)> = vec![
        (
            "a row with no key cell".to_string(),
            row_value(20, 1),
            String::new(),
        ),
        (
            "a key value under the published bound".to_string(),
            row_value(21, 1),
            "v".repeat(MIN_VALUE_CHARS - 1),
        ),
        (
            "a duplicated key value".to_string(),
            row_value(22, 1),
            row_value(22, 1),
        ),
    ];

    for (case, good, bad) in rows {
        let document = format!("key,name\n{good},imported\n{bad},refused\n");
        let refused = import(&gateway, &document, true).await;
        assert_failure(&refused, 400, BAD_REQUEST, &case);
        assert!(
            refused.body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("Errors found during import"),
            "case {case}: expected the refusal to state the import's warnings, got {:?}",
            refused.body
        );

        // The rollback: nothing was created, so the SAME document imported without
        // `failOnWarnings` now creates the good row — which it could not do if the
        // refused import had already stored that value.
        let accepted = import(&gateway, &document, false).await;
        assert_eq!(
            accepted.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            accepted.status, accepted.body
        );
        let ids = imported_ids(&accepted.body, &case);
        assert_eq!(
            ids.len(),
            1,
            "case {case}: expected the good row to be the only key created, got {ids:?}"
        );
        assert_stored(&gateway, &ids[0], &expected(&good, "imported"), &case).await;
    }
}

#[tokio::test]
async fn a_payload_that_is_not_an_api_key_file_is_refused() {
    let gateway = Gateway::new();

    // (case, document)
    let cases: Vec<(&str, String)> = vec![
        ("an empty payload", String::new()),
        ("a payload of whitespace only", "   \n".to_string()),
        (
            "a header declaring neither required column",
            "description,Enabled\nnot a key file,true\n".to_string(),
        ),
        (
            "a header declaring no key column",
            format!("name,description\nnamed,{}\n", "no key here"),
        ),
        (
            "a header declaring no name column",
            format!("key,description\n{},no name column\n", row_value(30, 1)),
        ),
        (
            "a payload past the body bound",
            format!("key,name\n{}", "x".repeat(1024 * 1024 + 1)),
        ),
    ];

    for (case, document) in cases {
        let answer = import(&gateway, &document, false).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
        assert!(
            answer.body["message"].as_str().is_some(),
            "case {case}: expected the refusal to state what to fix, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn the_format_query_parameter_is_held_to_the_modelled_enum() {
    let gateway = Gateway::new();
    let document = format!("key,name\n{},formatted\n", row_value(40, 1));

    // A format outside the modelled `ApiKeysFormat` enum (`one of: csv`) is refused
    // before the payload is read at all, with the code this surface answers a modelled
    // constraint failure with — 400 `BadRequestException`, the classification the sibling
    // `validation_message_rules` suite pins for this service — and no key is created.
    let refused = gateway
        .call_raw(
            "POST",
            "/apikeys?mode=import&format=json&failonwarnings=false",
            &document,
        )
        .await;
    assert_failure(&refused, 400, BAD_REQUEST, "a format outside the enum");

    // The modelled value is read, and the same payload imports.
    let accepted = import(&gateway, &document, false).await;
    assert_eq!(
        accepted.status, 201,
        "case format=csv: expected 201, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        imported_ids(&accepted.body, "format=csv").len(),
        1,
        "case format=csv: expected one created key, got {:?}",
        accepted.body
    );
}

/// RULE: a `usageplanIds` cell naming a STORED usage plan is carried over as the same
/// `UsagePlanKey` association `CreateUsagePlanKey` writes — so the imported key is read
/// back through `GetUsagePlanKey` — while an id naming no stored plan is the one thing
/// that row could not carry over and is reported as its warning.
///
/// Rows span how many of the cell's ids name a stored plan (all, none, some), and how
/// many plans one row names, which is what the comma-separated cell generalises over.
#[tokio::test]
async fn a_row_naming_a_stored_usage_plan_is_metered_by_it_and_an_unknown_one_warns() {
    let gateway = Gateway::new();

    // Two stored plans, so a row can name one, both, or one of each kind.
    let mut plan_ids: Vec<String> = Vec::new();
    for index in 0..2 {
        let plan = create_usage_plan(
            &gateway,
            "a plan an import meters a key against",
            serde_json::json!({ "name": format!("import-plan-{index}") }),
        )
        .await;
        plan_ids.push(
            plan["id"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a created plan id, got {plan:?}"))
                .to_owned(),
        );
    }

    /// One row of the usage-plan table.
    #[derive(Debug)]
    struct PlanCase {
        case: &'static str,
        /// What the row's `usageplanIds` cell names.
        named: Vec<String>,
        /// The plans the imported key must end up metered by.
        metered: Vec<String>,
        /// A fragment each reported warning must carry, one per expected warning.
        fragments: Vec<String>,
    }

    let cases: Vec<PlanCase> = vec![
        PlanCase {
            case: "one stored plan",
            named: vec![plan_ids[0].clone()],
            metered: vec![plan_ids[0].clone()],
            fragments: Vec::new(),
        },
        PlanCase {
            case: "both stored plans in one cell",
            named: vec![plan_ids[0].clone(), plan_ids[1].clone()],
            metered: vec![plan_ids[0].clone(), plan_ids[1].clone()],
            fragments: Vec::new(),
        },
        PlanCase {
            case: "one stored plan beside one that does not exist",
            named: vec![plan_ids[1].clone(), "no-such-plan".to_string()],
            metered: vec![plan_ids[1].clone()],
            fragments: vec!["no usage plan no-such-plan exists".to_string()],
        },
        PlanCase {
            case: "the same stored plan named twice",
            named: vec![plan_ids[0].clone(), plan_ids[0].clone()],
            metered: vec![plan_ids[0].clone()],
            fragments: Vec::new(),
        },
        PlanCase {
            case: "only ids that name no stored plan",
            named: vec!["absent-one".to_string(), "absent-two".to_string()],
            metered: Vec::new(),
            fragments: vec![
                "no usage plan absent-one exists".to_string(),
                "no usage plan absent-two exists".to_string(),
            ],
        },
    ];

    for (index, row) in cases.into_iter().enumerate() {
        let PlanCase {
            case,
            named,
            metered,
            fragments,
        } = row;
        let value = row_value(50, index + 1);
        let document = format!(
            "key,name,usageplanIds\n{value},metered-{index},\"{}\"\n",
            named.join(",")
        );
        let answer = import(&gateway, &document, false).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        let ids = imported_ids(&answer.body, case);
        assert_eq!(
            ids.len(),
            1,
            "case {case}: expected the row to create one key, got {ids:?}"
        );
        let key_id = &ids[0];

        let warnings = import_warnings(&answer.body, case);
        assert_eq!(
            warnings.len(),
            fragments.len(),
            "case {case}: expected {} warning(s), got {warnings:?}",
            fragments.len()
        );
        for fragment in &fragments {
            assert!(
                warnings
                    .iter()
                    .any(|warning| warning.contains(fragment.as_str())),
                "case {case}: expected a warning carrying {fragment:?}, got {warnings:?}"
            );
        }

        // The association is read back through `GetUsagePlanKey`: the imported key is
        // metered by every plan the cell named that exists, and by no other.
        for plan_id in &plan_ids {
            let read = get_usage_plan_key(&gateway, plan_id, key_id).await;
            let expected_status = if metered.contains(plan_id) { 200 } else { 404 };
            assert_eq!(
                read.status, expected_status,
                "case {case}: expected GetUsagePlanKey({plan_id}, {key_id}) to answer \
                 {expected_status}, got {} with {:?}",
                read.status, read.body
            );
            if expected_status == 200 {
                assert_eq!(
                    read.body["id"].as_str(),
                    Some(key_id.as_str()),
                    "case {case}: expected the association to name the imported key, got {:?}",
                    read.body
                );
                assert_eq!(
                    read.body["type"].as_str(),
                    Some("API_KEY"),
                    "case {case}: expected the modelled API_KEY type, got {:?}",
                    read.body
                );
                assert_eq!(
                    read.body["value"].as_str(),
                    Some(value.as_str()),
                    "case {case}: expected the imported value on the association, got {:?}",
                    read.body
                );
            }
        }
    }
}
