Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-dynamodb-direct-integration-positive apigateway__dynamodb__lambda__logs.deploy-cfn-missing-lambda-permission-for-apigateway-negative apigateway__dynamodb__lambda__logs.deploy-missing-lambda-permission-blocks-apigateway-invoke-negative apigateway__dynamodb__lambda__logs.deploy-wrong-integration-uri-negative apigateway__dynamodb__lambda__logs.deploy-cfn-serverless-api-full-stack-positive apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-lambda apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-roundtrip apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected apigateway__dynamodb__lambda__logs.apigateway-missing-cloudwatch-role-negative apigateway__dynamodb__lambda__logs.iam-role-missing-apigateway-trust-negative apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative apigateway__dynamodb__lambda__logs.apigw-log-group-arn-mismatch-negative apigateway__dynamodb__lambda__logs.apigateway-access-log-wrong-destination-arn-negative apigateway__dynamodb__lambda__logs.apigateway-log-group-resource-policy-missing-negative apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage apigateway__dynamodb__lambda__logs.apigw-access-logs-custom-format apigateway__dynamodb__lambda__logs.apigw-execution-logs-delivered-to-cloudwatch apigateway__dynamodb__lambda__logs.apigw-http-api-access-logs-json-format apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected apigateway__dynamodb__lambda__logs.deploy-full-stack-apigateway-lambda-dynamodb-logs apigateway__dynamodb__lambda__logs.apigw-dynamodb-batch-write-integration-positive apigateway__dynamodb__lambda__logs.apigw-dynamodb-getitem-integration-positive apigateway__dynamodb__lambda__logs.apigw-dynamodb-missing-execution-role-permission-negative apigateway__dynamodb__lambda__logs.apigw-dynamodb-putitem-integration-positive apigateway__dynamodb__lambda__logs.apigw-dynamodb-query-integration-response-mapping-positive apigateway__dynamodb__lambda__logs.apigw-dynamodb-wrong-table-arn-in-integration-negative apigateway__dynamodb__lambda__logs.ddb-esm-batch-size-one-record-per-invocation-positive apigateway__dynamodb__lambda__logs.ddb-esm-destination-on-failure-missing-config-negative apigateway__dynamodb__lambda__logs.ddb-esm-lambda-missing-logs-permission-negative apigateway__dynamodb__lambda__logs.ddb-esm-tumbling-window-aggregation-positive apigateway__dynamodb__lambda__logs.iam-role-missing-lambda-trust-negative apigateway__dynamodb__lambda__logs.iam-role-wrong-principal-service-negative apigateway__dynamodb__lambda__logs.iam-role-wrong-trust-principal-apigateway-negative
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive|_request_until_logged#10|AssertionError|assert status == 200, f"expected 200 from GET {url} on attempt {attempts}, got {status}: {body[:300]!r}"
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-dynamodb-direct-integration-positive|_poll_until#6|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-missing-lambda-permission-for-apigateway-negative|_poll_until#7|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-missing-lambda-permission-blocks-apigateway-invoke-negative|run#8|AssertionError|assert status == 500, f"expected 500 (Lambda unreachable, no permission), got {status}: {body}"
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-wrong-integration-uri-negative|run#11|AssertionError|assert status == 500, f"expected 500 (integration target does not exist), got {status}: {body}"
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-serverless-api-full-stack-positive|_poll_until#6|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-lambda|run#28|AssertionError|assert result["status"] == 200, f"unexpected integration status: {result}"
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-roundtrip|run#25|AssertionError|assert status == 200, f"unexpected status {status}: {body_text}"
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-missing-lambda-permission-rejected|run#32|AssertionError|assert allowed_result["status"] == 200, f"expected 200 after granting permission, got {allowed_result}"
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-missing-cloudwatch-role-negative|_error_code#6|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-missing-apigateway-trust-negative|_expect_update_account_rejected#13|AssertionError|raise AssertionError(
Issue-Key: apigateway__dynamodb__lambda__logs|UpdateAccount|BadRequestException
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-log-group-arn-mismatch-negative|run#19|AssertionError|assert "requestId" in error_message and "extendedRequestId" in error_message, (
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-access-log-wrong-destination-arn-negative|_poll_log_group_created#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-log-group-resource-policy-missing-negative|_poll_log_group_messages#18|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage|_wait_for_200#14|Timeout|time.sleep(5)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-access-logs-custom-format|_wait_for_200#19|Timeout|time.sleep(5)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-execution-logs-delivered-to-cloudwatch|_wait_for_200#10|Timeout|time.sleep(5)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-http-api-access-logs-json-format|_poll_http_get#23|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.cli-apigateway-stage-access-log-missing-destination-rejected|_poll_for_log_events#35|Timeout|time.sleep(8)
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-full-stack-apigateway-lambda-dynamodb-logs|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-dynamodb-batch-write-integration-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-dynamodb-getitem-integration-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-dynamodb-missing-execution-role-permission-negative|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-dynamodb-putitem-integration-positive|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-dynamodb-query-integration-response-mapping-positive|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-dynamodb-wrong-table-arn-in-integration-negative|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.ddb-esm-batch-size-one-record-per-invocation-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.ddb-esm-destination-on-failure-missing-config-negative|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.ddb-esm-lambda-missing-logs-permission-negative|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.ddb-esm-tumbling-window-aggregation-positive|run#13|Timeout|time.sleep(send_interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-missing-lambda-trust-negative|_retry_negative_create#10|Timeout|time.sleep(3.0)
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-wrong-principal-service-negative|_retry_negative_update#14|Timeout|time.sleep(3.0)
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-wrong-trust-principal-apigateway-negative|_poll_http_get#32|Timeout|time.sleep(interval_s)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# API Gateway deployed-stage invoke, account logging role and log delivery: 9 causes (34 checks)

