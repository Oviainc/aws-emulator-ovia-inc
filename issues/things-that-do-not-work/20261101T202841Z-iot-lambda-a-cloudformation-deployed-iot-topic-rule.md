Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.cfn-deploy-iot-rule-error-action-lambda-positive iot__lambda.cfn-deploy-iot-rule-lambda-env-update-positive iot__lambda.cfn-deploy-iot-rule-missing-lambda-permission-negative
Issue-Key: iot__lambda.cfn-deploy-iot-rule-error-action-lambda-positive|_poll_until#9|Timeout|last = probe()
Issue-Key: iot__lambda.cfn-deploy-iot-rule-lambda-env-update-positive|_poll_until#10|Timeout|last = probe()
Issue-Key: iot__lambda.cfn-deploy-iot-rule-missing-lambda-permission-negative|_poll_until#11|Timeout|last = probe()
Issue-Commit: dc4158ff05b54d9987477de08887f7f0381de207

# iot__lambda: a CloudFormation-deployed IoT topic rule never reaches its Lambda target — errorAction fallback never fires, an UpdateStack env change is never reflected in an invocation, and a missing Lambda::Permission is indistinguishable from a rule that never ran (3 causes)

> _Note: this issue was identified against the code base at commit `dc4158ff05b54d9987477de08887f7f0381de207`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Three net-new root causes, grounded in 3 failing behavioral checks, all in the CloudFormation-deployed IoT-rule -> Lambda surface. All three stall and are killed by the harness wall clock with no observable effect and no API-level error.

Related open issues that do NOT own these three checks (the replay report marked none of them OWNED, so they are unclaimed): 20261029T175031Z-iot-lambda-no-iot-topic-rule-with-a.md (an enabled plain-API topic rule with a lambda action never invokes its function), 20261029T175031Z-iot-cfn-deployed-aws-iot-topicrule-is-not.md (a CFN-deployed AWS::IoT::TopicRule is not a working rule), 20261024T035602Z-iot-topic-rules-never-execute-their-actions-when.md (rule actions never execute) and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (Lambda cannot run a deployment package). Those cover the plain-API primary-action path; the three causes below each add a distinct requirement on top of it that none of them states.

=== Cause 1 of 3: a topic rule's errorAction Lambda is never invoked when the primary lambda action fails ===
Failing check: iot__lambda.cfn-deploy-iot-rule-error-action-lambda-positive
AWS: Smithy (com.amazonaws.iot) models TopicRulePayload.errorAction as an `Action` ("The action to take when an error occurs"), and LambdaAction.functionArn as a required member. When a rule's primary action cannot be executed — here a `lambda` action whose functionArn names a function that does not exist — IoT executes the rule's errorAction instead, invoking the fallback function with the rule-failure event. Recorded behavior only for the event's shape (neither the iot nor the lambda model describes it): the event carries `ruleName` (the rule's name as created), `topic` (the topic the message was published to), `base64OriginalPayload` (base64 of the exact published payload bytes) and a non-empty `failures` list whose first element has a non-empty `errorMessage` describing why the primary action failed. The effect is observable through the fallback function's own side effects.
Emulator: nothing is ever invoked. CreateStack reaches CREATE_COMPLETE, iot-data:Publish is accepted, and the check then polls the fallback function's SQS sink until the harness kills it: `Error: Check timed out after 60.0s`. No errorAction is dispatched, and no API response reports the primary action's failure.
Why: a client that uses errorAction as its dead-letter path for rule failures gets total silence. The failure of the primary action is unreported in every API response AND unreported through the one channel AWS provides for it, so an IoT pipeline's error handling cannot be exercised or validated at all against the emulator.
Fix: the topic-rule action dispatcher in the iot crate — the dispatch path reached from the data plane's publish (services/service-com-amazonaws-iot-awsiotservice/src/state.rs, reached from services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/publish.rs) — must, when an action's execution fails, build the rule-failure event above and execute the rule's stored errorAction, rather than discarding the failure.
Traces: iot__lambda.cfn-deploy-iot-rule-error-action-lambda-positive.

