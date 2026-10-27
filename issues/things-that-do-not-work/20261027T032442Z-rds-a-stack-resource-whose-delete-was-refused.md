Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored|run#41|AssertionError|assert after[_DB]["ResourceStatus"] == "DELETE_FAILED", (
Issue-Commit: 70d60110d97a6c21bc790d7df8d6a76c7ca8ef4a

# rds: a stack resource whose delete was refused keeps its old ResourceStatus in DescribeStackResources — the event stream records DELETE_FAILED but the resource view still reports CREATE_COMPLETE (1 cause)

> _Note: this issue was identified against the code base at commit `70d60110d97a6c21bc790d7df8d6a76c7ca8ef4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResources

## Description
One net-new root cause in CloudFormation's per-resource status bookkeeping, reached only AFTER the whole deletion-protection refusal path already behaved correctly.

Related, and NOT this cause: open issue 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md (its Cause 3 says a FAILED stack lists NO resources at all, and its Cause 1 says there are no per-resource events at all) — neither is the signature here: the stack DOES list all of its resources and the per-resource DELETE_FAILED event for the DBInstance IS present with the right PhysicalResourceId and a reason citing deletion protection. What diverges is only that the listed resource's own ResourceStatus was never advanced. Open issues 20261025T050651Z-cloudformation-records-no-per-resource-stack-events-a.md and 20261025T053459Z-cloudformation-emits-no-per-resource-stack-events-and.md likewise assert the events are absent, which they are not here. No open issue carries this check's replay key, and the report marked it neither ALREADY FILED nor OWNED.

Cause 1 of 1: when DeleteStack fails on one resource, that resource's ResourceStatus in DescribeStackResources is not advanced to DELETE_FAILED; it stays at whatever it was before the delete was attempted (CREATE_COMPLETE).
Failing check id: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored.
AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): StackResource.ResourceStatus is a REQUIRED member documented 'Current status of the resource', targeting the ResourceStatus enum whose members include DELETE_IN_PROGRESS, DELETE_FAILED, DELETE_COMPLETE and DELETE_SKIPPED alongside CREATE_COMPLETE — the delete outcome of an individual resource is a first-class, modeled resource status, not an event-log-only detail. StackResource also carries ResourceStatusReason ('Success/failure message associated with the resource'). Recorded behavior pins the join: when an AWS::RDS::DBInstance with DeletionProtection=true is deleted through its stack, RDS refuses the delete, the stack settles at DELETE_FAILED with StackStatusReason 'The following resource(s) failed to delete: [<LogicalResourceId>]. ', and DescribeStackResources reports that one resource at ResourceStatus DELETE_FAILED while every resource it depends on remains CREATE_COMPLETE. The two views agree: the resource status the event stream last recorded is the status the resource view reports.
Emulator: the event stream is right — DescribeStackEvents contains a DELETE_FAILED event for logical id 'Db' with the correct PhysicalResourceId and a ResourceStatusReason citing deletion protection, the stack is DELETE_FAILED, and StackStatusReason names [Db]. But DescribeStackResources on the same stack reports logical id 'Db' with ResourceStatus 'CREATE_COMPLETE'. Replay: AssertionError: Db resource status 'CREATE_COMPLETE', expected DELETE_FAILED.
Why: DescribeStackResources is the view a client uses to decide what to do with a stack stuck in DELETE_FAILED — which resources to pass to DeleteStack's RetainResources, which to repair by hand. A resource view that still claims CREATE_COMPLETE tells the client nothing failed, so the failing resource cannot be identified from the resource list at all and tooling must fall back to parsing the event stream. It also makes the two views contradict each other for the same resource at the same instant, which no AWS client is written to tolerate.
Fix: the CloudFormation delete state machine's per-resource status bookkeeping — the same transition that appends the resource's DELETE_IN_PROGRESS/DELETE_FAILED stack event must write that status (and its reason) onto the stored stack-resource record that DescribeStackResources / DescribeStackResource / ListStackResources serve, so the resource view and the event stream cannot disagree.
Traces: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the built emulator (41/242 passed). This check was marked neither ALREADY FILED nor OWNED by the report.

rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored [mismatch]:
  File "check_rds_deploy_rds_1_protected_stack_delete_blocked_then_snapshot_retain_honored.py", line 133, in run
    assert after[_DB]["ResourceStatus"] == "DELETE_FAILED", (
  AssertionError: Db resource status 'CREATE_COMPLETE', expected DELETE_FAILED

Expected (AWS): DescribeStackResources(StackName=<the DELETE_FAILED stack>) reports the protected AWS::RDS::DBInstance at ResourceStatus 'DELETE_FAILED' (a modeled ResourceStatus enum member), with every other resource still 'CREATE_COMPLETE'.
Actual (emulator): the same call reports that resource at ResourceStatus 'CREATE_COMPLETE', contradicting the stack's own DELETE_FAILED event for the identical logical id.

Legs that PASSED earlier in the same check and must keep passing:
  - CreateStack of the VPC/Subnet/SecurityGroup/DBSubnetGroup/DBParameterGroup/DBInstance stack reaches CREATE_COMPLETE; the NoEcho parameter is masked as '****'.
  - The instance is 'available' with DeletionProtection true and uses the stack's parameter group.
  - DeleteStack on the protected stack settles at StackStatus DELETE_FAILED with StackStatusReason exactly 'The following resource(s) failed to delete: [Db]. ' (fixed text plus the template's logical id).
  - DescribeStackEvents contains a DELETE_FAILED event for LogicalResourceId 'Db' whose PhysicalResourceId is the DB instance id and whose ResourceStatusReason cites deletion protection.
  - The instance never leaves 'available' and DeletionProtection is not changed by the refused delete.
So the refusal and the event stream are correct; only the resource view's status is stale.

## Steps to Reproduce
1. Boot the emulator and point botocore 'cloudformation', 'rds' and 'ec2' clients at it.
2. cloudformation:CreateStack with a template containing an AWS::EC2::VPC, two AWS::EC2::Subnet, an AWS::EC2::SecurityGroup, an AWS::RDS::DBSubnetGroup, an AWS::RDS::DBParameterGroup (DeletionPolicy Retain) and an AWS::RDS::DBInstance named 'Db' (DeletionPolicy Snapshot, Engine mysql, DeletionProtection true). Wait for CREATE_COMPLETE.
3. cloudformation:DeleteStack(StackName=<stack id>) and poll DescribeStacks until StackStatus is DELETE_FAILED (it is, with StackStatusReason 'The following resource(s) failed to delete: [Db]. ').
4. cloudformation:DescribeStackEvents(StackName=<stack id>) — a DELETE_FAILED event for LogicalResourceId 'Db' is present, with the DB instance id as PhysicalResourceId and a reason citing deletion protection. (Correct.)
5. cloudformation:DescribeStackResources(StackName=<stack id>) and read the entry whose LogicalResourceId is 'Db'.
   Expected: ResourceStatus 'DELETE_FAILED' (and a ResourceStatusReason citing deletion protection), with every other logical id still 'CREATE_COMPLETE'.
   Observed: ResourceStatus 'CREATE_COMPLETE'.
6. Re-replay to confirm the fix: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored.
