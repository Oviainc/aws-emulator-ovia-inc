// SPDX-License-Identifier: CC0-1.0

//! `ListKeyspaces`, driven end to end over the service's own `awsJson1_0` wire protocol
//! through the harness in [`crate::keyspace_table_lifecycle`].
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. `ListKeyspaces` reports the keyspaces that are in state — every one that a create
//!    stored and none that a delete removed — each with the ARN it was minted with and
//!    the replication it was created with, and `replicationRegions` only where the
//!    strategy is `MULTI_REGION` (`KeyspaceSummary$replicationRegions`: "if the
//!    `replicationStrategy` of the keyspace is `MULTI_REGION`, a list of replication
//!    Regions is returned").
//! 2. It pages the account in ONE total order, a page's `nextToken` resuming exactly
//!    after that page's last item, for any page size; the final page carries no token.
//! 3. It is a report: two identical calls answer identically, and a listing does not
//!    change what a later caller sees.
//! 4. A `nextToken` this operation did not issue is the modelled `ValidationException`,
//!    never a silently ignored token and never the scaffold's 501.

use serde_json::{json, Value};

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, keyspace_arn, Emulator, REGION,
};

/// `rs` values, from the model's `rs` enum (`SINGLE_REGION` | `MULTI_REGION`).
const SINGLE_REGION: &str = "SINGLE_REGION";
const MULTI_REGION: &str = "MULTI_REGION";

/// The `keyspaces` member of a successful `ListKeyspaces` body.
fn summaries(body: &Value) -> Vec<Value> {
    body["keyspaces"]
        .as_array()
        .unwrap_or_else(|| panic!("ListKeyspaces must report a `keyspaces` list, got {body}"))
        .clone()
}

