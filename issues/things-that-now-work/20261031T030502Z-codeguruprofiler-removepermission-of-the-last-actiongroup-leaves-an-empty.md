Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.remove-permission-clears-policy codeguruprofiler.remove-permission-deletes-policy codeguruprofiler.remove-permission-stale-revision-conflict-rejected codeguruprofiler.remove-permission-stale-revision-id-rejected
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler.remove-permission-clears-policy|run#12|AssertionError|assert "policy" not in after, after
Issue-Key: codeguruprofiler.remove-permission-deletes-policy|run#17|AssertionError|assert "policy" not in after, after
Issue-Key: codeguruprofiler.remove-permission-stale-revision-conflict-rejected|run#19|AssertionError|assert "policy" not in after, after
Issue-Key: codeguruprofiler.remove-permission-stale-revision-id-rejected|run#21|AssertionError|assert "policy" not in after, after
Issue-Commit: e0eecc5f72a782e2fd5c24b79b4afd214154b2e2

# RemovePermission of the last actionGroup leaves an empty-Statement policy on the group, so GetPolicy still reports policy/revisionId instead of the policy-less state

> _Note: this issue was identified against the code base at commit `e0eecc5f72a782e2fd5c24b79b4afd214154b2e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RemovePermission

## Description
Cause 1 of 1: removing the only/last actionGroup statement does not delete the profiling group's resource-based policy; it keeps a policy object holding an empty Statement list, so every subsequent read and write of that policy diverges from AWS.

Failing checks for this cause: codeguruprofiler.remove-permission-clears-policy, codeguruprofiler.remove-permission-deletes-policy, codeguruprofiler.remove-permission-stale-revision-conflict-rejected, codeguruprofiler.remove-permission-stale-revision-id-rejected.

AWS: RemovePermission on `agentPermissions` (the only supported action group — Smithy: RemovePermission.actionGroup is enum ActionGroup with the single value `agentPermissions`, @httpLabel, required) removes the last statement and therefore DELETES the whole resource-based policy of the profiling group. A following GetPolicy (Smithy: GET /profilingGroups/{profilingGroupName}/policy, 200, output members `policy`/`revisionId`) still succeeds with HTTP 200 because the group exists, but the response body carries NEITHER `policy` NOR `revisionId` — the "policy-less" state. (The model marks both output members @required, yet the recorded behavior these checks were taken from shows them absent once no policy exists; recorded behavior is the authority here.) Because the policy is gone, a further policy-scoped mutation naming the now-consumed revisionId is refused with ResourceNotFoundException (HTTP 404) and the message `No policy attached to profiling group name: <profilingGroupName> in accountId: <accountId>; ` — fixed text is `No policy attached to profiling group name: `, ` in accountId: ` and the trailing `; `; the variable parts are the group name from the request and the account the lookup is scoped to. Not the "grants no <actionGroup>" wording, which describes a policy that exists but lacks that statement.

Emulator: the removal is applied but the policy object survives with zero statements. `GetPolicy` then answers HTTP 200 with a populated body: `policy` = "{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ ]\n}" and a `revisionId` (a fresh uuid minted by the removal). All four checks fail on `assert "policy" not in after` against exactly that response. The crate states this as intent: src/ops/remove_permission.rs:5-10 and :191-200 ("the policy goes on existing holding no statement at all, which is what GetPolicy then reports"), and src/state.rs:1823-1831 (`policy.revoke(action_group); policy.set_revision(..)` then returns the rendered document, never clearing the group's `resource_policy`). src/ops/get_policy.rs:178-189 already renders the policy-less state correctly — it is simply never reached, because the state never drops the policy. As a knock-on, a second RemovePermission against the consumed revisionId lands in the `PolicyWrite::NoStatement` arm (src/ops/remove_permission.rs:229-239) and answers the emulator-invented message `the resource-based policy of profiling group <name> grants no <actionGroup>` instead of AWS's `No policy attached to profiling group name: <name> in accountId: <accountId>; ` (which `crate::state::policy_not_found_message`, src/state.rs:2040-2045, already produces for the `NoPolicy` arm).

Why: a client that uses GetPolicy to decide whether a profiling group has a resource policy at all — the documented way to check before PutPermission, and the way infrastructure tooling detects drift — concludes a policy is still attached after it removed the last permission. It then supplies the stale revisionId to PutPermission (AWS would reject naming a revision when no policy exists) or reports the group as still policy-bearing. Teardown/idempotency logic that retries the removal gets a 404 whose message describes a different state than the one it is in, so error-message-driven branching takes the wrong branch.

Fix: the profiling-group state's policy removal — `crate::state` RemovePermission path (src/state.rs around :1800-1832): when the revoke leaves the policy with no statements, clear the group's `resource_policy` entirely so GetPolicy's existing policy-less branch (src/ops/get_policy.rs:186-189) is reached and so the next revision-checked write resolves to `PolicyWrite::NoPolicy` (yielding `policy_not_found_message`). The RemovePermission response itself still reports the post-removal document and the revision the removal minted, which the checks accept. Also correct the module/handler comments in src/ops/remove_permission.rs:5-10 and :191-200 that assert the opposite rule.

Traces: codeguruprofiler.remove-permission-clears-policy (asserts GetPolicy carries no policy/revisionId, then that a repeat removal with the consumed revisionId is ResourceNotFoundException with the `No policy attached ...` message), codeguruprofiler.remove-permission-deletes-policy, codeguruprofiler.remove-permission-stale-revision-conflict-rejected and codeguruprofiler.remove-permission-stale-revision-id-rejected (both pass their ConflictException code and `Policy has been updated since the revision: <requested>, new RevisionId: <current>` message assertions, then fail on the same final policy-less assertion).

## Evidence
Expected (recorded AWS), after `remove_permission(profilingGroupName=<name>, actionGroup="agentPermissions", revisionId=<current>)` removed the only statement:
  get_policy(profilingGroupName=<name>) -> HTTP 200, body with NO `policy` member and NO `revisionId` member.
  remove_permission(... revisionId=<the consumed revision>) -> HTTP 404 ResourceNotFoundException,
    Message: "No policy attached to profiling group name: <name> in accountId: <accountId>; "

Actual (emulator), from the replay of all four checks:
  get_policy(...) -> HTTP 200 with
    'policy': '{\n  "Version" : "2012-10-17",\n  "Statement" : [ ]\n}',
    'revisionId': '<uuid minted by the removal>'
  AssertionError raised at `assert "policy" not in after`:
    codeguruprofiler.remove-permission-clears-policy — check_..._clears_policy.py line 51
    codeguruprofiler.remove-permission-deletes-policy — check_..._deletes_policy.py line 59
    codeguruprofiler.remove-permission-stale-revision-conflict-rejected — line 76
    codeguruprofiler.remove-permission-stale-revision-id-rejected — line 78
  (The uuid and the group name differ per run and are not part of the requirement; the
   presence of the two members, and the empty-Statement document, are.)

Source of the divergence:
  src/state.rs:1823-1831 — revoke + set_revision + render; the group's resource_policy is never cleared when the last statement goes.
  src/ops/remove_permission.rs:191-200 — "also when no statement is left, since the policy itself still exists".
  src/ops/get_policy.rs:186-189 — the correct policy-less branch, unreachable today.
  src/state.rs:2040-2045 — `policy_not_found_message` already words AWS's 404 exactly as the check expects.

## Steps to Reproduce
1. create_profiling_group(profilingGroupName=<name>).
2. put_permission(profilingGroupName=<name>, actionGroup="agentPermissions", principals=["arn:aws:iam::<account>:root"]) and keep the returned revisionId.
3. get_policy(profilingGroupName=<name>) — confirms the policy is readable at that revision.
4. remove_permission(profilingGroupName=<name>, actionGroup="agentPermissions", revisionId=<that revisionId>) — removes the only statement.
5. get_policy(profilingGroupName=<name>) — emulator returns 200 with `policy` = an empty-Statement document and a `revisionId`; AWS returns 200 with neither member present.
6. remove_permission(profilingGroupName=<name>, actionGroup="agentPermissions", revisionId=<the revisionId from step 4's response or step 2>) — emulator answers 404 with "the resource-based policy of profiling group <name> grants no agentPermissions"; AWS answers 404 with "No policy attached to profiling group name: <name> in accountId: <account>; ".
7. Replay: codeguruprofiler.remove-permission-clears-policy, codeguruprofiler.remove-permission-deletes-policy, codeguruprofiler.remove-permission-stale-revision-conflict-rejected, codeguruprofiler.remove-permission-stale-revision-id-rejected.
