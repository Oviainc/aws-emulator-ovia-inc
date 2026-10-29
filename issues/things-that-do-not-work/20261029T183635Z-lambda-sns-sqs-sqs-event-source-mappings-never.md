Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.sqs-lambda-esm-batch-size-respected lambda__sns__sqs.sqs-lambda-esm-batch-window-respected lambda__sns__sqs.sqs-lambda-esm-filter-criteria-delivers lambda__sns__sqs.sqs-lambda-esm-filter-criteria-matches lambda__sns__sqs.sqs-lambda-esm-filter-criteria-suppresses-non-matching lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits lambda__sns__sqs.sqs-lambda-esm-partial-batch-failure-requeues lambda__sns__sqs.sqs-lambda-esm-disabled-mapping-no-invocation-blocked lambda__sns__sqs.sqs-lambda-esm-disabled-no-invocation-negative lambda__sns__sqs.esm-creation-fails-for-nonexistent-queue-arn lambda__sns__sqs.sqs-lambda-esm-wrong-arn-mapping-fails lambda__sns__sqs.sqs-lambda-esm-wrong-queue-arn-negative lambda__sns__sqs.sqs-to-lambda-esm-missing-permission lambda__sns__sqs.iam-trust-sqs-wrong-principal-negative lambda__sns__sqs.iam-role-trust-mismatched-principal-sns-blocked lambda__sns__sqs.iam-role-trust-sns-service-assume lambda__sns__sqs.iam-trust-sns-wrong-principal-negative lambda__sns__sqs.sns-to-lambda-subscription-basic lambda__sns__sqs.snstrig-publish-invokes-the-function-with-the-sns-record-envelope lambda__sns__sqs.sns-lambda-notification-message-attributes-filter lambda__sns__sqs.sns-to-lambda-missing-invoke-permission lambda__sns__sqs.sns-add-permission-source-arn-mismatch lambda__sns__sqs.pipeline-sns-sqs-lambda lambda__sns__sqs.shared-state-sns-fanout-two-lambdas-ddb lambda__sns__sqs.cli-lambda-add-permission-sns-invoke-workflow lambda__sns__sqs.cli-missing-lambda-permission-blocks-invoke-workflow lambda__sns__sqs.sns-sqs-cross-account-missing-topic-policy-negative lambda__sns__sqs.iam-role-trust-sqs-principal lambda__sns__sqs.cfn-deploy-iam-role-trust-lambda-positive
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-batch-size-respected|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-batch-window-respected|_drain_output_batches#14|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-filter-criteria-delivers|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-filter-criteria-matches|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-filter-criteria-suppresses-non-matching|_drain_sink#18|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-partial-batch-failure-requeues|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-disabled-mapping-no-invocation-blocked|_drain_sink#21|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-disabled-no-invocation-negative|run#22|AssertionError|assert control_delivered, (
Issue-Key: lambda__sns__sqs.esm-creation-fails-for-nonexistent-queue-arn|_client_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-wrong-arn-mapping-fails|_drain_sink#18|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-wrong-queue-arn-negative|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.sqs-to-lambda-esm-missing-permission|_client_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Key: lambda__sns__sqs.iam-trust-sqs-wrong-principal-negative|run#18|AssertionError|assert esm_err_code == "InvalidParameterValueException", esm_err_code
Issue-Key: lambda__sns__sqs.iam-role-trust-mismatched-principal-sns-blocked|run#16|AssertionError|raise AssertionError(
Issue-Key: lambda__sns__sqs.iam-role-trust-sns-service-assume|_error#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: lambda__sns__sqs.iam-trust-sns-wrong-principal-negative|run#20|AssertionError|assert rejected, (
Issue-Key: lambda__sns__sqs.sns-to-lambda-subscription-basic|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.snstrig-publish-invokes-the-function-with-the-sns-record-envelope|_drain#12|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sns__sqs.sns-lambda-notification-message-attributes-filter|_poll_for_message#18|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__sns__sqs.sns-to-lambda-missing-invoke-permission|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.sns-add-permission-source-arn-mismatch|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.pipeline-sns-sqs-lambda|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.shared-state-sns-fanout-two-lambdas-ddb|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.cli-lambda-add-permission-sns-invoke-workflow|_wait_for_marker#22|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__sns__sqs.cli-missing-lambda-permission-blocks-invoke-workflow|_wait_for_marker#28|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__sns__sqs.sns-sqs-cross-account-missing-topic-policy-negative|_client_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Key: lambda__sns__sqs.iam-role-trust-sqs-principal|_poll_for_message#24|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.cfn-deploy-iam-role-trust-lambda-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# lambda__sns__sqs: SQS event-source mappings never invoke, CreateEventSourceMapping accepts a nonexistent queue and an unauthorized role (wrong code when it does refuse), SNS->Lambda delivers nothing, Subscribe skips the cross-account topic policy, and SetTopicAttributes accepts untrusting feedback roles (6 causes)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Six net-new root causes across the SNS/SQS/Lambda wiring. Open issues 20261025T022656Z-lambda-sns-sqs-sns-sqs-to-lambda-delivery.md and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md own neighbouring checks; none of the checks listed below is marked owned or already-filed.

