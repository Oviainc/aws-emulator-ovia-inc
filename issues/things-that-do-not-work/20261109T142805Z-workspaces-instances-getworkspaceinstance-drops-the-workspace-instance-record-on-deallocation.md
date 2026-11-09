Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-instances-managed-instance-allocated-and-running
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-instances-managed-instance-allocated-and-running
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: workspaces-instances|GetWorkspaceInstance|2xx->4xx|$
Issue-Key: workspaces-instances|GetWorkspaceInstance|2xx->4xx|$.BillingConfiguration
Issue-Key: workspaces-instances|GetWorkspaceInstance|2xx->4xx|$.EC2ManagedInstance
Issue-Key: workspaces-instances|GetWorkspaceInstance|2xx->4xx|$.ProvisionState
Issue-Key: workspaces-instances|GetWorkspaceInstance|2xx->4xx|$.WorkspaceInstanceId
Issue-Key: workspaces-instances|GetWorkspaceInstance|2xx->4xx|status
Issue-Commit: e565d7c18bac9912b444485dbc603a58639ccbb7

# GetWorkspaceInstance drops the WorkSpace Instance record on deallocation instead of returning it in ProvisionState DEALLOCATED

> _Note: this issue was identified against the code base at commit `e565d7c18bac9912b444485dbc603a58639ccbb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkspaceInstance

## Description
Two causes, both observed on GetWorkspaceInstance when the WorkSpace Instance has finished deallocating (the fixture terminates the managed EC2 instance and then polls GetWorkspaceInstance for the terminal lifecycle state).

Cause 1 of 2: the deallocated WorkSpace Instance is forgotten instead of being reported in its terminal state.
AWS: GetWorkspaceInstance is a read-only lookup over the WorkSpace Instance resource, and DEALLOCATED is a modeled terminal lifecycle value, not a deletion. Smithy: GetWorkspaceInstance (@readonly) -> GetWorkspaceInstanceResponse with members WorkspaceInstanceId, ProvisionState (ProvisionStateEnum), EC2ManagedInstance, BillingConfiguration, WorkspaceInstanceErrors, EC2InstanceErrors; ProvisionStateEnum = ALLOCATING | ALLOCATED | DEALLOCATING | DEALLOCATED | ERROR_ALLOCATING | ERROR_DEALLOCATING. A separate DeleteWorkspaceInstance operation exists, which is what actually removes the resource; deallocation of the backing EC2 instance does not. The recording shows AWS answering 200 after deallocation with the record intact: ProvisionState DEALLOCATED, the same WorkspaceInstanceId that was created, EC2ManagedInstance.InstanceId still naming the (now terminated) managed EC2 instance, and the BillingConfiguration.BillingMode the instance was created with (HOURLY in this recording).
Emulator: once the managed instance stops being allocated, the instance record is gone: GetWorkspaceInstance answers an error whose body is {"__type": "ResourceNotFoundException", "Message": "Workspace instance is not found", "ResourceId": "<the requested WorkspaceInstanceId>", "ResourceType": "WORKSPACE_INSTANCE"} for every poll until the fixture's 180s poll budget is exhausted (19 attempts). WorkspaceInstanceId, ProvisionState, EC2ManagedInstance and BillingConfiguration are all absent.
Why: a client cannot observe the end of the deallocation lifecycle. Any caller that polls GetWorkspaceInstance until ProvisionState reaches a terminal value (DEALLOCATED / ERROR_DEALLOCATING) never terminates against the emulator, and a caller that treats ResourceNotFoundException as "the resource was deleted" reports a deletion that never happened — the resource is still listable/deletable on AWS until DeleteWorkspaceInstance is called. Post-deallocation reads of the retained BillingConfiguration and of EC2ManagedInstance.InstanceId (needed to correlate the terminated EC2 instance) return nothing.
Fix: the WorkSpace Instance lifecycle in the service's state — deallocation must transition the stored record's ProvisionState to DEALLOCATING and then DEALLOCATED, retaining WorkspaceInstanceId, EC2ManagedInstance and BillingConfiguration, and only DeleteWorkspaceInstance may remove it; GetWorkspaceInstance's handler then serves the retained record.
Traces: workspaces-instances-managed-instance-allocated-and-running#task#9.

Cause 2 of 2: ResourceNotFoundException is serialized with the wrong HTTP status.
AWS: Smithy: ResourceNotFoundException is @error("client") with @httpError(404) on this service, and its required members are Message, ResourceId and ResourceType. Every not-found answer from workspaces-instances therefore carries HTTP 404.
Emulator: the ResourceNotFoundException body is returned with HTTP status 400. The error code ("ResourceNotFoundException") and the shape of the body are right; the status line is not. The fixed text of the message is "Workspace instance is not found", with ResourceId taken from the request's WorkspaceInstanceId and ResourceType the constant "WORKSPACE_INSTANCE" — only ResourceId varies per request.
Why: SDK retry/error classification keys off the status as well as the __type header, so a 400 makes a missing resource indistinguishable from a validation failure; waiters and error-handling branches that match on 404 (or on the 4xx-not-found class) take the wrong path, and ValidationException — the error that genuinely is 400 on this service — becomes ambiguous.
Fix: the service's error serializer, which must map each modeled exception to its @httpError value (404 for ResourceNotFoundException) rather than defaulting client errors to 400.
Traces: workspaces-instances-managed-instance-allocated-and-running#task#9.

Note: the open issue 20261024T122725Z-workspaces-instances-createworkspaceinstance-event-propagation.md covers the managed EC2 instance never being created in EC2 state, and 20261107T124004Z-ec2-describeinstances-refuses-the-workspaces-instances-reported-instance.md covers EC2 DescribeInstances rejecting the reported instance id; neither covers the WorkSpace Instance record itself disappearing on deallocation or the error's HTTP status, which is what this issue reports.

## Evidence
Fixture workspaces-instances-managed-instance-allocated-and-running, step task#9, operation GetWorkspaceInstance.

Expected (recorded AWS response): HTTP 200 with body
{"BillingConfiguration": {"BillingMode": "HOURLY"}, "EC2ManagedInstance": {"InstanceId": "i-00000000000000001"}, "ProvisionState": "DEALLOCATED", "WorkspaceInstanceId": "wsinst-0000000000000001"}
(the ids/instance id above are this recording's values; the requirement is that the created WorkspaceInstanceId, its EC2ManagedInstance.InstanceId and its BillingConfiguration are still reported and ProvisionState is DEALLOCATED.)

Actual (emulator): HTTP 400 with body
{"__type": "ResourceNotFoundException", "Message": "Workspace instance is not found", "ResourceId": "wsinst-0000000000000001", "ResourceType": "WORKSPACE_INSTANCE"}

Replay detail: "status: expected 200, got 400"; "$.BillingConfiguration: missing"; "$.EC2ManagedInstance: missing"; "$.ProvisionState: missing"; "$.WorkspaceInstanceId: missing"; "$.ProvisionState: poll: not in ['DEALLOCATED'] after 19 attempt(s)" (poll capped at 180s).

Model evidence: GetWorkspaceInstanceResponse members WorkspaceInstanceId / ProvisionState / EC2ManagedInstance / BillingConfiguration; ProvisionStateEnum includes DEALLOCATING and DEALLOCATED; GetWorkspaceInstance is @readonly; removal is DeleteWorkspaceInstance. ResourceNotFoundException: @error("client"), @httpError(404), required Message/ResourceId/ResourceType.

## Steps to Reproduce
1. Replay fixture workspaces-instances-managed-instance-allocated-and-running against the emulator (replay_wire_corpus) and look at step task#9.
2. Manually: CreateWorkspaceInstance with BillingConfiguration.BillingMode=HOURLY, wait until GetWorkspaceInstance reports ProvisionState ALLOCATED and note the returned WorkspaceInstanceId and EC2ManagedInstance.InstanceId.
3. Drive the instance to deallocation as the fixture does (terminate the managed EC2 instance it reports), without calling DeleteWorkspaceInstance.
4. Call GetWorkspaceInstance with that WorkspaceInstanceId and poll it.
   - Expected: HTTP 200, ProvisionState DEALLOCATING then DEALLOCATED, with WorkspaceInstanceId, EC2ManagedInstance.InstanceId and BillingConfiguration still present.
   - Actual: ResourceNotFoundException ("Workspace instance is not found") on every poll, served with HTTP 400 instead of the modeled 404.
