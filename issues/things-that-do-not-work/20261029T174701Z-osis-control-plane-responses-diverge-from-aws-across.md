Issue-Source: trace-sweep-divergence
Issue-Trace: osis-catalog-list-pipeline-blueprints
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: osis-catalog-list-pipeline-blueprints osis-pipeline-crud-error-paths
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis|ListPipelineBlueprints|2xx->2xx|$.Blueprints[*]
Issue-Key: osis|CreatePipeline|4xx->4xx|$.message
Issue-Key: osis|CreatePipeline|4xx->2xx|$.message
Issue-Key: osis|CreatePipeline|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: osis|CreatePipeline|4xx->2xx|status
Issue-Key: osis|StartPipeline|4xx->4xx|$.message
Issue-Key: osis|StopPipeline|4xx->4xx|$.message
Issue-Key: osis|ListPipelines|2xx->2xx|$.Pipelines[*]
Issue-Key: osis|ListPipelines|2xx->4xx|$.Pipelines
Issue-Key: osis|ListPipelines|2xx->4xx|status
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# osis control-plane responses diverge from AWS across blueprints, errors, validation and listing

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipelineBlueprints

## Description
Replay of the OSIS (Amazon OpenSearch Ingestion) wire corpus found SEVEN distinct, independently fixable divergences in the pipeline control plane. This issue bundles all seven; each must be closed before submitting. The one other open osis issue (20261021T234042Z-osis-validatepipeline-stub.md, ValidatePipeline scaffold stub) covers NONE of them.

--- Cause 1 of 7: ListPipelineBlueprints serves an invented blueprint catalog ---
AWS: recorded behavior only. The Smithy model (ListPipelineBlueprints -> ListPipelineBlueprintsResponse.Blueprints: PipelineBlueprintsSummaryList of PipelineBlueprintSummary{BlueprintName, DisplayName, DisplayDescription, Service, UseCase}) constrains the SHAPE but not the catalog contents. The recording shows AWS returns 200 with a fixed, service-owned catalog of 49 summaries (indices $.Blueprints[0]..[48]) ordered by BlueprintName, e.g. [0] = {BlueprintName 'AWS-AlbAccessLogS3Pipeline', DisplayName 'ALB access logs', DisplayDescription 'Extracts data from ALB access logs.', Service 'ALB Logs', UseCase 'Log Analytics'}, [1] = 'AWS-ApacheLogEnrichment' (DisplayName 'GeoIP enrichment for Apache logs', Service 'Apache Logs', UseCase 'GeoIP'), [2] = 'AWS-ApacheLogPipeline' (DisplayName 'Apache logs', DisplayDescription "Extracts logs by matching the value in the 'log' key against the grok common Apache log pattern.", Service 'Apache Logs'), [3] = 'AWS-ApacheLogSampling'. The catalog is a published constant of the service, not per-account state.
Emulator: also 200, but with a shorter hand-written list of its own invented blueprints in a different order ('AWS-ApacheLogPipeline', 'AWS-CloudTrailLogPipeline', 'AWS-DynamoDBChangeDataCapturePipeline', ...). Every one of the 49 expected positions mismatches. Even where a BlueprintName coincides, its metadata is wrong: for 'AWS-ApacheLogPipeline' the emulator serves DisplayName 'Apache log ingestion', Service 'Apache HTTP Server' and a long prose DisplayDescription instead of AWS's DisplayName 'Apache logs', Service 'Apache Logs' and the one-sentence grok description.
Why: callers pick a blueprint by BlueprintName from this list and then call GetPipelineBlueprint/CreatePipeline with it. Against the emulator, a client discovers names AWS does not have and cannot discover the ones it does, so blueprint-driven pipeline creation (console flows, IaC that resolves a blueprint by Service/UseCase) silently targets nonexistent templates; UI code that keys on Service/UseCase buckets groups them wrongly.
Fix: the ListPipelineBlueprints handler's static catalog data (and the GetPipelineBlueprint lookup table that must agree with it) must be replaced with the real AWS blueprint catalog, same entries, same field values, same ordering.
Traces: osis-catalog-list-pipeline-blueprints#task#1.

