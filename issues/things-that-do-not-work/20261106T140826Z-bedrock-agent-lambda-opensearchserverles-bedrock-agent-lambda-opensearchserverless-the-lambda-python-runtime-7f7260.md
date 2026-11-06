Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__lambda__opensearchserverless.cli-bedrock-agent-invoke-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-get-agent-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-wrong-agent-id-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-missing-permission-negative
Issue-Key: bedrock-agent__lambda__opensearchserverless.cli-bedrock-agent-invoke-missing-permission-negative|run#15|AssertionError|assert neg_body.get("code") == "AccessDeniedException", (
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-get-agent-missing-permission-negative|run#6|AssertionError|assert denied_body["code"] == "AccessDeniedException", denied_body
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-missing-permission-negative|run#7|AssertionError|assert denied_body["code"] == "AccessDeniedException", denied_body
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-wrong-agent-id-negative|run#5|AssertionError|assert malformed_body["code"] == "ValidationException", malformed_body
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-missing-permission-negative|run#6|AssertionError|assert denied_body["code"] == "AccessDeniedException", denied_body
Issue-Commit: 0f21bdaacf606a414ed170f95e60e0507d4ecb5b

# bedrock-agent__lambda__opensearchserverless: the Lambda python runtime's bundled botocore has no bedrock-agent / bedrock-agent-runtime service data, so every in-function Bedrock Agent call dies as UnknownServiceError before it reaches the emulator

> _Note: this issue was identified against the code base at commit `0f21bdaacf606a414ed170f95e60e0507d4ecb5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the managed python runtime now has boto3 importable (the boto3-missing failure described in the still-open issue 20261105T111951Z-bedrock-agent-lambda-opensearchserverles-...-the-lambda-python-runtime.md no longer reproduces for these five checks), but the bundled botocore carries no service model for `bedrock-agent` or `bedrock-agent-runtime`. `boto3.client("bedrock-agent")` / `boto3.client("bedrock-agent-runtime")` therefore raises botocore's client-side `UnknownServiceError` inside the handler and no request is ever sent, so none of the authorization or lookup verdicts under test can be observed. Failing check ids: bedrock-agent__lambda__opensearchserverless.cli-bedrock-agent-invoke-missing-permission-negative, bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-get-agent-missing-permission-negative, bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-missing-permission-negative, bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-invoke-agent-wrong-agent-id-negative, bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-missing-permission-negative.

AWS: a python3.x managed Lambda runtime bundles a boto3/botocore whose data set includes every GA service, so a handler may create a `bedrock-agent` or `bedrock-agent-runtime` client and call it under the function's execution role. The call then reaches the service and is answered by the service's modeled errors. Smithy (bedrock-agent): GetAgent GET /agents/{agentId}/ with agentId @pattern ^[0-9a-zA-Z]{10}$, errors AccessDeniedException (403), ResourceNotFoundException (404), ValidationException (400), ThrottlingException (429), InternalServerException (500). AWS evaluates authorization before resource lookup, so a role lacking bedrock:GetAgent / bedrock:InvokeAgent / bedrock:Retrieve gets 403 AccessDeniedException for a syntactically valid but nonexistent id, while a role holding the action gets 404 ResourceNotFoundException for the same id, and a malformed agentId gets 400 ValidationException. The error code is the requirement here; the message text for each of these classes is AWS's fixed text for that class with the request-derived values (the agentId/agentAliasId/knowledgeBaseId from the request, the action and role ARN in the access-denied sentence) substituted — recorded behavior only for the exact message wording of these three classes.

Emulator: the handler never gets as far as an API call. Both `boto3.client("bedrock-agent", region_name=...)` and `boto3.client("bedrock-agent-runtime", region_name=...)` raise `botocore.exceptions.UnknownServiceError` in the managed python runtime, so the handler's own except branch returns {"ok": false, "code": "UnknownServiceError"} for the denied role, the allowed role and the malformed-id leg alike. No 403/404/400 is produced, because no request leaves the function.

Why: a client that packages business logic as a Lambda calling Bedrock Agent (GetAgent, InvokeAgent, Retrieve, RetrieveAndGenerate) cannot run at all against the emulator — construction of the client fails, which in real AWS never happens. Every permission-boundary and id-validation negative in this group is also rendered vacuous: all legs fail identically for an unrelated, client-side reason, so an emulator with no authorization at all is indistinguishable from a correct one.

Fix: the lambda runtime component that provisions the managed python runtime's site-packages — its bundled boto3/botocore must carry the botocore data directory entries (service-2.json / endpoint data) for bedrock-agent and bedrock-agent-runtime (and generally for every service the emulator serves), so a handler can construct those clients. No change is needed in the bedrock-agent handlers themselves for this cause.

Traces: the five failing check ids listed above; see trace_ids.

## Evidence
lambda-bedrock-agent-get-agent-missing-permission-negative: AssertionError: {'ok': False, 'code': 'UnknownServiceError'} at `assert denied_body["code"] == "AccessDeniedException"`. Expected: the role lacking bedrock:GetAgent gets 403 AccessDeniedException, and the sibling role holding it gets 404 ResourceNotFoundException for the same never-created 10-char agent id.
lambda-bedrock-agent-invoke-agent-missing-permission-negative: AssertionError: {'ok': False, 'code': 'UnknownServiceError'} where AccessDeniedException (then ResourceNotFoundException for the allowed role) is expected.
lambda-bedrock-agent-invoke-agent-wrong-agent-id-negative: AssertionError: {'ok': False, 'code': 'UnknownServiceError'} at `assert malformed_body["code"] == "ValidationException"` — a malformed agentId must be 400 ValidationException, a valid-but-nonexistent one 404 ResourceNotFoundException.
lambda-bedrock-agent-retrieve-missing-permission-negative: AssertionError: {'ok': False, 'code': 'UnknownServiceError'} where the role lacking bedrock:Retrieve must get AccessDeniedException and the sibling role must retrieve the ingested passage.
cli-bedrock-agent-invoke-missing-permission-negative: AssertionError: expected AccessDeniedException for the role lacking bedrock:InvokeAgent, got UnknownServiceError ({'ok': False, 'code': 'UnknownServiceError'}).
In all five the handler body is a bare `boto3.client("bedrock-agent"|"bedrock-agent-runtime", region_name=...)` followed by one API call; UnknownServiceError is botocore's client-side "unknown service" error, raised at client construction, so the emulator was never asked anything.

## Steps to Reproduce
1. CreateRole with the Lambda trust policy (plus AWSLambdaBasicExecutionRole), with and without an inline Allow of bedrock:GetAgent / bedrock:InvokeAgent / bedrock:Retrieve.
2. CreateFunction on a python3.x runtime whose handler is:
   import boto3
   def handler(event, context):
       client = boto3.client("bedrock-agent", region_name=event["region"])
       try:
           resp = client.get_agent(agentId="zzzzzzzzz9")
           return {"ok": True, "agent_name": resp["agent"]["agentName"]}
       except Exception as e:
           return {"ok": False, "code": getattr(e, "response", {}).get("Error", {}).get("Code", type(e).__name__)}
3. Wait for the function to be Active, then Invoke it with {"region": "<region>"}.
4. Observed: payload {"ok": false, "code": "UnknownServiceError"} for every role. Expected: {"ok": false, "code": "AccessDeniedException"} for the role without bedrock:GetAgent and {"ok": false, "code": "ResourceNotFoundException"} for the role with it.
5. Repeat with boto3.client("bedrock-agent-runtime") and invoke_agent / retrieve for the same result.
