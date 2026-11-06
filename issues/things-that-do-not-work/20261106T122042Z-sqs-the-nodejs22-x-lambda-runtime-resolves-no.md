Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.query-compatible-error-carriers-differ-by-consumer-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.query-compatible-error-carriers-differ-by-consumer-rejected|run#35|AssertionError|assert invoked.get("FunctionError") is None, (
Issue-Commit: 746aa34634846a71d839d81493dd1fa245276952

# sqs: the nodejs22.x Lambda runtime resolves no bundled AWS SDK for JavaScript v3, so a JS v3 consumer cannot observe SQS's awsQueryCompatible error carriers

> _Note: this issue was identified against the code base at commit `746aa34634846a71d839d81493dd1fa245276952`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause.

Cause 1 of 1: a nodejs22.x Lambda function whose ESM handler imports an @aws-sdk/client-* package faults at module resolution, so no AWS SDK for JavaScript v3 consumer can be driven against the emulator. Failing check id: sqs.query-compatible-error-carriers-differ-by-consumer-rejected.

AWS: AWS's managed Node.js runtimes (nodejs18.x/20.x/22.x) bundle the AWS SDK for JavaScript v3 in the execution environment, so a handler may `import {SQSClient, GetQueueUrlCommand, QueueDoesNotExist} from "@aws-sdk/client-sqs"` (and `import pj from "@aws-sdk/client-sqs/package.json"`) with nothing packaged in the deployment zip. Lambda Invoke then answers HTTP 200 with no `FunctionError` header/field and the handler's returned JSON as the payload. The behaviour under test is SQS's own protocol contract, which the Smithy model states: service AmazonSQS, protocols [awsJson1_0], protocol_modifiers [awsQueryCompatible]; error com.amazonaws.sqs#QueueDoesNotExist is a client error with httpError 400 and awsQueryError {code: "AWS.SimpleQueueService.NonExistentQueue", httpResponseCode: 400}. Under awsQueryCompatible a single refusal therefore carries BOTH carriers — the `x-amzn-query-error` header with the AWS-Query code `AWS.SimpleQueueService.NonExistentQueue` and the awsJson1.0 body `__type` with the modeled Smithy code `QueueDoesNotExist` — and which one a client surfaces depends on the SDK: botocore (which sends `x-amzn-query-mode: true`, content-type `application/x-amz-json-1.0`) reports the Query code, while the JS v3 client reports the modeled Smithy name and only falls back to the header for an unmodeled error. Verifying that divergence requires a running JS v3 consumer; AWS supplies one in the managed runtime.

Emulator: `CreateFunction` with Runtime=nodejs22.x succeeds and the function reaches Active, but `Invoke` of the handler comes back as a function error whose payload is `{"errorMessage":"Error resolving module '@aws-sdk/client-sqs' from 'index.mjs'","errorType":"ReferenceError","requestId":"<per-invocation request id>","trace":[]}`. The fixed text of that message is `Error resolving module '<specifier>' from '<module file>'`; the specifier and file name are taken from the handler's import statement, and the request id is a per-invocation value. Two things diverge: the runtime provides no `@aws-sdk/*` package at all, and the error it reports for an unresolvable module is `ReferenceError`, where AWS's Node.js runtime reports `errorType: "Runtime.ImportModuleError"` with a `Cannot find module` message for a module it genuinely cannot resolve. Because the handler never runs, not one of the check's four probes (absentGetUrl, absentReceive, badDelay, control) is issued, so the emulator's SQS awsQueryCompatible header/body carriers are never exercised from the JS side.

Why: a client relying on AWS's behaviour writes a Node.js Lambda against the bundled SDK v3 and gets a working handler; against the emulator every such function fails on its first import, so JS/TypeScript Lambda code cannot run at all, and no test can confirm that the emulator's SQS refusals carry the dual awsQueryCompatible codes a JS v3 client needs to catch `QueueDoesNotExist` by type. Code that catches `e instanceof QueueDoesNotExist` is untestable, and an application that depends on the error name silently diverges.

Fix: the Lambda nodejs runtime's module loader and the runtime image/bundle that backs it (the nodejs22.x execution environment used by the lambda Invoke handler) must resolve the `@aws-sdk/*` v3 client packages (including their `package.json` subpath export) the way the managed runtime does, and must classify an unresolvable module as `Runtime.ImportModuleError` rather than `ReferenceError`. Once the handler runs, the SQS error serializer must emit both carriers for one refusal (`x-amzn-query-error` with the awsQueryError code and `__type` with the Smithy error name) for the JS-side assertions to be meaningful.

