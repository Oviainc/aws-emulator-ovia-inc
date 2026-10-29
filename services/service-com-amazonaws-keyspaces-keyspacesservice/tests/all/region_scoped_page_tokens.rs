// SPDX-License-Identifier: CC0-1.0

//! Every collection this service pages belongs to ONE Amazon Web Services Region, so a
//! `nextToken` is Region-qualified — driven end to end over the service's own `awsJson1_0`
//! wire protocol with a credential scope per request, through the multi-Region harness in
//! [`crate::regional_partitioning`].
//!
//! The rule under test: a listing's `nextToken` resumes the ONE collection it was issued
//! for, and the Region is part of what names that collection — the same `keyspaceName`
//! names a different keyspace, holding a different set of tables, types and tags, in
//! another Region, and each Region holds keyspaces of its own — so a token issued over one
//! Region's collection is refused with the modelled `ValidationException` (HTTP 400) by a
//! listing signed for another Region, rather than resuming that other collection at a
//! resume point that silently skips the items sorting at or before it.
//!
//! The rows are every `@paginated` listing this service models (`ListKeyspaces`,
//! `ListTables`, `ListTypes` and `ListTagsForResource`, as `smithy_query(query="digest")`
//! lists them) over two keyspaces holding different numbers of items, crossed with every
//! ORDERED pair of the Regions below — so the refusal is asserted in both directions over
//! four kinds of collection rather than one example, alongside tokens of the same
//! collection that name no Region at all. Each Region's listing is then walked with its OWN
//! tokens at every page size, which is what shows the refusal is the rule and not a blanket
//! no.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{assert_failure, assert_ok, minimal_schema, settle_wait};
use crate::page_token_codec::{keyspace_name, table_name, tag_key, type_name};
use crate::regional_partitioning::{keyspace_arn_in, RegionalEmulator};

/// The Regions every row below is created in, in three different geographies: a token of
/// each is handed to each of the others.
const REGIONS: [&str; 3] = ["us-west-2", "eu-central-1", "ap-southeast-1"];

/// The keyspaces every Region creates, with the items each collection of each holds.
///
/// The SAME names in every Region, deliberately: a token honoured outside the Region it was
/// issued for would then resume at a plausible point and silently skip items instead of
/// failing loudly. The two keyspaces hold different numbers of items, so a page boundary
/// falls in a different place in each.
const KEYSPACES: [(&str, &[&str]); 2] = [
    (
        "rst_app",
        &["r_alpha", "r_bravo", "r_charlie", "r_delta"] as &[&str],
    ),
    ("rst_ledger", &["r_echo", "r_fox"] as &[&str]),
];

/// One collection of ONE Region: the listing that pages it, the request that names it to a
/// client signed for that Region, where its page arrives, how an item of that page is
/// named, the items it holds in the one order a listing reports them, and tokens of this
/// same collection that leave its Region out.
struct Collection {
    case: String,
    region: &'static str,
    operation: &'static str,
    request: Value,
    member: &'static str,
    item_name: fn(&Value) -> String,
    expected: Vec<String>,
    region_blind: Vec<(String, String)>,
}

/// The items of one page, named: an output list member a response omits because it would be
/// empty is an empty page, not a missing one.
fn page_items(collection: &Collection, body: &Value) -> Vec<String> {
    match body.get(collection.member) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.iter().map(collection.item_name).collect(),
        Some(other) => panic!(
            "case {:?}: `{}` must be a list, got {other}",
            collection.case, collection.member
        ),
    }
}

/// One listing call carrying an optional page size and an optional resume token, returned
/// unasserted so a refusal can be checked.
async fn page_call(
    emulator: &RegionalEmulator,
    collection: &Collection,
    max_results: Option<usize>,
    token: Option<&str>,
) -> (u16, Value) {
    let mut request = collection.request.clone();
    if let Some(size) = max_results {
        request["maxResults"] = json!(size);
    }
    if let Some(resume) = token {
        request["nextToken"] = json!(resume);
    }
    emulator
        .call(collection.region, collection.operation, request)
        .await
}

/// One listing call, asserted to have succeeded.
async fn page(
    emulator: &RegionalEmulator,
    collection: &Collection,
    max_results: Option<usize>,
    token: Option<&str>,
) -> Value {
    assert_ok(
        &collection.case,
        page_call(emulator, collection, max_results, token).await,
    )
}

