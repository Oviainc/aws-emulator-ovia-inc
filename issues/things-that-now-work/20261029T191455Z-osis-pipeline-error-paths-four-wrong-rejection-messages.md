Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: osis.create-pipeline-duplicate-name-rejected osis.create-pipeline-min-greater-than-max-units-rejected osis.start-pipeline-when-already-active-rejected osis.delete-pipeline-removes-resource osis.concurrent-start-pipeline-exactly-one-succeeds
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis.create-pipeline-duplicate-name-rejected|run#13|AssertionError|assert error.get("Message") == expected, f"expected message {expected!r}, got {error.get('Message')!r}"
Issue-Key: osis.create-pipeline-min-greater-than-max-units-rejected|run#17|AssertionError|assert error.get("Message") == expected, f"expected message {expected!r}, got {error.get('Message')!r}"
Issue-Key: osis.start-pipeline-when-already-active-rejected|run#13|AssertionError|assert message.startswith(head) and message.endswith(tail), (
Issue-Key: osis.delete-pipeline-removes-resource|run#21|AssertionError|assert mutation.get("Message") == expected_mutation, (
Issue-Key: osis.concurrent-start-pipeline-exactly-one-succeeds|run#14|AssertionError|assert sorted(results) == ["ok", "ok"], (
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# osis pipeline error paths: four wrong rejection messages plus StartPipeline concurrency semantics

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipeline

## Description
Behavioral replay of the OSIS (Amazon OpenSearch Ingestion) suite found FIVE distinct, independently fixable divergences in the pipeline control plane's rejection paths. This issue bundles all five; each must be closed before submitting. Related open issues: 20261029T174701Z-osis-control-plane-responses-diverge-from-aws-across.md describes the same defect as Cause 2 and Cause 4 below from the WIRE corpus (its Cause 2 and Cause 4) — the two can be closed together, but it does NOT carry these behavioral check ids and covers none of Causes 1, 3 or 5; 20261021T234042Z-osis-validatepipeline-stub.md (ValidatePipeline stub) covers none of them.

--- Cause 1 of 5: CreatePipeline duplicate-name ResourceAlreadyExistsException message is not AWS's wording ---
AWS: Smithy CreatePipeline (POST /2022-01-01/osis/createPipeline) models ResourceAlreadyExistsException with httpError 409 and member `message`; PipelineName is @required, @length 3..28, pattern ^[a-z][a-z0-9\-]+$. Recorded behavior: creating a pipeline whose PipelineName already exists in the account/Region is refused 409 with message `Pipeline with given name "<pipelineName>" already exists`. Fixed text: `Pipeline with given name "` ... `" already exists`, including the double quotes around the name; the single variable part is <pipelineName>, taken verbatim from the request's PipelineName.
Emulator: correct code/status (ResourceAlreadyExistsException) but message `Pipeline with name <pipelineName> already exists` — different wording ("with name" instead of "with given name") and the name is not quoted.
Why: the message is part of the error contract. Clients, test suites and log scrapers that match AWS's string (notably the quoting, which lets a name containing spaces be parsed out) do not classify the error; a caller reconciling an idempotent create loop cannot distinguish this refusal from another 409.
Fix: CreatePipeline's handler/validation duplicate-name path in the osis service crate (services/service-com-amazonaws-osis-amazonopensearchingestionservice/src/ops/create_pipeline.rs) — emit the AWS template with the requested name quoted.
Traces: osis.create-pipeline-duplicate-name-rejected.

