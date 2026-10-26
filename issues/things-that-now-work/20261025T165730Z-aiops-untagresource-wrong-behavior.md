Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f3b5c182376fbd512bc0d8c0af2ad2661dfcff47

# UntagResource removes stored tags without advancing lastModifiedAt/lastModifiedBy

_Filed: 2026-10-25T16:57:30Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f3b5c182376fbd512bc0d8c0af2ad2661dfcff47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Same defect as the TagResource side, on the other tag-write handler. `UntagResource` resolves the stored `InvestigationGroup` through `group_mut` and removes keys from `group.tags`, which is a successful mutation of the stored record, yet it never calls `InvestigationGroup::touch` — the helper whose own documentation says a successful mutation ALWAYS advances `lastModifiedAt`, and which `UpdateInvestigationGroup`, `PutInvestigationGroupPolicy` and `DeleteInvestigationGroupPolicy` all call. After an untag that actually removed a key, `GetInvestigationGroup` still reports the stamp and principal of the previous write. (The crate already has the pattern for the "nothing was actually removed" case: `DeleteInvestigationGroupPolicy` only re-stamps when the take succeeded.)

## Evidence

Rule and helper — `services/service-com-amazonaws-aiops-aiops/src/state.rs:173-182`: "A successful mutation ALWAYS advances `lastModifiedAt`, which is what lets a caller notice a configuration change and order two writers by it." ... `pub fn touch(&mut self, principal: &str, at: i64)`.

Sibling that re-stamps only a removal that happened — `src/ops/delete_investigation_group_policy.rs:173-181`:
```
        if group.policy.take().is_none() {
            return Err(... investigation_group_policy_not_found() ...);
        }
        group.touch(&principal, modified_at);
```
The path that skips the rule — `src/ops/untag_resource.rs:178` and `:186-192`:
```
        let group = store.group_mut(&input.resource_arn, &region).ok_or_else(|| {
...
        // Removal is per key and idempotent: a key the resource never carried leaves
        // the stored set exactly as it was.
        for key in &input.tag_keys {
            group.tags.remove(key);
        }

        Ok(UntagResourceOutput {})
```
No `touch`, `caller_principal` or `now_epoch_seconds` appears in `untag_resource.rs` (grep for `touch` in the crate matches only state.rs:179, update_investigation_group.rs:245, put_investigation_group_policy.rs:199, delete_investigation_group_policy.rs:181).
Test coverage: `tests/all/investigation_group_singleton.rs:28` states the rule and `:1079-1111` exercises it for the three non-tagging mutations only; `tests/all/resource_tags.rs` asserts the stored tag set but never `lastModifiedAt`.

## Steps to Reproduce

1. CreateInvestigationGroup with `{"tags":{"Team":"ops"}}` and note the ARN
2. GET /investigationGroups/{arn} and record `lastModifiedAt` / `lastModifiedBy`
3. DELETE /tags/{arn}?tagKeys=Team — 200, and GET /tags/{arn} shows the key gone (the record WAS mutated)
4. GET /investigationGroups/{arn}: `lastModifiedAt` and `lastModifiedBy` are unchanged, contradicting src/state.rs:173
