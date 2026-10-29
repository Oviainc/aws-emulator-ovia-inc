// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the token rule the listings of this service that page an
//! ACCOUNT-KEYED collection share.
//!
//! The rule: a `NextToken` names a page boundary only in the collection that
//! minted it, so a token minted by one of these listings is refused by another
//! rather than resumed from as an arbitrary key — a boundary of one collection
//! may be absent from the next, or sort past entries the next does hold, so
//! resuming from it there would silently drop them and report a short page as a
//! complete one. A BARE account id, carrying no collection at all, is likewise
//! no boundary of any of them. A refused call reports nothing and leaves the
//! page as it was, and the token a collection does mint still walks that
//! collection's whole page.
//!
//! Asserted over a TABLE whose rows are the account-keyed listings of this
//! service: `ListAdminAccountsForOrganization` (the organisation's
//! administrators), `ListMemberAccounts` (every account the control plane was
//! told about) and `ListAdminsManagingAccount` (the administrators that manage
//! the caller). The rows therefore differ in the collection they enumerate, in
//! the FORM of a page item (a summary object carrying the account id, or the
//! account id itself) and in the error the operation refuses a token naming no
//! boundary of its page with.
//!
//! The three collections are seeded to differ from each other and to INTERLEAVE
//! in ascending account-id order — the order every one of these pages is served
//! in — so a cross-collection token truncates one page to a suffix and another
//! to nothing at all; [`the_collections_this_suite_pages_differ_and_interleave`]
//! pins that precondition, so the refusal rows below cannot quietly stop
//! covering it.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`] — so the assertions
//! are about the WIRE response.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::admin_account_scopes::{full_admin_scope, put_admin_account, wait_out_the_onboarding};
use crate::administrator_gated_operations::associate_admin;
use crate::compliance_status::put_policy;
use crate::policy_lifecycle::{call, emulator, error_code, ACCOUNT};

/// An administrator `PutAdminAccount` onboards with a scope covering every
/// account, so it manages the caller too: it belongs to all three collections.
/// The model constrains `AWSAccountId` only to `^[0-9]+$` with length 1..1024,
/// so the ids this suite uses differ in length and in digits.
const FULL_SCOPE_ADMIN: &str = "100000000001";

/// An administrator onboarded with an `AccountScope` naming one OTHER account,
/// so it administers nothing of the caller's: it is an administrator of the
/// organisation and an account of it, but it does not manage the caller.
const NARROW_SCOPE_ADMIN: &str = "500000000005";

/// The one account the narrow administrator's scope does name — not the caller,
/// which is what keeps that administrator out of the managing-administrator
/// collection.
const SCOPED_AWAY_ACCOUNT: &str = "555566667777";

/// Accounts a stored policy names under the `ACCOUNT` key of its `IncludeMap`:
/// accounts of the organisation that are no administrators of it. Both sort
/// ABOVE every administrator above (`"7"` and `"9…"` beyond `"1…"`, `"5…"` and
/// the caller's `"0…"`), so a boundary naming one of them empties the
/// administrator pages entirely.
const POLICY_SCOPED_ACCOUNTS: &[&str] = &["900000000009", "7"];

/// One account-keyed listing: the operation that serves it, the output member
/// carrying its items, the member of an item that is the account id the page is
/// ordered by (`None` when the item IS that id), the modelled error code it
/// refuses a token naming no boundary of its page with, the collection its
/// boundaries are stamped with, and the accounts its page holds in the order it
/// serves them.
struct AccountPage {
    operation: &'static str,
    items: &'static str,
    key_member: Option<&'static str>,
    refusal: &'static str,
    scope: &'static str,
    expected: Vec<String>,
}

impl AccountPage {
    /// The `NextToken` this listing mints for the boundary `account`.
    fn token(&self, account: &str) -> String {
        state::encode_account_page_token(self.scope, account)
    }
}

/// The accounts of `keys` in the ascending account-id order every one of these
/// pages is served in: SORTED here rather than written out, so no row's own
/// ordering can be baked into it.
fn ascending(keys: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = keys.iter().map(|key| (*key).to_string()).collect();
    sorted.sort();
    sorted
}

