Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back|run#24|AssertionError|assert st == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Commit: 509002bb1fa78c9960f25791a050a4a75c2a7129

# rds: a stack UPDATE whose AWS::RDS::DBParameterGroup update is refused parks terminally in UPDATE_FAILED instead of rolling the stack back to UPDATE_ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `509002bb1fa78c9960f25791a050a4a75c2a7129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1 — an UpdateStack that fails on a resource refusal never performs the automatic update rollback: the stack settles in the terminal status UPDATE_FAILED, so no UPDATE_ROLLBACK_IN_PROGRESS / UPDATE_ROLLBACK_COMPLETE transition ever happens and the rolled-back-state contract of the deploy flow is unobservable.
Failing check: rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back

AWS: Smithy (cloudformation): StackStatus is an enum containing both UPDATE_FAILED and UPDATE_ROLLBACK_IN_PROGRESS / UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS / UPDATE_ROLLBACK_COMPLETE; UpdateStack's input carries the opt-in DisableRollback member. AWS uses the stack-level UPDATE_FAILED park state only when rollback has been disabled for that update (DisableRollback=true / a rollback-disabled stack), which is the state ContinueUpdateRollback then acts on. With rollback at its default (the recorded UpdateStack sends no DisableRollback), a resource whose update is refused drives the stack UPDATE_IN_PROGRESS -> UPDATE_ROLLBACK_IN_PROGRESS -> UPDATE_ROLLBACK_COMPLETE, emits exactly one UPDATE_FAILED *resource* event for the failing logical id whose ResourceStatusReason wraps the provider's refusal message, and restores every resource to its last accepted value. Recorded behavior for the resource leg: the DBParameterGroup update is refused by RDS with error code InvalidParameterValue and the message "Value: <value> is outside of range: <AllowedValues> for parameter: <parameterName>" — the fixed text is "Value: ", " is outside of range: ", " for parameter: "; the variable parts are the rejected ParameterValue from the request, the AllowedValues string DescribeEngineDefaultParameters reports for that DBParameterGroupFamily, and the ParameterName from the request (the check's recording exercises innodb_lock_wait_timeout one below its lower bound; those particular numbers are only the template filled in, not the requirement). Smithy (rds): ModifyDBParameterGroup models DBParameterGroupNotFoundFault and InvalidDBParameterGroupStateFault only, so InvalidParameterValue comes from the service-common error set.
Emulator: the stack create (CREATE_COMPLETE) and the in-range update (UPDATE_COMPLETE) both succeed, and the out-of-range update is indeed failed — but the stack's terminal status is 'UPDATE_FAILED'. The replay asserts the terminal status after the failing update and gets: "AssertionError: update to out-of-range innodb_lock_wait_timeout=0 ended 'UPDATE_FAILED', expected UPDATE_ROLLBACK_COMPLETE". Because the check stops at that assertion, the downstream requirements it guards were never reached and are not separately claimed here: exactly one DbParams UPDATE_FAILED resource event carrying the RDS refusal in ResourceStatusReason, the parameter still holding the last accepted value after rollback, and the direct ModifyDBParameterGroup refusal code/message.
Why: a client that deploys through CloudFormation relies on a failed update being automatically undone. Parking in UPDATE_FAILED means (a) waiters and CI gates that treat UPDATE_ROLLBACK_COMPLETE as the failure terminal never see it and either hang or mis-classify the outcome, (b) the stack is left in a mixed, half-updated state with no indication that any restore happened, and (c) the only AWS exit from UPDATE_FAILED is ContinueUpdateRollback/RollbackStack, which a client that never asked for DisableRollback has no reason to call — so the stack is effectively stranded.
Hypothesis: the update workflow treats a resource-level failure as the stack's terminal outcome rather than entering the rollback stage; neither the model nor the recording states whether DisableRollback is being defaulted to true or whether the rollback stage simply is not wired for UPDATE.
Fix: the cloudformation crate's UpdateStack workflow — the stage that must, when rollback is not disabled, transition the stack into UPDATE_ROLLBACK_IN_PROGRESS on the first failed resource update, restore the previously accepted resource state (here via the RDS DBParameterGroup provider), and land the stack in UPDATE_ROLLBACK_COMPLETE, recording the resource-level UPDATE_FAILED event with the provider's refusal as ResourceStatusReason. Related but not covering: the open issue 20261026T092336Z-cloudformation-rds-ssm-a-resource-that-starts-provisioning.md concerns missing CREATE_IN_PROGRESS resource events, and the resolved 20261026T235900Z-cloudformation-update-rollback-failed-recovery-never-converges-the.md concerns the UPDATE_ROLLBACK_FAILED recovery path reached only after a rollback has begun; neither covers an update that never enters rollback at all.
Traces: rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back

## Evidence
Check rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back [mismatch]:

  File "check_rds_deploy_cfn_param_group_out_of_range_update_rejected_rolls_back.py", line 69, in run
    assert st == "UPDATE_ROLLBACK_COMPLETE", (
  AssertionError: update to out-of-range innodb_lock_wait_timeout=0 ended 'UPDATE_FAILED', expected UPDATE_ROLLBACK_COMPLETE

Expected (AWS): after the refused UpdateStack the stack's terminal StackStatus is UPDATE_ROLLBACK_COMPLETE (reached via UPDATE_ROLLBACK_IN_PROGRESS), with exactly one UPDATE_FAILED event for LogicalResourceId 'DbParams' whose ResourceStatusReason contains the RDS refusal message "Value: <rejected value> is outside of range: <AllowedValues> for parameter: <parameterName>", and DescribeDBParameters(Source=user) still reporting the last accepted value.
Actual (emulator): terminal StackStatus is UPDATE_FAILED; no update-rollback transition is produced. The preceding legs of the same check pass (CREATE_COMPLETE, then UPDATE_COMPLETE with the in-range value), so the divergence is isolated to the rollback of a failed update.

Note on variable parts: 'innodb_lock_wait_timeout', the value 0 and the AllowedValues range in the recorded run are the request/engine-default values filling the message template; the requirement is the status transition and the message's fixed text with the request's own parameter name, rejected value and the family's AllowedValues.

## Steps to Reproduce
1. DescribeDBEngineVersions(Engine=mysql, DefaultOnly=true) to get the default DBParameterGroupFamily, then DescribeEngineDefaultParameters for that family to read innodb_lock_wait_timeout's AllowedValues (lower bound L).
2. CreateStack with a template containing one AWS::RDS::DBParameterGroup whose Parameters set innodb_lock_wait_timeout from a stack parameter; create it with an in-range value (e.g. 120). Wait for CREATE_COMPLETE and confirm DescribeDBParameters(Source=user) reports 120.
3. UpdateStack (UsePreviousTemplate, no DisableRollback) with another in-range value (e.g. 300). Wait: UPDATE_COMPLETE, parameter = 300.
4. UpdateStack with the out-of-range value L-1 and poll DescribeStacks for the terminal status.
5. Observe StackStatus = UPDATE_FAILED. AWS reaches UPDATE_ROLLBACK_COMPLETE, emits one 'DbParams' UPDATE_FAILED resource event whose ResourceStatusReason wraps RDS's "Value: <L-1> is outside of range: <AllowedValues> for parameter: innodb_lock_wait_timeout", and leaves the parameter at 300.
