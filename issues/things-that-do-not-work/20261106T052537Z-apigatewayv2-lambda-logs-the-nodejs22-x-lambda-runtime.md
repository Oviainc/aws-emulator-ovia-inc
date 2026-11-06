Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error
Issue-Key: apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error|run#6|AssertionError|assert ping == (200, None, {"statusCode": 400, "body": "Unknown action"}), f"ping invoke returned {ping!r}"
Issue-Commit: 1be7cfa8c8ca9e4bb16e8074ed05071db555de6e

# apigatewayv2__lambda__logs: the nodejs22.x Lambda runtime ships no AWS SDK for JavaScript v3, so a handler that requires @aws-sdk/client-dynamodb faults Runtime.ImportModuleError instead of returning its own statusCode

> _Note: this issue was identified against the code base at commit `1be7cfa8c8ca9e4bb16e8074ed05071db555de6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause under the suite heading `apigatewayv2__lambda__logs`, grounded in the failing check `direct-invoke-returns-handler-status-codes-without-a-function-error`. It is not carried by any open issue: the many filed "the Lambda python runtime provides no AWS SDK" issues are all scoped to the python runtimes and to other service headings, and `20261106T035624Z-dynamodb-lambda-rekognition-s3-2-net-new-causes.md` (which could only hypothesise between a missing node SDK and a stubbed Rekognition DetectLabels, because its log tail was empty) is attributed to a different check under a different heading. This replay settles that ambiguity for the node runtime with the error payload itself.

Cause 1 of 1: a RequestResponse `Invoke` of a `nodejs22.x` ZipFile function whose handler `require`s the AWS SDK for JavaScript v3 clients faults at module init, because the emulator's managed node runtime does not provide those modules on the handler's resolution path.

AWS: Smithy: lambda `Invoke` -> `InvocationResponse`; `StatusCode` is the `httpResponseCode` (200 for a successful RequestResponse invocation), `FunctionError` is bound to the `X-Amz-Function-Error` response header and is present ONLY when the function itself errored, and `Payload` is the `httpPayload` carrying the handler's return value. The AWS-managed Node.js runtimes (nodejs18.x and later, including nodejs22.x) bundle the AWS SDK for JavaScript v3 in the runtime image and place it on the handler's module resolution path, so `require('@aws-sdk/client-dynamodb')`, `require('@aws-sdk/lib-dynamodb')` and `require('@aws-sdk/client-apigatewaymanagementapi')` resolve from a ZipFile deployment that ships only the handler file and no node_modules. On AWS, therefore, this handler initialises, runs its own routing, and the invocation answers HTTP 200 with NO `X-Amz-Function-Error` header and the handler's own returned object as the payload — for an unrouted action `{"statusCode": 400, "body": "Unknown action"}`, and for the handler's caught-exception path `{"statusCode": 500, "body": "Internal server error"}`.

Emulator: the invocation answers HTTP 200 but with `FunctionError` = `Unhandled` and, as the payload, the Node runtime's init-failure record instead of the handler's return value: `{"errorMessage": "Error: Cannot find module '@aws-sdk/client-dynamodb'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-run id>", "trace": []}`. The fixed text here is the error class — `errorType` `Runtime.ImportModuleError` and an `errorMessage` of the form `Error: Cannot find module '<specifier>'`; the module specifier is the value taken from the handler's `require`, and the `requestId` is a per-run value. No handler code runs at all, so neither the 400 nor the 500 body is ever produced.

Why: a client gets a function error for a handler that AWS runs to completion, and gets it for the single most common shape of Node Lambda code — a ZipFile handler that calls another AWS service through the runtime-bundled SDK v3. Every effect downstream of such a handler is unreachable (no DynamoDB write, no PostToConnection reply, no handler-authored status code), and the only diagnosis the caller sees is a missing-module error for a module AWS guarantees is present. It also makes any `FunctionError`-free assertion on a node function impossible, so whole scenario families burn their budget at their first control step.

Fix: the lambda Invoke execution environment for the node runtimes — the runtime image/layer that backs `nodejs22.x` must provide the AWS SDK for JavaScript v3 (`@aws-sdk/*` clients and `@aws-sdk/lib-dynamodb`) on the handler's module resolution path (NODE_PATH), the same way the managed runtime does, and those clients must be able to reach the emulator's own service endpoints. The surrounding Invoke response assembly (StatusCode / FunctionError / Payload) is correct in shape; it is the environment that is incomplete.

Traces: apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error

## Evidence
Check `apigatewayv2__lambda__logs.direct-invoke-returns-handler-status-codes-without-a-function-error`, verdict mismatch, at its first assertion:

    assert ping == (200, None, {"statusCode": 400, "body": "Unknown action"}), f"ping invoke returned {ping!r}"
    AssertionError: ping invoke returned (200, 'Unhandled', {'errorMessage': "Error: Cannot find module '@aws-sdk/client-dynamodb'", 'errorType': 'Runtime.ImportModuleError', 'requestId': 'e57746bf-ee75-4a6a-9bf0-e960b8c8048e', 'trace': []})

(The requestId above is a per-run value, not a requirement.)

The function under test: `CreateFunction(Runtime='nodejs22.x', Handler='agent-message.handler', Code={'ZipFile': <zip containing only agent-message.js>}, Timeout=30, MemorySize=128)`. The handler's first three statements are:

    const { DynamoDBClient } = require('@aws-sdk/client-dynamodb');
    const { DynamoDBDocumentClient, PutCommand, GetCommand, UpdateCommand, DeleteCommand, ScanCommand } = require('@aws-sdk/lib-dynamodb');
    const { ApiGatewayManagementApiClient, PostToConnectionCommand } = require('@aws-sdk/client-apigatewaymanagementapi');

Expected (AWS): `Invoke` with the event `{"requestContext": {"connectionId": "direct-invoke", "routeKey": "$default"}, "body": "{\"action\": \"ping\"}"}` returns StatusCode 200, NO `FunctionError`, Payload `{"statusCode": 400, "body": "Unknown action"}`; and with `"action": "heartbeat"` returns StatusCode 200, NO `FunctionError`, Payload `{"statusCode": 500, "body": "Internal server error"}`.

Actual (emulator): StatusCode 200, `FunctionError` = `'Unhandled'`, Payload = the `Runtime.ImportModuleError` record for `@aws-sdk/client-dynamodb`. The handler never executes, so the heartbeat arm and the subsequent FilterLogEvents assertion on the logged `handleHeartbeat` stack are never reached (and are therefore not claimed here).

## Steps to Reproduce
1. Replay the behavioral suite and re-run the check id in trace_ids.
2. Create an IAM role trusted by lambda.amazonaws.com with AWSLambdaBasicExecutionRole attached.
3. Zip a single file `agent-message.js` whose first line is `const { DynamoDBClient } = require('@aws-sdk/client-dynamodb');` and which exports a `handler` that reads `JSON.parse(event.body).action` and returns `{statusCode: 400, body: 'Unknown action'}` for an unrouted action. Ship NO node_modules in the zip.
4. `CreateFunction(FunctionName=fn, Runtime='nodejs22.x', Role=<role arn>, Handler='agent-message.handler', Code={'ZipFile': <that zip>}, Timeout=30, MemorySize=128)` and wait until `GetFunctionConfiguration` reports State=Active.
5. `Invoke(FunctionName=fn, Payload=b'{"requestContext":{"connectionId":"direct-invoke","routeKey":"$default"},"body":"{\\"action\\":\\"ping\\"}"}')`.
6. Read `StatusCode`, whether `FunctionError` is present, and the decoded `Payload`. AWS: 200, no FunctionError, `{"statusCode":400,"body":"Unknown action"}`. Emulator: 200, `FunctionError='Unhandled'`, and a `Runtime.ImportModuleError` payload naming `@aws-sdk/client-dynamodb`.
