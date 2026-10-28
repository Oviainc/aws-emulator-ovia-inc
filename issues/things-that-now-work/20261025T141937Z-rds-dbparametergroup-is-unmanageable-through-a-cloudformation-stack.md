Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back|run#19|AssertionError|assert st == "UPDATE_COMPLETE", f"in-range update ended {st!r}"
Issue-Commit: ac48ac6b523870bf93baa2e503ea241ac14f12f9

# RDS DBParameterGroup is unmanageable through a CloudFormation stack UPDATE: an in-range parameter change ends UPDATE_FAILED instead of UPDATE_COMPLETE (1 cause)

> _Note: this issue was identified against the code base at commit `ac48ac6b523870bf93baa2e503ea241ac14f12f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBParameterGroup

## Description
Cause 1 of 1 — a CloudFormation stack UPDATE that changes a declared AWS::RDS::DBParameterGroup parameter to a value inside the engine's published AllowedValues is not applied: the stack reaches a terminal UPDATE_FAILED (and the resource/rollback state machine never reaches UPDATE_COMPLETE), even though the stack's CREATE of the very same resource succeeded and wrote the value the template asked for.

Related but NOT covering issues, named here so this is not closed as a duplicate:
- 20261025T122143Z-rds-db-parameter-groups-a-cloudformation-created-aws.md (open). Its Cause 1 is the CREATE path — a CFN-created AWS::RDS::DBParameterGroup was absent from RDS, so DescribeDBParameters on the physical id answered 404 DBParameterGroupNotFound. That symptom is GONE in this replay: the stack reached CREATE_COMPLETE, DescribeDBParameters(Source="user") on the physical id answered 200, and innodb_lock_wait_timeout read back as the template's "120". The failure has moved one step later, to the UPDATE path, which is a distinct provisioning path and a distinct root cause.
- 20261025T073911Z-events-lambda-rds-sqs-rds-modifydbparametergroup-is-an.md (fixed). That covered ModifyDBParameterGroup answering NotImplemented as a scaffold stub. It is not re-opened here: the operation clearly does apply a value now (the create-time Parameters landed), and this replay produces no NotImplemented refusal — the evidence stops at the stack verdict.

AWS: CloudFormation updates AWS::RDS::DBParameterGroup in place when only Properties.Parameters changes — it calls RDS ModifyDBParameterGroup (Smithy, services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json: ModifyDBParameterGroup takes required DBParameterGroupName and required Parameters, returns DBParameterGroupNameMessage{DBParameterGroupName} on HTTP 200; its only modeled client errors are DBParameterGroupNotFoundFault / awsQueryError DBParameterGroupNotFound 404 and InvalidDBParameterGroupStateFault / awsQueryError InvalidDBParameterGroupState 400). A value inside the parameter's published AllowedValues is accepted, so the resource goes UPDATE_IN_PROGRESS -> UPDATE_COMPLETE, the stack goes UPDATE_IN_PROGRESS -> UPDATE_COMPLETE, and DescribeDBParameters(DBParameterGroupName=<physical id>, Source="user") then reports the new value. Only an out-of-domain value is refused, with InvalidParameterValue and the message "Value: <value> is outside of range: <AllowedValues> for parameter: <parameter>" — fixed text "Value: ", " is outside of range: ", " for parameter: ", with the submitted value, the parameter's published AllowedValues range and the parameter name taken from the request and the group's catalogue; that refusal is what makes the stack roll back to UPDATE_ROLLBACK_COMPLETE with exactly one UPDATE_FAILED resource event whose ResourceStatusReason wraps the RDS message.
Emulator: after a successful create of the same stack (CREATE_COMPLETE, user value = the template value), UpdateStack with UsePreviousTemplate and only the parameter's value changed to another in-range value settles in the terminal state UPDATE_FAILED. The accepted, in-range change is therefore treated as a failure rather than applied, so the resource is never updated through IaC at all. Hypothesis: the CloudFormation provisioner for AWS::RDS::DBParameterGroup has no working in-place update path (no ModifyDBParameterGroup call on a Properties.Parameters delta, or the resource is looked up by logical rather than physical id on update) — the replay only observes the stack verdict, not the handler's own error.
Why: parameter groups are normally managed through IaC, and a client that changes one value in a template gets a stack it cannot move forward: every subsequent update of that stack is blocked by the failed update, and the parameter keeps its old value with no RDS-side explanation. Worse, because the in-range (positive-control) update fails, the entire refusal-and-rollback contract on top of it is unobservable: a client cannot tell an out-of-domain value (which AWS refuses with InvalidParameterValue and rolls back to UPDATE_ROLLBACK_COMPLETE, leaving the last ACCEPTED value in place) from an in-domain value, since both end UPDATE_FAILED. The emulator's UPDATE_FAILED is also the wrong terminal state for a refused update — AWS ends such a stack at UPDATE_ROLLBACK_COMPLETE with one UPDATE_FAILED *resource* event.
Fix: the CloudFormation stack-resource provisioner's UPDATE path for AWS::RDS::DBParameterGroup (services/service-com-amazonaws-cloudformation-cloudformation) — a Properties.Parameters delta must call RDS ModifyDBParameterGroup against the shared RDS state using the resource's physical id (the real DBParameterGroupName), drive the resource UPDATE_IN_PROGRESS -> UPDATE_COMPLETE on a 200, and on an RDS refusal mark the resource UPDATE_FAILED with the RDS message in ResourceStatusReason and take the stack to UPDATE_ROLLBACK_COMPLETE with the prior accepted value restored; the RDS side is services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_db_parameter_group.rs, which must accept an in-range value and refuse an out-of-domain one with InvalidParameterValue and AWS's message template.
Traces: rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back.

## Evidence
Failing check rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back, verdict mismatch:

  File "check_rds_deploy_cfn_param_group_out_of_range_update_rejected_rolls_back.py", line 67, in run
    assert st == "UPDATE_COMPLETE", f"in-range update ended {st!r}"
  AssertionError: in-range update ended 'UPDATE_FAILED'

What succeeded before that point in this same replay (so these are NOT the divergence): describe_db_engine_versions(Engine=mysql, DefaultOnly) returned a default DBParameterGroupFamily; describe_engine_default_parameters for that family published innodb_lock_wait_timeout with an AllowedValues range; create_stack of a single-resource AWS::RDS::DBParameterGroup template (Parameters: {innodb_lock_wait_timeout: {"Ref": "LockWait"}} = "120") reached CREATE_COMPLETE; the stack Output GroupName resolved; and describe_db_parameters(DBParameterGroupName=<that output>, Source="user") reported innodb_lock_wait_timeout = "120".

The diverging step: update_stack(StackName=<stack>, UsePreviousTemplate=True, Parameters=[{LockWait: "300"}]) — an in-range value, the check's positive control — polled to the terminal state 'UPDATE_FAILED'.
Expected (AWS): stack status 'UPDATE_COMPLETE', then describe_db_parameters(..., Source="user") reporting innodb_lock_wait_timeout = "300".
Actual (emulator): stack status 'UPDATE_FAILED'.

Because the positive control fails, the rest of the check is unreachable and ungraded: the out-of-range update (value = lower bound - 1) ending UPDATE_ROLLBACK_COMPLETE with exactly one DbParams UPDATE_FAILED event whose ResourceStatusReason carries "Value: <bad> is outside of range: <AllowedValues> for parameter: innodb_lock_wait_timeout"; the rolled-back group still holding the last accepted value; and the direct ModifyDBParameterGroup(<bad>) call being refused with code InvalidParameterValue and exactly that message.

## Steps to Reproduce
1. rds describe-db-engine-versions --engine mysql --default-only -> note DBParameterGroupFamily.
2. rds describe-engine-default-parameters --db-parameter-group-family <family> -> note innodb_lock_wait_timeout's AllowedValues range and its lower bound.
3. cloudformation create-stack --stack-name s --template-body <single resource AWS::RDS::DBParameterGroup with Family=<family>, Properties.Parameters={innodb_lock_wait_timeout: {"Ref":"LockWait"}}, Outputs.GroupName={"Ref":"<logical id>"}> --parameters ParameterKey=LockWait,ParameterValue=120 ; wait for CREATE_COMPLETE.
4. rds describe-db-parameters --db-parameter-group-name <GroupName output> --source user -> innodb_lock_wait_timeout = "120" (this works today).
5. cloudformation update-stack --stack-name s --use-previous-template --parameters ParameterKey=LockWait,ParameterValue=300 ; poll describe-stacks until the status stops ending in _IN_PROGRESS.
   - AWS: UPDATE_COMPLETE, and step 4 repeated reports "300".
   - Emulator: UPDATE_FAILED; the value is never changed.
6. (Unreachable today, also required) update-stack with ParameterValue=<lower bound - 1> -> AWS ends the stack UPDATE_ROLLBACK_COMPLETE with one <logical id> UPDATE_FAILED event whose ResourceStatusReason carries "Value: <that value> is outside of range: <AllowedValues> for parameter: innodb_lock_wait_timeout", the group keeps "300", and a direct rds modify-db-parameter-group with that value is refused with code InvalidParameterValue and that same message.
