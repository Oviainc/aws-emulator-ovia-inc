Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-automated-reasoning-policy-build-workflow
Issue-Step: teardown#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-automated-reasoning-policy-build-workflow bedrock-automated-reasoning-policy-lifecycle
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|DeleteAutomatedReasoningPolicy|2xx->4xx|status
Issue-Key: bedrock|DeleteAutomatedReasoningPolicy|4xx->2xx|status
Issue-Commit: a4ef059b31cd49fc5529baf331c68065b4514ad5

# DeleteAutomatedReasoningPolicy delete-eligibility and idempotency gates diverge from AWS

> _Note: this issue was identified against the code base at commit `a4ef059b31cd49fc5529baf331c68065b4514ad5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAutomatedReasoningPolicy

## Description
Three net-new causes, all in the emulator's DeleteAutomatedReasoningPolicy handler. The shared fault is that the two gates this operation runs before it removes a revision — the unforced-delete artifact check and the not-found check — are scoped differently from AWS's, so the emulator refuses deletes AWS accepts, accepts a delete AWS refuses, and reports a policy as missing where AWS still answers success. A fixer must close all THREE before submitting.

Cause 1 of 3: an unforced delete of a policy that holds only a BUILD WORKFLOW is refused instead of succeeding.
AWS: Smithy com.amazonaws.bedrock#DeleteAutomatedReasoningPolicy, http {method: DELETE, uri: /automated-reasoning-policies/{policyArn}, code: 202}; input member `force` (Boolean, default false) is documented as "When false, Amazon Bedrock validates if all artifacts have been deleted (e.g. policy version, test case, test result) for a policy before deletion." The documented artifact list names policy versions, test cases and test results — it does NOT name build workflows. The recording confirms it: after an INGEST_CONTENT build workflow has been started against the policy, an unforced delete of that policy answers 202 with an empty body.
Emulator: answers 400 ResourceInUseException. services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/delete_automated_reasoning_policy.rs `held_artifacts` counts `automated_reasoning_policy_build_workflows` as a blocking artifact alongside versions/test cases/test results, so the unforced branch returns `in_use("automated reasoning policy <policyArn> still has build workflows that have not been deleted; delete them or retry the request with force set to true")`. That sentence is the emulator's own wording for a refusal AWS does not issue at all here.
Why: a client that creates a policy, runs a build workflow and then tears the policy down in the documented order (no force) gets a hard 400 ResourceInUseException and leaks the policy. On AWS the teardown succeeds, so every such cleanup path fails only against the emulator.
Fix: the unforced-delete eligibility check in that operation's handler — `held_artifacts` must list only the artifact kinds AWS's `force` documentation names (policy versions, test cases, test results) and must not treat a build workflow as blocking.
Traces: bedrock-automated-reasoning-policy-build-workflow#teardown#10 (expected 202, got 400).

Cause 2 of 3: an unforced delete of a policy that still holds a published VERSION is accepted instead of refused.
AWS: same operation; with `force` false Amazon Bedrock validates that all artifacts — explicitly including a policy version — have been deleted before it deletes the policy, and refuses otherwise with a modeled 400 (ResourceInUseException, httpError 400: "Thrown when attempting to delete or modify a resource that is currently being used by other resources or operations"). The recording shows that refusal: in the lifecycle fixture the policy has a version cut against it, and the first teardown delete answers 400.
Emulator: answers 202 and removes the policy. The fixed text of AWS's refusal for this class is not carried by the recording (only its status), so the error code and status are the requirement here: 400 with the ResourceInUse/validation error class, with the policy ARN from the request as the message's variable part.
Why: a client relying on this guard to protect a policy that still has published versions sees the policy and all its versions silently destroyed by a call AWS rejects, and never learns that it had to delete the versions or pass force=true.
Fix: the same unforced-delete eligibility check in the operation's handler, together with the policy state it reads. Hypothesis: the gate itself is written (the `versions` arm of `held_artifacts`) but never fires in this fixture because CreateAutomatedReasoningPolicyVersion failed earlier in the same fixture, so the emulator's store holds no version to block on — see the related open issue 20261029T211357Z-bedrock-resource-state-defects-definitionhash-policy-version-creation.md, which owns that version-creation failure but does NOT carry this delete step's replay key.
Traces: bedrock-automated-reasoning-policy-lifecycle#teardown#9 (expected 400, got 202).

Cause 3 of 3: a repeated delete of the same policy ARN answers 404 instead of succeeding.
AWS: the operation's own documentation states "Deletes an Automated Reasoning policy or policy version. This operation is idempotent. If you delete a policy more than once, each call succeeds." The model marks the operation `idempotent` and gives success code 202. The recording agrees: the second teardown delete of the same policy ARN answers 202 with an empty body.
Emulator: answers 404 ResourceNotFoundException. The same file returns `not_found(store.automated_reasoning_policy_write_not_found_message(policyArn))` whenever the ARN resolves to no revision, and its module doc declares that deviation deliberate ("a repeated delete reports the policy as missing rather than succeeding"). The corpus contradicts that reading: the status must be 202 with an empty body, and no ResourceNotFoundException error code or message may be emitted for an ARN this account previously held.
Why: any client that retries a delete — an SDK retry, an idempotent teardown step, a reconciliation loop — turns a successful cleanup into a ResourceNotFoundException and aborts, because the one guarantee AWS documents for this operation is that repeating it succeeds.
Fix: the not-found gate in the operation's handler — a well-formed AutomatedReasoningPolicyArn that this account's store no longer resolves must return the 202 empty success rather than the modeled 404.
Traces: bedrock-automated-reasoning-policy-lifecycle#teardown#10 (expected 202, got 404).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up: true, 162 steps, 103 passed, score 63.6%). Three unowned divergence clusters, all service=bedrock, operation=DeleteAutomatedReasoningPolicy, verdict=mismatch, stub_path empty, owned_by empty:

1. fixture bedrock-automated-reasoning-policy-build-workflow, step teardown#10 — divergence_key "service=bedrock|DeleteAutomatedReasoningPolicy|status=202->400|keys=status"; detail "status: expected 202, got 400". Expected: 202, empty body. Actual: 400 ResourceInUseException, message "automated reasoning policy <policyArn> still has build workflows that have not been deleted; delete them or retry the request with force set to true" (fixed text emitted by the emulator; <policyArn> is the request's ARN).

2. fixture bedrock-automated-reasoning-policy-lifecycle, step teardown#9 — divergence_key "service=bedrock|DeleteAutomatedReasoningPolicy|status=400->202|keys=status"; detail "status: expected 400, got 202". Expected: 400 (modeled ResourceInUseException/ValidationException class, policy still holds a version). Actual: 202, policy removed.

3. fixture bedrock-automated-reasoning-policy-lifecycle, step teardown#10 — divergence_key "service=bedrock|DeleteAutomatedReasoningPolicy|status=202->404|keys=status"; detail "status: expected 202, got 404". Expected: 202, empty body (documented idempotency). Actual: 404 ResourceNotFoundException.

Model evidence (smithy_query, services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): DeleteAutomatedReasoningPolicy http {method DELETE, uri /automated-reasoning-policies/{policyArn}, code 202}, idempotent: true; errors ResourceInUseException (400), ConflictException (400), ResourceNotFoundException (404), ValidationException (400); input `force` Boolean default false; input `policyArn` pattern ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$.

Emulator code: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/delete_automated_reasoning_policy.rs — not-found gate at lines 188-195, in-use/artifact gate at lines 221-236, `held_artifacts` at lines 287-316 (build_workflows included at line 294-297 and reported at line 309).

## Steps to Reproduce
1. git_clone, then replay_wire_corpus against the emulator built from this checkout.
2. Cause 1: replay fixture bedrock-automated-reasoning-policy-build-workflow. It creates an Automated Reasoning policy, starts an INGEST_CONTENT build workflow against it, then at teardown#10 issues DELETE /automated-reasoning-policies/{policyArn} with no force query parameter. Observe 400 ResourceInUseException where the recording has 202 and an empty body.
3. Causes 2 and 3: replay fixture bedrock-automated-reasoning-policy-lifecycle. It creates a policy, updates it, creates a policy version, then issues two DELETE /automated-reasoning-policies/{policyArn} calls in teardown. At teardown#9 observe 202 where the recording has 400 (the policy still holds a version and the delete was not forced). At teardown#10 observe 404 ResourceNotFoundException where the recording has 202 — the operation is documented idempotent, so a repeated delete must succeed.
4. Confirm the fix by re-replaying bedrock-automated-reasoning-policy-build-workflow#teardown#10, bedrock-automated-reasoning-policy-lifecycle#teardown#9 and bedrock-automated-reasoning-policy-lifecycle#teardown#10.
