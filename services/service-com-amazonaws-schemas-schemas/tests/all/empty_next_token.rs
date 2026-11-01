// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a listing of this service does with a `NextToken`
//! the request left EMPTY.
//!
//! RULE under test: a paginated listing of this service reads a `NextToken` sent with
//! an empty value as no token at all — the model documents the first page as the one a
//! caller asks for by leaving the token empty, and a query parameter present with
//! nothing in it has been left empty exactly as an omitted one has — so such a request
//! is served the FIRST page, while a token that does carry characters is still resumed
//! only when this listing issued it.
//!
//! The rule is a property of the pagination machinery every listing here shares, so it
//! is asserted over a table that spans the whole family: every paginated listing of
//! this service, each with a selection of its own, crossed with the page sizes a first
//! page can be asked for in (unpaged, one short of the total, a page that divides it,
//! one that holds everything, and one larger than the total).
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The
//! resources are created through the service's own creates, so nothing here reads the
//! state struct directly. The harness is the crate's one harness, from
//! `registry_schema_lifecycle`.

use serde_json::{json, Value};

use crate::discoverer_state::create_discoverer;
use crate::next_token_lifetime::discoverer_ids;
use crate::registry_schema_lifecycle::{
    create_registry, registry_names, schema_names, schema_uri, versions_uri,
    wait_for_keyword_index, Answer, Emulator,
};

/// The registry the schema, version and search rows below page over.
const REGISTRY: &str = "empty-token";

/// A schema document that declares an event, so a keyword selects the schema it is
/// stored as. The declared source and detail type are what a search matches on (see
/// `tests/all/search_schemas.rs`).
fn indexed_document(detail_type: &str) -> String {
    json!({
        "openapi": "3.0.0",
        "info": {"version": "1.0.0", "title": detail_type},
        "paths": {},
        "components": {"schemas": {"AWSEvent": {
            "type": "object",
            "x-amazon-events-source": "my.orders",
            "x-amazon-events-detail-type": detail_type,
            "properties": {},
        }}},
    })
    .to_string()
}

