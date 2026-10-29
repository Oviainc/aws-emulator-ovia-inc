Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.eventbridge-rule-lambda-target-invocation-positive dynamodb__events__lambda.eventbridge-custom-bus-rule-lambda-target dynamodb__events__lambda.eventbridge-rule-disabled-no-invocation-negative dynamodb__events__lambda.eventbridge-rule-event-pattern-filter-lambda-invocation dynamodb__events__lambda.eventbridge-rule-input-transformer-lambda-payload dynamodb__events__lambda.eventbridge-rule-lambda-missing-invoke-permission-negative dynamodb__events__lambda.eventbridge-rule-target-arn-mismatch-negative dynamodb__events__lambda.eventbridge-rule-triggers-lambda-dynamodb-transactwrite-positive dynamodb__events__lambda.eventbridge-rule-triggers-lambda-writes-dynamodb-positive dynamodb__events__lambda.eventbridge-scheduled-rule-invokes-lambda dynamodb__events__lambda.eventbridge-triggered-lambda-reads-dynamodb-item-positive dynamodb__events__lambda.events-rule-role-assume-role-positive dynamodb__events__lambda.cli-eventbridge-put-events-dynamodb-pipeline-workflow dynamodb__events__lambda.cfn-eventbridge-rule-lambda-target-deployment dynamodb__events__lambda.ddb-stream-esm-destination-missing-permission-rejected dynamodb__events__lambda.ddb-stream-esm-filter-criteria-json-positive dynamodb__events__lambda.ddb-stream-esm-maximum-retry-attempts-config-positive dynamodb__events__lambda.ddb-stream-esm-starting-position-trim-horizon-positive dynamodb__events__lambda.cfn-eventbridge-custom-bus-rule-deployment dynamodb__events__lambda.cli-lambda-role-policy-missing-dynamodb-negative-workflow dynamodb__events__lambda.lambda-role-missing-trust-statement-negative
Issue-Key: dynamodb__events__lambda.eventbridge-rule-lambda-target-invocation-positive|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.eventbridge-custom-bus-rule-lambda-target|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.eventbridge-rule-disabled-no-invocation-negative|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.eventbridge-rule-event-pattern-filter-lambda-invocation|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.eventbridge-rule-input-transformer-lambda-payload|run#17|Timeout|time.sleep(2.0)
Issue-Key: dynamodb__events__lambda.eventbridge-rule-lambda-missing-invoke-permission-negative|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.eventbridge-rule-target-arn-mismatch-negative|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.eventbridge-rule-triggers-lambda-dynamodb-transactwrite-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.eventbridge-rule-triggers-lambda-writes-dynamodb-positive|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.eventbridge-scheduled-rule-invokes-lambda|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.eventbridge-triggered-lambda-reads-dynamodb-item-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.events-rule-role-assume-role-positive|_poll_until#35|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.cli-eventbridge-put-events-dynamodb-pipeline-workflow|_poll_for_item#26|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.cfn-eventbridge-rule-lambda-target-deployment|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.ddb-stream-esm-destination-missing-permission-rejected|_receive_one#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Key: dynamodb__events__lambda.ddb-stream-esm-filter-criteria-json-positive|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.ddb-stream-esm-maximum-retry-attempts-config-positive|_poll_stable#10|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.ddb-stream-esm-starting-position-trim-horizon-positive|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.cfn-eventbridge-custom-bus-rule-deployment|_deploy_cfn#1|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: dynamodb__events__lambda.cli-lambda-role-policy-missing-dynamodb-negative-workflow|_run_shell#16|AssertionError|raise AssertionError(
Issue-Key: dynamodb__events__lambda.lambda-role-missing-trust-statement-negative|_error#6|AssertionError|raise AssertionError(f"expected rejection but call succeeded and returned {result!r}")
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# EventBridge rules never invoke their Lambda targets, DDB-stream ESMs never deliver, custom-bus stack rolls back, CreateFunction skips role trust (5 causes, 21 checks)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Five net-new root causes in the dynamodb__events__lambda group. Each needs its own fix.

