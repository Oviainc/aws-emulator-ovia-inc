Issue-Source: trace-sweep-divergence
Issue-Trace: events-custom-event-bus-rule-target-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-custom-event-bus-rule-target-lifecycle events-input-transformer-config-roundtrip events-multi-target-rule-management-and-quota-errors
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|PutTargets|2xx->2xx|$.FailedEntries
Issue-Key: events|RemoveTargets|2xx->2xx|$.FailedEntries
Issue-Key: events|ListTargetsByRule|2xx->2xx|$
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# EventBridge target responses omit their empty FailedEntries and Targets members

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
The emulator's target-management responses drop list members when the list is empty, so a success response is indistinguishable from a response in which the member was not supported. AWS always emits the member (as []). Three causes; a fixer must close ALL THREE.

Cause 1 of 3 - PutTargets omits FailedEntries on full success.
AWS: Smithy PutTargetsResponse = {FailedEntryCount (Integer, default 0), FailedEntries (PutTargetsResultEntryList)}. A fully successful PutTargets returns 200 with FailedEntryCount 0 and FailedEntries as an empty list.
Emulator: 200 with FailedEntryCount present but $.FailedEntries absent.
Why: SDK and hand-written clients that iterate response.FailedEntries unconditionally get a null/None and either crash or skip the failure-handling branch entirely; a client cannot tell 'no failures' from 'failure reporting unimplemented'.
Fix: PutTargets serializer must always emit the FailedEntries member, empty list included.
Traces: events-custom-event-bus-rule-target-lifecycle#task#5; also events-input-transformer-config-roundtrip#task#3, events-multi-target-rule-management-and-quota-errors#task#2.

Cause 2 of 3 - RemoveTargets omits FailedEntries on full success.
AWS: Smithy RemoveTargetsResponse = {FailedEntryCount (default 0), FailedEntries (RemoveTargetsResultEntryList)}; a successful removal returns 200 with FailedEntries = [].
Emulator: 200 with $.FailedEntries absent.
Why: same as cause 1, on the removal path - partial-failure handling is unreachable.
Fix: RemoveTargets serializer must always emit FailedEntries.
Traces: events-custom-event-bus-rule-target-lifecycle#task#10; also events-multi-target-rule-management-and-quota-errors#task#4, events-multi-target-rule-management-and-quota-errors#task#8.

Cause 3 of 3 - ListTargetsByRule returns an empty body for a rule with no targets.
AWS: Smithy ListTargetsByRuleResponse = {Targets (TargetList), NextToken}. For an existing rule whose targets have all been removed, the recording shows 200 with body {'Targets': []}.
Emulator: 200 with a completely empty body {} - neither Targets nor NextToken.
Why: a client that reads response.Targets to confirm a rule was drained gets nothing to iterate; pagination/emptiness logic cannot distinguish an empty rule from a malformed response.
Fix: ListTargetsByRule serializer must emit Targets (empty list when the rule has no targets).
Traces: events-multi-target-rule-management-and-quota-errors#task#9.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator.

Cause 1 - events-custom-event-bus-rule-target-lifecycle#task#5 PutTargets: expected 200, got 200; $.FailedEntries missing (expected [], got None). 3 steps affected.
Cause 2 - events-custom-event-bus-rule-target-lifecycle#task#10 RemoveTargets: 200/200; $.FailedEntries missing (expected [], got None). 3 steps affected.
Cause 3 - events-multi-target-rule-management-and-quota-errors#task#9 ListTargetsByRule: 200/200; $ empty_body (expected {'Targets': []}, got {}).

Model evidence: PutTargetsResponse{FailedEntryCount, FailedEntries}, RemoveTargetsResponse{FailedEntryCount, FailedEntries}, ListTargetsByRuleResponse{Targets, NextToken} per the on-disk Smithy model. The divergence is structural (member presence), not a value or message difference.

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay the wire corpus (replay_wire_corpus).
2. PutRule on a bus, then PutTargets with a valid target: the 200 body has no FailedEntries member.
3. RemoveTargets for that target id: the 200 body has no FailedEntries member.
4. ListTargetsByRule for the now-empty rule: the 200 body is {} instead of {"Targets": []}.
5. Re-replay events-custom-event-bus-rule-target-lifecycle#task#5, events-custom-event-bus-rule-target-lifecycle#task#10 and events-multi-target-rule-management-and-quota-errors#task#9.
