Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected|_invoke_until_logged#17|Timeout|resp = awslambda.invoke(
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke|_invoke_until_logged#16|Timeout|resp = awslambda.invoke(
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission|_invoke_until_logged#16|Timeout|resp = awslambda.invoke(
Issue-Commit: 7067ba4961166338ae9e8f72da8985e44b35a5d9

# cloudformation__iam__lambda__logs: a Lambda function's first invoke never creates its implicit /aws/lambda/<fn> log group and no invocation record is ever delivered, so every implicit-log-group check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `7067ba4961166338ae9e8f72da8985e44b35a5d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: Lambda invocation logging into the implicit `/aws/lambda/<function-name>` log group never happens ===

AWS: a Lambda function deployed by a stack that declares only the function and its execution role leaves its log group to Lambda. After CREATE_COMPLETE and before any invoke, CloudWatch Logs has no group named `/aws/lambda/<function-name>`; on the first invoke the Lambda service creates that group with service defaults (no `retentionInDays`, no `kmsKeyId`, `logGroupClass` = `STANDARD`), creates a stream named `YYYY/MM/DD/[$LATEST]<32 hex>`, and delivers into it the function's own stdout lines plus the platform `START RequestId: <id>` / `REPORT RequestId: <id>` lines for that invocation. The group and its events are then readable through logs:DescribeLogGroups (prefix listing), logs:FilterLogEvents and logs:GetLogEvents, and the group survives deletion of the stack that never declared it. When the execution role lacks `logs:CreateLogGroup`, the invoke still succeeds (200, no FunctionError) but the group is never created, and once the group is created out of band the same role's `logs:CreateLogStream`/`logs:PutLogEvents` grant makes its lines land in it.
  Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328): FilterLogEvents -> FilterLogEventsResponse.events, errors InvalidParameterException|ResourceNotFoundException (400) | ServiceUnavailableException (500); DescribeLogGroups -> DescribeLogGroupsResponse.logGroups, paginated on nextToken. The lazy-creation rule itself is not in any model: recorded behavior only — the recordings show the group absent after CREATE_COMPLETE and present with the marker line after the first invoke.

Emulator: no invocation record ever reaches `/aws/lambda/<function-name>`. All three checks drive the same loop — invoke with a fresh marker, then poll FilterLogEvents(logGroupName='/aws/lambda/<fn>', filterPattern='"<marker>"') for a `SIM2REAL_MARKER <marker>` line — and that line never arrives, so each run is terminated by the harness with `Error: Check timed out after 60.0s` without ever reaching its later assertions (group defaults, stream naming, START/REPORT lines, survival across stack delete, the NAME_CONFLICT_VALIDATION rejection of a declared group at the implicit name, and the no-CreateLogGroup negative). The polling call itself does not refuse: FilterLogEvents on the absent group is answered (or answered ResourceNotFoundException, which the check treats as "no events yet"); what never changes is the state — the group is never created and no event is ever emitted.

Why: a client that deploys a function and then reads its logs — the normal way to confirm an invoke ran, to collect a handler's output, or to assert that a least-privilege role could not create its group — waits forever and then fails on a timeout rather than on a response. Log-driven debugging, log-based assertions in deploy pipelines, and any negative test that distinguishes "group absent because the role lacks logs:CreateLogGroup" from "group present" are all unusable, because both cases look identical: nothing is ever there.

Fix: the Lambda invocation log emitter and its CloudWatch Logs destination wiring — on invoke, resolve the function's log group (`/aws/lambda/<function-name>` when no LoggingConfig overrides it), create it lazily with service defaults when the execution role permits `logs:CreateLogGroup` (and leave it absent, without failing the invoke, when it does not), create the per-sandbox stream with Lambda's documented name shape, and put the handler's stdout plus the START/REPORT platform lines for that request id — plus the CloudWatch Logs state that must then serve them to DescribeLogGroups/FilterLogEvents/GetLogEvents.

Related but NOT covering: open issue 20261029T182816Z-cloudformation-iam-lambda-logs-an-aws-lambda-function.md describes a neighbouring pair of causes (a CREATE_COMPLETE stack whose function does not exist, and the CFN-DECLARED log group receiving no logs) and its replay keys do not carry these three checks' current failure signature (the WaiterError it keys on no longer occurs; these three now hang on log delivery).

