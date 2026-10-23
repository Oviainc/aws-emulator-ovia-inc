// SPDX-License-Identifier: CC0-1.0

//! What the payer-account denial HIDES: a reporting operation answers a caller that is
//! not the payer account the SAME 403 whether the store holds nothing or holds the
//! resources the payer account reads back on the very next call.
//!
//! RULE under test: Billing Conductor is a PAYER-ACCOUNT-ONLY API, so every reporting
//! operation of its control plane refuses a caller that is not the payer (management)
//! account of the consolidated billing family with HTTP 403 `AccessDeniedException` — the
//! shape the model lists as a common error of every operation of this service
//! (`errorKind` client, `@httpError 403`, `@required Message`), carried in the
//! `x-amzn-errortype` header restJson1 resolves an error shape from — whose `Message` is
//! built FROM THE REQUEST (the calling account id and the operation name). The decision is
//! the CALLER's alone, so the answer carries no member of the operation's OUTPUT shape
//! whatever the store holds: "not authorized" is never observable as "authorized, zero
//! results", which is the defect
//! `issues/.../billingconductor-listbillinggroups-returns-200-instead-of-403-accessdeniedexception-for.md`
//! reported. Being a read, the denial also changes nothing: the payer account reads the
//! same result set back afterwards.
//!
//! `tests/all/payer_account_gate.rs` is the suite that asserts the gate covers every
//! modelled operation, and `tests/all/payer_gate_precedes_idempotency_existence_and_assembly.rs`
//! the stages it precedes; both hold the store EMPTY. The rows here vary what those suites
//! hold fixed — HOW MUCH the store holds (nothing, one resource of each kind, several) and
//! the SHAPE of the reporting request (no members, a `MaxResults` page bound, a modelled
//! `Filters` predicate, a `BillingPeriod`, and a `NextToken` minted for the payer's own
//! page) — over three paginated reads of three different stored collections. The harness
//! is that suite's, reused rather than copied.

use serde_json::{json, Value};

use crate::payer_account_gate::{assert_denied, error_code, Emulator};

/// One reporting read: the modelled operation, the route the model binds it to, the
/// request body, the output member its result set travels in, and how many rows the PAYER
/// account must be served for a store holding `population` resources of each kind.
struct Read {
    case: &'static str,
    operation: &'static str,
    path: &'static str,
    body: Value,
    /// The member of the operation's output shape the page travels in — the one a denial
    /// must never carry.
    member: &'static str,
    /// The page length the payer account is served, given the population below.
    served: fn(usize) -> usize,
}

/// The reporting reads of this service that answer from stored collections, each in
/// several request shapes: no members at all, a `MaxResults` page bound, and a modelled
/// `Filters` predicate (with a `BillingPeriod`, the member that selects the month a
/// billing-group report is read for).
fn reporting_reads() -> Vec<Read> {
    vec![
        Read {
            case: "ListBillingGroups, no members",
            operation: "ListBillingGroups",
            path: "/list-billing-groups",
            body: json!({}),
            member: "BillingGroups",
            served: |population| population,
        },
        Read {
            case: "ListBillingGroups, bounded to one row a page",
            operation: "ListBillingGroups",
            path: "/list-billing-groups",
            body: json!({"MaxResults": 1}),
            member: "BillingGroups",
            served: |population| population.min(1),
        },
        Read {
            case: "ListBillingGroups, a billing period and an ACTIVE status filter",
            operation: "ListBillingGroups",
            path: "/list-billing-groups",
            // Every group these rows create carries a primary account, so its derived
            // `BillingGroupStatus` is `ACTIVE` and the filter selects all of them.
            body: json!({"BillingPeriod": "2026-01", "Filters": {"Statuses": ["ACTIVE"]}}),
            member: "BillingGroups",
            served: |population| population,
        },
        Read {
            case: "ListAccountAssociations, no members",
            operation: "ListAccountAssociations",
            path: "/list-account-associations",
            body: json!({}),
            member: "LinkedAccounts",
            served: |population| population,
        },
        Read {
            case: "ListAccountAssociations, the MONITORED association filter",
            operation: "ListAccountAssociations",
            path: "/list-account-associations",
            // Each created account is a member of its group, so all of them are
            // `MONITORED`.
            body: json!({"Filters": {"Association": "MONITORED"}}),
            member: "LinkedAccounts",
            served: |population| population,
        },
        Read {
            case: "ListPricingRules, no members",
            operation: "ListPricingRules",
            path: "/list-pricing-rules",
            body: json!({}),
            member: "PricingRules",
            served: |population| population,
        },
        Read {
            case: "ListPricingRules, bounded to one row a page",
            operation: "ListPricingRules",
            path: "/list-pricing-rules",
            body: json!({"MaxResults": 1}),
            member: "PricingRules",
            served: |population| population.min(1),
        },
    ]
}

