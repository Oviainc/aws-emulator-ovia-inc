Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# The "accountId from an ARN, empty segment means absent" rule is written out twice in this crate

_Filed: 2026-10-26T08:21:02Z_
_Operation: ListOrganizationRecommendationAccounts | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two handlers of this crate derive the modelled `accountId` member from an ARN with the identical three-step computation — take the ARN's account segment via `crate::state::arn_account`, drop it when the segment is EMPTY, own the result. The state module already owns the rule that an empty account segment is not an account (`arn_account`'s doc and `foreign_resource_refusal`'s guard), but the Option-shaped form of it lives in the two handlers rather than beside `arn_account`, so the copies can drift: a change to how an accountless ARN is reported has to be made in both places or the account-level and organization-level views of the same stored resource start disagreeing.

The fix is a single helper next to `arn_account` in `crate::state` (the module that already defines the rule) that both call sites use. Every response byte and every existing test stays unchanged — the helper computes exactly what the two copies compute today.

## Evidence

Copy 1 — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_organization_recommendation_accounts.rs:117-119:
```
        account_id: crate::state::arn_account(&entry.arn)
            .filter(|account| !account.is_empty())
            .map(str::to_string),
```

Copy 2 — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_organization_recommendation_resources.rs:136-138:
```
        account_id: crate::state::arn_account(&resource.arn)
            .filter(|account| !account.is_empty())
            .map(str::to_string),
```

The rule the two copies re-implement is already stated in the state module — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/state.rs:1632-1638 and 1650-1652:
```
/// The account segment of an AWS resource ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for a
/// string with fewer segments than that. The segment is EMPTY for a resource type
/// whose ARNs carry no account (an S3 bucket, a Trusted Advisor check), which is
/// not another account's resource.
pub fn arn_account(arn: &str) -> Option<&str> {
    arn.split(':').nth(4)
}
...
pub fn foreign_resource_refusal(aws_resource_arn: &str) -> Option<AccessDeniedException> {
    let account = arn_account(aws_resource_arn)?;
    (!account.is_empty() && account != ::engine::emulator_account_id()).then(|| {
```

Tests that already pin both copies' output, so the consolidation is verifiable without new tests: `tests/all/organization_recommendation_accounts.rs` (asserts each summary's `accountId`, e.g. `summary["accountId"].as_str() == Some(account_id)` at line 190) and `tests/all/organization_recommendation_resources.rs` (the per-resource `accountId` rows).

## Steps to Reproduce

1. Open src/ops/list_organization_recommendation_accounts.rs:117-119 and src/ops/list_organization_recommendation_resources.rs:136-138 and compare the two `account_id` expressions: they are the same computation on different ARNs.
2. Note that `crate::state::arn_account` (state.rs:1637) and `crate::state::foreign_resource_refusal` (state.rs:1650) already own the "an empty account segment is not an account" rule, so the Option-shaped derivation has no single home.
3. Change the rule in one handler only (e.g. stop dropping the empty segment) and observe that the account-level and organization-level views of the same stored resource now report `accountId` differently, with nothing in the crate forcing the two to agree.