--- Cause 2 of 7: CreatePipeline MinUnits>MaxUnits ValidationException message is not AWS's wording ---
AWS: Smithy CreatePipeline requires MinUnits and MaxUnits (PipelineUnits, @range min 1) and models ValidationException (httpError 400, member `message`). The recording shows a request with MinUnits greater than MaxUnits is rejected 400 with the message 'minUnits cannot be greater than maxUnits' — that string is entirely FIXED text, with NO variable parts: AWS does not interpolate the two values and uses the lower-camel wire-independent spelling 'minUnits'/'maxUnits'.
Emulator: 400 (correct status) but message 'MaxUnits (1) must be greater than or equal to MinUnits (5)' — different wording, inverted phrasing, member names capitalized, and the submitted unit counts interpolated into text AWS keeps constant.
Why: the message is part of the error contract. Clients and test suites that assert on or log this message (and humans reading it) see a string AWS never emits; anything matching 'minUnits cannot be greater than maxUnits' fails to classify the error.
Fix: CreatePipeline's input validation in the osis handler — emit ValidationException with exactly the fixed text 'minUnits cannot be greater than maxUnits'.
Traces: osis-pipeline-crud-error-paths#task#2.

--- Cause 3 of 7: CreatePipeline accepts a pipeline whose STS role does not exist (200 instead of 400) ---
AWS: Smithy CreatePipeline models ValidationException (400) and carries PipelineConfigurationBody (@length 1..100000) plus PipelineRoleArn. The recording shows AWS validates the sts_role_arn referenced by the pipeline configuration body before creating anything and fails the call 400 with message 'pipelineConfigurationBody is invalid with the following errors: [ValidationMessage(message=Unable to assume the STS role <roleArn>. This role must contain a trust relationship with 'osis-pipelines.amazonaws.com'.)]', and with an x-amzn-errortype response header naming the error shape. Fixed text: everything except <roleArn>, which is the role ARN taken verbatim from the request's pipeline configuration body (in the recording an absent role under account 000000000000); the surrounding 'pipelineConfigurationBody is invalid with the following errors: [ValidationMessage(message=...)]' envelope and the service principal 'osis-pipelines.amazonaws.com' are constant.
Emulator: returns 200 and CREATES the pipeline. No error, no message member, and no x-amzn-errortype header at all.
Why: this is the corpus' worst divergence — a failure path becomes a success path. A client testing its error handling for a mis-configured or missing pipeline role gets a healthy pipeline instead of a rejection, so misconfiguration is only discovered in production; worse, the bogus pipeline enters emulator state and pollutes every later call (see Cause 6).
Fix: CreatePipeline's handler/validation must validate the sts_role_arn in the pipeline configuration body (and PipelineRoleArn) against the role state before persisting, and reject with 400 ValidationException + the x-amzn-errortype header rather than creating the pipeline.
Traces: osis-pipeline-crud-error-paths#task#3.

--- Cause 4 of 7: StartPipeline on a nonexistent pipeline returns the wrong ResourceNotFoundException message ---
AWS: Smithy StartPipeline (PUT /2022-01-01/osis/startPipeline/{PipelineName}) models ResourceNotFoundException, httpError 404, member `message` ('You attempted to access or delete a resource that does not exist.'). The recording shows the message is 'No pipeline settings found' — entirely FIXED text; AWS deliberately does NOT echo the requested PipelineName for this operation.
Emulator: 404 (correct status) with message 'Pipeline <name> could not be found' (recorded instance: 'Pipeline sim2real-probe-plmi-004e3418 could not be found'), i.e. a generic template of its own that interpolates the requested pipeline name.
Why: wrong error text for a modeled error; log scrapers and client-side error matchers keyed to AWS's wording do not match, and callers see a name echoed where AWS reveals none.
Fix: StartPipeline's handler not-found path — return ResourceNotFoundException with exactly 'No pipeline settings found'.
Traces: osis-pipeline-crud-error-paths#task#7.

--- Cause 5 of 7: StopPipeline on a nonexistent pipeline returns the wrong ResourceNotFoundException message ---
AWS: Smithy StopPipeline (PUT /2022-01-01/osis/stopPipeline/{PipelineName}) models ResourceNotFoundException, httpError 404, member `message`. The recording shows the message 'The pipeline <pipelineName> could not be stopped because it does not exist' (recorded instance: 'The pipeline sim2real-probe-plmi-004e3418 could not be stopped because it does not exist'). Fixed text: 'The pipeline ' ... ' could not be stopped because it does not exist'; the single variable part is <pipelineName>, the PipelineName httpLabel from the request. Note this differs from StartPipeline's wording (Cause 4) — the two operations do NOT share one message, so a single shared not-found string is wrong for both.
Emulator: 404 with the same generic 'Pipeline <name> could not be found' it uses for StartPipeline.
Why: same error-contract break as Cause 4, and it shows the emulator funnels every pipeline lookup miss through one message instead of the per-operation wording AWS uses.
Fix: StopPipeline's handler not-found path — return ResourceNotFoundException with 'The pipeline <PipelineName> could not be stopped because it does not exist', interpolating only the requested name.
Traces: osis-pipeline-crud-error-paths#task#8.

