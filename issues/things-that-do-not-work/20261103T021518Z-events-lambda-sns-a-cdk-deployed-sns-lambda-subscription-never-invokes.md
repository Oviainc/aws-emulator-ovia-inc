Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission
Issue-Key: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission|_poll_until#15|Timeout|last = probe()
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# A CDK-deployed SNS->Lambda subscription never invokes the function, so the permission-boundary check never reaches its positive control

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: an SNS topic subscription to a Lambda function, provisioned by a CDK/CloudFormation stack (`AWS::SNS::Subscription` with Protocol `lambda` plus `AWS::Lambda::Permission` for the `sns.amazonaws.com` principal), never results in the function being invoked when the topic is published to. The check cannot complete and is killed at the harness cap.

AWS: Publish to a topic that has a confirmed `lambda`-protocol subscription asynchronously invokes the subscribed function with the SNS `Records` envelope, provided a resource policy on the function allows `lambda:InvokeFunction` for principal `sns.amazonaws.com` scoped to the topic ARN. With that permission absent, AWS performs no invocation at all — the publish still succeeds (Publish is modelled with no authorization error for the downstream delivery; delivery failures are out-of-band). So the negative half of this check (no permission -> no invocation) and the positive half (permission present -> invocation) must give different observable outcomes. Recorded behavior only for the delivery timing; the model states only that Publish returns a MessageId.

Emulator: neither half produces an invocation. The negative half's 40s observation window elapses with no delivery (which coincidentally satisfies its assertion), the stack is redeployed with the Lambda permission included, the same publish is issued, and the function is still never invoked — the positive control's `assert any(marker_fixed in b for b in bodies), "positive control: function was never invoked"` can never be satisfied, so the check burns its whole budget and is terminated with `Check timed out after 60.0s`.

Why: a client gets a false negative on permission enforcement. Because the no-permission and with-permission paths are indistinguishable (both silently deliver nothing), the emulator cannot be used to validate an SNS->Lambda wiring or its IAM boundary: a stack that is correctly permissioned looks exactly like one that is not, and the absence of delivery is read as the permission working rather than as delivery being unimplemented. Any application fanning out to Lambda via SNS gets no events.

Fix: the SNS subscription delivery path for the `lambda` protocol — Publish must dispatch to the subscribed function (and must gate that dispatch on the function's resource policy grant for `sns.amazonaws.com` / the topic ARN, so the two halves diverge). This is the delivery dispatcher reached from the Publish handler, not Publish's own response, which is correct.

Hypothesis: the timeout rather than a clean assertion failure is because the positive control waits for a delivery that never arrives; an alternative contributor would be the CDK/CloudFormation stack deploy itself not settling, but the stack's outputs (TopicArn) are consumed before the publish, so the deploy does complete.

Related (does NOT cover this check): 20261029T182228Z-events-lambda-sns-eventbridge-publishes-to-sns-topics-with-no-topic.md and 20261025T002357Z-events-lambda-sns-eventbridge-sns-targets-never-publish-and-an-sns.md own sibling checks in this service, including `cdk-deploy-sns-lambda-subscription-notification`; this check was not marked as owned by either, so its replay key is unclaimed and it is filed here.

Traces: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission.

## Evidence
Replay of simparity-behavioral-aws-v1, check `cdk-deploy-sns-lambda-subscription-missing-permission` (verdict: mismatch).

Error: `Check timed out after 60.0s`.

The check (GROUP `permission-boundary`) does the following, per its replayed source:
  1. Deploys the CDK app at `deployments/sns_lambda_cdk` with context `include_permission=false`, publishes `marker-broken-<n>` to the stack's TopicArn, and collects from the sink queue for 40s; asserts the marker never appears.
  2. Redeploys the same app with `include_permission=true`, publishes `marker-fixed-<n>`, collects for up to 90s, and asserts `any(marker_fixed in b for b in bodies)` with the message "positive control: function was never invoked".

Step 2 never observes the marker, so the check runs past the 60s harness cap and is terminated. Expected: step 1 yields no delivery and step 2 yields a body containing the published marker, making the two halves distinguishable. Actual: both halves yield no delivery; the check is killed before it can report the positive control's assertion.

The marker strings, queue names and topic ARN are per-run values and are not part of the requirement; the requirement is that a permissioned `lambda`-protocol subscription delivers the published message to the function and an unpermissioned one does not. `require_tools("cdk")` at the top of the check did not skip (the suite reports Skipped: 0), so the CDK toolchain was present and this is not a missing-tool failure.

## Steps to Reproduce
1. Deploy a CDK/CloudFormation stack containing an SNS topic, a Lambda function that forwards its event to an SQS queue, an `AWS::SNS::Subscription` with Protocol `lambda` pointing at the function, and an `AWS::Lambda::Permission` granting `lambda:InvokeFunction` to principal `sns.amazonaws.com` with SourceArn set to the topic ARN.
2. Publish a unique marker message to the topic ARN from the stack outputs.
3. Poll the sink queue with ReceiveMessage for up to 90s for a body containing the marker.
4. Observe no message ever arrives — the function is never invoked. On AWS the marker arrives within seconds.
5. Redeploy the identical stack WITHOUT the `AWS::Lambda::Permission` and repeat: the observable outcome is the same, so the permission boundary is unobservable.
6. Or replay the behavioral check `cdk-deploy-sns-lambda-subscription-missing-permission`, which times out at the 60s cap on its positive control.