> _Causes 1–5 are FIXED. Causes 6–9 are not, and are re-filed with their own replay keys as `issues/things-that-do-not-work/20261029T180026Z-apigateway-dynamodb-lambda-logs-4-of-the-9-causes-remain-open.md` so they are not recorded as working: access-log and execution-log delivery to CloudWatch Logs, the REST `AWS` DynamoDB integration, the DynamoDB-Streams event source mapping, and the lambda-side trust negatives. That report also carries what the fix of causes 1–5 established about each of them._

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Nine net-new root causes in the apigateway__dynamodb__lambda__logs group. Each needs its own fix.

Cause 1 of 9: the deployed execute-api invoke URL is not routed.
AWS: a deployed stage answers requests on its invoke URL with the method's integration result (200 for a wired method, 403/500 for the negative cases).
Emulator: HTTP 404 with body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}` — the front-door router never recognises the `/_aws/execute-api/<api-id>/<stage>/<path>` request as API Gateway at all.
Why: no end-to-end API Gateway traffic is possible; every negative case that expects 403/500 from the gateway instead gets a 404 from the router, so a client cannot tell a missing permission from a missing API.
Fix: the request router's execute-api path recognition (dispatch to the apigateway data plane).
Traces: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive, .deploy-cfn-apigateway-dynamodb-direct-integration-positive, .deploy-cfn-missing-lambda-permission-for-apigateway-negative, .deploy-missing-lambda-permission-blocks-apigateway-invoke-negative, .deploy-wrong-integration-uri-negative, .deploy-cfn-serverless-api-full-stack-positive

Cause 2 of 9: TestInvokeMethod on an AWS_PROXY integration always answers 500.
AWS: TestInvokeMethod of a Lambda-proxy method returns the function's mapped response (200 with the handler's body), and 403 when the resource policy denies.
Emulator: status 500, body `{"message":"Internal server error"}`, and the execution log records `Execution failed due to configuration error: the AWS_PROXY integration names a backend this emulator cannot call` / `Answered through this API's API_CONFIGURATION_ERROR gateway response`.
Why: the whole proxy-integration surface is unusable, and the permission-denied negative case is indistinguishable from the configuration error. The open issue 20261027T053218Z-apigateway-dynamodb-lambda-logs-a-cli-wired-aws.md carries one sibling check but not these.
Fix: the TestInvokeMethod handler's AWS_PROXY integration path.
Traces: apigateway__dynamodb__lambda__logs.cli-apigateway-create-deploy-invoke-lambda, .cli-apigateway-create-deploy-invoke-roundtrip, .cli-apigateway-missing-lambda-permission-rejected

Cause 3 of 9: UpdateAccount accepts a cloudwatchRoleArn that cannot be assumed.
AWS: `UpdateAccount` validates the role: a role whose trust policy does not name `apigateway.amazonaws.com`, or that does not exist, is refused with `BadRequestException` (HTTP 400).
Emulator: the call SUCCEEDS.
Why: misconfigured logging is accepted silently, so the operator never learns the role is unusable.
Fix: UpdateAccount validation (role existence + trust-policy principal check).
Traces: apigateway__dynamodb__lambda__logs.apigateway-missing-cloudwatch-role-negative, .iam-role-missing-apigateway-trust-negative

Cause 4 of 9: UpdateAccount rejects a well-formed cloudwatchRoleArn.
AWS: an IAM role ARN of the documented form is accepted.
Emulator: `BadRequestException` `Invalid role ARN specified: /cloudwatchRoleArn takes an IAM role ARN of the form arn:{partition}:iam::{account}:role/{name}` for an ARN the recording shows AWS accepting.
Why: the account-level logging role cannot be configured at all — the inverse of cause 3 and equally blocking.
Fix: UpdateAccount's cloudwatchRoleArn parser.
Traces: apigateway__dynamodb__lambda__logs.apigw-missing-cloudwatch-role-arn-no-logs-negative

Cause 5 of 9: the access-log-format validation message names only one required variable.
AWS: when `accessLogSettings.format` omits the required request identifiers, the `BadRequestException` message names both `$context.requestId` and `$context.extendedRequestId`.
Emulator: `/accessLogSettings/format must include at least $context.requestId, got "<the supplied format>"` — names only requestId and echoes the format.
Why: the caller is told to add one variable when AWS requires either of two, so the fix it applies can still be rejected.
Fix: the stage access-log-settings validator's message.
Traces: apigateway__dynamodb__lambda__logs.apigw-log-group-arn-mismatch-negative

