Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__rds.cfn-rds-instance-snapshot-provisioning-and-teardown cloudformation__rds.cfn-rds-cluster-snapshot-provisioning-and-teardown cloudformation__rds.deploy-rds-db-cluster-endpoint-round-trip-and-delete cloudformation__rds.cfn-rds-instance-invalid-engine-version-rejected-and-rolled-back cloudformation__rds.deploy-cfn-rds-db-parameter-group-invalid-family-rejected-and-rolled-back cloudformation__rds.deploy-rds-db-cluster-parameter-group-invalid-family-rejected cloudformation__rds.deploy-cfn-rds-group-resource-types-provision-and-deprovision cloudformation__rds.cfn-rds-instance-default-deletion-policy-snapshot
Issue-Key: cloudformation__rds|CreateStack|ValidationError
Issue-Key: cloudformation__rds.cfn-rds-instance-invalid-engine-version-rejected-and-rolled-back|run#9|AssertionError|assert got == _REJECTED, f"the DBInstance CREATE_FAILED reason was {got!r}, expected {_REJECTED!r}"
Issue-Key: cloudformation__rds.deploy-cfn-rds-db-parameter-group-invalid-family-rejected-and-rolled-back|run#6|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__rds.deploy-rds-db-cluster-parameter-group-invalid-family-rejected|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Key: cloudformation__rds.deploy-cfn-rds-group-resource-types-provision-and-deprovision|run#14|AssertionError|assert pg["DBParameterGroupArn"] == outputs["ParamGroupArnOut"], (
Issue-Key: cloudformation__rds.cfn-rds-instance-default-deletion-policy-snapshot|_poll_status#20|Timeout|time.sleep(interval_s)
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# cloudformation__rds: 6 net-new causes — RDS snapshot/endpoint resource types unregistered, invalid parameter-group families never roll a stack back, GetAtt ARN shape wrong, status reason unwrapped

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
6 net-new root causes in the CloudFormation/RDS resource providers.

Cause 1 of 6: AWS::RDS::DBSnapshot, AWS::RDS::ClusterSnapshot and AWS::RDS::DBClusterEndpoint are not registered resource types.
AWS: CreateStack accepts templates declaring these three RDS types and provisions a snapshot / cluster snapshot / custom cluster endpoint; 'Template format error: Unrecognized resource types' is reserved for types CloudFormation does not know. Smithy (rds): the underlying operations exist - CreateDBSnapshot, CreateDBClusterSnapshot, CreateDBClusterEndpoint.
Emulator: CreateStack is refused 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::RDS::DBSnapshot]' / '[AWS::RDS::ClusterSnapshot]' / '[AWS::RDS::DBClusterEndpoint]'.
Why: snapshot and custom-endpoint resources cannot be declared in a template at all, so no stack that owns its own backups or reader endpoints can deploy, and the failure arrives as a template error rather than anything a client can act on.
Fix: the CloudFormation resource-type registry plus providers for these three types, delegating to the RDS operations named above.
Traces: cloudformation__rds.cfn-rds-instance-snapshot-provisioning-and-teardown, cloudformation__rds.cfn-rds-cluster-snapshot-provisioning-and-teardown, cloudformation__rds.deploy-rds-db-cluster-endpoint-round-trip-and-delete.

Cause 2 of 6: a resource's CREATE_FAILED reason is the bare service message, not the SDK-wrapped one.
AWS: when an RDS call refuses a resource create, CloudFormation records the message the SDK surfaced: '<service message> (Service: Rds, Status Code: <code>, Request ID: <id>) (SDK Attempt Count: <n>)'. The leading service message here is 'Cannot find version <version> for <engine>' - fixed text with the requested version and engine substituted; the suffix's words are fixed and its id/count are per-run values.
Emulator: the DBInstance CREATE_FAILED reason is 'Cannot find version 0.0.1 for mysql' with no wrapper.
Why: tooling that splits the reason on '(Service:' to recover the service message, the status code or the request id finds none of them.
Fix: the CloudFormation resource-status-reason serializer (it must wrap provider refusals), not the RDS message itself, which is correct.
Traces: cloudformation__rds.cfn-rds-instance-invalid-engine-version-rejected-and-rolled-back.

