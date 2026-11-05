Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-automation-execution-with-governance-checks
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-automation-execution-with-governance-checks ssm-automation-signal-stopstep-and-invalid-filter-error ssm-automation-stop-and-signal-error-paths
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|StartAutomationExecution|4xx->4xx|$.Message
Issue-Key: ssm|GetAutomationExecution|4xx->4xx|$.Message
Issue-Key: ssm|DescribeAutomationStepExecutions|4xx->4xx|$.message
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.AutomationExecutionStatus
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.ExecutionEndTime
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.Outputs
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.Parameters
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.ResolvedTargets
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.StepExecutions[*].ExecutionEndTime
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.StepExecutions[*].Inputs.Duration
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.StepExecutions[*].IsStepBlocked
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.StepExecutions[*].Outputs
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.StepExecutions[*].OverriddenParameters
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.StepExecutions[*].StepStatus
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.Targets
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# ssm Automation execution APIs: stop is never applied and every refusal is emulator-authored (4 causes)

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomationExecution, GetAutomationExecution, DescribeAutomationStepExecutions, StopAutomationExecution

## Description
Four net-new root causes in the SSM Automation surface, all in the `ssm` service: three refusals/validation messages the emulator authors itself instead of using AWS's wording, and one state defect where StopAutomationExecution is accepted but never applied to the stored execution. A fixer must close ALL FOUR before submitting; each has its own fixture#step binding, listed under its Traces line and carried in this issue's replay pointer.

Cause 1 of 4: StartAutomationExecution against a runbook name that does not exist refuses with emulator prose instead of AWS's fixed sentence.
AWS: StartAutomationExecution models AutomationDefinitionNotFoundException (Smithy com-amazonaws-ssm-amazonssm.json: StartAutomationExecution errors = AutomationDefinitionNotFoundException / AutomationDefinitionVersionNotFoundException / AutomationExecutionLimitExceededException / IdempotentParameterMismatch / InvalidAutomationExecutionParametersException / InvalidTarget / InternalServerError; AutomationDefinitionNotFoundException doc: "An Automation runbook with the specified name couldn't be found.", httpError 400, single member Message). For an unresolvable DocumentName the recorded response is HTTP 400 with Message "Invalid input for field document name, expected valid document in account." — that whole sentence is FIXED text with NO variable parts: AWS does not echo the requested document name into it.
Emulator: HTTP 400 with the right error class but Message "Document <documentName> does not exist." — emulator-authored wording that additionally interpolates the requested name.
Why: clients and the AWS CLI/SDK surface the Message verbatim; log scrapers, runbook wrappers and support tooling that match AWS's fixed sentence (the only text AWS ever emits for this class) never match, and a caller cannot tell this refusal apart from a state-manager/document-store not-found, which carries different wording.
Fix: the StartAutomationExecution handler's document-resolution refusal path (services/service-com-amazonaws-ssm-amazonssm/src/ops/start_automation_execution.rs) must raise AutomationDefinitionNotFoundException with AWS's fixed message rather than formatting its own.
Traces: ssm-automation-execution-with-governance-checks#task#4.

Cause 2 of 4: GetAutomationExecution on an unknown execution id refuses with emulator prose instead of AWS's fixed sentence.
AWS: GetAutomationExecution models AutomationExecutionNotFoundException (Smithy: GetAutomationExecution errors = AutomationExecutionNotFoundException(400) / InternalServerError(500); exception doc: "There is no automation execution information for the requested automation execution ID.", single member Message). For a well-formed but unknown AutomationExecutionId (36 chars, as the modelled length requires) the recorded response is HTTP 400 with Message "The input automation execution does not exist." — again entirely FIXED text with no variable part; the id is NOT echoed. This is the same wording AWS uses for the equivalent refusal on the other execution-scoped Automation operations.
Emulator: HTTP 400 with Message "Automation execution <automationExecutionId> does not exist." — emulator-authored sentence that interpolates the requested id.
Why: the same message-matching breakage as Cause 1, on the read path every Automation poller uses; a caller that distinguishes "never existed" from other failures by AWS's sentence gets an unrecognised string.
Fix: the GetAutomationExecution handler's not-found path (services/service-com-amazonaws-ssm-amazonssm/src/ops/get_automation_execution.rs) — and the shared Automation not-found helper it should use, so SendAutomationSignal/StopAutomationExecution/DescribeAutomationStepExecutions emit the identical sentence.
Traces: ssm-automation-execution-with-governance-checks#task#5.

