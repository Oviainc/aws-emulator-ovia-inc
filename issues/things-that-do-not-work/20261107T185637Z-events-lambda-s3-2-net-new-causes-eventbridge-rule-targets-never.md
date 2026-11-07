Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3.health-ev-a-matching-event-reaches-the-lambda-and-lands-in-s3 events__lambda__s3.health-ev-aws-cli-created-rule-delivers-through-the-sdk-wired-target events__lambda__s3.health-ev-exists-filter-on-event-arn-withholds-an-incomplete-detail-at-the-rule events__lambda__s3.health-ev-permission-scoped-to-the-default-bus-rule-arn-does-not-admit-the-custom-bus-rule events__lambda__s3.health-ev-target-input-path-hands-the-function-the-detail-alone events__lambda__s3.health-ev-target-upsert-replaces-input-path-with-constant-input-for-later-deliveries events__lambda__s3.health-ev-the-export-bucket-wipe-is-idempotent-and-converges-to-empty events__lambda__s3.cfn-deploy-s3-lambda-notification
Issue-Key: events__lambda__s3.health-ev-a-matching-event-reaches-the-lambda-and-lands-in-s3|_poll_keys#7|Timeout|raise AssertionError(f"{description} not met within {deadline_s}s; keys were {last!r}")
Issue-Key: events__lambda__s3.health-ev-aws-cli-created-rule-delivers-through-the-sdk-wired-target|_poll_keys#22|Timeout|raise AssertionError(f"{description} not met within {deadline_s}s; keys were {last!r}")
Issue-Key: events__lambda__s3.health-ev-exists-filter-on-event-arn-withholds-an-incomplete-detail-at-the-rule|_poll_bodies#12|Timeout|raise AssertionError(f"{description} not met within {_DELIVERY_DEADLINE_S}s; bodies were {last!r}")
Issue-Key: events__lambda__s3.health-ev-permission-scoped-to-the-default-bus-rule-arn-does-not-admit-the-custom-bus-rule|_poll_bodies#18|Timeout|raise AssertionError(f"{description} not met within {_DELIVERY_DEADLINE_S}s; bodies were {last!r}")
Issue-Key: events__lambda__s3.health-ev-target-input-path-hands-the-function-the-detail-alone|_poll_bodies#9|Timeout|raise AssertionError(f"{description} not met within {_DELIVERY_DEADLINE_S}s; bodies were {last!r}")
Issue-Key: events__lambda__s3.health-ev-target-upsert-replaces-input-path-with-constant-input-for-later-deliveries|_poll_bodies#8|Timeout|raise AssertionError(f"{description} not met within {_DELIVERY_DEADLINE_S}s; bodies were {last!r}")
Issue-Key: events__lambda__s3.health-ev-the-export-bucket-wipe-is-idempotent-and-converges-to-empty|_poll_keys#9|Timeout|raise AssertionError(f"{description} not met within {deadline_s}s; keys were {last!r}")
Issue-Key: events__lambda__s3.cfn-deploy-s3-lambda-notification|_poll_until#7|AssertionError|raise AssertionError(
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# 2 net-new causes: EventBridge rule targets never invoke the function (every health-event check runs out the clock) and S3 notifications never invoke it either

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Two root causes, both grounded in failing checks this report does NOT mark ALREADY FILED or OWNED. Open issue 20261102T091831Z-events-lambda-s3-the-health-event-export-pipeline.md owns OTHER checks of this service and does not carry the replay keys below.

Cause 1 of 2: an EventBridge rule with a Lambda target never invokes the function, so the whole health-event export pipeline never converges.
AWS: PutEvents (or a service event) matching a rule's EventPattern invokes the rule's Lambda target within seconds, honouring InputPath/Input on the target and the function's resource policy (lambda:AddPermission with Principal events.amazonaws.com and SourceArn the rule ARN — a permission scoped to a default-bus rule ARN does not admit a custom-bus rule's deliveries). The function's side effects then appear.
Emulator: nothing is delivered, so each of these seven checks polls its effect until the suite's 180 s ceiling and ends "Error: Check timed out after 180.0s" without reaching its assertion — the delivery itself, the InputPath/Input transformation, the Exists filter at the rule, the bus-scoped permission and the bucket-wipe convergence are all unmeasurable. recorded behavior only.
Why: every EventBridge-driven architecture is inert, and because the delivery never happens, each negative control (a filter that should withhold, a permission that should not admit) is unfalsifiable: absence cannot be told from a dead pipeline.
Fix: the EventBridge rule-matching and target-invocation path for Lambda targets — pattern evaluation (including Exists on a nested detail member), InputPath/Input application, the function resource-policy check against the rule ARN and bus, and the asynchronous Invoke.
Traces: events__lambda__s3.health-ev-a-matching-event-reaches-the-lambda-and-lands-in-s3, .health-ev-aws-cli-created-rule-delivers-through-the-sdk-wired-target, .health-ev-exists-filter-on-event-arn-withholds-an-incomplete-detail-at-the-rule, .health-ev-permission-scoped-to-the-default-bus-rule-arn-does-not-admit-the-custom-bus-rule, .health-ev-target-input-path-hands-the-function-the-detail-alone, .health-ev-target-upsert-replaces-input-path-with-constant-input-for-later-deliveries, .health-ev-the-export-bucket-wipe-is-idempotent-and-converges-to-empty.

Cause 2 of 2: a CloudFormation-deployed S3 ObjectCreated -> Lambda notification never fires.
AWS: a stack that declares a bucket NotificationConfiguration with a LambdaConfiguration plus the AWS::Lambda::Permission for s3.amazonaws.com invokes the function on PutObject, and the handler's side effect appears within seconds.
Emulator: the side effect never appears — "AssertionError: effect not met within 120s. Last result: None".
Why: the most common serverless trigger does nothing after a successful stack deploy, so a client believes the wiring is correct while no event is ever processed.
Fix: the S3 notification dispatcher's Lambda destination (configuration matching and the asynchronous Invoke), and the CloudFormation path that installs the bucket's NotificationConfiguration.
Traces: events__lambda__s3.cfn-deploy-s3-lambda-notification.

## Evidence
Cause 1: expected the rule's Lambda target to be invoked (and its InputPath/Input/filter/permission semantics observable); got no delivery at all — all seven checks ended 'Error: Check timed out after 180.0s'.
Cause 2: expected the function's side effect after PutObject into the stack's bucket; got none — 'AssertionError: effect not met within 120s. Last result: None'.

## Steps to Reproduce
1) Cause 1: PutRule with an EventPattern, PutTargets a Lambda target (with and without InputPath/Input), AddPermission for events.amazonaws.com scoped to the rule ARN, PutEvents a matching event, and poll for the function's side effect.
2) Cause 2: deploy a stack declaring a bucket with a LambdaConfiguration notification plus AWS::Lambda::Permission for s3.amazonaws.com, PutObject, poll for the handler's effect.
