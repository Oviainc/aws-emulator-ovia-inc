// SPDX-License-Identifier: CC0-1.0

//! The ONE scope-qualified pagination-token codec every listing of this service that pages
//! a SCOPED collection resumes through ([`keyspaces::encode_page_token`] /
//! [`keyspaces::decode_page_token`]), driven end to end over the service's own `awsJson1_0`
//! wire protocol through the harness in [`crate::keyspace_table_lifecycle`].
//!
//! The rule under test: a listing's `nextToken` is the hex of `<scope>#<item>`, naming the
//! last item of the page just returned qualified by the scope it was listed within, so it
//! resumes that listing immediately after that item — and a token carrying any other scope
//! (another collection of the same kind, another listing over the same keyspace, a string
//! nothing issued) resumes nothing and is refused with the modelled `ValidationException`
//! rather than silently skipping items.
//!
//! The rows of the table are the three listings this service models over a collection a
//! keyspace holds (`ListTables`, `ListTypes` and `ListTagsForResource`), each over TWO
//! keyspaces of the one Region this suite drives, so each row's token is foreign to five
//! others and the rule is asserted over the whole cross product rather than one example.
//! `ListKeyspaces` pages what a REGION holds, and the Region half of every one of these
//! scopes is driven in [`crate::region_scoped_page_tokens`].

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, keyspace_arn, minimal_schema, settle_wait, Emulator,
};

/// The two keyspaces the table below pages.
const KEYSPACES: [&str; 2] = ["ptc_one", "ptc_two"];

/// The items every collection of every keyspace below holds — the same names in all of
/// them, deliberately: a token honoured outside the scope it was issued for would then
/// resume at a plausible point and silently skip items instead of failing loudly.
const ITEMS: [&str; 4] = ["p_alpha", "p_bravo", "p_charlie", "p_delta"];

/// One row of the table: a scope-qualified listing over one scope, the request members
/// that name the collection, the output member its page arrives in, how an item of that
/// page is named, and the scope half the listing qualifies its own tokens with.
struct Listing {
    case: String,
    operation: &'static str,
    request: Value,
    member: &'static str,
    item_name: fn(&Value) -> String,
    token_scope: String,
}

/// The `tableName` of a `TableSummary`.
///
/// This and the three namers below are shared with [`crate::region_scoped_page_tokens`],
/// which pages the same kinds of collection once per Region.
pub(crate) fn table_name(item: &Value) -> String {
    member_string(item, "tableName")
}

/// A `ListTypes` item, which is the type name itself rather than a structure.
pub(crate) fn type_name(item: &Value) -> String {
    item.as_str()
        .unwrap_or_else(|| panic!("a types item must be a string, got {item}"))
        .to_string()
}

/// The `key` of a `Tag`.
pub(crate) fn tag_key(item: &Value) -> String {
    member_string(item, "key")
}

/// The `keyspaceName` of a `KeyspaceSummary`.
pub(crate) fn keyspace_name(item: &Value) -> String {
    member_string(item, "keyspaceName")
}

fn member_string(item: &Value, member: &str) -> String {
    item.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a page item must carry `{member}`, got {item}"))
        .to_string()
}

/// The items of one page, named: an output list member a response omits because it would
/// be empty is an empty page, not a missing one.
fn page_items(listing: &Listing, body: &Value) -> Vec<String> {
    match body.get(listing.member) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.iter().map(listing.item_name).collect(),
        Some(other) => panic!(
            "case {:?}: `{}` must be a list, got {other}",
            listing.case, listing.member
        ),
    }
}

/// Call one listing with an optional page size and an optional resume token, asserting it
/// succeeded, and hand back its body.
async fn page(
    emulator: &Emulator,
    listing: &Listing,
    max_results: Option<usize>,
    token: Option<&str>,
) -> Value {
    let mut request = listing.request.clone();
    if let Some(size) = max_results {
        request["maxResults"] = json!(size);
    }
    if let Some(resume) = token {
        request["nextToken"] = json!(resume);
    }
    assert_ok(
        &listing.case,
        emulator.call(listing.operation, request).await,
    )
}

