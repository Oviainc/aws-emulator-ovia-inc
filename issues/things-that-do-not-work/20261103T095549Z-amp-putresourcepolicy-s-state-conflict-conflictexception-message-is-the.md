Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amp.resource-policy-concurrent-update-optimistic-lock
Issue-Shape-Id: com.amazonaws.amp#AmazonPrometheusService
Issue-Key: amp.resource-policy-concurrent-update-optimistic-lock|run#20|AssertionError|assert messages[i] == expected_message, messages[i]
Issue-Commit: 36b2ae1e2f8109374e2b22f19334f28f66fd960d

# PutResourcePolicy's state-conflict ConflictException message is the emulator's own prose instead of AWS's fixed "Can't update the workspace resource policy in non-(ACTIVE) state" sentence

> _Note: this issue was identified against the code base at commit `36b2ae1e2f8109374e2b22f19334f28f66fd960d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: the ConflictException AMP raises when a PutResourcePolicy arrives while the workspace's resource-based policy is not ACTIVE carries a message the emulator wrote itself, not the message AWS returns for that error class.

AWS: Smithy (services/service-com-amazonaws-amp-amazonprometheusservice/smithy-model/com-amazonaws-amp-amazonprometheusservice.json) models PutResourcePolicy -> ConflictException (HTTP 409, members message/resourceId/resourceType all required), and WorkspacePolicyStatusCode documents the rule the refusal enforces: "Update/Deletion is disallowed until Resource-based Policy is ACTIVE and Workspace is ACTIVE" for each of CREATING | ACTIVE | UPDATING | DELETING. The model does not fix the message text, so the message is recorded behavior only: for a put that loses the race against an in-flight policy update, AWS answers HTTP 409 ConflictException with message "Can't update the workspace resource policy in non-(ACTIVE) state. Current status is UPDATING". In that sentence the fixed text is "Can't update the workspace resource policy in non-(ACTIVE) state. Current status is " — including the literal parenthesised "(ACTIVE)" — and the only variable part is the trailing status token, which is the blocking WorkspacePolicyStatusCode value taken from the emulator's state at the time of the refusal (UPDATING in the recording, but CREATING or DELETING on the corresponding paths). No workspace id, policy status pair, or further clause appears.

Emulator: ops/put_resource_policy.rs answers the right error class and the right status (the check's assertions on exactly one winner, four losers and code == ConflictException all pass), but formats its own sentence: "The workspace is ACTIVE and its resource-based policy is UPDATING; a put is allowed only while the workspace is ACTIVE and any policy it holds is ACTIVE". It interpolates two state values (the workspace status and the policy status, or the literal "not attached" when no policy exists) into wording AWS never emits, and omits AWS's fixed "Can't update the workspace resource policy in non-(ACTIVE) state. Current status is <status>" text entirely.

Why: a client that classifies or asserts on the refusal text — SDK-level retry wrappers, operator runbooks, and tests that match AWS's documented conflict sentence — sees an unrecognised message for a conflict AWS words one fixed way. Code and status match, so the divergence is invisible until the text is read, and then it reads as a different failure than the one AWS reports.

Fix: the ConflictException construction in the PutResourcePolicy handler (the refusal built behind the crate::state::policy_mutation_allowed gate) must emit AWS's fixed sentence with the blocking policy/workspace status as its only interpolated value; the same wording rule applies to the sibling policy-mutation refusals that share this gate (DeleteResourcePolicy) so the whole surface answers one text.

Traces: behavioral check amp.resource-policy-concurrent-update-optimistic-lock (five racing puts presenting the same base revisionId; the four losers' Error.Message is compared to AWS's sentence).

## Evidence
Check amp.resource-policy-concurrent-update-optimistic-lock, line 70 of check_amp_resource_policy_concurrent_update_optimistic_lock.py:

  assert messages[i] == expected_message, messages[i]
  AssertionError: The workspace is ACTIVE and its resource-based policy is UPDATING; a put is allowed only while the workspace is ACTIVE and any policy it holds is ACTIVE

Expected (AWS, HTTP 409, Error.Code ConflictException):
  "Can't update the workspace resource policy in non-(ACTIVE) state. Current status is UPDATING"
  — fixed text: "Can't update the workspace resource policy in non-(ACTIVE) state. Current status is "; variable part: the blocking WorkspacePolicyStatusCode from emulator state (UPDATING here).

Actual (emulator, HTTP 409, Error.Code ConflictException):
  "The workspace is <workspaceStatus> and its resource-based policy is <policyStatus>; a put is allowed only while the workspace is ACTIVE and any policy it holds is ACTIVE"
  (built in services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/put_resource_policy.rs, the format! at the policy_mutation_allowed gate, lines ~241-260).

Error code, HTTP status, winner/loser counts and the surviving revision all match AWS; only the message text diverges.

## Steps to Reproduce
1. Create an AMP workspace and poll DescribeWorkspace until status.statusCode == ACTIVE.
2. PutResourcePolicy with a valid policy document; poll DescribeResourcePolicy until policyStatus == ACTIVE; keep the returned revisionId as the base revision.
3. Concurrently issue 5 PutResourcePolicy calls, all passing revisionId=<base revision> with different policy documents.
4. Exactly one call succeeds; capture Error.Message from each of the 4 ConflictException losers.
5. Observed: "The workspace is ACTIVE and its resource-based policy is UPDATING; a put is allowed only while the workspace is ACTIVE and any policy it holds is ACTIVE". Expected: "Can't update the workspace resource policy in non-(ACTIVE) state. Current status is UPDATING".