/// The linked account id of the `index`th resource set: a 12-digit account, as
/// `AccountId`'s modelled `^[0-9]{12}$` requires, and distinct per set so no account is
/// claimed by two billing groups.
fn linked_account(index: usize) -> String {
    format!("{:012}", 100_000_000_000_u64 + index as u64)
}

/// Create `population` pricing plans, billing groups and pricing rules THROUGH THE API as
/// the payer account, so what the reads below report is persisted state rather than a
/// fixture poked into the store.
///
/// Each group carries one linked account, which is also its `PrimaryAccountId`: the group
/// is then `ACTIVE` (a group needs a billing owner) and that account is `MONITORED`, so
/// one created set is exactly one row of every read above.
async fn populate(em: &Emulator, population: usize) {
    for index in 0..population {
        let (status, plan, _) = em
            .post(
                "/create-pricing-plan",
                &json!({"Name": format!("HiddenPlan{index}")}),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {index}: expected 200, got {status} with {plan}"
        );
        let plan_arn = plan["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("setup CreatePricingPlan {index}: no Arn in {plan}"))
            .to_string();

        let account = linked_account(index);
        let (status, group, _) = em
            .post(
                "/create-billing-group",
                &json!({
                    "Name": format!("HiddenGroup{index}"),
                    "AccountGrouping": {"LinkedAccountIds": [account]},
                    "ComputationPreference": {"PricingPlanArn": plan_arn},
                    "PrimaryAccountId": account,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup CreateBillingGroup {index}: expected 200, got {status} with {group}"
        );

        let (status, rule, _) = em
            .post(
                "/create-pricing-rule",
                &json!({
                    "Name": format!("HiddenRule{index}"),
                    "Scope": "GLOBAL",
                    "Type": "DISCOUNT",
                    "ModifierPercentage": 5.0,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup CreatePricingRule {index}: expected 200, got {status} with {rule}"
        );
    }
}

/// Assert the payer account is served `read`'s page over a store holding `population`
/// resource sets (`world` names that store, for the case).
async fn assert_served(em: &Emulator, world: &str, read: &Read, population: usize) {
    let case = format!("{} over {world}", read.case);
    let (status, body, _) = em.post(read.path, &read.body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the payer account to be served 200, got {status} with {body}"
    );
    let page = body[read.member].as_array().cloned().unwrap_or_default();
    assert_eq!(
        page.len(),
        (read.served)(population),
        "case {case:?}: expected {} row(s) in {}, got {body}",
        (read.served)(population),
        read.member
    );
}

/// Assert `read` is DENIED, and that the denial says nothing about the result set: no
/// member of the output shape, and nothing beyond the modelled error's own two keys.
async fn assert_denied_without_the_page(em: &Emulator, world: &str, read: &Read) {
    let case = format!("{} over {world}", read.case);
    let answered = em.post(read.path, &read.body).await;
    assert_denied(&case, read.operation, &answered);
    let (_, body, _) = &answered;
    assert!(
        body.get(read.member).is_none(),
        "case {case:?}: the denial must not carry the {} page, got {body}",
        read.member
    );
    let keys: Vec<&String> = body
        .as_object()
        .map(|object| object.keys().collect())
        .unwrap_or_default();
    assert!(
        keys.iter()
            .all(|key| key.as_str() == "__type" || key.as_str() == "Message"),
        "case {case:?}: expected only the modelled error's members, got {body}"
    );
}

/// RULE: the denial is decided from the caller, so a non-payer caller gets the identical
/// 403 — with no page in it — over a store that holds nothing, one resource set or
/// several, and the payer account reads that same population back afterwards, unchanged by
/// the refused reads.
#[tokio::test]
async fn a_reporting_read_is_denied_identically_whatever_the_store_holds() {
    // (case, how many plan/group/rule sets the payer account creates first)
    let populations = [
        ("an empty store", 0),
        ("one resource of each kind", 1),
        ("several resources of each kind", 3),
    ];

    for (world, population) in populations {
        let em = Emulator::payer();
        populate(&em, population).await;

        let reads = reporting_reads();
        // The payer account sees the population: this is the result set the denial below
        // must NOT serve.
        for read in &reads {
            assert_served(&em, world, read, population).await;
        }

        // The family's payer becomes another account, so the same caller is no longer
        // authorized — nothing about the store changed.
        em.designate_payer(Some("111122223333"));
        for read in &reads {
            assert_denied_without_the_page(&em, &format!("{world}, another payer"), read).await;
        }

        // A store that records no payer account at all denies the same way: the gate is
        // deny-by-default, not a comparison that an absent record passes.
        em.designate_payer(None);
        for read in &reads {
            assert_denied_without_the_page(&em, &format!("{world}, no payer"), read).await;
        }

        // The refused reads consumed nothing: the payer account is served the same pages.
        em.designate_payer(Some(crate::payer_account_gate::account_id()));
        for read in &reads {
            assert_served(&em, world, read, population).await;
        }
    }
}

/// RULE: a `NextToken` is not a capability. A page token the PAYER account was handed for
/// its own read does not get a non-payer caller the next page: the gate is ahead of the
/// paging, so the same request earns the 403 — and the token still works for the payer.
#[tokio::test]
async fn a_next_token_minted_for_the_payer_does_not_get_a_non_payer_the_next_page() {
    // (the modelled operation, which is also the case name, its route, and the output
    // member its page travels in; the token travels in `NextToken` for both)
    let paged: [(&str, &str, &str); 2] = [
        ("ListBillingGroups", "/list-billing-groups", "BillingGroups"),
        ("ListPricingRules", "/list-pricing-rules", "PricingRules"),
    ];

    let em = Emulator::payer();
    populate(&em, 3).await;

    for (operation, path, member) in paged {
        let case = operation;
        let request = json!({"MaxResults": 1});
        let (status, first, _) = em.post(path, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the payer account's first page, got {status} with {first}"
        );
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a NextToken over 3 rows, got {first}"))
            .to_string();
        let next = json!({"MaxResults": 1, "NextToken": token});

        em.designate_payer(None);
        let answered = em.post(path, &next).await;
        assert_denied(case, operation, &answered);
        let (_, denied_body, _) = &answered;
        assert!(
            denied_body.get(member).is_none(),
            "case {case:?}: the denial must not carry the {member} page, got {denied_body}"
        );

        // The same token, for the payer account, still names the next row — the denial
        // neither consumed nor invalidated it.
        em.designate_payer(Some(crate::payer_account_gate::account_id()));
        let (status, second, _) = em.post(path, &next).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the payer account's second page, got {status} with {second}"
        );
        assert_eq!(
            error_code(&second),
            "",
            "case {case:?}: expected a success body for the payer account, got {second}"
        );
        let page = second[member].as_array().cloned().unwrap_or_default();
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one row the token names, got {second}"
        );
        assert_ne!(
            page.first().and_then(|row| row["Arn"].as_str()),
            first[member]
                .as_array()
                .and_then(|rows| rows.first())
                .and_then(|row| row["Arn"].as_str()),
            "case {case:?}: the second page must not repeat the first row, got {second}"
        );
    }
}
