Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-batch-get-collection-positive bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-create-collection-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-create-collection-positive bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-list-collections-missing-permission-negative bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-list-collections-positive bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-tag-resource-positive
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-batch-get-collection-positive|run#7|AssertionError|assert result["ok"] is True, result
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-create-collection-missing-permission-negative|run#8|AssertionError|assert bad_result["error_code"] == "AccessDeniedException", (
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-create-collection-positive|run#5|AssertionError|assert result["ok"] is True, result
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-list-collections-missing-permission-negative|run#4|AssertionError|assert good_result["ok"] is True, good_result
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-list-collections-positive|run#6|AssertionError|assert result["ok"] is True, result
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-tag-resource-positive|run#8|AssertionError|assert tag_result["ok"] is True, tag_result
Issue-Commit: 52f91172eb2f0209f53a5585da2848c5cfe26576

# bedrock-agent__lambda__opensearchserverless: the Lambda python runtime's bundled botocore still has no 'opensearchserverless' (aoss) service model, so boto3.client("opensearchserverless") raises UnknownServiceError inside every handler (1 cause, 6 checks)

> _Note: this issue was identified against the code base at commit `52f91172eb2f0209f53a5585da2848c5cfe26576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the AWS SDK installed into the emulator's managed python Lambda execution environment carries a truncated botocore service-model data directory. The catalog it now advertises is sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager, opensearch, es, stepfunctions, sfn, sesv2, s3 — it does NOT contain opensearchserverless (aoss). A handler that does boto3.client("opensearchserverless") therefore dies inside boto3 with botocore.exceptions.UnknownServiceError before any request leaves the sandbox, so neither the OpenSearch Serverless control-plane answer nor the IAM decision these checks exist to pin down is ever exercised. Failing check ids (6): bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-batch-get-collection-positive, bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-create-collection-positive, bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-create-collection-missing-permission-negative, bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-list-collections-positive, bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-list-collections-missing-permission-negative, bedrock-agent__lambda__opensearchserverless.lambda-opensearchserverless-tag-resource-positive.

AWS: recorded behavior only for the CONTENTS of the managed runtime — the Smithy model describes the lambda Invoke wire shape, not the execution environment. AWS's managed python runtime ships a current boto3/botocore whose data directory holds a model for every public AWS service, so inside a handler boto3.client("opensearchserverless") constructs normally, the call is dispatched, and the handler observes the SERVICE's own answer. Smithy (com.amazonaws.opensearchserverless): BatchGetCollection(ids|names, each @length 1..100) -> collectionDetails/collectionErrorDetails, errors ValidationException (400), InternalServerException (500), @readonly; CreateCollection(name @required @length 3..64 @pattern ^[a-z][a-z0-9-]+$, type, encryptionConfig, tags @length 0..50) -> createCollectionDetail, errors ConflictException (409), ValidationException (400), ServiceQuotaExceededException (402), OcuLimitExceededException (402), InternalServerException (500); ListCollections(collectionFilters, maxResults @range 1..100, nextToken) -> collectionSummaries + nextToken, @paginated, errors ValidationException (400), InternalServerException (500); TagResource(resourceArn @required, tags @required) -> empty output, errors ResourceNotFoundException (404), ConflictException (409), ValidationException (400). The model does not list an authorization error on these operations: recorded behavior is that a caller whose role carries no matching aoss: action is refused by the service's authorization layer with HTTP 403 and error code AccessDeniedException, which is what the negative arms of these checks assert the handler sees.

Emulator: lambda Invoke answers 200 and the handler body runs (`import boto3` succeeds), but client construction raises and the handler's own except-branch reports botocore's exception type: the invocation payload comes back as {"ok": false, "error_code": "UnknownServiceError", "error_message": "Unknown service: 'opensearchserverless'. Valid service names are: ..."}. The fixed text of that message is botocore's own wording, "Unknown service: '<service-name>'. Valid service names are: <installed model list>"; its variable parts are the service id the handler asked for (opensearchserverless, taken from the boto3.client call) and the installed-model list, which is a property of the runtime image, not of the request or any state. Every distinct outcome AWS would produce — a successful collectionDetails with the collection's id/status ACTIVE/collectionEndpoint, a successful createCollectionDetail, a successful collectionSummaries list, an empty TagResource success, and the AccessDeniedException refusal on the no-permission arms — collapses into this one client-side failure.

Why: a client gets a Lambda that can reach only the handful of services in that catalog. Every OpenSearch Serverless glue function (the subject of these checks: in-handler CreateCollection, BatchGetCollection, ListCollections, TagResource) fails before a request is made, so a user cannot build or inspect a collection from inside a function. The permission-boundary negatives are worse than merely failing: a role that must surface AccessDeniedException instead surfaces a botocore UnknownServiceError, so a user cannot distinguish an IAM denial from a broken runtime image and will keep editing a policy that was never consulted.

Fix: the Lambda service's execution-environment component — the part that assembles the python runtime filesystem for the sandbox and puts boto3/botocore on the handler's import path. It must install a botocore whose data directory carries the full set of service models (at minimum every service the emulator itself serves, opensearchserverless/aoss included), the way AWS's managed python runtime does, instead of a fixed subset. Not the lambda Invoke serializer: the Invoke response wire shape is correct; the SDK data shipped into the sandbox is incomplete. Not the opensearchserverless handlers either: they are never reached on this path.

Related open work (does NOT cover these six checks; the suite marked none of them owned or already filed): 20261106T140124Z-bedrock-agent-lambda-opensearchserverles-bedrock-agent-lambda-opensearchserverless-the-python-lambda-runtime.md and 20261106T140826Z-...-the-lambda-python-runtime-7f7260.md report the same runtime cause for the bedrock-agent / bedrock-agent-runtime models, 20261107T175916Z-bedrock-agent-runtime-lambda-s3vectors-...md for s3vectors, and 20261105T111951Z-...-the-lambda-python-runtime.md reports this runtime's EARLIER symptom where boto3 was absent entirely (Runtime.ImportModuleError "No module named 'boto3'"). The catalog has since grown to 14 entries but still omits opensearchserverless, so none of those closes these checks.

Traces: the 6 failing check ids listed above; see trace_ids.

## Evidence
Expected (AWS): lambda Invoke(FunctionName=<fn>, InvocationType=RequestResponse, Payload=<event>) on a python function whose handler does boto3.client('opensearchserverless') and then calls the requested action -> HTTP 200, no X-Amz-Function-Error, and the handler's own JSON describing the SERVICE's outcome:
- batch_get_collection(names=[<coll>]) with aoss:BatchGetCollection granted -> {"ok": true, "result": {"collectionDetails": [{"name": <coll>, "id": <collection id>, "status": "ACTIVE", "collectionEndpoint": "https://..."}]}}
- create_collection(name=<coll>, type=SEARCH, encryptionConfig=...) with aoss:CreateCollection granted -> {"ok": true, "result": {"createCollectionDetail": {...}}}
- list_collections() with aoss:ListCollections granted -> {"ok": true, "result": {"collectionSummaries": [...]}}
- tag_resource(resourceArn=<collection arn>, tags=[...]) with aoss:TagResource granted -> {"ok": true, ...}
- the same calls from a role with no matching aoss action -> {"ok": false, "error_code": "AccessDeniedException"} (HTTP 403 from the service).
The client object itself always constructs.

Actual (emulator): HTTP 200, the handler runs, and the payload reports botocore's client-construction failure in all six checks:
- lambda-opensearchserverless-batch-get-collection-positive: AssertionError: {'ok': False, 'error_code': 'UnknownServiceError', 'error_message': "Unknown service: 'opensearchserverless'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager, opensearch, es, stepfunctions, sfn, sesv2, s3"} at `assert result["ok"] is True`.
- lambda-opensearchserverless-create-collection-positive: same payload at `assert result["ok"] is True`.
- lambda-opensearchserverless-create-collection-missing-permission-negative: AssertionError: expected AccessDeniedException from CreateCollection with no aoss permission; got 'UnknownServiceError' (same payload).
- lambda-opensearchserverless-list-collections-positive: same payload at `assert result["ok"] is True`.
- lambda-opensearchserverless-list-collections-missing-permission-negative: same payload at `assert good_result["ok"] is True` (the granted arm fails first, so the denial arm is never reached).
- lambda-opensearchserverless-tag-resource-positive: same payload at `assert tag_result["ok"] is True`.

Fixed text of the diverging message (botocore's own wording, surfaced by the handler): "Unknown service: '<service-name>'. Valid service names are: <installed model list>". Variable parts: the service id requested by the handler (opensearchserverless) and the installed-model list. Generated collection names and ids, function names, role names, request ids and timestamps vary per run and are not requirements.

## Steps to Reproduce
1. opensearchserverless CreateCollection(name=<lowercase name>, type=SEARCH, encryptionConfig={aWSOwnedKey: true}) from the harness and poll BatchGetCollection(ids=[<id>]) until status == ACTIVE.
2. iam CreateRole with a lambda.amazonaws.com trust policy; attach AWSLambdaBasicExecutionRole and an inline policy granting the aoss action under test (e.g. {"Effect":"Allow","Action":"aoss:BatchGetCollection","Resource":"*"}). For the negative arm create a sibling role with no aoss statement.
3. lambda CreateFunction(Runtime=python3.12, Handler=index.handler, Code.ZipFile = a zip containing index.py:
       import boto3
       def handler(event, context):
           try:
               client = boto3.client("opensearchserverless")
               resp = getattr(client, event["action"])(**event.get("params", {}))
               resp.pop("ResponseMetadata", None)
               return {"ok": True, "result": resp}
           except Exception as exc:
               err = getattr(exc, "response", {}) or {}
               return {"ok": False, "error_code": err.get("Error", {}).get("Code", type(exc).__name__), "error_message": str(exc)}
   ) and poll GetFunction until Configuration.State == Active.
4. lambda Invoke(FunctionName=<fn>, Payload={"mode":"api","action":"batch_get_collection","params":{"names":["<coll>"]}}).
5. Observe HTTP 200 whose payload is {"ok": false, "error_code": "UnknownServiceError", "error_message": "Unknown service: 'opensearchserverless'. Valid service names are: ..."} instead of {"ok": true, "result": {"collectionDetails": [...]}}. Substituting action=create_collection, list_collections or tag_resource reproduces identically, and invoking with the no-aoss-permission role yields the same UnknownServiceError instead of AccessDeniedException.
