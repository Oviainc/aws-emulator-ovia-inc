Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored|run#29|AssertionError|assert reason == f"The following resource(s) failed to delete: [{_DB}]. ", (
Issue-Commit: 4f4ec5316b812872d2e4beba3bc651533b6eb906

# RDS protected-instance DELETE_FAILED stack: StackStatusReason appends the RDS provider error to AWS's fixed resource-summary sentence (1 cause)

> _Note: this issue was identified against the code base at commit `4f4ec5316b812872d2e4beba3bc651533b6eb906`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One net-new root cause, in the CloudFormation stack DELETE state machine's status-reason composition, reached only AFTER the RDS deletion-protection refusal correctly failed the stack delete.

Related, and NOT this cause: the open issue 20261025T230342Z-cloudwatch-cloudformation-stackstatusreason-for-a-failed-aws-cloudwatch-dashboard.md records the same composition defect on the CREATE path ('The following resource(s) failed to create: [...]. ' with a CloudWatch provider error appended) and is keyed to a cloudwatch check. This entry is the DELETE path of the same state machine ('failed to delete'), reached through an RDS DBInstance with DeletionProtection=true, and the report marked this check neither ALREADY FILED nor OWNED, so the delete-path reason is not covered by that issue's replay key. Also related but distinct: 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md concerns stacks that never reach a *_FAILED state at all; here the stack DOES reach DELETE_FAILED correctly and only the reason text diverges.