Cause 1 of 6: an SQS event-source mapping never delivers a batch to the function, so every ESM behaviour knob is ungraded.
AWS: CreateEventSourceMapping (HTTP POST /2015-03-31/event-source-mappings, success code 202) with an SQS EventSourceArn starts a managed poller. BatchSize (@range 1..10000) caps records per invocation; MaximumBatchingWindowInSeconds (@range 0..300) holds a partial batch open; FilterCriteria suppresses non-matching records; ScalingConfig.MaximumConcurrency caps concurrent invocations; FunctionResponseTypes=['ReportBatchItemFailures'] requeues only the item ids the function returns; Enabled=false stops delivery. Each of these is observable only through actual invocations.
Emulator: every one of these checks runs to the harness cap and is terminated at 60 s, and the explicitly-calibrated one reports its positive control failed outright - "control ESM (Enabled=True, same wiring) delivered nothing in the window; the disabled mapping's silence over that same window is not evidence of anything without it". The poller never invokes the function, so batch size, batch window, filter criteria, maximum concurrency, partial-batch failure and the disabled-mapping negative are all unobservable.
Why: SQS-triggered Lambda is the most common serverless consumer pattern; here it processes nothing. Every negative assertion (disabled mapping, non-matching filter) also passes vacuously, so a client gets false confidence from exactly the tests meant to catch misconfiguration.
Fix: the event-source-mapping poller - receive from the EventSourceArn queue, build the SQS records batch honouring BatchSize/MaximumBatchingWindowInSeconds/FilterCriteria/ScalingConfig, invoke the function, and apply the ReportBatchItemFailures contract on the response.
Traces: lambda__sns__sqs.sqs-lambda-esm-batch-size-respected, lambda__sns__sqs.sqs-lambda-esm-batch-window-respected, lambda__sns__sqs.sqs-lambda-esm-filter-criteria-delivers, lambda__sns__sqs.sqs-lambda-esm-filter-criteria-matches, lambda__sns__sqs.sqs-lambda-esm-filter-criteria-suppresses-non-matching, lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits, lambda__sns__sqs.sqs-lambda-esm-partial-batch-failure-requeues, lambda__sns__sqs.sqs-lambda-esm-disabled-mapping-no-invocation-blocked, lambda__sns__sqs.sqs-lambda-esm-disabled-no-invocation-negative.

Cause 2 of 6: CreateEventSourceMapping accepts an EventSourceArn for a queue that does not exist.
AWS: CreateEventSourceMapping validates that the event source resolves before creating the mapping. A well-formed SQS ARN naming a queue that does not exist is refused with InvalidParameterValueException (Smithy: errorKind client, httpError 400) - AWS's message is the invalid-parameter sentence naming the inaccessible queue ARN, whose variable part is the EventSourceArn from the request.
Emulator: the call succeeds with HTTP 202 and a full EventSourceMappingConfiguration - UUID, BatchSize 10, EventSourceArn 'arn:aws:sqs:us-west-2:000000000000:nonexistent-queue-...', State 'Creating', StateTransitionReason 'USER_INITIATED', EventSourceMappingArn. A mapping is minted against a queue that was never created.
Why: a typo in a queue name is a silent success here and a permanently broken mapping in AWS; infrastructure code that relies on the 400 to catch the typo at deploy time passes.
Fix: the CreateEventSourceMapping handler's validation - resolve the EventSourceArn in the SQS store and refuse with InvalidParameterValueException (400) and AWS's wording when it does not exist.
Traces: lambda__sns__sqs.esm-creation-fails-for-nonexistent-queue-arn, lambda__sns__sqs.sqs-lambda-esm-wrong-arn-mapping-fails, lambda__sns__sqs.sqs-lambda-esm-wrong-queue-arn-negative.

