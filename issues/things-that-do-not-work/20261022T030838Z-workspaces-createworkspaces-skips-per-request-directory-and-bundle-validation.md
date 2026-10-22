Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-create-and-migrate-workspace-to-new-bundle
Issue-Step: setup#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-create-and-migrate-workspace-to-new-bundle workspaces-create-and-rebuild-workspace workspaces-create-and-restore-workspace workspaces-create-primary-and-standby-workspace-dr workspaces-operate-on-terminated-workspace-errors workspaces-create-workspaces-batch-start-stop-reboot
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*]
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.PendingRequests[*]
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorCode
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorMessage
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].WorkspaceRequest.BundleId
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].WorkspaceRequest.Tags
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].WorkspaceRequest.WorkspaceProperties
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# CreateWorkspaces skips per-request directory and bundle validation and truncates the echoed WorkspaceRequest

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkspaces

## Description
CreateWorkspaces must validate each WorkspaceRequest against existing directories and bundles and report the unsatisfiable ones in the 200-response FailedRequests list, echoing the submitted request verbatim. The emulator instead admits the request, mints a WorkSpace, and returns it in PendingRequests; where it does fail a request it uses the wrong per-item code and drops echoed members. Two causes below.

Cause 1 of 2: a request naming an unregistered DirectoryId is accepted instead of failed.
AWS: Smithy: CreateWorkspaces input Workspaces -> WorkspaceRequestList (required, @length 1..25); output has FailedRequests -> FailedCreateWorkspaceRequests and PendingRequests -> WorkspaceList; FailedCreateWorkspaceRequest carries WorkspaceRequest ('Information about the WorkSpace'), ErrorCode -> ErrorType and ErrorMessage. Recorded behavior: creating a WorkSpace in a directory that is not registered returns HTTP 200 with an EMPTY PendingRequests and FailedRequests[0] = {ErrorCode 'ResourceNotFound.Directory', ErrorMessage 'The specified directory could not be found in the specified region.', WorkspaceRequest echoed in full - BundleId, DirectoryId, UserName, Tags and WorkspaceProperties exactly as submitted}.
Emulator: HTTP 200 with FailedRequests empty and PendingRequests[0] a freshly minted WorkSpace in State PENDING for the nonexistent directory.
Why: the emulator silently creates resources AWS refuses, so every downstream step runs against state that cannot exist on AWS. A client's negative path (checking FailedRequests before polling) never fires, the caller believes a directory is registered when it is not, and the fixture's later describe/terminate steps operate on a phantom WorkSpace.
Fix: CreateWorkspaces handler/validation - resolve DirectoryId (and BundleId) against state per request element and route unresolvable elements into FailedRequests with the ErrorType code instead of admitting them.
Traces: workspaces-create-and-migrate-workspace-to-new-bundle#setup#3 (representative; cluster also covers workspaces-create-and-rebuild-workspace#task#3, workspaces-create-and-restore-workspace#setup#3, workspaces-create-primary-and-standby-workspace-dr#setup#3, workspaces-operate-on-terminated-workspace-errors#setup#3).

Cause 2 of 2: a two-element batch over an unregistered directory and an unknown bundle yields one wrong failure plus a spurious success, with a truncated WorkspaceRequest echo.
AWS: same model grounding as cause 1. Recorded behavior for a batch whose first element names an unregistered directory and whose second names an unknown bundle: HTTP 200, PendingRequests empty, FailedRequests of length 2 - [0] ErrorCode 'ResourceNotFound.Directory' with the submitted WorkspaceRequest echoed (BundleId, Tags, WorkspaceProperties intact), [1] ErrorCode 'ResourceNotFound.Bundle', ErrorMessage 'The specified bundle could not be found in the specified region.' with its own request echoed. Messages quoted as examples of the recorded text; the per-item ErrorCode values are the requirement.
Emulator: HTTP 200 with a single FailedRequests entry carrying ErrorCode 'ResourceAlreadyExistsException' (an exception shape name, not an ErrorType code) and a duplicate-user message, whose echoed WorkspaceRequest has the wrong BundleId and omits Tags and WorkspaceProperties entirely; the second unsatisfiable element is missing from FailedRequests altogether, and a PENDING WorkSpace appears in PendingRequests instead.
Why: the batch result no longer corresponds one-to-one with the submitted list, so a client cannot match failures back to inputs or retry the right elements; it also sees a conflict code where AWS reports a missing directory, and a creation that AWS never performed.
Fix: CreateWorkspaces handler/validation plus the FailedCreateWorkspaceRequest serializer - validate every element independently, preserve submitted-request echo (BundleId/Tags/WorkspaceProperties) byte-for-byte, and emit ErrorType codes.
Traces: workspaces-create-workspaces-batch-start-stop-reboot#task#3.

