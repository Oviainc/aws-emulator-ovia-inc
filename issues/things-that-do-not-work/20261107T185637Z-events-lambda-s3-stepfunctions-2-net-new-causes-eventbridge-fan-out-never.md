Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.eventbridge-fanout-single-event-invokes-lambda-and-sfn events__lambda__s3__stepfunctions.eventbridge-fanout-partial-failure-other-target-still-invoked events__lambda__s3__stepfunctions.eventbridge-rule-multiple-targets-independent-delivery-tracking events__lambda__s3__stepfunctions.cfn-deploy-eventbridge-fanout-lambda-sfn-stack events__lambda__s3__stepfunctions.cfn-deploy-lambda-role-missing-events-permission-negative events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume
Issue-Key: events__lambda__s3__stepfunctions.eventbridge-fanout-single-event-invokes-lambda-and-sfn|run#18|AssertionError|assert obj is not None, "Lambda target was never invoked by the fan-out event"
Issue-Key: events__lambda__s3__stepfunctions.eventbridge-fanout-partial-failure-other-target-still-invoked|run#27|AssertionError|assert obj2 is not None, "Lambda still was not invoked after granting the invoke permission"
Issue-Key: events__lambda__s3__stepfunctions.eventbridge-rule-multiple-targets-independent-delivery-tracking|run#18|AssertionError|assert objs is not None, f"Lambda target never independently delivered all of {marker_keys}"
Issue-Key: events__lambda__s3__stepfunctions.cfn-deploy-eventbridge-fanout-lambda-sfn-stack|_poll_until#16|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: events__lambda__s3__stepfunctions.cfn-deploy-lambda-role-missing-events-permission-negative|_poll_until#17|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task|run#21|AssertionError|assert correct_final["status"] == "SUCCEEDED", f"unexpected status: {correct_final}"
Issue-Key: events__lambda__s3__stepfunctions.sfn-task-token-lambda-missing-sendtask-permission-blocks-resume|_poll_for_object#10|AssertionError|raise AssertionError(f"object {key!r} never appeared in {bucket!r} within {timeout_s}s")
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# 2 net-new causes: EventBridge fan-out never invokes the Lambda target, and Lambda-run code cannot build s3/stepfunctions clients

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Two root causes, both grounded in failing checks this report does NOT mark ALREADY FILED or OWNED. Open issues 20261104T221115Z-events-lambda-s3-stepfunctions-every-lambda-task-state.md, 20261105T081644Z-events-lambda-s3-stepfunctions-the-lambda-python-runtime.md and 20261107T015155Z-events-lambda-s3-stepfunctions-the-in-function-sdk.md own OTHER checks of this service and do not carry the replay keys below.

Cause 1 of 2: a rule's Lambda target is never invoked, even after the invoke permission is granted.
AWS: PutEvents of a matching event invokes every target of the rule independently — a Lambda target and a Step Functions target each receive the event, and one target failing does not withhold the other. Each matching event produces its own invocation, so N events put produce N markers.
Emulator: the Lambda leg never fires. The checks report "Lambda target was never invoked by the fan-out event", "Lambda still was not invoked after granting the invoke permission", "Lambda target never independently delivered all of ['marker-0-...','marker-1-...','marker-2-...']", "lambda fanout target wrote marker object not met within 90s. Last result: None" and "calibration: directly-put event reaching the target lambda via the rule not met within 90.0s. Last result: None". (The marker strings are this run's values.)
Why: EventBridge fan-out is unusable, and because delivery never happens even on the calibrated positive path, the negative checks (a target whose role lacks the permission) measure nothing.
Fix: the EventBridge target dispatcher for Lambda targets — per-target independent delivery and the function resource-policy check — and the CloudFormation path that wires AWS::Events::Rule targets.
Traces: events__lambda__s3__stepfunctions.eventbridge-fanout-single-event-invokes-lambda-and-sfn, .eventbridge-fanout-partial-failure-other-target-still-invoked, .eventbridge-rule-multiple-targets-independent-delivery-tracking, .cfn-deploy-eventbridge-fanout-lambda-sfn-stack, .cfn-deploy-lambda-role-missing-events-permission-negative.

Cause 2 of 2: the in-process Lambda runtime's bundled SDK knows only eight services, so a Task state's function cannot call S3 or Step Functions.
AWS: the managed python runtime's boto3 can build a client for any service; a Lambda task that calls s3.put_object or sfn.send_task_success/send_task_failure reaches the service and the state machine proceeds (SUCCEEDED, or the task fails with the service's own error such as AccessDenied).
Emulator: constructing the client raises before any call: the execution ends status FAILED with error 'UnknownServiceError' and cause errorMessage "Unknown service: 's3'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager" (and the same for 'stepfunctions'), raised from the bundled client.py _raise_refusal. The enumerated service list is the emulator's own and is itself the defect. The waitForTaskToken check's evidence object is therefore never written ('evidence.json' never appeared within 60.0s) and the callback never resumes.
Why: no Lambda task can do real work, and every execution fails with an SDK-construction error that AWS never produces — so a state machine's error handling, its catchers and its task-token callbacks are all exercised against the wrong error, and IAM refusals cannot be distinguished from this fault.
Fix: the in-process Lambda runtime's bundled AWS SDK shim — its service registry and the client factory that refuses unknown services — so in-function clients for s3, stepfunctions and the rest route to the emulator.
Traces: events__lambda__s3__stepfunctions.sfn-execution-role-wrong-trust-blocks-lambda-task, .sfn-task-token-lambda-missing-sendtask-permission-blocks-resume.

## Evidence
Cause 1: expected each target of the rule to be invoked independently; got no Lambda invocation — 'Lambda target was never invoked by the fan-out event', 'Lambda still was not invoked after granting the invoke permission', 'lambda fanout target wrote marker object not met within 90s. Last result: None'.
Cause 2: expected the task's S3/Step Functions call to be made (or refused by IAM); got execution status FAILED, error 'UnknownServiceError', cause "Unknown service: 's3'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager".

## Steps to Reproduce
1) Cause 1: PutRule + PutTargets with a Lambda target and a Step Functions target, AddPermission for events.amazonaws.com, PutEvents one and then three matching events, and poll for per-event markers.
2) Cause 2: StartExecution of a state machine whose Task state invokes a function that calls boto3.client('s3').put_object / boto3.client('stepfunctions').send_task_success, and read the execution's error/cause.
