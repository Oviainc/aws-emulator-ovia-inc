Issue-Source: trace-sweep-divergence
Issue-Trace: rum-resource-policy-bad-revision-id
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-resource-policy-bad-revision-id rum-resource-policy-malformed-document rum-resource-policy-revision-conflict
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|PutResourcePolicy|4xx->4xx|$.message
Issue-Key: rum|DeleteResourcePolicy|4xx->4xx|$.message
Issue-Commit: 1c82a6d99cb1f7308f6a13b71a678a6984dedc7f

# RUM resource-policy revision-mismatch refusals share one invented message instead of each operation's AWS wording

> _Note: this issue was identified against the code base at commit `1c82a6d99cb1f7308f6a13b71a678a6984dedc7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
This issue bundles TWO distinct net-new root causes in the RUM resource-policy API's optimistic-locking refusals, found by wire replay. Both must be closed before submitting; each is numbered and carries its own failing fixture step. The fault they share: one home-grown sentence is emitted for both operations, where AWS deliberately words them differently.

Cause 1 of 2: PutResourcePolicy's stale-revision refusal carries the wrong message.
AWS: Smithy: PutResourcePolicy (PUT /appmonitor/{Name}/policy) takes an optional PolicyRevisionId (@length 1..255) and models InvalidPolicyRevisionIdException (client, httpError 400, message @required), documented as "The policy revision ID that you provided doeesn't match the latest policy revision ID." Recorded behavior: a PutResourcePolicy whose PolicyRevisionId is not the currently stored revision is refused with HTTP 400 InvalidPolicyRevisionIdException whose message is ENTIRELY fixed text — "If policyRevisionID is provided, then it must match the existing revision." — with NO variable parts, and the stored policy and revision are left untouched.
Emulator: the status (400) and the error code are right, but the message is "The policy revision ID <submitted revision id> does not match the latest policy revision ID." — different fixed text, and it interpolates the revision id from the request, a value AWS's message does not contain at all.
Why: the error code alone does not say which side was stale, so clients implementing compare-and-swap on the policy match this message (and operators read it in logs); the emulator's wording also leaks its own revision-id representation back to the caller.
Fix: the revision-mismatch branch of the PutResourcePolicy handler — it must emit its own fixed AWS sentence rather than a shared helper's parameterised one.
Traces: rum-resource-policy-bad-revision-id#task#3; the same cluster also covers rum-resource-policy-revision-conflict#task#3 (a stale revision after a successful second put) and rum-resource-policy-malformed-document#task#2.

Cause 2 of 2: DeleteResourcePolicy's stale-revision refusal carries the same shared message, where AWS words it differently from PutResourcePolicy.
AWS: Smithy: DeleteResourcePolicy (DELETE /appmonitor/{Name}/policy) takes PolicyRevisionId as an httpQuery member and models InvalidPolicyRevisionIdException (client, 400) alongside PolicyNotFoundException. Recorded behavior: a DeleteResourcePolicy carrying a stale PolicyRevisionId is refused with HTTP 400 InvalidPolicyRevisionIdException whose message is the fixed text "PolicyRevisionID must match the existing revision" — no trailing period, no variable parts, and deliberately NOT the sentence PutResourcePolicy uses (Cause 1). The policy and its revision are unchanged, and deleting with the current revision then succeeds.
Emulator: the identical generic sentence as Cause 1 — "The policy revision ID <submitted revision id> does not match the latest policy revision ID." — so the two operations are indistinguishable and neither matches AWS.
Why: same as Cause 1, plus a client that keys off the message to tell WHICH operation's optimistic-lock check failed (e.g. in a mixed put/delete reconcile) cannot. Because the single shared sentence is the root cause, fixing Cause 1 in a shared helper would silently change this operation too — each needs its own wording.
Fix: the revision-mismatch branch of the DeleteResourcePolicy handler must use its own AWS wording instead of sharing PutResourcePolicy's construction.
Traces: rum-resource-policy-bad-revision-id#task#4; the same cluster also covers rum-resource-policy-revision-conflict#task#4.

Note: the open BEHAVIORAL issue 20261029T192752Z-rum-rum-app-monitor-data-plane-resource-policy-tagging describes these same two wordings as its Causes 7 and 8, but it carries no wire replay key, so neither of these fixture steps is owned by it (the replay reported owned_by empty for both) and they are filed here with their bindings.

## Evidence
Wire replay of trace set simparity-wire-aws-v1: 239/279 steps passed, 40 mismatched. Both clusters are in service `rum` (bootstrapped as service-com-amazonaws-rum-rum) and neither reported a stub_path or an owned_by.

Cause 1 — PutResourcePolicy, expected status 400, got 400 (code InvalidPolicyRevisionIdException matched):
  rum-resource-policy-bad-revision-id#task#3: $.message: expected 'If policyRevisionID is provided, then it must match the existing revision.', got 'The policy revision ID <submitted revision id> does not match the latest policy revision ID.' (recorded instance interpolated the submitted value 'stale-nonexistent-revision-id-000'). affected_count 3: rum-resource-policy-bad-revision-id#task#3, rum-resource-policy-malformed-document#task#2, rum-resource-policy-revision-conflict#task#3.

Cause 2 — DeleteResourcePolicy, expected status 400, got 400:
  rum-resource-policy-bad-revision-id#task#4: $.message: expected 'PolicyRevisionID must match the existing revision' (no trailing period), got the identical 'The policy revision ID <submitted revision id> does not match the latest policy revision ID.' sentence as Cause 1. affected_count 2: rum-resource-policy-bad-revision-id#task#4, rum-resource-policy-revision-conflict#task#4.

The status and the error code matched in every step, so the divergence is confined to the message text: the fixed text differs and the emulator adds a variable part (the submitted revision id) that AWS's messages do not have.

## Steps to Reproduce
Replay fixture rum-resource-policy-bad-revision-id step task#3 (the representative) and rum-resource-policy-bad-revision-id#task#4 (in trace_ids).

Setup: CreateAppMonitor(Name=m, Domain=example.com); PutResourcePolicy(Name=m, PolicyDocument=<valid policy>) -> note the returned PolicyRevisionId rev1.

Cause 1: PutResourcePolicy(Name=m, PolicyDocument=<another valid policy>, PolicyRevisionId='stale-nonexistent-revision-id-000') -> observed 400 InvalidPolicyRevisionIdException with message 'The policy revision ID stale-nonexistent-revision-id-000 does not match the latest policy revision ID.'; expected 400 InvalidPolicyRevisionIdException with message exactly 'If policyRevisionID is provided, then it must match the existing revision.' GetResourcePolicy must still return the rev1 document. Also reproducible after a successful second put, passing rev1 as the stale id.

Cause 2: DeleteResourcePolicy(Name=m, PolicyRevisionId='stale-nonexistent-revision-id-000') -> observed the same sentence as Cause 1; expected 400 InvalidPolicyRevisionIdException with message exactly 'PolicyRevisionID must match the existing revision' (no trailing period). Deleting with the current revision must then succeed.