Cause 3 of 4: DescribeAutomationStepExecutions' enum ValidationException echoes the rejected value and renders the allowed-value set in its own order.
AWS: Filters[].Key targets StepExecutionFilterKey (Smithy: enum values StartTimeBefore, StartTimeAfter, StepExecutionStatus, StepExecutionId, StepName, Action, ParentStepExecutionId, ParentStepIteration, ParentStepIteratorValue; DescribeAutomationStepExecutions models InvalidFilterKey/InvalidFilterValue besides the framework ValidationException). For an off-enum filter key the recorded response is HTTP 400 ValidationException whose message is "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [<allowed values>]": the FIXED text is "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [" and "]", the variable parts are the member path taken from the request (here filters.1.member.key) and the enum's value list. AWS does NOT quote the offending value in this constraint message, and renders the set in the order the recording shows (ParentStepIteration, StepExecutionId, ParentStepIteratorValue, StartTimeAfter, StartTimeBefore, Action, StepName, ParentStepExecutionId, StepExecutionStatus) rather than in model-declaration order — recorded behavior only for the ordering, so the fix must reproduce the recorded rendering.
Emulator: HTTP 400 ValidationException with "1 validation error detected: Value '<rejectedValue>' at 'filters.1.member.key' failed to satisfy constraint: Member must satisfy enum value set: [...]" — it inserts "Value '<rejectedValue>' at" where AWS writes "Value at", and lists the set in model-declaration order.
Why: a client that parses the standard AWS validation sentence (the same shape every AWS service emits) to recover which member it got wrong reads a differently-shaped string, and the leaked value turns a constraint report into an echo of possibly sensitive input.
Fix: the shared enum-constraint message formatter used by the ssm request validators (the validation layer reached from the DescribeAutomationStepExecutions handler, not the handler body). NOTE: the open issue "SSM enum ValidationException reports stale value sets and echoes the offending value" (20261103T173026Z-ssm-enum-validationexception-reports-stale-value-sets-and.md) describes the same family of fault on other operations but does not carry this fixture's replay key, so this step must be re-replayed here. The open scaffold-stub issue 20261105T123903Z-ssm-describeautomationstepexecutions-stub.md only asks for the operation to exist; the message shape above is what this cause adds.
Traces: ssm-automation-signal-stopstep-and-invalid-filter-error#task#5.

