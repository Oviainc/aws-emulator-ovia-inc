// SPDX-License-Identifier: CC0-1.0

//! Which refusal a store-scoped paginated listing earns when the request is
//! faulty in TWO ways at once: its `policyStoreId` references a policy store
//! this service does not hold AND its `nextToken` is a cursor no listing of this
//! service minted for the enumeration being asked for.
//!
//! The rule under test: a list operation resolves the policy store its request
//! REFERENCES before it reads the request's own pagination cursor, so a
//! reference that names nothing answers the 404 `ResourceNotFoundException`
//! whatever cursor travelled with it — while the very same cursor against the
//! store that DOES exist earns its 400 `ValidationException` naming
//! `/nextToken`. What is pinned here is the PRECEDENCE and not a blanket 404:
//! the answer for one unknown store may never depend on which cursor
//! accompanied it, which is what sends a client paging a store that was deleted
//! mid-walk off to re-mint a token against a store that is gone.
//!
//! `tests/all/not_found_precedes_payload_refusal.rs` pins the same precedence
//! for the operations whose refused payload is a Cedar one; these three
//! listings' refused payload is the cursor, which is what this suite adds.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so
//! each answer is the one a client would really receive.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, error_code, field_list, ok, router, store_with_validation_off, user_pool, user_pool_arn,
    SIGNING_REGION,
};

/// A policy store id no store in a fresh engine carries.
const UNKNOWN_STORE: &str = "PSdoesnotexist11111111";

/// The prefix a `policyStoreId` carries when it references a store by ALIAS,
/// from `CreatePolicyStoreAliasInput$aliasName`'s model documentation: "The alias
/// name must always be prefixed with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// The alias name no alias in a fresh engine carries.
const UNKNOWN_ALIAS_NAME: &str = "not-an-alias";

/// The member a refused pagination cursor is reported against.
const NEXT_TOKEN_PATH: &str = "/nextToken";

/// How many resources of each kind a seeded store carries: enough that a page of
/// one leaves a cursor behind, so a real cursor can be minted for every listing.
const SEEDED_PER_STORE: usize = 2;

/// One of this service's paginated listings that enumerates the resources WITHIN
/// one policy store — the listings whose request carries both a `policyStoreId`
/// reference and a `nextToken`.
///
/// From the model's `@paginated` operations taking a required `policyStoreId`
/// (`smithy_query(query="operation", operation="ListPolicies,ListIdentitySources,
/// ListPolicyTemplates")`). `ListPolicyStoreAliases` is not one of them: it
/// enumerates the account's aliases and takes no `policyStoreId`, so it has no
/// store reference to resolve first.
struct Listing {
    /// The operation to call.
    operation: &'static str,
    /// The required output member carrying the page's items.
    items_member: &'static str,
}

/// Every store-scoped paginated listing this service implements.
const LISTINGS: &[Listing] = &[
    Listing {
        operation: "ListPolicies",
        items_member: "policies",
    },
    Listing {
        operation: "ListIdentitySources",
        items_member: "identitySources",
    },
    Listing {
        operation: "ListPolicyTemplates",
        items_member: "policyTemplates",
    },
];

/// What a row expects of the call.
enum Expect {
    /// HTTP 404 `ResourceNotFoundException` naming the resource the reference
    /// failed to resolve to.
    NotFound {
        resource_type: &'static str,
        resource_id: String,
    },
    /// HTTP 400 `ValidationException` naming `/nextToken`.
    CursorRefused,
    /// HTTP 200 carrying a page.
    Page,
}

/// Seed one policy store with [`SEEDED_PER_STORE`] resources of every kind the
/// listings above enumerate, and return its `policyStoreId`.
///
/// `label` distinguishes the one resource whose name is unique account-wide — the
/// user pool an identity source references — so two stores can be seeded on one
/// router.
async fn seed_store(router: &MultiServiceRouter, label: &str) -> String {
    let store_id = store_with_validation_off(router).await;

    // One pool, referenced by every seeded identity source: an identity source
    // can only name a user pool the user-pool service really holds.
    let pool = user_pool_arn(
        SIGNING_REGION,
        &user_pool(router, &format!("{label}-cursor-precedence-pool")).await,
    );

    for i in 0..SEEDED_PER_STORE {
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
                "name": format!("{label}-template-{i:03}"),
                "statement": "permit(principal == ?principal, action == Action::\"view\", \
                              resource == Photo::\"p\");",
            }),
        )
        .await;
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

    store_id
}

/// One page of `listing` against `reference`, with `next_token` set only when the
/// row supplies one — so a row tests the ABSENT member rather than a stand-in
/// value.
async fn call_listing(
    router: &MultiServiceRouter,
    listing: &Listing,
    reference: &str,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({ "policyStoreId": reference, "maxResults": 1 });
    if let Some(next_token) = next_token {
        request["nextToken"] = json!(next_token);
    }
    call(router, listing.operation, &request).await
}

