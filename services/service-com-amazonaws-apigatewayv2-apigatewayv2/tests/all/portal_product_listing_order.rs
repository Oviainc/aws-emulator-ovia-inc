// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ORDER, the continuation token and the display order of
//! the ApiGatewayV2 portal collections.
//!
//! The rules these cases assert, each one level above the recorded case that filed the
//! issue:
//!
//! * **A portal collection reader reports its records OLDEST FIRST** — in the order the
//!   account created them — and a page resumed by its own `NextToken` continues in that
//!   same order, so a client that reads `items[0]` as the first resource the account
//!   created gets it. The identifier the service mints carries no ordering, so the
//!   listing order is the records' own and not their identifiers'.
//! * **`ListPortalProducts` enumerates the products an account can SEE in two groups,
//!   one after the other**: the products the account owns, then the products other
//!   accounts shared with it. A walk that was not restricted to one owner therefore
//!   still has a group to report when the account's own products run out, so the page
//!   carrying the last of them carries the continuation token that starts the next group,
//!   and the walk ends on the group that has nothing after it. `ResourceOwner` restricts
//!   the walk to the group it names, which then ends it — and a token naming a position
//!   in a group the walk does not cover is refused, like any position this service did
//!   not issue for the collection being read.
//! * **A portal product always reports a `DisplayOrder`**: a product whose pages nothing
//!   has ordered is ordered EMPTY rather than unordered, so the structure's two
//!   collection members (`DisplayOrder$Contents`, `DisplayOrder$ProductPageArns`) are
//!   reported as empty lists rather than left out — and a stored order that named only
//!   some of them reports the rest the same way. What a writer reports is what a later
//!   `GetPortalProduct` reports.
//!
//! The rows vary what the rules generalise over: which portal collection is listed, how
//! many records it holds, the page size the walk uses, which owner group the request
//! selects, and which members of a display order a patch names. The recorded units that
//! filed the issue — listing the one and the two products an account just created — are
//! two rows.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::OTHER_ACCOUNTS_RESOURCE_OWNER;

use crate::api_lifecycle::{as_str, call, carries_nothing, emulator, error_code};
use crate::portal_lifecycle::portal_content;

/// The portal-products collection URI (`ListPortalProducts`'s modelled
/// `GET /v2/portalproducts`).
const PRODUCTS: &str = "/v2/portalproducts";

/// The portals collection URI (`ListPortals`'s modelled `GET /v2/portals`).
const PORTALS: &str = "/v2/portals";

/// One portal collection this rule covers: the two account-level collections of the
/// portal resources whose readers this service implements.
struct Collection {
    case: &'static str,
    uri: &'static str,
    /// The member each listed item is identified by.
    id_member: &'static str,
    /// The create request that adds one record named `display_name`.
    request: fn(&str) -> Value,
    /// The status the create answers.
    created_status: u16,
}

/// The create request of one portal product: `DisplayName` is its only required member.
fn product_request(display_name: &str) -> Value {
    json!({ "displayName": display_name })
}

/// The create request of one portal: `Authorization`, `EndpointConfiguration` and
/// `PortalContent` are required, and the simplest alternative of each union is enough.
fn portal_request(display_name: &str) -> Value {
    json!({
        "authorization": { "none": {} },
        "endpointConfiguration": { "none": {} },
        "portalContent": portal_content(display_name)
    })
}

fn collections() -> Vec<Collection> {
    vec![
        Collection {
            case: "the account's portal products",
            uri: PRODUCTS,
            id_member: "portalProductId",
            request: product_request,
            created_status: 201,
        },
        Collection {
            case: "the account's portals",
            uri: PORTALS,
            id_member: "portalId",
            request: portal_request,
            created_status: 201,
        },
    ]
}