/// The account-keyed listings the rule is asserted over, each with the
/// collection [`seeded`] gives it.
fn pages() -> Vec<AccountPage> {
    let mut member_accounts = vec![ACCOUNT, FULL_SCOPE_ADMIN, NARROW_SCOPE_ADMIN];
    member_accounts.extend_from_slice(POLICY_SCOPED_ACCOUNTS);
    vec![
        AccountPage {
            operation: "ListAdminAccountsForOrganization",
            items: "AdminAccounts",
            key_member: Some("AdminAccount"),
            // The model declares no client-input error for this operation, so a
            // request it refuses is a `ValidationException`.
            refusal: "ValidationException",
            scope: state::ADMIN_ACCOUNTS_FOR_ORGANIZATION_PAGE,
            expected: ascending(&[ACCOUNT, FULL_SCOPE_ADMIN, NARROW_SCOPE_ADMIN]),
        },
        AccountPage {
            operation: "ListMemberAccounts",
            items: "MemberAccounts",
            key_member: None,
            refusal: "ValidationException",
            scope: state::MEMBER_ACCOUNTS_PAGE,
            expected: ascending(&member_accounts),
        },
        AccountPage {
            operation: "ListAdminsManagingAccount",
            items: "AdminAccounts",
            key_member: None,
            refusal: "InvalidInputException",
            scope: state::ADMINS_MANAGING_ACCOUNT_PAGE,
            expected: ascending(&[ACCOUNT, FULL_SCOPE_ADMIN]),
        },
    ]
}

/// A fresh emulator whose three account-keyed collections differ and interleave,
/// each resource created the way a client creates it.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    // The default administrator, which is also what opens the organisation-scoped
    // gate these listings sit behind.
    associate_admin(&router).await;
    put_admin_account(
        &router,
        "full-scope administrator",
        FULL_SCOPE_ADMIN,
        Some(&full_admin_scope()),
    )
    .await;
    let narrow_scope = json!({
        "AccountScope": { "Accounts": [SCOPED_AWAY_ACCOUNT] },
        "OrganizationalUnitScope": { "AllOrganizationalUnitsEnabled": true },
        "RegionScope": { "AllRegionsEnabled": true },
        "PolicyTypeScope": { "AllPolicyTypesEnabled": true },
    });
    put_admin_account(
        &router,
        "narrow-scope administrator",
        NARROW_SCOPE_ADMIN,
        Some(&narrow_scope),
    )
    .await;
    // An onboarded administrator manages the caller only once its onboarding has
    // completed, so the managing-administrator collection is read after the span
    // has passed.
    wait_out_the_onboarding();
    put_policy(
        &router,
        "organisation-roster",
        Some(json!({ "ACCOUNT": POLICY_SCOPED_ACCOUNTS })),
        None,
    )
    .await;
    router
}

/// One call of a listing, as `(http status, error code or "", the account ids
/// served, NextToken)`.
async fn page(
    router: &MultiServiceRouter,
    listing: &AccountPage,
    request: &Value,
) -> (u16, String, Vec<String>, Option<String>) {
    let (status, body) = call(router, listing.operation, request).await;
    let code = if status == 200 {
        String::new()
    } else {
        error_code(&body)
    };
    let served: Vec<String> = body[listing.items]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            let key = match listing.key_member {
                Some(member) => &item[member],
                None => item,
            };
            key.as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {}: a page item must carry its account id, got {item:?}",
                        listing.operation
                    )
                })
                .to_string()
        })
        .collect();
    let next_token = body
        .get("NextToken")
        .filter(|token| !token.is_null())
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, code, served, next_token)
}

/// The listing's whole page, asserted to be served, to carry no token and to
/// hold exactly the collection the table says it does.
async fn whole(router: &MultiServiceRouter, listing: &AccountPage) -> Vec<String> {
    let operation = listing.operation;
    let (status, code, served, next) = page(router, listing, &json!({})).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case {operation}: expected the unbounded page to be served, got {status} / {code:?}"
    );
    assert_eq!(
        (served.clone(), next.clone()),
        (listing.expected.clone(), None),
        "case {operation}: expected the whole collection in ascending account-id order and no \
         NextToken, got {served:?} / {next:?}"
    );
    served
}

/// The precondition the refusal rows rest on: the three collections are
/// pairwise different, and each pair has a boundary of the one that sorts past
/// at least one entry of the other — so resuming from a foreign boundary would
/// silently drop entries rather than being harmless.
#[tokio::test]
async fn the_collections_this_suite_pages_differ_and_interleave() {
    let router = seeded().await;
    let listings = pages();
    for listing in &listings {
        let served = whole(&router, listing).await;
        for other in &listings {
            if other.operation == listing.operation {
                continue;
            }
            let other_served = whole(&router, other).await;
            assert_ne!(
                served, other_served,
                "case {} vs {}: this suite needs the two collections to differ, got {served:?} \
                 for both",
                listing.operation, other.operation
            );
            let truncating = other_served
                .iter()
                .any(|boundary| served.iter().any(|account| account < boundary));
            assert!(
                truncating,
                "case {} vs {}: this suite needs a boundary of the second to sort past an entry \
                 of the first, got {served:?} against {other_served:?}",
                listing.operation, other.operation
            );
        }
    }
}

