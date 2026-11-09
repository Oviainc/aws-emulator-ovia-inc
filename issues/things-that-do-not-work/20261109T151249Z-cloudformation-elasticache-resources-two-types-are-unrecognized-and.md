Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elasticache.cfn-elasticache-snapshot-unknown-cache-cluster-id-rejected cloudformation__elasticache.deploy-elasticache-cachecluster-and-snapshot-round-trip-and-delete cloudformation__elasticache.deploy-elasticache-serverlesscache-and-serverlesscachesnapshot-round-trip-and-delete cloudformation__elasticache.deploy-elasticache-subnetgroup-name-lowercased-round-trip-then-stack-delete cloudformation__elasticache.deploy-elasticache-parametergroup-round-trip-then-stack-delete cloudformation__elasticache.deploy-elasticache-user-and-usergroup-round-trip-then-child-and-stack-delete cloudformation__elasticache.deploy-elasticache-user-reserved-userid-default-rejected
Issue-Key: cloudformation__elasticache|CreateStack|ValidationError
Issue-Key: cloudformation__elasticache.deploy-elasticache-subnetgroup-name-lowercased-round-trip-then-stack-delete|run#12|AssertionError|assert out["SubnetGroupRef"] == requested_name.lower(), (
Issue-Key: cloudformation__elasticache.deploy-elasticache-parametergroup-round-trip-then-stack-delete|run#11|AssertionError|assert [g["CacheParameterGroupName"] for g in matched] == [out["ParamGroupRef"]], (
Issue-Key: cloudformation__elasticache.deploy-elasticache-user-and-usergroup-round-trip-then-child-and-stack-delete|run#15|AssertionError|assert len(matched) == 1, f"ListUsers found {len(matched)} entries for Ref {out['UserRef']!r}"
Issue-Key: cloudformation__elasticache.deploy-elasticache-user-reserved-userid-default-rejected|run#9|AssertionError|assert len(matched) == 1, f"ListUsers found {len(matched)} entries for {good_user!r}"
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# CloudFormation ElastiCache resources: two types are unrecognized, and SubnetGroup/ParameterGroup/User reach CREATE_COMPLETE without the resource ElastiCache can read (5 causes)

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Five net-new root causes behind every failing cloudformation__elasticache check. Two ElastiCache resource types are not in the emulator's CloudFormation resource registry at all, and three more report CREATE_COMPLETE while the resource is either named something the template never asked for or absent from ElastiCache entirely. None of these checks was marked ALREADY FILED or OWNED.

Cause 1 of 5: `AWS::ElastiCache::Snapshot` is refused as an unknown resource type.
  AWS: recorded behavior only (the resource registry is not in the Smithy model). AWS CloudFormation accepts a template containing `AWS::ElastiCache::Snapshot`, CreateStack returns a StackId, and the stack provisions an ElastiCache snapshot of the named cache cluster; a template naming a CacheClusterId that does not exist is accepted by CreateStack and fails during provisioning as a resource-level failure, not a template format error.
  Emulator: CreateStack is refused `400 ValidationError` with `Template format error: Unrecognized resource types: [AWS::ElastiCache::Snapshot]` — the fixed text is `Template format error: Unrecognized resource types: [` + `]` and the variable part is the type list; no stack is created, so neither the happy path nor the unknown-cache-cluster refusal can be reached.
  Why: any template that snapshots a cache cluster fails at submit time; a client cannot deploy or test ElastiCache backups through CloudFormation at all.
  Fix: the CloudFormation resource-type registry and the ElastiCache resource providers it dispatches to (the provider for AWS::ElastiCache::Snapshot and its Create/Delete handling). Related open issue 20261103T041241Z-ec2-ecs-elasticache-cloudformation-rejects-aws-ec2-transitgatewayroutetableassociation-and-aws-ec2.md records the same registry gap for two EC2 types only.
  Traces: cloudformation__elasticache.cfn-elasticache-snapshot-unknown-cache-cluster-id-rejected, cloudformation__elasticache.deploy-elasticache-cachecluster-and-snapshot-round-trip-and-delete.

