Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-create-and-migrate-workspace-to-new-bundle
Issue-Step: teardown#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-create-and-migrate-workspace-to-new-bundle workspaces-create-and-rebuild-workspace workspaces-create-and-restore-workspace workspaces-create-primary-and-standby-workspace-dr workspaces-create-workspaces-batch-start-stop-reboot workspaces-operate-on-terminated-workspace-errors
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces|TerminateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorCode
Issue-Key: workspaces|TerminateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorMessage
Issue-Key: workspaces|StartWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorCode
Issue-Key: workspaces|StartWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorMessage
Issue-Key: workspaces|StopWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorCode
Issue-Key: workspaces|StopWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorMessage
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# WorkSpaces batch change operations return exception-style ErrorCode instead of ErrorType codes

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TerminateWorkspaces

## Description
The WorkSpaces batch change operations (TerminateWorkspaces, StartWorkspaces, StopWorkspaces) report per-request failures inside a 200 response body, in FailedWorkspaceChangeRequest.ErrorCode. The emulator populates that member with the modeled EXCEPTION shape name ('ResourceNotFoundException') rather than the WorkSpaces per-item ErrorType vocabulary ('ResourceNotFound.Workspace'). This is one serialization fault shared by three operations; three causes below.

Cause 1 of 3: TerminateWorkspaces per-item ErrorCode.
AWS: Smithy: TerminateWorkspaces has NO modeled errors and output FailedRequests -> FailedTerminateWorkspaceRequests; the per-item member FailedWorkspaceChangeRequest.ErrorCode targets ErrorType ('The error code that is returned if the WorkSpace cannot be ... terminated'), which is a free-form per-item code distinct from the service's exception shapes. Recorded behavior: terminating a WorkSpace id that does not exist yields HTTP 200 with FailedRequests[0].ErrorCode = 'ResourceNotFound.Workspace' and the fixed ErrorMessage 'The specified WorkSpace could not be found.' (message quoted as an example of the recorded text).
Emulator: HTTP 200 with FailedRequests[0].ErrorCode = 'ResourceNotFoundException' and an id-interpolated ErrorMessage ('The WorkSpace ws-... could not be found.').
Why: clients branch on the dotted per-item ErrorCode to tell a missing WorkSpace apart from a state/permission failure inside a partially successful batch; an exception-shape name matches no branch, so cleanup and retry logic treats an already-gone WorkSpace as an unknown failure and the batch is reported as hard-failed.
Fix: the per-item failure serializer for the batch change operations (map internal not-found to the ErrorType code vocabulary, and emit the fixed per-item message), not the top-level error path.
Traces: workspaces-create-and-migrate-workspace-to-new-bundle#teardown#5 (representative; cluster also covers workspaces-create-and-rebuild-workspace#teardown#5, workspaces-create-and-restore-workspace#teardown#5, workspaces-create-primary-and-standby-workspace-dr#teardown#5, workspaces-create-workspaces-batch-start-stop-reboot#teardown#7, workspaces-operate-on-terminated-workspace-errors#setup#4, workspaces-operate-on-terminated-workspace-errors#task#13).

Cause 2 of 3: StartWorkspaces per-item ErrorCode.
AWS: Smithy: StartWorkspaces has NO modeled errors; output FailedRequests -> FailedStartWorkspaceRequests of FailedWorkspaceChangeRequest, ErrorCode targets ErrorType. Recorded behavior: starting an unknown WorkSpace id yields HTTP 200 with FailedRequests[0].ErrorCode = 'ResourceNotFound.Workspace' and ErrorMessage 'The specified WorkSpace could not be found.'
Emulator: HTTP 200 with FailedRequests[0].ErrorCode = 'ResourceNotFoundException' and an id-interpolated message.
Why: same client-side consequence as cause 1 for start flows.
Fix: same per-item failure serializer, shared by StartWorkspaces.
Traces: workspaces-create-workspaces-batch-start-stop-reboot#task#4 (cluster also covers workspaces-operate-on-terminated-workspace-errors#task#5).

Cause 3 of 3: StopWorkspaces per-item ErrorCode.
AWS: Smithy: StopWorkspaces has NO modeled errors; output FailedRequests -> FailedStopWorkspaceRequests of FailedWorkspaceChangeRequest, ErrorCode targets ErrorType. Recorded behavior: stopping an unknown WorkSpace id yields HTTP 200 with FailedRequests[0].ErrorCode = 'ResourceNotFound.Workspace' and ErrorMessage 'The specified WorkSpace could not be found.'
Emulator: HTTP 200 with FailedRequests[0].ErrorCode = 'ResourceNotFoundException' and an id-interpolated message.
Why: same client-side consequence as cause 1 for stop flows.
Fix: same per-item failure serializer, shared by StopWorkspaces.
Traces: workspaces-create-workspaces-batch-start-stop-reboot#task#5 (cluster also covers workspaces-operate-on-terminated-workspace-errors#task#6).

No open issue covers these; the existing open workspaces issues are unimplemented-operation stubs (CreateConnectionAlias, CreateIpGroup, DescribeAccount, ListAccountLinks) and the CreateIpGroup 501 race, all unrelated to batch per-item error serialization.

## Evidence
Replay of simparity-wire-aws-v1, three mismatch clusters, all HTTP 200 expected and 200 actual, body-only divergence:

TerminateWorkspaces (workspaces-create-and-migrate-workspace-to-new-bundle#teardown#5, 7 affected steps):
  $.FailedRequests[0].ErrorCode: expected 'ResourceNotFound.Workspace', got 'ResourceNotFoundException'
  $.FailedRequests[0].ErrorMessage: expected 'The specified WorkSpace could not be found.', got 'The WorkSpace ws-sim2realprobewor could not be found.'

StartWorkspaces (workspaces-create-workspaces-batch-start-stop-reboot#task#4, 2 affected steps):
  $.FailedRequests[0].ErrorCode: expected 'ResourceNotFound.Workspace', got 'ResourceNotFoundException'
  $.FailedRequests[0].ErrorMessage: expected 'The specified WorkSpace could not be found.', got 'The WorkSpace ws-sim2realprobewor could not be found.'

StopWorkspaces (workspaces-create-workspaces-batch-start-stop-reboot#task#5, 2 affected steps):
  $.FailedRequests[0].ErrorCode: expected 'ResourceNotFound.Workspace', got 'ResourceNotFoundException'
  $.FailedRequests[0].ErrorMessage: expected 'The specified WorkSpace could not be found.', got 'The WorkSpace ws-sim2realprobewor could not be found.'

Model grounding: FailedWorkspaceChangeRequest.ErrorCode -> ErrorType (per-item code), not an exception shape name; Terminate/Start/StopWorkspaces declare no modeled errors, so the only failure channel is the 200-body FailedRequests list.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture workspaces-create-and-migrate-workspace-to-new-bundle, step teardown#5: call TerminateWorkspaces with a WorkspaceId that does not exist. Observe 200 with FailedRequests[0].ErrorCode = 'ResourceNotFoundException' where the recording has 'ResourceNotFound.Workspace'.
3. Fixture workspaces-create-workspaces-batch-start-stop-reboot, step task#4: call StartWorkspaces with a nonexistent WorkspaceId; same ErrorCode divergence.
4. Same fixture, step task#5: call StopWorkspaces with a nonexistent WorkspaceId; same ErrorCode divergence.
