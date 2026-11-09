Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-automation-execution-not-found-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-automation-execution-not-found-errors ssm-automation-nonexistent-execution-errors ssm-automation-stop-and-signal-error-paths ssm-automation-filter-steps-and-complete ssm-automation-lifecycle-describe-steps-and-stop
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|DescribeAutomationStepExecutions|4xx->4xx|$.Message
Issue-Key: ssm|DescribeAutomationStepExecutions|2xx->2xx|$.StepExecutions[*]
Issue-Key: ssm|DescribeAutomationStepExecutions|2xx->2xx|$.StepExecutions[*].Inputs.Duration
Issue-Key: ssm|DescribeAutomationStepExecutions|2xx->2xx|$.StepExecutions[*].IsStepBlocked
Issue-Key: ssm|DescribeAutomationStepExecutions|2xx->2xx|$.StepExecutions[*].Outputs
Issue-Key: ssm|DescribeAutomationStepExecutions|2xx->2xx|$.StepExecutions[*].OverriddenParameters
Issue-Key: ssm|DescribeAutomationStepExecutions|2xx->2xx|$.StepExecutions[*].StepStatus
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# ssm DescribeAutomationStepExecutions diverges on not-found wording, step projection and cancelled step status

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutomationStepExecutions

## Description
Three net-new root causes on SSM's DescribeAutomationStepExecutions, all in the `ssm` service: its not-found refusal is emulator-authored prose, its StepExecution projection omits members AWS always returns while adding members AWS omits and renders resolved step inputs unquoted, and a cancelled execution's step is still reported as Success. A fixer must close ALL THREE before submitting; each has its own fixture#step binding, listed under its Traces line and carried in this issue's replay pointer.

Cause 1 of 3: the unknown-execution refusal uses emulator prose that interpolates the requested id, where AWS emits one fixed sentence.
AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json) models DescribeAutomationStepExecutions with required `AutomationExecutionId` (@length 36..36) and the client error AutomationExecutionNotFoundException (httpError 400, awsQueryError code AutomationExecutionNotFound, single member `Message`; doc: "There is no automation execution information for the requested automation execution ID."). The model does not state the message text, so: recorded behavior only — for a well-formed 36-character execution id that was never issued, AWS answers HTTP 400 with `Message` = `The input automation execution does not exist.` The whole sentence is FIXED text with NO variable part: AWS does not echo the requested AutomationExecutionId into it. This is the same sentence AWS uses for the equivalent refusal on the other execution-scoped Automation reads.
Emulator: HTTP 400 with the right error class but `Message` = `Automation execution <automationExecutionId> does not exist.` — emulator-authored wording that additionally splices in the requested id (the id in the recording is a per-run value, not part of any template).
Why: clients, the AWS CLI and every SDK surface Message verbatim. A runbook wrapper or log scraper that matches AWS's fixed sentence — the only text AWS ever emits for this class — never matches, and a caller cannot tell this refusal apart from a document-store or state-manager not-found, which carry different wording.
Fix: the DescribeAutomationStepExecutions handler's execution-resolution refusal path, and the shared Automation not-found helper it should call so every execution-scoped Automation operation emits the identical sentence (services/service-com-amazonaws-ssm-amazonssm/src/ops/, the describe_automation_step_executions handler plus the shared automation not-found error constructor). NOTE: the open issue 20261105T135904Z-ssm-automation-execution-apis-stop-is-never-applied.md files this same sentence defect for GetAutomationExecution (its Cause 2) and recommends exactly such a shared helper, but it carries no replay key for this fixture and does not cover DescribeAutomationStepExecutions, so this step must be re-replayed here.
Traces: ssm-automation-execution-not-found-errors#task#1 (representative); same cause on ssm-automation-nonexistent-execution-errors#task#1 and ssm-automation-stop-and-signal-error-paths#task#8.

