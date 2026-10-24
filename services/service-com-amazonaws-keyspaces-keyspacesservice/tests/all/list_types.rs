// SPDX-License-Identifier: CC0-1.0

//! `ListTypes`, driven end to end over the service's own `awsJson1_0` wire protocol
//! through the harness in [`crate::keyspace_table_lifecycle`].
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. `ListTypes` reports the user-defined types the NAMED keyspace holds, each under the
//!    FORMATTED name every other operation takes (`GetType$typeName`), whatever status the
//!    type is in; a keyspace that holds none reports the empty list, because `types` is a
//!    required output member.
//! 2. It pages one keyspace in ONE total order, a page's `nextToken` resuming exactly
//!    after that page's last item, for any page size; the final page carries no token.
//! 3. It is a report: two identical calls answer identically, and a listing does not
//!    change what a later caller sees.
//! 4. A keyspace that is not in state is the modelled `ResourceNotFoundException`, and a
//!    `nextToken` this operation did not issue for this keyspace is the modelled
//!    `ValidationException` — never a silently ignored token and never the scaffold's 501.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, keyspace_arn, minimal_schema, settle_wait, Emulator,
};

/// One row of the name table: the `typeName` as submitted to `CreateType`, and the
/// formatted name `ListTypes` must report it under.
struct Name {
    submitted: &'static str,
    formatted: String,
}

/// The name forms a UDT may take, spanning `CreateType$typeName`'s two documented forms
/// and the modelled `TypeName` length bounds (`len 1..48`): the unquoted form (already
/// lower case, mixed case folded down, digits and underscores, the minimum length and the
/// maximum length) and the double-quoted form (casing preserved, a space, and the `#` and
/// `/` characters a pagination token uses as separators).
fn names() -> Vec<Name> {
    let longest = "T".repeat(48);
    vec![
        Name {
            submitted: "address",
            formatted: "address".to_string(),
        },
        Name {
            submitted: "Address_V2",
            formatted: "address_v2".to_string(),
        },
        Name {
            submitted: "t",
            formatted: "t".to_string(),
        },
        Name {
            submitted: "TTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTT",
            formatted: longest.to_ascii_lowercase(),
        },
        Name {
            submitted: "\"MixedCase\"",
            formatted: "\"MixedCase\"".to_string(),
        },
        Name {
            submitted: "\"with space!\"",
            formatted: "\"with space!\"".to_string(),
        },
        Name {
            submitted: "\"has#hash/slash\"",
            formatted: "\"has#hash/slash\"".to_string(),
        },
    ]
}