=== Cause 2 of 3: a CloudFormation stack update that changes a Lambda function's Environment is not reflected in the next invocation ===
Failing check: iot__lambda.cfn-deploy-iot-rule-lambda-env-update-positive
AWS: recorded behavior only (CloudFormation resource semantics, not described by the iot or lambda Smithy model). UpdateStack with a template whose AWS::Lambda::Function Properties.Environment.Variables hold a new value updates that function in place; the stack settles in StackStatus=UPDATE_COMPLETE, and every invocation after the update observes the NEW environment value (the pre-update invocation must observe the OLD one). The check reads the value back out of the invocation event the function itself emits, so both halves are observable purely through the AWS API.
Emulator: the pre-update half already never happens — the first publish produces no invocation at all, so the check is killed polling for it: `Error: Check timed out after 60.0s`. The post-update assertion (UPDATE_COMPLETE, then an invocation seeing the new environment value) is never reached, so environment-variable propagation through UpdateStack is entirely unverified.
Why: a client that versions configuration through a Lambda environment variable and ships it with `cloudformation update-stack` has no way to tell whether the update took effect. Because no invocation occurs, a stale environment and a correctly updated one are indistinguishable, and the emulator cannot be used to validate a redeploy.
Fix: two components. The rule-action dispatcher named in Cause 1 must invoke the lambda action so an invocation exists to inspect; and the CloudFormation resource handler for AWS::Lambda::Function must, on stack update, apply a changed Properties.Environment to the live function (an UpdateFunctionConfiguration-equivalent on the lambda crate's state) so the next invocation sees the new value.
Traces: iot__lambda.cfn-deploy-iot-rule-lambda-env-update-positive.

=== Cause 3 of 3: a rule whose stack omits AWS::Lambda::Permission is indistinguishable from a rule that never ran, because the same-stack positive control also never delivers ===
Failing check: iot__lambda.cfn-deploy-iot-rule-missing-lambda-permission-negative
AWS: recorded behavior only. A `lambda` rule action is gated by the TARGET function's resource policy: IoT invokes it only when the function allows Principal=iot.amazonaws.com to lambda:InvokeFunction for SourceArn=arn:aws:iot:<region>:<account>:rule/<ruleName>. A stack that provisions the rule and the function but OMITS the AWS::Lambda::Permission resource therefore yields ZERO invocations for matching traffic, while a sibling rule in the SAME stack that does carry the permission still delivers. The absence is a second-order observable too: sqs:GetQueueAttributes on the un-invoked function's sink must report ApproximateNumberOfMessages="0".
Emulator: the positive control in the same stack never delivers either — the check is killed polling it: `Error: Check timed out after 60.0s`. With both halves silent, the permission gate is vacuously satisfied: the emulator reaches the same end state whether the resource policy allows the invocation or not, so this guardrail is unobservable and cannot be graded.
Why: a client relying on the emulator to prove that an IoT rule cannot reach a Lambda it was never granted permission to invoke gets a false pass. An under-permissioned deployment and a correctly permissioned one look identical, so IoT-to-Lambda authorization regressions cannot be caught.
Fix: the rule-action dispatcher named in Cause 1 must consult the target function's resource policy in the lambda crate's state before invoking (matching Principal=iot.amazonaws.com and the rule ARN against SourceArn) and skip the invocation when no statement allows it — so that "no permission" produces a deliberate non-invocation while a permitted sibling action in the same stack does deliver.
Traces: iot__lambda.cfn-deploy-iot-rule-missing-lambda-permission-negative.

## Evidence
All three checks report the same verdict and the same single line of detail; the emulator offers no further diagnosis:
  [mismatch] iot__lambda.cfn-deploy-iot-rule-error-action-lambda-positive — Error: Check timed out after 60.0s
  [mismatch] iot__lambda.cfn-deploy-iot-rule-lambda-env-update-positive — Error: Check timed out after 60.0s
  [mismatch] iot__lambda.cfn-deploy-iot-rule-missing-lambda-permission-negative — Error: Check timed out after 60.0s

Cause 1 (errorAction) — setup that SUCCEEDED: cloudformation:CreateStack of a template with AWS::SQS::Queue + AWS::IAM::Role (lambda.amazonaws.com trust, inline logs:* and sqs:SendMessage) + AWS::Lambda::Function (Runtime=python3.12, Handler=index.handler, ZipFile forwarder relaying its event to SQS, Environment.QUEUE_URL -> the queue) + AWS::Lambda::Permission (Principal=iot.amazonaws.com, SourceArn=arn:aws:iot:<region>:<account>:rule/<ruleName>) + AWS::IoT::TopicRule whose Actions=[{Lambda:{FunctionArn: arn:aws:lambda:<region>:<account>:function:<a name no function holds>}}] and ErrorAction={Lambda:{FunctionArn: <the fallback function>}}; the stack reached CREATE_COMPLETE; iot-data:Publish(topic=<topic>, payload=json {"marker": <marker>}) was accepted.
  expected: the fallback function is invoked and relays a rule-failure event to the queue, with ruleName == the created rule name, topic == the published topic, base64decode(base64OriginalPayload) == the exact published payload bytes, and failures being a non-empty list whose [0].errorMessage is non-empty.
  actual:   the queue never receives anything; still polling when the harness ceiling kills the check.

Cause 2 (UpdateStack env propagation) — stack deployed with the Lambda's Environment variable TOPIC_ARN set to a v1 value, reaching CREATE_COMPLETE; iot-data:Publish accepted.
  expected: the pre-update invocation delivers the published marker AND reports topic_arn equal to the v1 value; then cloudformation:UpdateStack with the same template carrying a v2 value settles in StackStatus=UPDATE_COMPLETE, and the NEXT invocation reports the v2 value.
  actual:   no invocation at all — the pre-update poll never sees a message and the check is killed, so UPDATE_COMPLETE and the v2 value are never observed.

Cause 3 (missing Lambda::Permission) — one stack provisioning TWO rules/functions/queues: a "good" pair WITH its AWS::Lambda::Permission and a "bad" pair with that resource omitted; stack reached CREATE_COMPLETE.
  expected: publishing to the good topic delivers the marker to the good queue (positive control), publishing to the bad topic delivers NOTHING within a window of at least 2x the measured positive-control latency, and sqs:GetQueueAttributes on the bad queue reports ApproximateNumberOfMessages == "0".
  actual:   the positive control never delivers either; the check is killed polling it, so the negative assertion is never reached and the permission gate is unobservable.

## Steps to Reproduce
Cause 1 — errorAction never fires:
1. cloudformation:CreateStack (Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM]) with: AWS::SQS::Queue Q; AWS::IAM::Role trusting lambda.amazonaws.com with inline logs:CreateLogGroup/CreateLogStream/PutLogEvents and sqs:SendMessage on Q's Arn; AWS::Lambda::Function FALLBACK (Runtime=python3.12, Handler=index.handler, Timeout=30, Code.ZipFile = a handler doing sqs.send_message(QueueUrl=os.environ['QUEUE_URL'], MessageBody=json.dumps(event)), Environment.Variables.QUEUE_URL={Ref: Q}); AWS::Lambda::Permission on FALLBACK (Action=lambda:InvokeFunction, Principal=iot.amazonaws.com, SourceArn=arn:aws:iot:<region>:<account>:rule/<ruleName>); AWS::IoT::TopicRule RuleName=<ruleName> with TopicRulePayload.Sql="SELECT * FROM '<topic>'", RuleDisabled=false, Actions=[{Lambda:{FunctionArn: arn:aws:lambda:<region>:<account>:function:<a function name that does not exist>}}], ErrorAction={Lambda:{FunctionArn: FALLBACK's Arn}}; Outputs: Q's url.
2. Wait for StackStatus=CREATE_COMPLETE.
3. iot:DescribeEndpoint(endpointType="iot:Data-ATS") and build an iot-data client against it; iot-data:Publish(topic=<topic>, qos=0, payload=json {"marker": <marker>}) -> 200.
4. sqs:ReceiveMessage(QueueUrl=Q) in a loop. Expect a rule-failure event with ruleName/topic/base64OriginalPayload/failures[0].errorMessage; observe nothing.

