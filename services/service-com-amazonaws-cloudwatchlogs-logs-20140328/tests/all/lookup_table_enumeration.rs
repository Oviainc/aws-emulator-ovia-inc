// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeLookupTables`, the operation that enumerates the lookup
//! tables an account holds.
//!
//! The rule under test: `DescribeLookupTables` reports EVERY lookup table the account holds,
//! each rendered from the record the create stored (so a listing and `GetLookupTable` cannot
//! disagree about one), in ascending table-name order whatever order the tables were created
//! in; `lookupTableNamePrefix` narrows the enumeration to the tables whose names start with
//! it and nothing else; a page holds at most `maxResults` tables — 50 when the request leaves
//! that member at its modelled default, which is the default the member documents — and
//! carries a `nextToken` exactly while tables remain beyond it, resuming the SAME enumeration
//! after the last table reported. A token this operation did not mint, or one minted for a
//! different prefix, is refused and reports nothing.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables here span what the rule generalises over rather than one request: the seeded
//! corpus differs in name case (upper-case names sort before lower-case ones in ascending
//! order) and in whether one name is a prefix of another, in column count, in data-row count
//! (none, one, several), in encoding width (a multibyte row, where the reported size in BYTES
//! is not the character count) and in which optional members the create named; the prefix
//! table spans a prefix matching several tables, exactly one, a whole name, a single
//! character, a name that is itself a prefix of another, and one matching nothing; the page
//! table spans every page size from one table to more than the corpus holds, plus the
//! member's published maximum and one past it, and the unset default is read off an account
//! holding one table more than that default.

use serde_json::{json, Map, Value};

use crate::harness::{Logs, REGION};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::lookup_table_arn;

/// The operation under test, the one that puts a table in the store, and the single-table
/// read a listing must agree with.
const DESCRIBE: &str = "DescribeLookupTables";
const CREATE: &str = "CreateLookupTable";
const GET: &str = "GetLookupTable";

/// The code CloudWatch Logs answers for a parameter it will not accept, which is what
/// `DescribeLookupTables` declares for one ("A parameter is specified incorrectly").
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// A KMS key ARN, for the seeded tables that are encrypted with a customer key.
const KMS_KEY: &str = "arn:aws:kms:eu-west-2:000000000000:key/aaaabbbb-cccc-dddd-eeee-ffff00001111";

/// The published maximum of `maxResults`: `DescribeLookupTablesMaxResults` carries
/// `@range(max: 100)`, which the member's own documentation states as well ("The default value
/// is 50 and the maximum value is 100").
const MAX_RESULTS_CEILING: i64 = 100;

/// The page size a request that names no `maxResults` gets, from the member's documentation:
/// "The default value is 50 and the maximum value is 100."
const DEFAULT_MAX_RESULTS: usize = 50;

/// One seeded lookup table: what the create uploads and names, and what every reader of it
/// must therefore report back.
struct Seed {
    /// What this table varies, for the assertion messages.
    case: &'static str,
    name: &'static str,
    table_body: &'static str,
    /// The column names of the body's header row.
    fields: &'static [&'static str],
    /// The body's data rows, excluding its header row.
    records: i64,
    description: Option<&'static str>,
    kms_key_id: Option<&'static str>,
}

