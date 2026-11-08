Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__lambda__sqs.change-triggered-rule-creation-is-denied-when-config-cannot-invoke-the-evaluator config__lambda__sqs.change-triggered-custom-rule-is-push-invoked-with-a-real-configuration-item
Issue-Key: config__lambda__sqs.change-triggered-rule-creation-is-denied-when-config-cannot-invoke-the-evaluator|_error_code#59|AssertionError|raise AssertionError(
Issue-Key: config__lambda__sqs.change-triggered-custom-rule-is-push-invoked-with-a-real-configuration-item|run#37|KeyError|assert discovered["relationships"] == [], (
Issue-Commit: f3f9216a884ddb496f260c89a030775b09cc5916

# config__lambda__sqs: 2 net-new causes — PutConfigRule accepts a custom rule whose evaluator Config cannot invoke, and recorded configuration items omit `relationships`

> _Note: this issue was identified against the code base at commit `f3f9216a884ddb496f260c89a030775b09cc5916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConfigRule

## Description
Two independent root causes in this scenario group; each is listed with its own failing check ids and both must be closed before this issue is submitted.

Cause 1 of 2: PutConfigRule does not pre-check that AWS Config can invoke the custom rule's Lambda evaluator.
AWS: Smithy: config PutConfigRule models InsufficientPermissionsException (client, HTTP 400) and its documentation states that for a Config Custom Lambda rule the caller must give Config permission to invoke the named function; when the function's resource policy does not allow config.amazonaws.com to invoke it, PutConfigRule is refused InsufficientPermissionsException and the rule is NOT created. The fixed part of the refusal is the error code and the explanation that Config is not authorized to invoke the function; the variable parts are the rule name and the function ARN taken from the request.
Emulator: PutConfigRule returns HTTP 200 with an empty body for exactly that request, so a change-triggered rule is created pointing at a function Config may not invoke.
Why: a hard, fail-fast configuration error is deferred into a compliance dashboard that reads green forever — the rule exists, is never evaluated, and reports no non-compliance. A client has no way to learn that its evaluator is unreachable.
Fix: the config PutConfigRule handler's validation — before storing a rule with Source.Owner=CUSTOM_LAMBDA, resolve the SourceIdentifier function and verify its resource policy permits config.amazonaws.com to invoke it, refusing InsufficientPermissionsException (400) otherwise.
Traces: config__lambda__sqs.change-triggered-rule-creation-is-denied-when-config-cannot-invoke-the-evaluator.

Cause 2 of 2: a recorded configuration item is served without the `relationships` member.
AWS: Smithy: config ConfigurationItem declares `relationships` (target RelationshipList) alongside version/accountId/arn/resourceType/resourceId/configuration/…; GetResourceConfigHistory returns `configurationItems` whose entries carry that member (an empty list when the resource has no related resources), so a client may read it unconditionally.
Emulator: the item returned by GetResourceConfigHistory for the recorded AWS::SQS::Queue has no `relationships` key at all, so a direct read of it raises KeyError in the client.
Why: client code (and the AWS SDKs' own modelled shapes) treat a modelled list member as present; omitting it turns every relationship traversal — the mechanism Config uses to express resource topology — into a crash or a silent empty result that cannot be distinguished from 'no relations'.
Fix: the config serializer/state for ConfigurationItem — emit `relationships` (empty list when there are none) on every configuration item returned by GetResourceConfigHistory and the other configuration-item reads.
Traces: config__lambda__sqs.change-triggered-custom-rule-is-push-invoked-with-a-real-configuration-item.

Related open issue that does not carry either check's replay key: 20261107T193246Z-config-4-net-new-causes-custom-lambda-and.md.

## Evidence
Cause 1 — expected: `PutConfigRule` refused with InsufficientPermissionsException, HTTP 400, because AWS Config cannot invoke the evaluator; the rule is not created. Actual: `AssertionError: PutConfigRule was expected to be refused with InsufficientPermissionsException because AWS Config cannot invoke the evaluator, and instead SUCCEEDED, returning {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', …}}}`.

Cause 2 — expected: the configuration item from `get_resource_config_history(resourceType='AWS::SQS::Queue', resourceId=<queue url>)` carries `relationships` (here an empty list). Actual: `KeyError: 'relationships'` raised at `assert discovered["relationships"] == []` on the last item of the returned history.

## Steps to Reproduce
Cause 1: create a Lambda function with NO lambda:InvokeFunction permission for config.amazonaws.com, start a configuration recorder, then call PutConfigRule with a change-triggered Config Custom Lambda rule whose SourceIdentifier is that function ARN — observe HTTP 200 instead of a 400 InsufficientPermissionsException. Cause 2: with a running allSupported recorder, create an SQS queue and call GetResourceConfigHistory for resourceType AWS::SQS::Queue — observe configuration items with no `relationships` member. Re-replay config__lambda__sqs.change-triggered-rule-creation-is-denied-when-config-cannot-invoke-the-evaluator and config__lambda__sqs.change-triggered-custom-rule-is-push-invoked-with-a-real-configuration-item.