/// The `types` member of a successful `ListTypes` body.
fn types(body: &Value) -> Vec<String> {
    body["types"]
        .as_array()
        .unwrap_or_else(|| panic!("ListTypes must report a `types` list, got {body}"))
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Create a keyspace and hand its name back.
async fn keyspace(emulator: &Emulator, name: &'static str) -> &'static str {
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": name}))
            .await,
    );
    name
}

/// Create one flat type in `keyspace_name` and hand back the formatted name `CreateType`
/// answered with.
async fn create_type(emulator: &Emulator, keyspace_name: &str, submitted: &str) -> String {
    let created = assert_ok(
        submitted,
        emulator
            .call(
                "CreateType",
                json!({
                    "keyspaceName": keyspace_name,
                    "typeName": submitted,
                    "fieldDefinitions": [{"name": "value", "type": "int"}],
                }),
            )
            .await,
    );
    created["typeName"]
        .as_str()
        .unwrap_or_else(|| panic!("case {submitted:?}: CreateType must answer a typeName: {created}"))
        .to_string()
}

/// Walk every page of `ListTypes` for one keyspace at `page_size`, returning the names in
/// page order and the number of pages it took. Asserts the token/size invariants on the
/// way.
async fn walk_pages(
    emulator: &Emulator,
    keyspace_name: &str,
    page_size: usize,
    total: usize,
) -> (Vec<String>, usize) {
    let case = format!("page size {page_size}");
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({"keyspaceName": keyspace_name, "maxResults": page_size});
        if let Some(resume) = &token {
            request["nextToken"] = json!(resume);
        }
        let page = assert_ok(&case, emulator.call("ListTypes", request).await);
        pages += 1;
        assert!(
            pages <= total + 1,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        let page_names = types(&page);
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

/// RULE 1: the listing is exactly the named keyspace's types, under their formatted
/// names, in one total order — and every name it reports is a name `GetType` resolves.
#[tokio::test]
async fn list_types_reports_the_keyspaces_types_under_their_formatted_names() {
    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "list_types_ks").await;
    let other = keyspace(&emulator, "list_types_other_ks").await;

    // A keyspace that holds no type reports the EMPTY list, not a missing member and not
    // a token: `types` is a required output member. The rows span a keyspace the caller
    // just created and a service-managed keyspace, which holds tables but no UDT.
    for empty in [ks, "system_schema"] {
        let listed = assert_ok(empty, emulator.call("ListTypes", json!({"keyspaceName": empty})).await);
        assert_eq!(
            types(&listed),
            Vec::<String>::new(),
            "case {empty:?}: a keyspace with no types reports an empty list, got {listed}"
        );
        assert_eq!(
            listed.get("nextToken"),
            None,
            "case {empty:?}: a complete page carries no nextToken, got {listed}"
        );
    }

    // Created in an order that is NOT the order they must be reported in, so a listing
    // that echoed insertion order would fail.
    let mut expected: Vec<String> = Vec::new();
    for name in names().iter().rev() {
        let formatted = create_type(&emulator, ks, name.submitted).await;
        assert_eq!(
            formatted, name.formatted,
            "case {:?}: CreateType must store the formatted name, got {formatted:?}",
            name.submitted
        );
        expected.push(formatted);
    }
    expected.sort();

    // A type of ANOTHER keyspace is not part of this keyspace's listing.
    let foreign = create_type(&emulator, other, "only_in_the_other_keyspace").await;

    // Read straight after the creates, while every type is still in its transient
    // `CREATING` status: a listing reports what the keyspace HOLDS, not only what has
    // settled.
    let while_creating = assert_ok(
        "immediately after the creates",
        emulator.call("ListTypes", json!({"keyspaceName": ks})).await,
    );
    assert_eq!(
        types(&while_creating),
        expected,
        "case {:?}: expected {expected:?}, got {while_creating}",
        "immediately after the creates"
    );

    tokio::time::sleep(settle_wait()).await;
    let settled = assert_ok(
        "after the types settle",
        emulator.call("ListTypes", json!({"keyspaceName": ks})).await,
    );
    assert_eq!(
        types(&settled),
        expected,
        "case {:?}: expected {expected:?}, got {settled}",
        "after the types settle"
    );

    let other_listed = assert_ok(
        "the other keyspace",
        emulator
            .call("ListTypes", json!({"keyspaceName": other}))
            .await,
    );
    assert_eq!(
        types(&other_listed),
        vec![foreign.clone()],
        "case {:?}: expected only {foreign:?}, got {other_listed}",
        "the other keyspace"
    );

    // Every name the listing reports is the formatted name `GetType` takes, so a client
    // can page a keyspace and then read each type it was told about.
    for name in types(&settled) {
        let got = assert_ok(
            &name,
            emulator
                .call(
                    "GetType",
                    json!({"keyspaceName": ks, "typeName": name.clone()}),
                )
                .await,
        );
        assert_eq!(
            (&got["keyspaceName"], &got["typeName"]),
            (&json!(ks), &json!(name)),
            "case {name:?}: the listed name must resolve through GetType, got {got}"
        );
    }

    // A table of the keyspace is not a type: `ListTypes` and `ListTables` report
    // different collections of the same keyspace.
    assert_ok(
        "create a table beside the types",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": ks,
                    "tableName": "not_a_type",
                    "schemaDefinition": minimal_schema("id"),
                }),
            )
            .await,
    );
    let after_table = assert_ok(
        "after a table is created",
        emulator.call("ListTypes", json!({"keyspaceName": ks})).await,
    );
    assert_eq!(
        types(&after_table),
        expected,
        "case {:?}: a table must not appear among the types, got {after_table}",
        "after a table is created"
    );
}