/// The seeded corpus, in the scrambled order the tables are CREATED in: the order a listing
/// reports is the rule under test, so it must not be the order they went in.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            case: "several columns, several rows, one of them multibyte",
            name: "users",
            table_body: "user,team,region\nalice,platform,eu-west-2\nbob,data,us-east-1\n\
                         \u{30ab}\u{30eb}\u{30ed}\u{30b9},ops,eu-west-1\n",
            fields: &["user", "team", "region"],
            records: 3,
            description: Some("the user directory"),
            kms_key_id: Some(KMS_KEY),
        },
        Seed {
            case: "a name another table's name is a prefix of",
            name: "users_eu",
            table_body: "user\nalice\n",
            fields: &["user"],
            records: 1,
            description: None,
            kms_key_id: None,
        },
        Seed {
            case: "an upper-case initial, which sorts before every lower-case one",
            name: "Users_upper",
            table_body: "user,team\nAlice,Platform\n",
            fields: &["user", "team"],
            records: 1,
            description: Some("the same directory, upper-cased"),
            kms_key_id: None,
        },
        Seed {
            case: "one column and no data rows",
            name: "metrics",
            table_body: "metric\n",
            fields: &["metric"],
            records: 0,
            description: None,
            kms_key_id: None,
        },
        Seed {
            case: "a shared name prefix, encrypted with a customer key",
            name: "accounts_us",
            table_body: "account,owner\n3,carol\n",
            fields: &["account", "owner"],
            records: 1,
            description: None,
            kms_key_id: Some(KMS_KEY),
        },
        Seed {
            case: "a shared name prefix, with a description",
            name: "accounts_eu",
            table_body: "account,owner\n1,alice\n2,bob\n",
            fields: &["account", "owner"],
            records: 2,
            description: Some("the EU account map"),
            kms_key_id: None,
        },
    ]
}

/// Every seeded name in the ascending order the operation documents, which for these names is
/// the ASCII order that puts `Users_upper` first.
fn ascending_names() -> Vec<String> {
    let mut names: Vec<String> = seeds().iter().map(|seed| seed.name.to_string()).collect();
    names.sort();
    names
}

/// Put the whole corpus in the store, through the wire call that creates a table.
async fn seed(logs: &Logs) {
    for seed in seeds() {
        let mut request = Map::new();
        request.insert("lookupTableName".to_string(), json!(seed.name));
        request.insert("tableBody".to_string(), json!(seed.table_body));
        if let Some(description) = seed.description {
            request.insert("description".to_string(), json!(description));
        }
        if let Some(kms_key_id) = seed.kms_key_id {
            request.insert("kmsKeyId".to_string(), json!(kms_key_id));
        }
        logs.ok(CREATE, &Value::Object(request)).await;
    }
}

/// The tables one page reports.
fn tables_of(page: &Value, case: &str) -> Vec<Value> {
    page.get("lookupTables")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: a page reports lookupTables, got {page}"))
        .clone()
}

/// The names one page reports, in the order it reports them.
fn names_of(page: &Value, case: &str) -> Vec<String> {
    tables_of(page, case)
        .iter()
        .map(|table| {
            table
                .get("lookupTableName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported table carries lookupTableName, got {table}")
                })
                .to_string()
        })
        .collect()
}

/// One `DescribeLookupTables` request, from the members a case names.
fn request(prefix: Option<&str>, max_results: Option<i64>, token: Option<&str>) -> Value {
    let mut request = Map::new();
    if let Some(prefix) = prefix {
        request.insert("lookupTableNamePrefix".to_string(), json!(prefix));
    }
    if let Some(max_results) = max_results {
        request.insert("maxResults".to_string(), json!(max_results));
    }
    if let Some(token) = token {
        request.insert("nextToken".to_string(), json!(token));
    }
    Value::Object(request)
}

/// Every table one enumeration reports, read page by page through the tokens it mints: a page
/// holds at most `maxResults` tables, a token arrives only with a page that reported
/// something, and the walk terminates.
async fn walk(logs: &Logs, case: &str, prefix: Option<&str>, max_results: Option<i64>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let page = logs
            .ok(DESCRIBE, &request(prefix, max_results, token.as_deref()))
            .await;
        let reported = names_of(&page, case);
        if let Some(max_results) = max_results {
            let held = i64::try_from(reported.len()).expect("a page's size fits in an i64");
            assert!(
                held <= max_results,
                "case {case:?}: a page holds at most maxResults={max_results} tables, got \
                 {reported:?}"
            );
        }
        names.extend(reported.clone());
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_some() {
            assert!(
                !reported.is_empty(),
                "case {case:?}: a page that reported nothing cannot carry a nextToken, got \
                 {token:?}"
            );
        }
        pages += 1;
        assert!(
            pages <= 256,
            "case {case:?}: the walk terminates, got {pages} pages and nextToken {token:?}"
        );
        if token.is_none() {
            break;
        }
    }
    names
}

