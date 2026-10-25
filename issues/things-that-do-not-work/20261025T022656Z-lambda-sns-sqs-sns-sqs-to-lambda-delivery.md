Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.cfn-deploy-iam-role-trust-sns-positive lambda__sns__sqs.cfn-deploy-sns-sqs-lambda-full-pipeline-positive lambda__sns__sqs.sns-lambda-dlq-on-invoke-failure lambda__sns__sqs.sns-notification-config-lambda-destination-update lambda__sns__sqs.sns-to-sqs-subscription-basic lambda__sns__sqs.snstrig-alias-subscription-invokes-the-alias-qualifier lambda__sns__sqs.snstrig-grant-scoped-to-another-topic-blocked-delivery lambda__sns__sqs.snstrig-message-body-filter-blocked-a-non-matching-publish lambda__sns__sqs.snstrig-publish-batch-invokes-the-function-once-per-matching-entry lambda__sns__sqs.sqs-to-lambda-esm-basic lambda__sns__sqs.cfn-deploy-iam-role-trust-lambda-invokes-sqs-positive lambda__sns__sqs.cfn-deploy-iam-role-trust-mismatch-negative lambda__sns__sqs.iam-role-trust-mismatched-principal-sqs-blocked lambda__sns__sqs.cfn-deploy-sqs-esm-filter-criteria-positive
Issue-Key: lambda__sns__sqs.cfn-deploy-iam-role-trust-sns-positive|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.cfn-deploy-sns-sqs-lambda-full-pipeline-positive|<lambda>#14|Timeout|lambda: sqs.receive_message(QueueUrl=out_queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5).get("Messages", []),
Issue-Key: lambda__sns__sqs.sns-lambda-dlq-on-invoke-failure|_poll_for_message#23|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Key: lambda__sns__sqs.sns-notification-config-lambda-destination-update|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.sns-to-sqs-subscription-basic|_wait_for_messages#11|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sns__sqs.snstrig-alias-subscription-invokes-the-alias-qualifier|_drain#13|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sns__sqs.snstrig-grant-scoped-to-another-topic-blocked-delivery|_drain#13|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sns__sqs.snstrig-message-body-filter-blocked-a-non-matching-publish|_drain#15|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sns__sqs.snstrig-publish-batch-invokes-the-function-once-per-matching-entry|_drain#16|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sns__sqs.sqs-to-lambda-esm-basic|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.cfn-deploy-iam-role-trust-lambda-invokes-sqs-positive|run#14|AssertionError|raise AssertionError(f"invoke never succeeded: {last_err}")
Issue-Key: lambda__sns__sqs.cfn-deploy-iam-role-trust-mismatch-negative|run#10|AssertionError|assert final_status == "ROLLBACK_COMPLETE", (
Issue-Key: lambda__sns__sqs.iam-role-trust-mismatched-principal-sqs-blocked|_client_error#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Related: 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md
Issue-Related: 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# lambda__sns__sqs: SNS/SQS-to-Lambda delivery never happens, a stack's function does not exist, a mistrusted-role stack completes, AssumeRole ignores the trust policy, and a CFN ESM id is not a UUID (5 causes)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 5 — nothing is ever delivered to a Lambda from SNS or from an SQS event source mapping, so ten checks hang to the harness 60 s cap.
AWS: Publish to a topic with a lambda subscription invokes the function once per matching message (respecting subscription filter policies and the alias qualifier in the endpoint ARN), PublishBatch invokes it once per matching entry, an invoke failure is retried and then written to the subscription's dead-letter queue, and an SQS ESM invokes the function with the queued messages. Recorded behavior only for the delivery delay.
Emulator: none of it happens — every one of the ten checks reports 'Error: Check timed out after 60.0s'; no invocation, no DLQ record, no SQS delivery.
Why: a client's entire notification/queue fan-out is inert while every API response says it is configured; waiters never resolve and no error explains the silence. Negative controls (a grant scoped to another topic, a non-matching message filter) prove nothing because the positive arm does not work either.
Fix: SNS subscription delivery to lambda endpoints (including filter-policy evaluation, alias qualifiers and DLQ redrive) in the sns service, and the SQS event-source-mapping poller in the lambda service.
Traces: lambda__sns__sqs.cfn-deploy-iam-role-trust-sns-positive, lambda__sns__sqs.cfn-deploy-sns-sqs-lambda-full-pipeline-positive, lambda__sns__sqs.sns-lambda-dlq-on-invoke-failure, lambda__sns__sqs.sns-notification-config-lambda-destination-update, lambda__sns__sqs.sns-to-sqs-subscription-basic, lambda__sns__sqs.snstrig-alias-subscription-invokes-the-alias-qualifier, lambda__sns__sqs.snstrig-grant-scoped-to-another-topic-blocked-delivery, lambda__sns__sqs.snstrig-message-body-filter-blocked-a-non-matching-publish, lambda__sns__sqs.snstrig-publish-batch-invokes-the-function-once-per-matching-entry, lambda__sns__sqs.sqs-to-lambda-esm-basic.