Cause 2 of 5: `AWS::ElastiCache::ServerlessCacheSnapshot` is refused as an unknown resource type.
  AWS: recorded behavior only. AWS CloudFormation accepts the type, creates the serverless-cache snapshot, exposes its Ref and `Fn::GetAtt ARN`, and on an update that drops it deletes only the snapshot while the cache stays available.
  Emulator: CreateStack is refused `400 ValidationError` with `Template format error: Unrecognized resource types: [AWS::ElastiCache::ServerlessCacheSnapshot]`.
  Why: serverless-cache backups cannot be expressed in a template; the whole stack (VPC, subnets, security group, serverless cache) is rejected because of the one unknown type.
  Fix: same registry plus a provider for AWS::ElastiCache::ServerlessCacheSnapshot.
  Traces: cloudformation__elasticache.deploy-elasticache-serverlesscache-and-serverlesscachesnapshot-round-trip-and-delete.

Cause 3 of 5: `AWS::ElastiCache::SubnetGroup` ignores the template's CacheSubnetGroupName and invents a physical id.
  AWS: recorded behavior only. When a template supplies CacheSubnetGroupName, AWS creates the subnet group under exactly that name lower-cased, and `Ref` on the resource returns that lower-cased name — so a mixed-case `Sim2Real-ECSNG-<suffix>` yields the Ref `sim2real-ecsng-<suffix>`, and DescribeCacheSubnetGroups finds the group under it.
  Emulator: the Ref is an auto-generated physical id of the form `<stack-name>-<logical-id>-<hex>` (the requested name appears nowhere), so the explicit name property is dropped and the lower-casing rule is unobservable.
  Why: a client that names a subnet group in its template and then addresses it by that name from the SDK, another stack, or an Outputs export addresses a group that does not exist under that name.
  Fix: the AWS::ElastiCache::SubnetGroup provider: honour CacheSubnetGroupName as the physical resource id, lower-cased, and only generate a name when the property is absent.
  Traces: cloudformation__elasticache.deploy-elasticache-subnetgroup-name-lowercased-round-trip-then-stack-delete.

Cause 4 of 5: `AWS::ElastiCache::ParameterGroup` reaches CREATE_COMPLETE but no such parameter group exists in ElastiCache.
  AWS: recorded behavior only. With no CacheParameterGroupName property AWS generates the name, `Ref` returns it, and DescribeCacheParameterGroups lists exactly one group with the template's Description, the requested CacheParameterGroupFamily and the stack's propagated tags.
  Emulator: the stack is CREATE_COMPLETE and `Ref` yields a generated name, but an unfiltered DescribeCacheParameterGroups contains NO group carrying the template's Description — i.e. the resource the Ref names is not in ElastiCache's state.
  Why: a deploy reports success while the resource does not exist; anything that later reads, tags or attaches that parameter group (including another resource in the same stack) fails against an id CloudFormation handed out.
  Fix: the AWS::ElastiCache::ParameterGroup provider must actually call/create through the ElastiCache parameter-group state (Description, CacheParameterGroupFamily, Properties, tags) and return the created group's name as the physical id. Related open issue 20261104T091523Z-elasticache-via-cloudformation-endpoint-outputs-are-returned-with.md records the same CREATE_COMPLETE-without-the-resource failure for ReplicationGroup/ServerlessCache; it does not cover the parameter group.
  Traces: cloudformation__elasticache.deploy-elasticache-parametergroup-round-trip-then-stack-delete.

