Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each
Issue-Key: apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each|_poll#10|Timeout|time.sleep(interval_s)
Issue-Commit: bc4d78eb61f619d05d76afe3f781275ae6918fe7

# apigateway__dynamodb__lambda: a signed GET on a deployed AWS_PROXY stage never produces any HTTP response, so repeat visits and their per-visit stream INSERTs are unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `bc4d78eb61f619d05d76afe3f781275ae6918fe7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration

## Description
Cause 1 of 1: a SigV4-signed GET to a deployed REST stage whose GET method has an AWS_PROXY Lambda integration never produces any HTTP response at all, so the whole serverless-ingest behaviour behind it (two visits landing as two distinct items, one DynamoDB-Streams INSERT per visit with no OldImage) reaches no verdict.

This failing check is marked neither ALREADY FILED nor OWNED by the replay report. The open issues 20261029T235159Z-apigateway-dynamodb-lambda-2-causes-the-malformed-proxy.md, 20261031T023113Z-apigateway-dynamodb-lambda-the-request-validator-rejection-check.md and 20261102T091623Z-apigateway-18-further-requests-to-a-deployed-rest.md describe the same surface failing to answer, but they carry other checks' replay keys and do not cover this one.

AWS: Smithy (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json) models the control plane only — PutIntegration.type is the IntegrationType enum and takes AWS_PROXY, PutMethod.authorizationType takes AWS_IAM, CreateDeployment/CreateStage publish the stage. The execute-api data plane is not modelled, so recorded behavior only: a GET to <stage-invoke-url>/visit that is SigV4-signed for the `execute-api` service with the caller's credentials is authorized, the integration function is invoked with a proxy event whose requestContext.identity.sourceIp is the caller's address, and the stage answers HTTP 200 with the handler's envelope body ({"ip": ..., "seen_at": ...} here, Content-Type application/json). Each invocation's PutItem is committed, so two visits produce two items under the same `ip` hash key with distinct `seen_at` range keys, and the table's NEW_AND_OLD_IMAGES stream carries exactly one eventName INSERT per visit, each with a NewImage and NO OldImage.
Emulator: no response is ever produced for the signed GET. The check polls the stage invoke URL until it sees status 200 and is killed by the harness at its 60 s cap — reported as `Error: Check timed out after 60.0s`. No status line, no body, so neither the 200, nor the proxy event's sourceIp, nor the two items, nor the stream records are ever observed.
Why: a client calling an IAM-authorized proxy API gets no answer and no error — it hangs until its own socket timeout instead of receiving 200 (or a 403/502 refusal). Because nothing is returned, the downstream contract an ingest pipeline depends on is equally unobservable: that a repeated visit is a new item rather than an overwrite, and that the stream reports it as a single INSERT with no OldImage (a stream consumer counting visits would miscount if the write were served as a MODIFY or carried an OldImage).
Fix: the execute-api data plane's request handling for a deployed stage — stage/resource/method resolution from the invoke URL, SigV4 (AWS_IAM) authorization of the request, construction of the proxy event (including requestContext.identity.sourceIp), the Lambda invocation, and translation of the returned {statusCode, headers, body} envelope — must terminate the request with an HTTP response instead of leaving the connection open.
Traces: apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each.

## Evidence
Check apigateway__dynamodb__lambda.apigw-repeat-visits-land-as-distinct-items-and-stream-one-insert-each (verdict: mismatch):
Error: Check timed out after 60.0s

Setup performed by the check: CreateTable with hash key `ip` (S) + range key `seen_at` (S), BillingMode PAY_PER_REQUEST and StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES}; wait for TableStatus ACTIVE and capture LatestStreamArn; CreateFunction whose handler reads requestContext.identity.sourceIp, PutItem's {ip, seen_at, path} and returns {"statusCode":200, "headers":{"Content-Type":"application/json"}, "body": json({"ip":..., "seen_at":...})}; CreateRestApi with resource /visit, GET method, PutIntegration type=AWS_PROXY integrationHttpMethod=POST to that function, lambda AddPermission for apigateway.amazonaws.com, then CreateDeployment with stageName=prod.

Expected (AWS): each of two SigV4-signed (`execute-api`) GETs on <stage-url>/visit answers HTTP 200 with a JSON body carrying the caller's IP (never 0.0.0.0) and a distinct seen_at; a ConsistentRead Query on `ip` returns exactly those 2 items; reading the stream from TRIM_HORIZON yields exactly one record per visit with eventName INSERT, Keys matching the two written keys, NewImage.path == "/visit", and no record carrying an OldImage.

Actual (emulator): the signed GET never reaches any status — the poll for "the stage answers 200" never succeeds and the harness kills the check at the 60 s cap. No status, no body, no items, no stream records observed.

## Steps to Reproduce
1. CreateTable: KeySchema ip(HASH,S) + seen_at(RANGE,S), BillingMode=PAY_PER_REQUEST, StreamSpecification={StreamEnabled:true, StreamViewType:NEW_AND_OLD_IMAGES}. Poll DescribeTable until TableStatus=ACTIVE and keep LatestStreamArn.
2. CreateFunction (python) with a handler that takes requestContext.identity.sourceIp, PutItem's {ip, seen_at, path} into the table and returns {"statusCode":200,"headers":{"Content-Type":"application/json"},"body":json({"ip":ip,"seen_at":stamp})}, with VISITOR_TABLE in its environment.
3. CreateRestApi; CreateResource pathPart=visit; PutMethod GET; PutIntegration type=AWS_PROXY integrationHttpMethod=POST uri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations; lambda AddPermission for principal apigateway.amazonaws.com; CreateDeployment stageName=prod.
4. Issue two GETs to <stage-invoke-url>/visit, each SigV4-signed for service `execute-api` with the run's credentials. Expected: HTTP 200 each with the handler's JSON body and a non-0.0.0.0 caller IP. Observe that neither request ever completes.
5. Query the table with ConsistentRead on the returned ip: expected the 2 items returned by the two visits.
6. DescribeStream/GetShardIterator(TRIM_HORIZON)/GetRecords on LatestStreamArn: expected exactly 2 records, both eventName=INSERT, no OldImage, NewImage.path=="/visit".