Cause 2 of 5 — a stack-deployed function does not exist afterwards.
AWS: a stack that reaches CREATE_COMPLETE with an AWS::Lambda::Function has registered that function, so Invoke on it answers (200, or a modeled refusal such as AccessDeniedException). Emulator: Invoke is refused 404 ResourceNotFoundException 'Function not found: arn:aws:lambda:...:function:<fn>' (recorded message, an example). Why: a successful deploy is followed by not-found on the resource it claims to own. Fix: the CloudFormation AWS::Lambda::Function provisioner must create the function in the lambda service. Related open issue 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md does not list this check.
Traces: lambda__sns__sqs.cfn-deploy-iam-role-trust-lambda-invokes-sqs-positive.

Cause 3 of 5 — a stack whose Lambda execution role is not trustable reaches CREATE_COMPLETE.
AWS: AWS::Lambda::Function creation fails when the referenced role's trust policy does not allow lambda.amazonaws.com, so the stack rolls back (ROLLBACK_COMPLETE). Emulator: the stack reports 'CREATE_COMPLETE'. Why: a misconfigured trust policy ships to production and only shows up as unexplained invoke failures later. Fix: role-trust validation in the CloudFormation AWS::Lambda::Function provisioner / lambda CreateFunction, and the stack rollback path. Related open issue 20261023T113357Z-lambda-createfunction-accepts-a-role-whose-trust-policy.md records the direct CreateFunction form, not this stack form.
Traces: lambda__sns__sqs.cfn-deploy-iam-role-trust-mismatch-negative.

Cause 4 of 5 — STS AssumeRole succeeds for a principal the role's trust policy does not allow.
AWS: AssumeRole is refused AccessDenied (403) when the caller is not a trusted principal in the role's AssumeRolePolicyDocument. Emulator: the call returns 200 with Credentials and an AssumedRoleUser for a mistrusted principal. Why: the trust policy — the only thing standing between two principals — is not enforced, so a negative-permission test passes when it should fail and real isolation does not exist. Fix: trust-policy evaluation in the sts AssumeRole handler.
Traces: lambda__sns__sqs.iam-role-trust-mismatched-principal-sqs-blocked.

Cause 5 of 5 — the CFN-created event source mapping's id is not a UUID.
AWS: the physical resource id of AWS::Lambda::EventSourceMapping is the mapping UUID; Smithy (lambda) GetEventSourceMapping.UUID is @length(min:36,max:36). Emulator: the id is 'sim2real-xsvc-stk-<id>-EventSourceMapping-26134B335EA0' and GetEventSourceMapping on it is refused 400 ValidationException for the length constraint. Why: a stack-created mapping cannot be read, updated or deleted through the Lambda API using the id the stack reports. Fix: the AWS::Lambda::EventSourceMapping provisioner must record the UUID returned by lambda:CreateEventSourceMapping.
Traces: lambda__sns__sqs.cfn-deploy-sqs-esm-filter-criteria-positive.

## Evidence
Cause 1: Error: Check timed out after 60.0s (ten checks).
Cause 2: 'invoke never succeeded: An error occurred (ResourceNotFoundException) when calling the Invoke operation: Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-fn-b2ba1be3a18d'.
Cause 3: 'expected the mistrusted-role stack to roll back, got CREATE_COMPLETE'.
Cause 4: 'expected the call to be rejected, but it succeeded with {Credentials: {...}, AssumedRoleUser: {...}, HTTPStatusCode: 200}'.
Cause 5: ClientError (ValidationException) ... Value 'sim2real-xsvc-stk-bbeec60ffbd3-EventSourceMapping-26134B335EA0' at 'uUID' failed to satisfy constraint: Member must have length less than or equal to 36.

## Steps to Reproduce
Cause 1: subscribe a function (and an SQS queue) to a topic, grant sns.amazonaws.com invoke, Publish/PublishBatch, and poll for invocations and DLQ records; separately wire an SQS ESM and send messages.
Cause 2: deploy a stack with AWS::Lambda::Function and Invoke the function it reports.
Cause 3: deploy a stack whose function references a role whose trust policy omits lambda.amazonaws.com.
Cause 4: create a role trusting only principal A and call AssumeRole as principal B.
Cause 5: deploy a stack with AWS::Lambda::EventSourceMapping (FilterCriteria) and call GetEventSourceMapping with the reported physical id.
