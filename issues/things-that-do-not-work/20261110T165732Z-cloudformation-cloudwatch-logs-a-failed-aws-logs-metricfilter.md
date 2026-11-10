Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__logs.deploy-logs-3-invalid-filter-pattern-rejected-rolls-back cloudformation__cloudwatch__logs.deploy-logs-3-invalid-pattern-update-rejected-keeps-old-filter
Issue-Key: cloudformation__cloudwatch__logs.deploy-logs-3-invalid-filter-pattern-rejected-rolls-back|run#27|AssertionError|assert status_code == "400", (
Issue-Key: cloudformation__cloudwatch__logs.deploy-logs-3-invalid-pattern-update-rejected-keeps-old-filter|run#25|AssertionError|assert handler_message == expected, (
Issue-Commit: 0c065617979d1ee76fa77b423bd58183fc04c861

# cloudformation__cloudwatch__logs: a failed AWS::Logs::MetricFilter reason omits the `Invalid request provided: <Type>.` prefix and the `(Service: ..., Status Code: 400, Request ID: ...)` suffix

> _Note: this issue was identified against the code base at commit `0c065617979d1ee76fa77b423bd58183fc04c861`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricFilter

## Description
One net-new root cause, grounded in two failing checks (one on the create path, one on the update path).

Cause 1 of 1: when the AWS::Logs::MetricFilter resource handler fails because CloudWatch Logs refused PutMetricFilter, the ResourceStatusReason CloudFormation publishes is missing both the resource-type prefix and the downstream service-error envelope.

AWS: Smithy — CloudWatch Logs `PutMetricFilter` declares `InvalidParameterException` (client error, HTTP 400) and `filterPattern` is a required member; an unparsable pattern such as `{ $.latency = }` is refused with that error. The recording shows CloudFormation wrapping that refusal in its documented reason shape: `Resource handler returned message: "Invalid request provided: AWS::Logs::MetricFilter. Invalid metric filter pattern ... (Service: CloudWatchLogs, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`. The FIXED text of that template is `Resource handler returned message: "`, `Invalid request provided: `, the literal resource type, `(Service: `, `, Status Code: `, `, Request ID: `, `" (RequestToken: `, `, HandlerErrorCode: `. The VARIABLE parts are the resource type name taken from the template, the service's own message, the status code (400 here), the downstream request id and the handler request token — none of a single run's id values is part of the requirement.

Emulator: the reason is `Resource handler returned message: "Invalid metric filter pattern" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`. The `Invalid request provided: AWS::Logs::MetricFilter.` prefix is absent and there is no `(Service: ..., Status Code: ..., Request ID: ...)` group at all, so parsing the reason for `Status Code` yields nothing. The HandlerErrorCode (`InvalidRequest`) and the surrounding rollback behaviour are correct; only the message body diverges. The same wording is served on the UPDATE_FAILED path.

Why: tooling and operators read the service name, HTTP status and downstream request id out of this reason to attribute a stack failure to the API call that caused it, and CDK/CloudFormation consoles parse the `Invalid request provided: <Type>.` prefix to attribute it to a resource type. With both missing, a stack failure cannot be traced back to the PutMetricFilter rejection, and a client asserting on the documented reason shape finds no match.

Fix: the resource-handler error formatter used by the AWS::Logs::MetricFilter handler — it must prefix the downstream message with `Invalid request provided: <resource type>.` and append the `(Service: <service>, Status Code: <status>, Request ID: <id>)` group taken from the refused CloudWatch Logs call, on both the create and the update path.

Traces: cloudformation__cloudwatch__logs.deploy-logs-3-invalid-filter-pattern-rejected-rolls-back, cloudformation__cloudwatch__logs.deploy-logs-3-invalid-pattern-update-rejected-keeps-old-filter.

## Evidence
Create path — expected the CREATE_FAILED ResourceStatusReason to carry a `Status Code` of `400` inside a `(Service: ..., Status Code: 400, Request ID: ...)` group.
Actual (check deploy-logs-3-invalid-filter-pattern-rejected-rolls-back, verdict mismatch):
```
AssertionError: MetricFilter CREATE_FAILED reason carried downstream Status Code '<no Status Code in reason>' (reason 'Resource handler returned message: "Invalid metric filter pattern" (RequestToken: ..., HandlerErrorCode: InvalidRequest)'); PutMetricFilter rejects the pattern with HTTP 400
```

Update path — expected handler message prefix `Resource handler returned message: "Invalid request provided: AWS::Logs::MetricFilter. Invalid metric filter pattern`.
Actual (check deploy-logs-3-invalid-pattern-update-rejected-keeps-old-filter, verdict mismatch):
```
AssertionError: MetricFilter UPDATE_FAILED reason 'Resource handler returned message: "Invalid metric filter pattern" (RequestToken: ..., HandlerErrorCode: InvalidRequest)'; expected the handler message 'Resource handler returned message: "Invalid request provided: AWS::Logs::MetricFilter. Invalid metric filter pattern'
```
(The RequestToken values are per-run and are not part of the requirement.)

## Steps to Reproduce
1. CreateStack with an AWS::Logs::LogGroup, an AWS::Logs::LogStream and an AWS::Logs::MetricFilter whose FilterPattern is `{ $.latency = }`.
2. Wait for ROLLBACK_COMPLETE and read the MetricFilter's CREATE_FAILED ResourceStatusReason from DescribeStackEvents: it has no `(Service: ..., Status Code: 400, Request ID: ...)` group and no `Invalid request provided: AWS::Logs::MetricFilter.` prefix.
3. Deploy the same stack with a valid FilterPattern (`{ $.latency > 0 }`), then UpdateStack to the invalid pattern and read the UPDATE_FAILED ResourceStatusReason — same wording defect.