/// The `nextToken` of a page, which a truncated page always carries.
fn next_token(case: &str, body: &Value) -> String {
    body.get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a truncated page must carry a nextToken: {body}"))
        .to_string()
}

/// Create the keyspaces, tables, types and tags the table pages, and hand back the six
/// listings over them.
///
/// Everything is created in an order that is NOT the sorted one, so a resume point cannot
/// come out right from insertion order alone.
async fn seeded_listings(emulator: &Emulator) -> Vec<Listing> {
    for keyspace in KEYSPACES {
        assert_ok(
            keyspace,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
                .await,
        );
        for item in ITEMS.iter().rev() {
            assert_ok(
                &format!("{keyspace}.{item} table"),
                emulator
                    .call(
                        "CreateTable",
                        json!({
                            "keyspaceName": keyspace,
                            "tableName": item,
                            "schemaDefinition": minimal_schema("pk"),
                        }),
                    )
                    .await,
            );
            assert_ok(
                &format!("{keyspace}.{item} type"),
                emulator
                    .call(
                        "CreateType",
                        json!({
                            "keyspaceName": keyspace,
                            "typeName": item,
                            "fieldDefinitions": [{"name": "value", "type": "int"}],
                        }),
                    )
                    .await,
            );
        }
        let tags: Vec<Value> = ITEMS
            .iter()
            .rev()
            .enumerate()
            .map(|(index, key)| json!({"key": key, "value": format!("value{index}")}))
            .collect();
        assert_ok(
            &format!("{keyspace} tags"),
            emulator
                .call(
                    "TagResource",
                    json!({"resourceArn": keyspace_arn(keyspace), "tags": tags}),
                )
                .await,
        );
    }

    // Everything created above settles out of its transient status before the listings
    // run, so the pages below are of settled resources.
    tokio::time::sleep(settle_wait()).await;

    let mut listings = Vec::new();
    for keyspace in KEYSPACES {
        let arn = keyspace_arn(keyspace);
        listings.push(Listing {
            case: format!("ListTables of {keyspace}"),
            operation: "ListTables",
            request: json!({"keyspaceName": keyspace}),
            member: "tables",
            item_name: table_name,
            // `ListTables` qualifies its tokens with the kind of collection and the
            // keyspace's own ARN, hex-encoded.
            token_scope: format!("tables/{}", hex::encode(&arn)),
        });
        listings.push(Listing {
            case: format!("ListTypes of {keyspace}"),
            operation: "ListTypes",
            request: json!({"keyspaceName": keyspace}),
            member: "types",
            item_name: type_name,
            // `ListTypes` pages the same keyspace by a name of the same shape as a table's,
            // so the kind half of its scope is what keeps it apart from `ListTables`.
            token_scope: format!("types/{}", hex::encode(&arn)),
        });
        listings.push(Listing {
            case: format!("ListTagsForResource of {arn}"),
            operation: "ListTagsForResource",
            request: json!({"resourceArn": arn}),
            member: "tags",
            item_name: tag_key,
            // The tag listing's scope is the kind of collection and the resource ARN,
            // hex-encoded so the scope half cannot carry the separator whatever the ARN
            // does.
            token_scope: format!("tags/{}", hex::encode(&arn)),
        });
    }
    listings
}

/// A token this listing would have issued for `item`, as the shared codec encodes it: hex
/// of the listing's own scope, the separator, and the item.
fn token_for(listing: &Listing, item: &str) -> String {
    hex::encode(format!("{}#{item}", listing.token_scope))
}

