Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.eventbridge-fanout-single-event-invokes-lambda-and-sfn events__lambda__s3__stepfunctions.eventbridge-fanout-partial-failure-other-target-still-invoked events__lambda__s3__stepfunctions.eventbridge-rule-multiple-targets-independent-delivery-tracking events__lambda__s3__stepfunctions.s3-upload-lambda-putevents-blocked-without-events-permission events__lambda__s3__stepfunctions.cfn-deploy-lambda-role-missing-events-permission-negative events__lambda__s3__stepfunctions.cfn-deploy-eventbridge-fanout-lambda-sfn-stack events__lambda__s3__stepfunctions.cfn-deploy-eventbridge-target-role-missing-trust-negative
Issue-Key: events__lambda__s3__stepfunctions.eventbridge-fanout-single-event-invokes-lambda-and-sfn|_fire_until#17|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3__stepfunctions.eventbridge-fanout-partial-failure-other-target-still-invoked|_fire_until#26|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3__stepfunctions.eventbridge-rule-multiple-targets-independent-delivery-tracking|_fire_until#17|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3__stepfunctions.s3-upload-lambda-putevents-blocked-without-events-permission|_poll_messages#24|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__s3__stepfunctions.cfn-deploy-lambda-role-missing-events-permission-negative|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__s3__stepfunctions.cfn-deploy-eventbridge-fanout-lambda-sfn-stack|_deploy_cfn#5|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: events__lambda__s3__stepfunctions.cfn-deploy-eventbridge-target-role-missing-trust-negative|_deploy_cfn#4|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# events/lambda/s3/sfn: EventBridge fan-out never invokes any target, S3→Lambda is inert, and CFN stacks declaring an EventBridge rule with Lambda/SFN targets roll back (3 causes)

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Three root causes. 20261023T114649Z-sfn-step-functions-never-executes-task-states-every-execution.md, 20261024T043150Z-sfn-5-of-the-6-interpreter-causes-remain-open.md, 20261028T035442Z-events-lambda-s3-sfn-an-s3-objectcreated-upload.md and 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md own other checks in this service; the checks below were not bound to them.

Cause 1 of 3: a single matching event does not fan out to a rule's multiple targets.
AWS: one PutEvents entry matching a rule with several targets invokes every target exactly once and independently: the Lambda target runs and the Step Functions target starts an execution with the envelope as input. A target whose invocation fails does not prevent the sibling targets from being invoked, and each target's success/failure is tracked separately (per-target dead-lettering and FailedInvocations).
Emulator: no target is ever invoked. Every fan-out check ends "Check timed out after 60.0s", so partial-failure isolation and per-target tracking cannot be observed.
Why: fan-out is the primary reason to use EventBridge; a client building it gets nothing, and cannot tell a failing target from a dead dispatcher.
Fix: the EventBridge rule dispatcher's per-target invocation loop and its per-target failure accounting (Lambda and Step Functions invokers).
Traces: events__lambda__s3__sfn.eventbridge-fanout-single-event-invokes-lambda-and-sfn, events__lambda__s3__sfn.eventbridge-fanout-partial-failure-other-target-still-invoked, events__lambda__s3__sfn.eventbridge-rule-multiple-targets-independent-delivery-tracking

Cause 2 of 3: an S3 ObjectCreated upload never invokes the Lambda that would call PutEvents, so the denied-PutEvents leg is ungraded.
AWS: PutObject into a bucket with a LambdaFunctionConfiguration invokes the function once; a function whose execution role lacks events:PutEvents then fails that call with AccessDeniedException, which is observable in the function's result/log. The positive control establishes that the upload did invoke the function at all.
Emulator: the function is never invoked; the check ends "Check timed out after 60.0s".
Why: the S3→Lambda→EventBridge chain cannot be exercised and a missing events:PutEvents permission is indistinguishable from an uninvoked function.
Fix: the S3 bucket-notification Lambda dispatcher (see also the S3 notification cause filed for events__lambda__s3).
Traces: events__lambda__s3__sfn.s3-upload-lambda-putevents-blocked-without-events-permission, events__lambda__s3__sfn.cfn-deploy-lambda-role-missing-events-permission-negative

Cause 3 of 3: a CloudFormation stack declaring an EventBridge rule with Lambda and Step Functions targets (and a target execution role) rolls back.
AWS: the stack reaches CREATE_COMPLETE; a target role whose trust policy does not name events.amazonaws.com is still created (trust is enforced at invocation time, not at create time).
Emulator: the stack ends ROLLBACK_COMPLETE. "AssertionError: Stack <name> ended in ROLLBACK_COMPLETE".
Why: the IaC form of an EventBridge fan-out cannot be deployed, and the bare rollback hides which resource failed, so a client cannot even tell whether the trust policy was the reason.
Fix: the CloudFormation providers for AWS::Events::Rule (with Targets and RoleArn), AWS::StepFunctions::StateMachine and AWS::IAM::Role, plus the stack-event surface that should name the failing resource.
Traces: events__lambda__s3__sfn.cfn-deploy-eventbridge-fanout-lambda-sfn-stack, events__lambda__s3__sfn.cfn-deploy-eventbridge-target-role-missing-trust-negative

## Evidence
Causes 1 and 2: 5 checks end "Error: Check timed out after 60.0s" with neither the Lambda target nor the Step Functions target invoked after a matching PutEvents, and the S3-triggered Lambda never invoked after PutObject.
Cause 3: "AssertionError: Stack sim2real-xsvc-stk-8b15f9268185 ended in ROLLBACK_COMPLETE" and "AssertionError: Stack sim2real-xsvc-stk-464160a19ed7 ended in ROLLBACK_COMPLETE" — expected CREATE_COMPLETE.

## Steps to Reproduce
(1) PutRule + PutTargets with both a Lambda ARN and a state-machine ARN (plus RoleArn), AddPermission for the function, PutEvents a matching entry; poll both targets — neither is invoked. (2) PutBucketNotificationConfiguration with a Lambda destination and PutObject — the function never runs. (3) CreateStack with AWS::Events::Rule carrying a Lambda target and a StateMachine target plus AWS::IAM::Role — ROLLBACK_COMPLETE.