/// RULES 2 and 3: one total order, resumed exactly, for every page size — and the same
/// call twice answers the same thing.
#[tokio::test]
async fn list_types_pages_a_keyspace_in_one_total_order_for_any_page_size() {
    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "paged_types_ks").await;

    let mut expected: Vec<String> = Vec::new();
    for name in names().iter().rev() {
        expected.push(create_type(&emulator, ks, name.submitted).await);
    }
    expected.sort();

    // Page sizes that divide the collection evenly, ones that do not, the smallest the
    // model allows (`maxResults` range 1..1000), sizes at and past the collection, and
    // the published maximum.
    for page_size in [1usize, 2, 3, 4, 5, 6, 7, 8, 1000] {
        let case = format!("page size {page_size}");
        let (seen, pages) = walk_pages(&emulator, ks, page_size, expected.len()).await;
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
    // and the full listing afterwards is still the whole keyspace.
    let first = assert_ok(
        "first page, once",
        emulator
            .call("ListTypes", json!({"keyspaceName": ks, "maxResults": 2}))
            .await,
    );
    let again = assert_ok(
        "first page, twice",
        emulator
            .call("ListTypes", json!({"keyspaceName": ks, "maxResults": 2}))
            .await,
    );
    assert_eq!(
        first, again,
        "case {:?}: a read must not change what a later caller sees: {first} then {again}",
        "first page twice"
    );
    let whole = assert_ok(
        "whole keyspace after the reads",
        emulator.call("ListTypes", json!({"keyspaceName": ks})).await,
    );
    assert_eq!(
        types(&whole),
        expected,
        "case {:?}: expected {expected:?}, got {whole}",
        "whole keyspace after the reads"
    );

    // An absent `maxResults` is the whole keyspace in the same order, with no token.
    assert_eq!(
        whole.get("nextToken"),
        None,
        "case {:?}: a complete page carries no nextToken, got {whole}",
        "whole keyspace after the reads"
    );
}

/// RULE 4: a keyspace that is not in state is the modelled `ResourceNotFoundException`,
/// naming the keyspace that was looked for.
#[tokio::test]
async fn listing_the_types_of_an_absent_keyspace_is_a_modelled_not_found() {
    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "present_types_ks").await;
    create_type(&emulator, ks, "present").await;

    // Rows: a keyspace nothing ever created, and one that WAS created and then deleted —
    // so the refusal tracks state rather than a fixed list of names.
    assert_ok(
        "delete the keyspace",
        emulator
            .call("DeleteKeyspace", json!({"keyspaceName": ks}))
            .await,
    );
    for case in ["never_created_ks", ks] {
        let (status, body) = emulator
            .call("ListTypes", json!({"keyspaceName": case}))
            .await;
        assert_failure(
            case,
            (status, body.clone()),
            404,
            "ResourceNotFoundException",
        );
        assert_eq!(
            body["resourceArn"],
            json!(keyspace_arn(case)),
            "case {case:?}: the exception must name the resource that was looked for: {body}"
        );
    }
}

