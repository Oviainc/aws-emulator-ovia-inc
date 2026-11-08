Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket
Issue-Step: teardown#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket stack-policy-input-validation cloudformation-exports-refactors-and-organizations-read cloudformation-refarch-s3ev-stack-lookup-from-bucket cloudformation-tsrb-describe-resources-filter-mistakes
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DeleteStack|2xx->4xx|status
Issue-Key: cloudformation|DescribeStacks|4xx->2xx|$
Issue-Key: cloudformation|ListStackResources|2xx->2xx|$.ListStackResourcesResponse.ListStackResourcesResult.StackResourceSummaries.member[LogicalResourceId="Vec"].ResourceStatusReason
Issue-Key: cloudformation|DescribeOrganizationsAccess|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|DescribeOrganizationsAccess|4xx->2xx|status
Issue-Key: cloudformation|DescribeStackResources|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Related: 20261103T171935Z-cloudformation-never-validates-a-stack-policy-so-invalid.md
Issue-Commit: 6211fb78e0841381478152dba6893fe342be7489

# cloudformation: 5 net-new causes — DeleteStack refuses a settled stack, deleted stacks keep answering, and three read/validation shapes diverge

> _Note: this issue was identified against the code base at commit `6211fb78e0841381478152dba6893fe342be7489`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
Bundle of 5 net-new root causes of service `cloudformation` found by replaying the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout. All five clusters report stub_path='' and owned_by=[], so no open issue carries their replay keys. A fixer MUST close all five before submitting.

Cause 1 of 5: DeleteStack on a stack the emulator is still tracking is REFUSED with HTTP 400 instead of accepted.
AWS: Smithy: DeleteStack input StackName (required), optional RetainResources/RoleARN/ClientRequestToken/DeletionMode/DeploymentConfig; output shape is Unit and the ONLY modeled error is TokenAlreadyExistsException (client, httpError 400). So for a stack that exists in any settled status, DeleteStack is accepted: HTTP 200 with an empty DeleteStackResponse body, and deletion proceeds asynchronously. Recorded behavior confirms 200 for both recordings in this cluster.
Emulator: answers HTTP 400. The cluster keys only on `status` (detail: "status: expected 200, got 400"), so the response body of the refusal is not in the replay record. Hypothesis: a stack-state/busy guard in the DeleteStack handler rejects stacks the recorded service accepts (both affected steps are teardown deletes of a stack whose last operation had already settled).
Why: the stack cannot be deleted at all. Every teardown that relies on DeleteStack leaks the stack and all its resources, repeat runs of the same template collide on an already-existing stack name, and `cdk destroy` / `aws cloudformation delete-stack` fail outright.
Fix: the DeleteStack handler's accept/refuse guard (and the stack status it consults) in the cloudformation service's stack lifecycle component.
Traces: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket#teardown#9; stack-policy-input-validation#teardown#11.

Cause 2 of 5: after the delete, DescribeStacks keeps serving the stack instead of refusing it as non-existent.
AWS: Smithy: DescribeStacks input StackName/NextToken, output Stacks; no modeled errors — the not-found case is the service-common query error ValidationError. Recorded behavior: once the named stack is gone, DescribeStacks on that NAME answers HTTP 400 with ErrorResponse.Error.Type='Sender', Code='ValidationError' and Message='Stack with id <StackName> does not exist'. Fixed text: 'Stack with id ' + ' does not exist'; the variable part is the stack name or id taken from the request (the probe stack name in this recording is a per-run value, not part of the requirement).
Emulator: answers HTTP 200 and returns the stack, with StackStatus 'UPDATE_COMPLETE', for the whole 180 s poll window (37 poll attempts, error code None).
Why: a client polling for deletion never observes it. Delete-then-recreate flows and waiters (`stack-delete-complete`) hang until their own timeout, and a caller cannot distinguish a live stack from one it just deleted.
Fix: the DescribeStacks lookup/stack-registry state must stop resolving a deleted stack by name and raise the ValidationError not-found path; follows Cause 1 but must be verified independently, since a delete that is accepted and never removes the stack produces the same wire shape.
Traces: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket#teardown#10.

Cause 3 of 5: a resource that failed to create carries no ResourceStatusReason in ListStackResources.
AWS: Smithy: ListStackResources input StackName (required), output StackResourceSummaries (list of StackResourceSummary, which models ResourceStatusReason). Recorded behavior: for a resource whose create failed inside a stack, the summary for that logical id carries a non-empty ResourceStatusReason explaining the failure (the recording asserts its presence, not its text: '<shaped-not-asserted>'). The requirement is that the member is PRESENT and non-empty for a failed resource; its text is the provider's failure reason.
Emulator: the summary for the failed logical id omits ResourceStatusReason entirely (missing, got None), while status stays 200.
Why: the only machine-readable explanation of why a stack create failed is the per-resource reason. A client (or `cdk deploy` output) reports a failure with no cause, and operators cannot tell a quota refusal from a bad property.
Fix: the ListStackResources serializer / the stack-resource state that records a resource's failure reason at CREATE_FAILED time. Note the open issue 20261108T175930Z-cloudformation-3-net-new-causes-deletestack-s-busy.md describes a CREATE_FAILED resource with no ResourceStatusReason on a different operation/key and does NOT own this cluster's key.
Traces: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket#task#5.