No open issue covers this; the open workspaces issues are unimplemented-operation stubs (CreateConnectionAlias, CreateIpGroup, DescribeAccount, ListAccountLinks) and the CreateIpGroup 501 race.

## Evidence
Replay of simparity-wire-aws-v1, two mismatch clusters, expected 200 / actual 200, body-only divergence:

Cluster A - workspaces-create-and-migrate-workspace-to-new-bundle#setup#3 (5 affected steps):
  $.FailedRequests[0]: missing (expected {'ErrorCode': 'ResourceNotFound.Directory', 'ErrorMessage': 'The specified directory could not be found in the specified region.', 'WorkspaceRequest': {'BundleId': 'wsb-...', 'DirectoryId': 'd-...', 'Tags': [{'Key': 'probe', ...}], 'UserName': 'Administrator', 'WorkspaceProperties': {'RecycleMode': 'DISABLED', 'RunningMode': 'ALWAYS_ON'}}}, got None)
  $.PendingRequests[0]: extra (expected None, got a minted WorkSpace in State 'PENDING')

Cluster B - workspaces-create-workspaces-batch-start-stop-reboot#task#3:
  $.FailedRequests[0].ErrorCode: expected 'ResourceNotFound.Directory', got 'ResourceAlreadyExistsException'
  $.FailedRequests[0].ErrorMessage: expected 'The specified directory could not be found in the specified region.', got a duplicate-user message
  $.FailedRequests[0].WorkspaceRequest.BundleId: echoed value differs from the submitted BundleId
  $.FailedRequests[0].WorkspaceRequest.Tags: missing (submitted Tags not echoed)
  $.FailedRequests[0].WorkspaceRequest.WorkspaceProperties: missing (submitted WorkspaceProperties not echoed)
  $.FailedRequests[1]: missing (expected a second failure with ErrorCode 'ResourceNotFound.Bundle')
  $.PendingRequests[0]: extra (expected None, got a minted WorkSpace in State 'PENDING')

Model grounding: CreateWorkspaces output = {FailedRequests: FailedCreateWorkspaceRequests, PendingRequests: WorkspaceList}; FailedCreateWorkspaceRequest = {WorkspaceRequest, ErrorCode -> ErrorType, ErrorMessage}; the operation's only modeled errors are InvalidParameterValuesException and ResourceLimitExceededException, so missing-resource outcomes are reported per element in the 200 body.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture workspaces-create-and-migrate-workspace-to-new-bundle, step setup#3: call CreateWorkspaces with one WorkspaceRequest naming a DirectoryId that was never registered. Observe FailedRequests empty and a PENDING WorkSpace in PendingRequests, where the recording has FailedRequests[0].ErrorCode = 'ResourceNotFound.Directory' and no pending request.
3. Fixture workspaces-create-workspaces-batch-start-stop-reboot, step task#3: call CreateWorkspaces with two requests - one for an unregistered directory, one for an unknown bundle. Observe a single FailedRequests entry with ErrorCode 'ResourceAlreadyExistsException', a truncated WorkspaceRequest echo (no Tags/WorkspaceProperties, altered BundleId), no second failure entry, and a spurious PendingRequests entry.