Cause 2 of 3: the StepExecution projection drops members AWS always returns, adds members AWS omits, and renders resolved step inputs as bare strings instead of JSON literals.
AWS: Smithy: output `StepExecutions` -> StepExecutionList of StepExecution, whose members include StepName, Action (@pattern ^aws:[a-zA-Z]{3,25}$), OnFailure, MaxAttempts, ExecutionStartTime, ExecutionEndTime, StepStatus (AutomationExecutionStatus enum), `Inputs` (NormalStringMap, doc "Fully-resolved values passed into the step before execution"), `Outputs` (AutomationParameterMap), `OverriddenParameters` (AutomationParameterMap), `ValidNextSteps` (ValidNextStepList), IsCritical, StepExecutionId. For a completed step the recording shows AWS always carrying the collection members even when empty — `Outputs: {}`, `OverriddenParameters: {}` — plus `IsStepBlocked: false` (that member is NOT in the on-disk model: recorded behavior only) and `ValidNextSteps` listing the step(s) that follow. A resolved string value in `Inputs` is rendered as its JSON literal, i.e. with surrounding double quotes (the recording shows a duration input as `"PT30S"`, quotes included, not bare PT30S). The recording also shows AWS NOT emitting `MaxAttempts` or `IsCritical` for this step, so those members are absent from the AWS payload for a step that did not override them.
Emulator: the step object diverges structurally — `Outputs`, `OverriddenParameters`, `IsStepBlocked` and `ValidNextSteps` are all omitted rather than emitted empty/false/populated; `Inputs` values are rendered bare (`PT30S`) instead of as JSON string literals (`"PT30S"`); and `MaxAttempts` and `IsCritical` are emitted where the AWS payload has neither. Status is 200 -> 200: only the body shape diverges.
Why: SDKs and consoles index these members directly. A client that reads step `Outputs`/`OverriddenParameters` hits a missing key on every Automation step read (not just cancelled ones), a step-graph viewer that walks `ValidNextSteps` sees no successors, a blocked-step check on `IsStepBlocked` reads null instead of false, and anything that JSON-decodes a resolved input value fails on the unquoted form — the value's type is lost. The extra members make the emulator's payload a superset no AWS response ever contains, so strict/round-trip clients see unexpected fields.
Fix: the DescribeAutomationStepExecutions output serializer for StepExecution (services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_automation_step_executions.rs and the shared Automation step projection it uses, which GetAutomationExecution also renders) — always project Outputs/OverriddenParameters as empty maps, IsStepBlocked as false, ValidNextSteps from the runbook's step graph, JSON-encode resolved Inputs values, and stop emitting MaxAttempts/IsCritical where AWS does not.
Traces: ssm-automation-filter-steps-and-complete#task#4.

Cause 3 of 3: after StopAutomationExecution the step is still reported Success — DescribeAutomationStepExecutions never reflects the cancellation.
AWS: StopAutomationExecution terminates a running execution; a subsequent DescribeAutomationStepExecutions reports the in-flight step with `StepStatus` = `Cancelled` (AutomationExecutionStatus enum) rather than a successful terminal status, and the same projection rules as Cause 2 apply to that step (Outputs {}, OverriddenParameters {}, IsStepBlocked false, Inputs rendered as JSON literals). Smithy: StepExecution.StepStatus -> AutomationExecutionStatus; StopAutomationExecution has an empty output and errors AutomationExecutionNotFoundException/InvalidAutomationStatusUpdateException/InternalServerError.
Emulator: the stop is accepted, but the step is reported with `StepStatus` = `Success` — the long-running step is allowed to run to completion and the cancellation is never recorded on the step — together with the same projection gaps as Cause 2 on this read path.
Why: the standard cancel-then-inspect pattern reports the opposite outcome: an operator or pipeline that stops a runbook and then reads the step list is told the step SUCCEEDED, so a cancelled change is accounted for as an applied one. That is a wrong answer, not just a missing one.
Fix: StopAutomationExecution must transition the stored execution and its in-flight step to Cancelled in the Automation execution state, and the step read path must serve that stored status (services/service-com-amazonaws-ssm-amazonssm/src/ops/stop_automation_execution.rs plus the automation state module and the step projection named in Cause 2). Related but distinct: 20261105T135904Z-ssm-automation-execution-apis-stop-is-never-applied.md files the stop-never-applied defect as seen through GetAutomationExecution and carries that fixture's key; it does not carry this fixture's key and says nothing about the DescribeAutomationStepExecutions view, which is what this cause adds.
Traces: ssm-automation-lifecycle-describe-steps-and-stop#task#7 (representative); same cause at ssm-automation-lifecycle-describe-steps-and-stop#task#8.

## Evidence
Cause 1 — fixture ssm-automation-execution-not-found-errors, step task#1, DescribeAutomationStepExecutions [mismatch, status 400 -> 400]:
  expected $.Message: 'The input automation execution does not exist.'
  actual   $.Message: 'Automation execution f3402900-1cb2-4c10-9fef-1158a5f18656 does not exist.'
  The expected sentence is fixed text end to end; the emulator's variant interpolates the requested AutomationExecutionId (the uuid shown is that run's value, not part of the template). Status and error class match.
  Cluster (affected_count 3): ssm-automation-execution-not-found-errors#task#1, ssm-automation-nonexistent-execution-errors#task#1, ssm-automation-stop-and-signal-error-paths#task#8.

