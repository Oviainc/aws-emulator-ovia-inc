// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAdminAccountsForOrganization`: the enumeration of
//! the Firewall Manager administrator accounts an organization holds.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the router, reusing the request harness in
//! [`crate::policy_lifecycle`] — so the assertions are about the WIRE response.
//!
//! The rules asserted here, each over a TABLE rather than one request:
//!
//! * the enumeration reports every administrator the organization holds, and
//!   only those: the ONE default administrator `AssociateAdminAccount`
//!   associated (`DefaultAdmin` true) and every account `PutAdminAccount`
//!   onboarded (`DefaultAdmin` false), each carrying the `OrganizationStatus`
//!   that holds at the instant of the call;
//! * the status an administrator carries is the CLOCK's: a just-onboarded
//!   account is still `ONBOARDING` and reaches `ONBOARDING_COMPLETE` when its
//!   onboarding span has passed, and a reading never moves it;
//! * the page is ordered by account id and bounded by `MaxResults`, so walking
//!   the `NextToken` chain reports every administrator exactly once whatever the
//!   page size, and the last page carries no token;
//! * a `NextToken` this service did not mint names no page boundary and is
//!   refused, while one it did resumes the enumeration at that account;
//! * the call is organisation-scoped, so with no administrator association it is
//!   refused before it reports anything.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::ORGANIZATION_STATUS_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::admin_account_disassociation::wait_out_the_disassociation;
use crate::admin_account_scopes::{
    organization_status, put_admin_account, wait_out_the_onboarding,
};
use crate::administrator_gated_operations::associate_admin;
use crate::policy_lifecycle::{call, call_in, emulator, error_code, ACCOUNT, REGION};

/// A second Region, to assert the answer does not depend on which Region the
/// request addresses.
const OTHER_REGION: &str = "eu-central-1";

/// Account ids used as administrators below. The model constrains
/// `AWSAccountId` only to `^[0-9]+$` with length 1..1024, so these differ in
/// length and in digits — including the one-digit and the long forms. They are
/// deliberately NOT in ascending order here: what the ordering rule expects is
/// derived by sorting, never written out by hand.
const ADMIN_ACCOUNTS: &[&str] = &[
    "123456789012",
    "7",
    "000000000001",
    "99999999999999999999",
    "1",
];

/// One administrator as the enumeration reports it: the account, whether it is
/// the default administrator, and its `OrganizationStatus`.
type Summary = (String, bool, String);

/// `ListAdminAccountsForOrganization` with `request`, as `(http status, error
/// code or "", the administrators reported, NextToken)`.
///
/// Every `Status` that comes back is checked against the model's own
/// `OrganizationStatus` catalog here, so no assertion below can pass on a value
/// the enum does not carry.
async fn list_admins(
    router: &MultiServiceRouter,
    request: &Value,
    region: &str,
) -> (u16, String, Vec<Summary>, Option<String>) {
    let (status, body) = call_in(router, "ListAdminAccountsForOrganization", request, region).await;
    let code = if status == 200 {
        String::new()
    } else {
        error_code(&body)
    };
    let reported: Vec<Summary> = body["AdminAccounts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            let reported_status = summary["Status"].as_str().unwrap_or_default().to_string();
            assert!(
                ORGANIZATION_STATUS_VALUES.contains(&reported_status.as_str()),
                "every reported Status must be a modelled OrganizationStatus (catalog \
                 {ORGANIZATION_STATUS_VALUES:?}), got {reported_status:?} in {summary:?}"
            );
            (
                summary["AdminAccount"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                summary["DefaultAdmin"].as_bool().unwrap_or(false),
                reported_status,
            )
        })
        .collect();
    let next_token = body
        .get("NextToken")
        .filter(|token| !token.is_null())
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, code, reported, next_token)
}

/// The whole enumeration, in the order the service reports it, asking for no
/// page bound — which still bounds the page, so the token chain is followed to
/// the end.
async fn every_admin(router: &MultiServiceRouter, case: &str) -> Vec<Summary> {
    walk(router, None, case).await
}