/// RULE 4: a `nextToken` this operation did not issue for this keyspace is refused with
/// the modelled `ValidationException`, and the token it DID issue resumes the listing —
/// so the refusal is the rule, not a blanket no.
#[tokio::test]
async fn a_next_token_this_operation_did_not_issue_is_a_modelled_validation_error() {
    let emulator = Emulator::start();
    let ks = keyspace(&emulator, "token_types_ks").await;
    let other = keyspace(&emulator, "token_types_other_ks").await;

    let mut expected: Vec<String> = Vec::new();
    for name in names().iter().rev() {
        expected.push(create_type(&emulator, ks, name.submitted).await);
    }
    expected.sort();
    create_type(&emulator, other, "elsewhere").await;

    // A token minted by the two other paginated listings of the same account: pagination
    // is per-collection, so neither resumes a type listing.
    for table in ["tbl_one", "tbl_two"] {
        assert_ok(
            table,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": ks,
                        "tableName": table,
                        "schemaDefinition": minimal_schema("id"),
                    }),
                )
                .await,
        );
    }
    let table_page = assert_ok(
        "first page of ListTables",
        emulator
            .call("ListTables", json!({"keyspaceName": ks, "maxResults": 1}))
            .await,
    );
    let table_token = table_page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("ListTables must truncate at maxResults 1: {table_page}"))
        .to_string();
    let keyspace_page = assert_ok(
        "first page of ListKeyspaces",
        emulator.call("ListKeyspaces", json!({"maxResults": 1})).await,
    );
    let keyspace_token = keyspace_page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("ListKeyspaces must truncate at maxResults 1: {keyspace_page}"))
        .to_string();
    // A token this operation issued for ANOTHER keyspace of the same account.
    let other_page = assert_ok(
        "first page of the other keyspace",
        emulator
            .call(
                "ListTypes",
                json!({"keyspaceName": other, "maxResults": 1}),
            )
            .await,
    );
    assert_eq!(
        other_page.get("nextToken"),
        None,
        "case {:?}: the other keyspace holds one type, so its only page is complete: {other_page}",
        "first page of the other keyspace"
    );

    let foreign: Vec<(&str, String)> = vec![
        ("a ListTables token", table_token),
        ("a ListKeyspaces token", keyspace_token),
        ("not hex at all", "not-a-pagination-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of a byte string that is not UTF-8", "ff".to_string()),
        // Hand-built tokens: the right scope but another keyspace, the right keyspace but
        // no type, no scope at all, and the scope spelled differently.
        (
            "hex of another keyspace's resume point",
            hex_of("types/token_types_other_ks#elsewhere"),
        ),
        ("hex of an empty type name", hex_of("types/token_types_ks#")),
        ("hex with no separator", hex_of("types/token_types_ks")),
        ("hex with no scope", hex_of("token_types_ks#address")),
        (
            "hex of another collection's scope",
            hex_of("tables/token_types_ks#address"),
        ),
    ];
    for (case, token) in foreign {
        assert_failure(
            case,
            emulator
                .call(
                    "ListTypes",
                    json!({"keyspaceName": ks, "nextToken": token}),
                )
                .await,
            400,
            "ValidationException",
        );
    }

    // And the token this operation issues resumes the listing it came from.
    let first = assert_ok(
        "first page",
        emulator
            .call("ListTypes", json!({"keyspaceName": ks, "maxResults": 1}))
            .await,
    );
    let own_token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("ListTypes must truncate at maxResults 1: {first}"))
        .to_string();
    let second = assert_ok(
        "second page",
        emulator
            .call(
                "ListTypes",
                json!({"keyspaceName": ks, "nextToken": own_token}),
            )
            .await,
    );
    let after_first: Vec<String> = expected.iter().skip(1).cloned().collect();
    assert_eq!(
        types(&second),
        after_first,
        "case {:?}: expected the listing to resume after the first page, got {second}",
        "second page"
    );

    // The keyspace is still within its type cap, so none of the above was refused for a
    // reason other than the token.
    assert!(
        expected.len() < keyspaces::MAX_TYPES_PER_KEYSPACE,
        "case {:?}: the fixture must stay under the per-keyspace type cap, got {} types",
        "token fixture",
        expected.len()
    );
}

/// Hex of a UTF-8 string, written here so this test does not depend on the service's own
/// token encoder: a token the test built by hand is exactly what must be refused.
fn hex_of(value: &str) -> String {
    value.bytes().map(|byte| format!("{byte:02x}")).collect()
}
