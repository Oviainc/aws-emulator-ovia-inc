Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workspaces-instances.create-workspace-instance-invalid-ami-error-allocating-transition
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: workspaces-instances.create-workspace-instance-invalid-ami-error-allocating-transition|_error_detail#14|AssertionError|raise AssertionError(f"expected ClientError, but the call SUCCEEDED: {result!r}")
Issue-Commit: c88704caf8c3e34003fb0ddcb0086cf5f8f27701

# CreateWorkspaceInstance accepts a well-formed but nonexistent ManagedInstance.ImageId with HTTP 200 and mints an instance, instead of refusing it with ValidationException "Invalid AMI ID provided"

> _Note: this issue was identified against the code base at commit `c88704caf8c3e34003fb0ddcb0086cf5f8f27701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkspaceInstance

## Description
One root cause in the CreateWorkspaceInstance path of the workspaces-instances emulator: ManagedInstance.ImageId is never resolved against any AMI/image state, so any syntactically well-formed ami- id is accepted and a WorkSpace Instance is created for an image that does not exist.

Cause 1 of 1: a well-formed, nonexistent ImageId is accepted and a WorkSpace Instance is created.
AWS: Smithy (com.amazonaws.workspacesinstances#EUCMIFrontendAPIService, CreateWorkspaceInstance) — input member ManagedInstance targets ManagedInstanceRequest and is @required; the operation is @idempotent and models ValidationException with @httpError 400 (alongside AccessDeniedException 403, ConflictException 409, ServiceQuotaExceededException 402, ThrottlingException 429, InternalServerException 500). ManagedInstanceRequest.ImageId names the AMI the managed EC2 instance is launched from, and the launch happens in the caller's own account/Region, so the id must resolve to an image there. Recorded behavior: CreateWorkspaceInstance with an ImageId that matches the ami- id form but exists in no image of the caller's account/Region is refused AT REQUEST TIME with HTTP 400, error code ValidationException and the message "Invalid AMI ID provided" — that message is fixed text in full, it has no variable parts and in particular does not echo the offending id — and the refused call adds NO WorkSpace Instance (the ListWorkspaceInstances id set, read across all pages, is unchanged across the rejected call, and no instance carrying the request's Name tag appears).
Emulator: the call SUCCEEDS. It answers HTTP 200 with content-type application/x-amz-json-1.0 and a freshly minted WorkspaceInstanceId (observed body shape {"WorkspaceInstanceId": "wsinst-..."}) for ManagedInstance={"ImageId": <a well-formed ami- id that exists nowhere>, "InstanceType": "m5.large", "SubnetId": <a subnet of the signed Region>}. No ValidationException is raised and the instance is persisted, so the id set returned by ListWorkspaceInstances grows by the rejected create. The positive control in the same check — the identical request with a real AMI — also succeeds and reaches ALLOCATED, so the emulator does not distinguish the two ImageId values at all: there is no AMI existence check in the handler, only shape-level acceptance.
Why: a client that relies on AWS refusing a bad AMI at the call site gets a success instead. It records a WorkspaceInstanceId for a resource that can never legitimately allocate, so the error never reaches the caller on the request it belongs to (or never reaches it at all); provisioning code that branches on ValidationException/"Invalid AMI ID provided" to fall back to another image takes the success path, and quota plus state is consumed by a resource AWS would never have created. Any teardown logic keyed on the create having failed is also skipped, leaving an orphan.
Fix: input validation in the CreateWorkspaceInstance handler for this service (ops/create_workspace_instance.rs, with the image lookup in the service's state module) — resolve ManagedInstance.ImageId against the image state of the request's account/Region BEFORE minting a WorkspaceInstanceId or recording any state, and on no match refuse with the modeled ValidationException (HTTP 400) carrying exactly "Invalid AMI ID provided". The refusal must be a pure rejection: no id minted, no record written, no ClientToken consumed.
Related open issue: 20261107T120211Z-workspaces-instances-createworkspaceinstance-3-divergences-wrong-conflictexception-message-for-a.md describes the same acceptance as its "Cause 2 of 3", but the replay suite did not report this check as owned by it (its stored replay key for this check no longer matches the current failure frame), so it is filed here with the live failing check id to re-replay. A fixer closing that issue's Cause 2 closes this one as well.
Traces: workspaces-instances.create-workspace-instance-invalid-ami-error-allocating-transition.

## Evidence
Behavioral suite: workspaces-instances 10/25 passed (40%), 15 failed. 14 of the 15 failures are marked OWNED by an already-open issue and skipped; this is the one surviving failure.

Check workspaces-instances.create-workspace-instance-invalid-ami-error-allocating-transition [mismatch]:
  File "check_workspaces_instances_create_workspace_instance_invalid_ami_error_allocating_transition.py", line 39, in run
    code, message = _error_detail(...)
  File "...", line 142, in _error_detail
    raise AssertionError(f"expected ClientError, but the call SUCCEEDED: {result!r}")
  AssertionError: expected ClientError, but the call SUCCEEDED: {'WorkspaceInstanceId': 'wsinst-0000000000000002', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '49', ...}, 'RetryAttempts': 0}}

Expected (AWS): HTTP 400, Error.Code == "ValidationException", Error.Message == "Invalid AMI ID provided" (fixed text, no substituted values), and the ListWorkspaceInstances id set unchanged — no instance tagged with the request's Name marker is created.
Actual (emulator): HTTP 200 and a newly minted wsinst- WorkspaceInstanceId; the instance is created and listed. The returned id value itself is environment-specific and is not part of the requirement — the requirement is that there is no 200 and no id at all.

The check's positive control (same ManagedInstance with a real AMI resolved from ssm:GetParameter /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64) passes and reaches ProvisionState ALLOCATED, which rules out an environment problem: the emulator treats the nonexistent AMI exactly like the real one.

## Steps to Reproduce
1. Boot the emulator with the workspaces-instances, ec2, iam and ssm services mounted.
2. iam:CreateServiceLinkedRole(AWSServiceName="workspaces-instances.amazonaws.com") (tolerate InvalidInput if it already exists).
3. Pick a subnet of the signed Region via ec2:DescribeVpcs / ec2:DescribeSubnets.
4. Record the full ListWorkspaceInstances id set (page through NextToken).
5. Call CreateWorkspaceInstance with ManagedInstance={"ImageId": "ami-0000000000deadbee" (well-formed ami- form, exists in no image state), "InstanceType": "m5.large", "SubnetId": <that subnet>} and Tags=[{"Key":"Name","Value":<unique marker>}].
6. Observed: HTTP 200 with a WorkspaceInstanceId. Expected: HTTP 400 with Error.Code "ValidationException" and Error.Message "Invalid AMI ID provided".
7. Page ListWorkspaceInstances again and read tags with ListTagsForResource: observed, a new instance carrying Name=<marker> exists. Expected: the id set is unchanged and no such instance exists.
8. Control: repeat step 5 with a real AMI id (ssm:GetParameter "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64") — this must still succeed and reach ProvisionState ALLOCATED after the fix.