Cause 3 of 6: CreateEventSourceMapping does not authorize the function's execution role against the queue, and when it does refuse it uses the wrong error code.
AWS: CreateEventSourceMapping checks that the function's execution role can actually poll the queue (sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes). A role without them is refused with InvalidParameterValueException (400) - not ResourceNotFoundException (404), which is reserved for a function or event source that does not exist.
Emulator: the unauthorized mapping is created and merely returns its UUID ('312b7f30-c1a0-4675-b6f7-ba1ad982ba6f'); and in the sibling check, where a refusal does occur, the code is ResourceNotFoundException where AWS raises InvalidParameterValueException.
Why: a client cannot discover an under-privileged execution role at mapping-creation time, which is where AWS reports it; and when something is refused, the 404 code sends the caller to look for a missing resource instead of a missing permission, and is caught by different SDK error handling.
Fix: the CreateEventSourceMapping handler must evaluate the execution role's SQS permissions and classify the refusal as InvalidParameterValueException (400).
Traces: lambda__sns__sqs.sqs-to-lambda-esm-missing-permission, lambda__sns__sqs.iam-trust-sqs-wrong-principal-negative.

Cause 4 of 6: SetTopicAttributes accepts a *FeedbackRoleArn whose trust policy does not name sns.amazonaws.com.
AWS: SetTopicAttributes (Smithy: TopicArn + AttributeName required, AttributeValue; modeled errors AuthorizationErrorException 403, InvalidParameterException 400/InvalidParameter, InvalidSecurityException 403, NotFoundException 404) validates a delivery-status feedback role: SNS must be able to assume it, so a role whose trust policy names lambda.amazonaws.com, ec2.amazonaws.com or any principal other than sns.amazonaws.com is refused with InvalidParameter at HTTP 400. Fixed text is AWS's invalid-parameter wording; the variable parts are the attribute name and the role ARN from the request.
Emulator: accepted - HTTP 200 with an empty SetTopicAttributes body - for a role trusting lambda.amazonaws.com, for one trusting ec2.amazonaws.com, and for the positive-refusal case generally.
Why: the exact trust-policy mistake this validation exists to catch is stored silently, leaving a topic whose delivery-status logging is dead in AWS.
Fix: the SetTopicAttributes handler's validation for the *SuccessFeedbackRoleArn/*FailureFeedbackRoleArn attributes.
Traces: lambda__sns__sqs.iam-role-trust-mismatched-principal-sns-blocked, lambda__sns__sqs.iam-role-trust-sns-service-assume, lambda__sns__sqs.iam-trust-sns-wrong-principal-negative.