/// Every held table is reported, in ascending name order, with every member read off the
/// record the create stored — which is what the single-table read reports too.
#[tokio::test]
async fn every_held_table_is_reported_in_ascending_name_order() {
    let logs = Logs::new();
    seed(&logs).await;

    let page = logs.ok(DESCRIBE, &request(None, None, None)).await;
    let reported = tables_of(&page, "the whole account");
    assert_eq!(
        names_of(&page, "the whole account"),
        ascending_names(),
        "case \"the whole account\": every held table is reported in ascending name order, got \
         {reported:?}"
    );
    assert!(
        page.get("nextToken").is_none(),
        "case \"the whole account\": one page holds every table, so no nextToken follows, got \
         {page}"
    );

    for seed in seeds() {
        let table = reported
            .iter()
            .find(|table| {
                table.get("lookupTableName").and_then(Value::as_str) == Some(seed.name)
            })
            .unwrap_or_else(|| {
                panic!("case {:?}: the listing reports {}, got {reported:?}", seed.case, seed.name)
            });

        let expected_arn = lookup_table_arn(REGION, seed.name);
        assert_eq!(
            table.get("lookupTableArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {:?}: expected lookupTableArn {expected_arn}, got {table}",
            seed.case
        );
        assert_eq!(
            table.get("description").and_then(Value::as_str),
            seed.description,
            "case {:?}: the description is reported only when the create named one, got {table}",
            seed.case
        );
        assert_eq!(
            table.get("kmsKeyId").and_then(Value::as_str),
            seed.kms_key_id,
            "case {:?}: the kmsKeyId is reported only when the create named one, got {table}",
            seed.case
        );
        let fields: Vec<String> = table
            .get("tableFields")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {:?}: a reported table carries tableFields, got {table}", seed.case))
            .iter()
            .map(|field| field.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            fields, seed.fields,
            "case {:?}: expected tableFields {:?}, got {fields:?}",
            seed.case, seed.fields
        );
        assert_eq!(
            table.get("recordsCount").and_then(Value::as_i64),
            Some(seed.records),
            "case {:?}: expected recordsCount {}, got {table}",
            seed.case,
            seed.records
        );
        let expected_bytes =
            i64::try_from(seed.table_body.len()).expect("the body's size fits in an i64");
        assert_eq!(
            table.get("sizeBytes").and_then(Value::as_i64),
            Some(expected_bytes),
            "case {:?}: expected sizeBytes {expected_bytes} (the body's UTF-8 length), got {table}",
            seed.case
        );
        assert!(
            table.get("tableBody").is_none(),
            "case {:?}: a listing carries a table's metadata and not its content, got {table}",
            seed.case
        );

        // The listing and the single-table read are two readers of ONE record, so what they
        // report about a table cannot differ.
        let read = logs
            .ok(GET, &json!({"lookupTableArn": expected_arn}))
            .await;
        for member in ["lookupTableArn", "lookupTableName", "description", "kmsKeyId"] {
            assert_eq!(
                table.get(member).and_then(Value::as_str),
                read.get(member).and_then(Value::as_str),
                "case {:?}: the listing and the read agree about {member}, got {table} and {read}",
                seed.case
            );
        }
        for member in ["sizeBytes", "lastUpdatedTime"] {
            assert_eq!(
                table.get(member).and_then(Value::as_i64),
                read.get(member).and_then(Value::as_i64),
                "case {:?}: the listing and the read agree about {member}, got {table} and {read}",
                seed.case
            );
        }
    }
}

/// `lookupTableNamePrefix` selects the tables whose names START with it, and nothing else.
#[tokio::test]
async fn a_name_prefix_selects_exactly_the_tables_it_starts() {
    let logs = Logs::new();
    seed(&logs).await;

    // (case, the prefix, the names it selects in ascending order)
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "a prefix several names share",
            "accounts",
            vec!["accounts_eu", "accounts_us"],
        ),
        ("a whole name", "accounts_eu", vec!["accounts_eu"]),
        (
            "one character",
            "a",
            vec!["accounts_eu", "accounts_us"],
        ),
        (
            "a name that is itself another name's prefix",
            "users",
            vec!["users", "users_eu"],
        ),
        ("one character past a whole name", "users_", vec!["users_eu"]),
        (
            "an upper-case prefix, which a lower-case name does not start with",
            "U",
            vec!["Users_upper"],
        ),
        ("a prefix no name starts with", "zzz", vec![]),
    ];

    for (case, prefix, expected) in cases {
        let page = logs
            .ok(DESCRIBE, &request(Some(prefix), None, None))
            .await;
        let names = names_of(&page, case);
        assert_eq!(
            names, expected,
            "case {case:?}: prefix {prefix:?} selects {expected:?}, got {names:?}"
        );
        assert!(
            page.get("nextToken").is_none(),
            "case {case:?}: one page holds every selected table, so no nextToken follows, got \
             {page}"
        );
    }
}

