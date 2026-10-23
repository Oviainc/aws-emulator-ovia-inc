Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.deploy-apigateway-2-create-rollback-removes-rest-api apigateway.deploy-authorizer-cfn-enforces-identity-source apigateway.deploy-rest-api-cfn-stack-invoke-behaves apigateway.deploy-stage-with-variables-cfn-behaves apigateway__cloudformation__lambda.cfn-apigw-stage-deployment-invocation-positive apigateway.deploy-apigateway-2-control-stack-serves-https apigateway.apikey-not-in-usage-plan-rejected apigateway.deploy-usage-plan-cfn-behaves apigateway.deploy-usage-plan-cfn-quota-enforced apigateway.usage-plan-missing-api-key-rejected apigateway.usage-plan-throttle-exceeded-rejected apigateway.deploy-apigateway-1-cfn-deployment-snapshot-ordering
Issue-Key: api-gateway|GetRestApi|NotFoundException
Issue-Key: api-gateway|PutMethod|NotFoundException
Issue-Key: api-gateway|GetStage|NotFoundException
Issue-Key: apigateway__cloudformation__lambda|GetStage|NotFoundException
Issue-Key: apigateway.deploy-apigateway-2-control-stack-serves-https|run#11|AssertionError|assert named == [api_id], (
Issue-Key: api-gateway|GetApiKey|NotFoundException
Issue-Key: apigateway.deploy-apigateway-1-cfn-deployment-snapshot-ordering|run#16|AssertionError|assert m_done is not None and d_start is not None, (
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# CloudFormation-provisioned API Gateway resources are unaddressable through the API Gateway API: Ref'd REST API ids and ApiKeys resolve to nothing, and stack events carry no resource events (4 causes)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRestApi

## Description
Four root causes, all on the CloudFormation -> API Gateway provisioning seam, all grounded in failing checks that the suite marked neither ALREADY FILED nor OWNED. Related open issues describe the same class for OTHER services (20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md, 20261023T075118Z-cloudformation-read-models-omit-drift-results-type-registry.md, 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md); none of them carries these API Gateway replay keys, so a fixer closing those would never re-run the checks below.

Cause 1 of 4 — the physical resource id of AWS::ApiGateway::RestApi is '<account>:<stack-api-name>', not the REST API id, so every follow-up API Gateway call on the Ref'd value 404s.
AWS: Smithy GetRestApi/GetStage/PutMethod take restApiId as an @httpLabel String and model NotFoundException (HTTP 404) only for an id that names no API. In real AWS a stack's RestApi Ref is the 10-character API id, which GetRestApi, GetStage and PutMethod all resolve.
Emulator: GetRestApi, GetStage and PutMethod answer 404 NotFoundException with 'Invalid API identifier specified 000000000000:<stack-api-name>' — the id handed back for the stack's RestApi is the account id joined to the template name, which the API Gateway store does not know.
Why: no CloudFormation-deployed REST API can be read, staged, or extended through the API Gateway API; every deploy-then-verify workflow fails at the first read.
Fix: the CloudFormation resource provider for AWS::ApiGateway::RestApi must register the API in API Gateway state and return its real id as the physical resource id.
Traces: apigateway.deploy-apigateway-2-create-rollback-removes-rest-api, apigateway.deploy-authorizer-cfn-enforces-identity-source, apigateway.deploy-rest-api-cfn-stack-invoke-behaves, apigateway.deploy-stage-with-variables-cfn-behaves, apigateway__cloudformation__lambda.cfn-apigw-stage-deployment-invocation-positive.

Cause 2 of 4 — GetRestApis does not list a REST API a stack created.
AWS: recorded behavior only — after a stack with an AWS::ApiGateway::RestApi reaches CREATE_COMPLETE, GetRestApis contains exactly one API with that name.
Emulator: GetRestApis returns [] for that name while the stack reports CREATE_COMPLETE.
Why: discovery by name — the usual way tooling finds a stack's API — returns nothing, so the API is invisible even though the stack claims success.
Fix: same provider as cause 1; the created API must land in the API Gateway list/state.
Traces: apigateway.deploy-apigateway-2-control-stack-serves-https.

Cause 3 of 4 — an AWS::ApiGateway::ApiKey created by a stack cannot be fetched with GetApiKey.
AWS: Smithy GetApiKey (GET /apikeys/{apiKey}) models NotFoundException (HTTP 404) for an unknown key; in real AWS the stack's ApiKey physical id is the key id GetApiKey resolves.
Emulator: GetApiKey answers 404 NotFoundException 'Invalid API Key identifier specified' for the key the stack reports as created.
Why: every usage-plan / API-key metering scenario built from a template dies before it can exercise throttling or quota behavior.
Fix: the CloudFormation resource provider for AWS::ApiGateway::ApiKey (and the UsagePlan/UsagePlanKey providers that reference it).
Traces: apigateway.apikey-not-in-usage-plan-rejected, apigateway.deploy-usage-plan-cfn-behaves, apigateway.deploy-usage-plan-cfn-quota-enforced, apigateway.usage-plan-missing-api-key-rejected, apigateway.usage-plan-throttle-exceeded-rejected.

Cause 4 of 4 — the stack event stream contains only stack-level events; no per-resource events are emitted.
AWS: recorded behavior only — DescribeStackEvents reports a CREATE_IN_PROGRESS/CREATE_COMPLETE pair per logical resource, in dependency order, interleaved with the stack's own events.
Emulator: the whole stream for the stack was [(stack, CREATE_IN_PROGRESS), (stack, CREATE_COMPLETE)] — the expected resource events (e.g. a Method CREATE_COMPLETE before a Deployment CREATE_IN_PROGRESS) are absent.
Why: a client cannot observe resource ordering or attribute a failure to a resource; snapshot-ordering guarantees between a Method and the Deployment that snapshots it are unobservable.
Fix: event recording in the CloudFormation stack executor (per-resource events around each resource's provisioning).
Traces: apigateway.deploy-apigateway-1-cfn-deployment-snapshot-ordering.

## Evidence
Cause 1: botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the GetStage operation: Invalid API identifier specified 000000000000:sim2real-e2e-api-c7d5e4aa2b5c (same shape for GetRestApi and PutMethod).
Cause 2: AssertionError: GetRestApis name search for 'sim2real-behav-agw-ctl-<id>-api' returned [], expected exactly the stack's RestApi.
Cause 3: botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the GetApiKey operation: Invalid API Key identifier specified.
Cause 4: AssertionError: missing events: OldMethod CREATE_COMPLETE, DeploymentV1 CREATE_IN_PROGRESS; stream was [(stack,'CREATE_IN_PROGRESS'), (stack,'CREATE_COMPLETE')].

## Steps to Reproduce
1. CreateStack with a template declaring AWS::ApiGateway::RestApi (+ Resource/Method/Deployment/Stage), wait for CREATE_COMPLETE, then call GetRestApi/GetStage with the Ref'd RestApi value — expect 200, observe 404 'Invalid API identifier specified <account>:<name>'.
2. Call GetRestApis and search by the template's API name — expect one item, observe [].
3. Declare AWS::ApiGateway::ApiKey + UsagePlan, then GetApiKey on the stack's key id — expect 200, observe 404.
4. Call DescribeStackEvents for any of those stacks — expect per-resource CREATE_IN_PROGRESS/CREATE_COMPLETE events, observe stack-level events only.
