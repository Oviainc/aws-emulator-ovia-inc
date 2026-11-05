Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-and-generate-positive
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-and-generate-positive|_invoke_lambda_json#9|AssertionError|raise AssertionError(f"lambda function error: {body}")
Issue-Commit: dcfd0b13909fe287d91ffc9df5650d4d8b87d5a3

# bedrock-agent-runtime__lambda__s3vectors: the Python Lambda runtime provides no AWS SDK, so a handler that calls RetrieveAndGenerate dies with Runtime.ImportModuleError (1 cause)

> _Note: this issue was identified against the code base at commit `dcfd0b13909fe287d91ffc9df5650d4d8b87d5a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the emulator's managed Python Lambda execution environment has no AWS SDK for Python on the handler's import path, so any handler that does `import boto3` faults at module load instead of running. Failing check id: bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-and-generate-positive.

AWS: recorded behavior only for the contents of the managed runtime (the Smithy model does not describe the execution environment). AWS's managed Python runtimes (here Runtime=python3.12, Handler=index.handler, deployed as a ZipFile containing index.py) ship the AWS SDK for Python (boto3 and botocore) preinstalled in the execution environment, so `import boto3` inside a handler succeeds with no layer and no vendored dependency. Smithy: Invoke -> InvocationResponse with StatusCode (httpResponseCode), FunctionError (header X-Amz-Function-Error) and Payload (httpPayload). On a successful invocation AWS answers 200 with FunctionError ABSENT and Payload = exactly the JSON the handler returned; FunctionError is only set (value "Unhandled" for an uncaught fault, "Handled" for a raised exception) when the handler itself failed. A module-level ImportError is therefore not something AWS produces for `import boto3` in python3.12.

Emulator: Invoke returns 200 but sets FunctionError, and the payload is the runtime's own fault document rather than the handler's return value:
  {"errorMessage": "Unable to import module 'index': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-invocation id>", "stackTrace": []}
Fixed text of that message: errorType "Runtime.ImportModuleError" and the sentence "Unable to import module '<handler module>': No module named '<module>'"; the variable parts are the handler's module name (here the one named by Handler, index) and the module that could not be imported (boto3). requestId is a per-invocation value and is not part of the requirement. The check's handler calls bedrock-agent-runtime RetrieveAndGenerate through boto3 and never gets to run, so the KB-grounded answer, its citations and retrievedReferences are never produced and the assertion on the handler payload (`ok` is True) fails.

Why: a client gets a Lambda that cannot use any AWS service. Every function whose code calls another AWS API through the standard SDK — the overwhelmingly common Lambda shape, and the whole point of this end-to-end check (Lambda -> bedrock-agent-runtime RetrieveAndGenerate over an S3 Vectors knowledge base) — fails at import time. The failure also misattributes: the caller sees a handler/packaging error (Runtime.ImportModuleError) rather than a missing SDK in the environment, so a user debugging it concludes their deployment package is wrong and starts vendoring boto3, which on real AWS is unnecessary. Any emulator-wide scenario that fans out through Lambda glue code is unreachable.

Fix: the Lambda service's invoke/execution-environment component — the in-process Python runtime that loads the handler module for Invoke must expose boto3/botocore (configured against the emulator's endpoints and credentials) on the handler's sys.path, the way the managed python3.12 runtime does. Not a change to the Invoke serializer: the wire shape of the response is correct, it is the environment the handler loads in that is missing the SDK.

Related open issues describe the same missing-SDK symptom for other service pairings and do NOT cover this check (the suite did not mark it owned): 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md, 20261105T080633Z-lambda-sesv2-the-emulator-s-python-lambda-runtime.md, 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md, 20261105T081727Z-events-lambda-sns-sqs-a-lambda-handler-cannot.md. If the runtime is fixed centrally, this check should be re-replayed to confirm.

Traces: bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-and-generate-positive.

## Evidence
Expected (AWS): lambda Invoke(FunctionName=<fn>, InvocationType=RequestResponse, Payload={kb_id, gen_model_arn, query}) -> HTTP 200, no X-Amz-Function-Error header, Payload = the handler's own JSON, e.g. {"ok": true, "answer": "...", "num_citations": >=1, "refs": [...]}, because the python3.12 managed runtime has boto3 preinstalled and `client = boto3.client("bedrock-agent-runtime")` plus retrieve_and_generate(...) run normally.

Actual (emulator): HTTP 200 with FunctionError set and Payload = {"errorMessage": "Unable to import module 'index': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-invocation id>", "stackTrace": []}. The harness raises AssertionError("lambda function error: {...}") at check_bedrock_agent_runtime__lambda__s3vectors_lambda_bedrock_retrieve_and_generate_positive.py line 220 (_invoke_lambda_json), reached from run() line 92. Fixed text: errorType "Runtime.ImportModuleError" and "Unable to import module '<module>': No module named '<module>'"; variable parts: the handler module name from the Handler setting (index) and the missing module name (boto3). requestId varies per invocation and is not a requirement.

## Steps to Reproduce
1. iam CreateRole with a lambda.amazonaws.com trust policy and an inline policy allowing bedrock:Retrieve/RetrieveAndGenerate/InvokeModel and logs:*.
2. lambda CreateFunction with Runtime=python3.12, Handler=index.handler, MemorySize=256, Timeout=90, Code.ZipFile = a zip whose index.py begins with `import json` and `import boto3` and whose handler calls boto3.client("bedrock-agent-runtime").retrieve_and_generate(...). Poll GetFunction until Configuration.State == Active.
3. lambda Invoke(FunctionName=<fn>, InvocationType=RequestResponse, Payload={"kb_id": ..., "gen_model_arn": ..., "query": "..."}).
4. Observe the response: FunctionError is set and the Payload is the runtime fault document with errorType Runtime.ImportModuleError / "No module named 'boto3'", instead of the handler's returned JSON. Any handler that merely does `import boto3` and returns {} reproduces it with no Bedrock setup at all.
