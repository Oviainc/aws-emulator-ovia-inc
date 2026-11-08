Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive|_request_until_logged#10|AssertionError|raise AssertionError(
Issue-Commit: 63b1f768cca526e685e8991a6e4ba2a43400c274

# apigateway/dynamodb/lambda/logs: a REST stage with AccessLogSettings delivers no access-log events to the configured log group

> _Note: this issue was identified against the code base at commit `63b1f768cca526e685e8991a6e4ba2a43400c274`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
One net-new root cause, grounded in a failing check the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 1 — a CloudFormation-deployed REST stage whose AccessLogSettings name a DestinationArn and a Format never writes an access-log event for the requests it serves.
AWS: recorded behavior only for the delivery itself — a stage configured with AccessLogSettings{DestinationArn: <log group arn>, Format: <$context template>} writes exactly one access-log event per request into that log group, and logs FilterLogEvents/GetLogEvents over the group returns one record per request rendered from the declared $context fields. Smithy (apigateway) carries the setting on the stage (Stage.accessLogSettings), and the account-level role that permits the push is Account.cloudwatchRoleArn as returned by UpdateAccount (PATCH /account).
Emulator: the configured log group stays empty — 16 identical requests through the stage over a 150 s budget produced no events at all, so the check could not grade the record contents.
Why: stage access logs are the primary record of what reached an API; with none delivered, no client can audit requests, correlate a gateway request id with a backend invocation, or verify that a declared custom format is honored. It also makes a correctly-configured stage indistinguishable from one whose logging is misconfigured.
Fix: the access-log emitter in the execute-api data plane — render the stage's AccessLogSettings.Format per request and deliver it to the DestinationArn log group through CloudWatch Logs (PutLogEvents), creating the log stream on first request.
Failing check: apigateway__dynamodb__lambda__logs.deploy-cfn-apigateway-access-logging-enabled-positive.

## Evidence
AssertionError: access log entries delivered to the configured log group not met within 150.0s after 16 identical requests (raised from the check's _request_until_logged helper). Expected at least one access-log event per served request in the AWS::Logs::LogGroup named by the stage's AccessLogSettings.DestinationArn, each rendered from the declared $context format; observed an empty log group. The 150 s / 16-request figures are the harness budget, not a requirement. The check carried no 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
CreateStack a template with an AWS::Logs::LogGroup, an AWS::ApiGateway::RestApi with a Lambda-proxy method, and an AWS::ApiGateway::Stage whose AccessLogSettings set DestinationArn to the log group and Format to a JSON $context template; set the account cloudwatchRoleArn to a role with AmazonAPIGatewayPushToCloudWatchLogs; issue repeated GETs against the stage URL and poll logs FilterLogEvents on the group -> expect one access-log record per request, observe no events.