/// Create one record in `collection` and answer the id it was minted.
async fn create(
    emulator: &MultiServiceRouter,
    collection: &Collection,
    case: &str,
    display_name: &str,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        collection.uri,
        Some((collection.request)(display_name)),
    )
    .await;
    assert_eq!(
        status, collection.created_status,
        "case {case:?}: expected {} from the create of {display_name:?}, got {status} {created}",
        collection.created_status
    );
    as_str(&created, collection.id_member)
}

/// One page of a listing, which must answer 200.
async fn list(emulator: &MultiServiceRouter, uri: &str, case: &str, query: &str) -> Value {
    let (status, body) = call(emulator, "GET", &format!("{uri}{query}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the listing of {uri}, got {status} {body}"
    );
    body
}

/// The items one page reports.
fn items(body: &Value) -> Vec<Value> {
    body["items"].as_array().cloned().unwrap_or_default()
}

/// The ids one page reports, in the order it reported them.
fn ids(body: &Value, id_member: &str) -> Vec<String> {
    items(body)
        .iter()
        .map(|item| as_str(item, id_member))
        .collect()
}

/// The `NextToken` a page carries, or `None` when it carries none.
fn next_token(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// Walk `uri` from its first page to its last at `page_size` and answer every page.
///
/// Bounded by one more request than the walk can need, so a token that never clears is
/// caught as a failure instead of looping forever.
async fn walk(
    emulator: &MultiServiceRouter,
    uri: &str,
    case: &str,
    page_size: usize,
    total: usize,
) -> Vec<Value> {
    let mut pages: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..total + 3 {
        let query = match &token {
            Some(carried) => format!("?maxResults={page_size}&nextToken={carried}"),
            None => format!("?maxResults={page_size}"),
        };
        let body = list(emulator, uri, case, &query).await;
        assert!(
            items(&body).len() <= page_size,
            "case {case:?}: a page must hold at most {page_size} items, got {body}"
        );
        token = next_token(&body);
        pages.push(body);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {case:?}: expected the walk to end, got a NextToken {token:?} after {} pages",
        pages.len()
    );
    pages
}

#[tokio::test]
async fn a_portal_collection_is_listed_oldest_first() {
    // The rule generalises over which collection is listed, how many records it holds and
    // the page size the walk uses, so the table walks all three.
    for collection in collections() {
        for total in [1_usize, 2, 3, 8] {
            let emulator = emulator();
            let mut created: Vec<String> = Vec::new();
            for index in 0..total {
                let case = format!("{}, total={total}, record #{index}", collection.case);
                created.push(
                    create(
                        &emulator,
                        &collection,
                        &case,
                        &format!("Ordered Record {index}"),
                    )
                    .await,
                );
            }

            for page_size in 1..=total + 1 {
                let case = format!("{}, total={total}, maxResults={page_size}", collection.case);
                let pages = walk(&emulator, collection.uri, &case, page_size, total).await;
                let seen: Vec<String> = pages
                    .iter()
                    .flat_map(|page| ids(page, collection.id_member))
                    .collect();
                assert_eq!(
                    seen, created,
                    "case {case:?}: expected the walk to report the records oldest first, \
                     got {seen:?} against the created order {created:?}"
                );
            }

            // A record created LATER is reported after every record created before it,
            // whatever id it was minted.
            let case = format!("{}, total={total}, one more record", collection.case);
            created.push(create(&emulator, &collection, &case, "The Newest Record").await);
            let body = list(&emulator, collection.uri, &case, "").await;
            assert_eq!(
                ids(&body, collection.id_member),
                created,
                "case {case:?}: expected the newest record last, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_minted_identifier_does_not_decide_the_listing_order() {
    // The ids are drawn from a bijection over the id space, so the identifier of the
    // record created second may sort before the first's: this case pins that the listing
    // order is the records' OWN and would not be satisfied by sorting the ids. Counted
    // across the table, since whether two particular ids sort the other way is a fact of
    // the id space rather than of the rule.
    let mut rows_where_the_ids_sort_the_other_way = 0_usize;
    for collection in collections() {
        for total in [2_usize, 3, 5, 8] {
            let emulator = emulator();
            let mut created: Vec<String> = Vec::new();
            for index in 0..total {
                let case = format!("{}, total={total}, record #{index}", collection.case);
                created.push(
                    create(&emulator, &collection, &case, &format!("Minted {index}"))
                        .await,
                );
            }
            let mut by_id = created.clone();
            by_id.sort();
            if by_id == created {
                continue;
            }
            rows_where_the_ids_sort_the_other_way += 1;

            let case = format!("{}, total={total}", collection.case);
            let listed = ids(&list(&emulator, collection.uri, &case, "").await, collection.id_member);
            assert_eq!(
                listed, created,
                "case {case:?}: expected the created order, got {listed:?}"
            );
            assert_ne!(
                listed, by_id,
                "case {case:?}: expected a listing that is not merely the ids sorted, got \
                 {listed:?}"
            );
        }
    }
    assert!(
        rows_where_the_ids_sort_the_other_way > 0,
        "expected at least one row whose minted ids sort differently from the creation \
         order, so the ordering assertions are not satisfied by sorting the ids; got none"
    );
}

#[tokio::test]
async fn the_portal_products_walk_continues_into_the_group_shared_with_the_account() {
    // The rule generalises over how many products the account owns and over the page
    // size: whatever they are, the walk reports every owned product once and then the
    // group shared with the account, which is what ends it.
    for total in [0_usize, 1, 2, 5] {
        let emulator = emulator();
        let mut created: Vec<String> = Vec::new();
        for index in 0..total {
            let case = format!("total={total}, product #{index}");
            created.push(
                create(
                    &emulator,
                    &collections()[0],
                    &case,
                    &format!("Grouped Product {index}"),
                )
                .await,
            );
        }

        for page_size in 1..=total + 1 {
            let case = format!("total={total}, maxResults={page_size}");
            let pages = walk(&emulator, PRODUCTS, &case, page_size, total).await;
            let seen: Vec<String> = pages
                .iter()
                .flat_map(|page| ids(page, "portalProductId"))
                .collect();
            assert_eq!(
                seen, created,
                "case {case:?}: expected every owned product once, oldest first, got {seen:?}"
            );
            // One page per page-size-worth of owned products (at least one, since even an
            // account owning none is answered a page), and then the shared group.
            let owned_pages = total.div_ceil(page_size).max(1);
            assert_eq!(
                pages.len(),
                owned_pages + 1,
                "case {case:?}: expected the owned group's {owned_pages} page(s) and then \
                 the shared group, got {} pages",
                pages.len()
            );
            let last = pages.last().expect("the walk answered at least one page");
            assert!(
                items(last).is_empty() && carries_nothing(last, "nextToken"),
                "case {case:?}: expected the shared group to report nothing and end the \
                 walk, got {last}"
            );
            // The page that carries the last owned product is NOT the end of the walk:
            // the group shared with the account is still to come, so it carries the token
            // that starts it.
            let carrying_the_last_owned = &pages[owned_pages - 1];
            assert!(
                next_token(carrying_the_last_owned).is_some(),
                "case {case:?}: expected a NextToken on the page holding the last owned \
                 product, got {carrying_the_last_owned}"
            );
        }
    }
}

#[tokio::test]
async fn a_resource_owner_restricts_the_portal_products_walk_to_the_group_it_names() {
    let emulator = emulator();
    let mut created: Vec<String> = Vec::new();
    for index in 0..3 {
        let case = format!("product #{index}");
        created.push(
            create(
                &emulator,
                &collections()[0],
                &case,
                &format!("Owned Product {index}"),
            )
            .await,
        );
    }

    // (the case, the query, the ids the page must report, whether the walk goes on).
    // `SELF` is the account's own group and `OTHER_ACCOUNTS` the group shared with it;
    // this emulator serves one account and every product it holds was created by that
    // account's own call, so the shared group holds nothing.
    let rows: Vec<(&str, String, Vec<String>, bool)> = vec![
        (
            "no owner at all, which covers both groups",
            String::new(),
            created.clone(),
            true,
        ),
        (
            "this account's own group",
            "?resourceOwner=SELF".to_string(),
            created.clone(),
            false,
        ),
        (
            "the group shared with this account",
            format!("?resourceOwner={OTHER_ACCOUNTS_RESOURCE_OWNER}"),
            Vec::new(),
            false,
        ),
        (
            "the shared group, paged below the account's own total",
            format!("?maxResults=1&resourceOwner={OTHER_ACCOUNTS_RESOURCE_OWNER}"),
            Vec::new(),
            false,
        ),
    ];
    for (case, query, expected, goes_on) in rows {
        let body = list(&emulator, PRODUCTS, case, &query).await;
        assert_eq!(
            ids(&body, "portalProductId"),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert_eq!(
            next_token(&body).is_some(),
            goes_on,
            "case {case:?}: expected the walk to {}, got {body}",
            if goes_on { "go on" } else { "end here" }
        );
    }

    // The token that starts the shared group is this collection's own: it reports the
    // group's products — none — and ends the walk.
    let token = next_token(&list(&emulator, PRODUCTS, "the first group", "").await)
        .expect("the owned group is not the last group of the walk");
    let shared = list(
        &emulator,
        PRODUCTS,
        "the shared group resumed by its own token",
        &format!("?nextToken={token}"),
    )
    .await;
    assert!(
        items(&shared).is_empty() && carries_nothing(&shared, "nextToken"),
        "case \"the shared group resumed by its own token\": expected an empty final page, \
         got {shared}"
    );

    // A value this service never issued names no position in either group, whichever
    // owner the request selects.
    for owner in ["", "?resourceOwner=SELF&", "?resourceOwner=OTHER_ACCOUNTS&"] {
        let query = if owner.is_empty() {
            "?nextToken=not-a-token".to_string()
        } else {
            format!("{owner}nextToken=not-a-token")
        };
        let (status, body) = call(&emulator, "GET", &format!("{PRODUCTS}{query}"), None).await;
        assert_eq!(
            status, 400,
            "case {query:?}: expected 400 for a token this service did not issue, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {query:?}: expected BadRequestException, got {body}"
        );
    }

    // And a token this service DID issue names no position in a walk that does not cover
    // the group it was issued for: the shared group's token under a walk restricted to
    // the account's own products, and an owned-group token under a walk restricted to the
    // shared group, are both refused rather than answered with another group's page.
    let owned_token = next_token(&list(&emulator, PRODUCTS, "the first owned page", "?maxResults=1").await)
        .expect("products remain after a page of one");
    let rows: Vec<(&str, String)> = vec![
        (
            "the shared group's token under resourceOwner=SELF",
            format!("?resourceOwner=SELF&nextToken={token}"),
        ),
        (
            "an owned-group token under resourceOwner=OTHER_ACCOUNTS",
            format!("?resourceOwner={OTHER_ACCOUNTS_RESOURCE_OWNER}&nextToken={owned_token}"),
        ),
    ];
    for (case, query) in rows {
        let (status, body) = call(&emulator, "GET", &format!("{PRODUCTS}{query}"), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a position in a group this walk does not \
             cover, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }
}

/// A page ARN of the form this service mints, for a display order to name.
/// `DisplayOrder$OverviewPageArn` and the members of `ProductPageArns` are `len 20..2048`,
/// so they are spelled as full ARNs of this emulator's account.
fn page_arn(resource: &str) -> String {
    let account = ::engine::emulator_account_id();
    format!("arn:aws:apigateway:eu-west-1:{account}:/portalpages/{resource}")
}

#[tokio::test]
async fn a_portal_product_always_reports_a_display_order_with_its_collections() {
    let emulator = emulator();

    // (the case, the patch to apply — none at all for a freshly created product — and the
    // display order every reader must then report). The rows vary which members of the
    // ordering the stored value names: none of them, each collection alone, the single
    // ARN alone, and all of them.
    let rows: Vec<(&str, Option<Value>, Value)> = vec![
        (
            "a product nothing has ordered",
            None,
            json!({ "contents": [], "productPageArns": [] }),
        ),
        (
            "an ordering that names nothing",
            Some(json!({ "displayOrder": {} })),
            json!({ "contents": [], "productPageArns": [] }),
        ),
        (
            "an ordering naming only the overview page",
            Some(json!({ "displayOrder": { "overviewPageArn": page_arn("overview") } })),
            json!({
                "contents": [],
                "overviewPageArn": page_arn("overview"),
                "productPageArns": []
            }),
        ),
        (
            "an ordering naming only the product pages",
            Some(json!({
                "displayOrder": { "productPageArns": [page_arn("first"), page_arn("second")] }
            })),
            json!({
                "contents": [],
                "productPageArns": [page_arn("first"), page_arn("second")]
            }),
        ),
        (
            "an ordering naming only the sections",
            Some(json!({
                "displayOrder": {
                    "contents": [{
                        "sectionName": "Getting started",
                        "productRestEndpointPageArns": [page_arn("endpoint")]
                    }]
                }
            })),
            json!({
                "contents": [{
                    "sectionName": "Getting started",
                    "productRestEndpointPageArns": [page_arn("endpoint")]
                }],
                "productPageArns": []
            }),
        ),
        (
            "an ordering naming every member",
            Some(json!({
                "displayOrder": {
                    "contents": [{
                        "sectionName": "Reference",
                        "productRestEndpointPageArns": [page_arn("endpoint")]
                    }],
                    "overviewPageArn": page_arn("overview"),
                    "productPageArns": [page_arn("first")]
                }
            })),
            json!({
                "contents": [{
                    "sectionName": "Reference",
                    "productRestEndpointPageArns": [page_arn("endpoint")]
                }],
                "overviewPageArn": page_arn("overview"),
                "productPageArns": [page_arn("first")]
            }),
        ),
    ];

    for (case, patch, expected) in rows {
        let (status, created) = call(
            &emulator,
            "POST",
            PRODUCTS,
            Some(product_request(&format!("Ordered: {case}"))),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreatePortalProduct, got {status} {created}"
        );
        let product_id = as_str(&created, "portalProductId");
        let uri = format!("{PRODUCTS}/{product_id}");

        // A create carries no ordering of its own (`CreatePortalProductRequest` has no
        // such member), so the product it answers is ordered empty.
        assert_eq!(
            created.get("displayOrder"),
            Some(&json!({ "contents": [], "productPageArns": [] })),
            "case {case:?}: expected the empty ordering on a created product, got {created}"
        );

        let written = match &patch {
            Some(patch) => {
                let (status, patched) = call(&emulator, "PATCH", &uri, Some(patch.clone())).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from UpdatePortalProduct, got {status} \
                     {patched}"
                );
                patched
            }
            None => created,
        };
        assert_eq!(
            written.get("displayOrder"),
            Some(&expected),
            "case {case:?}: expected the writer to report {expected}, got {written}"
        );

        // What the writer reported is what a later reader reports: the ordering was
        // stored, not echoed.
        let (status, fetched) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortalProduct, got {status} {fetched}"
        );
        assert_eq!(
            fetched.get("displayOrder"),
            Some(&expected),
            "case {case:?}: expected GetPortalProduct to report {expected}, got {fetched}"
        );
        // Both collections are reported, so a client that walks either of them
        // dereferences nothing.
        for member in ["contents", "productPageArns"] {
            assert!(
                fetched["displayOrder"].get(member).is_some_and(Value::is_array),
                "case {case:?}: expected the ordering to report a {member} list, got {fetched}"
            );
        }
    }
}
