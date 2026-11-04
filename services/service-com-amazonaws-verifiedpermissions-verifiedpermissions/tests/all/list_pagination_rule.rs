// SPDX-License-Identifier: CC0-1.0

//! The ONE pagination rule every `@paginated` list operation of this service
//! pages by, driven end-to-end through each of them.
//!
//! The rule: a list operation enumerates its persisted resources in a stable
//! order, resumes at the cursor its previous page handed back, carries at most
//! the asked-for `maxResults` — the operation's own documented default when the
//! caller names none, never more than the documented maximum — reports a
//! `nextToken` exactly while selected resources remain, and refuses a cursor it
//! did not mint FOR THAT LISTING with a `ValidationException` naming
//! `/nextToken`. A listing is the operation and, for an operation that
//! enumerates one policy store's resources, that store: a cursor names a
//! position in one enumeration and means nothing in another.
//!
//! The suites next door assert what each operation reports about ONE kind of
//! resource; this one asserts that every one of them pages the SAME way, so the
//! rule they share cannot drift apart. Every assertion goes over the wire through
//! the shared [`super::harness`], so a listed resource is only listed if it really
//! reached state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, error_code, fault_message, ok, store_with_validation_off as store, user_pool, user_pool_arn,
    SIGNING_REGION,
};

/// The page size `ListPolicies`, `ListIdentitySources`, `ListPolicyTemplates`
/// and `ListPolicyStores` use when the caller names no `maxResults`, from each
/// one's own `maxResults` documentation: "If you do not specify this parameter,
/// the operation defaults to 10 ... per response."
const DEFAULT_PAGE_SIZE: usize = 10;

/// The page size `ListPolicyStoreAliases` uses when the caller names no
/// `maxResults`, from `ListPolicyStoreAliasesInput$maxResults`'s own
/// documentation: "If you do not specify this parameter, the operation defaults
/// to 5 policy store aliases per response" — the figure that member also carries
/// as its modelled `@default: 5`.
const ALIAS_DEFAULT_PAGE_SIZE: usize = 5;

/// The largest page any of them builds, from the next sentence of that same
/// documentation: "You can specify a maximum of 50 ... per response."
const MAX_PAGE_SIZE: usize = 50;

/// How many resources of each kind the table seeds: two past the maximum page, so
/// a page capped at [`MAX_PAGE_SIZE`] still leaves a cursor behind.
const SEEDED: usize = MAX_PAGE_SIZE + 2;

/// How many pages a walk takes before declaring the cursor broken. Far above any
/// walk below, so a cursor that never advances fails the test instead of hanging
/// it.
const PAGE_WALK_LIMIT: usize = 500;

/// The prefix every policy store alias name carries, from
/// `CreatePolicyStoreAliasInput$aliasName`'s documentation: "The alias name must
/// always be prefixed with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// One `@paginated` list operation of this service, as this suite drives it.
#[derive(Debug)]
struct Listing {
    /// The operation to call.
    operation: &'static str,
    /// The required output member carrying the page's items.
    items_member: &'static str,
    /// The item member that identifies one resource, so a walk can tell the
    /// resources apart.
    id_member: &'static str,
    /// The page size this operation uses when the request names no `maxResults`.
    default_page_size: usize,
    /// The request every page of this listing is built from, before `maxResults`
    /// and `nextToken` are added.
    request: Value,
}

