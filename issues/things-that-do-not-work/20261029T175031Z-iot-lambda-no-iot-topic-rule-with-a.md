Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.cfn-deploy-iot-rule-error-action-lambda-positive iot__lambda.cfn-deploy-iot-rule-lambda-env-update-positive iot__lambda.cfn-deploy-iot-rule-lambda-stack-positive iot__lambda.cfn-deploy-iot-rule-missing-lambda-permission-negative iot__lambda.cli-iot-create-topic-rule-lambda-invoke iot__lambda.cli-iot-delete-topic-rule-then-publish-no-invoke-negative iot__lambda.cli-lambda-add-permission-wrong-principal-negative iot__lambda.iot-rule-action-role-trusts-iot-service-positive iot__lambda.iot-rule-action-role-wrong-trust-negative iot__lambda.iot-rule-disabled-no-invocation-negative iot__lambda.iot-rule-error-action-lambda-positive iot__lambda.iot-rule-lambda-action-positive iot__lambda.iot-rule-lambda-missing-invoke-permission-negative iot__lambda.iot-rule-lambda-payload-data-format iot__lambda.iot-rule-lambda-wrong-arn-negative iot__lambda.iot-rule-lambda-wrong-source-arn-negative iot__lambda.iot-rule-republish-chain-lambda-positive iot__lambda.iot-rule-sql-filter-lambda-positive iot__lambda.iot-topicrule-create-with-untrusted-role-negative
Issue-Key: iot__lambda.cfn-deploy-iot-rule-error-action-lambda-positive|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: iot__lambda.cfn-deploy-iot-rule-lambda-env-update-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: iot__lambda.cfn-deploy-iot-rule-lambda-stack-positive|_poll_for_log_marker#10|Timeout|time.sleep(interval_s)
Issue-Key: iot__lambda.cfn-deploy-iot-rule-missing-lambda-permission-negative|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: iot__lambda.cli-iot-create-topic-rule-lambda-invoke|_receive_once#10|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s)
Issue-Key: iot__lambda.cli-iot-delete-topic-rule-then-publish-no-invoke-negative|_receive_once#18|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s)
Issue-Key: iot__lambda.cli-lambda-add-permission-wrong-principal-negative|_receive_once#15|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s)
Issue-Key: iot__lambda.iot-rule-action-role-trusts-iot-service-positive|_publish_until_delivered#13|Timeout|resp = sqs.receive_message(
Issue-Key: iot__lambda.iot-rule-action-role-wrong-trust-negative|_publish_until_delivered#6|Timeout|resp = sqs.receive_message(
Issue-Key: iot__lambda.iot-rule-disabled-no-invocation-negative|_receive_once#15|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s)
Issue-Key: iot__lambda.iot-rule-error-action-lambda-positive|_publish_until_delivered#14|Timeout|time.sleep(1.5)
Issue-Key: iot__lambda.iot-rule-lambda-action-positive|_publish_until_delivered#12|Timeout|time.sleep(1.5)
Issue-Key: iot__lambda.iot-rule-lambda-missing-invoke-permission-negative|_receive_once#14|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s)
Issue-Key: iot__lambda.iot-rule-lambda-payload-data-format|_publish_until_delivered#13|Timeout|time.sleep(1.5)
Issue-Key: iot__lambda.iot-rule-lambda-wrong-arn-negative|_receive_all#18|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=max_messages, WaitTimeSeconds=5)
Issue-Key: iot__lambda.iot-rule-lambda-wrong-source-arn-negative|_receive_once#19|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s)
Issue-Key: iot__lambda.iot-rule-republish-chain-lambda-positive|_publish_until_delivered#16|Timeout|time.sleep(1.5)
Issue-Key: iot__lambda.iot-rule-sql-filter-lambda-positive|_receive_once#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait_s)
Issue-Key: iot__lambda.iot-topicrule-create-with-untrusted-role-negative|_publish_until_delivered#6|Timeout|resp = sqs.receive_message(
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# iot__lambda: no IoT topic rule with a lambda action ever invokes its target function, so every end-to-end IoT->Lambda check stalls with no observable effect (1 cause)

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One root cause, grounded in 19 failing behavioral checks that span the whole IoT-rule -> Lambda surface (plain-API rules, CLI-created rules, CFN-deployed rules, SQL-filtered rules, error actions, republish chains, and the role/permission negatives that each first establish a positive baseline).