Related open issues that do NOT cover this check (the replay report does not mark it owned by either, so it is filed separately): 20261106T062901Z-lambda-nodejs-runtime-8-net-new-execution-fidelity.md and 20261106T052537Z-apigatewayv2-lambda-logs-the-nodejs22-x-lambda-runtime.md describe the same missing-SDK family under other service headings; this one is the sqs-heading occurrence and additionally gates the SQS awsQueryCompatible dual-carrier contract.

Traces: sqs.query-compatible-error-carriers-differ-by-consumer-rejected (the suite's sqs section; assertion at check line 234, `assert invoked.get("FunctionError") is None`).

## Evidence
Failing check: [mismatch] sqs.query-compatible-error-carriers-differ-by-consumer-rejected

Replay output:
  File "check_sqs_query_compatible_error_carriers_differ_by_consumer_rejected.py", line 234, in run
    assert invoked.get("FunctionError") is None, (
  AssertionError: the JS v3 consumer failed: b'{"errorMessage":"Error resolving module \'@aws-sdk/client-sqs\' from \'index.mjs\'","errorType":"ReferenceError","requestId":"<request id>","trace":[]}'

Expected (AWS): Invoke of a nodejs22.x function returns HTTP 200 with no FunctionError and the handler's JSON payload; the handler's `import ... from "@aws-sdk/client-sqs"` resolves from the runtime's bundled AWS SDK for JavaScript v3 with nothing packaged in the zip. The check's own source states the premise: "The nodejs22.x runtime bundles the AWS SDK for JavaScript v3, so no packaging is needed." The handler then probes GetQueueUrl/ReceiveMessage on an absent queue and SendMessage with DelaySeconds=901 and reports, per probe, {ctor, name, message, isQueueDoesNotExist, httpStatusCode, requestId}.

Actual (emulator): Invoke reports a function error; payload errorType `ReferenceError`, errorMessage `Error resolving module '<specifier>' from '<module file>'` (fixed text; specifier and file come from the handler's import), trace []. The handler body never executes.

Model evidence (services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json):
  service AmazonSQS 2012-11-05, protocols [awsJson1_0], protocol_modifiers [awsQueryCompatible]
  com.amazonaws.sqs#QueueDoesNotExist: client error, httpError 400, awsQueryError {code "AWS.SimpleQueueService.NonExistentQueue", httpResponseCode 400}
So one refusal must carry the Query code AWS.SimpleQueueService.NonExistentQueue (x-amzn-query-error) and the Smithy code QueueDoesNotExist (body __type); the botocore half of the check passes its header assertions (x-amzn-query-mode: true, content-type application/x-amz-json-1.0), only the JS v3 half is unreachable.

## Steps to Reproduce
1. Create an IAM role trusting lambda.amazonaws.com with sqs:GetQueueUrl/ReceiveMessage/SendMessage on the test queues plus logs write permissions.
2. Create an SQS queue and derive an absent queue URL in the same account/region.
3. Zip a single ESM file index.mjs whose first line is `import {SQSClient, GetQueueUrlCommand, ReceiveMessageCommand, SendMessageCommand, QueueDoesNotExist} from "@aws-sdk/client-sqs";` (plus `import pj from "@aws-sdk/client-sqs/package.json" with {type: "json"};`) and which exports an async handler that probes GetQueueUrl on the absent name, ReceiveMessage on the absent URL, SendMessage with DelaySeconds=901 on the present queue, and GetQueueUrl on the present name, returning per-probe {ctor, name, message, isQueueDoesNotExist, httpStatusCode}.
4. lambda CreateFunction with Runtime=nodejs22.x, Handler=index.handler, that zip as Code.ZipFile; wait for State=Active.
5. lambda Invoke the function with {region, absentName, absentUrl, presentUrl, presentName}.
6. Observe: the response carries FunctionError and the payload {"errorMessage":"Error resolving module '@aws-sdk/client-sqs' from 'index.mjs","errorType":"ReferenceError",...}. On AWS the invoke succeeds with no FunctionError and the probe results show absentGetUrl/absentReceive as QueueDoesNotExist (isQueueDoesNotExist true, httpStatusCode 400) while botocore sees AWS.SimpleQueueService.NonExistentQueue for the same refusal.

Replay the check directly: sqs.query-compatible-error-carriers-differ-by-consumer-rejected