Cause 4 of 4: StopAutomationExecution is accepted but never applied — the execution stays InProgress forever, and GetAutomationExecution's projection omits the empty collection members AWS always returns and renders step Inputs unquoted.
AWS: StopAutomationExecution (Smithy: input AutomationExecutionId required + Type (StopType), empty output, errors AutomationExecutionNotFoundException / InvalidAutomationStatusUpdateException / InternalServerError) terminates a running execution: a subsequent GetAutomationExecution reports AutomationExecution.AutomationExecutionStatus = "Cancelled", sets ExecutionEndTime, and marks the in-flight step StepExecutions[0].StepStatus = "Cancelled" with its own ExecutionEndTime. The same payload always carries the execution's collection members even when empty — Parameters {}, Outputs {}, Targets [], ResolvedTargets {"ParameterValues": [], "Truncated": false} — and per step Outputs {}, OverriddenParameters {} and IsStepBlocked false (Smithy: AutomationExecution members Parameters/Outputs/Targets/ResolvedTargets/StepExecutions; StepExecution members Outputs/OverriddenParameters/ExecutionEndTime/StepStatus — IsStepBlocked is recorded behavior only, it is not in the on-disk model). Step Inputs is a NormalStringMap of fully-resolved values, and a resolved string value is rendered as its JSON literal, i.e. with surrounding double quotes (recorded: Duration is "\"PT5M\"", not PT5M).
Emulator: the stop call is accepted, but polling GetAutomationExecution still reports AutomationExecutionStatus "InProgress" after 30 attempts (and StepExecutions[0].StepStatus "InProgress"), with ExecutionEndTime and the step's ExecutionEndTime absent; Parameters, Outputs, Targets, ResolvedTargets, the step's Outputs/OverriddenParameters and IsStepBlocked are all omitted instead of emitted empty/false, and step Inputs renders the resolved string bare (PT5M) instead of as a JSON string literal.
Why: any caller that stops a runbook and waits for a terminal status waits forever — the standard cancel-then-wait pattern never completes and no cancellation is observable — while clients that read the always-present collection members (SDKs that index Outputs/ResolvedTargets, and consoles that show resolved step inputs) hit missing keys and mis-typed values on every Automation read, not just cancelled ones.
Fix: the StopAutomationExecution handler must transition the stored execution and its in-flight step to Cancelled with end timestamps in the Automation execution state (services/service-com-amazonaws-ssm-amazonssm/src/ops/stop_automation_execution.rs plus the automation state module it writes), and the GetAutomationExecution serializer must always project the empty collection members and the per-step defaults and JSON-encode resolved Inputs values. Related but distinct: the open behavioral issue 20261105T123903Z-ssm-automation-aws-executeawsapi-and-aws-executescript-steps.md covers step actions never running and SendAutomationSignal returning 501; it says nothing about stop/cancel being dropped or about this projection, and carries no wire replay key for this fixture.
Traces: ssm-automation-stop-and-signal-error-paths#task#6.