Cause 2 — fixture ssm-automation-filter-steps-and-complete, step task#4, DescribeAutomationStepExecutions [mismatch, status 200 -> 200]:
  expected $.StepExecutions[0] to carry: Outputs {}, OverriddenParameters {}, IsStepBlocked false, ValidNextSteps [<next step name>], Inputs.Duration as the JSON literal '"PT30S"', plus StepName/Action/OnFailure/ExecutionStartTime/ExecutionEndTime/StepStatus/StepExecutionId; MaxAttempts and IsCritical absent.
  actual   $.StepExecutions[0] carried: StepName, Action, OnFailure, MaxAttempts 1, ExecutionStartTime, ExecutionEndTime, StepStatus, Inputs.Duration 'PT30S' (unquoted), StepExecutionId, IsCritical true — with Outputs, OverriddenParameters, IsStepBlocked and ValidNextSteps all missing.
  (the timestamps and the StepExecutionId are per-run values and are not the requirement; the member set, the empty-collection defaults, IsStepBlocked false, ValidNextSteps and the JSON-literal rendering of resolved Inputs are.)

Cause 3 — fixture ssm-automation-lifecycle-describe-steps-and-stop, step task#7, DescribeAutomationStepExecutions after StopAutomationExecution [mismatch, status 200 -> 200]:
  $.StepExecutions[0].StepStatus: expected 'Cancelled', got 'Success'
  $.StepExecutions[0].Inputs.Duration: expected '"PT30S"' (JSON string literal), got 'PT30S'
  $.StepExecutions[0].IsStepBlocked: expected False, got missing
  $.StepExecutions[0].Outputs: expected {}, got missing
  $.StepExecutions[0].OverriddenParameters: expected {}, got missing
  Same divergence repeats at step task#8 of that fixture (affected_count 2).

Model citations (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json): DescribeAutomationStepExecutions input AutomationExecutionId(required, @length 36..36) + Filters(StepExecutionFilterList @length 1..6) + NextToken + MaxResults(1..50) + ReverseOrder, output StepExecutions(StepExecutionList)+NextToken, paginated{inputToken NextToken, outputToken NextToken, items StepExecutions, pageSize MaxResults}, errors AutomationExecutionNotFoundException(400, code AutomationExecutionNotFound, member Message)/InvalidFilterKey/InvalidFilterValue/InvalidNextToken/InternalServerError; StepExecution members StepName/Action/TimeoutSeconds/OnFailure/MaxAttempts/ExecutionStartTime/ExecutionEndTime/StepStatus(AutomationExecutionStatus)/ResponseCode/Inputs(NormalStringMap: map<String,String>)/Outputs(AutomationParameterMap)/Response/FailureMessage/WarningMessage/FailureDetails/StepExecutionId/OverriddenParameters(AutomationParameterMap)/IsEnd/NextStep/IsCritical/ValidNextSteps(ValidNextStepList)/Targets/TargetLocation/TriggeredAlarms/ParentStepDetails — there is no IsStepBlocked member in the model, so that member is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1 — fixture ssm-automation-execution-not-found-errors, step task#1: call ssm.DescribeAutomationStepExecutions(AutomationExecutionId="<any well-formed 36-character uuid never issued>"). Observe HTTP 400 with Message 'Automation execution <id> does not exist.' instead of AWS's fixed 'The input automation execution does not exist.'
3. Cause 2 — fixture ssm-automation-filter-steps-and-complete, step task#4: CreateDocument an Automation runbook with two steps (e.g. an aws:sleep step with Duration PT30S followed by a second step), StartAutomationExecution it, let it finish, then call DescribeAutomationStepExecutions on the execution id. Inspect StepExecutions[0]: Outputs, OverriddenParameters, IsStepBlocked and ValidNextSteps are absent, Inputs.Duration is the bare string PT30S rather than the JSON literal "PT30S", and MaxAttempts/IsCritical are present where AWS emits neither.
4. Cause 3 — fixture ssm-automation-lifecycle-describe-steps-and-stop, step task#7: start the same kind of long-running Automation execution, call StopAutomationExecution on it, then call DescribeAutomationStepExecutions. Observe StepExecutions[0].StepStatus 'Success' instead of 'Cancelled', with the same projection gaps as step 3. Repeat the read (task#8) — the status never becomes Cancelled.
