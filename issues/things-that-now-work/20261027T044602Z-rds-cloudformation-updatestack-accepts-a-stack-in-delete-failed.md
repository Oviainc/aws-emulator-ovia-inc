Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored|_error#43|AssertionError|raise AssertionError("call unexpectedly succeeded; real AWS rejects it")
Issue-Commit: 11fd98189f90040dc28179001c4adbbf7cfc6f77

# cloudformation UpdateStack accepts a stack in DELETE_FAILED state instead of refusing it with ValidationError

> _Note: this issue was identified against the code base at commit `11fd98189f90040dc28179001c4adbbf7cfc6f77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
One net-new root cause, found while replaying the RDS deletion-policy deploy check. (The check's service token is `rds`, so this issue is filed under `rds`; the defective component is the CloudFormation UpdateStack handler, which the check drives through boto3's cloudformation client.)

Cause 1 of 1: UpdateStack on a stack whose StackStatus is DELETE_FAILED is accepted by the emulator; real CloudFormation refuses it.

AWS: CloudFormation's stack state machine does not allow an update out of a terminal failed-delete state. A stack in DELETE_FAILED must be re-deleted (or its blocking resource fixed out-of-band); UpdateStack against it is rejected synchronously. Smithy: UpdateStack (com-amazonaws-cloudformation-cloudformation) models only InsufficientCapabilitiesException and TokenAlreadyExistsException, so the state refusal is the awsQuery common error `ValidationError` at HTTP 400 — it is not an operation-specific modeled shape. Measured behavior recorded with the check (real AWS, us-west-2): the call fails with Error.Code `ValidationError` and Error.Message
  Stack:<stack-id> is in DELETE_FAILED state and can not be updated.
Message template: the fixed text is `Stack:`, ` is in `, and ` state and can not be updated.`; the variable parts are the stack identity (the stack ARN/StackId the request resolved, taken from the request and the emulator's state — NOT the literal id from any one recording) and the state name, which is the stack's current StackStatus (`DELETE_FAILED` here). Note the AWS wording is `can not` (two words), and there is no space after `Stack:`.

Emulator: UpdateStack returns success (a 200 with a StackId) for the same stack. The check's `_error()` helper raised `AssertionError: call unexpectedly succeeded; real AWS rejects it` at the UpdateStack call, i.e. no ClientError at all was produced. Everything the check asserted before this point passed: the protected delete did end DELETE_FAILED with StackStatusReason naming the DBInstance, the DELETE_FAILED resource event cited deletion protection, and the instance stayed `available` with DeletionProtection true — so the gap is specifically the missing precondition check on UpdateStack, not the delete path.

Why: a client that recovers a protected stack relies on this refusal to know the stack cannot be moved forward through CloudFormation and that protection must be lifted on the resource itself (ModifyDBInstance). With the update silently accepted, the emulator leads a caller to believe the DELETE_FAILED stack was updated: the stack's status and its resources' real configuration diverge from the template the caller just submitted, deletion protection is left in whatever state the accepted update implies, and any retry/rollback logic keyed on ValidationError never runs. Downstream assertions about the post-DELETE_FAILED recovery sequence (lift protection, re-delete, snapshot/retain artifacts) are never reached.

Fix: the UpdateStack handler's precondition validation in the CloudFormation service crate (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/update_stack.rs) — before mutating stack state it must read the stack's current StackStatus and reject the non-updatable terminal statuses (DELETE_FAILED among them) with a 400 `ValidationError` carrying the message template above, leaving the stack's status and its resources untouched.

Related open issues that do NOT cover this (no stack-state precondition on UpdateStack): 20261025T015306Z-cloudformation-refusal-taxonomy-diverges-wrong-not-found-validation.md and 20261025T221744Z-cloudformation-3-net-new-root-causes-stack-policies.md.

Traces: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored (check_rds_deploy_rds_1_protected_stack_delete_blocked_then_snapshot_retain_honored.py line 141).

## Evidence
Expected (real AWS, measured while authoring the check): UpdateStack(StackName=<stack-id in DELETE_FAILED>, TemplateBody=<same template with DeletionProtection=false>, Parameters=[UsePreviousValue]) -> HTTP 400, Error.Code = "ValidationError", Error.Message = "Stack:<stack-id> is in DELETE_FAILED state and can not be updated." The stack remains DELETE_FAILED and the DBInstance keeps DeletionProtection=true.

Actual (emulator): the same UpdateStack call returns successfully — no ClientError is raised. Replay output:
  File "check_rds_deploy_rds_1_protected_stack_delete_blocked_then_snapshot_retain_honored.py", line 141, in run
    code, msg = _error(lambda: cfn.update_stack(...))
  File "check_rds_deploy_rds_1_protected_stack_delete_blocked_then_snapshot_retain_honored.py", line 274, in _error
    raise AssertionError("call unexpectedly succeeded; real AWS rejects it")
  AssertionError: call unexpectedly succeeded; real AWS rejects it

Smithy (com-amazonaws-cloudformation-cloudformation, UpdateStack): errors = InsufficientCapabilitiesException, TokenAlreadyExistsException only; the state refusal is the awsQuery common error ValidationError / 400.

## Steps to Reproduce
1. CreateStack with a template containing AWS::EC2::VPC + two Subnets + SecurityGroup + AWS::RDS::DBSubnetGroup + AWS::RDS::DBParameterGroup (DeletionPolicy Retain) + AWS::RDS::DBInstance (Engine mysql, DeletionPolicy Snapshot, DeletionProtection=true). Wait for CREATE_COMPLETE.
2. DeleteStack on that stack and wait until DescribeStacks reports StackStatus=DELETE_FAILED (the protected DBInstance refuses deletion).
3. Call UpdateStack on the same stack id with the same template but DeletionProtection=false and Parameters=[{ParameterKey: <each>, UsePreviousValue: true}].
4. Observe: the emulator returns 200 with a StackId. Real AWS returns HTTP 400 ValidationError "Stack:<stack-id> is in DELETE_FAILED state and can not be updated." and leaves the stack in DELETE_FAILED with DeletionProtection still true.
