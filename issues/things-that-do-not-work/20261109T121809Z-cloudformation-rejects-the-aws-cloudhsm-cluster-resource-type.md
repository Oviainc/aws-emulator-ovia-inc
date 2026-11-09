Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudhsmv2.deploy-cloudformation-cloudhsm-cluster-round-trip-and-delete cloudformation__cloudhsmv2.deploy-cloudformation-cloudhsm-cluster-invalid-hsm-type-rejected
Issue-Key: cloudformation__cloudhsmv2|CreateStack|ValidationError
Issue-Commit: 9cecc173ac9c5f86564428d1030fb1c1f0593211

# CloudFormation rejects the AWS::CloudHSM::Cluster resource type: CreateStack fails "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `9cecc173ac9c5f86564428d1030fb1c1f0593211`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Both behavioral checks the suite lists under the `cloudformation__cloudhsmv2` heading fail on their very first CloudFormation call, with one root cause. There is 1 cause in this issue.

Cause 1 of 1: the emulator's CloudFormation template resource-type registry does not know `AWS::CloudHSM::Cluster`, so any template that declares a CloudHSM v2 cluster is refused at CreateStack time.

AWS: recorded behavior only (the CloudFormation resource registry is not part of any Smithy model the repo carries; the CloudHSMv2 model corroborates the property contract). The recordings show real CloudFormation ACCEPTING a template whose `Resources` contains a resource of `"Type": "AWS::CloudHSM::Cluster"` with properties `HsmType`, `Mode`, `SubnetIds`, `Tags`, and then:
  (a) provisioning it to `CREATE_COMPLETE`, with `Ref` and `Fn::GetAtt "Arn"` both returning the cluster ARN `arn:<partition>:cloudhsm:<region>:<account>:cluster/<clusterId>` (the partition, region, account and cluster id are values from the request/environment, not fixed text), and read-only attributes `ClusterId`, `State`, `VpcId`, `SecurityGroup`, `BackupPolicy` resolvable through `Fn::GetAtt` and agreeing member-for-member with what CloudHSMv2 `DescribeClusters` reports for the same cluster (`State` = `UNINITIALIZED`, `SubnetMapping` = {the subnet's AZ: the subnet id}); stack-level tags propagating onto the cluster alongside the template's own `Tags`; and `DeleteStack` leaving the documented `DELETED` tombstone still readable through `DescribeClusters(Filters={clusterIds:[...]})`.
  (b) refusing a bad property value through early property validation rather than at provisioning time: with `HsmType` = `not-a-valid-hsm-type` the stack ends `ROLLBACK_COMPLETE` and `DescribeEvents`/`OperationEvents` carries an `EventType` `VALIDATION_ERROR` row for the cluster's logical id with `ValidationName` `PROPERTY_VALIDATION`, `ValidationStatus` `FAILED`, `ValidationFailureMode` `FAIL`, `ValidationPath` `/Resources/<logicalId>/Properties/HsmType` and `ValidationStatusReason` whose fixed text is `Property value [<value>] does not match pattern: <pattern>` — the variable parts being the offending property value from the template and the registry's pattern, here `^((p|)hsm[0-9][a-z.]*\.[a-zA-Z]+)$`, which is exactly the pattern the CloudHSMv2 Smithy model puts on `CreateCluster.HsmType` (Smithy: CreateCluster.HsmType @required, @length(0,32), @pattern ^((p|)hsm[0-9][a-z.]*\.[a-zA-Z]+)$). No cluster exists after that rejection.

Emulator: CreateStack is refused outright, before any stack is created, with HTTP 400 `ValidationError` and message `Template format error: Unrecognized resource types: [AWS::CloudHSM::Cluster]`. The fixed text there is `Template format error: Unrecognized resource types: [` ... `]` and the variable part is the list of unknown type names; the defect is that `AWS::CloudHSM::Cluster` is in that list at all. Nothing downstream of CreateStack is exercised, so neither the GetAtt surface nor the PROPERTY_VALIDATION event rows can be observed yet.

Why: a client cannot deploy a CloudHSM cluster through infrastructure-as-code at all. Every template that mentions the type — including templates whose CloudHSM resource is incidental to a larger stack — is rejected whole, so the rest of the stack never deploys either; and a negative test that expects a property-validation rollback instead sees an immediate client error with a completely different code, status path and message, so error handling keyed on `ROLLBACK_COMPLETE` plus a `VALIDATION_ERROR` event never runs.

Fix: CloudFormation's template resource-type registry — the module that resolves `Resources[*].Type` to a provisioning handler and is the source of the "Unrecognized resource types" refusal — must register `AWS::CloudHSM::Cluster`, with (1) a provider that creates/deletes the cluster through the CloudHSMv2 state so `DescribeClusters` agrees and a stack delete leaves the `DELETED` tombstone, (2) `Ref`/`Fn::GetAtt` resolution for `Arn`, `ClusterId`, `State`, `VpcId`, `SecurityGroup`, `BackupPolicy`, (3) stack-tag propagation merged with the resource's own `Tags`, and (4) the registry's property schema for `HsmType` (the pattern above) wired into the early property-validation stage that emits the `VALIDATION_ERROR` / `PROPERTY_VALIDATION` operation-event rows and rolls the stack back.

Note on attribution: the suite lists these checks under the composite heading `cloudformation__cloudhsmv2`; the failing call and the missing component are CloudFormation's, so this issue is filed against `cloudformation`. The CloudHSMv2-side defects the same pair of checks would hit later are filed separately against `cloudhsmv2`.

Traces: cloudformation__cloudhsmv2.deploy-cloudformation-cloudhsm-cluster-round-trip-and-delete, cloudformation__cloudhsmv2.deploy-cloudformation-cloudhsm-cluster-invalid-hsm-type-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, 0/4 passed, bootstrap_signal false (the emulator is serving: it answers CreateStack with a modeled ValidationError).

[error] cloudformation__cloudhsmv2.deploy-cloudformation-cloudhsm-cluster-round-trip-and-delete
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CloudHSM::Cluster]

[error] cloudformation__cloudhsmv2.deploy-cloudformation-cloudhsm-cluster-invalid-hsm-type-rejected
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CloudHSM::Cluster]

