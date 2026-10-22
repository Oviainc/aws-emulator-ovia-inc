Issue-Source: trace-sweep-divergence
Issue-Trace: rbin-invalid-parameter-and-notfound-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rbin-invalid-parameter-and-notfound-errors rbin-tag-level-rule-lifecycle rbin-lock-conflict-tag-level-rule rbin-lock-unlock-lifecycle rbin-update-and-delete-conflict-when-locked
Issue-Shape-Id: com.amazonaws.rbin#AmazonRecycleBin
Issue-Key: rbin|CreateRule|4xx->4xx|$.Message
Issue-Key: rbin|CreateRule|4xx->4xx|$.Reason
Issue-Key: rbin|GetRule|4xx->4xx|$.Message
Issue-Key: rbin|GetRule|4xx->4xx|$.Reason
Issue-Key: rbin|ListRules|4xx->4xx|$.Message
Issue-Key: rbin|ListRules|4xx->4xx|$.Reason
Issue-Key: rbin|DeleteRule|4xx->4xx|$.Message
Issue-Key: rbin|DeleteRule|4xx->4xx|$.Reason
Issue-Key: rbin|LockRule|4xx->4xx|$.Message
Issue-Key: rbin|UpdateRule|4xx->4xx|$.Message
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# rbin error responses: ValidationException body omits Message and Reason, 404 adds an unmodeled Reason, conflict wording diverges

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
THREE distinct root causes in how the rbin (Amazon Recycle Bin) emulator builds its modeled error payloads. Every one of them answers with the right HTTP status, so nothing here is a status defect: the defects are in the error BODY. All THREE must be closed before submitting, and each names its own fixture#step below.

Related but NOT covering: the open issue 20261022T161637Z-rbin-retention-rule-fidelity-createrule-skips-the-pending.md describes the not-found wording and the validation-message formatting from the BEHAVIORAL corpus (its keys are rbin.* check ids). It carries none of the wire fixture steps below, and it does not mention the completely ABSENT Message/Reason seen here or the ConflictException wording at all, so these causes are recorded with their own wire replay keys.

Cause 1 of 3: a constraint-rejected request answers 400 with an EMPTY ValidationException body — no Message and no Reason member at all.
AWS: Smithy: ValidationException is a client error with httpError 400 and members Message (ErrorMessage) and Reason (ValidationExceptionReason, values [INVALID_PAGE_TOKEN, INVALID_PARAMETER_VALUE]); it is a service-common error bound to every rbin operation. Recorded behavior: a request that violates a modeled member constraint answers 400 with Reason set to the enum value for that class of fault (INVALID_PARAMETER_VALUE for a bad parameter value) and a Message built from the request, naming the member path and the constraint it failed, in the form "1 validation error detected: Value at '<memberPath>' failed to satisfy constraint: Member must <constraint>". The message is composed from the request, so that COMPOSITION (member path + constraint, no echoed value) is the requirement, not any one recorded sentence; the recorded strings below are examples.
Emulator: for all three recorded constraint classes the status is 400 but BOTH members are missing from the JSON body (replay reports "$.Message: missing ... got None" and "$.Reason: missing ... got None"). Failing steps and the constraint each one exercises:
  - CreateRule, retentionPeriod.retentionPeriodValue above its modeled range — fixture rbin-invalid-parameter-and-notfound-errors step task#1 (expected Message "1 validation error detected: Value at 'retentionPeriod.retentionPeriodValue' failed to satisfy constraint: Member must have value less than or equal to 3650", Reason INVALID_PARAMETER_VALUE).
  - GetRule, identifier failing the RuleIdentifier pattern — fixture rbin-invalid-parameter-and-notfound-errors step task#2 (expected Message "1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z]{11}", Reason INVALID_PARAMETER_VALUE).
  - ListRules, nextToken failing its pattern — fixture rbin-invalid-parameter-and-notfound-errors step task#4 (expected Message "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9+/=]{1,2048}$", Reason INVALID_PARAMETER_VALUE).
Why: a client gets a 400 it cannot interpret. SDK error objects expose an empty message, so nothing surfaces to a user or a log, and any client that branches on the modeled Reason enum (retry/refresh on INVALID_PAGE_TOKEN vs fail fast on INVALID_PARAMETER_VALUE) sees Reason absent and takes the wrong branch. The fault is uniform across range and pattern constraints and across three operations, so it is the shared constraint path, not one handler.
Fix: the centralized constraint-validation step of the request walk (deserialize -> validate -> handle -> serialize) and its ValidationException serializer in this crate — populate Reason from the fault class and compose Message from the member path and failed constraint before the 400 is written.
Traces: rbin-invalid-parameter-and-notfound-errors#task#1, rbin-invalid-parameter-and-notfound-errors#task#2, rbin-invalid-parameter-and-notfound-errors#task#4.