Cause 5 of 6: SNS->Lambda delivery never invokes the function, and Subscribe is not authorized against the topic policy.
AWS: Subscribe(Protocol='lambda') (Smithy: SubscribeInput.TopicArn/Protocol required; errors AuthorizationErrorException 403/AuthorizationError, InvalidParameterException 400, NotFoundException 404) plus lambda:InvokeFunction for sns.amazonaws.com scoped by SourceArn makes Publish invoke the function once per message with the SNS Records envelope; a grant scoped to a different topic, or a missing grant, blocks the invocation. Separately, a cross-account Subscribe against a topic whose policy grants the caller nothing is refused with AuthorizationError at HTTP 403.
Emulator: every delivery check runs to the harness cap and is terminated at 60 s (no invocation ever happens, so the permitted and the blocked arms are identical), and the cross-account Subscribe succeeds - returning a real SubscriptionArn at HTTP 200 - with no topic-policy grant at all.
Why: SNS fan-out to Lambda delivers nothing, and the resource-policy controls that are supposed to scope it are inert, so a client both loses the feature and gets false assurance from its negative tests.
Fix: the SNS Publish delivery path for lambda-protocol subscriptions (build the Records envelope, evaluate the target function's resource policy for sns.amazonaws.com/SourceArn, invoke) and the Subscribe handler's authorization against the topic resource policy.
Traces: lambda__sns__sqs.sns-to-lambda-subscription-basic, lambda__sns__sqs.snstrig-publish-invokes-the-function-with-the-sns-record-envelope, lambda__sns__sqs.sns-lambda-notification-message-attributes-filter, lambda__sns__sqs.sns-to-lambda-missing-invoke-permission, lambda__sns__sqs.sns-add-permission-source-arn-mismatch, lambda__sns__sqs.pipeline-sns-sqs-lambda, lambda__sns__sqs.shared-state-sns-fanout-two-lambdas-ddb, lambda__sns__sqs.cli-lambda-add-permission-sns-invoke-workflow, lambda__sns__sqs.cli-missing-lambda-permission-blocks-invoke-workflow, lambda__sns__sqs.sns-sqs-cross-account-missing-topic-policy-negative.

Cause 6 of 6: an IAM-role-trust positive control for Lambda/SQS never completes, so the trust differential is ungraded.
AWS: a role whose trust policy names lambda.amazonaws.com (or sqs.amazonaws.com) can be assumed by that service and the wired chain runs end to end; a stack or role with the wrong trusted principal fails. The positive arm must succeed for the negative arm to mean anything.
Emulator: both checks run to the harness cap and are terminated at 60 s - the positive-control chain never completes, so neither the correct-trust nor the mismatched-trust outcome is observable.
Why: a client cannot validate service-trust wiring at all: correct and incorrect trust policies are both silent.
Fix: the role-assumption/service-trust path used by the Lambda and SQS integration handlers, together with the CloudFormation provisioning of the roles under test.
Traces: lambda__sns__sqs.iam-role-trust-sqs-principal, lambda__sns__sqs.cfn-deploy-iam-role-trust-lambda-positive.

## Evidence
Cause 1 - "AssertionError: control ESM (Enabled=True, same wiring) delivered nothing in the window; the disabled mapping's silence over that same window is not evidence of anything without it"; the other eight ESM checks: "Error: Check timed out after 60.0s". Cause 2 - expected 400 InvalidParameterValueException, actual "AssertionError: expected the call to be rejected, but it succeeded with {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'UUID': 'a7dc4da6-...', 'EventSourceArn': 'arn:aws:sqs:us-west-2:000000000000:nonexistent-queue-4454ebacc173', 'State': 'Creating', 'StateTransitionReason': 'USER_INITIATED', ...}". Cause 3 - "AssertionError: expected the call to be rejected, but it succeeded with '312b7f30-c1a0-4675-b6f7-ba1ad982ba6f'" and "assert esm_err_code == 'InvalidParameterValueException' ... AssertionError: ResourceNotFoundException". Cause 4 - "AssertionError: SetTopicAttributes accepted a feedback role trusting lambda.amazonaws.com instead of sns.amazonaws.com", "... trusting ec2.amazonaws.com instead of sns.amazonaws.com", and "expected the call to be refused, but it succeeded: {'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}". Cause 5 - "AssertionError: expected the call to be rejected, but it succeeded with {'SubscriptionArn': 'arn:aws:sns:us-west-2:000000000000:sim2real-xsvc-topic-...:da909504-...', 'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}" plus "Error: Check timed out after 60.0s" on the nine delivery checks. Cause 6 - "Error: Check timed out after 60.0s" on both trust checks.

## Steps to Reproduce
Cause 1: CreateFunction + CreateQueue, CreateEventSourceMapping(BatchSize/MaximumBatchingWindowInSeconds/FilterCriteria/ScalingConfig as desired, Enabled=true), SendMessage - no invocation ever occurs, with or without Enabled=false.
Cause 2: CreateEventSourceMapping with EventSourceArn='arn:aws:sqs:<region>:<account>:a-queue-that-does-not-exist' - 202 with a mapping instead of 400 InvalidParameterValueException.
Cause 3: CreateFunction with an execution role lacking sqs:ReceiveMessage/DeleteMessage, then CreateEventSourceMapping on an existing queue - accepted; in the sibling path observe ResourceNotFoundException where InvalidParameterValueException is modeled.
Cause 4: CreateRole trusting lambda.amazonaws.com (or ec2.amazonaws.com), then SetTopicAttributes(AttributeName='LambdaSuccessFeedbackRoleArn'/'SQSSuccessFeedbackRoleArn', AttributeValue=<role ARN>) - 200.
Cause 5: Subscribe(Protocol='lambda', Endpoint=<function ARN>) + AddPermission for sns.amazonaws.com and Publish - no invocation; and Subscribe from another account's role against a topic with no policy grant - 200 with a SubscriptionArn instead of 403 AuthorizationError.
Cause 6: CreateRole trusting lambda.amazonaws.com / sqs.amazonaws.com and exercise the wired chain (directly and via CreateStack) - the positive control never completes.