/// Mint one real cursor for `listing` over `store_id`, by asking it for a page
/// smaller than what the store holds. The token is whatever the service issued —
/// this suite never builds one itself.
async fn minted_cursor(router: &MultiServiceRouter, listing: &Listing, store_id: &str) -> String {
    let (status, body) = call_listing(router, listing, store_id, None).await;
    assert_eq!(
        status, 200,
        "case {:?}: a page over the seeded store must succeed, got {status} {body}",
        listing.operation
    );
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

/// Assert one answer against what the row expects of it.
fn assert_answer(case: &str, listing: &Listing, expected: &Expect, status: u16, body: &Value) {
    match expected {
        Expect::NotFound {
            resource_type,
            resource_id,
        } => {
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(*resource_type),
                "case {case:?}: expected resourceType {resource_type}, got {body}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(resource_id.as_str()),
                "case {case:?}: expected resourceId {resource_id}, got {body}"
            );
        }
        Expect::CursorRefused => {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            let paths: Vec<String> = field_list(body)
                .into_iter()
                .map(|(path, _)| path)
                .collect();
            assert_eq!(
                paths,
                vec![NEXT_TOKEN_PATH.to_string()],
                "case {case:?}: only the cursor is at fault, got {body}"
            );
        }
        Expect::Page => {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            assert!(
                body[listing.items_member].is_array(),
                "case {case:?}: {} is required on the response, got {body}",
                listing.items_member
            );
        }
    }
}

/// A `policyStoreId` that resolves to no policy store outranks the refusal of the
/// request's own cursor, on every store-scoped listing — and the same cursor
/// against the store that DOES exist still earns its 400 naming `/nextToken`.
#[tokio::test]
async fn an_unresolvable_policy_store_outranks_a_cursor_refusal() {
    let router = router();
    let live_store = seed_store(&router, "live").await;
    let other_store = seed_store(&router, "other").await;

    for listing in LISTINGS {
        // A cursor of the SAME operation for another store: a token this service
        // really minted, which still belongs to no enumeration of `live_store`.
        let foreign_cursor = minted_cursor(&router, listing, &other_store).await;

        // The rows differ along HOW the cursor fails the listing under test:
        // absent altogether, a value that is not this service's encoding, hex
        // that decodes to text carrying no listing, and a cursor this service
        // minted for a different store. Each carries what the LIVE store must
        // answer for it, so the 404 rows below are the only thing the unknown
        // reference changes.
        let cursors: [(&str, Option<String>, Expect); 4] = [
            ("no cursor at all", None, Expect::Page),
            (
                "a token that is not this service's encoding",
                Some("notacursor".to_string()),
                Expect::CursorRefused,
            ),
            (
                "hex of text carrying no listing",
                Some(hex::encode("zz")),
                Expect::CursorRefused,
            ),
            (
                "a cursor this service minted for another store",
                Some(foreign_cursor),
                Expect::CursorRefused,
            ),
        ];

        for (cursor_case, cursor, live_expectation) in &cursors {
            // The rows differ along WHAT the `policyStoreId` names: nothing at
            // all, an alias naming nothing, and the store that does exist.
            let references: [(&str, String, Expect); 3] = [
                (
                    "a store id no store carries",
                    UNKNOWN_STORE.to_string(),
                    Expect::NotFound {
                        resource_type: "POLICY_STORE",
                        resource_id: UNKNOWN_STORE.to_string(),
                    },
                ),
                (
                    "an alias naming nothing",
                    format!("{ALIAS_PREFIX}{UNKNOWN_ALIAS_NAME}"),
                    // The reference named an ALIAS, so the alias is the resource
                    // reported missing, by its bare name.
                    Expect::NotFound {
                        resource_type: "POLICY_STORE_ALIAS",
                        resource_id: UNKNOWN_ALIAS_NAME.to_string(),
                    },
                ),
                (
                    "the store that does exist",
                    live_store.clone(),
                    match live_expectation {
                        Expect::Page => Expect::Page,
                        _ => Expect::CursorRefused,
                    },
                ),
            ];

            for (reference_case, reference, expected) in &references {
                let case = format!(
                    "{} with {cursor_case} against {reference_case}",
                    listing.operation
                );
                let (status, body) =
                    call_listing(&router, listing, reference, cursor.as_deref()).await;
                assert_answer(&case, listing, expected, status, &body);
            }
        }
    }
}

/// The consequence a client sees: a store deleted part-way through a walk answers
/// the walk's own cursor with the 404 that says the store is gone, rather than a
/// 400 about a cursor that was perfectly good when it was issued.
#[tokio::test]
async fn a_store_deleted_mid_walk_reports_the_missing_store_not_its_cursor() {
    let router = router();

    for listing in LISTINGS {
        // A store per listing, so each walk is interrupted on its own store.
        let store_id = seed_store(&router, &format!("mid-walk-{}", listing.operation)).await;
        let cursor = minted_cursor(&router, listing, &store_id).await;

        // The cursor resumes the walk while the store is there, so what changes
        // below is the store and not the token.
        let case = format!("{}: the walk resumes before the delete", listing.operation);
        let (status, body) = call_listing(&router, listing, &store_id, Some(&cursor)).await;
        assert_answer(&case, listing, &Expect::Page, status, &body);

        ok(
            &router,
            "DeletePolicyStore",
            &json!({ "policyStoreId": store_id }),
        )
        .await;

        let case = format!("{}: the walk continues after the delete", listing.operation);
        let (status, body) = call_listing(&router, listing, &store_id, Some(&cursor)).await;
        assert_answer(
            &case,
            listing,
            &Expect::NotFound {
                resource_type: "POLICY_STORE",
                resource_id: store_id.clone(),
            },
            status,
            &body,
        );
    }
}
