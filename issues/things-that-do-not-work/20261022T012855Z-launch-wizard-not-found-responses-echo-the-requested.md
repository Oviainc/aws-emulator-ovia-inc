Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: launch-wizard.get-deployment-nonexistent-id-not-found launch-wizard.deploy-create-deployment-unknown-workload-not-found launch-wizard.get-workload-unknown-name-not-found
Issue-Shape-Id: com.amazonaws.launchwizard#LaunchWizard
Issue-Key: launch-wizard.get-deployment-nonexistent-id-not-found|run#5|AssertionError|assert message == "The specified deployment does not exist.", f"got message={message!r}"
Issue-Key: launch-wizard.deploy-create-deployment-unknown-workload-not-found|run#7|AssertionError|assert message == "The specified workload does not exist or insufficient permissions.", (
Issue-Key: launch-wizard.get-workload-unknown-name-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Launch Wizard not-found responses echo the requested identifier instead of AWS's fixed ResourceNotFoundException messages, and GetWorkload answers 501 NotImplemented for an unknown workload

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeployment, CreateDeployment, GetWorkload

## Description
Resource-lookup failures in Launch Wizard do not carry AWS's not-found contract: two operations return a per-request message that renders the identifier the caller asked for, and one does not produce the not-found error at all. 3 causes, all in launch-wizard.

Cause 1 of 3: GetDeployment's ResourceNotFoundException message renders the requested deploymentId.
AWS: GetDeployment for a well-formed but nonexistent deploymentId fails with ResourceNotFoundException and the fixed, identifier-free message "The specified deployment does not exist." Smithy: GetDeployment errors = [ResourceNotFoundException, ValidationException]; ResourceNotFoundException errorKind client, httpError 404, doc "The specified workload or deployment resource can't be found."; GetDeploymentInput.deploymentId @length 2..128 @pattern ^[a-zA-Z0-9-]+$. The fixed message text is recorded behavior.
Emulator: ResourceNotFoundException with "Deployment '<deploymentId>' was not found." — a template that interpolates the caller's id.
Why: AWS deliberately keeps resource-lookup failures identifier-free (existence of a resource id is not disclosed through the message); a client matching AWS's message, and any log pipeline that assumes the not-found message is a constant, diverges, and the emulator reflects caller input back into the error channel.
Fix: the GetDeployment handler's not-found branch (the message template, not the code/status, which are correct).
Traces: launch-wizard.get-deployment-nonexistent-id-not-found; the template lives in services/service-com-amazonaws-launchwizard-launchwizard/src/ops/get_deployment.rs around lines 151-161.

Cause 2 of 3: CreateDeployment's unknown-workload ResourceNotFoundException message renders the requested workloadName.
AWS: CreateDeployment naming a workloadName absent from the catalog fails with ResourceNotFoundException and the fixed message "The specified workload does not exist or insufficient permissions." (recorded behavior; Smithy: CreateDeployment errors = [ResourceLimitException, ResourceNotFoundException, ValidationException]). The message deliberately conflates absence with missing permission.
Emulator: ResourceNotFoundException with "Workload 'Sim2RealNoSuchWorkloadXyz' was not found." — identifier echoed, and the permission arm of AWS's wording dropped.
Why: same as Cause 1 — a caller cannot distinguish the emulator's refusal from AWS's, and the emulator discloses the exact name it was asked about.
Fix: the CreateDeployment handler's workload-resolution branch (message template only; the ResourceNotFoundException code and the no-deployment-created effect were both correct in this replay).
Traces: launch-wizard.deploy-create-deployment-unknown-workload-not-found.

Cause 3 of 3: GetWorkload answers NotImplemented (501) instead of ResourceNotFoundException for an unknown workload name.
AWS: GetWorkload for a well-formed but nonexistent workloadName fails with ResourceNotFoundException (httpError 404 per the model) and the fixed message "The specified workload does not exist or insufficient permissions." Smithy: GetWorkload errors = [ResourceNotFoundException, ValidationException].
Emulator: returns error code "NotImplemented" with "operation GetWorkload is scaffolded but not implemented in this emulator" — the handler is still the scaffold stub (services/service-com-amazonaws-launchwizard-launchwizard/src/ops/get_workload.rs). An issue for that stub was auto-filed for the blocked e2e check (`GetWorkload` unimplemented); this cause records the error contract the implementation must satisfy, which that stub issue does not state: unknown name -> ResourceNotFoundException with AWS's fixed message, and an invalid name -> ValidationException (see the companion ValidationException-message issue for that message rule).
Why: a client's not-found handling path is never exercised — a 501/NotImplemented is not retryable or catchable as ResourceNotFoundException, so every workload-existence probe fails open.
Fix: the GetWorkload handler (implement the catalog lookup and its not-found branch).
Traces: launch-wizard.get-workload-unknown-name-not-found.

## Evidence
launch-wizard.get-deployment-nonexistent-id-not-found — code ResourceNotFoundException matched; expected message "The specified deployment does not exist.", got "Deployment 'sim2real0000000000nosuch' was not found."
launch-wizard.deploy-create-deployment-unknown-workload-not-found — code ResourceNotFoundException matched; expected message "The specified workload does not exist or insufficient permissions.", got "Workload 'Sim2RealNoSuchWorkloadXyz' was not found."
launch-wizard.get-workload-unknown-name-not-found — expected ResourceNotFoundException, got 'NotImplemented' ("operation GetWorkload is scaffolded but not implemented in this emulator").
Model consulted: services/service-com-amazonaws-launchwizard-launchwizard/smithy-model/com-amazonaws-launchwizard-launchwizard.json (GetDeployment/CreateDeployment/GetWorkload error lists, ResourceNotFoundException httpError 404). Emulator source consulted: src/ops/get_deployment.rs (not-found message template), src/ops/get_workload.rs (scaffold stub).

## Steps to Reproduce
1. Boot the emulator and point a botocore launch-wizard client at it.
2. Cause 1: lw.get_deployment(deploymentId="sim2real0000000000nosuch") -> inspect Error.Code/Error.Message.
3. Cause 2: lw.create_deployment(workloadName="Sim2RealNoSuchWorkloadXyz", deploymentPatternName="bogusPattern1", name="sim2real-lw-x", specifications={"Placeholder": "value"}) -> inspect Error.Message.
4. Cause 3: lw.get_workload(workloadName="Sim2RealNoSuchWorkloadXyz") -> Error.Code is NotImplemented instead of ResourceNotFoundException.
