Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.deploy-cachecluster-cfn-behavioral elasticache.deploy-replicationgroup-cfn-auth-enforced elasticache.deploy-replication-group-cfn-behaves elasticache.deploy-serverless-cache-cfn-behaves
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.deploy-cachecluster-cfn-behavioral|run#6|KeyError|o["OutputKey"]: o["OutputValue"]
Issue-Key: elasticache.deploy-replicationgroup-cfn-auth-enforced|run#7|KeyError|o["OutputKey"]: o["OutputValue"]
Issue-Key: elasticache|DescribeReplicationGroups|ReplicationGroupNotFoundFault
Issue-Key: elasticache|DescribeServerlessCaches|ServerlessCacheNotFoundFault
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# ElastiCache via CloudFormation: endpoint Outputs are returned with no OutputValue, and a CREATE_COMPLETE stack never creates its ReplicationGroup or ServerlessCache (2 causes)

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
TWO net-new root causes on the CloudFormation-deployed ElastiCache surface (the replay reports both checks pairs under the elasticache service heading). In both the stack is reported as fully created, so a client gets no error at all and only discovers the gap when it tries to use the cache.

Relationship to open issues: the same two shapes of defect are already filed for OTHER services — stack Outputs returned without OutputValue in 20261102T093047Z-cloudformation-sns-sqs-sts-an-executed-change-set.md, 20261102T101202Z-cloudformation-dynamodb-lambda-3-net-new-root-causes.md and 20261103T200742Z-ec2-ssm-cloudformation-deployed-ec2-ssm-estates-are-unusable-instance.md, and stack-declared resources missing from their owning service in 20261102T090835Z-cloudformation-iam-lambda-logs-a-stack-that-reaches-create-complete-never-creates.md and 20261102T114105Z-rds-secretsmanager-a-cloudformation-created-aws-rds-dbcluster.md. None of those names an AWS::ElastiCache::* resource type or carries these checks' replay keys, so the ElastiCache attribute resolution and resource provisioning below are unexercised by them.

