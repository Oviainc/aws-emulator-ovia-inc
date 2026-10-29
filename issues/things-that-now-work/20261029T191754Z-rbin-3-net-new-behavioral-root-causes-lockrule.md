Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rbin.concurrent-lock-attempts-single-winner rbin.lock-state-transition-guards-rejected rbin.create-rule-tag-quota-exceeded-rejected
Issue-Shape-Id: com.amazonaws.rbin#AmazonRecycleBin
Issue-Key: rbin.concurrent-lock-attempts-single-winner|run#9|AssertionError|assert len(successes) == n_racers, f"n_ok={len(successes)} of {n_racers}"
Issue-Key: rbin.lock-state-transition-guards-rejected|run#11|AssertionError|assert message == (
Issue-Key: rbin.create-rule-tag-quota-exceeded-rejected|run#7|AssertionError|assert message == "SERVICE_QUOTA_EXCEEDED", f"expected the exact tag-quota message, got {message!r}"
Issue-Commit: 0e13384b9161da1d0d65830e33a6518406bea9f8

# rbin: 3 net-new behavioral root causes — LockRule rejects an idempotent relock, its already-locked ConflictException message is emulator-authored, and CreateRule's tag-quota ServiceQuotaExceededException message is prose instead of the reason token

> _Note: this issue was identified against the code base at commit `0e13384b9161da1d0d65830e33a6518406bea9f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LockRule

## Description
Three net-new, independent fidelity defects in the rbin (Amazon Recycle Bin) control plane, all reached through implemented handlers that answer — they are wrong answers, not stubs.

Cause 1 of 3: LockRule is not idempotent — relocking an already-locked rule with the SAME LockConfiguration is refused.
AWS: LockRule (Smithy: PATCH /rules/{Identifier}/lock, input LockConfiguration required, modeled errors ConflictException(409)/ResourceNotFoundException(404)/ValidationException(400)/InternalServerException(500)) accepts a lock request against a rule that is already locked when the requested unlock delay is the one in force: the call returns 200 with LockState="locked" and the rule's existing LockConfiguration, and no ConflictException is raised. ConflictException is documented as "The specified retention rule lock request can't be completed" and carries a Reason — it is the refusal for a request that conflicts with the lock state, not for a request that asks for the state the rule is already in. Recorded behavior confirms it: N concurrent LockRule calls with identical LockConfiguration against one unlocked rule all succeed.
Emulator: exactly one of the racing calls succeeds and every other one is refused (ConflictException on the already-locked rule); the check observed n_ok=1 of 6. Any repeat of a lock request — concurrent or sequential, with an unchanged unlock delay — is treated as a conflict.
Why: lock-then-lock is the normal shape of idempotent/retried control-plane code and of any concurrent reconciler. A client that retries LockRule after a timeout, or that has more than one worker converging a rule onto a locked state, gets a hard 409 from the emulator where AWS returns success, so correct code fails only under emulation.
Fix: the LockRule handler's lock-state guard (services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/lock_rule.rs and the lock-state transition logic it calls in src/state.rs): when the rule is already locked and the requested UnlockDelay equals the one in force, return the rule unchanged with LockState=locked instead of raising ConflictException.
Traces: rbin.concurrent-lock-attempts-single-winner (n_ok=1 of 6), and the positive-control half of rbin.lock-state-transition-guards-rejected.

Cause 2 of 3: the ConflictException message for a conflicting relock is emulator-authored text, not AWS's wording.
AWS: when LockRule is called against an already-locked rule with a DIFFERENT unlock delay, AWS refuses with code ConflictException (HTTP 409) and the message "Cannot lock Recycle Bin rule <Identifier> because it has a different unlock delay already set." — fixed text "Cannot lock Recycle Bin rule " + " because it has a different unlock delay already set.", with the single variable part being the rule Identifier taken from the request (recorded behavior; the Smithy model types the member only as ConflictException$Message with a ConflictExceptionReason enum, so the wording itself is recorded behavior).
Emulator: same code (ConflictException) but the message "Cannot lock Recycle Bin rule <Identifier> because it is already locked." — i.e. it reports the lock STATE as the cause rather than the delay mismatch, so it uses the same text for every relock, including the one AWS accepts (Cause 1). The identifier in the observed message was a generated rule id and is a value from state, not part of the template.
Why: the message is the only thing distinguishing "your unlock delay differs from the one in force" from "this rule is locked"; a client or operator that matches on, logs or surfaces it mis-diagnoses the failure, and message-asserting tests written against AWS fail.
Fix: the refusal path of the LockRule handler (src/ops/lock_rule.rs) — raise the conflict only on a delay mismatch and render AWS's wording for it.
Traces: rbin.lock-state-transition-guards-rejected (assertion at line 54 on the ConflictException message). This check also asserts, downstream of the first failure, that UnlockRule on a never-locked rule refuses with ConflictException and the message "Cannot unlock Recycle Bin rule <Identifier> because it is not locked."; those assertions were never reached and must be re-verified when the check is replayed.

Cause 3 of 3: CreateRule's tag-quota ServiceQuotaExceededException carries a descriptive sentence where AWS sends the reason token.
AWS: CreateRule rejects a request whose Tags exceed the 50-tags-per-resource quota with code ServiceQuotaExceededException (Smithy: CreateRule error ServiceQuotaExceededException, httpError 402, doc "The request would cause a service quota for the number of tags per resource to be exceeded", members Message + Reason:ServiceQuotaExceededExceptionReason). The recorded message is the reason token itself, the fixed string "SERVICE_QUOTA_EXCEEDED", with no variable parts; and no rule is persisted. Exactly 50 tags is accepted.
Emulator: returns ServiceQuotaExceededException (code correct) with the message "The request would create a retention rule with 51 tags, exceeding the quota of 50 tags per resource" — emulator-authored prose that interpolates the submitted tag count and the limit. The count 51 and limit 50 in the observed text come from the request and the quota, so no part of that sentence is AWS's text.
Why: a client that asserts on or classifies by the error message (the usual way to separate the tag-count quota from the other quota refusals that share this code) sees text AWS never sends.
Fix: the tag-quota validation in the CreateRule handler (src/ops/create_rule.rs / its validation module) — emit the reason token as the Message (and set Reason) instead of a formatted sentence.
Traces: rbin.create-rule-tag-quota-exceeded-rejected.

Related, NOT part of this issue: rbin.ebs-snapshot-retained-in-recycle-bin-on-delete errors on ec2 CreateSnapshot answering NotImplemented, which is already owned by the open issues 20261022T235108Z-ec2-createsnapshot-stub.md and 20261024T155459Z-rbin-createrule-event-propagation.md (rbin rules are never consulted on deletion); it is deliberately not re-filed here.

## Evidence
Suite: rbin 15/19 (79%), 4 failed.

1) [mismatch] rbin.concurrent-lock-attempts-single-winner — AssertionError: n_ok=1 of 6 (check_rbin_concurrent_lock_attempts_single_winner.py:52, `assert len(successes) == n_racers`). Expected: all 6 concurrent LockRule calls with LockConfiguration={"UnlockDelay":{"UnlockDelayValue":7,"UnlockDelayUnit":"DAYS"}} succeed and GetRule then reports LockState="locked". Actual: 1 success, 5 ClientError refusals.

2) [mismatch] rbin.lock-state-transition-guards-rejected — AssertionError at check_rbin_lock_state_transition_guards_rejected.py:54. Expected message: "Cannot lock Recycle Bin rule <id> because it has a different unlock delay already set." Actual message: "Cannot lock Recycle Bin rule 00000000001 because it is already locked." (code ConflictException matched; "00000000001" is the generated rule id, a state value, not part of the template).

3) [mismatch] rbin.create-rule-tag-quota-exceeded-rejected — AssertionError: expected the exact tag-quota message, got 'The request would create a retention rule with 51 tags, exceeding the quota of 50 tags per resource' (check_rbin_create_rule_tag_quota_exceeded_rejected.py:32). Expected code ServiceQuotaExceededException (matched) with message "SERVICE_QUOTA_EXCEEDED".

Model citations: smithy_query(operation="LockRule") — PATCH /rules/{Identifier}/lock, 200, errors ConflictException 409 / ResourceNotFoundException 404 / ValidationException 400; LockState enum values [locked, pending_unlock, unlocked]; LockConfiguration.UnlockDelay required. smithy_query(operation="CreateRule") — POST /rules, 201, errors ServiceQuotaExceededException 402 / ValidationException 400. smithy_query(shape="ServiceQuotaExceededException") — doc "The request would cause a service quota for the number of tags per resource to be exceeded", members Message + Reason(ServiceQuotaExceededExceptionReason). smithy_query(shape="ConflictException") — doc "The specified retention rule lock request can't be completed", members Message + Reason.

## Steps to Reproduce
Cause 1:
1. POST /rules {"ResourceType":"EBS_VOLUME","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"}} -> 201, note Identifier.
2. Issue 6 concurrent PATCH /rules/{Identifier}/lock with {"LockConfiguration":{"UnlockDelay":{"UnlockDelayValue":7,"UnlockDelayUnit":"DAYS"}}}.
3. Expect all 6 -> 200 with LockState="locked"; actual 1 x 200 and 5 x 409 ConflictException. (Sequentially: the second identical lock is likewise refused.)

Cause 2:
1. Create a rule and lock it with UnlockDelayValue=7.
2. PATCH /rules/{Identifier}/lock with UnlockDelayValue=10.
3. Expect 409 ConflictException, message "Cannot lock Recycle Bin rule {Identifier} because it has a different unlock delay already set."; actual "...because it is already locked."
4. GetRule must still show LockState=locked and UnlockDelayValue=7 (unchanged).
5. Also re-verify the companion refusal: UnlockRule on a never-locked rule -> 409 ConflictException "Cannot unlock Recycle Bin rule {Identifier} because it is not locked.", and GetRule reports no LockState.

Cause 3:
1. POST /rules with ResourceType=EBS_VOLUME, RetentionPeriod 3 DAYS and 51 Tags.
2. Expect 402 ServiceQuotaExceededException with Message "SERVICE_QUOTA_EXCEEDED"; actual the same code with the prose message quoted in Evidence.
3. ListRules for EBS_VOLUME must contain no rule with that Description (no partial persist).
4. Repeat with exactly 50 Tags -> 201, and ListTagsForResource on the rule ARN returns 50 tags.
