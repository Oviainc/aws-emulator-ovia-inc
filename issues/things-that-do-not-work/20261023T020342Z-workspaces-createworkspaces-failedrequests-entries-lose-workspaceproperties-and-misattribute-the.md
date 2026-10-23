Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-create-and-migrate-workspace-to-new-bundle
Issue-Step: setup#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-create-and-migrate-workspace-to-new-bundle workspaces-create-and-rebuild-workspace workspaces-create-and-restore-workspace workspaces-create-primary-and-standby-workspace-dr workspaces-operate-on-terminated-workspace-errors workspaces-create-workspaces-batch-start-stop-reboot
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].WorkspaceRequest.WorkspaceProperties
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorCode
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorMessage
Issue-Commit: 9aaeda1848c43b0bda5e2e66a82e857bd94691b3

# CreateWorkspaces FailedRequests entries lose WorkspaceProperties and misattribute the failure

> _Note: this issue was identified against the code base at commit `9aaeda1848c43b0bda5e2e66a82e857bd94691b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkspaces

## Description
CreateWorkspaces answers 200 with the right FailedRequests cardinality, but each failed entry is built wrongly: the echoed WorkspaceRequest drops WorkspaceProperties, and when more than one field of a request is unresolvable the emulator reports the wrong resource in ErrorCode/ErrorMessage. Two causes.

Cause 1 of 2: the echoed WorkspaceRequest omits WorkspaceProperties (fixture workspaces-create-and-migrate-workspace-to-new-bundle, step setup#3; also workspaces-create-workspaces-batch-start-stop-reboot, step task#3, for both FailedRequests[0] and FailedRequests[1]).
AWS: Smithy - CreateWorkspacesResult.FailedRequests is a list of FailedCreateWorkspaceRequest{WorkspaceRequest, ErrorCode, ErrorMessage}, where WorkspaceRequest 'Information about the WorkSpace' is the submitted request echoed back, including its WorkspaceProperties (WorkspaceProperties{RunningMode, ...}). Recorded behavior: the echoed WorkspaceProperties carries the resolved/defaulted property set, e.g. {'RunningMode': 'ALWAYS_ON', 'RecycleMode': 'DISABLED'} (RecycleMode is not in the on-disk model - recorded behavior only for that member); the requirement here is that WorkspaceProperties is present and reflects the request's effective properties rather than being absent.
Emulator: WorkspaceProperties is missing entirely from the echoed WorkspaceRequest (expected an object, got None) on every failed entry, across five fixtures.
Why: clients retry a partial CreateWorkspaces batch by resubmitting the WorkspaceRequest objects returned in FailedRequests. With WorkspaceProperties stripped, the retry silently loses the running mode and the other requested properties, so the WorkSpace is recreated with different properties than asked for; property-echo assertions in SDK/CLI workflows also fail.
Fix: the CreateWorkspaces handler/serializer must echo the submitted WorkspaceRequest in full into each FailedCreateWorkspaceRequest, including the WorkspaceProperties structure with the effective (defaulted) property values.

Cause 2 of 2: a request whose bundle does not exist is reported as a directory failure (fixture workspaces-create-workspaces-batch-start-stop-reboot, step task#3, FailedRequests[1]).
AWS: Smithy - WorkspaceRequest requires DirectoryId (@pattern ^(d-[0-9a-f]{8,63}$)|(wsd-[0-9a-z]{8,63}$)$) and BundleId (@pattern ^wsb-[0-9a-z]{8,63}$); per-request failures are reported in FailedCreateWorkspaceRequest.ErrorCode/ErrorMessage rather than as a top-level error. Recorded behavior: the entry whose bundle cannot be resolved fails with ErrorCode 'ResourceNotFound.Bundle' and ErrorMessage 'The specified bundle could not be found in the specified region.' (message quoted as an example of that code).
Emulator: the same entry fails with ErrorCode 'ResourceNotFound.Directory' and the corresponding directory message, i.e. it attributes the failure to the directory. Hypothesis: the handler resolves/validates the directory first and reports that code unconditionally, never reaching the bundle lookup or losing which lookup actually failed.
Why: ErrorCode is the field clients branch on. Told the directory is missing when the bundle id is the bad input, remediation goes to the wrong resource - directories get re-registered or the whole batch is abandoned while the real fix is the bundle id.
Fix: the CreateWorkspaces handler's per-request validation must report the code for the resource that actually failed to resolve (ResourceNotFound.Bundle for an unknown BundleId, ResourceNotFound.Directory only for an unknown DirectoryId).

Traces: workspaces-create-and-migrate-workspace-to-new-bundle#setup#3 (representative of a 5-fixture cluster also covering workspaces-create-and-rebuild-workspace#task#3, workspaces-create-and-restore-workspace#setup#3, workspaces-create-primary-and-standby-workspace-dr#setup#3, workspaces-operate-on-terminated-workspace-errors#setup#3) and workspaces-create-workspaces-batch-start-stop-reboot#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (2 mismatch clusters, service workspaces, operation CreateWorkspaces, expected 200 / actual 200):
- workspaces-create-and-migrate-workspace-to-new-bundle#setup#3 (cluster of 5 fixtures): $.FailedRequests[0].WorkspaceRequest.WorkspaceProperties missing - expected {'RecycleMode': 'DISABLED', 'RunningMode': 'ALWAYS_ON'}, got None.
- workspaces-create-workspaces-batch-start-stop-reboot#task#3: $.FailedRequests[0].WorkspaceRequest.WorkspaceProperties missing (expected {'RecycleMode': 'DISABLED', 'RunningMode': 'ALWAYS_ON'}, got None); $.FailedRequests[1].WorkspaceRequest.WorkspaceProperties missing likewise; $.FailedRequests[1].ErrorCode value_mismatch (expected 'ResourceNotFound.Bundle', got 'ResourceNotFound.Directory'); $.FailedRequests[1].ErrorMessage value_mismatch (expected 'The specified bundle could not be found in the specified region.', got 'The specified directory could not be found in the specified region.').
Model grounding: FailedCreateWorkspaceRequest.WorkspaceRequest is documented as 'Information about the WorkSpace' (the echoed request) and WorkspaceRequest.WorkspaceProperties targets WorkspaceProperties.

## Steps to Reproduce
1. Replay fixture workspaces-create-workspaces-batch-start-stop-reboot against the emulator (replay_wire_corpus).
2. Call CreateWorkspaces with a batch in which at least one WorkspaceRequest carries WorkspaceProperties (e.g. RunningMode ALWAYS_ON) and cannot be created, and one whose DirectoryId resolves but whose BundleId does not.
3. Inspect the 200 response: each FailedRequests[].WorkspaceRequest has no WorkspaceProperties member (step task#3 / setup#3).
4. Inspect the entry with the unknown BundleId: ErrorCode is 'ResourceNotFound.Directory' where the recording has 'ResourceNotFound.Bundle'.
