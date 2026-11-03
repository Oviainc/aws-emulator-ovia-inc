Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.delete-resource-during-in-progress-create-conflict
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.delete-resource-during-in-progress-create-conflict|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: 8b4952d5bd6ac92656117b5e6e80986c41b691fd

# cloudcontrol: a Cloud Control CreateResource of AWS::CloudFormation::Stack never becomes observable at CloudFormation, so an in-flight create window can neither be released nor driven to a terminal state (check hangs)

> _Note: this issue was identified against the code base at commit `8b4952d5bd6ac92656117b5e6e80986c41b691fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
ONE net-new root cause in the cloudcontrol (Cloud Control API) service. A fixer must close it before submitting.

Cause 1 of 1: a CreateResource for TypeName "AWS::CloudFormation::Stack" is accepted and a RequestToken minted, but the stack it describes is never provisioned at CloudFormation, and the Cloud Control request's lifecycle is never joined to the stack's lifecycle. The replayed check therefore never completes: it hangs until the suite's 60 s per-check budget expires.

AWS: Cloud Control API is the uniform control plane over the CloudFormation resource registry, and AWS::CloudFormation::Stack is a public registry type. CreateResource(TypeName="AWS::CloudFormation::Stack", DesiredState={"StackName":<name>,"TemplateBody":<template>}) (Smithy: com.amazonaws.cloudcontrol#CreateResource, DesiredState and TypeName required; output ProgressEvent) asynchronously runs that type's create handler, which creates a REAL CloudFormation stack: while the stack is CREATE_IN_PROGRESS the Cloud Control request reports OperationStatus IN_PROGRESS with the stack's identifier, the stack and its logical resources are visible through CloudFormation's own reads (DescribeStacks / DescribeStackResource, which reports a gating AWS::CloudFormation::WaitCondition as CREATE_IN_PROGRESS), and SignalResource against that WaitCondition is what lets the stack — and with it the Cloud Control request — reach its terminal state (SUCCESS). A DeleteResource aimed at the same identifier while that create is still running is refused ConcurrentOperationException (Smithy: modeled on DeleteResource, httpError 409, "Another resource operation is currently being performed on this resource") with the message "Concurrent operation found for resource <Identifier> with RequestToken <token of the request that holds the resource>" — fixed text "Concurrent operation found for resource " and " with RequestToken ", with the identifier taken from the request and the token taken from the in-flight request in the service's state — and the refusal has no effect: the held create stays IN_PROGRESS, and once it has finished the same DeleteResource is accepted and the stack reaches DELETE_COMPLETE.

Emulator: the replay of this flow produces no response-level assertion at all — it reports only "Check timed out after 60.0s", i.e. one of the reads the flow depends on never changes state. The accepted create is recorded purely inside Cloud Control's own store (services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/create_resource.rs records a RequestRecord carrying a ResourceEffect::Create of the parsed DesiredState properties and a deferred `materialise_due` effect), so nothing about the request is ever handed to CloudFormation. Hypothesis (the timeout names no stage, so the stalling read is inferred from the check's shape and from that handler): the flow gets past the create and the ConcurrentOperationException refusal and then stalls on the CloudFormation reads — DescribeStackResource for the template's "Gate" WaitCondition never reports CREATE_IN_PROGRESS because no stack exists at CloudFormation, so SignalResource is never sent and the Cloud Control request can never be released to a terminal state. Equivalently the request's OperationStatus is not a function of any stack lifecycle at all.

Why: a client that provisions a stack through Cloud Control (the documented, uniform way to drive registry types) gets a RequestToken for work that no owning service ever performs: the stack is absent from CloudFormation, SignalResource/DescribeStackResource cannot be used to coordinate with it, and any workflow that polls the Cloud Control request to a terminal state while CloudFormation drives the stack blocks forever instead of completing or failing. The in-flight-create conflict semantics (ConcurrentOperationException and the subsequent accepted delete) are unverifiable for this type because the window can never be opened or closed deterministically.

Fix: the CreateResource/DeleteResource effect application for registry types backed by another service — here the AWS::CloudFormation::Stack provider in the cloudcontrol crate's resource-effect/state code and the deferred materialisation it drives (services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/state.rs, ops/create_resource.rs, ops/delete_resource.rs) — must create and delete the resource in the OWNING service (CloudFormation) and derive the request's OperationStatus from that service's lifecycle, so the request stays IN_PROGRESS while the stack is CREATE_IN_PROGRESS and only terminates when the stack does. This is the same private-store/owning-service split already filed for other types in 20261102T095426Z-cloudcontrol-6-net-new-causes-cloud-control-s.md (Logs, CloudWatch, SQS, SSM) and in the reverse direction in 20261103T111126Z-cloudcontrol-4-net-new-causes-cloudformation-provisioned-resources.md (Cause 1 of 4); neither names AWS::CloudFormation::Stack and the suite did not mark this check owned, so it is reported here. If a single shared join between the Cloud Control store and the owning services lands, this cause closes with it.

Traces: cloudcontrol.delete-resource-during-in-progress-create-conflict.

## Evidence
From the replay of suite simparity-behavioral-aws-v1 against the emulator. Generated stack names, identifiers, request tokens and timings are per-run values, not requirements.

[mismatch] cloudcontrol.delete-resource-during-in-progress-create-conflict — Error: "Check timed out after 60.0s". The check's own waits are longer than the suite budget (120 s for the create request to name its stack, 120 s for the WaitCondition to be observed waiting, 180 s for the create to terminate), so the 60 s expiry means a read it polls never changed state; no response-level assertion was ever reached.

Expected, in order:
1. cc.create_resource(TypeName="AWS::CloudFormation::Stack", DesiredState={"StackName": n, "TemplateBody": <WaitCondition template>}) returns ProgressEvent with OperationStatus PENDING or IN_PROGRESS; polling GetResourceRequestStatus yields an IN_PROGRESS event carrying Identifier.
2. cc.delete_resource(TypeName="AWS::CloudFormation::Stack", Identifier=<that identifier>) is refused HTTP 409 ConcurrentOperationException, message exactly "Concurrent operation found for resource <Identifier> with RequestToken <create token>".
3. GetResourceRequestStatus for the create token still reports IN_PROGRESS (the refused delete had no effect).
4. cfn.describe_stack_resource(StackName=n, LogicalResourceId="Gate") reports ResourceStatus CREATE_IN_PROGRESS, and cfn.signal_resource(...Status="SUCCESS") releases it; the create request then reaches SUCCESS.
5. The same cc.delete_resource is now accepted and reaches SUCCESS, and cfn.describe_stacks(StackName=<identifier>) reports StackStatus DELETE_COMPLETE.

Actual: the flow never reaches step 5 within the budget; the emulator's CreateResource records the request only in Cloud Control's own store (ops/create_resource.rs: RequestRecord::accepted(..., ResourceEffect::Create{properties, identifier_property, type_version_id}) plus store.materialise_due(now)), so no CloudFormation stack is created and no stack lifecycle can drive the request or answer the CloudFormation reads the flow depends on.

## Steps to Reproduce
Boot the emulator; cc = client("cloudcontrol"), cfn = client("cloudformation").

1. template = json.dumps({"Resources": {"Gate": {"Type": "AWS::CloudFormation::WaitCondition", "CreationPolicy": {"ResourceSignal": {"Count": 1, "Timeout": "PT10M"}}}}}).
2. started = cc.create_resource(TypeName="AWS::CloudFormation::Stack", DesiredState=json.dumps({"StackName": n, "TemplateBody": template})); token = started["ProgressEvent"]["RequestToken"]. Poll cc.get_resource_request_status(RequestToken=token) until the ProgressEvent carries Identifier: it must report OperationStatus IN_PROGRESS (the WaitCondition holds the create open).
3. cfn.describe_stack_resource(StackName=n, LogicalResourceId="Gate") must report ResourceStatus "CREATE_IN_PROGRESS" — the stack Cloud Control created must be visible through CloudFormation's own reads.
4. cc.delete_resource(TypeName="AWS::CloudFormation::Stack", Identifier=<identifier from step 2>) must be refused with HTTP 409 ConcurrentOperationException and message "Concurrent operation found for resource <identifier> with RequestToken <token>"; cc.get_resource_request_status(RequestToken=token) must still report IN_PROGRESS.
5. cfn.signal_resource(StackName=n, LogicalResourceId="Gate", UniqueId="release", Status="SUCCESS"); poll the create token to a terminal state — it must reach SUCCESS.
6. cc.delete_resource for the same Identifier must now be ACCEPTED, its request must reach SUCCESS, and cfn.describe_stacks(StackName=<identifier>) must report StackStatus "DELETE_COMPLETE".

Each of steps 2-6 must complete without any poll stalling: nothing in the flow may depend on a state change that never happens.