## Evidence
Cause 1 — fixture ssm-automation-execution-with-governance-checks, step task#4, StartAutomationExecution [mismatch, status 400 -> 400]:
  expected $.Message: 'Invalid input for field document name, expected valid document in account.'
  actual   $.Message: 'Document sim2real-probe-ssm-absent-runbook-708c0c40 does not exist.'
  (the recorded sentence is fixed text; the emulator's variant interpolates the requested DocumentName)

Cause 2 — fixture ssm-automation-execution-with-governance-checks, step task#5, GetAutomationExecution [mismatch, status 400 -> 400]:
  expected $.Message: 'The input automation execution does not exist.'
  actual   $.Message: 'Automation execution 99999999-8888-7777-6666-555555555555 does not exist.'
  (fixed text with no variable part; the emulator interpolates the requested AutomationExecutionId)

Cause 3 — fixture ssm-automation-signal-stopstep-and-invalid-filter-error, step task#5, DescribeAutomationStepExecutions [mismatch, status 400 -> 400]:
  expected $.message: "1 validation error detected: Value at 'filters.1.member.key' failed to satisfy constraint: Member must satisfy enum value set: [ParentStepIteration, StepExecutionId, ParentStepIteratorValue, StartTimeAfter, StartTimeBefore, Action, StepName, ParentStepExecutionId, StepExecutionStatus]"
  actual   $.message: "1 validation error detected: Value 'NotARealFilterKey' at 'filters.1.member.key' failed to satisfy constraint: Member must satisfy enum value set: [StartTimeBefore, StartTimeAfter, StepExecutionStatus, StepExecutionId, StepName, Action, ParentStepExecutionId, ParentStepIteration, ParentStepIteratorValue]"
  (differences: the emulator adds "Value '<rejected value>' at" where AWS writes "Value at", and renders the enum set in model-declaration order instead of the recorded order; the member path filters.1.member.key is the request-derived variable part and matches)

Cause 4 — fixture ssm-automation-stop-and-signal-error-paths, step task#6, GetAutomationExecution after StopAutomationExecution [mismatch, status 200 -> 200]:
  $.AutomationExecution.AutomationExecutionStatus: expected 'Cancelled', got 'InProgress' (poll: not in ['Cancelled'] after 30 attempts)
  $.AutomationExecution.ExecutionEndTime: expected set, got missing
  $.AutomationExecution.Outputs: expected {}, got missing
  $.AutomationExecution.Parameters: expected {}, got missing
  $.AutomationExecution.Targets: expected [], got missing
  $.AutomationExecution.ResolvedTargets: expected {'ParameterValues': [], 'Truncated': False}, got missing
  $.AutomationExecution.StepExecutions[0].StepStatus: expected 'Cancelled', got 'InProgress'
  $.AutomationExecution.StepExecutions[0].ExecutionEndTime: expected set, got missing
  $.AutomationExecution.StepExecutions[0].Outputs: expected {}, got missing
  $.AutomationExecution.StepExecutions[0].OverriddenParameters: expected {}, got missing
  $.AutomationExecution.StepExecutions[0].IsStepBlocked: expected False, got missing
  $.AutomationExecution.StepExecutions[0].Inputs.Duration: expected '"PT5M"' (JSON string literal), got 'PT5M'
  (the recorded timestamps themselves are per-run values and are not the requirement; their presence, the Cancelled statuses, the always-present empty collections and the JSON-literal rendering are)

Model citations (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json): StartAutomationExecution -> AutomationDefinitionNotFoundException(400, member Message); GetAutomationExecution -> AutomationExecutionNotFoundException(400, member Message), output AutomationExecution; DescribeAutomationStepExecutions -> Filters: StepExecutionFilterList, key enum StepExecutionFilterKey (9 values), errors InvalidFilterKey/InvalidFilterValue/InvalidNextToken/AutomationExecutionNotFoundException; StopAutomationExecution -> input AutomationExecutionId(required, len 36..36) + Type, empty output, errors AutomationExecutionNotFoundException/InvalidAutomationStatusUpdateException/InternalServerError; AutomationExecution members Parameters/Outputs/Targets/ResolvedTargets/StepExecutions/ExecutionEndTime; StepExecution members Inputs(NormalStringMap)/Outputs/OverriddenParameters/StepStatus/ExecutionEndTime (no IsStepBlocked member — recorded behavior only).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1 — fixture ssm-automation-execution-with-governance-checks, step task#4: call StartAutomationExecution with DocumentName set to a runbook that was never created. Observe HTTP 400 with Message 'Document <name> does not exist.' instead of AWS's fixed 'Invalid input for field document name, expected valid document in account.'
3. Cause 2 — same fixture, step task#5: call GetAutomationExecution with a well-formed 36-character AutomationExecutionId that was never issued. Observe HTTP 400 with Message 'Automation execution <id> does not exist.' instead of AWS's fixed 'The input automation execution does not exist.'
4. Cause 3 — fixture ssm-automation-signal-stopstep-and-invalid-filter-error, step task#5: call DescribeAutomationStepExecutions with a real AutomationExecutionId and Filters=[{Key: 'NotARealFilterKey', Values: [...]}]. Observe the ValidationException message quoting the rejected value ('Value \'NotARealFilterKey\' at ...') and listing the enum set in model order, instead of AWS's 'Value at \'filters.1.member.key\' ...' with the recorded set order.
5. Cause 4 — fixture ssm-automation-stop-and-signal-error-paths, step task#6: CreateDocument an Automation runbook with a long-running step (e.g. aws:sleep Duration PT5M), StartAutomationExecution it, call StopAutomationExecution on the returned AutomationExecutionId, then poll GetAutomationExecution. Observe AutomationExecutionStatus stuck at 'InProgress' (and StepExecutions[0].StepStatus 'InProgress') with no ExecutionEndTime after 30 polls, the Parameters/Outputs/Targets/ResolvedTargets and per-step Outputs/OverriddenParameters/IsStepBlocked members absent rather than empty/false, and StepExecutions[0].Inputs.Duration rendered as PT5M rather than as the JSON string literal "PT5M".
