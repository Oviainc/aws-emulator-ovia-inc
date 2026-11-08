Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__ec2__elasticache.vvd-agent-role-deletes-denied-while-owner-deletes-succeed dynamodb__ec2__elasticache.e2e-vvd-create-vpc-valkey-dynamodb-lifecycle
Issue-Key: dynamodb__ec2__elasticache.vvd-agent-role-deletes-denied-while-owner-deletes-succeed|_refusal#19|AssertionError|raise AssertionError("the call was expected to be refused, but it succeeded")
Issue-Key: dynamodb__ec2__elasticache.e2e-vvd-create-vpc-valkey-dynamodb-lifecycle|_poll#9|Timeout|time.sleep(interval_s)
Issue-Commit: d8469fdbee4aefb676d6a1b6581464fb2b4efb05

# dynamodb__ec2__elasticache: 2 net-new causes — deletes are authorized for a role granted no Delete action, and the VPC/Valkey/DynamoDB lifecycle never converges

> _Note: this issue was identified against the code base at commit `d8469fdbee4aefb676d6a1b6581464fb2b4efb05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCacheSubnetGroup

## Description
Two independent root causes in the dynamodb__ec2__elasticache scenario group. Both must be closed before submitting.

Cause 1 of 2: a delete is AUTHORIZED for an assumed role whose inline policy grants no Delete action at all.
AWS: recorded behavior only for the per-service refusal codes (IAM authorization is not expressed in the service Smithy models). The role under test is built from the scenario's own template and grants exactly `ec2:CreateVpc`, `ec2:CreateSubnet`, `ec2:CreateTags`, `ec2:Describe*`, `elasticache:CreateCacheSubnetGroup`, `elasticache:CreateReplicationGroup`, `elasticache:AddTagsToResource`, `elasticache:Describe*`, `iam:CreateServiceLinkedRole` (conditioned) and `dynamodb:CreateTable`, `dynamodb:DescribeTable` — the check asserts up front that no granted action contains `:Delete`. With no Allow matching, each delete must be refused with that service's authorization code and must change nothing: ElastiCache `DeleteCacheSubnetGroup` -> `AccessDenied`, EC2 `DeleteSubnet` and `DeleteVpc` -> `UnauthorizedOperation`, DynamoDB `DeleteTable` -> `AccessDeniedException`. The twin is what proves the refusal is the missing grant and not a broken request: the OWNER's identical calls, in the same order, succeed, after which each resource stops describing.
Emulator: the delete is accepted and performed. The check's refusal loop raises `AssertionError: the call was expected to be refused, but it succeeded` on its first iteration — i.e. the first delete in dependency order, ElastiCache `DeleteCacheSubnetGroup`, returned 200 for a principal with no `elasticache:DeleteCacheSubnetGroup` grant. Everything before it passed, so the role, the AssumeRole session and the agent's permitted creates (CreateVpc, CreateSubnet, CreateCacheSubnetGroup, CreateTable) are all honoured — only the deny-by-default on delete is missing.
Why: authorization is a hard boundary a caller relies on. A least-privilege role that AWS stops from deleting infrastructure deletes it here, so a scenario that must prove its agent cannot destroy its estate instead destroys it, and any negative test of a missing Delete grant silently inverts.
Fix: the authorization gate in front of the mutating ElastiCache/EC2/DynamoDB delete handlers — a request signed by an assumed-role session with no matching Allow must be refused with the service's authorization code and must not reach the state mutation. Related and distinct: the open issue 20261024T015148Z-dynamodb-does-not-enforce-iam-listtables-succeeds-for.md reports the same deny-by-default gap on a DynamoDB READ; this check is grounded in the delete path across elasticache/ec2/dynamodb and the suite does not mark it covered.
Traces: dynamodb__ec2__elasticache.vvd-agent-role-deletes-denied-while-owner-deletes-succeed

Cause 2 of 2: the VPC + Valkey replication group + DynamoDB table lifecycle never reaches its first set of assertions.
AWS: recorded behavior only. The scenario, run under an assumed agent role, fires a frozen trajectory — ec2:CreateVpc, ec2:DescribeVpcs, ec2:DescribeAvailabilityZones, ec2:CreateSubnet, elasticache:CreateCacheSubnetGroup, elasticache:CreateReplicationGroup (engine valkey 8.0, cache.t3.micro, 1 cache cluster, transit encryption on), dynamodb:CreateTable — then reads each resource back: the VPC non-default with CIDR 10.0.0.0/16, its subnets in the chosen AZs, the replication group polled until it LEAVES `creating` and settles `available` with Engine=valkey / TransitEncryptionEnabled=true / CacheNodeType=cache.t3.micro / one member cluster whose EngineVersion is 8.x and whose CacheSubnetGroup sits in the solved VPC, and the table ACTIVE with key schema PK(HASH)/SK(RANGE). The restore phase then removes everything but the cache subnet group, which outlives its deleted VPC until swept, and the role stack's destroy leaves nothing describable.
Emulator: the check never completes — it is cut off with `Check timed out after 180.0s`, so none of the read-back assertions is reached and the scenario produces no verdict. Hypothesis: the only unbounded wait before the first assertion is the poll for the replication group to leave `creating` (polled every 15 s), so a CreateReplicationGroup that is accepted but whose status never advances off `creating` would consume the whole budget; the sibling check in this group reaches its assertions and exercises CreateCacheSubnetGroup successfully, so the subnet-group and VPC steps are not the stall.
Why: a caller that creates a Valkey replication group and waits for `available` — the documented way to know a cache is usable — waits forever, and every scenario built on a settled cache is unrunnable.
Fix: the ElastiCache CreateReplicationGroup handler and the replication-group state machine that advances `creating` -> `available` (and the member cache cluster it must mint), so DescribeReplicationGroups/DescribeCacheClusters converge.
Traces: dynamodb__ec2__elasticache.e2e-vvd-create-vpc-valkey-dynamodb-lifecycle

## Evidence
Both from the behavioral suite replay against this checkout (service heading `### dynamodb__ec2__elasticache`; the group is 0/2 passing).