Cause 1 of 1: the stack-level StackStatusReason of a DELETE_FAILED stack concatenates the failing resource provider's exception name and message onto AWS's fixed resource-summary sentence, instead of being that sentence alone.
Failing check id: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored.
AWS: recorded behavior only for the TEXT. The Smithy model (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json) types Stack.StackStatusReason as a free-form string documented only as 'Success/failure message associated with the stack status', and types the per-resource diagnosis separately as StackEvent.ResourceStatusReason, 'Success/failure message associated with the resource' — i.e. the two are distinct fields with distinct jobs, and StackEvent.ResourceStatus admits DELETE_FAILED. The underlying refusal IS modeled on the RDS side: Smithy (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json) gives DeleteDBInstance the client error InvalidDBInstanceStateFault (httpError 400, awsQueryError code InvalidDBInstanceState). What the recording pins for the STACK is narrower: a DeleteStack whose only un-deletable resource is a DeletionProtection=true DBInstance settles at StackStatus DELETE_FAILED with StackStatusReason whose FIXED text is 'The following resource(s) failed to delete: [' + ']. ' — the trailing space after the period included — and whose only VARIABLE part is the comma-separated list of the template's own failing LogicalResourceIds (here the single id 'Db', taken from the submitted template, not from this one run). Nothing of the provider's exception code or message appears in the stack-level reason.
Emulator: DescribeStacks reports StackStatus DELETE_FAILED (correct) but StackStatusReason 'The following resource(s) failed to delete: [Db]. InvalidDBInstanceState: DB instance <db-instance-identifier> cant be deleted because deletion protection is enabled' — AWS's fixed summary, correct bracketed logical-id substitution, then the RDS exception code and message appended after the trailing space. The appended segment is text AWS does not emit in this field. (Hypothesis, not asserted by this check and therefore not filed as a separate cause: the appended RDS text itself also looks emulator-authored — real DeleteDBInstance refuses a protected instance with InvalidParameterCombination and the message 'Cannot delete protected DB Instance, please disable deletion protection and try again.', not with an InvalidDBInstanceState/'cant be deleted because deletion protection is enabled' wording. Whoever fixes this cause should check the provider refusal while they are in the path, but the graded requirement here is only the stack-level reason.)
Why: a client that reads the stack-level reason gets a string that does not match the summary form AWS emits. IaC wrappers and deployment tooling parse this field to recover the set of failed logical resource ids from the bracketed list and to classify a stack failure as resource-level; an unbounded provider-error suffix breaks that parse, and with several failed resources it makes the field's content depend on provider error text rather than on the resource list. It also collapses the layering a client depends on: the per-resource cause is supposed to be read from DescribeStackEvents' ResourceStatusReason on the DELETE_FAILED resource event, so folding it into the stack summary lets a consumer that never reads stack events appear to work while one that does see the cause duplicated.
Fix: the CloudFormation stack-delete state machine's status-reason composition (the same component as the create-path defect in the related cloudwatch issue) — the stack-level StackStatusReason must be rendered as the fixed 'The following resource(s) failed to delete: [<comma-separated failing LogicalResourceIds>]. ' sentence with nothing appended, and the provider's refusal (RDS's error code and message) must be carried only on that resource's StackEvent ResourceStatusReason.
Traces: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the built emulator (38/242 passed). This check was marked neither ALREADY FILED nor OWNED by the report.

rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored [mismatch]:
  File "check_rds_deploy_rds_1_protected_stack_delete_blocked_then_snapshot_retain_honored.py", line 107, in run
    assert reason == f"The following resource(s) failed to delete: [{_DB}]. ", (
  AssertionError: StackStatusReason does not name the protected DBInstance [Db]: 'The following resource(s) failed to delete: [Db]. InvalidDBInstanceState: DB instance sim2real-rds-dp-8aac53087311-db-4b0fa165c33d cant be deleted because deletion protection is enabled'

Expected (AWS, recorded): StackStatusReason == 'The following resource(s) failed to delete: [Db]. ' — fixed text 'The following resource(s) failed to delete: [' + ']. ' (trailing space included); variable part = the failing LogicalResourceIds from the submitted template ('Db' here).
Actual (emulator): the same string with 'InvalidDBInstanceState: DB instance <instance-id> cant be deleted because deletion protection is enabled' appended. The instance identifier in that suffix is a per-run generated name and is not itself part of any requirement.

Legs that PASSED earlier in the same check, and must keep passing:
  - cloudformation:CreateStack of the deletion-policy template reaches CREATE_COMPLETE; DescribeStacks masks the NoEcho parameter DbPassword as '****'.
  - DescribeStackResources resolves the Db and Pg physical resource ids; rds:DescribeDBInstances reports DBInstanceStatus 'available', DeletionProtection true, and exactly the stack's parameter group.
  - cloudformation:DeleteStack on the protected stack does settle at StackStatus DELETE_FAILED (the state transition is right; only the reason text is wrong).

Legs NOT reached because the assertion above aborted the check (they remain ungraded):
  - A DELETE_FAILED StackEvent for LogicalResourceId 'Db' whose PhysicalResourceId is the instance id and whose ResourceStatusReason cites deletion protection — this is where the provider error belongs.
  - The instance staying 'available' throughout the refused delete with DeletionProtection unchanged, and every other stack resource remaining CREATE_COMPLETE while Db is DELETE_FAILED.
  - cloudformation:UpdateStack on a DELETE_FAILED stack refused with ValidationError, message 'Stack:<stack-id> is in DELETE_FAILED state and can not be updated.'
  - After lifting protection with rds:ModifyDBInstance, the stack reaching DELETE_COMPLETE; DescribeDBInstances answering DBInstanceNotFound with message 'DBInstance <identifier> not found.'; DeletionPolicy Snapshot leaving an available manual snapshot whose id starts with the stack name and whose Engine is 'mysql'; DeletionPolicy Retain leaving the DBParameterGroup with the template's max_connections=77 under Source='user'.

## Steps to Reproduce
1. Boot the emulator and point botocore 'rds', 'cloudformation' and 'ec2' clients at it.
2. cloudformation:CreateStack(StackName=<fresh>, TemplateBody=<VPC + two subnets + SG + DBSubnetGroup + AWS::RDS::DBParameterGroup (logical id 'Pg', family from DescribeDBEngineVersions, max_connections=77, DeletionPolicy Retain) + AWS::RDS::DBInstance (logical id 'Db', engine mysql, DeletionProtection true, DeletionPolicy Snapshot)>, Parameters=[AzA, AzB, EngineVersion, Family, DbPassword(NoEcho)]). Wait for CREATE_COMPLETE.
3. Confirm rds:DescribeDBInstances on the Db physical id reports DBInstanceStatus 'available' and DeletionProtection true.
4. cloudformation:DeleteStack(StackName=<stack id>) and poll cloudformation:DescribeStacks until terminal.
   Expected: StackStatus 'DELETE_FAILED' and StackStatusReason exactly 'The following resource(s) failed to delete: [Db]. ' — nothing after the trailing space.
   Observed: StackStatus 'DELETE_FAILED' but StackStatusReason 'The following resource(s) failed to delete: [Db]. InvalidDBInstanceState: DB instance <instance-id> cant be deleted because deletion protection is enabled'.
5. cloudformation:DescribeStackEvents(StackName=<stack id>): the provider refusal must appear there, on the 'Db' DELETE_FAILED event's ResourceStatusReason (with that event's PhysicalResourceId equal to the instance id), and must NOT be folded into the stack-level reason.
6. Then continue the flow to re-expose the ungraded legs: UpdateStack must be refused ValidationError 'Stack:<stack-id> is in DELETE_FAILED state and can not be updated.'; rds:ModifyDBInstance(DeletionProtection=false, ApplyImmediately=true); DeleteStack again must reach DELETE_COMPLETE, leaving a manual snapshot (id prefixed with the stack name, Engine 'mysql') and the retained DBParameterGroup carrying max_connections=77.
7. Re-replay to confirm the fix: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored.