Traces: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke, cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission, cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected.

## Evidence
Expected (AWS): after `cfn create-stack` with a template declaring an AWS::IAM::Role + AWS::Lambda::Function reaches CREATE_COMPLETE, `logs describe-log-groups --log-group-name-prefix /aws/lambda/<fn>` returns no group; after one `lambda invoke` whose handler prints `SIM2REAL_MARKER <marker>`, `logs filter-log-events --log-group-name /aws/lambda/<fn> --filter-pattern '"<marker>"'` returns that event, DescribeLogGroups reports the group with `logGroupClass=STANDARD` and with `retentionInDays` and `kmsKeyId` absent, the event's `logStreamName` equals the `context.log_stream_name` the handler returned and matches `^\d{4}/\d{2}/\d{2}/\[\$LATEST\][0-9a-f]{32}$`, and GetLogEvents on that stream contains `START RequestId: <request-id>` and `REPORT RequestId: <request-id>` (the marker, request id, function name and stream suffix are per-run values filling that template, not requirements).

Actual (emulator): the marker line never appears. All three checks are cut off by the harness with the replay detail `Error: Check timed out after 60.0s`, every one of them inside the invoke-then-poll loop `_invoke_until_logged(...)` that polls FilterLogEvents for `SIM2REAL_MARKER <marker>`:
  - deploy-logs-2-implicit-log-group-created-on-first-invoke (verdict: mismatch) — `Error: Check timed out after 60.0s`; the unsatisfied condition is the arrival of the marker in `/aws/lambda/<fn>` after the first invoke.
  - deploy-logs-2-implicit-log-group-denied-without-create-permission (verdict: mismatch) — `Error: Check timed out after 60.0s`; the control function (role WITH AWSLambdaBasicExecutionRole) never delivers its barrier line, so the no-CreateLogGroup assertion is never reached.
  - deploy-logs-2-declared-log-group-collides-with-implicit-rejected (verdict: mismatch) — `Error: Check timed out after 60.0s`; the pre-step stack's function never populates `/aws/lambda/<fn>`, so the collision deploy that must fail validation is never attempted.
No HTTP status or error code diverges on the polling call itself; the divergence is that the log group and its events are never created, so the awaited state never arrives.

## Steps to Reproduce
1. cloudformation CreateStack with Capabilities=[CAPABILITY_IAM] and a template whose only resources are an AWS::IAM::Role trusting lambda.amazonaws.com with the AWSLambdaBasicExecutionRole managed policy, and an AWS::Lambda::Function (runtime python3.12, handler index.handler, inline ZipFile code that prints `SIM2REAL_MARKER <marker from the event>` and returns context.log_group_name / context.log_stream_name).
2. Poll DescribeStacks until CREATE_COMPLETE, then wait on the lambda FunctionActiveV2 waiter for the function name.
3. logs DescribeLogGroups with logGroupNamePrefix=/aws/lambda/<fn> — expected: no such group yet.
4. lambda Invoke (InvocationType=RequestResponse) with payload {"marker": "<marker>"}; assert no FunctionError.
5. Poll logs FilterLogEvents(logGroupName='/aws/lambda/<fn>', filterPattern='"<marker>"'). Expected: within seconds an event whose message contains `SIM2REAL_MARKER <marker>`, in a stream matching `YYYY/MM/DD/[$LATEST]<32 hex>`, in a group reported with logGroupClass=STANDARD and no retentionInDays/kmsKeyId; GetLogEvents on that stream also shows `START RequestId: <id>` and `REPORT RequestId: <id>`. Observed: the group stays absent and no event ever arrives — the poll runs until the 60 s harness cap.
6. Negative variant: repeat with a second function whose role grants only logs:CreateLogStream + logs:PutLogEvents on `arn:aws:logs:<region>:<account>:log-group:/aws/lambda/<fn>:*` (no logs:CreateLogGroup). Expected: invokes succeed, `/aws/lambda/<fn>` stays absent, and after an out-of-band logs CreateLogGroup the same role's lines land in it. Observed: indistinguishable from the control — nothing is ever logged for either function.