/// Walk the `NextToken` chain with `max_results` (none for an unbounded
/// request), asserting at each step that a page is no larger than it asked for
/// and that only the LAST page carries no token.
async fn walk(router: &MultiServiceRouter, max_results: Option<i64>, case: &str) -> Vec<Summary> {
    let mut collected: Vec<Summary> = Vec::new();
    let mut token: Option<String> = None;
    // One more round than there can be administrators, so a chain that never
    // ends fails here instead of looping forever.
    for round in 0..ADMIN_ACCOUNTS.len() + 3 {
        let mut request = json!({});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(ref token) = token {
            request["NextToken"] = json!(token);
        }
        let (status, code, page, next) = list_admins(router, &request, REGION).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?} / MaxResults {max_results:?} / page {round}: expected 200, got \
             {status} / {code:?}"
        );
        if let Some(max) = max_results {
            assert!(
                page.len() <= usize::try_from(max).unwrap_or(usize::MAX),
                "case {case:?} / MaxResults {max_results:?} / page {round}: a page must not \
                 exceed the requested size, got {} entries: {page:?}",
                page.len()
            );
        }
        assert!(
            !page.is_empty() || next.is_none(),
            "case {case:?} / MaxResults {max_results:?} / page {round}: an empty page must not \
             carry a NextToken, got {next:?}"
        );
        collected.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => return collected,
        }
    }
    panic!("case {case:?} / MaxResults {max_results:?}: the NextToken chain did not end");
}

/// What the enumeration must report for `default_admin` and `onboarded`, in
/// account-id order: the expectation is SORTED here rather than written out, so
/// a row's own ordering cannot be baked into it.
fn expected(default_admin: &str, onboarded: &[&str], onboarded_status: &str) -> Vec<Summary> {
    let mut rows: Vec<Summary> = onboarded
        .iter()
        .map(|account| {
            (
                (*account).to_string(),
                false,
                onboarded_status.to_string(),
            )
        })
        .collect();
    rows.push((
        default_admin.to_string(),
        true,
        organization_status("ONBOARDING_COMPLETE").to_string(),
    ));
    rows.sort();
    rows
}

/// With no administrator association the call is organisation-scoped and
/// refused — in either Region, and whatever page parameters it carries — so it
/// reports nothing about the organization.
#[tokio::test]
async fn with_no_administrator_association_the_enumeration_is_refused() {
    let requests = [
        ("no members at all", json!({})),
        ("a page bound", json!({ "MaxResults": 1 })),
        (
            "a token",
            json!({ "NextToken": hex::encode(ADMIN_ACCOUNTS[0]) }),
        ),
    ];

    for region in [REGION, OTHER_REGION] {
        for (case, request) in &requests {
            let router = emulator();
            let (status, code, reported, token) = list_admins(&router, request, region).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "AccessDeniedException"),
                "case {case:?} in {region}: with no administrator association the call must be \
                 refused, got {status} / {code:?}"
            );
            assert!(
                reported.is_empty() && token.is_none(),
                "case {case:?} in {region}: a refused call must report nothing, got \
                 {reported:?} / {token:?}"
            );
        }
    }
}

/// One row of the organization table: which account holds the default
/// administrator association, and which accounts are onboarded beside it.
struct OrganizationCase {
    name: &'static str,
    default_admin: &'static str,
    onboarded: Vec<&'static str>,
}

/// The organization class the reporting rule is asserted over: the rows differ
/// in how many administrators are onboarded beside the default one (none, one,
/// several, every id in the table), in which account holds the default
/// association, and in the FORM of those account ids.
fn organization_cases() -> Vec<OrganizationCase> {
    vec![
        OrganizationCase {
            name: "the default administrator alone",
            default_admin: ACCOUNT,
            onboarded: Vec::new(),
        },
        OrganizationCase {
            name: "one onboarded administrator",
            default_admin: ACCOUNT,
            onboarded: vec![ADMIN_ACCOUNTS[0]],
        },
        OrganizationCase {
            name: "several onboarded administrators",
            default_admin: ACCOUNT,
            onboarded: vec![ADMIN_ACCOUNTS[1], ADMIN_ACCOUNTS[2], ADMIN_ACCOUNTS[0]],
        },
        OrganizationCase {
            // A default administrator that is not the calling account, and a
            // one-digit onboarded id beside a twenty-digit one.
            name: "a default administrator that is not the caller",
            default_admin: ADMIN_ACCOUNTS[3],
            onboarded: vec![ADMIN_ACCOUNTS[4], ADMIN_ACCOUNTS[0]],
        },
    ]
}

