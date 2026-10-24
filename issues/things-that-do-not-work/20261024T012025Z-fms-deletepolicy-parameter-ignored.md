Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# DeletePolicy ignores DeleteAllPolicyResources

_Filed: 2026-10-24T01:20:24Z_
_Operation: DeletePolicy | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeletePolicy's `DeleteAllPolicyResources` member selects what happens to the protections and resources Firewall Manager created on the policy's behalf: when true the service deletes all policy resources it manages (for example the policy's web ACLs / Shield protections and the managed objects in the member accounts), when false (the declared default) it leaves them in place and only removes the policy. The handler never reads the member — it is deserialized and dropped, and the policy record plus its tag-index entry are removed identically for `true`, `false` and an absent value, with no record of the choice kept anywhere in state.

So nothing a later call can observe depends on the flag. Expected: the flag's value drives the cleanup of the policy-managed resources (and, at minimum, is recorded/acted on rather than discarded, so a true-delete and a false-delete are distinguishable). Actual: the parameter is silently ignored.

## Evidence

Model: DeletePolicy input is `PolicyId` (required) and `DeleteAllPolicyResources` (boolean, default false) — a documented input member whose whole purpose is to select between two different outcomes. Handler: `src/ops/delete_policy.rs` handle (lines 177-201) uses only `input.policy_id` (`store.policies.remove(&input.policy_id)` then `store.tags_by_arn.remove(...)`); `input.delete_all_policy_resources` is never referenced, and no field of `src/state.rs` records it. Test coverage: `tests/all/policy_lifecycle.rs::delete_policy_removes_the_policy_from_state` (lines 596-658) loops over None/false/true and asserts only that the policy is gone in all three cases — i.e. it pins the current indifference rather than asserting any effect of the flag.

## Steps to Reproduce

1. PutPolicy a policy and note its PolicyId.
2. POST DeletePolicy {"PolicyId": <id>, "DeleteAllPolicyResources": false}. Observed: 200 and the policy plus every resource/tag association it owned is dropped wholesale, exactly as for `true`.
3. Repeat the create and delete with "DeleteAllPolicyResources": true: the emulator's state after the two deletes is byte-for-byte the same, so no client can tell which mode it asked for.