/// A page holds at most `maxResults` tables and the token resumes the same enumeration after
/// the last one reported, so walking the pages of any page size reports the enumeration once
/// and whole.
#[tokio::test]
async fn walking_the_pages_reports_the_enumeration_once_and_whole() {
    let logs = Logs::new();
    seed(&logs).await;

    let whole = ascending_names();
    for max_results in [1, 2, 3, 5, 6, 7, MAX_RESULTS_CEILING] {
        let case = format!("maxResults {max_results} over the whole account");
        let walked = walk(&logs, &case, None, Some(max_results)).await;
        assert_eq!(
            walked, whole,
            "case {case:?}: the pages concatenate to the whole enumeration {whole:?}, got \
             {walked:?}"
        );
    }

    // The same, inside a filtered enumeration: the token is a position within THAT scan.
    for max_results in [1, 2] {
        let case = format!("maxResults {max_results} under the prefix \"accounts\"");
        let walked = walk(&logs, &case, Some("accounts"), Some(max_results)).await;
        assert_eq!(
            walked,
            vec!["accounts_eu".to_string(), "accounts_us".to_string()],
            "case {case:?}: the pages concatenate to the filtered enumeration, got {walked:?}"
        );
    }

    // A first page of one names the first table and a token; the token's page resumes AFTER
    // it rather than repeating it.
    let first = logs.ok(DESCRIBE, &request(None, Some(1), None)).await;
    assert_eq!(
        names_of(&first, "the first page of one"),
        vec![whole[0].clone()],
        "case \"the first page of one\": the first table of the enumeration is reported, got \
         {first}"
    );
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a page with tables beyond it carries a nextToken");
    let second = logs
        .ok(DESCRIBE, &request(None, Some(1), Some(token)))
        .await;
    assert_eq!(
        names_of(&second, "the page after the first"),
        vec![whole[1].clone()],
        "case \"the page after the first\": the walk resumes after the last table reported, got \
         {second}"
    );
}