Cause 1 — dynamodb__ec2__elasticache.vvd-agent-role-deletes-denied-while-owner-deletes-succeed (verdict: mismatch):
  expected: each of the agent's four deletes, in dependency order (elasticache DeleteCacheSubnetGroup, ec2 DeleteSubnet, ec2 DeleteVpc, dynamodb DeleteTable), is refused with AccessDenied / UnauthorizedOperation / UnauthorizedOperation / AccessDeniedException respectively, and the estate re-describes unchanged.
  actual:   the refusal helper raised on the loop's first call —
            File "check_dynamodb__ec2__elasticache_vvd_agent_role_deletes_denied_while_owner_deletes_succeed.py", line 79, in run: code = _refusal(lambda: call(agent[svc]))
            File "...", line 218, in _refusal: raise AssertionError("the call was expected to be refused, but it succeeded")
            AssertionError: the call was expected to be refused, but it succeeded
  (the preceding steps passed: the role was created with a no-Delete policy, AssumeRole succeeded, all four agent creates were allowed, and the owner read every created resource back.)

Cause 2 — dynamodb__ec2__elasticache.e2e-vvd-create-vpc-valkey-dynamodb-lifecycle (verdict: mismatch):
  expected: the deploy/solve/post_invoke/cleanup/destroy phases complete and every read-back assertion holds (replication group settles `available`, engine valkey 8.0, transit encryption on, one member cluster, table ACTIVE).
  actual:   Error: Check timed out after 180.0s — the check is cut off with no assertion reached.

## Steps to Reproduce
Cause 1: 
1. Create an IAM role trusted by the account whose single inline policy allows ONLY ec2:CreateVpc, ec2:CreateSubnet, ec2:CreateTags, ec2:Describe*, elasticache:CreateCacheSubnetGroup, elasticache:CreateReplicationGroup, elasticache:AddTagsToResource, elasticache:Describe*, iam:CreateServiceLinkedRole (conditioned on elasticache.amazonaws.com) and dynamodb:CreateTable, dynamodb:DescribeTable — no Delete action of any kind.
2. AssumeRole into it and, as that session, create a VPC (10.0.0.0/16), a subnet in an available AZ, an ElastiCache cache subnet group over that subnet, and an on-demand DynamoDB table keyed PK(HASH)/SK(RANGE); wait for the table ACTIVE.
3. As the assumed session call, in order: elasticache DeleteCacheSubnetGroup, ec2 DeleteSubnet, ec2 DeleteVpc, dynamodb DeleteTable. Each must be refused with AccessDenied / UnauthorizedOperation / UnauthorizedOperation / AccessDeniedException and must leave the resource describable and unchanged. The emulator performs the first one (DeleteCacheSubnetGroup) instead.
4. Control: the account owner's identical calls in the same order succeed and each resource then stops describing.

Cause 2:
1. Replay dynamodb__ec2__elasticache.e2e-vvd-create-vpc-valkey-dynamodb-lifecycle, or by hand: under an assumed agent role create a VPC, two subnets in distinct AZs, a cache subnet group, then CreateReplicationGroup(Engine='valkey', EngineVersion='8.0', CacheNodeType='cache.t3.micro', NumCacheClusters=1, CacheSubnetGroupName=<group>, TransitEncryptionEnabled=True), then a DynamoDB table.
2. Poll DescribeReplicationGroups for that id until Status leaves 'creating'. It must reach 'available'; on the emulator the lifecycle does not converge and the check is killed at the 180 s harness budget.
