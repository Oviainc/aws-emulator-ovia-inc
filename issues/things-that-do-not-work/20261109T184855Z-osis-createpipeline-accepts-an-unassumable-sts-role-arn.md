Issue-Source: trace-sweep-divergence
Issue-Trace: osis-pipeline-crud-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: osis-pipeline-crud-error-paths
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis|CreatePipeline|4xx->2xx|$.Pipeline.IngestEndpointUrls[*]
Issue-Related: 20261029T174701Z-osis-control-plane-responses-diverge-from-aws-across.md
Issue-Commit: a6e21bc74f65c989ddc253a2698c32bee6b937c3

# osis CreatePipeline accepts an unassumable sts_role_arn and creates the pipeline instead of returning ValidationException

> _Note: this issue was identified against the code base at commit `a6e21bc74f65c989ddc253a2698c32bee6b937c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipeline

## Description
osis CreatePipeline performs no semantic validation of the submitted PipelineConfigurationBody: a configuration naming an IAM role the service cannot assume is accepted and a pipeline is created.

Cause 1 of 1: CreatePipeline does not reject a pipeline configuration whose sink/source `sts_role_arn` names a role that does not exist or does not trust the ingestion service principal.

AWS: CreatePipeline (Smithy: POST /2022-01-01/osis/createPipeline, success code 200) models `ValidationException` (errorKind=client, httpError=400, single `message` member of type ErrorMessage) and documents it as "An exception for missing or invalid input fields". `PipelineConfigurationBody` is @required with @length min 1 / max 100000, and the pipeline configuration body embeds the STS role the pipeline assumes to reach its sink; `PipelineRoleArn` is constrained to pattern ^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b|aws-iso-e|aws-iso-f):iam::[0-9]+:role\/.*$. Before accepting the body AWS resolves that role and requires it to carry a trust relationship with the OpenSearch Ingestion service principal; when it cannot, the call fails synchronously with HTTP 400, header `x-amzn-errortype: ValidationException`, and a `message` whose FIXED text is "pipelineConfigurationBody is invalid with the following errors: [ValidationMessage(message=Unable to assume the STS role <roleArn>. This role must contain a trust relationship with 'osis-pipelines.amazonaws.com'.)]" — the only VARIABLE part is <roleArn>, the role ARN taken verbatim from the submitted configuration body (in the recorded step that value was an absent role in the caller's account). The wrapper mirrors the ValidationMessage list shape ValidatePipeline returns (ValidationMessage.Message, Smithy shape com.amazonaws.osis#ValidationMessage), so the same validator feeds both operations. No pipeline is created and nothing is persisted.

Emulator: the replayed step got HTTP 200 with a full CreatePipeline success payload — a `Pipeline` object including populated `IngestEndpointUrls` — and no `message` field and no `x-amzn-errortype` response header. The role reference in the configuration body is never resolved, so an unassumable role is indistinguishable from a valid one.

Why: a client that relies on AWS's behaviour treats HTTP 200 as proof that its IAM wiring is correct. Against the emulator the misconfigured pipeline is created and reported healthy, so every negative test of role/trust-policy misconfiguration passes when it must fail, error-handling branches that parse ValidationException are never exercised, and the client proceeds to publish to a pipeline whose sink can never be written. The failure also surfaces later and in the wrong place rather than at the create call.

Fix: CreatePipeline's validation stage, ahead of state mutation in the CreatePipeline handler — resolve the sts_role_arn referenced by the submitted PipelineConfigurationBody against emulator IAM state, and when the role is absent or lacks a trust relationship with 'osis-pipelines.amazonaws.com' abort with the modelled ValidationException (400, x-amzn-errortype header, message rendered from the template above with the request's role ARN) and create no pipeline. The same validator should back ValidatePipeline so the two agree. Note the success payload this step wrongly returns also advertises `IngestEndpointUrls` on a real-AWS hostname the emulator does not serve; that endpoint-host defect is separately tracked by the open issue 20261109T133548Z-osis-getpipeline-advertises-ingestendpointurls-on-a-real-aws.md and is not this issue's cause — here the payload should not exist at all. The broader osis control-plane issue 20261029T174701Z-osis-control-plane-responses-diverge-from-aws-across.md does not cover this step (replay reports this cluster as unowned).

Traces: osis-pipeline-crud-error-paths#task#3 (CreatePipeline, expected 400, got 200).

## Evidence
Fixture osis-pipeline-crud-error-paths, step task#3, operation CreatePipeline.

Expected (recorded AWS response): HTTP 400; header x-amzn-errortype present (ValidationException); body $.message = "pipelineConfigurationBody is invalid with the following errors: [ValidationMessage(message=Unable to assume the STS role <roleArn>. This role must contain a trust relationship with 'osis-pipelines.amazonaws.com'.)]" where <roleArn> is the role ARN from the submitted configuration body; no Pipeline object.

Actual (emulator): HTTP 200; $.message missing; header x-amzn-errortype missing; a success $.Pipeline was returned, including $.Pipeline.IngestEndpointUrls[0] on a hostname the emulator does not serve (flagged host_not_own).

Replay verdict: mismatch — "status: expected 400, got 200; $.message: missing; $headers.x-amzn-errortype: missing".

Model citations: Smithy CreatePipeline errors include ValidationException (client, httpError 400); ValidationException has one member `message` (ErrorMessage); PipelineConfigurationBody @required @length(1,100000); PipelineRoleArn pattern ^arn:(aws|...):iam::[0-9]+:role\/.*$; com.amazonaws.osis#ValidationMessage.Message is the per-error message shape echoed inside the wrapper text.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Replay fixture osis-pipeline-crud-error-paths up to step task#3, i.e. POST /2022-01-01/osis/createPipeline with a PipelineConfigurationBody whose sts_role_arn names an IAM role that does not exist (or exists without a trust relationship with osis-pipelines.amazonaws.com).
3. Observe the emulator answers HTTP 200 with a created Pipeline (and IngestEndpointUrls) instead of HTTP 400 ValidationException with x-amzn-errortype: ValidationException and the "pipelineConfigurationBody is invalid with the following errors: [ValidationMessage(message=Unable to assume the STS role <roleArn>. This role must contain a trust relationship with 'osis-pipelines.amazonaws.com'.)]" message.
4. A follow-up ListPipelines/GetPipeline shows the pipeline that AWS would never have created.