/// A request that leaves `maxResults` at its modelled default gets the page size the member
/// documents — 50 — not the whole store and not an empty page.
#[tokio::test]
async fn an_unset_max_results_pages_at_the_documented_default() {
    let logs = Logs::new();

    // One table more than the default page size, so the default is observable: the names are
    // zero-padded, so their ascending order is the order they are numbered in.
    let held = DEFAULT_MAX_RESULTS + 1;
    for index in 0..held {
        logs.ok(
            CREATE,
            &json!({
                "lookupTableName": format!("page_{index:03}"),
                "tableBody": "user\nalice\n",
            }),
        )
        .await;
    }

    let page = logs.ok(DESCRIBE, &request(None, None, None)).await;
    let names = names_of(&page, "an unset maxResults");
    assert_eq!(
        names.len(),
        DEFAULT_MAX_RESULTS,
        "case \"an unset maxResults\": expected the documented default of {DEFAULT_MAX_RESULTS} \
         tables, got {}",
        names.len()
    );
    assert_eq!(
        names[0], "page_000",
        "case \"an unset maxResults\": the page starts at the first table of the enumeration, \
         got {}",
        names[0]
    );

    let walked = walk(&logs, "an unset maxResults, walked", None, None).await;
    assert_eq!(
        walked.len(),
        held,
        "case \"an unset maxResults, walked\": the pages concatenate to every held table \
         ({held}), got {}",
        walked.len()
    );
}

/// An account holding no lookup table reports an empty enumeration, not a missing resource,
/// and reading it changes nothing a later caller sees.
#[tokio::test]
async fn an_account_holding_no_table_reports_an_empty_enumeration() {
    let logs = Logs::new();

    for case in ["the first read", "the second read"] {
        let page = logs.ok(DESCRIBE, &request(None, None, None)).await;
        let names = names_of(&page, case);
        assert!(
            names.is_empty(),
            "case {case:?}: an account holding no table reports no table, got {names:?}"
        );
        assert!(
            page.get("nextToken").is_none(),
            "case {case:?}: an empty enumeration has no next set, got {page}"
        );
    }

    // And a read of a populated account is repeatable too: a listing reports, it does not
    // consume.
    seed(&logs).await;
    let first = logs.ok(DESCRIBE, &request(None, None, None)).await;
    let second = logs.ok(DESCRIBE, &request(None, None, None)).await;
    assert_eq!(
        first, second,
        "case \"two reads of one account\": a listing reports the store rather than changing \
         it, got {first} then {second}"
    );
}

/// A `nextToken` this operation did not mint for THIS enumeration is refused, and a
/// `maxResults` above the member's published maximum is refused for its bound.
#[tokio::test]
async fn a_token_or_a_bound_the_operation_did_not_issue_is_refused() {
    let logs = Logs::new();
    seed(&logs).await;

    // A token minted under one prefix names a position in THAT scan, so continuing a
    // different enumeration with it is refused.
    let page = logs
        .ok(DESCRIBE, &request(Some("accounts"), Some(1), None))
        .await;
    let foreign = page
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a first page of one under a two-table prefix carries a nextToken")
        .to_string();

    // (case, the request, the code it answers)
    let cases: Vec<(&str, Value)> = vec![
        ("a token that is not one of this operation's", request(None, None, Some("not-a-token"))),
        (
            "a token minted for another prefix",
            request(Some("users"), Some(1), Some(&foreign)),
        ),
        (
            "a token minted under a prefix, continued without one",
            request(None, Some(1), Some(&foreign)),
        ),
        (
            "a maxResults one past the published maximum",
            request(None, Some(MAX_RESULTS_CEILING + 1), None),
        ),
        (
            "a prefix outside the member's pattern",
            request(Some("not-a-valid-prefix"), None, None),
        ),
    ];

    for (case, body) in cases {
        let (code, message) = logs.refusal(DESCRIBE, &body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
    }

    // The published maximum itself is accepted: the bound is the last admitted value.
    let page = logs
        .ok(DESCRIBE, &request(None, Some(MAX_RESULTS_CEILING), None))
        .await;
    assert_eq!(
        names_of(&page, "a maxResults at the published maximum"),
        ascending_names(),
        "case \"a maxResults at the published maximum\": the whole enumeration fits one page, \
         got {page}"
    );

    // Every refusal above reported nothing, and the store is as it was.
    let walked = walk(&logs, "after the refusals", None, Some(2)).await;
    assert_eq!(
        walked,
        ascending_names(),
        "case \"after the refusals\": a refused listing changes nothing, got {walked:?}"
    );
}
