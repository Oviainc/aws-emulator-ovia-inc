Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.eventbridge-input-transformer-missing-path-not-delivered
Issue-Key: events__sqs.eventbridge-input-transformer-missing-path-not-delivered|run#25|AssertionError|raise AssertionError(
Issue-Commit: d3131d60bceeaea824d338672e432581131c6d9f

# EventBridge: PutTargets accepts an InputTransformer whose InputTemplate references a placeholder InputPathsMap does not declare, instead of refusing it with ValidationException

> _Note: this issue was identified against the code base at commit `d3131d60bceeaea824d338672e432581131c6d9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
One root cause (Cause 1 of 1). Open issue 20261029T182228Z-events-sqs-eventbridge-puttargets-accepts-conflicting-invalid-input-forms-the.md lists this check id under its Cause 7, but it is bound to it by a `_collect#14|Timeout` replay key (SQS delivery was inert, so the check never got this far) and the suite does NOT mark this failure as owned: delivery and the empty-string substitution of an unresolvable input path now both pass, and the check fails later, on the configuration-time negative. That is a distinct, still-unimplemented validation, so it is filed here and that issue is named rather than assumed to cover it.

Cause 1 of 1: PutTargets accepts an InputTransformer whose InputTemplate contains a placeholder that InputPathsMap does not declare.
AWS: an InputTemplate may only use placeholders declared as keys of InputPathsMap — Smithy doc for InputTransformer.InputTemplate: "Input template where you specify placeholders that will be filled with the values of the keys from InputPathsMap ... Enclose each InputPathsMaps value in brackets: <value>". A target whose template names an undeclared placeholder is refused at configuration time: HTTP 400, error code `ValidationException`, message `InputTemplate for target <TargetId> contains invalid placeholder <placeholder>.` (recorded behavior; the Smithy model declares only ConcurrentModificationException, InternalException, LimitExceededException, ManagedRuleException and ResourceNotFoundException for PutTargets, so this refusal travels on the service-common ValidationException channel). In that message the fixed text is `InputTemplate for target `, ` contains invalid placeholder ` and the trailing `.`; the two variable parts are the target's `Id` from the request (`t1` in the recorded run) and the undeclared placeholder name taken verbatim from the InputTemplate (`missing` in the recorded run). No target is installed when the request is refused.
Emulator: the call returns HTTP 200 with FailedEntryCount=0 / FailedEntries=[] and raises nothing, so the check's `raise AssertionError("an InputTemplate placeholder that InputPathsMap does not declare must be refused")` is reached. The handler validates only duplicate target Ids and ARN-valued members; it never inspects InputTransformer at all.
Why: a caller that deploys a typo'd placeholder gets a success back and a target that AWS would never have created. The misconfiguration then surfaces only at delivery time (or not at all), and tooling that relies on PutTargets to reject a bad template — CloudFormation/IaC rollbacks, deploy gates, tests asserting ValidationException — proceeds as if the configuration were valid.
Fix: cross-member validation of InputTransformer in the EventBridge PutTargets handler (same whole-request refusal path as the existing duplicate-Id / bad-ARN checks): every `<name>` placeholder occurring in InputTemplate must be a key of InputPathsMap, otherwise refuse the whole call on the 400 ValidationException channel before anything is stored.
Traces: events__sqs.eventbridge-input-transformer-missing-path-not-delivered. Handler with no InputTransformer validation: services/service-com-amazonaws-eventbridge-awsevents/src/ops/put_targets.rs lines 197-219 (the only request-level refusals) and 259-274 (targets stored unchecked).

## Evidence
Failing check events__sqs.eventbridge-input-transformer-missing-path-not-delivered (mismatch):

  File "check_events__sqs_eventbridge_input_transformer_missing_path_not_delivered.py", line 131, in run
    raise AssertionError(
  AssertionError: an InputTemplate placeholder that InputPathsMap does not declare must be refused

The check calls PutTargets with Targets=[{Id: "t1", Arn: <queue arn>, InputTransformer: {InputPathsMap: {"who": "$.detail.who"}, InputTemplate: '{"who": <who>, "nope": <missing>}'}}].
Expected: a botocore ClientError with Error.Code == "ValidationException" (HTTP 400) and Error.Message == "InputTemplate for target t1 contains invalid placeholder missing." — where `t1` is the request's target Id and `missing` is the undeclared placeholder from the template.
Actual: the call succeeds (200, FailedEntryCount=0, FailedEntries=[]) and the target is installed; no error is raised.
The earlier phases of the same check pass, i.e. the rule delivers and an unresolvable input path renders as the empty string — only the configuration-time refusal is missing.

## Steps to Reproduce
1. events.put_rule(Name=<rule>, EventPattern='{"source": ["<some-source>"]}') on the default bus.
2. events.put_targets(Rule=<rule>, Targets=[{"Id": "t1", "Arn": "<sqs queue arn>", "InputTransformer": {"InputPathsMap": {"who": "$.detail.who"}, "InputTemplate": '{"who": <who>, "nope": <missing>}'}}]).
3. Observe: the emulator answers 200 with FailedEntryCount=0 and stores the target. AWS refuses with HTTP 400 ValidationException and the message "InputTemplate for target t1 contains invalid placeholder missing.", leaving the rule's targets unchanged.
4. Re-replay events__sqs.eventbridge-input-transformer-missing-path-not-delivered to confirm the fix.