Cause 3 of 6: an AWS::RDS::DBParameterGroup with an invalid Family is created instead of refused, so the stack reaches CREATE_COMPLETE.
AWS: CreateDBParameterGroup validates DBParameterGroupFamily against the engine families the account can see and refuses an unknown family; the resource therefore goes CREATE_FAILED and the stack settles ROLLBACK_COMPLETE.
Emulator: the stack settles CREATE_COMPLETE - the bogus family is accepted.
Why: a template typo in a parameter-group family deploys 'successfully' and leaves an unusable group behind; a client's negative test for the rejection can never pass.
Fix: Family validation in the RDS CreateDBParameterGroup handler (the CFN provider correctly propagates whatever RDS answers).
Traces: cloudformation__rds.deploy-cfn-rds-db-parameter-group-invalid-family-rejected-and-rolled-back.

Cause 4 of 6: the same hole for AWS::RDS::DBClusterParameterGroup.
AWS: CreateDBClusterParameterGroup refuses an unknown DBParameterGroupFamily, so the stack settles ROLLBACK_COMPLETE.
Emulator: the stack settles CREATE_COMPLETE.
Why/Fix: as cause 3, in the CreateDBClusterParameterGroup handler's family validation.
Traces: cloudformation__rds.deploy-rds-db-cluster-parameter-group-invalid-family-rejected.

Cause 5 of 6: GetAtt of a DB parameter group's ARN uses a resource type the RDS ARN namespace does not have.
AWS: a DB parameter group's ARN is arn:aws:rds:<region>:<account>:pg:<name> - the resource type is 'pg' and the separator is ':'; DescribeDBParameterGroups returns exactly that in DBParameterGroupArn, and the stack's !GetAtt must be the same string.
Emulator: RDS returns 'arn:aws:rds:<region>:<account>:pg:<name>' but the stack output is 'arn:aws:rds:<region>:<account>:db-parameter-group/<name>', so the two disagree.
Why: a client that feeds the stack output into an RDS call (AddTagsToResource, DescribeDBParameterGroups by ARN) or compares it with the API's own ARN gets a malformed identifier.
Fix: the AWS::RDS::DBParameterGroup provider's Arn attribute in the CloudFormation service - it must publish the ARN RDS minted rather than build its own.
Traces: cloudformation__rds.deploy-cfn-rds-group-resource-types-provision-and-deprovision.

Cause 6 of 6: a stack whose DBInstance relies on the default DeletionPolicy never settles.
AWS: a single-instance RDS stack creates and deletes well inside the check's 900 s budget, and DeletionPolicy defaults to Snapshot for AWS::RDS::DBInstance, so the delete takes a final snapshot and reaches DELETE_COMPLETE.
Emulator: the check does not finish within 900 s - the stack never reaches a terminal status (recorded behavior only; no status was ever observed).
Why: a client waiting on the stack's terminal status hangs forever, and the default-DeletionPolicy snapshot contract is unobservable.
Fix: the AWS::RDS::DBInstance provider's create/delete settling and its default DeletionPolicy handling in the CloudFormation service.
Traces: cloudformation__rds.cfn-rds-instance-default-deletion-policy-snapshot.

## Evidence
Cause 1: ClientError (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::RDS::DBSnapshot] / [AWS::RDS::ClusterSnapshot] / [AWS::RDS::DBClusterEndpoint].
Cause 2: DBInstance CREATE_FAILED reason was ('Cannot find version 0.0.1 for mysql', 'InvalidRequest'); expected ('Cannot find version 0.0.1 for mysql (Service: Rds, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest').
Cause 3: invalid family stack settled at 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE.
Cause 4: rejected stack ended 'CREATE_COMPLETE'.
Cause 5: RDS Arn 'arn:aws:rds:us-west-2:<acct>:pg:<name>' != stack GetAtt output 'arn:aws:rds:us-west-2:<acct>:db-parameter-group/<name>'.
Cause 6: Check timed out after 900.0s.

## Steps to Reproduce
Cause 1: CreateStack with a template declaring AWS::RDS::DBSnapshot (then ClusterSnapshot, then DBClusterEndpoint).
Cause 2: deploy a stack with an AWS::RDS::DBInstance whose EngineVersion is '0.0.1' and read the CREATE_FAILED ResourceStatusReason.
Causes 3-4: deploy a stack with an AWS::RDS::DBParameterGroup (and separately a DBClusterParameterGroup) whose Family is a non-existent string; wait for the stack's terminal status.
Cause 5: deploy a stack with an AWS::RDS::DBParameterGroup, output !GetAtt <pg>.Arn, and compare with DescribeDBParameterGroups' DBParameterGroupArn.
Cause 6: deploy a stack with an AWS::RDS::DBInstance and no explicit DeletionPolicy, then delete it, waiting for terminal status.