Cause 5 of 5: `AWS::ElastiCache::User` Refs the user NAME and creates no user under the template's UserId.
  AWS: recorded behavior only. AWS creates the user with the template's UserId, `Ref` returns that UserId, `Fn::GetAtt Arn` is `arn:<partition>:elasticache:<region>:<account>:user:<user-id>`, and DescribeUsers lists exactly one user with that UserId, the template's UserName, Engine, AccessString and Authentication.
  Emulator: the stack reaches CREATE_COMPLETE but `Ref` on the User resource returns `default` — the template's UserName, not its UserId — and an unfiltered DescribeUsers contains no user with the template's UserId at all (0 entries), in both the plain round-trip and the reserved-UserId check's positive control.
  Why: a template-created RBAC user is invisible to the SDK, and every dependent reference (the UserGroup's `UserIds: [{"Ref": "User"}]`, an Outputs export, a later tag or modify call) resolves to the literal user name `default` instead of the user — so the user group is built from a wrong id and nothing downstream can find the user.
  Fix: the AWS::ElastiCache::User provider: create through the ElastiCache user state and use UserId as the physical resource id (never UserName). Related open issue 20261108T030552Z-elasticache-createusergroup-misses-the-defaultuserrequired-refusal-and-rejects.md concerns CreateUserGroup validation, not the CloudFormation provider.
  Traces: cloudformation__elasticache.deploy-elasticache-user-and-usergroup-round-trip-then-child-and-stack-delete, cloudformation__elasticache.deploy-elasticache-user-reserved-userid-default-rejected.

## Evidence
Cause 1 — cloudformation__elasticache.cfn-elasticache-snapshot-unknown-cache-cluster-id-rejected and ...deploy-elasticache-cachecluster-and-snapshot-round-trip-and-delete: `botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ElastiCache::Snapshot]`.
Cause 2 — ...deploy-elasticache-serverlesscache-and-serverlesscachesnapshot-round-trip-and-delete: same ValidationError with `[AWS::ElastiCache::ServerlessCacheSnapshot]`.
Cause 3 — ...deploy-elasticache-subnetgroup-name-lowercased-round-trip-then-stack-delete: template Properties carry CacheSubnetGroupName='Sim2Real-ECSNG-<suffix>'; expected Ref == 'sim2real-ecsng-<suffix>'. Actual: `AssertionError: Ref 'sim2real-cfnecsng-<hex>-SubnetGroup-<hex>' != lower-cased name 'sim2real-ecsng-<suffix>'`.
Cause 4 — ...deploy-elasticache-parametergroup-round-trip-then-stack-delete: after CREATE_COMPLETE, expected exactly one DescribeCacheParameterGroups entry whose Description is the template's and whose CacheParameterGroupName equals the Ref. Actual: `AssertionError: DescribeCacheParameterGroups found [] for Ref 'sim2real-cfnecpg-<hex>-ParamGroup-<hex>'`.
Cause 5 — ...deploy-elasticache-user-and-usergroup-round-trip-then-child-and-stack-delete: `AssertionError: ListUsers found 0 entries for Ref 'default'` (the Ref should be the template UserId); ...deploy-elasticache-user-reserved-userid-default-rejected: `AssertionError: ListUsers found 0 entries for '<the template's UserId>'`.

## Steps to Reproduce
Cause 1: CreateStack with a template whose Resources contain {"Type": "AWS::ElastiCache::Snapshot", "Properties": {"SnapshotName": "<name>", "CacheClusterId": "<id>"}}. Expect a StackId; the emulator answers 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::ElastiCache::Snapshot]'.
Cause 2: CreateStack with a template containing AWS::ElastiCache::ServerlessCache plus {"Type": "AWS::ElastiCache::ServerlessCacheSnapshot", "Properties": {"ServerlessCacheSnapshotName": "<name>", "ServerlessCacheName": {"Ref": "Cache"}}}.
Cause 3: CreateStack with AWS::ElastiCache::SubnetGroup Properties {CacheSubnetGroupName: 'Sim2Real-ECSNG-<suffix>', Description, SubnetIds} and Outputs {Ref of the resource}; after CREATE_COMPLETE read the Output and DescribeCacheSubnetGroups. The Ref must be the lower-cased requested name and the group must be listed under it.
Cause 4: CreateStack with AWS::ElastiCache::ParameterGroup Properties {Description: '<unique>', CacheParameterGroupFamily: 'valkey7', Properties: {maxmemory-policy: 'allkeys-lru'}, Tags} and Output {Ref}; after CREATE_COMPLETE, page DescribeCacheParameterGroups and look for the group whose Description is '<unique>' and whose name equals the Ref.
Cause 5: CreateStack with AWS::ElastiCache::User Properties {UserId: '<id>', UserName: 'default', Engine: 'redis', AccessString: 'on ~* +@all', AuthenticationMode: {Type: 'no-password-required'}} plus an AWS::ElastiCache::UserGroup whose UserIds is [{"Ref": "User"}], and Outputs {Ref, GetAtt Arn}; after CREATE_COMPLETE compare the Ref to '<id>' and page DescribeUsers for UserId == '<id>'.
