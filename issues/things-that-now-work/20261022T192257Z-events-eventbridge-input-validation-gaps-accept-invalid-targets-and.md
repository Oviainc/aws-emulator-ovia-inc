Issue-Source: trace-sweep-divergence
Issue-Trace: events-put-targets-validation-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-put-targets-validation-errors events-put-events-per-entry-failure-envelope events-create-partner-event-source-invalid-name-pattern events-partner-event-source-invalid-inputs
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|PutTargets|4xx->2xx|$.__type
Issue-Key: events|PutTargets|4xx->2xx|$.message
Issue-Key: events|PutTargets|4xx->2xx|status
Issue-Key: events|DeleteRule|2xx->4xx|status
Issue-Key: events|PutEvents|2xx->2xx|$.Entries[*].ErrorCode
Issue-Key: events|PutEvents|2xx->2xx|$.Entries[*].ErrorMessage
Issue-Key: events|PutEvents|2xx->2xx|$.FailedEntryCount
Issue-Key: events|CreatePartnerEventSource|4xx->4xx|$.message
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# EventBridge input validation gaps accept invalid targets and event entries

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Request validation on the events write paths is incomplete: invalid requests are accepted with 200, the corrupted state they leave then breaks a later legitimate call, a per-entry failure is not reported at all, and one validation message quotes a constraint AWS does not. Four causes; a fixer must close ALL FOUR.

Cause 1 of 4 - PutTargets accepts two targets with the same Id.
AWS: Smithy PutTargets input Targets is a TargetList (@length 1..100) of Target{Id (required, @length 1..64, pattern ^[\.\-_A-Za-z0-9]+$), Arn (required), ...}; Ids must be unique within a rule because Id is the update key. The recording shows HTTP 400 with __type ValidationException (message example: "Parameter targets is not valid. Reason: More than one target with Id 'dup' is provided."), i.e. the whole request is rejected - not a per-entry FailedEntries result.
Emulator: HTTP 200, no $.__type and no $.message - the duplicate-Id request is accepted and the targets are installed.
Why: a client that mistakenly reuses a target Id gets silent success and then an ambiguous rule whose target set cannot be addressed or removed deterministically; the modeled ValidationException branch is unreachable.
Fix: PutTargets request validation must reject a Targets list containing duplicate Id values with 400 / ValidationException, before any state mutation.
Traces: events-put-targets-validation-errors#task#3; also events-put-targets-validation-errors#task#4.

