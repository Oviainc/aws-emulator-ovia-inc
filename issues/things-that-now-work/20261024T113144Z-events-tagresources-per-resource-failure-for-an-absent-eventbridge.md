Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__resourcegroupstaggingapi.tag-resources-nonexistent-rule-rejected-per-resource
Issue-Key: events__resourcegroupstaggingapi.tag-resources-nonexistent-rule-rejected-per-resource|run#18|AssertionError|assert detail["ErrorMessage"] == expected_message, (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# TagResources per-resource failure for an absent EventBridge rule carries a generic ARN-echoing message instead of naming the rule and its event bus

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResources

## Description
One cause.

Cause 1 of 1: the FailureInfo.ErrorMessage EventBridge contributes for a well-formed but absent rule ARN does not follow EventBridge's own wording rule.

AWS: in a ResourceGroupsTaggingAPI TagResources batch, the per-resource FailureInfo carries the hosting service's own error (Smithy: resourcegroupstaggingapi#FailureInfo.ErrorCode — "any valid error code returned by the Amazon Web Services service that hosts the resource"; members StatusCode, ErrorCode, ErrorMessage). EventBridge's ResourceNotFoundException is modeled only as "An entity that you specified does not exist" (Smithy: cloudwatchevents#ResourceNotFoundException), so the exact text is recorded behavior: EventBridge builds the message FROM THE REQUEST, identifying the missing rule by NAME and the event bus it was looked up on — e.g. "Rule <rule-name> does not exist on EventBus default." The requirement is that rule (rule name + event bus identified), not the literal recorded string. ErrorCode ResourceNotFoundException and StatusCode 400 are also required and the emulator already gets both right.

Emulator: the per-resource ErrorMessage is a generic, service-agnostic string that echoes the caller's ARN back — "The specified resource wasn't found: arn:aws:events:<region>:<account>:rule/<name>" — naming neither the rule by name nor the event bus. It looks like a shared tagging-layer not-found message rather than EventBridge's own.

Why: a client that parses or logs the per-resource failure gets no indication of which event bus the rule was sought on (the usual cause of this failure is a rule that exists on a non-default bus), and message-matching tooling carried over from AWS does not recognise the failure.

Fix: the EventBridge-side resolution used by the tagging layer's per-resource tag/untag path should raise EventBridge's own ResourceNotFoundException wording, composed from the request's rule name and event bus name, instead of the generic ARN-echo message in the shared tagging not-found path. Related wording-only issues already open for other services: 20261024T090454Z-medical-imaging-listtagsforresource-error-response.md ("echoing the caller's ARN instead of the resource kind") — it does not cover EventBridge.

Traces: events__resource-groups-tagging-api.tag-resources-nonexistent-rule-rejected-per-resource (mismatch; ErrorCode and StatusCode matched, ErrorMessage diverged).

## Evidence
Expected (recorded): FailedResourcesMap[<ghost rule ARN>] = {ErrorCode: "ResourceNotFoundException", StatusCode: 400, ErrorMessage: "Rule <ghost-name> does not exist on EventBus default."} — message composed from the requested rule name and event bus.
Actual: ErrorCode "ResourceNotFoundException" and StatusCode 400 (both correct), ErrorMessage "The specified resource wasn't found: arn:aws:events:us-west-2:000000000000:rule/<ghost-name>".
AssertionError: the per-resource ErrorMessage for an absent rule was "The specified resource wasn't found: arn:aws:events:...:rule/sim2real-behav-ghost-...", expected 'Rule sim2real-behav-ghost-... does not exist on EventBus default.'

## Steps to Reproduce
1. events PutRule with an EventPattern and State=ENABLED; keep its RuleArn.
2. Build a well-formed ARN for a rule that does not exist: arn:aws:events:<region>:<account>:rule/<ghost-name>.
3. resourcegroupstaggingapi TagResources with ResourceARNList=[live rule ARN, ghost rule ARN] and one tag.
4. Inspect FailedResourcesMap[ghost ARN].ErrorMessage — it echoes the ARN instead of naming the rule and its event bus.