/// Seed ONE policy store with `count` resources of every kind this service
/// pages, and return the listings that enumerate THAT store.
///
/// `label` distinguishes the resources whose names are unique account-wide — a
/// policy store alias, and the user pool an identity source references — so a
/// suite can seed two stores on one router and still create each resource.
async fn seed_store(router: &MultiServiceRouter, label: &str, count: usize) -> Vec<Listing> {
    let store_id = store(router).await;

    for i in 0..count {
        // The scopes differ per policy, so the seeded policies are distinct
        // resources rather than copies of one.
        ok(
            router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": format!(
                    "permit(principal == User::\"u{i:03}\", action == Action::\"view\", \
                     resource == Photo::\"p{i:03}\");"
                ) } },
            }),
        )
        .await;
        ok(
            router,
            "CreatePolicyTemplate",
            &json!({
                "policyStoreId": store_id,
                "name": format!("template-{i:03}"),
                "statement": "permit(principal == ?principal, action == Action::\"view\", \
                              resource == Photo::\"p\");",
            }),
        )
        .await;
        ok(
            router,
            "CreatePolicyStoreAlias",
            &json!({
                "aliasName": format!("{ALIAS_PREFIX}{label}-alias-{i:03}"),
                "policyStoreId": store_id,
            }),
        )
        .await;
    }

    // One pool, referenced by every seeded identity source: an identity source
    // can only name a user pool the user-pool service really holds.
    let pool = user_pool_arn(
        SIGNING_REGION,
        &user_pool(router, &format!("{label}-pagination-pool")).await,
    );
    for i in 0..count {
        ok(
            router,
            "CreateIdentitySource",
            &json!({
                "policyStoreId": store_id,
                "principalEntityType": format!("MyCorp::User{i:03}"),
                "configuration": { "cognitoUserPoolConfiguration": { "userPoolArn": pool } },
            }),
        )
        .await;
    }

    vec![
        Listing {
            operation: "ListPolicies",
            items_member: "policies",
            id_member: "policyId",
            default_page_size: DEFAULT_PAGE_SIZE,
            request: json!({ "policyStoreId": store_id }),
        },
        Listing {
            operation: "ListIdentitySources",
            items_member: "identitySources",
            id_member: "identitySourceId",
            default_page_size: DEFAULT_PAGE_SIZE,
            request: json!({ "policyStoreId": store_id }),
        },
        Listing {
            operation: "ListPolicyTemplates",
            items_member: "policyTemplates",
            id_member: "policyTemplateId",
            default_page_size: DEFAULT_PAGE_SIZE,
            request: json!({ "policyStoreId": store_id }),
        },
    ]
}

/// Every `@paginated` list operation this service implements, each with
/// [`SEEDED`] resources of its kind already persisted.
async fn seeded_listings(router: &MultiServiceRouter) -> Vec<Listing> {
    let mut listings = seed_store(router, "paged", SEEDED).await;
    // `ListPolicyStoreAliases` enumerates the ACCOUNT's aliases rather than one
    // store's, so it takes no `policyStoreId` and the aliases seeded above are
    // all of them.
    listings.push(Listing {
        operation: "ListPolicyStoreAliases",
        items_member: "policyStoreAliases",
        id_member: "aliasName",
        default_page_size: ALIAS_DEFAULT_PAGE_SIZE,
        request: json!({}),
    });
    // `ListPolicyStores` enumerates the ACCOUNT's policy stores, so its seeded
    // resources are the stores themselves: the one `seed_store` created above,
    // plus enough more to reach [`SEEDED`] of them.
    for _ in 1..SEEDED {
        store(router).await;
    }
    listings.push(Listing {
        operation: "ListPolicyStores",
        items_member: "policyStores",
        id_member: "policyStoreId",
        default_page_size: DEFAULT_PAGE_SIZE,
        request: json!({}),
    });
    listings
}

/// One page of `listing`, requiring success. `max_results` and `next_token` are
/// omitted from the request when `None`, so a row tests the ABSENT member rather
/// than a stand-in value.
async fn page(
    router: &MultiServiceRouter,
    listing: &Listing,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Value {
    let mut request = listing.request.clone();
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        request["nextToken"] = json!(next_token);
    }
    ok(router, listing.operation, &request).await
}

/// The ids one page carries. The items member is **required**, so a response that
/// omits it is a defect rather than an empty page.
fn ids_of(case: &str, listing: &Listing, body: &Value) -> Vec<String> {
    let items = body[listing.items_member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {} is required, got {body}", listing.items_member));
    items
        .iter()
        .map(|item| {
            item[listing.id_member]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {} is required, got {item}", listing.id_member)
                })
                .to_string()
        })
        .collect()
}