/// A token minted by ANOTHER account-keyed listing — and a bare account id,
/// stamped with no collection at all — names no boundary of this listing's page:
/// each is refused with the operation's modelled error, reports nothing, and
/// leaves the page as it was.
#[tokio::test]
async fn a_token_of_another_account_keyed_collection_is_refused() {
    let router = seeded().await;
    let listings = pages();
    for listing in &listings {
        let operation = listing.operation;
        let expected = whole(&router, listing).await;

        let mut foreign: Vec<(String, String)> = Vec::new();
        for other in &listings {
            if other.operation == operation {
                continue;
            }
            // Every boundary of the other collection, so the rows cover a
            // boundary that sorts inside this page, one that sorts past all of
            // it, and one it does not hold at all.
            for account in &other.expected {
                foreign.push((
                    format!("a token {} minted for {account}", other.operation),
                    other.token(account),
                ));
            }
        }
        // The unstamped form: the hex of a bare account id, which names a
        // boundary of no collection — including this page's own accounts.
        for account in &expected {
            foreign.push((
                format!("the bare account id {account}, carrying no collection"),
                hex::encode(account),
            ));
        }

        for (case, token) in foreign {
            let (status, code, served, next) =
                page(&router, listing, &json!({ "NextToken": token })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, listing.refusal),
                "case {operation} / {case}: a token naming no boundary of this page must be \
                 refused, got {status} / {code:?}"
            );
            assert!(
                served.is_empty() && next.is_none(),
                "case {operation} / {case}: a refused call must report nothing, got {served:?} / \
                 {next:?}"
            );

            let (_, _, after, _) = page(&router, listing, &json!({})).await;
            assert_eq!(
                after, expected,
                "case {operation} / {case}: a refused token must leave the page as it was, got \
                 {after:?}"
            );
        }
    }
}

/// The token a listing MINTS names a boundary of its own collection: it resumes
/// that page exactly at that account, and walking the chain reports the whole
/// collection exactly once at every page size.
#[tokio::test]
async fn a_listings_own_token_resumes_and_walks_its_own_collection() {
    let router = seeded().await;
    for listing in &pages() {
        let operation = listing.operation;
        let expected = whole(&router, listing).await;

        for (index, account) in expected.iter().enumerate() {
            let (status, code, served, _) = page(
                &router,
                listing,
                &json!({ "NextToken": listing.token(account) }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {operation} / boundary {account}: a token this page minted must resume it, \
                 got {status} / {code:?}"
            );
            assert_eq!(
                served,
                expected[index..].to_vec(),
                "case {operation} / boundary {account}: expected the page to resume at that \
                 account, got {served:?}"
            );
        }

        // No bound, the smallest the model allows (`PaginationMaxResults` range
        // 1..100) and a size either side of the collection's own length.
        for max_results in [None, Some(1), Some(2), Some(100)] {
            let case = format!("MaxResults {max_results:?}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut ended = false;
            // One more round than the chain can need, so a token that never
            // clears fails here instead of looping forever.
            for round in 0..expected.len() + 3 {
                let mut request = json!({});
                if let Some(max) = max_results {
                    request["MaxResults"] = json!(max);
                }
                if let Some(token) = token.as_deref() {
                    request["NextToken"] = json!(token);
                }
                let (status, code, served, next) = page(&router, listing, &request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (200, ""),
                    "case {operation} / {case} / page {round}: expected 200, got {status} / \
                     {code:?}"
                );
                if let Some(max) = max_results {
                    assert!(
                        served.len() <= max,
                        "case {operation} / {case} / page {round}: a page must not exceed the \
                         requested size, got {served:?}"
                    );
                }
                walked.extend(served);
                match next {
                    Some(next) => token = Some(next),
                    None => {
                        ended = true;
                        break;
                    }
                }
            }
            assert!(
                ended,
                "case {operation} / {case}: the NextToken chain did not end, walked {walked:?}"
            );
            assert_eq!(
                walked, expected,
                "case {operation} / {case}: walking this listing's own tokens must report its \
                 whole collection exactly once in ascending account-id order, got {walked:?}"
            );
        }
    }
}
