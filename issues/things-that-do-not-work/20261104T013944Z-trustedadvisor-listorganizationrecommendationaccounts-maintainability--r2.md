Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 44d77109d3457b765fae885a8f4ecd8776c4625f

# The affectedAccountId ownership test is implemented twice in this crate — once through arn_account_id, once through the lower-level arn_account

_Filed: 2026-11-04T01:39:44Z_
_Operation: ListOrganizationRecommendationAccounts | Class: maintainability_

> _Note: this issue was identified against the code base at commit `44d77109d3457b765fae885a8f4ecd8776c4625f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one derivation for "the account that owns a stored row": `crate::state::arn_account_id`, whose own doc calls it the derivation EVERY view of a stored resource's owning account goes through, so the account-level and the organization-level view of one row cannot disagree. `arn_account` is the raw segment reader underneath it, which keeps an EMPTY account segment (the form a check ARN carries) rather than treating it as "no account".

The two organization-level lists apply the SAME modelled filter — `affectedAccountId` (`AccountId`, `length 12..12`, `pattern ^\d+$`) keeps only the rows owned by that account — but they implement that one rule two different ways:

* `ListOrganizationRecommendationResources` compares through the crate's own derivation, `arn_account_id(..).as_deref() == Some(account)`;
* `ListOrganizationRecommendationAccounts` compares through the lower-level `arn_account(member_arn) == Some(account)`, bypassing the derivation the crate declares as the single one and admitting the empty-account case that `arn_account_id` exists to reject.

The two copies agree today only because the modelled `pattern ^\d+$` plus `length 12..12` means a request can never name the empty account, and because every ARN in `recommendations` is built by `state::recommendation_arn` with a 12-digit segment — i.e. the agreement is a coincidence of two unrelated constraints, not of one shared rule, so the copies can drift. Every response and every existing test stay byte-identical if the accounts handler routes its comparison through `arn_account_id`, exactly as its sibling and as the `accountId` it reports beside each row already do.

## Evidence

The crate's declared single derivation — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/state.rs:2219-2232:
```
/// The owning account a modelled `accountId` member reports for a resource,
/// derived from that resource's own ARN: its account segment, owned, and `None`
/// when the ARN names no account at all — it carries fewer segments than an ARN,
/// or its account segment is EMPTY, which (see [`arn_account`]) is what the
/// segment is for a resource type whose ARNs carry no account.
///
/// Every view that reports a stored resource's owning account goes through this,
/// so the account-level and the organization-level view of one stored resource
/// cannot report its account differently.
pub fn arn_account_id(arn: &str) -> Option<String> {
```
and the raw reader it wraps — state.rs:2210-2217:
```
/// The account segment of an AWS resource ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for a
/// string with fewer segments than that. The segment is EMPTY for a resource type
/// whose ARNs carry no account (an S3 bucket, a Trusted Advisor check), which is
/// not another account's resource.
pub fn arn_account(arn: &str) -> Option<&str> {
```

Copy 1 — the sibling list, through the declared derivation: src/ops/list_organization_recommendation_resources.rs:255-260:
```
            .filter(|(_, resource)| {
                filters.keeps(resource)
                    && input.affected_account_id.as_deref().is_none_or(|account| {
                        crate::state::arn_account_id(&resource.arn).as_deref() == Some(account)
                    })
            })
```

Copy 2 — the same rule, through the raw reader instead: src/ops/list_organization_recommendation_accounts.rs:254-259:
```
            .filter(|member_arn| {
                input
                    .affected_account_id
                    .as_deref()
                    .is_none_or(|account| crate::state::arn_account(member_arn) == Some(account))
            })
```
while the very same handler REPORTS the account through the declared derivation — src/ops/list_organization_recommendation_accounts.rs:118-119:
```
    AccountRecommendationLifecycleSummary {
        account_id: crate::state::arn_account_id(&entry.arn),
```

The suite that pins the one derivation for both organization views — tests/all/owning_account_from_arn.rs:3-18:
```
//! End-to-end cover for the ONE derivation both organization-level views of a
//! stored resource report their `accountId` from
//! (`service_com_amazonaws_trustedadvisor_trustedadvisor::state::arn_account_id`),
...
//! Both organization-level lists derive it the same way, so the
//! account-level and the organization-level view of one stored resource can
//! never report different owners
```

Test coverage of both filters, which a fix must leave unchanged: tests/all/organization_recommendation_accounts.rs:301-341 (rows "no affectedAccountId", "affectedAccountId of the calling account", "affectedAccountId that owns no member") and :694-726 (the modelled `length 12..12` / `pattern ^\d+$` refusals), tests/all/organization_recommendation_resources.rs (the same filter on the resources list), and tests/all/owning_account_from_arn.rs:349-379 (`the_shared_derivation_reports_no_account_for_an_arn_that_carries_none`).

## Steps to Reproduce

1. Open src/ops/list_organization_recommendation_resources.rs:255-260 and read how the modelled `affectedAccountId` filter decides which rows to keep: `crate::state::arn_account_id(&resource.arn).as_deref() == Some(account)`.
2. Open src/ops/list_organization_recommendation_accounts.rs:254-259 and read how the SAME modelled filter decides it in the sibling handler: `crate::state::arn_account(member_arn) == Some(account)` — the raw segment reader, not the crate's declared derivation.
3. Read src/state.rs:2219-2227, which states that the declared derivation is the one every view of a stored row's owning account goes through, and src/state.rs:2210-2214, which records that the raw reader keeps an EMPTY account segment the declared derivation rejects.
4. Note that the same accounts handler already reports the row's account through the declared derivation (list_organization_recommendation_accounts.rs:119), so one handler uses both forms for one rule.
5. Confirm the two copies are behaviourally interchangeable today — `affectedAccountId` is modelled `length 12..12, pattern ^\d+$` (tests/all/organization_recommendation_accounts.rs:694-726 pins those refusals) and every key of `state.recommendations` is built by `state::recommendation_arn` with a 12-digit segment — so routing the accounts filter through `arn_account_id` leaves every response and every existing test unchanged.