Cause 2 of 3: the 404 ResourceNotFoundException composes its Message differently and adds a Reason member the recordings never carry.
AWS: Smithy: ResourceNotFoundException is a client error, httpError 404, with members Message and Reason (ResourceNotFoundExceptionReason, sole value RULE_NOT_FOUND). Recorded behavior: GetRule/DeleteRule on a well-formed but unknown or already-deleted Identifier answer 404 with a Message composed from the requested identifier as "Recycle Bin Rule '<identifier>' does not exist." and with NO Reason member present. The composition from the request's Identifier is the requirement; the absence of Reason on this error is recorded behavior only (the member is modeled, but the service does not populate it on this path).
Emulator: the 404 and the error code are correct, but the Message is composed as "Retention rule with ID <identifier> not found" and a Reason of 'RULE_NOT_FOUND' is added. Failing steps: DeleteRule on a nonexistent identifier — fixture rbin-invalid-parameter-and-notfound-errors step task#3 (expected "Recycle Bin Rule 'ZZZZZZZZZZZ' does not exist.", got 'Retention rule with ID ZZZZZZZZZZZ not found', plus Reason 'RULE_NOT_FOUND' where the recording has none); GetRule on a just-deleted identifier — fixture rbin-tag-level-rule-lifecycle step teardown#7 (same two differences).
Why: a client that surfaces or matches AWS's not-exist wording (idempotent-delete handling, log scraping, user-facing messages) does not match, and a client that treats a present Reason as meaningful sees a field AWS does not send on this path, so the emulator validates code the real service would leave untested.
Fix: the ResourceNotFoundException construction in the GetRule and DeleteRule handlers (and any other rbin handler raising it for an unknown identifier) — compose the message from the requested identifier in AWS's form and leave Reason unset on this path.
Traces: rbin-invalid-parameter-and-notfound-errors#task#3, rbin-tag-level-rule-lifecycle#teardown#7.

Cause 3 of 3: the 409 ConflictException Message is not composed in AWS's form.
AWS: Smithy: ConflictException is a client error, httpError 409, with members Message and Reason (ConflictExceptionReason, sole value INVALID_RULE_STATE); it is modeled on LockRule, UnlockRule, UpdateRule and DeleteRule. Recorded behavior: a rejected lock/modify/delete answers 409 with a Message composed from the request's identifier and the blocking condition, in the form "Cannot <action> Recycle Bin rule <identifier> because it is <condition>." The composition from the request identifier plus the condition is the requirement; the recorded sentences below are examples of it. Status, error code and Reason all match already — only the composition differs.
Emulator: the message is composed from the rule's internal lock_state string instead, e.g. "Retention rule with ID 00000000001 is a tag-level retention rule and can't be locked" where the recording has "Cannot lock Recycle Bin rule 00000000001 because it is a tag-level rule." (LockRule, fixture rbin-lock-conflict-tag-level-rule step task#2); "Retention rule with ID 00000000001 can't be deleted while its lock state is pending_unlock" where the recording has "Cannot delete Recycle Bin rule 00000000001 because it is locked." (DeleteRule, fixture rbin-lock-unlock-lifecycle step teardown#6, cluster also covers rbin-update-and-delete-conflict-when-locked#task#4); "Retention rule with ID 00000000001 can't be modified while its lock state is locked" where the recording has "Cannot modify Recycle Bin rule 00000000001 because it is locked." (UpdateRule, fixture rbin-update-and-delete-conflict-when-locked step task#3).
Why: the emulator leaks an internal state token (pending_unlock/locked) into a client-visible message instead of AWS's condition wording, so a client that surfaces or matches the conflict message diverges, and a rule in pending_unlock is described to the caller differently from the way AWS describes it.
Fix: the ConflictException construction shared by the LockRule, UpdateRule and DeleteRule handlers in this crate — compose the message from the action and the blocking condition in AWS's form rather than from the stored lock-state string.
Traces: rbin-lock-conflict-tag-level-rule#task#2, rbin-lock-unlock-lifecycle#teardown#6, rbin-update-and-delete-conflict-when-locked#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (8 fixtures, 38 steps, 19 passed, 19 mismatched; emulator up, 0 state_reset_failures).

Cause 1 — status 400 expected and 400 returned, body empty:
  rbin-invalid-parameter-and-notfound-errors#task#1 CreateRule: $.Message missing (expected "1 validation error detected: Value at 'retentionPeriod.retentionPeriodValue' failed to satisfy constraint: Member must have value less than or equal to 3650", got None); $.Reason missing (expected 'INVALID_PARAMETER_VALUE', got None).
  rbin-invalid-parameter-and-notfound-errors#task#2 GetRule: $.Message missing (expected "...Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z]{11}", got None); $.Reason missing (expected 'INVALID_PARAMETER_VALUE', got None).
  rbin-invalid-parameter-and-notfound-errors#task#4 ListRules: $.Message missing (expected "...Value at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9+/=]{1,2048}$", got None); $.Reason missing (expected 'INVALID_PARAMETER_VALUE', got None).

