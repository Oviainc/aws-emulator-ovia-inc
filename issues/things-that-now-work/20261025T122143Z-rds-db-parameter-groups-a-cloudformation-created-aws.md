Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-rds-instance-cfn-parameter-group-effective rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back rds.paramgroup-out-of-range-value-rejected-engine-setting-unchanged
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBParameters|DBParameterGroupNotFound
Issue-Key: rds.paramgroup-out-of-range-value-rejected-engine-setting-unchanged|run#8|AssertionError|assert declared.get("ApplyType") == "dynamic", (
Issue-Commit: e6eec4415380d0861b3cd74514fe77af8743d21c

# RDS DB parameter groups: a CloudFormation-created AWS::RDS::DBParameterGroup is absent from RDS, and a new group's DescribeDBParameters omits the engine-default parameter catalogue (2 causes)

> _Note: this issue was identified against the code base at commit `e6eec4415380d0861b3cd74514fe77af8743d21c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBParameters

## Description
Two net-new root causes in the RDS DB-parameter-group surface, both observed through DescribeDBParameters. Related but NOT covering issues, named here so they are not closed as duplicates: 20261024T120544Z-rds-describedbparameters-stub.md (DescribeDBParameters filed as an unimplemented stub — the operation now answers, so neither cause below is that stub), 20261024T030130Z-rds-s3-rds-snapshot-export-to-s3-and-restore-from.md (same class of defect for a CloudFormation-created DBCLUSTERParameterGroup, under service token rds__s3), and 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md (CloudFormation reporting CREATE_COMPLETE for resources never created, filed under cloudformation).

Cause 1 of 2 — a CloudFormation stack that declares AWS::RDS::DBParameterGroup reaches CREATE_COMPLETE and hands back a physical resource id, but no DB parameter group of that name exists in RDS.
AWS: CloudFormation provisions AWS::RDS::DBParameterGroup by calling RDS CreateDBParameterGroup (Smithy: CreateDBParameterGroup requires DBParameterGroupName, DBParameterGroupFamily, Description; returns DBParameterGroup with DBParameterGroupArn) and, when Properties.Parameters is present, ModifyDBParameterGroup. The resource's physical id — the value !Ref returns and the stack output carries — is the real DBParameterGroupName, so every RDS read of that name succeeds: DescribeDBParameters(DBParameterGroupName=<physical id>) returns the group's parameters (Smithy: DescribeDBParameters, sole modeled client error DBParameterGroupNotFoundFault / awsQuery code DBParameterGroupNotFound, HTTP 404), and a DBInstance that references it via DBParameterGroupName lists it in DBInstance.DBParameterGroups.
Emulator: the stack create completes and the output/!Ref yields a generated name of the form <stack-name>-<logical-id>-<suffix>, but RDS has no such group: DescribeDBParameters on that exact name is refused with DBParameterGroupNotFound and the message "DBParameterGroup not found: <name>". The CloudFormation resource is therefore a bookkeeping entry only — the RDS-side resource it stands for was never created (and the template's Properties.Parameters were never written to any group).
Why: a client that deploys parameter groups through IaC — the normal way they are managed — gets a stack that claims success and a group name that nothing in RDS recognises. Every follow-on call fails 404: reading the effective parameters, attaching the group to a DBInstance, updating the value through a stack update, or asserting the rollback value after a refused update. Nothing in the stack's state tells the client the resource is missing.
Fix: the CloudFormation resource provisioner for AWS::RDS::DBParameterGroup — the create/update/delete path must call RDS's own CreateDBParameterGroup / ModifyDBParameterGroup / DeleteDBParameterGroup against the shared RDS state and use the created group's DBParameterGroupName as the physical resource id (CloudFormation stack-resource provisioning, services/service-com-amazonaws-cloudformation-cloudformation; RDS side services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_parameter_group.rs).
Traces: rds.deploy-rds-instance-cfn-parameter-group-effective, rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back.

Cause 2 of 2 — DescribeDBParameters on a freshly created parameter group does not return the engine family's default parameter catalogue, so a parameter's declared metadata (ApplyType, AllowedValues, DataType) is unreadable.
AWS: DescribeDBParameters with no Source returns every parameter the engine family publishes, each a Parameter structure (Smithy: Parameter members ParameterName, ParameterValue, Description, Source, ApplyType, DataType, AllowedValues, IsModifiable, MinimumEngineVersion, ApplyMethod, SupportedEngineModes). For a group just created in family postgres17, the postgres GUC from_collapse_limit is present with Source="engine-default", ApplyType="dynamic", DataType="integer" and an AllowedValues range string, i.e. the declared domain a client reads before writing a value. Those fields are what ModifyDBParameterGroup validates a write against.
Emulator: DescribeDBParameters(DBParameterGroupName=<new postgres17 group>) returns a catalogue in which from_collapse_limit carries no ApplyType at all — the check reads ApplyType as None (the parameter is absent from the returned map, or present without the member). AllowedValues is likewise unavailable, so the parameter's published domain cannot be read.
Why: a client cannot discover what a parameter accepts, whether it is dynamic or static, or what its default is — the only documented way to do so is this call. It also makes range validation unverifiable end to end: without AllowedValues there is no published domain for ModifyDBParameterGroup to refuse a value against (AWS refuses an out-of-domain value with InvalidParameterValue and the message "Value: <value> is outside of range: <AllowedValues> for parameter: <parameter>", whose fixed text is "Value: ", " is outside of range: ", " for parameter: " and whose variable parts are the submitted value, the parameter's published AllowedValues range and the parameter name).
Fix: the engine-default parameter catalogue backing DescribeDBParameters — it must be seeded per DBParameterGroupFamily with the full parameter set and each entry's Source/ApplyType/DataType/AllowedValues/IsModifiable, and DescribeDBParameters must serialize those members (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_parameters.rs and the RDS parameter-group state it reads).
Traces: rds.paramgroup-out-of-range-value-rejected-engine-setting-unchanged.

## Evidence
Cause 1 — rds.deploy-rds-instance-cfn-parameter-group-effective: stack create waiter completed, stack output ParamGroupName = "sim2real-rds-cfn-72c4339ef085-ParamGroup-A3C8B69335D7"; the next call failed with "botocore.errorfactory.DBParameterGroupNotFoundFault: An error occurred (DBParameterGroupNotFound) when calling the DescribeDBParameters operation: DBParameterGroup not found: sim2real-rds-cfn-72c4339ef085-ParamGroup-A3C8B69335D7". Same in rds.deploy-cfn-param-group-out-of-range-update-rejected-rolls-back: "DBParameterGroup not found: sim2real-rds2-pg-08ebcfce1cfd-DbParams-FB98FC01DA6F" after CREATE_COMPLETE. Expected: HTTP 200 with the group's Parameters (and, for the first check, innodb_lock_wait_timeout=120 listed with Source="user" and the instance's DBParameterGroups naming the group).

Cause 2 — rds.paramgroup-out-of-range-value-rejected-engine-setting-unchanged, after CreateDBParameterGroup(family=postgres17): "AssertionError: from_collapse_limit must be ApplyType 'dynamic' in family postgres17; got None" reading the DescribeDBParameters result (fully paginated, no Source filter). Expected: an entry ParameterName=from_collapse_limit with ApplyType="dynamic", DataType="integer", Source="engine-default" and a numeric AllowedValues range.

## Steps to Reproduce
Cause 1:
1. rds describe-db-engine-versions --engine mysql --default-only -> note DBParameterGroupFamily.
2. cloudformation create-stack with a template whose only resource is AWS::RDS::DBParameterGroup (Family = that family, Parameters: {innodb_lock_wait_timeout: "120"}) and an Output of {"Ref": "<logical id>"}; wait for CREATE_COMPLETE.
3. rds describe-db-parameters --db-parameter-group-name <that output value> -> HTTP 404 DBParameterGroupNotFound "DBParameterGroup not found: <name>" instead of 200 with the group's parameters. rds describe-db-parameter-groups shows no such group either.

Cause 2:
1. rds describe-db-engine-versions --engine postgres --default-only -> DBParameterGroupFamily (postgres17).
2. rds create-db-parameter-group --db-parameter-group-name g --db-parameter-group-family postgres17 --description d.
3. rds describe-db-parameters --db-parameter-group-name g (paginate fully) -> the from_collapse_limit entry has no ApplyType and no AllowedValues (or is absent), instead of ApplyType=dynamic / DataType=integer / Source=engine-default / AllowedValues=<range>.