/// RULE: every scope-qualified listing issues the SAME token for the same resume point —
/// hex of `<scope>#<item>` — and that token resumes its own listing immediately after the
/// item it names, whatever page size it was issued at.
#[tokio::test]
async fn a_page_token_names_the_resume_point_and_continues_its_own_listing() {
    let emulator = Emulator::start();
    let listings = seeded_listings(&emulator).await;

    for listing in &listings {
        let whole = page(&emulator, listing, None, None).await;
        let expected = page_items(listing, &whole);
        let mut sorted = ITEMS.map(str::to_string).to_vec();
        sorted.sort();
        assert_eq!(
            expected, sorted,
            "case {:?}: expected the whole collection {sorted:?}, got {expected:?}",
            listing.case
        );

        for size in 1..expected.len() {
            let case = format!("{} at page size {size}", listing.case);
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let body = page(&emulator, listing, Some(size), token.as_deref()).await;
                let items = page_items(listing, &body);
                seen.extend(items.clone());
                match body.get("nextToken").and_then(Value::as_str) {
                    None => break,
                    Some(issued) => {
                        let last = items
                            .last()
                            .unwrap_or_else(|| panic!("case {case:?}: a page that carries a token carries items: {body}"));
                        // The token is exactly what the shared codec encodes for that
                        // resume point, and is opaque: hex, with no resource name legible
                        // in it.
                        assert_eq!(
                            issued,
                            token_for(listing, last),
                            "case {case:?}: expected the token for the resume point {last:?}, got {issued:?}"
                        );
                        assert!(
                            issued.chars().all(|c| c.is_ascii_hexdigit()) && !issued.contains(last),
                            "case {case:?}: expected an opaque hex token, got {issued:?}"
                        );
                        // Decoding it back is what the listing does with it, and it names
                        // the item the page stopped at.
                        assert_eq!(
                            keyspaces::decode_page_token(issued, &listing.token_scope).as_deref(),
                            Some(last.as_str()),
                            "case {case:?}: expected the token to decode to {last:?}, got {issued:?}"
                        );
                        token = Some(issued.to_string());
                    }
                }
                assert!(
                    seen.len() <= expected.len(),
                    "case {case:?}: pagination did not terminate, saw {seen:?}"
                );
            }
            assert_eq!(
                seen, expected,
                "case {case:?}: expected the resumed pages to be the whole collection \
                 {expected:?}, got {seen:?}"
            );
        }
    }
}

/// RULE: a token carrying any scope but this listing's own resumes nothing and is refused
/// with the modelled `ValidationException` (HTTP 400).
///
/// The cross product of the six rows covers what the scopes have to keep apart: the same
/// listing over another collection, and another listing over the same keyspace — the pair
/// (`ListTables`, `ListTypes`) pages names of the same shape, so a token of one honoured by
/// the other would resume rather than fail.
#[tokio::test]
async fn a_page_token_from_another_scope_is_a_modelled_validation_error() {
    let emulator = Emulator::start();
    let listings = seeded_listings(&emulator).await;

    let mut issued: Vec<(String, String)> = Vec::new();
    for listing in &listings {
        let first = page(&emulator, listing, Some(1), None).await;
        issued.push((
            listing.case.clone(),
            next_token(&listing.case, &first),
        ));
    }

    for (index, listing) in listings.iter().enumerate() {
        for (origin, token) in issued.iter().enumerate() {
            if origin == index {
                continue;
            }
            let (from, token) = token;
            assert_failure(
                &format!("{} resumed with a token from {from}", listing.case),
                page_call(&emulator, listing, token).await,
                400,
                "ValidationException",
            );
        }

        // Hand-built strings: the right scope but another collection's item, an empty item
        // half, no separator at all, the resume point unencoded, and bytes that are not a
        // token in the first place.
        let hand_built = [
            (
                "the scope with an empty item half",
                hex::encode(format!("{}#", listing.token_scope)),
            ),
            (
                "the scope with no separator",
                hex::encode(&listing.token_scope),
            ),
            (
                "the resume point unencoded",
                format!("{}#{}", listing.token_scope, ITEMS[0]),
            ),
            ("not hex at all", "not-a-pagination-token".to_string()),
            ("odd-length hex", "abc".to_string()),
            ("hex of bytes that are not UTF-8", "ff".to_string()),
            ("hex of nothing", String::new()),
        ];
        for (what, token) in hand_built {
            assert_failure(
                &format!("{} resumed with {what}", listing.case),
                page_call(&emulator, listing, &token).await,
                400,
                "ValidationException",
            );
        }
    }
}

/// One listing call carrying `token`, returned unasserted so a refusal can be checked.
async fn page_call(emulator: &Emulator, listing: &Listing, token: &str) -> (u16, Value) {
    let mut request = listing.request.clone();
    request["nextToken"] = json!(token);
    emulator.call(listing.operation, request).await
}