Cause 2 — UpdateStack env change not reflected:
1. CreateStack with an AWS::IoT::TopicRule whose Lambda action targets a forwarder function, and whose AWS::Lambda::Function has Environment.Variables.TOPIC_ARN set to a v1 value; the forwarder echoes {marker, topic_arn: os.environ['TOPIC_ARN']} onto queue Q. Wait for CREATE_COMPLETE.
2. iot-data:Publish a message with marker1; ReceiveMessage from Q -> expect marker1 and topic_arn == the v1 value. (Currently nothing arrives.)
3. cloudformation:UpdateStack with the same template but TOPIC_ARN set to a v2 value; poll describe_stacks until StackStatus=UPDATE_COMPLETE.
4. iot-data:Publish a message with marker2; ReceiveMessage from Q -> expect marker2 and topic_arn == the v2 value.

Cause 3 — missing Lambda::Permission indistinguishable from a dead rule:
1. CreateStack provisioning two independent triples (role + forwarder function + queue + topic rule): GOOD, which also includes an AWS::Lambda::Permission granting Principal=iot.amazonaws.com / SourceArn=arn:aws:iot:<region>:<account>:rule/<goodRuleName>; and BAD, identical except the AWS::Lambda::Permission resource is omitted. Wait for CREATE_COMPLETE.
2. iot-data:Publish to the GOOD topic; ReceiveMessage from the good queue -> expect the marker, and record the delivery latency. (Currently nothing arrives — the check dies here.)
3. iot-data:Publish to the BAD topic; poll the bad queue for at least 2x the latency measured in step 2 -> expect zero messages.
4. sqs:GetQueueAttributes(QueueUrl=bad queue, AttributeNames=[ApproximateNumberOfMessages]) -> expect "0".
