Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f3b5c182376fbd512bc0d8c0af2ad2661dfcff47

# TagResource mutates the stored group without advancing lastModifiedAt/lastModifiedBy

_Filed: 2026-10-25T16:57:30Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f3b5c182376fbd512bc0d8c0af2ad2661dfcff47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate single-homes the "a write re-stamps the record" rule in `InvestigationGroup::touch`, whose own documentation states the rule unconditionally, and every other handler that mutates a stored group calls it: `UpdateInvestigationGroup`, `PutInvestigationGroupPolicy` and `DeleteInvestigationGroupPolicy`. `TagResource` mutates the very same `InvestigationGroup` record (it inserts into `group.tags` through `group_mut`) and never calls `touch`, so a successful tag write leaves `lastModifiedAt`/`lastModifiedBy` reporting the previous writer and instant. A caller that uses `lastModifiedAt` to notice a change — the use the comment names — cannot see that the group was modified, and `lastModifiedBy` keeps naming the principal of an earlier, unrelated call. Note the crate already applies the stamp to a write (`PutInvestigationGroupPolicy`) whose member `GetInvestigationGroup` does not even report, so the tag write is the one record mutation that is exempt from the rule the crate states.

## Evidence

The rule, stated by the crate itself — `services/service-com-amazonaws-aiops-aiops/src/state.rs:171-182`:
```
    /// Record that `principal` just changed this group.
    ///
    /// A successful mutation ALWAYS advances `lastModifiedAt`, which is what lets a
    /// caller notice a configuration change and order two writers by it.
    pub fn touch(&mut self, principal: &str, at: i64) {
        self.last_modified_by = principal.to_string();
        self.last_modified_at = at.max(self.last_modified_at.saturating_add(1));
    }
```
Siblings that apply it: `src/ops/update_investigation_group.rs:245` `group.touch(&principal, modified_at);`, `src/ops/put_investigation_group_policy.rs:199` `group.touch(&principal, modified_at);`, `src/ops/delete_investigation_group_policy.rs:181` `group.touch(&principal, modified_at);`.

The path that skips it — `src/ops/tag_resource.rs:183` resolves the same mutable record and `:202-204` writes it with no `touch` and no `caller_principal`/`now_epoch_seconds` call anywhere in the handler:
```
        let group = store.group_mut(&input.resource_arn, &region).ok_or_else(|| {
...
        for (key, value) in input.tags {
            group.tags.insert(key, value);
        }

        Ok(TagResourceOutput {})
```
Test coverage: the rule is also stated in `tests/all/investigation_group_singleton.rs:28` — "6. A successful mutation advances `lastModifiedAt`; a read never changes it." — but the mutation table at `tests/all/investigation_group_singleton.rs:1079-1111` enumerates only `UpdateInvestigationGroup`, `PutInvestigationGroupPolicy` and `DeleteInvestigationGroupPolicy`, so no test exercises a tag write against the stamp; `tests/all/resource_tags.rs` never reads `lastModifiedAt` (no match for `lastModified` in that file).

## Steps to Reproduce

1. CreateInvestigationGroup in a Region and note the ARN
2. GET /investigationGroups/{arn} and record `lastModifiedAt` and `lastModifiedBy`
3. POST /tags/{arn} with `{"tags":{"Team":"ops"}}` — 200, and GET /tags/{arn} confirms the tag was stored (the record WAS mutated)
4. GET /investigationGroups/{arn} again: `lastModifiedAt` and `lastModifiedBy` are unchanged, although the crate's own `touch` doc (src/state.rs:173) says "A successful mutation ALWAYS advances `lastModifiedAt`"
5. For contrast, POST /investigationGroups/{arn}/policy and repeat the GET: that mutation does advance the stamp (put_investigation_group_policy.rs:199)