Cause 4 of 5: DescribeOrganizationsAccess succeeds for an account with no organization instead of refusing the call.
AWS: Smithy: DescribeOrganizationsAccess input CallAs, output Status (enum OrganizationStatus), modeled errors InvalidOperationException (400) and OperationNotFoundException (404) — the model does not describe the no-organization case, so recorded behavior only: with no organization set up, the call answers HTTP 400 with ErrorResponse.Error.Type='Sender', Code='ValidationError', Message='You must have an organization setup before operating a SERVICE_MANAGED stack set'. That message is fixed text with no variable parts.
Emulator: answers HTTP 200 with no ErrorResponse at all.
Why: a client probing whether SERVICE_MANAGED stack sets are usable is told they are, then every service-managed StackSet operation it attempts fails later and for a different reason. Organization-trust bootstrap checks silently pass against an account that has no organization.
Fix: the DescribeOrganizationsAccess handler must consult organization state and emit the ValidationError refusal when no organization exists.
Traces: cloudformation-exports-refactors-and-organizations-read#task#3.

Cause 5 of 5: DescribeStackResources rewords the StackName/PhysicalResourceId filter refusal.
AWS: Smithy doc for DescribeStackResources: "You must specify either StackName or PhysicalResourceId, but not both" and "A ValidationError is returned if you specify both StackName and PhysicalResourceId in the same request." Recorded behavior for that request: HTTP 400, ErrorResponse.Error.Type='Sender', Code='ValidationError', Message='Either StackName or PhysicalResourceId must be specified.' — entirely fixed text (no request- or state-derived parts), including the trailing period.
Emulator: same HTTP 400 and the same error code, but Message='Cannot specify both StackName and PhysicalResourceId in the same request' — different wording and no trailing period.
Why: the message is part of the contract. Clients and test suites that match on AWS's refusal text (and humans reading it) see an unrecognised string, so the mutually-exclusive-filter refusal cannot be asserted against the emulator.
Fix: the DescribeStackResources input validation's message template.
Traces: cloudformation-refarch-s3ev-stack-lookup-from-bucket#task#7; cloudformation-tsrb-describe-resources-filter-mistakes#task#4.

## Evidence
Replay of simparity-wire-aws-v1 (119 fixtures, 1080 steps, 919 passed, score 85.1%, emulator_up=true, bootstrap_signal=false) against the emulator built from this checkout. Five divergence clusters, every one verdict=mismatch, stub_path='', owned_by=[]:
1. divergence_key 'service=cloudformation|DeleteStack|status=200->400|keys=status', fixture cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket, step teardown#9, affected_count 2 (also stack-policy-input-validation#teardown#11). Detail: 'status: expected 200, got 400'. Expected 200 with an empty body; got 400.
2. divergence_key 'service=cloudformation|DescribeStacks|status=400->200|keys=$,$.ErrorResponse,status', fixture cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket, step teardown#10, affected_count 1. Expected HTTP 400 {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'Stack with id <stack-name> does not exist'}}}; got HTTP 200 DescribeStacksResponse with the stack present, StackStatus 'UPDATE_COMPLETE'. Replay also reports 'poll capped at 180s' and 'poll: error code None not in [ValidationError] after 37 attempt(s)'.
3. divergence_key 'service=cloudformation|ListStackResources|status=200->200|keys=$.ListStackResourcesResponse.ListStackResourcesResult.StackResourceSummaries.member[LogicalResourceId="Vec"].ResourceStatusReason', fixture cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket, step task#5, affected_count 1. Detail: ResourceStatusReason missing (expected '<shaped-not-asserted>', i.e. present, got None).
4. divergence_key 'service=cloudformation|DescribeOrganizationsAccess|status=400->200|keys=$.ErrorResponse,status', fixture cloudformation-exports-refactors-and-organizations-read, step task#3, affected_count 1. Expected HTTP 400 ErrorResponse Error.Type 'Sender', Code 'ValidationError', Message 'You must have an organization setup before operating a SERVICE_MANAGED stack set'; got HTTP 200 and no ErrorResponse.
5. divergence_key 'service=cloudformation|DescribeStackResources|status=400->400|keys=$.ErrorResponse.Error.Message', fixture cloudformation-refarch-s3ev-stack-lookup-from-bucket, step task#7, affected_count 2 (also cloudformation-tsrb-describe-resources-filter-mistakes#task#4). Detail: Message value_mismatch — expected 'Either StackName or PhysicalResourceId must be specified.', got 'Cannot specify both StackName and PhysicalResourceId in the same request'.
Stack ids/uuids, generated probe stack names, request ids and timestamps in these records are per-run values filling the templates; the requirements are the statuses, the error codes and the fixed text of the messages.

## Steps to Reproduce
1. Clone this checkout, build the emulator and replay the simparity-wire-aws-v1 wire corpus against it.
2. Cause 1: replay cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket step teardown#9 (or stack-policy-input-validation step teardown#11) — call DeleteStack on the stack the fixture created once its last operation has settled. Expect HTTP 200 and an empty body; observe HTTP 400.
3. Cause 2: continue to step teardown#10 — poll DescribeStacks with that StackName. Expect HTTP 400 ValidationError 'Stack with id <name> does not exist'; observe HTTP 200 still returning the stack (UPDATE_COMPLETE) for the full 180 s poll.
4. Cause 3: replay step task#5 of the same fixture — ListStackResources on the stack whose resource 'Vec' failed to create. Expect the summary for that logical id to carry a non-empty ResourceStatusReason; observe the member absent.
5. Cause 4: replay cloudformation-exports-refactors-and-organizations-read step task#3 — DescribeOrganizationsAccess with no organization configured. Expect HTTP 400 ValidationError 'You must have an organization setup before operating a SERVICE_MANAGED stack set'; observe HTTP 200.
6. Cause 5: replay cloudformation-refarch-s3ev-stack-lookup-from-bucket step task#7 (or cloudformation-tsrb-describe-resources-filter-mistakes step task#4) — DescribeStackResources with both StackName and PhysicalResourceId. Expect HTTP 400 ValidationError 'Either StackName or PhysicalResourceId must be specified.'; observe the reworded message 'Cannot specify both StackName and PhysicalResourceId in the same request'.
