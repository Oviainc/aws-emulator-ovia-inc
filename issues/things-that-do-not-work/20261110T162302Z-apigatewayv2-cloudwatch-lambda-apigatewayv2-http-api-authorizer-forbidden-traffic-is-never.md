Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid
Issue-Key: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid|_poll_until#24|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# apigatewayv2 HTTP API authorizer-forbidden traffic is never counted: AWS/ApiGateway Count/4xx stay empty under {ApiId, Stage}

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: an HTTP API stage fronted by a Lambda authorizer meters none of its traffic.
AWS: every request an HTTP API stage answers — including the ones a Lambda authorizer denies with 403 — produces one Count datapoint and one lower-case '4xx' datapoint in the AWS/ApiGateway namespace under exactly the {ApiId, Stage} dimension set. Metric emission is not in the Smithy model, so this is documented/recorded behavior only; the recording shows Count Sum equal to the total number of requests served (18 in the recorded run) and the 4xx share matching the 403s.
Emulator: GetMetricStatistics over AWS/ApiGateway Count with Dimensions=[{ApiId},{Stage}] returns no datapoints for 240s after the requests were served ('Last result: (0, 0)'), so neither the allowed nor the authorizer-denied traffic appears.
Why: a client cannot tell an authorizer that denies everything from an API with no traffic; 4xx-rate alarms and dashboards over a protected HTTP API read zero forever.
Fix: the apigatewayv2 data-plane request/authorizer path (services/service-com-amazonaws-apigatewayv2-apigatewayv2) must publish Count and the 4xx/5xx class metrics into the cloudwatch metric store (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801) under {ApiId, Stage} for authorizer-rejected requests as well as integration-served ones.
Why this is not covered by an existing issue: the open issue 20261109T154749Z-apigatewayv2-cloudwatch-lambda-2-net-new-causes-the.md carries three other checks of this heading but not this replay key.
Traces: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid.

## Evidence
Expected: AWS/ApiGateway Count Sum for Dimensions=[{'Name':'ApiId',...},{'Name':'Stage','Value':'live'}] reaching the number of requests served (18 in the recorded run); actual: AssertionError "Count Sum [{'Name': 'ApiId', ...}, {'Name': 'Stage', 'Value': 'live'}] reaching 18 not met within 240s. Last result: (0, 0)".

## Steps to Reproduce
1. CreateApi (HTTP), attach a REQUEST Lambda authorizer to a route, CreateStage and deploy.
2. Send a mix of authorized requests (200) and unauthorized requests (403 from the authorizer) to the invoke URL.
3. Poll cloudwatch GetMetricStatistics, Namespace AWS/ApiGateway, MetricName Count then '4xx', Statistics=[Sum], Dimensions=[{ApiId},{Stage}] -> emulator returns no datapoints (0) for the whole window.