/// The `nextToken` of a page, which a truncated page always carries.
fn next_token(case: &str, body: &Value) -> String {
    body.get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a truncated page must carry a nextToken: {body}"))
        .to_string()
}

/// The names of the service-managed keyspaces every Region holds, which every Region's
/// `ListKeyspaces` reports beside what was created in it.
fn service_managed_names() -> Vec<String> {
    keyspaces::SERVICE_MANAGED_KEYSPACES
        .iter()
        .map(|(keyspace, _)| (*keyspace).to_string())
        .collect()
}

/// Create, in EVERY Region, keyspaces of the same names holding tables, types and tags of
/// the same names, and hand back the collections they make up — grouped into FAMILIES, one
/// per kind of collection, each holding that kind's collection in each Region. A family is
/// exactly the set a token must not cross.
async fn seeded_families(emulator: &RegionalEmulator) -> Vec<Vec<Collection>> {
    for region in REGIONS {
        for (keyspace, items) in KEYSPACES {
            assert_ok(
                &format!("CreateKeyspace {keyspace} signed for {region}"),
                emulator
                    .call(region, "CreateKeyspace", json!({"keyspaceName": keyspace}))
                    .await,
            );
            // Created in an order that is NOT the sorted one, so a resume point cannot come
            // out right from insertion order alone.
            for item in items.iter().rev() {
                assert_ok(
                    &format!("CreateTable {keyspace}.{item} signed for {region}"),
                    emulator
                        .call(
                            region,
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
                    &format!("CreateType {keyspace}.{item} signed for {region}"),
                    emulator
                        .call(
                            region,
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
            let tags: Vec<Value> = items
                .iter()
                .rev()
                .enumerate()
                .map(|(index, key)| json!({"key": key, "value": format!("value{index}")}))
                .collect();
            assert_ok(
                &format!("TagResource {keyspace} signed for {region}"),
                emulator
                    .call(
                        region,
                        "TagResource",
                        json!({
                            "resourceArn": keyspace_arn_in(region, keyspace),
                            "tags": tags,
                        }),
                    )
                    .await,
            );
        }
    }

    // Everything created above settles out of its transient status before the listings run,
    // so the pages below are of settled resources.
    tokio::time::sleep(settle_wait()).await;

    let mut families: Vec<Vec<Collection>> = Vec::new();
    for (keyspace, items) in KEYSPACES {
        let mut sorted: Vec<String> = items.iter().map(|item| (*item).to_string()).collect();
        sorted.sort();
        let first = sorted
            .first()
            .expect("every keyspace of the table holds items")
            .clone();

        families.push(
            REGIONS
                .iter()
                .copied()
                .map(|region| Collection {
                    case: format!("ListTables of {keyspace} signed for {region}"),
                    region,
                    operation: "ListTables",
                    request: json!({"keyspaceName": keyspace}),
                    member: "tables",
                    item_name: table_name,
                    expected: sorted.clone(),
                    region_blind: vec![
                        (
                            "the keyspace name with no Region in it".to_string(),
                            hex::encode(format!("{keyspace}#{first}")),
                        ),
                        (
                            "this listing's own scope over a Region-blind keyspace name"
                                .to_string(),
                            hex::encode(format!(
                                "tables/{}#{first}",
                                hex::encode(keyspace)
                            )),
                        ),
                    ],
                })
                .collect(),
        );
        families.push(
            REGIONS
                .iter()
                .copied()
                .map(|region| Collection {
                    case: format!("ListTypes of {keyspace} signed for {region}"),
                    region,
                    operation: "ListTypes",
                    request: json!({"keyspaceName": keyspace}),
                    member: "types",
                    item_name: type_name,
                    expected: sorted.clone(),
                    region_blind: vec![
                        (
                            "the keyspace name with no Region in it".to_string(),
                            hex::encode(format!("types/{keyspace}#{first}")),
                        ),
                        (
                            "this listing's own scope over a Region-blind keyspace name"
                                .to_string(),
                            hex::encode(format!("types/{}#{first}", hex::encode(keyspace))),
                        ),
                    ],
                })
                .collect(),
        );
        families.push(
            REGIONS
                .iter()
                .copied()
                .map(|region| Collection {
                    case: format!("ListTagsForResource of {keyspace} signed for {region}"),
                    region,
                    operation: "ListTagsForResource",
                    request: json!({"resourceArn": keyspace_arn_in(region, keyspace)}),
                    member: "tags",
                    item_name: tag_key,
                    expected: sorted.clone(),
                    region_blind: vec![
                        (
                            "the resource ARN with no kind of collection in it".to_string(),
                            hex::encode(format!(
                                "{}#{first}",
                                hex::encode(keyspace_arn_in(region, keyspace))
                            )),
                        ),
                        (
                            "this listing's own scope over the resource path without its \
                             Region"
                                .to_string(),
                            hex::encode(format!(
                                "tags/{}#{first}",
                                hex::encode(format!("/keyspace/{keyspace}/"))
                            )),
                        ),
                    ],
                })
                .collect(),
        );
    }

    // What a REGION holds: the keyspaces created in it and the service-managed catalog it
    // holds before any call — the same set of names in every Region of the table, so only
    // the Region tells the three collections apart.
    let mut held = service_managed_names();
    held.extend(KEYSPACES.iter().map(|(keyspace, _)| (*keyspace).to_string()));
    held.sort();
    let first_held = held
        .first()
        .expect("every Region holds the service-managed catalog")
        .clone();
    families.push(
        REGIONS
            .iter()
            .copied()
            .map(|region| Collection {
                case: format!("ListKeyspaces signed for {region}"),
                region,
                operation: "ListKeyspaces",
                request: json!({}),
                member: "keyspaces",
                item_name: keyspace_name,
                expected: held.clone(),
                region_blind: vec![
                    (
                        "a keyspace name alone".to_string(),
                        hex::encode(&first_held),
                    ),
                    (
                        "this listing's own scope with no Region in it".to_string(),
                        hex::encode(format!("keyspaces/#{first_held}")),
                    ),
                ],
            })
            .collect(),
    );
    families
}

/// RULE: a `nextToken` issued over one Region's collection is refused with the modelled
/// `ValidationException` by a listing of the same-named collection in another Region, as is
/// any token of that collection that names no Region.
#[tokio::test]
async fn a_page_token_from_another_region_is_a_modelled_validation_error() {
    let emulator = RegionalEmulator::start();
    let families = seeded_families(&emulator).await;

    for family in &families {
        // One token per Region, each a resume point INSIDE a collection every Region of the
        // family holds items of: honoured elsewhere it would resume rather than fail.
        let mut issued: Vec<(String, String)> = Vec::new();
        for collection in family {
            let first = page(&emulator, collection, Some(1), None).await;
            issued.push((
                collection.case.clone(),
                next_token(&collection.case, &first),
            ));
        }

        for (index, collection) in family.iter().enumerate() {
            for (origin, (from, token)) in issued.iter().enumerate() {
                if origin == index {
                    continue;
                }
                assert_failure(
                    &format!("{} resumed with the token from {from}", collection.case),
                    page_call(&emulator, collection, None, Some(token)).await,
                    400,
                    "ValidationException",
                );
            }
            for (what, token) in &collection.region_blind {
                assert_failure(
                    &format!("{} resumed with {what}", collection.case),
                    page_call(&emulator, collection, None, Some(token)).await,
                    400,
                    "ValidationException",
                );
            }
        }
    }
}

/// RULE, the other half: a listing resumed with the token its OWN Region issued reports
/// that Region's whole collection, at every page size — nothing skipped and nothing
/// repeated — so the refusal above is each Region's collection being kept whole rather than
/// a blanket no.
#[tokio::test]
async fn each_regions_listing_pages_its_own_collection_whole() {
    let emulator = RegionalEmulator::start();
    let families = seeded_families(&emulator).await;

    for collection in families.iter().flatten() {
        let whole = page(&emulator, collection, None, None).await;
        let unpaged = page_items(collection, &whole);
        assert_eq!(
            unpaged, collection.expected,
            "case {:?}: expected this Region's whole collection {:?}, got {unpaged:?}",
            collection.case, collection.expected
        );

        for size in 1..=collection.expected.len() {
            let case = format!("{} at page size {size}", collection.case);
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let body = page(&emulator, collection, Some(size), token.as_deref()).await;
                let items = page_items(collection, &body);
                seen.extend(items);
                assert!(
                    seen.len() <= collection.expected.len(),
                    "case {case:?}: pagination did not terminate, saw {seen:?}"
                );
                match body.get("nextToken").and_then(Value::as_str) {
                    None => break,
                    Some(issued) => token = Some(issued.to_string()),
                }
            }
            assert_eq!(
                seen, collection.expected,
                "case {case:?}: expected the resumed pages to be {:?}, got {seen:?}",
                collection.expected
            );
        }
    }
}
