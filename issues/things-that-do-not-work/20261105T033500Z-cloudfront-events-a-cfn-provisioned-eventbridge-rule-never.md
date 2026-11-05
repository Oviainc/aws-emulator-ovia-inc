Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.deploy-cfn-missing-lambda-permission-blocks-cloudfront-invocation
Issue-Key: cloudfront__events.deploy-cfn-missing-lambda-permission-blocks-cloudfront-invocation|_drain_marker#23|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=3)
Issue-Commit: 8de6682f074d8bd25eca3d1022f82b7ad69da0d3

# cloudfront__events: a CFN-provisioned EventBridge rule never invokes its Lambda target even after lambda:AddPermission closes the permission gap, so the negative/positive-control check never reaches a verdict

> _Note: this issue was identified against the code base at commit `8de6682f074d8bd25eca3d1022f82b7ad69da0d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: an EventBridge rule provisioned by a CloudFormation stack (AWS::Events::Rule with a Lambda target and no AWS::Lambda::Permission) is never made to invoke its Lambda target after the missing invoke permission is granted at runtime with lambda AddPermission, so the check's positive control never observes any target effect and the check exhausts the harness budget without a verdict.

AWS: events PutEvents is accepted (Smithy: PutEvents input Entries @length 1..10; output FailedEntryCount default 0 plus one PutEventsResultEntry carrying EventId per accepted entry; only InternalException/500 is modeled), and target delivery itself is not modeled — recorded/documented behavior only. On AWS, every entry whose Source, DetailType and Detail match a rule's EventPattern is delivered to each of that rule's targets; for a Lambda target EventBridge calls lambda:InvokeFunction asynchronously and is authorized solely by the function's resource-based policy. With no statement permitting Principal events.amazonaws.com for SourceArn = the rule's ARN, the invocation is refused and nothing runs (the correct negative). Once lambda AddPermission adds exactly that statement for the SAME rule ARN and function (Smithy: lambda AddPermission input Action, Principal, SourceArn; 201 with the created Statement), a subsequent matching PutEvents entry IS delivered and the function runs — here the function writes the received event to an SQS queue, so the event body becomes receivable within seconds.

Emulator: the stack deploys and both resources exist (lambda GetFunction and events DescribeRule both answer 200 for the stack's outputs), PutEvents is accepted without a ClientError, and lambda AddPermission is accepted — but no invocation ever happens. The function never runs for either phase: the absence poll in the negative phase sees nothing (so the negative assertion is satisfied only vacuously), and the positive control re-sends the matching entry every 20 s for a 120 s window and still receives no message, so the check is killed by the 60 s harness cap with "Check timed out after 60.0s" rather than producing a pass or a mismatch.

Why: a client cannot distinguish "blocked because the resource policy is missing" from "never delivered at all". Every IAM/resource-policy negative test against EventBridge→Lambda passes for the wrong reason, and every real pipeline (rule → Lambda → downstream SQS/CloudFront call) produces nothing, even though PutEvents reports the entry as accepted with FailedEntryCount 0. Granting the permission has no observable effect, so there is no way to validate a permission fix against the emulator.

Fix: the EventBridge rule-match → target dispatch path (the events PutEvents handler and its rule/target dispatcher) plus the Lambda-target invocation path that must consult the target function's resource-based policy (principal events.amazonaws.com, SourceArn = rule ARN) and then actually run the function; the permission lookup must be re-evaluated per delivery so a policy added after the rule was created takes effect.

Related open issues that do NOT carry this check's replay key and therefore do not cover it: 20261102T091831Z-cloudfront-events-a-matched-rule-never-invokes-its.md, 20261104T051153Z-cloudfront-events-a-matching-putevents-entry-never-invokes.md, 20261024T182555Z-cloudfront-events-a-matched-eventbridge-rule-never-produces.md and 20261029T175823Z-cloudfront-events-13-more-cloudfront-eventbridge-checks-fail.md. This check's distinct premise is the CloudFormation-provisioned rule/target pair whose permission gap is closed afterwards by a direct lambda AddPermission call.

Traces: cloudfront__events.deploy-cfn-missing-lambda-permission-blocks-cloudfront-invocation.

## Evidence
Expected (AWS): phase 1 — with no AWS::Lambda::Permission in the stack, the marker event is not delivered (no SQS message), which is the intended negative; phase 2 — after lambda AddPermission(FunctionName=<stack function>, StatementId=AllowEventBridgeInvokeAfterFix, Action=lambda:InvokeFunction, Principal=events.amazonaws.com, SourceArn=<rule ARN from the stack output>), a matching PutEvents entry (Source "sim2real.cloudfront.audit", DetailType "CloudFront API Call (Simulated CloudTrail)", Detail.eventSource "cloudfront.amazonaws.com", Detail.marker <per-run marker>) causes the function to run and SQS ReceiveMessage returns a body whose detail.marker equals the marker just sent.

Actual (emulator): stack reaches CREATE_COMPLETE and exports QueueUrl/FunctionName/RuleName/RuleArn; lambda GetFunction and events DescribeRule succeed; PutEvents is accepted (no ClientError, no failed entry); lambda AddPermission is accepted. No SQS message ever arrives in either phase — the positive control polls for 120 s re-sending the entry every 20 s and sees nothing, and the run ends with "Error: Check timed out after 60.0s" (the harness cap), so the check never reaches a verdict. No error code or message is emitted by any call; the divergence is a silently dropped target delivery, not a refusal.

## Steps to Reproduce
1. CreateStack with a template containing: AWS::SQS::Queue, an AWS::IAM::Role trusted by lambda.amazonaws.com with sqs:SendMessage on the queue, an AWS::Lambda::Function (python3.12, inline ZipFile) whose handler sends the received event to QUEUE_URL, and an AWS::Events::Rule whose EventPattern is {source: ["sim2real.cloudfront.audit"], detail-type: ["CloudFront API Call (Simulated CloudTrail)"], detail: {eventSource: ["cloudfront.amazonaws.com"]}} with a single target = the function ARN. Deliberately omit AWS::Lambda::Permission. Output QueueUrl, FunctionName, RuleName and RuleArn.
2. Wait for CREATE_COMPLETE and confirm lambda GetFunction(FunctionName) and events DescribeRule(Name) both succeed.
3. events PutEvents with one entry matching the pattern and a unique detail.marker; poll sqs ReceiveMessage for ~45 s — no message is expected (negative phase).
4. lambda AddPermission(FunctionName=<function>, StatementId=AllowEventBridgeInvokeAfterFix, Action=lambda:InvokeFunction, Principal=events.amazonaws.com, SourceArn=<RuleArn>).
5. events PutEvents with a new unique detail.marker and poll sqs ReceiveMessage, re-sending every 20 s for 120 s. Expected: a body with detail.marker == the new marker. Observed: nothing is ever delivered and the check is cut off by the 60 s harness cap.