Cause 1 of 2: Fn::GetAtt on AWS::ElastiCache::CacheCluster and AWS::ElastiCache::ReplicationGroup endpoint attributes resolves to nothing, so the stack's Output rows come back with an OutputKey and NO OutputValue member.
AWS: recorded behavior only (CloudFormation attribute resolution is not in the ElastiCache Smithy model): a stack whose Outputs are {"Address": {"Fn::GetAtt": ["Cluster", "RedisEndpoint.Address"]}, "Port": {"Fn::GetAtt": ["Cluster", "RedisEndpoint.Port"]}} (and, for a replication group, ["ReplicationGroup", "PrimaryEndPoint.Address"] / ["PrimaryEndPoint.Port"]) reaches CREATE_COMPLETE and DescribeStacks returns each Output with BOTH OutputKey and OutputValue present: OutputValue is the cache's endpoint host for the .Address attribute and the endpoint port (as a string) for the .Port attribute. AWS never returns an Output row without an OutputValue once a stack is CREATE_COMPLETE.
Emulator: the stacks reach CREATE_COMPLETE (the SDK's stack_create_complete waiter returns), but each Outputs entry carries no OutputValue key at all, so reading it raises KeyError: 'OutputValue' in the client.
Why: the endpoint of a cache deployed by template is normally published through exactly these Outputs; a client (or a CDK/CLI consumer reading `describe-stacks`) has no address to connect to and cannot tell a resolved-but-empty value from an unresolved attribute. The emulator also returns a response shape AWS never produces for a completed stack.
Fix: the CloudFormation resource-attribute resolution for the AWS::ElastiCache::CacheCluster (RedisEndpoint.Address/RedisEndpoint.Port) and AWS::ElastiCache::ReplicationGroup (PrimaryEndPoint.Address/PrimaryEndPoint.Port) resource types, plus the Outputs serializer, which must not emit an Output row with an absent OutputValue.
Traces: elasticache.deploy-cachecluster-cfn-behavioral, elasticache.deploy-replicationgroup-cfn-auth-enforced.

Cause 2 of 2: a stack that reports CREATE_COMPLETE never creates its AWS::ElastiCache::ReplicationGroup or AWS::ElastiCache::ServerlessCache — ElastiCache does not know the resource exists.
AWS: Smithy: DescribeReplicationGroups models ReplicationGroupNotFoundFault {errorKind client, httpError 404, awsQueryError ReplicationGroupNotFoundFault} and DescribeServerlessCaches models ServerlessCacheNotFoundFault {client, httpError 404, awsQueryError ServerlessCacheNotFoundFault/404} — the refusals a client gets for a name that does not exist. Recorded behavior: once a stack containing AWS::ElastiCache::ReplicationGroup (or AWS::ElastiCache::ServerlessCache) reaches CREATE_COMPLETE, the resource EXISTS in ElastiCache: DescribeReplicationGroups(ReplicationGroupId=<the template's id>) answers HTTP 200 with Status "available" and a NodeGroups[0].PrimaryEndpoint, and DescribeServerlessCaches(ServerlessCacheName=<the template's name>) answers HTTP 200 with Status "available" and an Endpoint. The 404 is correct only AFTER the stack is deleted.
Emulator: after CREATE_COMPLETE, DescribeReplicationGroups answers 404 ReplicationGroupNotFoundFault ("ReplicationGroup <id> not found.") and DescribeServerlessCaches answers 404 ServerlessCacheNotFoundFault ("Serverless cache with name <name> not found.") — the stack's ElastiCache resources were never provisioned into the ElastiCache service, yet nothing failed and no resource event reported a problem.
Why: a template is the normal way these caches are provisioned. A client is told its estate is up, then every subsequent control-plane read and every data-plane connection fails against a resource that does not exist; the stack status is actively misleading, and a stack UPDATE that is supposed to drive ModifyReplicationGroup has nothing to modify.
Fix: the CloudFormation provisioner's handlers for the AWS::ElastiCache::ReplicationGroup and AWS::ElastiCache::ServerlessCache resource types must call through to the ElastiCache service (CreateReplicationGroup / CreateServerlessCache and their updates/deletes) and must fail the resource — CREATE_FAILED and the stack's rollback — when that call is refused, instead of reporting success for a resource that was never created. Note the serverless path additionally depends on the separately filed CreateServerlessCache subnet-resolution defect.
Traces: elasticache.deploy-replication-group-cfn-behaves, elasticache.deploy-serverless-cache-cfn-behaves.

## Evidence
All four failing checks are [error] verdicts from this replay of suite simparity-behavioral-aws-v1 (40/161 passed), listed under the elasticache service heading, and none is marked ALREADY FILED or OWNED.

Cause 1 — Outputs with no OutputValue:
  elasticache.deploy-cachecluster-cfn-behavioral, check_elasticache_deploy_cachecluster_cfn_behavioral.py line 69. The stack declares AWS::EC2::SecurityGroup + AWS::ElastiCache::SubnetGroup + AWS::ElastiCache::CacheCluster and Outputs {"Address": Fn::GetAtt[Cluster, "RedisEndpoint.Address"], "Port": Fn::GetAtt[Cluster, "RedisEndpoint.Port"]}; CreateStack is followed by the stack_create_complete waiter, which RETURNS (so the stack is CREATE_COMPLETE), then DescribeStacks(StackName=<stack>)["Stacks"][0]["Outputs"] is read.
    KeyError: 'OutputValue'
  elasticache.deploy-replicationgroup-cfn-auth-enforced, check line 75: same pattern with Outputs {"Address": Fn::GetAtt[ReplicationGroup, "PrimaryEndPoint.Address"], "Port": Fn::GetAtt[ReplicationGroup, "PrimaryEndPoint.Port"]} over an AWS::ElastiCache::ReplicationGroup with TransitEncryptionEnabled=true and an AuthToken.
    KeyError: 'OutputValue'
  Expected: DescribeStacks on the CREATE_COMPLETE stack returns Outputs=[{OutputKey:"Address", OutputValue:<the cache endpoint host>}, {OutputKey:"Port", OutputValue:<the endpoint port>}].
  Actual:   each Output row has OutputKey and no OutputValue member at all.

Cause 2 — stack-declared ElastiCache resources do not exist:
  elasticache.deploy-replication-group-cfn-behaves: CreateStack(TemplateBody=<AWS::ElastiCache::SubnetGroup + AWS::ElastiCache::ReplicationGroup with ReplicationGroupId=<id>, Engine=redis, CacheNodeType=cache.t3.micro, NumCacheClusters=1, AutomaticFailoverEnabled=false, SnapshotRetentionLimit=0>, OnFailure="DELETE"); the check polls DescribeStacks until StackStatus==CREATE_COMPLETE and asserts it, then calls DescribeReplicationGroups(ReplicationGroupId=<id>):
    botocore.errorfactory.ReplicationGroupNotFoundFault: An error occurred (ReplicationGroupNotFoundFault) when calling the DescribeReplicationGroups operation: ReplicationGroup sim2real-rg-89c0c14c389c not found.
  elasticache.deploy-serverless-cache-cfn-behaves: CreateStack(TemplateBody=<AWS::ElastiCache::ServerlessCache with ServerlessCacheName=<name>, Engine=redis, SubnetIds, SecurityGroupIds>, OnFailure="DELETE"); after asserting CREATE_COMPLETE, DescribeServerlessCaches(ServerlessCacheName=<name>):
    botocore.errorfactory.ServerlessCacheNotFoundFault: An error occurred (ServerlessCacheNotFoundFault) when calling the DescribeServerlessCaches operation: Serverless cache with name sim2real-sc-232a8ad9bf86 not found.
  Expected: HTTP 200 with Status "available" and the resource's endpoint (NodeGroups[0].PrimaryEndpoint for the replication group, Endpoint for the serverless cache); the 404 only after the stack is deleted (which each check asserts later).
  Actual:   HTTP 404 with the not-found fault while the stack is live and CREATE_COMPLETE. (The ids/names shown are this run's generated values; the requirement is that the resource the template declares is describable.)

Model evidence: smithy_query(query="operation", operation="CreateServerlessCache,ModifyServerlessCache,DescribeCacheParameterGroups", model_path="services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json") -> ServerlessCacheNotFoundFault {client, httpError 404, awsQueryError ServerlessCacheNotFoundFault/404}; the ReplicationGroupNotFoundFault classification {client, 404} is recorded in the elasticache model the same way. The CloudFormation attribute names and the post-CREATE_COMPLETE existence contract are recorded behavior, not model statements.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the four checks listed in trace_ids.

2. Cause 1 (cache cluster): cloudformation CreateStack with a template declaring an AWS::ElastiCache::SubnetGroup over two subnets of the run's VPC and an AWS::ElastiCache::CacheCluster (Engine=redis, CacheNodeType=cache.t3.micro, NumCacheNodes=1) in it, plus Outputs {"Address": {"Fn::GetAtt": ["Cluster", "RedisEndpoint.Address"]}, "Port": {"Fn::GetAtt": ["Cluster", "RedisEndpoint.Port"]}}. Wait for CREATE_COMPLETE, then DescribeStacks(StackName=<stack>) and read Stacks[0].Outputs. Expect every row to carry OutputKey AND OutputValue, with the Address row holding the cluster's endpoint host and the Port row its port. Observe rows with no OutputValue (KeyError in any SDK client).

3. Cause 1 (replication group): same, with an AWS::ElastiCache::ReplicationGroup (Engine=redis, NumCacheClusters=1, TransitEncryptionEnabled=true, AuthToken=<40 printable ASCII chars>) and Outputs over Fn::GetAtt ["ReplicationGroup", "PrimaryEndPoint.Address"] / ["PrimaryEndPoint.Port"].

4. Cause 2 (replication group): CreateStack with AWS::ElastiCache::SubnetGroup + AWS::ElastiCache::ReplicationGroup (ReplicationGroupId=<fresh id>, Engine=redis, CacheNodeType=cache.t3.micro, NumCacheClusters=1, AutomaticFailoverEnabled=false, CacheSubnetGroupName={"Ref":"SubnetGroup"}), OnFailure=DELETE. Poll DescribeStacks until CREATE_COMPLETE, then elasticache DescribeReplicationGroups(ReplicationGroupId=<that id>). Expect HTTP 200, Status "available", NodeGroups[0].PrimaryEndpoint present. Observe HTTP 404 ReplicationGroupNotFoundFault. Then UpdateStack changing SnapshotRetentionLimit 0 -> 1 and confirm the member cluster reports SnapshotRetentionLimit=1, and after DeleteStack confirm the 404 returns.

5. Cause 2 (serverless cache): CreateStack with an AWS::ElastiCache::ServerlessCache (ServerlessCacheName=<fresh>, Engine=redis, SubnetIds=<subnets of the run's VPC>, SecurityGroupIds=[<sg>]), OnFailure=DELETE. Poll until CREATE_COMPLETE, then elasticache DescribeServerlessCaches(ServerlessCacheName=<that name>). Expect HTTP 200 with Status "available" and an Endpoint. Observe HTTP 404 ServerlessCacheNotFoundFault. After the stack is deleted, DescribeServerlessCaches must answer ServerlessCacheNotFoundFault with "Serverless cache with name <name> not found.".

6. In both Cause 2 scenarios, also confirm the alternative contract: if the underlying ElastiCache create call is refused, the stack resource must reach CREATE_FAILED with that refusal as its ResourceStatusReason and the stack must roll back — never CREATE_COMPLETE with no resource.