--- Cause 6 of 7: ListPipelines reports a pipeline that should not exist ---
AWS: Smithy ListPipelines -> ListPipelinesResponse.Pipelines (PipelineSummaryList). The recording shows 200 with Pipelines empty ([]) at this point in the fixture: no pipeline exists in the account/Region, because the creates that preceded it were rejected and/or the created ones were deleted.
Emulator: 200 with one extra summary at $.Pipelines[0] — a pipeline in Status 'CREATING' (StatusReason 'The pipeline is being created.', MinUnits 1, MaxUnits 1, Tags [], a pipeline ARN and created/updated timestamps). The emulator's pipeline state retains a pipeline AWS never created.
Why: list output is how clients reconcile desired vs actual state. A phantom pipeline makes idempotent create/delete loops and IaC drift detection diverge: the client sees a resource it must delete (and DeletePipeline/GetPipeline then behave inconsistently with AWS), or refuses to create the one it needs because a name appears taken.
Hypothesis: this is the state left behind by Cause 3 (the CreatePipeline the emulator wrongly accepted), so fixing Cause 3 may remove the leaked entry — but it must be confirmed by replaying this step, and the ListPipelines/state path must not retain pipelines whose create failed, nor pipelines already deleted.
Fix: the osis pipeline state store and the ListPipelines handler that projects it — a pipeline must enter state only on a create AWS would accept, and leave it on delete.
Traces: osis-pipeline-crud-error-paths#task#10.

--- Cause 7 of 7: ListPipelines rejects a request AWS accepts (400 instead of 200 with an empty list) ---
AWS: Smithy ListPipelines (GET /2022-01-01/osis/listPipelines, http code 200) takes only the optional pagination inputs MaxResults (httpQuery maxResults, @range 1..100) and NextToken (httpQuery nextToken, @length 0..3000, pattern ^([\s\S]*)$), and models exactly two 400s: ValidationException and InvalidPaginationTokenException. The recording shows this request is well-formed for AWS: it answers 200 with $.Pipelines = [] (an empty account listing is a success, never an error).
Emulator: answers 400 and the body has no Pipelines member at all (expected [], got None).
Why: a listing that 4xx's where AWS returns an empty page breaks the normal pagination/empty-account path — SDK paginators surface a client error instead of terminating cleanly, so any caller that enumerates pipelines (drift detection, cleanup, console listing) fails outright rather than seeing zero results.
Fix: the ListPipelines handler's request/pagination parameter validation — accept the modeled maxResults/nextToken inputs within their modeled ranges and return 200 with an empty Pipelines list when no pipelines exist, reserving 400 for ValidationException/InvalidPaginationTokenException.
Traces: osis-pipeline-crud-error-paths#task#11.

## Evidence
replay_wire_corpus (trace_set simparity-wire-aws-v1): emulator_up=true, bootstrap_signal=false, steps_total=22, steps_passed=4, steps_mismatched=18, divergence_count=7, score_pct=18.2. All 7 divergence clusters are service=osis; none carries stub_path or owned_by.

1) osis-catalog-list-pipeline-blueprints#task#1 ListPipelineBlueprints, verdict=mismatch, status 200->200. detail: "$.Blueprints[0]: missing (expected {'BlueprintName': 'AWS-AlbAccessLogS3Pipeline', 'DisplayDescription': 'Extracts data from ALB access logs.', 'DisplayName': 'ALB access logs', 'Service': 'ALB Logs', 'UseCase': 'Log Analytics'}, got {'BlueprintName': 'AWS-ApacheLogPipeline', 'DisplayName': 'Apache log ingestion', 'DisplayDescription': 'Ingest Apache access logs over HTTP, parse them with grok, and write them to an OpenSearch index.', 'Service': 'Apache HTTP Server', 'UseCase': 'Log Analytics'})"; same for $.Blueprints[1] (expected 'AWS-ApacheLogEnrichment', got 'AWS-CloudTrailLogPipeline'), [2] (expected 'AWS-ApacheLogPipeline' with DisplayName 'Apache logs', got 'AWS-DynamoDBChangeDataCapturePipeline'), [3] (expected 'AWS-ApacheLogSampling'). divergence_key keys list covers $.Blueprints[0]..$.Blueprints[48] — all 49 positions diverge.

2) osis-pipeline-crud-error-paths#task#2 CreatePipeline, status 400->400. detail: "$.message: value_mismatch (expected 'minUnits cannot be greater than maxUnits', got 'MaxUnits (1) must be greater than or equal to MinUnits (5)')".