=== Cause 1 of 1: an enabled topic rule whose action is `lambda` never invokes the target function on a matching publish ===
AWS: a topic rule with actions=[{"lambda": {"functionArn": <arn>}}] and SQL selecting a topic invokes that function asynchronously for every matching iot-data:Publish, passing the selected message as the invocation event, provided the function's resource policy allows iot.amazonaws.com to lambda:InvokeFunction for the rule ARN (lambda:AddPermission with Principal=iot.amazonaws.com, SourceArn=arn:aws:iot:<region>:<account>:rule/<ruleName>). The effect is observable: the invoked function runs and its side effects appear (here, a forwarder function relaying the received event onto SQS) and its log stream appears under /aws/lambda/<function>. The negative half is equally load-bearing — a disabled rule, a wrong functionArn, a wrong SourceArn, a wrong principal, an untrusted role or a deleted rule must produce NO invocation — but each of those checks first proves the positive path works, so none of them can be graded either. Smithy: iot CreateTopicRule's TopicRulePayload.actions carries a LambdaAction{functionArn}; the invocation itself is iot's server-side behavior and is not described by either service's model (recorded behavior only).
Emulator: nothing observable ever happens. All 19 checks make no progress past the publish and are killed by the harness's 60 s ceiling ("Check timed out after 60.0s") while polling for the invocation's effect — the forwarder's SQS message, the function's CloudWatch log line, or the error-action target. iot:CreateTopicRule, lambda:CreateFunction, lambda:AddPermission and iot-data:Publish all succeed, so the break is squarely in the rule engine's action dispatch (and, where dispatch would reach it, in Lambda's inability to run a deployment package).
Why: a client that wires its devices to business logic through an IoT rule's lambda action — the single most common IoT Core integration — sees its rule created, its permission attached and its publish accepted 200, and its function is simply never called. Nothing in any API response reports the failure. Worse, every negative/guardrail assertion in this area becomes vacuously true: a rule that is disabled, points at the wrong ARN, or has no invoke permission is indistinguishable from one that is correctly configured, so the emulator cannot be used to validate IoT authorization at all.
Fix: the topic-rule action dispatcher in the iot crate (services/service-com-amazonaws-iot-awsiotservice/src/ops/create_topic_rule.rs registers the rule; the dispatch path reached from the data plane's publish, in that crate's src/state.rs and in services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/publish.rs) must match an enabled rule's SQL against the published topic and actually invoke the LambdaAction's functionArn with the selected message as the event, honouring the function's resource policy so that a missing or mismatched permission produces no invocation rather than the same silence as a rule that never ran. Two open issues cover adjacent parts of this and own none of these 19 checks: 20261024T035602Z-iot-topic-rules-never-execute-their-actions-when.md (rule actions never execute) and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (Lambda cannot run a deployment package) — both must be in place for these checks to pass, and the four cfn-deploy-* checks additionally depend on Cause 1 of the sibling issue filed for service `iot` (a CFN-deployed AWS::IoT::TopicRule is never registered with IoT).
Traces: all 19 check ids listed in trace_ids.

## Evidence
Every one of the 19 checks reports the same verdict and detail, with no further diagnosis available from the emulator:
  [mismatch] Error: Check timed out after 60.0s

Representative — iot__lambda.iot-rule-lambda-action-positive:
  setup that succeeded: iam:CreateRole (+AWSLambdaBasicExecutionRole, +sqs:SendMessage inline), lambda:CreateFunction(Runtime=python3.12, Handler=index.handler) reaching State=Active, lambda:AddPermission(Principal=iot.amazonaws.com, SourceArn=arn:aws:iot:<region>:<account>:rule/<ruleName>), iot:CreateTopicRule(sql="SELECT * FROM '<topic>'", actions=[{"lambda":{"functionArn":<arn>}}]), iot-data:Publish(topic=<topic>, payload={"deviceId":"sensor-1","temperature":21.5,"unit":"C"})
  expected: the function is invoked with the published message as its event, and the forwarder relays that event onto the SQS queue (assert messages[0] == the published payload)
  actual:   the queue never receives anything; the check is still polling when the 60 s harness ceiling kills it

The same shape holds for the CLI-created (cli-iot-*), CFN-deployed (cfn-deploy-iot-rule-*), SQL-filter, error-action and republish-chain variants, and for the role/permission negatives, each of which establishes the positive baseline before asserting its absence.

## Steps to Reproduce
1. iam:CreateRole with a lambda.amazonaws.com trust policy; attach AWSLambdaBasicExecutionRole and an inline sqs:SendMessage policy for a queue Q.
2. lambda:CreateFunction(FunctionName=F, Runtime=python3.12, Handler=index.handler, Code=a zip whose handler does sqs.send_message(QueueUrl=os.environ['QUEUE_URL'], MessageBody=json.dumps(event)), Environment={QUEUE_URL: Q}); wait for State=Active.
3. lambda:AddPermission(FunctionName=F, Principal=iot.amazonaws.com, Action=lambda:InvokeFunction, SourceArn=arn:aws:iot:<region>:<account>:rule/<ruleName>).
4. iot:CreateTopicRule(ruleName=<ruleName>, topicRulePayload={sql: "SELECT * FROM '<topic>'", ruleDisabled: false, actions: [{"lambda": {"functionArn": <F's arn>}}]}).
5. iot-data:Publish(topic=<topic>, payload=json of {"deviceId":"sensor-1","temperature":21.5,"unit":"C"}) -> 200.
6. sqs:ReceiveMessage(QueueUrl=Q) in a loop, and/or logs:FilterLogEvents on /aws/lambda/F -> expect the published event; observe nothing, indefinitely.
7. Then repeat with each negative variant (ruleDisabled=true, a wrong functionArn, a wrong SourceArn, a wrong AddPermission principal, a role that does not trust iot.amazonaws.com, and a deleted rule) and confirm the absence is distinguishable from step 6's silence.