--- Cause 2 of 5: CreatePipeline MinUnits>MaxUnits ValidationException message is not AWS's wording ---
AWS: Smithy CreatePipeline requires MinUnits and MaxUnits (PipelineUnits, @range min 1) and models ValidationException, httpError 400, member `message`. Recorded behavior: a request with MinUnits greater than MaxUnits is refused 400 with message `minUnits cannot be greater than maxUnits` — entirely FIXED text with NO variable parts: AWS does not interpolate the submitted values and uses the lower-camel spellings minUnits/maxUnits.
Emulator: 400 with message `MaxUnits (1) must be greater than or equal to MinUnits (5)` — inverted phrasing, capitalized member names, and the submitted unit counts interpolated into text AWS keeps constant (the numbers 1 and 5 are this run's request values, not part of any template).
Why: anything matching AWS's fixed string fails to classify the error, and operators reading the message see wording AWS never emits.
Fix: CreatePipeline's unit-range validation (create_pipeline.rs) — emit ValidationException with exactly the fixed text `minUnits cannot be greater than maxUnits`. Same defect as Cause 2 of 20261029T174701Z-osis-control-plane-responses-diverge-from-aws-across.md.
Traces: osis.create-pipeline-min-greater-than-max-units-rejected.

--- Cause 3 of 5: StartPipeline on an ACTIVE pipeline returns the wrong ConflictException message (no eligible-status list) ---
AWS: Smithy StartPipeline (PUT /2022-01-01/osis/startPipeline/{PipelineName}) models ConflictException, httpError 409, member `message`. Recorded behavior: starting a pipeline that is already ACTIVE is refused 409 with message `Only pipelines with one of the following statuses are eligible for starting: <statuses>. The current status is <status>.` Fixed text: the head `Only pipelines with one of the following statuses are eligible for starting: ` and the tail `. The current status is ` + <status> + `.`; the variable parts are <statuses> — the eligible-status set STOPPED, START_FAILED, CREATE_FAILED rendered comma-separated, whose ORDER varies between AWS responses and is therefore compared as a set — and <status>, the pipeline's current status from service state (ACTIVE here).
Emulator: ConflictException with the correct 409, but message `Pipeline <pipelineName> cannot be started while its status is ACTIVE` — a template of its own that echoes the pipeline name (which AWS does not include) and omits the eligible-status list entirely.
Why: the eligible-status list is the actionable content of this error — a client decides whether to stop-then-start, wait, or give up based on which statuses permit a start. Against the emulator that list is absent, and matchers keyed to AWS's wording do not fire.
Fix: StartPipeline's status-eligibility check (services/service-com-amazonaws-osis-amazonopensearchingestionservice/src/ops/start_pipeline.rs) — build the ConflictException message from the eligible-status set and the pipeline's current status, with AWS's fixed text and no pipeline name.
Traces: osis.start-pipeline-when-already-active-rejected.

--- Cause 4 of 5: StartPipeline on a pipeline that does not exist returns the wrong ResourceNotFoundException message ---
AWS: Smithy StartPipeline models ResourceNotFoundException, httpError 404, member `message` (shape doc: "You attempted to access or delete a resource that does not exist."). Recorded behavior: after DeletePipeline removes the pipeline, StartPipeline for that name is refused 404 with message `No pipeline settings found` — entirely FIXED text; AWS deliberately does not echo the requested PipelineName for this operation.
Emulator: 404 with message `Pipeline <pipelineName> could not be found`, i.e. a generic lookup-miss template that interpolates the requested name.
Why: wrong text for a modeled error; client-side matchers and log scrapers keyed to AWS's wording miss it, and the emulator reveals a name AWS does not. It also shows the emulator funnels every pipeline lookup miss through one shared message, while AWS words it per operation.
Fix: StartPipeline's not-found path (start_pipeline.rs) — return ResourceNotFoundException with exactly `No pipeline settings found`. Same defect as Cause 4 of 20261029T174701Z-osis-control-plane-responses-diverge-from-aws-across.md, observed here via the post-delete leg of the delete-pipeline check.
Traces: osis.delete-pipeline-removes-resource.

--- Cause 5 of 5: concurrent duplicate StartPipeline on a STOPPED pipeline is refused with ConflictException instead of being accepted idempotently ---
AWS: Smithy StartPipeline models both ConflictException (409) and a 200 output carrying Pipeline. Recorded behavior (2/2 replays against real AWS): two StartPipeline calls fired concurrently at the same STOPPED pipeline BOTH observe 200 — AWS accepts the duplicate request idempotently, and the single pipeline resource still performs exactly one STOPPED -> STARTING -> ACTIVE transition. ConflictException is reserved for statuses that are not eligible for starting (Cause 3), not for a second start that races the first.
Emulator: one racer gets 200 and the other is refused with code `ConflictException` (results observed: ['ok', 'ConflictException']) — the second call is treated as a conflicting state transition because the pipeline has already left STOPPED.
Why: a client that retries StartPipeline (SDK retry, two workers, an IaC apply running twice) gets a hard 409 where AWS returns success, so correct, idempotent caller code fails against the emulator and surfaces a spurious conflict for a transition that did happen.
Fix: StartPipeline's handler and the pipeline state transition it guards (start_pipeline.rs plus the osis pipeline state store) — a start request for a pipeline that is already starting toward ACTIVE as a result of an accepted start must be accepted idempotently (200 with the pipeline) rather than classified as a conflict, while still rejecting genuinely ineligible statuses per Cause 3.
Traces: osis.concurrent-start-pipeline-exactly-one-succeeds.

## Evidence
run_behavioral_suite (suite simparity-behavioral-aws-v1): total 4/21 passed (19.0%), 9 mismatched, 8 errored, bootstrap_signal false — the emulator is up and answering. All failures are service osis. The five checks below are verdict=mismatch and are marked neither "ALREADY FILED" nor "OWNED by".

1) [mismatch] osis.create-pipeline-duplicate-name-rejected — AssertionError: expected message 'Pipeline with given name "sim2real-osis-5f693c211486" already exists', got 'Pipeline with name sim2real-osis-5f693c211486 already exists'. (The name sim2real-osis-5f693c211486 is this run's generated pipeline name and fills the template; it is not part of the requirement.) The preceding assertion on Code == ResourceAlreadyExistsException passed.

2) [mismatch] osis.create-pipeline-min-greater-than-max-units-rejected — AssertionError: expected message 'minUnits cannot be greater than maxUnits', got 'MaxUnits (1) must be greater than or equal to MinUnits (5)'.

