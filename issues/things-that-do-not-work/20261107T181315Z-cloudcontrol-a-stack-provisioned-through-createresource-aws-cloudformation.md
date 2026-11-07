Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.delete-resource-during-in-progress-create-conflict
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.delete-resource-during-in-progress-create-conflict|_poll_until#11|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# cloudcontrol: a stack provisioned through CreateResource(AWS::CloudFormation::Stack) is not observable through CloudFormation's own reads — DescribeStackResource never reports the stack's WaitCondition as CREATE_IN_PROGRESS, so a held create can never be released or driven to a terminal state

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
ONE net-new root cause in the cloudcontrol (Cloud Control API) service. A fixer must close it before submitting.

Cause 1 of 1: a CreateResource for TypeName "AWS::CloudFormation::Stack" is accepted, mints a RequestToken, names an Identifier and correctly stays IN_PROGRESS, and a DeleteResource aimed at that same Identifier while the create is in flight is correctly refused ConcurrentOperationException with AWS's message — but the stack the request describes is never visible through CloudFormation's own read path. CloudFormation DescribeStackResource(StackName=<the stack name in DesiredState>, LogicalResourceId=<a logical resource in TemplateBody>) never reports that resource with ResourceStatus "CREATE_IN_PROGRESS", so the AWS::CloudFormation::WaitCondition that gates the create can never be found, SignalResource can never be sent, and the Cloud Control request can never be driven to a terminal state.

AWS: Cloud Control API is the uniform control plane over the CloudFormation resource registry, and AWS::CloudFormation::Stack is a public registry type. CreateResource (Smithy: com.amazonaws.cloudcontrol#CreateResource; TypeName required, length 10-196, pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}$; DesiredState required; output ProgressEvent) asynchronously runs that type's create handler, which creates a REAL CloudFormation stack. While that stack is CREATE_IN_PROGRESS the Cloud Control request reports OperationStatus IN_PROGRESS carrying the stack's Identifier, AND the stack plus every logical resource in its template is readable through CloudFormation's own API: DescribeStacks reports the stack, and DescribeStackResource(StackName, LogicalResourceId) returns a StackResourceDetail whose ResourceStatus is "CREATE_IN_PROGRESS" for a resource still being created — for an AWS::CloudFormation::WaitCondition with a CreationPolicy.ResourceSignal that is the state in which it waits. SignalResource(StackName, LogicalResourceId, UniqueId, Status="SUCCESS") then satisfies the wait condition, the stack reaches CREATE_COMPLETE, and with it the Cloud Control request reaches OperationStatus SUCCESS; a subsequent DeleteResource for the same Identifier is then accepted, reaches SUCCESS, and DescribeStacks reports StackStatus DELETE_COMPLETE. Recorded behavior for the parts the model does not state: the check's earlier beats (the IN_PROGRESS create, the 409 ConcurrentOperationException refusal with message "Concurrent operation found for resource <Identifier> with RequestToken <token of the in-flight request>" — fixed text "Concurrent operation found for resource " and " with RequestToken ", with the identifier taken from the request and the token taken from the in-flight request in the service's state, Smithy: ConcurrentOperationException modeled on DeleteResource, httpError 409 — and the create still IN_PROGRESS afterwards) all passed, so they are not part of this cause.

Emulator: the flow gets past the refusal and then stalls in the release step. The check polls CloudFormation DescribeStackResource(StackName=<the created stack name>, LogicalResourceId="Gate") for 120 s and that read never reports ResourceStatus "CREATE_IN_PROGRESS" — it either raises a ClientError (the stack or the logical resource is unknown to CloudFormation) or reports some other status. The replay ends with: AssertionError: the WaitCondition of <stack name> waiting for its signal not met within 120s. Last result: False. Because the gate is never observed waiting, SignalResource is never sent and the held Cloud Control create is never released; the create request's terminal state and the subsequent accepted DeleteResource (and the stack's DELETE_COMPLETE) are never reached.

Why: a client that provisions a stack through Cloud Control — the documented, uniform way to drive registry types — gets a RequestToken for work no owning service ever performs observably. The stack cannot be inspected, coordinated with or signalled through CloudFormation (DescribeStacks / DescribeStackResource / SignalResource), so any workflow that drives a gated stack to completion (resource signals, CreationPolicy-based provisioning, drift inspection of a Cloud-Control-created stack) blocks forever instead of completing or failing, and the in-flight-create conflict window can never be closed.