/// The `SchemaVersion` labels a `ListSchemaVersions` answer reports, in the order it
/// lists them (the sibling reducer in `registry_schema_lifecycle` collects them as a
/// SET; a page is about order as much as about membership).
fn version_labels(answer: &Answer) -> Vec<String> {
    answer.body["SchemaVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a SchemaVersions array in {:?}", answer.body))
        .iter()
        .map(|item| {
            item["SchemaVersion"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One paginated listing: its route carrying the members that SELECT it, how the items
/// of a page are read out of an answer, and the items the whole listing holds in the
/// order it serves them.
struct Listing {
    case: &'static str,
    /// The route, carrying this listing's selecting members and no pagination member.
    uri: String,
    items: fn(&Answer) -> Vec<String>,
    /// Every item of the listing, in key order.
    all: Vec<String>,
}

/// A listing's route with the pagination members this case sends appended: a route
/// that already carries its selection continues that query string, and a member this
/// case does not send contributes nothing to it.
fn paginated(uri: &str, members: &[&str]) -> String {
    let query: Vec<&str> = members
        .iter()
        .copied()
        .filter(|member| !member.is_empty())
        .collect();
    if query.is_empty() {
        return uri.to_string();
    }
    let separator = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{separator}{}", query.join("&"))
}

/// The `NextToken` an answer carries, or `None` for a page that ends its listing.
fn token_of(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Create every resource the table pages over, and return the table.
async fn listings(emulator: &Emulator) -> Vec<Listing> {
    // Three registries, so a listing of them has more than one page at every row's
    // page size.
    let mut registries: Vec<String> = Vec::new();
    for name in ["reg.gamma", "reg-alpha", "reg_beta"] {
        create_registry(emulator, name).await;
        registries.push(name.to_string());
    }
    registries.sort();

    // Three schemas in one registry, each indexed under the same event source so one
    // keyword selects all three, with one detail type of its own.
    create_registry(emulator, REGISTRY).await;
    let mut schemas: Vec<String> = Vec::new();
    for (name, detail_type) in [
        ("orders.placed", "Order Placed"),
        ("orders.cancelled", "Order Cancelled"),
        ("orders.shipped", "Order Shipped"),
    ] {
        let created = emulator
            .call(
                "POST",
                &schema_uri(REGISTRY, name),
                Some(json!({"Content": indexed_document(detail_type), "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: expected 201 from CreateSchema, got {} {:?}",
            created.status, created.body
        );
        schemas.push(name.to_string());
    }
    schemas.sort();

    // Three versions of one of them, so the version listing pages too.
    let versioned = "orders.placed";
    let mut versions = vec!["1".to_string()];
    for revision in ["Order Placed v2", "Order Placed v3"] {
        let updated = emulator
            .call(
                "PUT",
                &schema_uri(REGISTRY, versioned),
                Some(json!({"Content": indexed_document(revision), "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {revision:?}: expected 200 from UpdateSchema, got {} {:?}",
            updated.status, updated.body
        );
        versions.push(updated.str_at("SchemaVersion"));
    }
    versions.sort();

    // Three discoverers, each for a source of its own.
    let mut discoverers: Vec<String> = Vec::new();
    for bus in ["bus-one", "bus-two", "bus-three"] {
        discoverers.push(create_discoverer(emulator, "empty token", bus).await);
    }
    discoverers.sort();

    // The search rows select by keyword, so the table starts once the registry's keyword
    // index has caught up with the schemas written above.
    wait_for_keyword_index();

    let schemas_uri = format!("/v1/registries/name/{REGISTRY}/schemas");
    vec![
        Listing {
            case: "ListRegistries, no selecting member",
            uri: "/v1/registries?scope=Local".to_string(),
            items: registry_names,
            all: {
                let mut all = registries.clone();
                all.push(REGISTRY.to_string());
                all.sort();
                all
            },
        },
        Listing {
            case: "ListRegistries, narrowed by a name prefix",
            uri: "/v1/registries?scope=Local&registryNamePrefix=reg".to_string(),
            items: registry_names,
            all: registries.clone(),
        },
        Listing {
            case: "ListSchemas, one registry",
            uri: schemas_uri.clone(),
            items: schema_names,
            all: schemas.clone(),
        },
        Listing {
            case: "ListSchemas, narrowed by a name prefix",
            uri: format!("{schemas_uri}?schemaNamePrefix=orders."),
            items: schema_names,
            all: schemas.clone(),
        },
        Listing {
            case: "ListSchemaVersions, one schema",
            uri: versions_uri(REGISTRY, versioned),
            items: version_labels,
            all: versions,
        },
        Listing {
            case: "ListDiscoverers, no selecting member",
            uri: "/v1/discoverers".to_string(),
            items: discoverer_ids,
            all: discoverers.clone(),
        },
        Listing {
            case: "SearchSchemas, a keyword that selects every row",
            uri: format!("{schemas_uri}/search?keywords=my.orders"),
            items: schema_names,
            all: schemas.clone(),
        },
        Listing {
            case: "SearchSchemas, a keyword value that limits nothing",
            uri: format!("{schemas_uri}/search?keywords="),
            items: schema_names,
            all: schemas,
        },
    ]
}

/// RULE: a `NextToken` sent with an empty value is no token, so the listing serves its
/// first page — the same page the request that omits the member entirely is served, at
/// every page size and for every listing of this service.
#[tokio::test]
async fn an_empty_next_token_asks_for_the_first_page() {
    let emulator = Emulator::new();
    let listings = listings(&emulator).await;

    for listing in &listings {
        let total = listing.all.len();
        assert!(
            total > 1,
            "case {:?}: the table must page over more than one item, got {:?}",
            listing.case,
            listing.all
        );

        // Rows: unpaged, one short of the total, a size that divides it, exactly the
        // total, and more than the total.
        for limit in [None, Some(1), Some(total - 1), Some(total), Some(total + 2)] {
            let case = format!("{}: limit={limit:?}", listing.case);
            let paging = limit.map(|limit| format!("limit={limit}")).unwrap_or_default();

            let omitted = emulator
                .call("GET", &paginated(&listing.uri, &[&paging]), None)
                .await;
            assert_eq!(
                omitted.status, 200,
                "case {case:?}: expected 200 with no NextToken member, got {} {:?}",
                omitted.status, omitted.body
            );
            let first_page = (listing.items)(&omitted);

            let empty = emulator
                .call(
                    "GET",
                    &paginated(&listing.uri, &[&paging, "nextToken="]),
                    None,
                )
                .await;
            assert_eq!(
                empty.status, 200,
                "case {case:?}: an empty NextToken asks for the first page, got {} {:?}",
                empty.status, empty.body
            );
            assert_eq!(
                (listing.items)(&empty),
                first_page,
                "case {case:?}: an empty NextToken must serve the page an absent one does, \
                 got {:?}",
                empty.body
            );

            // The page it serves is the FIRST page of the listing, not an arbitrary
            // one: it starts at the first item and holds a page's worth of them.
            let expected: Vec<String> = listing
                .all
                .iter()
                .take(limit.unwrap_or(total).min(total))
                .cloned()
                .collect();
            assert_eq!(
                (listing.items)(&empty),
                expected,
                "case {case:?}: expected the first page {expected:?}, got {:?}",
                (listing.items)(&empty)
            );

            // And it is a first page in the pagination sense too: whether it hands
            // back a token, and what that token resumes, agree with the request that
            // sent no token at all.
            assert_eq!(
                token_of(&empty).is_some(),
                token_of(&omitted).is_some(),
                "case {case:?}: expected a NextToken only where an absent one yields one, \
                 got {:?} against {:?}",
                token_of(&empty),
                token_of(&omitted)
            );
            if let Some(token) = token_of(&empty) {
                let resumed = emulator
                    .call(
                        "GET",
                        &paginated(&listing.uri, &[&paging, &format!("nextToken={token}")]),
                        None,
                    )
                    .await;
                assert_eq!(
                    resumed.status, 200,
                    "case {case:?}: the token that page minted must resume it, got {} {:?}",
                    resumed.status, resumed.body
                );
                let rest = (listing.items)(&resumed);
                let expected: Vec<String> = listing
                    .all
                    .iter()
                    .skip(first_page.len())
                    .take(limit.unwrap_or(total))
                    .cloned()
                    .collect();
                assert_eq!(
                    rest, expected,
                    "case {case:?}: resuming it continues after the first page, expected \
                     {expected:?}, got {rest:?}"
                );
            }
        }
    }
}

/// RULE (the other side of the same sentence): only a token left EMPTY is no token — a
/// `NextToken` that carries characters is a token presented, and a listing resumes it
/// only when it issued it, so one it did not mint is still refused.
#[tokio::test]
async fn a_non_empty_token_this_listing_did_not_issue_is_still_refused() {
    let emulator = Emulator::new();
    let listings = listings(&emulator).await;

    for listing in &listings {
        // Rows: a value that is not a token at all, one that is hex but not of the
        // token's shape, a token-shaped value whose fields are not hex, and a value
        // whose only content is a space — none of them empty, so each is a token this
        // listing is asked to resume and did not issue.
        for token in ["not-a-token", "zz", "6162.6364.zzzz", "%20"] {
            let case = format!("{}: nextToken={token:?}", listing.case);
            let answer = emulator
                .call(
                    "GET",
                    &paginated(&listing.uri, &["limit=1", &format!("nextToken={token}")]),
                    None,
                )
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {:?}",
                answer.body
            );
        }
    }
}