3) [mismatch] osis.start-pipeline-when-already-active-rejected — AssertionError: expected 'Only pipelines with one of the following statuses are eligible for starting: '<statuses>'. The current status is ACTIVE.', got 'Pipeline sim2real-osis-e22434b90b6e cannot be started while its status is ACTIVE'. The preceding assertion on Code == ConflictException passed; the check compares the status list as a SET ({STOPPED, START_FAILED, CREATE_FAILED}) and the rest of the message exactly, because AWS varies the list order between calls.

4) [mismatch] osis.delete-pipeline-removes-resource — AssertionError: expected StartPipeline message 'No pipeline settings found', got 'Pipeline sim2real-osis-bb2e93b4074b could not be found' (after a DeletePipeline that the emulator did complete).

5) [mismatch] osis.concurrent-start-pipeline-exactly-one-succeeds — AssertionError: expected OSIS to accept a concurrent duplicate StartPipeline idempotently on both racers, got results=['ok', 'ConflictException'].

Model grounding (services/service-com-amazonaws-osis-amazonopensearchingestionservice/smithy-model/com-amazonaws-osis-amazonopensearchingestionservice.json, sdkId OSIS, restJson1): CreatePipeline requires PipelineName (@length 3..28, pattern ^[a-z][a-z0-9\-]+$), MinUnits/MaxUnits (@range min 1) and PipelineConfigurationBody, and models ValidationException(400), ResourceAlreadyExistsException(409), LimitExceededException(409), ResourceNotFoundException(404); StartPipeline (PUT /2022-01-01/osis/startPipeline/{PipelineName}) models ConflictException(409), ResourceNotFoundException(404), ValidationException(400), DisabledOperationException(409), AccessDeniedException(403) and returns 200 with Pipeline.

## Steps to Reproduce
Boot the emulator from this checkout and replay the osis behavioral checks of suite simparity-behavioral-aws-v1 (run_behavioral_suite).

Cause 1 — osis.create-pipeline-duplicate-name-rejected: CreatePipeline a pipeline, then CreatePipeline again with the SAME PipelineName and a different PipelineConfigurationBody. Expect 409 ResourceAlreadyExistsException with message 'Pipeline with given name "<PipelineName>" already exists'.

Cause 2 — osis.create-pipeline-min-greater-than-max-units-rejected: CreatePipeline with MinUnits=5, MaxUnits=1. Expect 400 ValidationException with the fixed message 'minUnits cannot be greater than maxUnits'.

Cause 3 — osis.start-pipeline-when-already-active-rejected: CreatePipeline, wait for ACTIVE, then StartPipeline. Expect 409 ConflictException with message 'Only pipelines with one of the following statuses are eligible for starting: <STOPPED, START_FAILED, CREATE_FAILED in any order>. The current status is ACTIVE.' and the pipeline still ACTIVE afterwards.

Cause 4 — osis.delete-pipeline-removes-resource: CreatePipeline, wait for ACTIVE, DeletePipeline, wait until it is gone, then StartPipeline for that name. Expect 404 ResourceNotFoundException with the fixed message 'No pipeline settings found'.

Cause 5 — osis.concurrent-start-pipeline-exactly-one-succeeds: CreatePipeline, wait for ACTIVE, StopPipeline, wait for STOPPED, then fire two StartPipeline calls for that pipeline concurrently from two threads. Expect BOTH to return 200 and the pipeline to reach ACTIVE through exactly one transition.

Re-replay checks osis.create-pipeline-duplicate-name-rejected, osis.create-pipeline-min-greater-than-max-units-rejected, osis.start-pipeline-when-already-active-rejected, osis.delete-pipeline-removes-resource and osis.concurrent-start-pipeline-exactly-one-succeeds to confirm all five causes are closed.
