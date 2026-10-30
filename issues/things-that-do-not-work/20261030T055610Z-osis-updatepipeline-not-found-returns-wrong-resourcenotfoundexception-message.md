Issue-Source: trace-sweep-divergence
Issue-Trace: osis-pipeline-crud-error-paths
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: osis-pipeline-crud-error-paths
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis|UpdatePipeline|4xx->4xx|$.message
Issue-Commit: bfd8b7223bf6a1ab9b658956ab58262126f30b11

# osis UpdatePipeline not-found returns wrong ResourceNotFoundException message

> _Note: this issue was identified against the code base at commit `bfd8b7223bf6a1ab9b658956ab58262126f30b11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePipeline

## Description
Replay of the OSIS wire corpus found ONE net-new divergence not covered by any open issue: UpdatePipeline's not-found error message. (The other seven osis wire divergences in this corpus are already owned by 20261029T174701Z-osis-control-plane-responses-diverge-from-aws-across.md; 20261029T191455Z-osis-pipeline-error-paths-four-wrong-rejection-messages.md covers CreatePipeline/StartPipeline wording and concurrency but NOT UpdatePipeline; the remaining open osis issues are unimplemented-operation stubs.)

--- Cause 1 of 1: UpdatePipeline on a nonexistent pipeline returns the wrong ResourceNotFoundException message ---
AWS: Smithy UpdatePipeline (PUT /2022-01-01/osis/updatePipeline/{PipelineName}, success code 200, PipelineName @required @httpLabel @length 3..28 pattern ^[a-z][a-z0-9\-]+$) models ResourceNotFoundException with httpError 404 and member `message`. For the catalog/shape contract the model says nothing about the message text, so this is recorded behavior only: updating a pipeline name that does not exist in the account/Region is refused 404 with message 'Service returned error code ResourceNotFoundException'. That string is entirely FIXED text with NO variable parts — AWS does not echo the requested PipelineName for this operation, and notably words it differently from both StartPipeline ('No pipeline settings found') and StopPipeline ('The pipeline <PipelineName> could not be stopped because it does not exist'), so one shared lookup-miss message cannot be correct for all three.
Emulator: correct status 404 and (per the replay, only $.message diverged) the correct error shape, but message 'Pipeline <PipelineName> could not be found' — a generic template of its own that interpolates the requested pipeline name. In the recording the interpolated name was this run's generated probe name; the name itself is not part of the requirement, the template is.
Why: the error message is part of the modeled error contract. Clients, SDK error matchers, IaC providers and log scrapers that key on AWS's text do not classify this refusal, so an update against a deleted/never-created pipeline is reported as an unrecognized error instead of a known not-found; the emulator also discloses a resource name where AWS discloses none.
Fix: the UpdatePipeline handler's not-found path in the osis service crate — return ResourceNotFoundException (404) with exactly the fixed text 'Service returned error code ResourceNotFoundException', instead of routing through the shared 'Pipeline <name> could not be found' lookup-miss message used by the other pipeline operations.
Traces: osis-pipeline-crud-error-paths#task#5.

## Evidence
replay_wire_corpus (trace_set simparity-wire-aws-v1): emulator_up=true, bootstrap_signal=false, steps_total=22, steps_passed=4, steps_mismatched=18, divergence_count=8, score_pct=18.2. All 8 divergence clusters are service=osis; this is the only one carrying NEITHER stub_path NOR owned_by.

osis-pipeline-crud-error-paths#task#5, operation UpdatePipeline, verdict=mismatch, expected_status=404, actual_status=404 (status matches; only the body message diverges).
detail: "$.message: value_mismatch (expected 'Service returned error code ResourceNotFoundException', got 'Pipeline sim2real-probe-plmi-004e3418 could not be found')".
divergence_key: service=osis|UpdatePipeline|status=404->404|keys=$.message
affected: [osis-pipeline-crud-error-paths#task#5] (affected_count 1).

The recorded message 'Service returned error code ResourceNotFoundException' is 100% fixed text. The emulator's message is a template whose single variable part is the requested PipelineName (httpLabel); 'sim2real-probe-plmi-004e3418' is that probe run's generated name and is NOT part of the requirement.

Model grounding (services/service-com-amazonaws-osis-amazonopensearchingestionservice/smithy-model/com-amazonaws-osis-amazonopensearchingestionservice.json, sdkId OSIS, restJson1): UpdatePipeline PUT /2022-01-01/osis/updatePipeline/{PipelineName} -> 200 UpdatePipelineResponse{Pipeline}; errors AccessDeniedException(403), ConflictException(409), DisabledOperationException(409), InternalException(500), ResourceNotFoundException(404), ValidationException(400); input PipelineName @required/@httpLabel @length 3..28 pattern ^[a-z][a-z0-9\-]+$, MinUnits/MaxUnits (PipelineUnits @range min 1), PipelineConfigurationBody @length 1..100000, PipelineRoleArn @length 20..2048.

## Steps to Reproduce
Boot the emulator from this checkout and replay the osis fixtures of trace set simparity-wire-aws-v1 (replay_wire_corpus).

Cause 1 — fixture osis-pipeline-crud-error-paths, step task#5: issue PUT /2022-01-01/osis/updatePipeline/{PipelineName} (UpdatePipeline) for a PipelineName that does not exist in the account/Region (in the fixture, the probe pipeline whose create was rejected earlier in the same fixture), with a valid update body. Expect HTTP 404 ResourceNotFoundException whose `message` is exactly 'Service returned error code ResourceNotFoundException'. The emulator returns 404 with 'Pipeline <PipelineName> could not be found'.

Re-replay fixture osis-pipeline-crud-error-paths step task#5 to confirm the cause is closed.
