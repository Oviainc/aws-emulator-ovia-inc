// SPDX-License-Identifier: CC0-1.0

//! The ONE pagination rule every `@paginated` list operation of this service
//! pages by, driven end-to-end through each of them.
//!
//! The rule: a list operation enumerates its persisted resources in a stable
//! order, resumes at the cursor its previous page handed back, carries at most
//! the asked-for `maxResults` — the operation's own documented default when the
//! caller names none, never more than the documented maximum — reports a
//! `nextToken` exactly while selected resources remain, and refuses a cursor it
//! did not mint with a `ValidationException` naming `/nextToken`.
//!
//! The suites next door assert what each operation reports about ONE kind of
//! resource; this one asserts that all four page the SAME way, so the rule they
//! share cannot drift apart. Every assertion goes over the wire through the
//! shared [`super::harness`], so a listed resource is only listed if it really
//! reached state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, error_code, fault_message, ok, store_with_validation_off as store, user_pool, user_pool_arn,
    SIGNING_REGION,
};

/// The page size `ListPolicies`, `ListIdentitySources` and `ListPolicyTemplates`
/// use when the caller names no `maxResults`, from each one's own `maxResults`
/// documentation: "If you do not specify this parameter, the operation defaults
/// to 10 ... per response."
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

/// Every `@paginated` list operation this service implements, each with
/// [`SEEDED`] resources of its kind already persisted.
///
/// `ListPolicyStores` is not here: it is still the scaffold, and it joins this
/// table when its own issue implements it.
async fn seeded_listings(router: &MultiServiceRouter) -> Vec<Listing> {
    let store_id = store(router).await;

    for i in 0..SEEDED {
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
                "aliasName": format!("{ALIAS_PREFIX}alias-{i:03}"),
                "policyStoreId": store_id,
            }),
        )
        .await;
    }

    // One pool, referenced by every seeded identity source: an identity source
    // can only name a user pool the user-pool service really holds.
    let pool = user_pool_arn(SIGNING_REGION, &user_pool(router, "pagination-pool").await);
    for i in 0..SEEDED {
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
        Listing {
            operation: "ListPolicyStoreAliases",
            items_member: "policyStoreAliases",
            id_member: "aliasName",
            default_page_size: ALIAS_DEFAULT_PAGE_SIZE,
            request: json!({}),
        },
    ]
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

/// Every list operation refuses a `nextToken` it did not mint the same way: HTTP
/// 400 `ValidationException`, whose `fieldList` names `/nextToken` — not an empty
/// page, which would hide the client's mistake.
#[tokio::test]
async fn every_list_operation_refuses_a_forged_cursor_the_same_way() {
    let router = super::harness::router();
    let listings = seeded_listings(&router).await;

    for listing in &listings {
        // Rows differ along HOW the cursor is wrong: not this service's encoding
        // at all, an odd number of hex digits, and hex that decodes to bytes
        // which are not text.
        for (how, token) in [
            ("not a cursor", "notacursor"),
            ("odd-length hex", "953"),
            ("hex that is not text", "ff"),
        ] {
            let case = format!("{}: {how}", listing.operation);
            let mut request = listing.request.clone();
            request["nextToken"] = json!(token);
            let (status, body) = call(&router, listing.operation, &request).await;
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
    }
}
