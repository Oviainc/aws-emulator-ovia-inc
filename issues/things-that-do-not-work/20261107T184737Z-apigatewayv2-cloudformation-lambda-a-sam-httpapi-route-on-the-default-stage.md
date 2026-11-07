Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route
Issue-Key: apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route|_poll#14|AssertionError|raise AssertionError(f"{what} not reached within {timeout_s}s; last {last!r}")
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# A SAM HttpApi route on the $default stage answers 500 forever instead of serving the function's payload-2.0 response

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (HTTP API $default stage route)

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: a route created by a SAM AWS::Serverless::Function HttpApi event never serves its integration — every GET on the $default stage answers HTTP 500 for the whole 120 s observation window.
Failing check ids for this cause: apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route.

AWS: recorded behavior only — the HTTP API data plane and the SAM HttpApi event expansion are not described by the ApiGatewayV2 Smithy model (SAM expands the event into an Api + Route + AWS_PROXY Integration with PayloadFormatVersion 2.0 on the implicit $default stage, plus the lambda invoke permission). The recording shows GET /hello on the $default stage answered HTTP 200 with the handler's body, the handler receiving the payload-format-2.0 envelope and Globals.Function MemorySize, Globals.HttpApi CORS answering only the allowed Origin, and an undeclared route answered 404.
Emulator: the route is reachable (so this is not the unattributable-address refusal) but answers HTTP 500 on every attempt — the check polls to its 120 s deadline and reports `last <Response [500]>`. No 200 is ever produced, so the payload-format assertions, the CORS assertions and the undeclared-route 404 assertion are never reached. The 500 carries no AWS error code tied to an integration failure the test set up; it is the only answer the route gives.
Why: a SAM-defined HTTP API is unusable: the published route returns a server error for a correctly declared, correctly permitted integration, so nothing deployed through the most common SAM pattern can be exercised, and a client cannot tell a real integration failure from this unconditional one.
Fix: the ApiGatewayV2 HTTP API execute-api invoke path for routes materialised by the SAM transform — resolve the $default stage's route, perform the AWS_PROXY invoke of the target function with a payload-format-2.0 event, and map the handler's statusCode/headers/body back onto the HTTP response; a 500 must only be produced when the integration itself fails.
Traces: apigatewayv2__cloudformation__lambda.sam-httpapi-event-serves-payload-v2-route.

## Evidence
Expected (recorded AWS): GET /hello on the $default stage -> HTTP 200 with the handler's body; the event is payload format 2.0; CORS answers only the allowed Origin; an undeclared route -> 404.

Actual (emulator): AssertionError: GET /hello answering 200 not reached within 120.0s; last <Response [500]>

## Steps to Reproduce
1. CreateStack (CAPABILITY_IAM, CAPABILITY_AUTO_EXPAND) with a SAM template whose AWS::Serverless::Function declares an HttpApi event for GET /hello, with Globals.Function.MemorySize and Globals.HttpApi.CorsConfiguration set.
2. Wait for CREATE_COMPLETE and read the HTTP API's $default stage endpoint.
3. GET /hello with an Origin header.
4. Observe HTTP 500 on every attempt instead of 200 with the handler's body.
Or replay the check id in trace_ids.
