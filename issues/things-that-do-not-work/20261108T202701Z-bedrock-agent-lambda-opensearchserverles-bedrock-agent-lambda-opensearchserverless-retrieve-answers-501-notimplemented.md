Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__lambda__opensearchserverless.deploy-opensearchserverless-collection-data-access-policy-positive bedrock-agent__lambda__opensearchserverless.deploy-stack-missing-opensearchserverless-data-access-negative bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-knowledge-base-positive bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-and-generate-positive
Issue-Key: bedrock-agent__lambda__opensearchserverless|Retrieve|NotImplemented
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-knowledge-base-positive|run#6|AssertionError|assert body["ok"] is True, body
Issue-Key: bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-and-generate-positive|run#6|AssertionError|assert body["ok"] is True, body
Issue-Commit: f392f82803b1854878505550560cb55ee4e9c61d

# bedrock-agent__lambda__opensearchserverless: Retrieve answers 501 NotImplemented, and the python Lambda runtime's botocore has no bedrock-agent-runtime model so handlers die with UnknownServiceError (2 causes, 4 checks)

> _Note: this issue was identified against the code base at commit `f392f82803b1854878505550560cb55ee4e9c61d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Retrieve

## Description
Two distinct root causes keep every bedrock-agent + Lambda + OpenSearch Serverless retrieval path from completing. Both must be closed before re-replaying the four checks below.

=== Cause 1 of 2: bedrock-agent-runtime Retrieve answers 501 NotImplemented, so an AOSS-backed knowledge base can never be read back. Failing check ids (2): bedrock-agent__lambda__opensearchserverless.deploy-opensearchserverless-collection-data-access-policy-positive, bedrock-agent__lambda__opensearchserverless.deploy-stack-missing-opensearchserverless-data-access-negative.

AWS: Smithy (com.amazonaws.bedrockagentruntime#Retrieve): http {method: POST, uri: /knowledgebases/{knowledgeBaseId}/retrieve, code: 200}; knowledgeBaseId is @httpLabel @required (KnowledgeBaseIdentifier, @length 10..2048, @pattern ^[0-9a-zA-Z]{10}$|^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:knowledge-base/[0-9a-zA-Z]{10}$); retrievalQuery @required @sensitive; output retrievalResults @required; @readonly and @paginated on nextToken. Modeled errors ONLY: AccessDeniedException 403, BadGatewayException 502, ConflictException 409, DependencyFailedException 424, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 400, ThrottlingException 429, ValidationException 400 — no NotImplemented and no 501. After an S3 ingestion job reports status COMPLETE with statistics.numberOfNewDocumentsIndexed >= 1, Retrieve answers 200 with retrievalResults whose content.text carries the ingested document text.

Emulator: HTTP 501, code NotImplemented, message 'operation Retrieve is scaffolded but not implemented in this emulator'. Fixed text: 'operation ' ... ' is scaffolded but not implemented in this emulator'; the only variable part is the operation name, which comes from the handler, not the request or state. Everything upstream answers: the AOSS collection and its data access policy are created and GetAccessPolicy returns the expected Principal/Rules, the vector index PUT is signed as the KB role, CreateKnowledgeBase and CreateDataSource(type=S3) succeed and the ingestion job runs — only the read-back is refused, which is also the positive control the negative check needs before it can assert CreateKnowledgeBase's storage-config refusal.

Why: a client cannot observe any retrieval from an OpenSearch-Serverless-backed knowledge base, and 501/NotImplemented is outside Retrieve's modeled error set, so SDKs surface an unclassifiable ClientError rather than one of the nine modeled exceptions. It also makes the AOSS data-access-policy boundary untestable in BOTH directions: the granted arm cannot prove retrieval works, so the denied arm's ValidationException from CreateKnowledgeBase ('The knowledge base storage configuration provided is invalid... Dependency error document status code: 403, error message: Bad Authorization') is never reached.

Fix: the Retrieve operation's handler — resolve knowledgeBaseId (both ARN and 10-char forms), search the documents ingested into its data sources, return 200 with populated retrievalResults, and refuse with the modeled ResourceNotFoundException (404) / ValidationException (400) rather than 501. Related open work that does NOT cover these two checks (the suite marked neither ALREADY FILED nor OWNED): 20261029T171944Z-bedrock-agent-runtime-retrieve-stub.md and 20261025T173258Z-bedrock-agent-runtime-retrieve-is-a-scaffold-stub.md carry the same stub under other service tokens and do not hold these replay keys.

Traces: the two check ids above; stub marker and the 501 return are at services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/retrieve.rs lines 4-6 and 194.

=== Cause 2 of 2: the managed python Lambda execution environment ships a botocore whose service-model data directory has no bedrock-agent-runtime, so boto3.client('bedrock-agent-runtime') raises UnknownServiceError inside the handler and no Retrieve / RetrieveAndGenerate request ever leaves the sandbox. Failing check ids (2): bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-knowledge-base-positive, bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-and-generate-positive.

AWS: recorded behavior only for the CONTENTS of the managed runtime image — the Smithy model describes the lambda Invoke wire shape, not the execution environment. AWS's managed python runtime ships a current boto3/botocore whose data directory holds a model for every public AWS service, so inside a handler boto3.client('bedrock-agent-runtime') constructs normally and the call is dispatched to the service; the handler then observes the SERVICE's answer. Smithy (com.amazonaws.bedrockagentruntime): Retrieve -> 200 with @required retrievalResults; RetrieveAndGenerate -> http {method: POST, uri: /retrieveAndGenerate, code: 200} with @required sessionId (@length 2..100, @pattern ^[0-9a-zA-Z._:-]+$) and @required output, plus citations; both model AccessDeniedException 403, ResourceNotFoundException 404 and ValidationException 400. So a handler whose role grants bedrock:Retrieve against an ingested KB gets 200 and retrievalResults[*].content.text containing the ingested fact, and one granted bedrock:RetrieveAndGenerate + bedrock:InvokeModel gets 200 with output.text grounded in that document and at least one citation whose retrievedReferences[*].content.text contains it.

Emulator: Invoke answers 200 and the handler runs ('import boto3' succeeds), but client construction raises and the handler's own except-branch reports botocore's exception type, so the invocation payload is {'ok': false, 'code': 'UnknownServiceError'}. botocore's message text is fixed as "Unknown service: '<service-name>'. Valid service names are: <comma-separated list of the models the runtime actually has>"; its variable parts are the service id the handler asked for (bedrock-agent-runtime, from the boto3.client call) and the installed-model list, which is a property of the runtime image rather than of the request or any state.

Why: a client gets a Lambda that cannot reach bedrock-agent-runtime at all. Every Bedrock knowledge-base glue function — the entire subject of these two checks — fails before a request is made, and the client-side UnknownServiceError is indistinguishable from a real service refusal, so a user debugging a RAG function will chase an IAM policy or a knowledge-base id that was never consulted.

Fix: the Lambda service's execution-environment component — the part that assembles the python runtime filesystem for the sandbox and puts boto3/botocore on the handler's import path. It must install a botocore whose data directory carries the full set of service models (at minimum every service the emulator itself serves, including bedrock-agent-runtime and opensearchserverless), the way AWS's managed python runtime does. Not the Invoke serializer: the Invoke response wire shape is correct; the SDK data shipped into the sandbox is incomplete. Related open work that does NOT cover these two checks (the suite marked neither ALREADY FILED nor OWNED): 20261106T140124Z-bedrock-agent-lambda-opensearchserverles-...-the-python-lambda-runtime.md reports the identical runtime cause but its replay key lists only the four NEGATIVE checks of this service token; 20261106T131020Z-bedrock-agent-runtime-lambda-s3vectors-the-lambda-python.md reports it under bedrock-agent-runtime__lambda__s3vectors; and 20261105T111951Z-...-the-lambda-python-runtime.md reports this runtime's earlier symptom (boto3 absent entirely, Runtime.ImportModuleError). These two positive checks are on none of those keys.

Traces: the two check ids above.

## Evidence
Cause 1 — expected (AWS): with an AOSS collection whose data access policy grants the KB role aoss:APIAccessAll, a vector index created as that role, CreateKnowledgeBase ACTIVE, CreateDataSource(type=S3) and an ingestion job ending status=COMPLETE with statistics.numberOfDocumentsScanned >= 1 and numberOfNewDocumentsIndexed >= 1, POST /knowledgebases/{knowledgeBaseId}/retrieve with retrievalQuery.text answers HTTP 200 and retrievalResults[*].content.text containing the ingested sentence.
Cause 1 — actual (emulator): HTTP 501, code NotImplemented:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the Retrieve operation: operation Retrieve is scaffolded but not implemented in this emulator
seen in bedrock-agent__lambda__opensearchserverless.deploy-opensearchserverless-collection-data-access-policy-positive (at the _poll_until(runtime.retrieve(...)) step after COMPLETE ingestion) and in .deploy-stack-missing-opensearchserverless-data-access-negative (in its positive-control arm, before it can assert the ValidationException 'The knowledge base storage configuration provided is invalid... Dependency error document status code: 403, error message: Bad Authorization'). 501 and NotImplemented are both outside Retrieve's modeled error set.

Cause 2 — expected (AWS): lambda Invoke(FunctionName=<fn>, InvocationType=RequestResponse, Payload=<event>) on a python function whose handler does boto3.client('bedrock-agent-runtime') and calls retrieve / retrieve_and_generate -> HTTP 200, no X-Amz-Function-Error, and a payload of {'ok': true, 'results': [...text containing the ingested fact...]} / {'ok': true, 'answer': <text grounded in the document>, 'citation_count': >=1, 'refs': [...text containing the fact...]}. The client object always constructs.
Cause 2 — actual (emulator): HTTP 200, handler runs, payload reports botocore's client-construction failure:
- bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-knowledge-base-positive: AssertionError: {'ok': False, 'code': 'UnknownServiceError'} at `assert body["ok"] is True`.
- bedrock-agent__lambda__opensearchserverless.lambda-bedrock-agent-retrieve-and-generate-positive: AssertionError: {'ok': False, 'code': 'UnknownServiceError'} at `assert body["ok"] is True`.
Fixed text of the diverging message (botocore's own wording, surfaced by the handler): "Unknown service: '<service-name>'. Valid service names are: <installed model list>". Variable parts: the service id requested by the handler (bedrock-agent-runtime) and the installed-model list. Generated function names, role names, collection/knowledge-base ids, request ids and timestamps vary per run and are not requirements.

## Steps to Reproduce
Cause 1:
1. iam CreateRole trusting bedrock.amazonaws.com with aoss:APIAccessAll, bedrock:InvokeModel and s3:GetObject/s3:ListBucket.
2. opensearchserverless CreateSecurityPolicy (encryption + network), CreateAccessPolicy(type='data') listing that role ARN with Permission aoss:APIAccessAll, CreateCollection(type=VECTORSEARCH); poll until ACTIVE. Verify GetAccessPolicy(name=<policy>, type='data') returns the role in Principal and aoss:* / aoss:APIAccessAll in Rules[*].Permission.
3. Sign a PUT of a vector index against the collection endpoint as that role.
4. s3 CreateBucket + PutObject of a document containing a marker sentence.
5. bedrock-agent CreateKnowledgeBase pointing at the collection ARN and index; poll until ACTIVE. CreateDataSource(type='S3', s3Configuration.bucketArn=arn:aws:s3:::<bucket>). StartIngestionJob and poll until status COMPLETE.
6. bedrock-agent-runtime Retrieve(knowledgeBaseId=<kb>, retrievalQuery={'text': '<question about the marker>'}).
7. Observe HTTP 501 / NotImplemented instead of HTTP 200 with retrievalResults containing the marker.

Cause 2:
1. Build a ready knowledge base as in Cause 1 (or any KB the emulator will serve).
2. iam CreateRole with a lambda.amazonaws.com trust policy, AWSLambdaBasicExecutionRole attached, plus an inline policy allowing bedrock:Retrieve (and, for the second arm, bedrock:RetrieveAndGenerate + bedrock:InvokeModel + bedrock:GetInferenceProfile).
3. lambda CreateFunction(Runtime=python3.12, Handler=index.handler, Code.ZipFile=<zip of index.py>) where index.py is:
       import boto3
       def handler(event, context):
           client = boto3.client('bedrock-agent-runtime', region_name=event['region'])
           try:
               resp = client.retrieve(knowledgeBaseId=event['kb_id'], retrievalQuery={'text': event['query']}, retrievalConfiguration={'vectorSearchConfiguration': {'numberOfResults': 3}})
               return {'ok': True, 'results': [r['content']['text'] for r in resp.get('retrievalResults', [])]}
           except Exception as e:
               code = getattr(e, 'response', {}).get('Error', {}).get('Code', type(e).__name__)
               return {'ok': False, 'code': code}
   Poll GetFunction until Configuration.State == Active.
4. lambda Invoke(FunctionName=<fn>, InvocationType=RequestResponse, Payload={'region': <region>, 'kb_id': <kb>, 'query': '<question about the ingested fact>'}).
5. Observe HTTP 200 whose payload is {'ok': false, 'code': 'UnknownServiceError'} instead of {'ok': true, 'results': [...]}. Substituting client.retrieve_and_generate(input={'text': ...}, retrieveAndGenerateConfiguration={'type':'KNOWLEDGE_BASE','knowledgeBaseConfiguration':{'knowledgeBaseId':<kb>,'modelArn':<inference-profile arn>}}) reproduces identically.
