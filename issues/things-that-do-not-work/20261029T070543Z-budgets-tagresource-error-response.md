Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 37c02434d582c8370b608107fbfd29504c12ec2b

# The three ARN-addressed tagging handlers skip crate::state::foreign_account_refusal, the gate its own doc calls shared by every operation handler

_Filed: 2026-10-29T07:05:43Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `37c02434d582c8370b608107fbfd29504c12ec2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has exactly one account-scoping gate, `crate::state::foreign_account_refusal`, and its doc comment states it is applied by every operation handler of the service. 23 of the 26 handlers call it as their first act and answer the modelled `AccessDeniedException` (HTTP 403) for a request that names an account other than the one this emulator answers for.

The three tagging handlers — `TagResource`, `UntagResource` and `ListTagsForResource` — never call it, even though the account is right there in the request: `crate::state::parse_resource_arn` explicitly reads the twelve-digit account field out of the `ResourceARN` and hands it back on both `TaggedResource::Budget` and `TaggedResource::Action`, and `resource_tags` / `resource_tags_mut` then look the resource up under the `(account_id, budget_name)` key built from it.

Result: a `ResourceARN` whose account field is NOT the caller's account is not refused by the gate. It is resolved against the store under that foreign key, finds nothing (no write path can ever create a budget under a foreign account, precisely because the gate blocks the creates), and is answered `NotFoundException` (HTTP 404) — "the budget the ResourceARN names doesn't exist" — instead of the `AccessDeniedException` (HTTP 403) every one of these three operations models and that the same crate gives for the same cross-account attempt expressed as an `AccountId`. The caller is told the resource is absent when the correct answer is that it is not theirs to reach, and the one gate the state module says cannot drift between handlers has drifted in three of them.

## Evidence

The crate's own rule, state.rs:1680-1696:
```
/// The refusal message a request whose `AccountId` is not the caller's own account
/// takes, or `None` when it names the caller's account.
...
/// Shared by every operation handler, so the one gate cannot drift between them.
pub fn foreign_account_refusal(account_id: &str) -> Option<String> {
    (account_id != ::engine::emulator_account_id())
        .then(|| ACCESS_DENIED_MESSAGE.to_string())
}
```

A sibling handler applying it — services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/delete_budget.rs:146-156:
```
// The budgets control plane is scoped to the account the caller signed with, so
// an `AccountId` naming any other account is refused with the modelled 403
// before anything is read or written (`crate::state::foreign_account_refusal`).
if let Some(message) = crate::state::foreign_account_refusal(&input.account_id) {
    return Err(DeleteBudgetErrorModeled::AccessDeniedException(...));
}
```
(`grep foreign_account_refusal src/ops` matches 23 of the 26 handler files; the only three with no match are tag_resource.rs, untag_resource.rs and list_tags_for_resource.rs.)

The three that skip it — services/.../src/ops/tag_resource.rs:149-172 (handler body opens straight on `service_state` then `parse_resource_arn`, no gate):
```
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| { ... })?;
        ...
        let target = crate::state::parse_resource_arn(&input.resource_arn).map_err(...)?;
```
Same shape at services/.../src/ops/untag_resource.rs:134-158 and services/.../src/ops/list_tags_for_resource.rs:146-166.

The account IS available to them — state.rs:2412-2434:
```
    if account_id.len() != 12 || !account_id.chars().all(|digit| digit.is_ascii_digit()) {
        return Err(malformed());
    }
...
            Ok(TaggedResource::Budget {
                account_id: account_id.to_string(),
                budget_name: resource.to_string(),
            })
```
and it is used only as a store key — state.rs:2479-2482:
```
        } => state
            .budgets
            .get_mut(&(account_id.clone(), budget_name.clone()))
            .map(|stored| &mut stored.tags),
```
with the 404 the handler then produces at tag_resource.rs:188-202 ("Unable to tag resource: the budget the ResourceARN names doesn't exist.").

The modelled error these three declare and never construct — OPERATIONS.md:153/159/165: `ListTagsForResource` / `TagResource` / `UntagResource` each list `AccessDeniedException (HTTP 403)`, whose model doc is "You are not authorized to use this operation with the given parameters." (quoted in state.rs:1685-1686 and in ACCESS_DENIED_MESSAGE, state.rs:1702-1703).

Test coverage: tests/all/account_scoping.rs is the suite for this rule — its header states "modelled `AccessDeniedException` (HTTP 403) before it reads or writes anything" — and its table `every_operation` (account_scoping.rs:170-297) enumerates "Every IMPLEMENTED operation of this service" but contains no row for TagResource, UntagResource or ListTagsForResource, so nothing pins their behaviour for a foreign-account ARN. `grep -r 'AccessDenied|foreign' tests/` returns no tagging-path case either.

## Steps to Reproduce

1. Create a budget under the caller's own account: POST / with X-Amz-Target: AWSBudgetServiceGateway.CreateBudget and {"AccountId":"<caller>","Budget":{"BudgetName":"b1","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} — 200.
2. Send the same cross-account attempt through an AccountId-bearing operation: DescribeBudget with {"AccountId":"000000000000","BudgetName":"b1"} — answered HTTP 403 AccessDeniedException by the shared gate (delete_budget.rs:149 and 22 siblings).
3. Now send it through the tagging plane: TagResource with {"ResourceARN":"arn:aws:budgets::000000000000:budget/b1","ResourceTags":[{"Key":"k","Value":"v"}]} — answered HTTP 404 NotFoundException ("the budget the ResourceARN names doesn't exist") instead of HTTP 403 AccessDeniedException; the gate was never consulted.
4. Repeat with UntagResource {"ResourceARN":"arn:aws:budgets::000000000000:budget/b1","ResourceTagKeys":["k"]} and ListTagsForResource {"ResourceARN":"arn:aws:budgets::000000000000:budget/b1"} — both answer 404 for an account that is not the caller's.
5. Confirm the inconsistency in code: grep -n foreign_account_refusal services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops — 23 handler files match, tag_resource.rs / untag_resource.rs / list_tags_for_resource.rs do not.