/// The `keyspaceName`s of a page, in the order the page reports them.
fn names(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["keyspaceName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Walk every page of `ListKeyspaces` at `page_size`, returning the names in page order
/// and the number of pages it took. Asserts the token/size invariants on the way.
async fn walk_pages(emulator: &Emulator, page_size: usize, total: usize) -> (Vec<String>, usize) {
    let case = format!("page size {page_size}");
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({"maxResults": page_size});
        if let Some(resume) = &token {
            request["nextToken"] = json!(resume);
        }
        let page = assert_ok(&case, emulator.call("ListKeyspaces", request).await);
        pages += 1;
        assert!(
            pages <= total + 1,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        let page_names = names(&page);
        assert!(
            page_names.len() <= page_size,
            "case {case:?}: page {pages} carries {} items, more than the requested {page_size}",
            page_names.len()
        );
        seen.extend(page_names.clone());
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => {
                // A token is issued only when more follows, so a page that carries one is
                // a FULL page.
                assert_eq!(
                    page_names.len(),
                    page_size,
                    "case {case:?}: page {pages} carries a nextToken but is not full: {page}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    (seen, pages)
}

/// RULE 1: every stored keyspace is reported with what it was created with, and
/// `replicationRegions` appears exactly for a `MULTI_REGION` keyspace.
#[tokio::test]
async fn list_keyspaces_reports_each_stored_keyspace_with_its_replication() {
    /// One row: the keyspace name, the `replicationSpecification` to create it with (or
    /// `Value::Null` for none at all), and the summary `ListKeyspaces` must report.
    struct Case {
        name: &'static str,
        specification: Value,
        strategy: &'static str,
        regions: Option<Vec<&'static str>>,
    }

    let cases = vec![
        // No replicationSpecification at all: the keyspace is single-Region, so no
        // region list is reported.
        Case {
            name: "default_ks",
            specification: Value::Null,
            strategy: SINGLE_REGION,
            regions: None,
        },
        // The strategy named explicitly, same answer.
        Case {
            name: "explicit_single_ks",
            specification: json!({"replicationStrategy": SINGLE_REGION}),
            strategy: SINGLE_REGION,
            regions: None,
        },
        // MULTI_REGION at the modelled minimum region count (`RegionList` len 2..).
        Case {
            name: "multi_two_ks",
            specification: json!({
                "replicationStrategy": MULTI_REGION,
                "regionList": [REGION, "us-east-1"],
            }),
            strategy: MULTI_REGION,
            regions: Some(vec![REGION, "us-east-1"]),
        },
        // MULTI_REGION with more than the minimum, and in an order the service keeps.
        Case {
            name: "multi_three_ks",
            specification: json!({
                "replicationStrategy": MULTI_REGION,
                "regionList": ["eu-west-1", REGION, "ap-south-1"],
            }),
            strategy: MULTI_REGION,
            regions: Some(vec!["eu-west-1", REGION, "ap-south-1"]),
        },
    ];

    let emulator = Emulator::start();

    // An account with no keyspaces is an empty list, not a missing member and not a token.
    let empty = assert_ok(
        "no keyspaces yet",
        emulator.call("ListKeyspaces", json!({})).await,
    );
    assert_eq!(
        empty["keyspaces"],
        json!([]),
        "case {:?}: expected an empty keyspaces list, got {empty}",
        "no keyspaces yet"
    );
    assert_eq!(
        empty.get("nextToken"),
        None,
        "case {:?}: a complete page carries no nextToken, got {empty}",
        "no keyspaces yet"
    );

    for case in &cases {
        let mut request = json!({"keyspaceName": case.name});
        if !case.specification.is_null() {
            request["replicationSpecification"] = case.specification.clone();
        }
        assert_ok(case.name, emulator.call("CreateKeyspace", request).await);
    }

    let listed = assert_ok(
        "all keyspaces",
        emulator.call("ListKeyspaces", json!({})).await,
    );
    let reported = summaries(&listed);
    assert_eq!(
        reported.len(),
        cases.len(),
        "case {:?}: expected {} keyspaces, got {listed}",
        "all keyspaces",
        cases.len()
    );

    for case in &cases {
        let mut expected = json!({
            "keyspaceName": case.name,
            "resourceArn": keyspace_arn(case.name),
            "replicationStrategy": case.strategy,
        });
        if let Some(regions) = &case.regions {
            expected["replicationRegions"] = json!(regions);
        }
        let actual = reported
            .iter()
            .find(|summary| summary["keyspaceName"] == json!(case.name))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: ListKeyspaces did not report the keyspace: {listed}",
                    case.name
                )
            });
        assert_eq!(
            actual, &expected,
            "case {:?}: expected the summary {expected}, got {actual}",
            case.name
        );
        if case.regions.is_none() {
            assert_eq!(
                actual.get("replicationRegions"),
                None,
                "case {:?}: a single-Region keyspace reports no replicationRegions, got {actual}",
                case.name
            );
        }
    }

    // RULE 1's other half: a keyspace removed from state is no longer reported, and the
    // rest of the listing is untouched.
    let removed = cases[0].name;
    assert_ok(
        removed,
        emulator
            .call("DeleteKeyspace", json!({"keyspaceName": removed}))
            .await,
    );
    let after_delete = assert_ok(
        "after the delete",
        emulator.call("ListKeyspaces", json!({})).await,
    );
    let mut expected_rest: Vec<String> = cases
        .iter()
        .map(|case| case.name.to_string())
        .filter(|name| name != removed)
        .collect();
    expected_rest.sort();
    assert_eq!(
        names(&after_delete),
        expected_rest,
        "case {:?}: expected {expected_rest:?}, got {after_delete}",
        "after the delete"
    );
}

/// RULE 2 and RULE 3: one total order, resumed exactly, for every page size — and the
/// same call twice answers the same thing.
#[tokio::test]
async fn list_keyspaces_pages_the_account_in_one_total_order_for_any_page_size() {
    let emulator = Emulator::start();

    // Created in an order that is NOT the order they must be reported in, so a listing
    // that echoed insertion order would fail.
    let created = ["mid_ks", "zulu_ks", "alpha_ks", "nova_ks", "bravo_ks"];
    for name in created {
        assert_ok(
            name,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": name}))
                .await,
        );
    }
    let mut expected: Vec<String> = created.iter().map(|name| name.to_string()).collect();
    expected.sort();

    // Page sizes that divide the collection evenly, ones that do not, the smallest the
    // model allows (`maxResults` range 1..1000), and sizes at and past the collection.
    for page_size in [1usize, 2, 3, 4, 5, 6, 1000] {
        let case = format!("page size {page_size}");
        let (seen, pages) = walk_pages(&emulator, page_size, expected.len()).await;
        assert_eq!(
            seen, expected,
            "case {case:?}: expected {expected:?} across {pages} pages, got {seen:?}"
        );
        let expected_pages = expected.len().div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }

    // RULE 3: a report changes nothing. The same first page, asked twice, is identical,
    // and the full listing afterwards is still the whole account.
    let first = assert_ok(
        "first page, once",
        emulator
            .call("ListKeyspaces", json!({"maxResults": 2}))
            .await,
    );
    let again = assert_ok(
        "first page, twice",
        emulator
            .call("ListKeyspaces", json!({"maxResults": 2}))
            .await,
    );
    assert_eq!(
        first, again,
        "case {:?}: a read must not change what a later caller sees: {first} then {again}",
        "first page twice"
    );
    let whole = assert_ok(
        "whole account after the reads",
        emulator.call("ListKeyspaces", json!({})).await,
    );
    assert_eq!(
        names(&whole),
        expected,
        "case {:?}: expected {expected:?}, got {whole}",
        "whole account after the reads"
    );
}

/// RULE 4: a `nextToken` this operation did not issue is refused with the modelled
/// `ValidationException`, and the token it DID issue is accepted — so the refusal is the
/// rule, not a blanket no.
#[tokio::test]
async fn a_next_token_this_operation_did_not_issue_is_a_modelled_validation_error() {
    let emulator = Emulator::start();
    for name in ["token_ks_a", "token_ks_b"] {
        assert_ok(
            name,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": name}))
                .await,
        );
    }
    assert_ok(
        "create a table to mint a ListTables token",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": "token_ks_a",
                    "tableName": "tbl_one",
                    "schemaDefinition": {
                        "allColumns": [{"name": "id", "type": "text"}],
                        "partitionKeys": [{"name": "id"}],
                    },
                }),
            )
            .await,
    );
    assert_ok(
        "a second table, so ListTables truncates",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": "token_ks_a",
                    "tableName": "tbl_two",
                    "schemaDefinition": {
                        "allColumns": [{"name": "id", "type": "text"}],
                        "partitionKeys": [{"name": "id"}],
                    },
                }),
            )
            .await,
    );
    let table_page = assert_ok(
        "first page of ListTables",
        emulator
            .call(
                "ListTables",
                json!({"keyspaceName": "token_ks_a", "maxResults": 1}),
            )
            .await,
    );
    let table_token = table_page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("ListTables must truncate at maxResults 1: {table_page}"))
        .to_string();

    // Each row: the case, and a token no `ListKeyspaces` page ever handed out. A token
    // this operation issued always decodes to a `keyspaceName` (len 1..48, pattern
    // `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`), so every row below is outside that.
    let foreign: Vec<(&str, String)> = vec![
        ("a ListTables token", table_token),
        ("not hex at all", "not-a-pagination-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of a byte string that is not UTF-8", "ff".to_string()),
        (
            "hex of a keyspace-qualified name",
            hex_of("token_ks_a#tbl_one"),
        ),
        // Outside the modelled `keyspaceName` pattern: an illegal character, an illegal
        // first character, and one past the published length bound of 48.
        ("hex of a name with a dash", hex_of("token-ks-a")),
        ("hex of a name starting with _", hex_of("_token_ks_a")),
        ("hex of a name one past the length bound", hex_of(&"k".repeat(49))),
    ];
    for (case, token) in foreign {
        assert_failure(
            case,
            emulator
                .call("ListKeyspaces", json!({"nextToken": token}))
                .await,
            400,
            "ValidationException",
        );
    }

    // And the token this operation issues resumes the listing it came from.
    let first = assert_ok(
        "first page",
        emulator
            .call("ListKeyspaces", json!({"maxResults": 1}))
            .await,
    );
    let own_token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("ListKeyspaces must truncate at maxResults 1: {first}"))
        .to_string();
    let second = assert_ok(
        "second page",
        emulator
            .call("ListKeyspaces", json!({"nextToken": own_token}))
            .await,
    );
    assert_eq!(
        names(&second),
        vec!["token_ks_b".to_string()],
        "case {:?}: expected the listing to resume after the first page, got {second}",
        "second page"
    );
}

/// Hex of a UTF-8 string, written here so this test does not depend on the service's own
/// token encoder: a token the test built by hand is exactly what must be refused.
fn hex_of(value: &str) -> String {
    value.bytes().map(|byte| format!("{byte:02x}")).collect()
}
