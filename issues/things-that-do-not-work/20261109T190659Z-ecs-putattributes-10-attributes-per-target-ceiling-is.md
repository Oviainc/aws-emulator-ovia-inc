Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.put-attributes-exceeding-limit-rejected
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.put-attributes-exceeding-limit-rejected|_find#4|Timeout|return None
Issue-Commit: 4d3194b3b12f41db158240bd472cda702e18823e

# ecs: PutAttributes' 10-attributes-per-target ceiling is never reached or enforced — the container-instance target never becomes registerable, so the check burns its whole 180s budget

> _Note: this issue was identified against the code base at commit `4d3194b3b12f41db158240bd472cda702e18823e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAttributes

## Description
Cause 1 of 1: the PutAttributes per-target attribute-limit flow never reaches an observable step, and the ceiling itself is unimplemented.

AWS: ECS applies at most 10 custom attributes to each resource. PutAttributes (Smithy: PutAttributes, input `attributes` (Attributes) required, `cluster` (String); modeled errors AccessDeniedException 403, AttributeLimitExceededException 400, ClientException 400, ClusterNotFoundException 400, InvalidParameterException 400, ServerException 500, TargetNotFoundException 400). The model documents the input member as "You can specify up to 10 custom attributes for each resource. You can specify up to 10 attributes in a single call." and documents AttributeLimitExceededException as "You can apply up to 10 custom attributes for each resource. You can view the attributes of a resource with ListAttributes. You can remove existing attributes on a resource with DeleteAttributes." So on real AWS: a single PutAttributes carrying 10 attributes against one `container-instance` targetId succeeds; a further PutAttributes adding an 11th custom attribute to that same targetId is refused with HTTP 400, error code `AttributeLimitExceededException` and the recorded message `Attribute per target limit exceeded.` (that message is fixed text, with no variable part); the refused attribute is NOT applied, so ListAttributes(targetType="container-instance") still reports exactly the 10 custom attributes for that targetId; and after DeleteAttributes frees exactly one slot, the identical PutAttributes call then succeeds and ListAttributes converges to 10 custom attributes again with the deleted name absent. Reaching any of that requires a `container-instance` target that ECS itself validated through a real registration: RegisterContainerInstance demands an AWS-signed EC2 instance identity document, so the target is obtained by launching an EC2 host from the ECS-optimized AMI with `ECS_CLUSTER=<cluster>` in its user data and waiting for its agent to self-register, after which ListContainerInstances/DescribeContainerInstances report a container instance whose `ec2InstanceId` is that instance.

Emulator: the check never reaches a single one of those assertions. It is reported `[mismatch] ... Error: Check timed out after 180.0s`. The only waiting step before the first PutAttributes is the bounded poll for the launched EC2 host to appear in ListContainerInstances/DescribeContainerInstances with the matching `ec2InstanceId`, so the emulator never turns an EC2 instance launched with ECS_CLUSTER user data into a container instance ECS will accept as a PutAttributes target; the whole 180s fixture budget is spent in that poll. Independently, the operation under test cannot answer correctly even if the target were obtainable: services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/put_attributes.rs is still the scaffold whose handle() returns `Scaffolded::not_implemented("PutAttributes")` (HTTP 501 NotImplemented), so no per-target ceiling exists and no AttributeLimitExceededException can be produced.

Why: a client that uses container-instance attributes for placement — writing custom attributes and relying on the documented 10-per-resource ceiling to size its own attribute schema — gets no refusal where AWS refuses and no registered target to write to at all. Code that catches AttributeLimitExceededException to prune attributes via DeleteAttributes never sees it, so against the emulator it either hangs waiting for a container instance that never registers, or (once a target exists) sees 501 NotImplemented, a status and code AWS never returns for this operation and which no SDK error handler maps to a retry or a prune. Attribute-driven placement constraints therefore cannot be modelled or tested at all.

Fix: two components. (1) The ECS container-instance registration/state path — the agent-plane flow that admits an EC2 instance launched with ECS_CLUSTER user data as an ACTIVE, agent-connected container instance discoverable through ListContainerInstances/DescribeContainerInstances by its ec2InstanceId. Related open issues describe this same host-registration gap for OTHER checks and do not carry this check's replay key: 20261109T083521Z-ecs-ec2-instances-launched-with-ecs-cluster-user.md, 20261107T080429Z-ecs-2-net-new-causes-no-aws-signed.md, 20261109T013920Z-ecs-4-net-new-causes-ec2-launched-hosts.md, 20261109T061533Z-ecs-5-net-new-causes-ec2-hosts-never.md. (2) The PutAttributes handler and its validation — the per-target attribute ceiling and the AttributeLimitExceededException refusal, plus the DeleteAttributes/ListAttributes state it reads. The 501 stub itself is already named by 20261029T175650Z-ecs-7-net-new-root-causes-in-state.md; what is NOT covered anywhere is the 10-per-target ceiling behaviour and its refusal message, which is what this check asserts.

Traces: ecs.put-attributes-exceeding-limit-rejected.

## Evidence
Failing check: [mismatch] ecs.put-attributes-exceeding-limit-rejected
Suite detail: `Error: Check timed out after 180.0s`

Expected (AWS), in order, within the fixture budget:
1. CreateCluster, then an EC2 host launched from the ECS-optimized AMI with `ECS_CLUSTER=<cluster>` user data appears in ListContainerInstances and DescribeContainerInstances with `ec2InstanceId` equal to the launched instance id.
2. PutAttributes(cluster=<cluster>, attributes=[10 x {name, targetId=<containerInstanceArn>, value}]) -> HTTP 200.
3. PutAttributes of one more custom attribute on the SAME targetId -> HTTP 400, Error.Code `AttributeLimitExceededException`, Error.Message `Attribute per target limit exceeded.` (fixed text; no request- or state-derived parts).
4. ListAttributes(cluster=<cluster>, targetType="container-instance") -> the rejected attribute name is absent and exactly 10 custom attributes remain for that targetId.
5. DeleteAttributes removes one attribute; the identical PutAttributes from step 3 then -> HTTP 200, and ListAttributes converges to exactly 10 custom attributes with the deleted name absent.

Actual (emulator): none of steps 2-5 is ever attempted. The check is killed at the 180s budget inside the bounded poll of step 1 — no container instance is ever reported for the launched EC2 instance, so PutAttributes is never called with a valid `container-instance` target. Static corroboration of the second component: services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/put_attributes.rs lines 4-6 carry the `SCAFFOLDED, NOT IMPLEMENTED` marker and line 180 returns `Err(crate::scaffolded::Scaffolded::not_implemented("PutAttributes"))`, i.e. HTTP 501 `NotImplemented` — a status/code AWS never returns for PutAttributes, and one that cannot carry AttributeLimitExceededException.

## Steps to Reproduce
1. Replay the behavioral check `ecs.put-attributes-exceeding-limit-rejected` against the emulator.
2. Equivalently, by hand: ecs CreateCluster(clusterName=<c>); create an IAM instance role with AmazonEC2ContainerServiceforEC2Role and an instance profile; ec2 RunInstances of the ECS-optimized AMI (ssm GetParameter /aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id) into a public subnet with UserData `#!/bin/bash\necho ECS_CLUSTER=<c> >> /etc/ecs/ecs.config`.
3. Poll ecs ListContainerInstances(cluster=<c>) and DescribeContainerInstances for an entry whose ec2InstanceId is the launched instance. Observe it never appears; the poll runs to its deadline (the suite kills the check at 180s).
4. To exercise the second component directly, call ecs PutAttributes(cluster=<c>, attributes=[{name:"attr-0", value:"v", targetId:<any container-instance arn>}]). Observe HTTP 501 `NotImplemented` instead of a 200 (or a modeled 400 TargetNotFoundException/AttributeLimitExceededException).
5. Expected instead: after 10 attributes are applied to one targetId, an 11th PutAttributes on that targetId is refused HTTP 400 `AttributeLimitExceededException` with message `Attribute per target limit exceeded.`, is not applied, and succeeds once DeleteAttributes frees a slot.