Cause 6 of 9: access logs are never delivered to CloudWatch Logs, so the checks never finish.
AWS: a stage with accessLogSettings writes one log event per request to the destination log group, and creates the per-stage group.
Emulator: nothing is ever written; the check polls the destination until the 60s budget expires (`Check timed out after 60.0s`).
Why: no access-log consumer works, and the negative cases (wrong destination ARN, missing log-group resource policy, missing destination) cannot be distinguished from a hang.
Fix: the apigateway stage access-log emitter (and the cloudwatchlogs destination it writes to).
Traces: apigateway__dynamodb__lambda__logs.apigateway-access-log-wrong-destination-arn-negative, .apigateway-log-group-resource-policy-missing-negative, .apigw-access-log-group-created-per-stage, .apigw-access-logs-custom-format, .apigw-execution-logs-delivered-to-cloudwatch, .apigw-http-api-access-logs-json-format, .cli-apigateway-stage-access-log-missing-destination-rejected, .deploy-full-stack-apigateway-lambda-dynamodb-logs

Cause 7 of 9: the REST `AWS` (non-proxy) DynamoDB integration never answers.
AWS: a method with an `AWS` integration to DynamoDB answers with the mapped DynamoDB reply (and 403/500 for the missing-role and wrong-table-ARN negatives).
Emulator: the checks hang to the 60s budget without an answer.
Why: the compute-less integration pattern is unusable in every direction.
Fix: the apigateway AWS-integration invoke path (request/response mapping templates and the signed service call).
Traces: apigateway__dynamodb__lambda__logs.apigw-dynamodb-batch-write-integration-positive, .apigw-dynamodb-getitem-integration-positive, .apigw-dynamodb-missing-execution-role-permission-negative, .apigw-dynamodb-putitem-integration-positive, .apigw-dynamodb-query-integration-response-mapping-positive, .apigw-dynamodb-wrong-table-arn-in-integration-negative

Cause 8 of 9: DynamoDB-Streams event source mappings never deliver, so ESM checks hang.
AWS: an enabled ESM polls the stream and invokes the function, honouring BatchSize, the on-failure destination and tumbling windows.
Emulator: no delivery occurs; the checks time out at 60s.
Why: every stream-driven workload is silently inert — writes are accepted but nothing downstream runs.
Fix: the lambda event-source-mapping poller for DynamoDB streams.
Traces: apigateway__dynamodb__lambda__logs.ddb-esm-batch-size-one-record-per-invocation-positive, .ddb-esm-destination-on-failure-missing-config-negative, .ddb-esm-lambda-missing-logs-permission-negative, .ddb-esm-tumbling-window-aggregation-positive

Cause 9 of 9: IAM trust-policy negatives on the logging/invoke roles hang.
AWS: creating or using a role whose trust policy names the wrong service principal is refused promptly.
Emulator: the checks hang to the 60s budget rather than being answered either way.
Why: a caller gets no answer at all for a plain misconfiguration.
Fix: the role-trust evaluation on the path these checks drive (apigateway/lambda service-principal assumption).
Traces: apigateway__dynamodb__lambda__logs.iam-role-missing-lambda-trust-negative, .iam-role-wrong-principal-service-negative, .iam-role-wrong-trust-principal-apigateway-negative

## Evidence
Representative replay details:
- Cause 1: AssertionError: expected 200 from GET https://<host>/_aws/execute-api/<api-id>/prod/ping on attempt 1, got 404: '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'. Negative cases: "expected 500 (Lambda unreachable, no permission), got 404: {'__type': 'UnknownService', ...}" and "expected 500 (integration target does not exist), got 404: ...".
- Cause 2: AssertionError: unexpected status 500: {"message":"Internal server error"} — with the execution log line 'Execution failed due to configuration error: the AWS_PROXY integration names a backend this emulator cannot call: no deployment package runs in process and no outbound connection is made'.
- Cause 3: AssertionError: expected the call to be rejected, but it succeeded / update_account unexpectedly succeeded with a role missing the apigateway.amazonaws.com trust principal.
- Cause 4: botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the UpdateAccount operation: Invalid role ARN specified: /cloudwatchRoleArn takes an IAM role ARN of the form arn:{partition}:iam::{account}:role/{name}.
- Cause 5: AssertionError: expected the BadRequestException to name both $context.requestId and $context.extendedRequestId, got message: '/accessLogSettings/format must include at least $context.requestId, got "{...}"'.
- Causes 6-9: Error: Check timed out after 60.0s for each listed check.

## Steps to Reproduce
Boot the emulator and replay the 34 checks listed under Traces for each cause. Cause 1 is reproduced by deploying any REST API stage and issuing a plain HTTPS GET to its invoke URL; cause 2 by calling TestInvokeMethod on an AWS_PROXY method; causes 3/4 by UpdateAccount with and without a well-formed, trusting cloudwatchRoleArn; cause 5 by PutStage/UpdateStage with an accessLogSettings.format missing the request identifiers; causes 6-9 by the listed polling checks, which currently never complete.