Fix: the Cloud Control resource-effect application for registry types that are backed by another service — specifically the AWS::CloudFormation::Stack provider behind CreateResource/DeleteResource and the deferred materialisation it drives, in the cloudcontrol crate's create/delete handlers and its resource state — must create the stack in CloudFormation's own state (stack plus the template's logical resources, each with its own ResourceStatus lifecycle) and derive the Cloud Control request's OperationStatus from that stack's lifecycle, so the request stays IN_PROGRESS while the stack is CREATE_IN_PROGRESS and terminates only when the stack does.

Related but not covering: open issue 20261103T200743Z-cloudcontrol-a-cloud-control-createresource-of-aws-cloudformation.md was filed from a run of this same check in which the whole flow timed out at the harness cap with no response-level assertion; its replay key is the harness timeout. In this run the emulator progresses further — the create is accepted, stays IN_PROGRESS and names an Identifier, and the ConcurrentOperationException refusal and its no-effect property now hold — and the failure is a distinct, specific one: the CloudFormation-side visibility of the stack's logical resources. The suite did not mark this check owned, so it is reported here. The private-store/owning-service split filed in 20261102T095426Z-cloudcontrol-6-net-new-causes-cloud-control-s.md (Logs, CloudWatch, SQS, SSM) and in the reverse direction in 20261103T111126Z-cloudcontrol-4-net-new-causes-cloudformation-provisioned-resources.md names neither AWS::CloudFormation::Stack nor the DescribeStackResource read; if one shared join between the Cloud Control store and the owning services lands, this cause closes with it.

Traces: cloudcontrol.delete-resource-during-in-progress-create-conflict.

## Evidence
From the replay of suite simparity-behavioral-aws-v1 against the emulator. Generated stack names, identifiers, request tokens and timings are per-run values, not requirements.

[mismatch] cloudcontrol.delete-resource-during-in-progress-create-conflict —
  File "check_cloudcontrol_delete_resource_during_in_progress_create_conflict.py", line 107, in _release
    _poll_until(...)
  File "check_cloudcontrol_delete_resource_during_in_progress_create_conflict.py", line 89, in _poll_until
    raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
  AssertionError: the WaitCondition of sim2real-behav-ccrace-<suffix> waiting for its signal not met within 120s. Last result: False

The probe behind that predicate is:
  cfn.describe_stack_resource(StackName=<stack name>, LogicalResourceId="Gate")["StackResourceDetail"]["ResourceStatus"] == "CREATE_IN_PROGRESS"
with any ClientError treated as False. So for 120 s DescribeStackResource either refused the read or reported a status other than CREATE_IN_PROGRESS.

What already holds in this run (passed assertions, not part of this cause): CreateResource(TypeName="AWS::CloudFormation::Stack") returns a ProgressEvent with OperationStatus PENDING/IN_PROGRESS; GetResourceRequestStatus yields an IN_PROGRESS event carrying Identifier; DeleteResource for that Identifier is refused with code ConcurrentOperationException and message exactly "Concurrent operation found for resource <Identifier> with RequestToken <create token>"; GetResourceRequestStatus still reports IN_PROGRESS after the refusal.

Expected, from the point of divergence:
1. cfn.describe_stack_resource(StackName=<stack name from DesiredState.StackName>, LogicalResourceId="Gate") returns 200 with StackResourceDetail.ResourceStatus == "CREATE_IN_PROGRESS" while the gated create is held.
2. cfn.signal_resource(StackName=<stack name>, LogicalResourceId="Gate", UniqueId=<any>, Status="SUCCESS") succeeds and releases the wait condition.
3. GetResourceRequestStatus for the create token then reaches OperationStatus "SUCCESS".
4. DeleteResource for the same Identifier is now ACCEPTED, its request reaches "SUCCESS", and cfn.describe_stacks(StackName=<Identifier>) reports StackStatus "DELETE_COMPLETE".

Actual: step 1 never happens within 120 s, so steps 2-4 are never reached. The stack Cloud Control reports as being created is not present (or not resource-level readable) in CloudFormation's state.

## Steps to Reproduce
Boot the emulator; cc = client("cloudcontrol"), cfn = client("cloudformation").

1. template = json.dumps({"Resources": {"Gate": {"Type": "AWS::CloudFormation::WaitCondition", "CreationPolicy": {"ResourceSignal": {"Count": 1, "Timeout": "PT10M"}}}}}).
2. started = cc.create_resource(TypeName="AWS::CloudFormation::Stack", DesiredState=json.dumps({"StackName": n, "TemplateBody": template})); token = started["ProgressEvent"]["RequestToken"]. Poll cc.get_resource_request_status(RequestToken=token) until the ProgressEvent carries Identifier; it must report OperationStatus "IN_PROGRESS" (the WaitCondition holds the create open).
3. cc.delete_resource(TypeName="AWS::CloudFormation::Stack", Identifier=<identifier from step 2>) must be refused HTTP 409 ConcurrentOperationException with message "Concurrent operation found for resource <identifier> with RequestToken <token>", and cc.get_resource_request_status(RequestToken=token) must still report IN_PROGRESS. (Both already hold.)
4. THE DIVERGENCE: cfn.describe_stack_resource(StackName=n, LogicalResourceId="Gate") must return 200 with StackResourceDetail.ResourceStatus == "CREATE_IN_PROGRESS". Today this read does not yield that status within 120 s of polling.
5. cfn.signal_resource(StackName=n, LogicalResourceId="Gate", UniqueId="release", Status="SUCCESS"); then poll the create token — it must reach OperationStatus "SUCCESS".
6. cc.delete_resource for the same Identifier must now be accepted, its request must reach "SUCCESS", and cfn.describe_stacks(StackName=<identifier>) must report StackStatus "DELETE_COMPLETE".

No step may depend on a state change that never happens.
