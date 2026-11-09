Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__timestream-influxdb.deploy-timestream-influxdb-instance-round-trip-and-delete cloudformation__timestream-influxdb.deploy-timestream-influxdb-instance-bad-deployment-type-rejected-then-valid
Issue-Key: cloudformation__timestream-influxdb|CreateStack|ValidationError
Issue-Commit: 41c5ae389f495755d1c1e47282b4b58fd25764aa

# cloudformation__timestream-influxdb: AWS::Timestream::InfluxDBInstance is not a registered resource type — CreateStack refuses every InfluxDB instance template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `41c5ae389f495755d1c1e47282b4b58fd25764aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in two failing behavioral checks.

Cause 1 of 1: CloudFormation's resource-type registry does not know AWS::Timestream::InfluxDBInstance, so a template that declares one is rejected at CreateStack instead of being deployed.

AWS: Smithy (cloudformation): CreateStack takes TemplateBody and returns CreateStackOutput.StackId; it models ValidationError (client, httpError 400) for a template CloudFormation cannot accept. AWS::Timestream::InfluxDBInstance is a supported CloudFormation resource type backed by the Timestream for InfluxDB control plane (its properties Name, DbInstanceType, AllocatedStorage, DbStorageType, Username, Password, VpcSubnetIds, VpcSecurityGroupIds, PubliclyAccessible, DeploymentType, Tags map onto the modeled timestream-influxdb CreateDbInstance input — Smithy: CreateDbInstance.name/@pattern, .dbInstanceType enum, .allocatedStorage @range 20..15360, .deploymentType enum SINGLE_AZ|WITH_MULTIAZ_STANDBY — and its Ref / Fn::GetAtt Arn / Endpoint / InfluxAuthParametersSecretArn come from CreateDbInstance output members id, arn, endpoint, influxAuthParametersSecretArn). Recorded behavior: CreateStack ACCEPTS such a template and returns a StackId; the stack then either reaches CREATE_COMPLETE (valid DeploymentType) or rolls back to ROLLBACK_COMPLETE with a per-resource PROPERTY_VALIDATION/FAILED row when a property value is not a valid enum value. The ValidationError "Template format error: Unrecognized resource types: [<Type>]" is AWS's refusal only for a type CloudFormation genuinely does not support; its fixed text is "Template format error: Unrecognized resource types: [" + the list of offending type names + "]".

Emulator: CreateStack answers HTTP 400 Error.Code "ValidationError" with Error.Message "Template format error: Unrecognized resource types: [AWS::Timestream::InfluxDBInstance]" — the message's fixed text is correct for an unknown type, but the type is one AWS does support, so the refusal itself is the defect. No stack is ever created, so both checks die on their first CreateStack and nothing downstream (stack events, Outputs, the instance in Timestream for InfluxDB, the rollback-absence read) is exercised.

Why: no client can provision Timestream for InfluxDB through CloudFormation, CDK or SAM against the emulator. Infrastructure-as-code that deploys an InfluxDB instance fails at template validation rather than at deploy time, and the negative path AWS takes (ROLLBACK_COMPLETE plus a PROPERTY_VALIDATION row naming /Resources/<Logical>/Properties/DeploymentType) can never be observed either.

Fix: the CloudFormation resource-type registry / template-validation stage must register AWS::Timestream::InfluxDBInstance, plus a resource handler for it that creates, reads (Ref -> the instance id, Fn::GetAtt Arn/Endpoint/InfluxAuthParametersSecretArn), property-validates (enum members rejected with a PROPERTY_VALIDATION FAILED event carrying the property path) and deletes the instance through the timestream-influxdb state.

Traces: cloudformation__timestream-influxdb.deploy-timestream-influxdb-instance-round-trip-and-delete, cloudformation__timestream-influxdb.deploy-timestream-influxdb-instance-bad-deployment-type-rejected-then-valid. Related but NOT covering: several open issues report other unregistered resource types (e.g. 20261109T143515Z-cloudformation-logs-cloudformation-rejects-six-modelled-aws-logs-resource-types.md, 20261109T163044Z-cloudformation-emr-containers-2-net-new-root-causes.md); none names AWS::Timestream::InfluxDBInstance.

## Evidence
Both checks fail on their first call:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Timestream::InfluxDBInstance]

Template submitted (minimal form): Resources.DbInstance.Type = "AWS::Timestream::InfluxDBInstance" with Properties Name, DbInstanceType "db.influx.medium", AllocatedStorage 20, DbStorageType "InfluxIOIncludedT1", Username, Password (NoEcho Ref), VpcSubnetIds, VpcSecurityGroupIds, PubliclyAccessible false, DeploymentType, Tags; Outputs reading {"Ref": "DbInstance"} and Fn::GetAtt of Arn / Endpoint / InfluxAuthParametersSecretArn.

Expected (AWS): CreateStack returns a StackId (HTTP 200). With DeploymentType "SINGLE_AZ" the stack reaches CREATE_COMPLETE and Outputs.InstanceId names an instance ListDbInstances reports with status AVAILABLE, deploymentType SINGLE_AZ and the template's Name. With an invalid DeploymentType the stack reaches ROLLBACK_COMPLETE and the stack events carry one validation row (LogicalResourceId "DbInstance", ResourceStatus FAILED, status reason "<value> is not a valid enum value. Supported values: [SINGLE_AZ, WITH_MULTIAZ_STANDBY]" at resource-property path /Resources/DbInstance/Properties/DeploymentType) and no instance of that name exists.

Actual (emulator): HTTP 400, ValidationError, "Template format error: Unrecognized resource types: [AWS::Timestream::InfluxDBInstance]"; no stack id is returned. (The bracketed type name is the request's; the rest of the message is the emulator's fixed unknown-type text.)

## Steps to Reproduce
1. Create a VPC with at least one subnet and one security group.
2. cloudformation:CreateStack with StackName=<any>, Parameters=[{Password,<16 hex chars>}] and TemplateBody declaring a single resource of Type "AWS::Timestream::InfluxDBInstance" with Properties Name=<3-40 chars matching ^[a-zA-Z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$>, DbInstanceType="db.influx.medium", AllocatedStorage=20, DbStorageType="InfluxIOIncludedT1", Username=<name>, Password={"Ref":"Password"}, VpcSubnetIds=[<subnet>], VpcSecurityGroupIds=[<sg>], PubliclyAccessible=false, DeploymentType="SINGLE_AZ".
3. Observe: AWS returns a StackId and the stack converges to CREATE_COMPLETE with the instance visible via timestream-influxdb ListDbInstances/GetDbInstance; the emulator refuses the call with ValidationError "Template format error: Unrecognized resource types: [AWS::Timestream::InfluxDBInstance]".
4. Repeat with DeploymentType="MULTI_AZ_BOGUS" to see the negative path (AWS: ROLLBACK_COMPLETE + PROPERTY_VALIDATION FAILED event); the emulator never gets that far.