3) osis-pipeline-crud-error-paths#task#3 CreatePipeline, expected_status=400, actual_status=200. detail: "status: expected 400, got 200 / $.message: missing (expected \"pipelineConfigurationBody is invalid with the following errors: [ValidationMessage(message=Unable to assume the STS role arn:aws:iam::000000000000:role/sim2real-osis-absent-role. This role must contain a trust relationship with 'osis-pipelines.amazonaws.com'.)]\", got None) / $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)".

4) osis-pipeline-crud-error-paths#task#7 StartPipeline, status 404->404. detail: "$.message: value_mismatch (expected 'No pipeline settings found', got 'Pipeline sim2real-probe-plmi-004e3418 could not be found')".

5) osis-pipeline-crud-error-paths#task#8 StopPipeline, status 404->404. detail: "$.message: value_mismatch (expected 'The pipeline sim2real-probe-plmi-004e3418 could not be stopped because it does not exist', got 'Pipeline sim2real-probe-plmi-004e3418 could not be found')".

6) osis-pipeline-crud-error-paths#task#10 ListPipelines, status 200->200. detail: "$.Pipelines[0]: extra (expected None, got {'Status': 'CREATING', 'StatusReason': {'Description': 'The pipeline is being created.'}, 'PipelineName': 'sim2real-probe-plno-004e3418', 'PipelineArn': 'arn:aws:osis:us-west-2:000000000000:pipeline/sim2real-probe-plno-004e3418', 'MinUnits': 1, 'MaxUnits': 1, 'CreatedAt': ..., 'LastUpdatedAt': ..., 'Tags': []})" (generated ARN/timestamps are per-run and not themselves the finding; the extra element is).

7) osis-pipeline-crud-error-paths#task#11 ListPipelines, expected_status=200, actual_status=400. detail: "status: expected 200, got 400 / $.Pipelines: missing (expected [], got None)".

Model grounding (services/service-com-amazonaws-osis-amazonopensearchingestionservice/smithy-model/com-amazonaws-osis-amazonopensearchingestionservice.json, sdkId OSIS, restJson1): CreatePipeline requires PipelineName/MinUnits/MaxUnits/PipelineConfigurationBody and models ValidationException(400), ResourceAlreadyExistsException(409), ResourceNotFoundException(404); StartPipeline/StopPipeline model ResourceNotFoundException(404) with member `message`; ListPipelines models only MaxResults(@range 1..100)/NextToken(@length 0..3000) and the 400s ValidationException and InvalidPaginationTokenException; PipelineBlueprintSummary = {BlueprintName, DisplayName, DisplayDescription, Service, UseCase}.

## Steps to Reproduce
Boot the emulator from this checkout and replay the osis fixtures of trace set simparity-wire-aws-v1 (replay_wire_corpus).

Cause 1: POST /2022-01-01/osis/listPipelineBlueprints (ListPipelineBlueprints) with an empty body -> fixture osis-catalog-list-pipeline-blueprints step task#1. Compare the returned Blueprints list element-by-element with the recorded 49-entry AWS catalog.

Causes 2-7 all live in fixture osis-pipeline-crud-error-paths:
- task#2: POST /2022-01-01/osis/createPipeline with MinUnits greater than MaxUnits -> expect 400 ValidationException message 'minUnits cannot be greater than maxUnits'.
- task#3: POST /2022-01-01/osis/createPipeline whose PipelineConfigurationBody references an sts_role_arn for a role that does not exist -> expect 400 with the 'pipelineConfigurationBody is invalid with the following errors: [ValidationMessage(message=Unable to assume the STS role <roleArn>. ...)]' message and an x-amzn-errortype header; the emulator returns 200 and creates the pipeline.
- task#7: PUT /2022-01-01/osis/startPipeline/{PipelineName} for a pipeline that does not exist -> expect 404 message 'No pipeline settings found'.
- task#8: PUT /2022-01-01/osis/stopPipeline/{PipelineName} for a pipeline that does not exist -> expect 404 message 'The pipeline <PipelineName> could not be stopped because it does not exist'.
- task#10: GET /2022-01-01/osis/listPipelines -> expect 200 with Pipelines == []; the emulator lists one phantom CREATING pipeline.
- task#11: GET /2022-01-01/osis/listPipelines (the fixture's second listing, with its recorded pagination query parameters) -> expect 200 with Pipelines == []; the emulator returns 400 with no Pipelines member.

Re-replay fixture osis-catalog-list-pipeline-blueprints step task#1 and fixture osis-pipeline-crud-error-paths steps task#2, task#3, task#7, task#8, task#10 and task#11 to confirm all seven causes are closed.
