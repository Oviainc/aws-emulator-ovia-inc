Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.eventbridge-cloudfront-distribution-arn-resolution-in-target-input
Issue-Key: cloudfront__events.eventbridge-cloudfront-distribution-arn-resolution-in-target-input|_warm_up_role#7|Timeout|resp = lam.invoke(FunctionName=fn_name, Payload=b"{}", InvocationType="RequestResponse")
Issue-Commit: 8015bc879e5141c181811b2c40a1894f3f2e5f5b

# cloudfront__events: an EventBridge rule whose Lambda target carries an InputTransformer never delivers the rendered input, so the CloudTrail-shaped distribution-ARN resolution check hangs to its deadline

> _Note: this issue was identified against the code base at commit `8015bc879e5141c181811b2c40a1894f3f2e5f5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
One net-new root cause (Cause 1 of 1), grounded in the only failing cloudfront__events check the replay report does not already attribute to an open issue.

Cause 1 of 1: a matched event is never delivered to a Lambda target configured with an InputTransformer, so the transformed payload (here, a CloudFront distribution ARN assembled from a CloudTrail-shaped detail) never reaches the target or anything downstream of it. Failing check: cloudfront__events.eventbridge-cloudfront-distribution-arn-resolution-in-target-input.

AWS: EventBridge PutTargets accepts a target with an InputTransformer (Smithy: com.amazonaws.eventbridge#InputTransformer, members InputPathsMap -> TransformerPaths, a map of up to 100 JSON-path values, and the required InputTemplate -> TransformerInput, @length min 1 max 8192). Per the modeled documentation, each InputPathsMap key is referenced in InputTemplate enclosed in angle brackets (<value>), and the extracted values are substituted into the template to form the input actually sent to the target; the template may itself be a JSON object. When an event published with PutEvents matches the rule's EventPattern, EventBridge renders the template once per matched event and invokes each target with the rendered input — a Lambda target is invoked with the rendered JSON as its event payload, provided the function's resource policy admits events.amazonaws.com for that rule ARN (lambda:AddPermission with Principal events.amazonaws.com and SourceArn arn:aws:events:<region>:<account>:rule/<ruleName>). PutEvents reports success for the entry (FailedEntryCount 0) and delivery is asynchronous. Recorded behavior only for the end-to-end timing: the recording shows the rendered payload arriving at the target (and from the target's handler into the SQS queue) well inside the check's poll window.

Emulator: the emulator accepts every setup call — CreateQueue, IAM CreateRole/PutRolePolicy, Lambda CreateFunction (the function reaches Active and a direct RequestResponse Invoke succeeds in the warm-up phase, so the function and its role do work), events.PutRule, lambda.AddPermission, events.PutTargets with the InputTransformer, and events.PutEvents (FailedEntryCount 0) — and then produces no target effect at all. No invocation carrying the rendered input ever occurs, so no message whose body has correlationId == the event's $.detail.eventID is ever enqueued. The check never gets to its assertion and is killed by the harness: "Check timed out after 60.0s" (verdict: mismatch). Nothing is refused; there is no error code or message to compare — the divergence is silent non-delivery, not a rejection.

Why: a client that wires an EventBridge rule to a Lambda target with an InputTransformer — the standard way to hand a target a small, reshaped payload instead of the full envelope, e.g. lifting $.detail.requestParameters.distributionId and $.detail.eventID out of an "AWS API Call via CloudTrail" detail and composing arn:aws:cloudfront::<account>:distribution/<distributionId> — sees PutEvents succeed and then nothing happen. Every downstream effect the target would produce (here an SQS message, in general an invalidation, a write, a notification) is silently missing, and no DLQ record or FailedInvocations signal distinguishes "not matched" from "matched but not delivered". Automation built on this pattern appears to work at configuration time and loses every event at run time, and the InputTransformer's rendering rules (angle-bracket substitution, JSON-object templates) cannot be verified at all.

Fix: the EventBridge rule-matching and target-dispatch path for a rule on the default bus — the component that, after PutEvents, matches an entry against each rule's EventPattern, renders the target's InputTransformer (InputPathsMap extraction + InputTemplate substitution) and performs the Lambda target invocation under the target's permission check. The InputTransformer rendering itself cannot be exercised until dispatch happens, so both the dispatch step and the transformer renderer belong in scope.

Related open issues (none of which the replay report marks as owning this check, so this is filed separately and should be de-duplicated against them if a single dispatch fix closes them all): 20261102T091831Z-cloudfront-events-a-matched-rule-never-invokes-its.md, 20261104T051153Z-cloudfront-events-a-matching-putevents-entry-never-invokes.md, 20261029T175823Z-cloudfront-events-13-more-cloudfront-eventbridge-checks-fail.md, 20261101T214154Z-cloudfront-events-two-more-root-causes-a-matched.md.

Traces: cloudfront__events.eventbridge-cloudfront-distribution-arn-resolution-in-target-input (group arn-resolution; services cloudfront, events).

## Evidence
Failing check: cloudfront__events.eventbridge-cloudfront-distribution-arn-resolution-in-target-input
Verdict: mismatch — "Error: Check timed out after 60.0s"

Setup performed by the check (all accepted by the emulator):
  1. sqs.CreateQueue + GetQueueAttributes(QueueArn)
  2. iam.CreateRole (trust lambda.amazonaws.com) + PutRolePolicy (sqs:SendMessage on the queue, logs:*)
  3. lambda.CreateFunction (python3.12, handler relays the received event to SQS), waiter function_active_v2 succeeds
  4. warm-up: lambda.Invoke(RequestResponse) succeeds with no FunctionError, warm-up messages drained
  5. events.PutRule with EventPattern {"source":["sim2real.cloudfront.simulated-cloudtrail"],"detail-type":["AWS API Call via CloudTrail"],"detail":{"eventName":["CreateInvalidation2020_05_31"]}}
  6. lambda.AddPermission(Principal=events.amazonaws.com, SourceArn=arn:aws:events:<region>:<account>:rule/<ruleName>)
  7. events.PutTargets with InputTransformer
       InputPathsMap = {"distId":"$.detail.requestParameters.distributionId","corrId":"$.detail.eventID"}
       InputTemplate = {"distributionArn": "arn:aws:cloudfront::<account>:distribution/<distId>", "correlationId": "<corrId>"}
  8. events.PutEvents with one matching entry; the check asserts and gets FailedEntryCount == 0

Expected (AWS): the matched entry is delivered to the Lambda target with the rendered input, so the handler enqueues a body containing correlationId == the entry's $.detail.eventID and distributionArn == arn:aws:cloudfront::<account>:distribution/<distributionId-from-the-request> (template fixed text: the literal "arn:aws:cloudfront::", ":distribution/", and the member names "distributionArn"/"correlationId"; variable parts: the account id from the caller identity and the distribution id and event id taken from the request's detail).

Actual (emulator): no such message ever arrives. The poll loop in _wait_for_matching_message never sees a body whose correlationId matches, and the check is terminated at the harness's 60 s cap before reaching its assertion. No refusal, no error code, no DLQ record and no FailedInvocations indication accompany the loss.

Not environment-dependent: the generated queue/role/function/rule names, the account id, the correlation id and the host/port differ per run and are not part of the requirement; the requirement is that a matched event reaches an InputTransformer-configured Lambda target with the template rendered.

## Steps to Reproduce
1. Create an SQS queue and note its ARN.
2. Create an IAM role trusted by lambda.amazonaws.com with an inline policy allowing sqs:SendMessage on that queue (and logs:CreateLogGroup/CreateLogStream/PutLogEvents on *).
3. Create a python3.12 Lambda function with that role whose handler does boto3.client("sqs").send_message(QueueUrl=<queue>, MessageBody=json.dumps(event)); wait for it to become Active and confirm a direct RequestResponse Invoke returns with no FunctionError.
4. events.PutRule on the default bus with EventPattern {"source":["sim2real.cloudfront.simulated-cloudtrail"],"detail-type":["AWS API Call via CloudTrail"],"detail":{"eventName":["CreateInvalidation2020_05_31"]}}.
5. lambda.AddPermission on the function: Action lambda:InvokeFunction, Principal events.amazonaws.com, SourceArn arn:aws:events:<region>:<account>:rule/<ruleName>.
6. events.PutTargets on that rule with one Lambda target carrying InputTransformer InputPathsMap {"distId":"$.detail.requestParameters.distributionId","corrId":"$.detail.eventID"} and InputTemplate '{"distributionArn": "arn:aws:cloudfront::<account>:distribution/<distId>", "correlationId": "<corrId>"}'.
7. events.PutEvents with one entry: Source "sim2real.cloudfront.simulated-cloudtrail", DetailType "AWS API Call via CloudTrail", Detail a CloudTrail-shaped object with eventName "CreateInvalidation2020_05_31", requestParameters.distributionId "EDFDVBD6EXAMPLE" and a unique eventID. Observe FailedEntryCount == 0.
8. Poll sqs.ReceiveMessage for up to 120 s for a body whose correlationId equals the entry's eventID.
   Expected: such a message arrives and its distributionArn is arn:aws:cloudfront::<account>:distribution/EDFDVBD6EXAMPLE.
   Actual: no message ever arrives; the check is killed at the harness's 60 s cap with "Check timed out after 60.0s".