Cause 2 — status 404 expected and 404 returned:
  rbin-invalid-parameter-and-notfound-errors#task#3 DeleteRule: $.Message value_mismatch (expected "Recycle Bin Rule 'ZZZZZZZZZZZ' does not exist.", got 'Retention rule with ID ZZZZZZZZZZZ not found'); $.Reason type_mismatch (expected None, got 'RULE_NOT_FOUND').
  rbin-tag-level-rule-lifecycle#teardown#7 GetRule: $.Message value_mismatch (expected "Recycle Bin Rule '00000000001' does not exist.", got 'Retention rule with ID 00000000001 not found'); $.Reason type_mismatch (expected None, got 'RULE_NOT_FOUND').

Cause 3 — status 409 expected and 409 returned, Message only:
  rbin-lock-conflict-tag-level-rule#task#2 LockRule: expected 'Cannot lock Recycle Bin rule 00000000001 because it is a tag-level rule.', got "Retention rule with ID 00000000001 is a tag-level retention rule and can't be locked".
  rbin-lock-unlock-lifecycle#teardown#6 DeleteRule (cluster affects 2 steps, also rbin-update-and-delete-conflict-when-locked#task#4): expected 'Cannot delete Recycle Bin rule 00000000001 because it is locked.', got "Retention rule with ID 00000000001 can't be deleted while its lock state is pending_unlock".
  rbin-update-and-delete-conflict-when-locked#task#3 UpdateRule: expected 'Cannot modify Recycle Bin rule 00000000001 because it is locked.', got "Retention rule with ID 00000000001 can't be modified while its lock state is locked".

Model references used: smithy_query shape=ValidationException/ValidationExceptionReason/ResourceNotFoundException/ResourceNotFoundExceptionReason/ConflictException/ConflictExceptionReason on services/service-com-amazonaws-rbin-amazonrecyclebin/smithy-model/com-amazonaws-rbin-amazonrecyclebin.json; digest shows ValidationException as a service-common error and ConflictException bound to DeleteRule/LockRule/UnlockRule/UpdateRule.

## Steps to Reproduce
1. Cause 1a: POST /rules (CreateRule) with ResourceType EBS_SNAPSHOT and RetentionPeriod {RetentionPeriodValue: >3650, RetentionPeriodUnit: DAYS} -> expect 400 ValidationException with Reason INVALID_PARAMETER_VALUE and a Message naming 'retentionPeriod.retentionPeriodValue' and its range constraint; observe 400 with neither member in the body. (rbin-invalid-parameter-and-notfound-errors#task#1)
2. Cause 1b: GET /rules/{Identifier} (GetRule) with an Identifier that fails the ^[0-9a-zA-Z]{11}$ pattern -> expect 400 with Reason INVALID_PARAMETER_VALUE and a Message naming 'identifier' and the pattern; observe an empty body. (task#2)
3. Cause 1c: POST /list-rules (ListRules) with a NextToken that fails ^[A-Za-z0-9+/=]{1,2048}$ -> expect 400 with Reason INVALID_PARAMETER_VALUE and a Message naming 'nextToken' and the pattern; observe an empty body. (task#4)
4. Cause 2: DELETE /rules/ZZZZZZZZZZZ (DeleteRule, well-formed but nonexistent) -> expect 404 ResourceNotFoundException, Message "Recycle Bin Rule 'ZZZZZZZZZZZ' does not exist." and NO Reason member; observe 'Retention rule with ID ZZZZZZZZZZZ not found' plus Reason 'RULE_NOT_FOUND'. Repeat with GetRule on an identifier just deleted in the same fixture. (task#3, rbin-tag-level-rule-lifecycle#teardown#7)
5. Cause 3: create a TAG-level rule (CreateRule with ResourceTags) then PATCH /rules/{id}/lock -> expect 409 ConflictException "Cannot lock Recycle Bin rule <id> because it is a tag-level rule."; observe the lock_state-derived wording. Then create a LOCKED Region-level rule and PATCH /rules/{id} (UpdateRule) and DELETE /rules/{id} -> expect "Cannot modify/delete Recycle Bin rule <id> because it is locked."; observe wording built from the internal lock state (including 'pending_unlock' on the unlock-delay path). (rbin-lock-conflict-tag-level-rule#task#2, rbin-update-and-delete-conflict-when-locked#task#3, rbin-lock-unlock-lifecycle#teardown#6)