Expected (recorded AWS behavior asserted by the checks):
  round-trip check: CreateStack accepted -> wait_stack == 'CREATE_COMPLETE'; stack Outputs ClusterRef == ClusterArn == arn:<partition>:cloudhsm:<region>:<account>:cluster/<clusterId>; {ClusterId, HsmType, Mode, State, VpcId, SecurityGroup, BackupPolicy, SubnetMapping} from DescribeClusters(Filters={vpcIds:[stack VPC]}) equal the stack's GetAtt values, with State == 'UNINITIALIZED' and SubnetMapping == {<subnet AZ>: <subnet id>}; TagList == [('sim2real', <stack name>), ('sim2real-hsm', <template tag value>)]; after DeleteStack, DescribeClusters(Filters={clusterIds:[<clusterId>]}) still returns the cluster with State == 'DELETED'.
  invalid-hsm-type check: the control stack (valid HsmType 'hsm2m.medium') reaches CREATE_COMPLETE; the bad stack ends 'ROLLBACK_COMPLETE' and describe_events/OperationEvents contains exactly one EventType == 'VALIDATION_ERROR' row with ValidationFailureMode == 'FAIL': ('Cluster', 'PROPERTY_VALIDATION', 'FAILED', 'FAIL', "Property value [not-a-valid-hsm-type] does not match pattern: ^((p|)hsm[0-9][a-z.]*\\.[a-zA-Z]+)$", '/Resources/Cluster/Properties/HsmType'); and no cluster carries the rejected template's tag.

Actual: HTTP 400, Error.Code 'ValidationError', Error.Message 'Template format error: Unrecognized resource types: [AWS::CloudHSM::Cluster]'. No stack is created, so no stack event, no Output and no GetAtt is produced.

Model corroboration for the property pattern: services/service-com-amazonaws-cloudhsmv2-baldrapiservice/smithy-model/com-amazonaws-cloudhsmv2-baldrapiservice.json — CreateCluster.HsmType: required, length 0..32, pattern ^((p|)hsm[0-9][a-z.]*\.[a-zA-Z]+)$.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite().
2. Replay check cloudformation__cloudhsmv2.deploy-cloudformation-cloudhsm-cluster-round-trip-and-delete. It calls cloudformation.CreateStack with a JSON template whose Resources are an AWS::EC2::VPC, an AWS::EC2::Subnet and a resource of Type 'AWS::CloudHSM::Cluster' (Properties HsmType='hsm2m.medium', Mode='NON_FIPS', SubnetIds=[{Ref: Subnet}], Tags=[...]) and Outputs taking Ref plus Fn::GetAtt Arn/ClusterId/State/VpcId/SecurityGroup/BackupPolicy off that resource. Observe the immediate 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::CloudHSM::Cluster]' instead of an accepted stack that reaches CREATE_COMPLETE.
3. Replay check cloudformation__cloudhsmv2.deploy-cloudformation-cloudhsm-cluster-invalid-hsm-type-rejected. The same template with HsmType='not-a-valid-hsm-type' must be accepted by CreateStack and then end ROLLBACK_COMPLETE with a VALIDATION_ERROR / PROPERTY_VALIDATION operation-event row naming /Resources/Cluster/Properties/HsmType and the registry pattern; observe the same immediate CreateStack ValidationError instead — the positive control leg of the same check fails identically.
4. After the fix, re-replay both check ids listed in trace_ids: they also assert the CloudHSMv2 read-back, so the CloudHSMv2-side issue must be closed before they pass.