/// The enumeration reports every administrator the organization holds — the
/// default one and each onboarded account — with the role and the status that
/// hold at the instant of the call, in account-id order; and a reading never
/// moves a status itself.
#[tokio::test]
async fn every_administrator_is_reported_with_its_role_and_clock_derived_status() {
    for case in organization_cases() {
        let name = case.name;
        let router = emulator();

        // The default administrator association, made the way a client makes
        // it. `AssociateAdminAccount` returns the role ready, so the account is
        // onboarded from the moment it is associated.
        let (status, body) = call(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": case.default_admin }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected AssociateAdminAccount to answer 200, got {status} with \
             {body:?}"
        );

        let reported = every_admin(&router, name).await;
        assert_eq!(
            reported,
            expected(case.default_admin, &[], ""),
            "case {name:?}: before any account is onboarded only the default administrator is \
             reported, got {reported:?}"
        );

        for account in &case.onboarded {
            put_admin_account(&router, name, account, None).await;
        }

        // Straight after the writes: the onboarding span is never zero, so the
        // caller still sees the transient status.
        let reported = every_admin(&router, name).await;
        assert_eq!(
            reported,
            expected(
                case.default_admin,
                &case.onboarded,
                organization_status("ONBOARDING")
            ),
            "case {name:?}: a just-onboarded administrator is still onboarding, got {reported:?}"
        );

        // A read changes nothing a later caller sees.
        for round in 1..4 {
            let again = every_admin(&router, name).await;
            assert_eq!(
                again, reported,
                "case {name:?}: read {round} must answer identically, got {again:?} after \
                 {reported:?}"
            );
        }

        wait_out_the_onboarding();
        let settled = every_admin(&router, name).await;
        assert_eq!(
            settled,
            expected(
                case.default_admin,
                &case.onboarded,
                organization_status("ONBOARDING_COMPLETE")
            ),
            "case {name:?}: once the onboarding span has passed every administrator is \
             onboarded, got {settled:?}"
        );

        // The Region the request is signed for does not change what the
        // organization holds.
        let (status, code, in_other_region, _) =
            list_admins(&router, &json!({}), OTHER_REGION).await;
        assert_eq!(
            (status, code.as_str(), in_other_region.clone()),
            (200, "", settled.clone()),
            "case {name:?}: the enumeration must not depend on the request's Region, got \
             {status} / {code:?} / {in_other_region:?}"
        );

        // `GetAdminScope` agrees with the enumeration, administrator by
        // administrator, so the two readers of the store cannot disagree.
        for (account, default_admin, status_reported) in &settled {
            let (status, body) =
                call(&router, "GetAdminScope", &json!({ "AdminAccount": account })).await;
            assert_eq!(
                (status, body["Status"].as_str()),
                (200, Some(status_reported.as_str())),
                "case {name:?} / {account} (default: {default_admin}): GetAdminScope must \
                 report the same status as the enumeration, got {status} with {body:?}"
            );
        }
    }
}

/// Walking the `NextToken` chain reports every administrator exactly once,
/// whatever the page size — and a `MaxResults` outside the modelled range is
/// refused.
#[tokio::test]
async fn the_token_chain_reports_every_administrator_exactly_once_at_every_page_size() {
    let router = emulator();
    associate_admin(&router).await;
    for account in ADMIN_ACCOUNTS {
        put_admin_account(&router, "pagination", account, None).await;
    }
    wait_out_the_onboarding();

    let whole = expected(
        ACCOUNT,
        ADMIN_ACCOUNTS,
        organization_status("ONBOARDING_COMPLETE"),
    );

    // No bound, the smallest and the largest the model allows
    // (`PaginationMaxResults` range 1..100), and sizes either side of the
    // administrator count.
    for max_results in [None, Some(1), Some(2), Some(5), Some(6), Some(100)] {
        let case = format!("MaxResults {max_results:?}");
        let walked = walk(&router, max_results, &case).await;
        assert_eq!(
            walked, whole,
            "case {case}: walking the token chain must report every administrator exactly once, \
             in account-id order, got {walked:?}"
        );
    }

    // One past each published bound of `PaginationMaxResults`.
    for past_bound in [0, 101] {
        let (status, _, reported, _) =
            list_admins(&router, &json!({ "MaxResults": past_bound }), REGION).await;
        assert_eq!(
            status, 400,
            "case MaxResults {past_bound}: a page size outside the modelled 1..100 range must \
             be refused, got {status} with {reported:?}"
        );
    }
}

