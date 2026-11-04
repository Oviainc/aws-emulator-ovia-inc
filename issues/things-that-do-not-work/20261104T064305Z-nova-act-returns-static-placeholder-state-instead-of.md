Issue-Source: trace-sweep-divergence
Issue-Trace: nova-act-act-terminal-state-conflict
Issue-Step: setup#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: nova-act-act-terminal-state-conflict nova-act-session-and-act-workflow nova-act-list-models-compatibility nova-act-workflow-run-lifecycle
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act|CreateAct|2xx->2xx|$.status
Issue-Key: nova-act|ListModels|2xx->2xx|$.modelAliases[*]
Issue-Key: nova-act|GetWorkflowRun|2xx->2xx|$.endedAt
Issue-Key: nova-act|GetWorkflowRun|2xx->2xx|$.status
Issue-Commit: 71ad45cb8263c75348f691f106f1f184e4e6c4d9

# nova-act returns static placeholder state instead of deriving act status, workflow-run completion and model compatibility filtering

> _Note: this issue was identified against the code base at commit `71ad45cb8263c75348f691f106f1f184e4e6c4d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAct

## Description
Three net-new divergences in the nova-act emulator share one root cause: response fields that AWS DERIVES (from the request, from the resource's lifecycle, or from the requested compatibility version) are instead served as a fixed placeholder. 3 causes must be closed.

Cause 1 of 3 - CreateAct reports RUNNING for an act that is waiting on the client (fixture nova-act-act-terminal-state-conflict, step setup#5; same cluster also nova-act-session-and-act-workflow#task#7).
AWS: Smithy CreateAct -> CreateActResponse.status, target ActStatus (enum RUNNING | PENDING_CLIENT_ACTION | PENDING_HUMAN_ACTION | SUCCEEDED | FAILED | TIMED_OUT), required; HTTP PUT /workflow-definitions/{workflowDefinitionName}/workflow-runs/{workflowRunId}/sessions/{sessionId}/acts, success code 201. The model does not state which member of the enum a freshly created act takes, so: recorded behavior only - both recordings show a 201 whose status is PENDING_CLIENT_ACTION, i.e. the act is created in a state that hands control back to the caller rather than one that says the service is already executing. Hypothesis: an act created with CreateActRequest.toolSpecs present cannot proceed until the client runs a tool and reports the result, so the created act is PENDING_CLIENT_ACTION; only an act with no client-side work starts in RUNNING.
Emulator: HTTP status matches (201) but $.status is the constant 'RUNNING'.
Why: a client drives an act by polling/branching on status - PENDING_CLIENT_ACTION is its signal to execute the requested tool and submit the result. Told RUNNING, the client waits for a service-side step that will never happen, so the act never advances and the session stalls or times out. It also makes the terminal-state-conflict path untestable, since the state machine never leaves RUNNING.
Fix: the CreateAct handler's status derivation in the nova-act service crate - compute the created act's ActStatus from the request (presence of client-side work) and the stored act record instead of emitting a literal.

Cause 2 of 3 - ListModels ignores clientCompatibilityVersion when building modelAliases (fixture nova-act-list-models-compatibility, step task#1).
AWS: Smithy ListModels - input ListModelsRequest.clientCompatibilityVersion (@httpQuery "clientCompatibilityVersion", required, Integer), documented as "The client compatibility version to filter models by compatibility"; output requires modelSummaries, modelAliases and compatibilityInformation; ModelSummary.minimumCompatibilityVersion is required, and CompatibilityInformation carries the requested clientCompatibilityVersion plus supportedModelIds. So the response is a FUNCTION of the requested version: entries whose minimum compatibility version exceeds it are not offered. The recording for the requested version lists exactly two aliases and has no third element.
Emulator: returns 200 with an extra alias at $.modelAliases[2] ({"aliasName": "nova-act-v2", "latestModelId": "nova-act-v2.0"}) that the recorded response does not contain - the alias list is a fixed catalogue, unfiltered by the requested version. (The literal alias/model names here are this corpus's catalogue values, not a requirement; the requirement is that the alias list contains only aliases resolvable under the requested compatibility version.)
Why: an older client asks with its own compatibility version precisely so it is only offered models it can speak to. Handed an alias that resolves to a newer model, the client selects it and every subsequent act fails - or silently runs against an incompatible model - and the compatibilityInformation it was given contradicts the alias list in the same response.
Fix: the ListModels handler/serializer in the nova-act service crate - filter modelAliases (and modelSummaries) by the requested clientCompatibilityVersion against each model's minimumCompatibilityVersion, so the alias list and compatibilityInformation.supportedModelIds agree.

Cause 3 of 3 - a workflow run never reaches a terminal status and never gets endedAt (fixture nova-act-workflow-run-lifecycle, step task#7).
AWS: Smithy GetWorkflowRun -> GetWorkflowRunResponse.status, target WorkflowRunStatus (enum RUNNING | SUCCEEDED | FAILED | TIMED_OUT | DELETING), required; endedAt is an OPTIONAL DateTimestamp (@timestampFormat date-time). Optional-plus-terminal-status is the modelled contract: endedAt is absent while the run is RUNNING and present once it has ended. The recording's final GetWorkflowRun of the lifecycle returns 200 with status SUCCEEDED and endedAt set to an RFC3339 date-time.
Emulator: returns 200 but $.status is still 'RUNNING' and $.endedAt is absent after the run has been completed earlier in the same fixture. (The recorded instant is per-run and not a requirement; the requirement is that a run that has ended reports a terminal status and a populated endedAt.)
Why: clients poll GetWorkflowRun until status leaves RUNNING and then read endedAt to compute duration and to decide success vs failure. A run that is permanently RUNNING makes every waiter loop until its own timeout and reports no outcome, so orchestration built on nova-act runs never completes.
Fix: workflow-run state in the nova-act service crate - record the terminal transition (status plus endedAt) when the run completes/stops, and have the GetWorkflowRun handler read status and endedAt from that stored record rather than reporting a creation-time constant.

Related open issues cover DIFFERENT causes and do not cover any of the above: 20261023T043407Z-nova-act-createworkflowdefinition-skips-precondition-checks-and-returns.md and 20261029T174545Z-nova-act-validationexception-message-text-diverges-from-aws.md own the CreateWorkflowDefinition/ValidationException divergences in this same replay.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): 51 steps, 30 passed, 21 mismatched, score 58.8%.

Cause 1 - CreateAct, expected 201 / actual 201:
  $.status: value_mismatch (expected 'PENDING_CLIENT_ACTION', got 'RUNNING')
  cluster key service=nova-act|CreateAct|status=201->201|keys=$.status
  affected: nova-act-act-terminal-state-conflict#setup#5, nova-act-session-and-act-workflow#task#7

Cause 2 - ListModels, expected 200 / actual 200:
  $.modelAliases[2]: value_mismatch (expected None, got {'aliasName': 'nova-act-v2', 'latestModelId': 'nova-act-v2.0'})
  cluster key service=nova-act|ListModels|status=200->200|keys=$.modelAliases[2]
  affected: nova-act-list-models-compatibility#task#1

Cause 3 - GetWorkflowRun, expected 200 / actual 200:
  $.status: value_mismatch (expected 'SUCCEEDED', got 'RUNNING')
  $.endedAt: missing (expected an RFC3339 date-time, got None)
  cluster key service=nova-act|GetWorkflowRun|status=200->200|keys=$.endedAt,$.status
  affected: nova-act-workflow-run-lifecycle#task#7

Model references used: CreateAct (CreateActResponse.status -> ActStatus, required; http PUT .../acts code 201), ListModels (ListModelsRequest.clientCompatibilityVersion @httpQuery required Integer, "filter models by compatibility"; ModelSummary.minimumCompatibilityVersion required; CompatibilityInformation.supportedModelIds required), GetWorkflowRun (status -> WorkflowRunStatus required; endedAt optional DateTimestamp, @timestampFormat date-time).

## Steps to Reproduce
1. git_clone the project and boot the emulator built from the checkout.
2. replay_wire_corpus() against trace set simparity-wire-aws-v1.
3. Cause 1: fixture nova-act-act-terminal-state-conflict, step setup#5 (and nova-act-session-and-act-workflow#task#7) - PUT /workflow-definitions/{name}/workflow-runs/{runId}/sessions/{sessionId}/acts with a task and toolSpecs; the 201 body reports $.status 'RUNNING' where the recording reports 'PENDING_CLIENT_ACTION'.
4. Cause 2: fixture nova-act-list-models-compatibility, step task#1 - POST /models?clientCompatibilityVersion=<recorded version>; the 200 body carries a third entry in $.modelAliases that the recording does not, because the catalogue is not filtered by the requested version.
5. Cause 3: fixture nova-act-workflow-run-lifecycle, step task#7 - run the lifecycle to completion, then GET /workflow-definitions/{name}/workflow-runs/{runId}; the 200 body still reports $.status 'RUNNING' and omits $.endedAt instead of reporting the terminal status with endedAt populated.
6. Re-replay those three fixture steps after the fix; all three must pass.