/// Walk every page of `listing` at `max_results` items a time and return
/// `(the ids in page order, the length of each page)`.
async fn walk(
    case: &str,
    router: &MultiServiceRouter,
    listing: &Listing,
    max_results: Option<i64>,
) -> (Vec<String>, Vec<usize>) {
    let mut ids = Vec::new();
    let mut page_lengths = Vec::new();
    let mut token: Option<String> = None;
    for walked in 0.. {
        assert!(
            walked < PAGE_WALK_LIMIT,
            "case {case:?}: the cursor never finished after {walked} pages"
        );
        let body = page(router, listing, max_results, token.as_deref()).await;
        let batch = ids_of(case, listing, &body);
        page_lengths.push(batch.len());
        ids.extend(batch);
        match body["nextToken"].as_str() {
            Some(next) => {
                assert!(
                    next.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_=+/.".contains(c)),
                    "case {case:?}: nextToken must match ^[A-Za-z0-9-_=+/\\.]*$, got {next:?}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    (ids, page_lengths)
}

/// Every list operation pages by the same rule: an absent `maxResults` takes the
/// operation's own default, an asked-for one is honoured up to the documented
/// maximum, and a walk reports every persisted resource exactly once in a stable
/// order with a `nextToken` present exactly while resources remain.
#[tokio::test]
async fn every_list_operation_pages_by_the_same_rule() {
    let router = super::harness::router();
    let listings = seeded_listings(&router).await;

    for listing in &listings {
        let operation = listing.operation;

        // An absent `maxResults` is the operation's own default page, not the
        // whole store in one response.
        let case = format!("{operation}: no maxResults");
        let body = page(&router, listing, None, None).await;
        let ids = ids_of(&case, listing, &body);
        assert_eq!(
            ids.len(),
            listing.default_page_size,
            "case {case:?}: expected the operation's default page size {}, got {} items",
            listing.default_page_size,
            ids.len()
        );
        assert!(
            body["nextToken"].is_string(),
            "case {case:?}: expected a cursor while {} resources remain, got {body}",
            SEEDED - listing.default_page_size
        );

        // Rows differ along the asked-for page size: one item a page, a size
        // that divides nothing evenly, exactly the maximum page, every seeded
        // resource at once, and a size far above the documented maximum — which
        // is capped at it, not honoured.
        for asked in [1i64, 7, MAX_PAGE_SIZE as i64, SEEDED as i64, 1_000] {
            let case = format!("{operation}: maxResults={asked}");
            let effective = (asked as usize).min(MAX_PAGE_SIZE);
            let (ids, page_lengths) = walk(&case, &router, listing, Some(asked)).await;

            assert!(
                page_lengths.iter().all(|len| *len <= effective),
                "case {case:?}: no page may exceed the effective page size {effective}, \
                 got page lengths {page_lengths:?}"
            );
            let full_pages = SEEDED / effective;
            assert_eq!(
                page_lengths.len(),
                full_pages + usize::from(!SEEDED.is_multiple_of(effective)),
                "case {case:?}: unexpected number of pages, got page lengths {page_lengths:?}"
            );
            assert_eq!(
                ids.len(),
                SEEDED,
                "case {case:?}: expected every one of the {SEEDED} seeded resources, \
                 got {} ids",
                ids.len()
            );
            let mut sorted = ids.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                SEEDED,
                "case {case:?}: a resource may not be reported twice across pages, got {ids:?}"
            );
            assert_eq!(
                ids, sorted,
                "case {case:?}: the walk must enumerate in a stable, ascending order, got {ids:?}"
            );
        }
    }
}

/// Mint one real cursor for `listing`, by asking it for a page smaller than what
/// it holds. The token is whatever the service issued — this suite never builds
/// one itself, so what it asserts is about the service's own cursors.
async fn minted_cursor(router: &MultiServiceRouter, listing: &Listing) -> String {
    let body = page(router, listing, Some(1), None).await;
    body["nextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: a page smaller than the listing must hand back a cursor, got {body}",
                listing.operation
            )
        })
        .to_string()
}

/// Assert `listing` refuses `token` the ONE way this service refuses a cursor it
/// did not mint for that listing: HTTP 400 `ValidationException` whose
/// `fieldList` names `/nextToken` and nothing else — never a 200 whose page is
/// silently truncated or empty, which would hide the client's mistake.
async fn assert_cursor_refused(
    router: &MultiServiceRouter,
    listing: &Listing,
    token: &str,
    case: &str,
) {
    let mut request = listing.request.clone();
    request["nextToken"] = json!(token);
    let (status, body) = call(router, listing.operation, &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: unexpected error code, got {body}"
    );
    assert_eq!(
        fault_message(&body),
        "Invalid input",
        "case {case:?}: unexpected message, got {body}"
    );
    assert_eq!(
        body["fieldList"][0]["path"].as_str(),
        Some("/nextToken"),
        "case {case:?}: the refusal must name the offending member, got {body}"
    );
    assert_eq!(
        body["fieldList"][0]["message"].as_str(),
        Some("it is not a cursor this service issued"),
        "case {case:?}: unexpected fieldList reason, got {body}"
    );
    assert_eq!(
        body["fieldList"].as_array().map(Vec::len),
        Some(1),
        "case {case:?}: only the cursor is at fault, got {body}"
    );
}

/// Every list operation refuses a `nextToken` it did not mint the same way: HTTP
/// 400 `ValidationException`, whose `fieldList` names `/nextToken` — not an empty
/// page, which would hide the client's mistake.
#[tokio::test]
async fn every_list_operation_refuses_a_forged_cursor_the_same_way() {
    let router = super::harness::router();
    let listings = seeded_listings(&router).await;

    // Rows differ along HOW the cursor is wrong: not this service's encoding at
    // all, an odd number of hex digits, hex that decodes to bytes which are not
    // text, and — the rows a cursor has to carry its listing to refuse — hex
    // that decodes PERFECTLY WELL to text naming no listing of this service: a
    // string sorting above every id a store mints, an id in the shape AWS
    // publishes for one (`SPEXAMPLEabcdefg111111`), and a key in the shape of a
    // policy store alias name.
    let forged: Vec<(String, String)> = vec![
        ("not a cursor".to_string(), "notacursor".to_string()),
        ("odd-length hex".to_string(), "953".to_string()),
        ("hex that is not text".to_string(), "ff".to_string()),
        ("hex of text past every minted id".to_string(), hex::encode("zz")),
        (
            "hex of a published id shape".to_string(),
            hex::encode("SPEXAMPLEabcdefg111111"),
        ),
        (
            "hex of an alias name".to_string(),
            hex::encode(format!("{ALIAS_PREFIX}paged-alias-000")),
        ),
    ];

    for listing in &listings {
        for (how, token) in &forged {
            let case = format!("{}: {how}", listing.operation);
            assert_cursor_refused(&router, listing, token, &case).await;
        }
    }
}

/// A cursor belongs to the LISTING that minted it — the operation that issued it
/// and, for an operation that enumerates one policy store, that store — so every
/// other listing refuses it, and only the one that minted it resumes with it.
#[tokio::test]
async fn a_cursor_is_accepted_only_by_the_listing_that_minted_it() {
    let router = super::harness::router();
    let listings = seeded_listings(&router).await;

    // One real cursor per listing, so the rows below differ along WHICH of this
    // service's own cursors is submitted to WHICH listing.
    let mut minted: Vec<(&str, String)> = Vec::new();
    for listing in &listings {
        minted.push((listing.operation, minted_cursor(&router, listing).await));
    }

    for listing in &listings {
        for (minted_by, token) in &minted {
            if *minted_by == listing.operation {
                let case = format!("{}: the cursor it minted itself", listing.operation);
                let body = page(&router, listing, Some(1), Some(token)).await;
                let ids = ids_of(&case, listing, &body);
                assert_eq!(
                    ids.len(),
                    1,
                    "case {case:?}: its own cursor must resume the walk, got {ids:?}"
                );
                continue;
            }
            let case = format!("{}: a cursor minted by {minted_by}", listing.operation);
            assert_cursor_refused(&router, listing, token, &case).await;
        }
    }
}

/// How many resources of each kind the two-store table below seeds: enough that a
/// page of one leaves a cursor behind.
const SEEDED_PER_STORE: usize = 2;

/// A cursor of a store-scoped listing names a position in ONE store's
/// enumeration, so the same operation refuses it for any other store — rather
/// than resuming that store's walk at a key drawn from somewhere else.
#[tokio::test]
async fn a_store_scoped_cursor_is_refused_by_another_store() {
    let router = super::harness::router();
    // Two stores holding the same kinds of resource. `ListPolicyStoreAliases` is
    // not among these listings: it enumerates the ACCOUNT's aliases, so it has
    // one enumeration and no store to disagree about.
    let first = seed_store(&router, "first", SEEDED_PER_STORE).await;
    let second = seed_store(&router, "second", SEEDED_PER_STORE).await;

    for (mine, other) in first.iter().zip(second.iter()) {
        let operation = mine.operation;
        assert_eq!(
            operation, other.operation,
            "both stores must be seeded with the same listings, got {operation} and {}",
            other.operation
        );

        let token = minted_cursor(&router, mine).await;
        let case = format!("{operation}: a cursor minted for another store");
        assert_cursor_refused(&router, other, &token, &case).await;

        // The store that minted it still resumes with it, so the refusal above
        // is about WHICH store is being enumerated and not about the cursor.
        let case = format!("{operation}: the cursor its own store minted");
        let body = page(&router, mine, Some(1), Some(&token)).await;
        let ids = ids_of(&case, mine, &body);
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: the minting store must resume its own walk, got {ids:?}"
        );
    }
}
