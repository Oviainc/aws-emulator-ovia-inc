Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__lambda__opensearchserverless.cli-bedrock-agent-invoke-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-wrong-agent-id-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-missing-permission-negative
Issue-Key: bedrock-agent__lambda__opensearchserverless.cli-bedrock-agent-invoke-missing-permission-negative|run#15|AssertionError|assert neg_body.get("code") == "AccessDeniedException", (
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-missing-permission-negative|run#7|AssertionError|assert denied_body["code"] == "AccessDeniedException", denied_body
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-wrong-agent-id-negative|run#5|AssertionError|assert malformed_body["code"] == "ValidationException", malformed_body
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-missing-permission-negative|run#6|AssertionError|assert denied_body["code"] == "AccessDeniedException", denied_body
Issue-Commit: 0f21bdaacf606a414ed170f95e60e0507d4ecb5b

# bedrock-agent__lambda__opensearchserverless: the python Lambda runtime's bundled botocore ships a 4-service data catalog, so boto3.client("bedrock-agent-runtime") raises UnknownServiceError inside every handler (1 cause, 4 checks)

> _Note: this issue was identified against the code base at commit `0f21bdaacf606a414ed170f95e60e0507d4ecb5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the AWS SDK present in the emulator's managed python Lambda execution environment carries a truncated botocore service-model data directory (only sqs, events, eventbridge and dynamodb), so a handler that constructs a client for bedrock-agent-runtime dies inside boto3 with botocore.exceptions.UnknownServiceError instead of reaching the service. No InvokeAgent / Retrieve request ever leaves the sandbox, so neither the IAM decision nor the resource lookup these checks exist to pin down is exercised. Failing check ids (4): bedrock-agent__lambda__opensearchserverless.cli-bedrock-agent-invoke-missing-permission-negative, bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-missing-permission-negative, bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-wrong-agent-id-negative, bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-missing-permission-negative.

AWS: recorded behavior only for the CONTENTS of the managed runtime — the Smithy model describes the lambda Invoke wire shape, not the execution environment. Smithy (lambda Invoke -> InvocationResponse, http {method: POST, uri: /2015-03-31/functions/{FunctionName}/invocations, code: 200}): StatusCode is the @httpResponseCode, FunctionError is header X-Amz-Function-Error, Payload is the @httpPayload blob. AWS's managed python runtime ships a current boto3/botocore whose data directory holds the model for every public AWS service, so inside a handler boto3.client("bedrock-agent-runtime") constructs normally and the call is dispatched; the handler then observes the SERVICE's own answer. Smithy (com.amazonaws.bedrockagentruntime): InvokeAgent and Retrieve both model AccessDeniedException (403, "The request is denied because of missing access permissions"), ResourceNotFoundException (404) and ValidationException (400). So a handler whose execution role omits bedrock:InvokeAgent sees AccessDeniedException even for a never-created agent (authorization precedes the lookup), a handler whose role grants it sees ResourceNotFoundException for a well-formed but nonexistent agentId, and a malformed agentId is refused ValidationException.

Emulator: Invoke answers 200 and the handler runs (`import boto3` succeeds), but client construction raises and the handler's own except-branch reports botocore's exception type: the invocation payload comes back as {"ok": false, "code": "UnknownServiceError"}. botocore's message text is fixed as "Unknown service: '<service-name>'. Valid service names are: <comma-separated list of the models the runtime actually has>"; its variable parts are the service id the handler asked for (here bedrock-agent-runtime, from the boto3.client call) and the installed-model list, which is a property of the runtime image and not of the request or any state. The three error codes AWS distinguishes (AccessDeniedException, ResourceNotFoundException, ValidationException) all collapse into this one client-side failure.

Why: a client gets a Lambda that can reach only four AWS services. Every Bedrock Agent glue function — the entire subject of these checks (Lambda -> bedrock-agent-runtime InvokeAgent and Retrieve) — fails before a request is made. The permission-boundary negatives are worse than failed: an under-permissioned role that must surface AccessDeniedException instead surfaces a botocore UnknownServiceError, so a user cannot tell an IAM denial from a broken runtime image and will go on editing a policy that was never consulted. The arn-resolution negative likewise cannot distinguish ValidationException (malformed agentId) from ResourceNotFoundException (valid but nonexistent), so no agent-id resolution behavior is observable from inside a function at all.

Fix: the Lambda service's execution-environment component — the part that assembles the python runtime filesystem for the sandbox and puts boto3/botocore on the handler's import path. It must install a botocore whose data directory carries the full set of service models (at minimum every service the emulator itself serves, including bedrock-agent-runtime and opensearchserverless), the way AWS's managed python runtime does, instead of a four-service subset. Not the Invoke serializer: the Invoke response wire shape is correct; the SDK data shipped into the sandbox is incomplete.

Related open work (does NOT cover these four checks; the suite marked none of them owned or already filed): 20261106T131020Z-bedrock-agent-runtime-lambda-s3vectors-the-lambda-python.md reports the identical runtime cause under the bedrock-agent-runtime__lambda__s3vectors service token, and 20261105T111951Z-bedrock-agent-lambda-opensearchserverles-bedrock-agent-lambda-opensearchserverless-the-lambda-python-runtime.md reports this runtime's EARLIER symptom, where boto3 was absent entirely (Runtime.ImportModuleError "No module named 'boto3'"). Here the import succeeds and the failure has moved to the botocore data catalog, so adding boto3 alone does not close these.

Traces: the 4 failing check ids listed above; see trace_ids.

## Evidence
Expected (AWS): lambda Invoke(FunctionName=<fn>, InvocationType=RequestResponse, Payload=<event>) on a python function whose handler does `import boto3` / `boto3.client('bedrock-agent-runtime')` and then calls invoke_agent or retrieve -> HTTP 200, no X-Amz-Function-Error, and the handler's own JSON describing the SERVICE's outcome: {"ok": false, "code": "AccessDeniedException"} when the execution role lacks bedrock:InvokeAgent / bedrock:Retrieve, {"ok": false, "code": "ResourceNotFoundException"} when the role grants it and the agentId is well-formed but never created, {"ok": false, "code": "ValidationException"} for a malformed agentId, and {"ok": true, ...} on the permitted positive arms. The client object itself always constructs.

Actual (emulator): HTTP 200, the handler runs, and the payload reports botocore's client-construction failure:
- bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-missing-permission-negative: AssertionError: {'ok': False, 'code': 'UnknownServiceError'} at `assert denied_body["code"] == "AccessDeniedException"`.
- .lambda-bedrock-agent-retrieve-missing-permission-negative: AssertionError: {'ok': False, 'code': 'UnknownServiceError'} at `assert denied_body["code"] == "AccessDeniedException"`.
- .lambda-bedrock-agent-invoke-agent-wrong-agent-id-negative: AssertionError: {'ok': False, 'code': 'UnknownServiceError'} at `assert malformed_body["code"] == "ValidationException"`.
- .cli-bedrock-agent-invoke-missing-permission-negative: AssertionError: expected AccessDeniedException for the role lacking bedrock:InvokeAgent, got UnknownServiceError ({'ok': False, 'code': 'UnknownServiceError'}) — same handler, driven through `aws lambda invoke`.

Fixed text of the diverging message (botocore's own wording, surfaced by the handler): "Unknown service: '<service-name>'. Valid service names are: <installed model list>". Variable parts: the service id requested by the handler (bedrock-agent-runtime) and the installed-model list. Generated function names, role names, agent ids, request ids and timestamps in these payloads vary per run and are not requirements.

## Steps to Reproduce
1. iam CreateRole with a lambda.amazonaws.com trust policy and an inline policy granting logs:* plus bedrock:InvokeAgent and bedrock:Retrieve (and a sibling role identical except that the bedrock action is omitted, for the negative arm).
2. lambda CreateFunction with Runtime=python3.12, Handler=index.handler, Code.ZipFile = a zip containing only index.py:
       import boto3, uuid
       def handler(event, context):
           try:
               client = boto3.client('bedrock-agent-runtime')
               resp = client.invoke_agent(agentId=event['agentId'], agentAliasId=event['agentAliasId'], sessionId=str(uuid.uuid4()), inputText='hi')
               return {'ok': True}
           except Exception as e:
               code = getattr(e, 'response', {}).get('Error', {}).get('Code', type(e).__name__)
               return {'ok': False, 'code': code}
   Poll GetFunction until Configuration.State == Active.
3. lambda Invoke(FunctionName=<fn>, InvocationType=RequestResponse, Payload={"agentId":"zzzzzzzzz9","agentAliasId":"zzzzzzzz88","inputText":"What is 2 + 2?"}) — equivalently `aws lambda invoke --function-name <fn> --payload <json> out.json`.
4. Observe HTTP 200 whose payload is {"ok": false, "code": "UnknownServiceError"} instead of {"ok": false, "code": "AccessDeniedException"} (role without bedrock:InvokeAgent) or {"ok": false, "code": "ResourceNotFoundException"} (role with it, nonexistent agent). Substituting client.retrieve(knowledgeBaseId=..., retrievalQuery={'text': ...}) reproduces identically, as does boto3.client('opensearchserverless') — any service outside sqs/events/eventbridge/dynamodb.
5. Repeat with a malformed agentId (e.g. 'not-an-agent-id'): AWS answers ValidationException, the emulator still answers UnknownServiceError, so the two refusal classes are indistinguishable from inside a function.
