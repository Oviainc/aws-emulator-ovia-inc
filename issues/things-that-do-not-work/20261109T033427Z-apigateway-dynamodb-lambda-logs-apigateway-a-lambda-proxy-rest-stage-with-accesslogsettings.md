Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigw-access-logs-custom-format
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-access-logs-custom-format|_poll_log_events_with_traffic#23|Timeout|resp = logs.filter_log_events(logGroupName=log_group_name, startTime=start_time_ms)
Issue-Commit: aba2adfa3005273c5f6b7f8438c028c27604934b

# apigateway: a Lambda-proxy REST stage with accessLogSettings (custom $context JSON format) never delivers an access-log event to its CloudWatch Logs destination — the check never terminates (1 cause)

> _Note: this issue was identified against the code base at commit `aba2adfa3005273c5f6b7f8438c028c27604934b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: a deployed REST API stage whose accessLogSettings name a CloudWatch Logs group and a custom JSON $context format produces no access-log event for the requests it serves, so the leg that waits for one never completes. Failing check: apigateway__dynamodb__lambda__logs.apigw-access-logs-custom-format.

RELATED, NOT COVERING: the open issue 20261026T072730Z-apigateway-dynamodb-lambda-logs-apigateway-a-stage-with-accesslogsettings-custom-context-json.md describes the same class of defect but carries only the replay key of a different check (apigateway-access-logs-custom-format-positive, a mock-integration stage with a 60 s cap). The replay suite does NOT report this check as owned by it, so it is filed here; a fixer should treat the two together. That sibling exercises a mock integration, this one exercises an AWS_PROXY (Lambda) integration, so the two can diverge in the stage-serving leg even if the log-emission leg is shared.

AWS: Smithy (com.amazonaws.apigateway) models Stage.accessLogSettings -> AccessLogSettings{format, destinationArn}. `format` is documented as "A single line format of the access logs of data, as specified by selected $context variables. The format must include at least $context.requestId"; `destinationArn` as "The Amazon Resource Name (ARN) of the CloudWatch Logs log group or Kinesis Data Firehose delivery stream to receive access logs". With the account's cloudwatchRoleArn set to a role that trusts apigateway.amazonaws.com and carries AmazonAPIGatewayPushToCloudWatchLogs, each request served by that stage causes API Gateway to write exactly ONE log event into the named log group, with every $context.* reference in the format string replaced by that request's value — never left as the literal template text. For the format this check sets, the delivered event is a JSON object whose fixed text is the operator-supplied format string (the member names requestId/httpMethod/resourcePath/status) and whose variable parts come from the request and the stage's routing: `requestId` equals the x-amzn-RequestId the stage returned for that request, `httpMethod` is the request method (GET), `resourcePath` is the matched resource path (here /echo), and `status` is the decimal HTTP status the stage returned as a string ("200"). UpdateStage accepting the two patchOperations (replace /accessLogSettings/destinationArn and replace /accessLogSettings/format) with a 200 is the configuration contract; the delivery of the event is the behavioral contract. Delivery latency is recorded behavior only; what is structural is that the invoke returns an HTTP status and that an access-log event appears for it.

Emulator: the replay reports exactly `Error: Check timed out after 180.0s`. Nothing is refused — there is no error code and no non-2xx to quote for the logging configuration — the awaited effect simply never occurs: the check drives GET traffic through the deployed stage (re-driving it every 20 s on its own budget) and polls logs.FilterLogEvents on the destination group, and no event carrying any of the x-amzn-RequestId values the emulator handed back ever appears, so the claim is never graded: neither the presence of a record nor the substitution of $context.requestId / $context.httpMethod / $context.resourcePath / $context.status. Because the poll has no terminating condition other than the harness cap, the whole check hangs instead of failing fast.

Why: access logging is the primary post-deployment audit and debugging surface of a REST API. A client that enables accessLogSettings on a stage sees UpdateStage accepted and then finds an empty log group — it cannot distinguish a mis-set destinationArn, a missing account CloudWatch role, a stage serving no traffic, or an un-rendered format template. Automation that waits for the access-log record of a request (deployment smoke tests, request tracing, compliance evidence) stalls rather than reporting a failure.

Fix: the API Gateway stage's access-log emission path — the UpdateStage handler's accessLogSettings state plus the execute-api request-serving path for AWS_PROXY integrations, which must render the format template per served request and PutLogEvents it into the destinationArn log group (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/ and the execute-api data-plane serving path).

Traces: apigateway__dynamodb__lambda__logs.apigw-access-logs-custom-format.

## Evidence
Expected (AWS): after update_stage(patchOperations=[{op: replace, path: /accessLogSettings/destinationArn, value: arn:aws:logs:<region>:<account>:log-group:<group>}, {op: replace, path: /accessLogSettings/format, value: '{"requestId":"$context.requestId","httpMethod":"$context.httpMethod","resourcePath":"$context.resourcePath","status":"$context.status"}'}]) and a GET on <invoke-url>/echo that returns 200 with an x-amzn-RequestId header, logs.filter_log_events on <group> returns at least one event whose message parses as JSON with requestId == that request's x-amzn-RequestId, httpMethod == "GET", resourcePath == "/echo" and status == "200" (member names are the format's fixed text; the four values come from the request and the stage's routing).

Actual (emulator): `Error: Check timed out after 180.0s`. No access-log event is ever delivered for any request id the emulator returned, so the assertions `len(events) > 0` and `no delivered access-log entry's requestId matched a request this check actually sent` are never reached within the budget. No error code and no non-2xx response is produced for the logging configuration itself — UpdateStage is accepted and the effect never materializes.

## Steps to Reproduce
1. Create an IAM role trusting apigateway.amazonaws.com, attach arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs, and set it via apigateway.update_account(patchOperations=[{op: replace, path: /cloudwatchRoleArn, value: <roleArn>}]).
2. Create a Lambda execution role (AWSLambdaBasicExecutionRole) and a python3.12 echo function returning {statusCode: 200, body: ...}; wait for State == Active.
3. create_rest_api, create_resource(pathPart="echo"), put_method(GET, authorizationType=NONE), put_integration(type=AWS_PROXY, integrationHttpMethod=POST, uri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>/invocations), and lambda.add_permission for principal apigateway.amazonaws.com with SourceArn arn:aws:execute-api:<region>:<account>:<apiId>/*/*.
4. create_deployment(restApiId=<apiId>, stageName="prod").
5. logs.create_log_group(logGroupName=<group>) and build destinationArn = arn:aws:logs:<region>:<account>:log-group:<group> (no trailing ':*').
6. update_stage(restApiId=<apiId>, stageName="prod", patchOperations=[{replace, /accessLogSettings/destinationArn, <arn>}, {replace, /accessLogSettings/format, '{"requestId":"$context.requestId","httpMethod":"$context.httpMethod","resourcePath":"$context.resourcePath","status":"$context.status"}'}]).
7. GET <invoke-url>/echo until it returns 200; record the x-amzn-RequestId header.
8. Poll logs.filter_log_events(logGroupName=<group>, startTime=<ms>) while re-driving the GET every 20 s; observe that no event ever arrives and the check exhausts the 180 s harness budget.
9. Or replay the check id in trace_ids.