Cause 1 of 5: an EventBridge rule never invokes its Lambda target.
AWS: `PutEvents` on the default or a custom bus matches the rule's event pattern and invokes every enabled target; a DISABLED rule and a non-matching pattern invoke nothing; an input transformer reshapes the payload the target receives; a scheduled rule fires on its schedule.
Emulator: no target is ever invoked. Each check polls for the effect and ends with `Check timed out after 60.0s`, so neither delivery nor the documented non-delivery (disabled rule, pattern miss, target-ARN mismatch, missing invoke permission) can be observed.
Why: event-driven fan-out is silently inert — a producer's PutEvents is accepted and nothing downstream runs, and every negative case is indistinguishable from the positive one.
Fix: the eventbridge rule-matching/target-dispatch path (and the lambda invoke it must perform).
Traces: dynamodb__events__lambda.eventbridge-rule-lambda-target-invocation-positive, .eventbridge-custom-bus-rule-lambda-target, .eventbridge-rule-disabled-no-invocation-negative, .eventbridge-rule-event-pattern-filter-lambda-invocation, .eventbridge-rule-input-transformer-lambda-payload, .eventbridge-rule-lambda-missing-invoke-permission-negative, .eventbridge-rule-target-arn-mismatch-negative, .eventbridge-rule-triggers-lambda-dynamodb-transactwrite-positive, .eventbridge-rule-triggers-lambda-writes-dynamodb-positive, .eventbridge-scheduled-rule-invokes-lambda, .eventbridge-triggered-lambda-reads-dynamodb-item-positive, .events-rule-role-assume-role-positive, .cli-eventbridge-put-events-dynamodb-pipeline-workflow, .cfn-eventbridge-rule-lambda-target-deployment

Cause 2 of 5: DynamoDB-stream event source mappings never deliver.
AWS: an enabled ESM polls the table's stream and invokes the function, honouring BatchSize, FilterCriteria, MaximumRetryAttempts, StartingPosition=TRIM_HORIZON and the on-failure destination.
Emulator: no delivery; the checks poll until `Check timed out after 60.0s`.
Why: every stream-triggered workload is inert, and the ESM configuration knobs have no observable effect.
Fix: the lambda event-source-mapping poller for DynamoDB streams.
Traces: dynamodb__events__lambda.ddb-stream-esm-destination-missing-permission-rejected, .ddb-stream-esm-filter-criteria-json-positive, .ddb-stream-esm-maximum-retry-attempts-config-positive, .ddb-stream-esm-starting-position-trim-horizon-positive

Cause 3 of 5: a stack declaring a custom event bus and a rule rolls back.
AWS: the stack reaches `CREATE_COMPLETE` with the bus, rule and target created.
Emulator: the stack ends in `ROLLBACK_COMPLETE`, so none of the resources exist.
Why: the template cannot be deployed at all; the caller gets a failed stack for a valid template.
Fix: the cloudformation provisioners for AWS::Events::EventBus / AWS::Events::Rule (read the stack events for the failing resource).
Traces: dynamodb__events__lambda.cfn-eventbridge-custom-bus-rule-deployment

Cause 4 of 5: the CLI workflow for a Lambda role missing DynamoDB permissions exits 254.
AWS: each CLI step either succeeds or fails with a service error the CLI renders as a 255-class error naming the AWS error code; the recorded run completes the workflow.
Emulator: the command fails with `rc=254`, i.e. the CLI could not complete the call against the emulator.
Why: the CLI path for this workflow is unusable, so the authorization outcome it exists to show is never reached.
Fix: the operations this workflow drives (the handler that answers the failing CLI call) — see the command transcript in the check output.
Traces: dynamodb__events__lambda.cli-lambda-role-policy-missing-dynamodb-negative-workflow

Cause 5 of 5: a function is created with an execution role Lambda cannot assume.
AWS: `CreateFunction` validates the role's trust policy; a role that does not trust `lambda.amazonaws.com` is refused with `InvalidParameterValueException` (HTTP 400) stating the role cannot be assumed by Lambda.
Emulator: the call SUCCEEDS (returns None/no error).
Why: a function is created that could never run in AWS, so the misconfiguration is discovered only much later — and the negative path is untested.
Fix: CreateFunction's role validation (trust-policy principal check against iam state).
Traces: dynamodb__events__lambda.lambda-role-missing-trust-statement-negative

## Evidence
- Causes 1 and 2 (18 checks): Error: Check timed out after 60.0s.
- Cause 3: AssertionError: Stack <stack name> ended in ROLLBACK_COMPLETE.
- Cause 4: AssertionError: Command failed (rc=254).
- Cause 5: AssertionError: expected rejection but call succeeded and returned None.

## Steps to Reproduce
Boot the emulator and replay the 21 checks listed in trace_ids. Cause 1 is reproduced by PutRule + PutTargets(lambda) + PutEvents and then polling for the target's effect; cause 2 by CreateEventSourceMapping over a table stream and writing an item; cause 3 by deploying a template with AWS::Events::EventBus + AWS::Events::Rule; cause 5 by CreateFunction with a role whose trust policy omits lambda.amazonaws.com.