/// A `NextToken` this service minted resumes the enumeration at that account; a
/// token it did not mint names no page boundary and is refused as a bad request
/// rather than answered with an invented page.
#[tokio::test]
async fn a_next_token_resumes_the_page_only_when_this_service_minted_it() {
    let router = emulator();
    associate_admin(&router).await;
    for account in ADMIN_ACCOUNTS {
        put_admin_account(&router, "tokens", account, None).await;
    }
    wait_out_the_onboarding();

    let whole = expected(
        ACCOUNT,
        ADMIN_ACCOUNTS,
        organization_status("ONBOARDING_COMPLETE"),
    );

    // Every administrator is a page boundary: resuming at it reports exactly
    // the administrators at or after it.
    for (index, (account, _, _)) in whole.iter().enumerate() {
        let (status, code, reported, _) = list_admins(
            &router,
            &json!({ "NextToken": hex::encode(account) }),
            REGION,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {account:?}: a token naming a page boundary must resume the enumeration, got \
             {status} / {code:?}"
        );
        assert_eq!(
            reported,
            whole[index..].to_vec(),
            "case {account:?}: expected the resumed page to start at the boundary account, got \
             {reported:?}"
        );
    }

    let foreign = [
        ("not hex at all", "zzzz".to_string()),
        ("hex of a non-account string", hex::encode("abc")),
        (
            "a token minted for a policy page",
            hex::encode("0f13c2a4-1111-4a26-9d0e-6b4f8d2a7c31"),
        ),
        (
            "a token minted for a resource-set page",
            hex::encode("0123456789AbCdEfGhIjKl"),
        ),
        ("hex of an empty key", hex::encode("")),
    ];
    for (case, token) in foreign {
        let (status, code, reported, next) = list_admins(
            &router,
            &json!({ "NextToken": token }),
            REGION,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a token this operation did not mint must be refused, got {status} / \
             {code:?}"
        );
        assert!(
            reported.is_empty() && next.is_none(),
            "case {case:?}: a refused call must report nothing, got {reported:?} / {next:?}"
        );

        // The refusal changed nothing: the enumeration still reports the same
        // administrators.
        let after = every_admin(&router, case).await;
        assert_eq!(
            after, whole,
            "case {case:?}: a refused token must leave the enumeration as it was, got {after:?}"
        );
    }
}

/// A default administrator whose disassociation is still pending is reported as
/// being removed, and once the tear-down has completed there is no
/// administrator association left, so the organisation-scoped call is refused.
#[tokio::test]
async fn a_pending_disassociation_is_reported_as_offboarding() {
    let router = emulator();
    associate_admin(&router).await;
    let onboarded = ADMIN_ACCOUNTS[0];
    put_admin_account(&router, "offboarding", onboarded, None).await;

    let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
    assert_eq!(
        status, 200,
        "the holder's disassociation must be accepted, got {status} with {body:?}"
    );

    // The tear-down span is never zero, so a caller reading straight after the
    // call still sees the default administrator on its way out.
    let (status, code, reported, _) =
        list_admins(&router, &json!({}), REGION).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "while the disassociation is pending the association still exists, so the call must be \
         served, got {status} / {code:?}"
    );
    let default_admin: Vec<&Summary> = reported.iter().filter(|(_, is_default, _)| *is_default).collect();
    assert_eq!(
        default_admin,
        vec![&(
            ACCOUNT.to_string(),
            true,
            organization_status("OFFBOARDING").to_string()
        )],
        "a default administrator whose disassociation is pending is being removed, got \
         {reported:?}"
    );

    wait_out_the_disassociation();

    let (status, code, reported, _) =
        list_admins(&router, &json!({}), REGION).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "AccessDeniedException"),
        "once the tear-down has completed no administrator association is left, so the \
         organisation-scoped call must be refused, got {status} / {code:?} with {reported:?}"
    );
}