Cause 2 of 4 - DeleteRule then fails 400 on a rule that AWS deletes with 200.
AWS: Smithy DeleteRule errors = [ConcurrentModificationException, InternalException, ManagedRuleException, ResourceNotFoundException]; in the recording the teardown DeleteRule for this rule succeeds with HTTP 200 (the rule's targets were never installed, because the request in cause 1 was rejected).
Emulator: HTTP 400 for that same DeleteRule. Hypothesis: this is the downstream consequence of cause 1 - the wrongly-accepted duplicate-Id targets remain attached, so the emulator's 'rule still has targets' guard refuses the delete. It must be re-checked after cause 1 is fixed, and if a 400 survives, the DeleteRule guard itself is wrong.
Why: teardown cannot complete, so the rule leaks and a client cannot reclaim its name; worse, the failure is reported on a request that is itself valid.
Fix: PutTargets validation (cause 1); then the DeleteRule handler's target/force guard.
Traces: events-put-targets-validation-errors#teardown#5.

Cause 3 of 4 - PutEvents does not fail an entry whose DetailType is over 128 characters.
AWS: PutEventsRequestEntry.DetailType is documented as a "Free-form string, with a maximum of 128 characters", and PutEventsResultEntry documents the non-retryable per-entry code InvalidArgument ("A specified parameter is not valid"). An over-long DetailType therefore produces HTTP 200 with FailedEntryCount 1 and Entries[i].{ErrorCode: 'InvalidArgument', ErrorMessage: <names the parameter and the 128-character limit>} (recorded example: "Parameter DetailType is not valid. Reason: DetailType can't have more than 128 characters.").
Emulator: HTTP 200 with $.FailedEntryCount 0 and Entries[0] carrying neither ErrorCode nor ErrorMessage - the entry is accepted and published.
Why: the per-entry failure envelope is the only way a PutEvents caller learns an event was dropped; reporting 0 failures for an invalid entry makes the client believe an event was delivered that AWS would have rejected.
Fix: PutEvents per-entry validation must enforce the DetailType length and populate FailedEntryCount plus the entry's ErrorCode/ErrorMessage.
Traces: events-put-events-per-entry-failure-envelope#task#2.

Cause 4 of 4 - CreatePartnerEventSource's validation message quotes an anchored pattern AWS does not emit.
AWS: CreatePartnerEventSource.Name targets EventSourceName with pattern ^aws\.partner(/[\.\-_A-Za-z0-9]+){2,}$ (@length 1..256). AWS's standard validation message is built from the request and the violated constraint, and it quotes the constraint WITHOUT the regex anchors: "1 validation error detected: Value '<value>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: aws\.partner(/[\.\-_A-Za-z0-9]+){2,}". Status 400 and the error code already match.
Emulator: same status and code, but the message quotes the anchored Smithy form ^aws\.partner(/[\.\-_A-Za-z0-9]+){2,}$.
Why: the message is generated from a known rule (member name + modeled constraint), so clients and fixtures that assert on the reported pattern - and humans copying it into a regex test - get a pattern string AWS never returns. Lowest-severity cause in this bundle; it travels with HIGH causes.
Fix: the validation-message formatter must strip the leading ^ and trailing $ from a modeled pattern when rendering it.
Traces: events-create-partner-event-source-invalid-name-pattern#task#1; also events-partner-event-source-invalid-inputs#task#1, events-partner-event-source-invalid-inputs#task#2.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator.

Cause 1 - events-put-targets-validation-errors#task#3 PutTargets: status expected 400, got 200; $.__type missing (expected 'ValidationException', got None); $.message missing (expected "Parameter targets is not valid. Reason: More than one target with Id 'dup' is provided.", got None). 2 steps affected (task#3, task#4).
Cause 2 - events-put-targets-validation-errors#teardown#5 DeleteRule: status expected 200, got 400.
Cause 3 - events-put-events-per-entry-failure-envelope#task#2 PutEvents: 200/200; $.FailedEntryCount value_mismatch (expected 1, got 0); $.Entries[0].ErrorCode missing (expected 'InvalidArgument', got None); $.Entries[0].ErrorMessage missing (expected "Parameter DetailType is not valid. Reason: DetailType can't have more than 128 characters.", got None).
Cause 4 - events-create-partner-event-source-invalid-name-pattern#task#1 CreatePartnerEventSource: 400/400; $.message value_mismatch - expected the constraint quoted as aws\\.partner(/[\\.\\-_A-Za-z0-9]+){2,}, got ^aws\\.partner(/[\\.\\-_A-Za-z0-9]+){2,}$. 3 steps affected.

Model evidence: Target.Id required @length 1..64 pattern ^[\.\-_A-Za-z0-9]+$; DeleteRule errors include ManagedRuleException/ResourceNotFoundException but the recorded teardown is a 200; PutEventsRequestEntry.DetailType doc states the 128-character maximum and PutEventsResultEntry.ErrorCode documents InvalidArgument; CreatePartnerEventSource.Name pattern ^aws\.partner(/[\.\-_A-Za-z0-9]+){2,}$.

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay the wire corpus (replay_wire_corpus).
2. PutRule a rule, then PutTargets with two Target entries sharing Id 'dup': the emulator answers 200 instead of 400 / ValidationException.
3. DeleteRule that rule (no Force): the emulator answers 400 where the recording shows 200.
4. PutEvents with one entry whose DetailType exceeds 128 characters: the 200 body reports FailedEntryCount 0 and the entry carries no ErrorCode/ErrorMessage instead of InvalidArgument.
5. CreatePartnerEventSource with Name 'not-a-partner-name': the 400 message quotes the pattern with ^...$ anchors.
6. Re-replay events-put-targets-validation-errors#task#3, events-put-targets-validation-errors#teardown#5, events-put-events-per-entry-failure-envelope#task#2 and events-create-partner-event-source-invalid-name-pattern#task#1.
