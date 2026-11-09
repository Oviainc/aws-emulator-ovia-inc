Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-capacity-provider-management
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-capacity-provider-management ecs-cluster-capacity-provider-lifecycle ecs-catalog-describe-capacity-providers
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[name="FARGATE"].tags
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[name="FARGATE"].type
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[name="FARGATE_SPOT"].tags
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[name="FARGATE_SPOT"].type
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.failures[arn="sim2real-probe-missing-provider"]
Issue-Key: ecs|CreateCapacityProvider|4xx->2xx|$.__type
Issue-Key: ecs|CreateCapacityProvider|4xx->2xx|$.message
Issue-Key: ecs|CreateCapacityProvider|4xx->2xx|status
Issue-Key: ecs|DescribeCapacityProviders|2xx->4xx|$.capacityProviders
Issue-Key: ecs|DescribeCapacityProviders|2xx->4xx|$.failures
Issue-Key: ecs|DescribeCapacityProviders|2xx->4xx|status
Issue-Commit: 20487886de73e0d49e1ee92e88e94f424cdb9e47

# ECS capacity-provider surface omits type and tags, returns no failures for unknown providers, and skips Auto Scaling group validation

> _Note: this issue was identified against the code base at commit `20487886de73e0d49e1ee92e88e94f424cdb9e47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCapacityProviders

## Description
Four net-new root causes in the capacity-provider surface. All four must be closed before submitting.

Cause 1 of 4: DescribeCapacityProviders (fixture ecs-capacity-provider-management, step task#2).
AWS: each CapacityProvider carries type and tags. Smithy: CapacityProvider.type -> CapacityProviderType enum, CapacityProvider.tags -> Tags (list @length 0..50). For the two built-in Fargate providers the recording shows type equal to the provider's own name class ('FARGATE' for the FARGATE provider, 'FARGATE_SPOT' for FARGATE_SPOT) and tags [] when untagged.
Emulator: 200 but both members are omitted for the built-in providers.
Why: a client that switches on capacityProvider.type() to tell a Fargate provider from an Auto Scaling or managed-instances one gets an absent value, and code reading tags() gets absent instead of an empty list.
Fix: the built-in capacity-provider seed data / the CapacityProvider serializer.

Cause 2 of 4: DescribeCapacityProviders (fixture ecs-capacity-provider-management, step task#3).
AWS: naming a capacity provider that does not exist does not fail the call — 200 with a failures entry {"arn": <the identifier as requested>, "reason": "The specified capacity provider does not exist. Specify a valid name or ARN and try again."}. The reason is fixed text; arn is the request's value. Smithy: DescribeCapacityProviders output carries failures (Failures).
Emulator: 200 with no failures entry for that identifier.
Why: the caller cannot tell an unknown provider from a provider it forgot to ask about — the unknown name simply vanishes from the answer.
Fix: DescribeCapacityProviders handler — emit a failure per unresolved identifier.

Cause 3 of 4: CreateCapacityProvider (fixture ecs-capacity-provider-management, step task#5).
AWS: when autoScalingGroupProvider names an Auto Scaling group that does not exist, the request is refused: HTTP 400, error code ClientException, message 'The specified Auto Scaling group does not exist. Specify a valid Auto Scaling group and try again.' (fixed text).
Emulator: HTTP 200 — the capacity provider is created against a nonexistent ASG.
Why: a negative test asserting the refusal passes wrongly, and the emulator's state then contains a capacity provider that could never exist in AWS, so later attach/scale behavior diverges.
Fix: CreateCapacityProvider's validation stage — resolve the ASG before creating state.

Cause 4 of 4: DescribeCapacityProviders (fixture ecs-catalog-describe-capacity-providers, step task#1).
AWS: a DescribeCapacityProviders call that names no provider and no cluster answers 200 and lists the account's built-in providers (capacityProviderArn arn:aws:ecs:<region>:<account>:capacity-provider/FARGATE and .../FARGATE_SPOT, name, status 'ACTIVE', tags [], type) with failures [].
Emulator: HTTP 400 — the unfiltered catalog call is rejected outright and no body is produced.
Why: every client that enumerates capacity providers before choosing one (console-style discovery, CDK/CFN capacity-provider resolution) fails at the first call.
Fix: DescribeCapacityProviders handler — treat absent capacityProviders/cluster as "list all" instead of a validation error.

Related open issue, which does not cover these causes: "ecs: 7 net-new root causes in state and derivation" (20261029T175650Z-ecs-7-net-new-root-causes-in-state.md) mentions DeleteCapacityProvider answering NotImplemented.

## Evidence
Replay of simparity-wire-aws-v1:
- DescribeCapacityProviders 200/200: $.capacityProviders[name="FARGATE"].type expected 'FARGATE', got absent; .tags expected [], got absent; same for FARGATE_SPOT (2 affected steps).
- DescribeCapacityProviders 200/200: $.failures[arn="<unknown-provider>"] expected {'arn': '<unknown-provider>', 'reason': 'The specified capacity provider does not exist. Specify a valid name or ARN and try again.'}, got None.
- CreateCapacityProvider: status expected 400, got 200; $.__type expected 'ClientException', got absent; $.message expected 'The specified Auto Scaling group does not exist. Specify a valid Auto Scaling group and try again.', got absent (2 affected steps).
- DescribeCapacityProviders: status expected 200, got 400; $.capacityProviders expected the two built-in providers (arn .../capacity-provider/FARGATE and .../FARGATE_SPOT, status 'ACTIVE', tags [], type), got absent; $.failures expected [], got absent.
Smithy (ECS 2014-11-13): DescribeCapacityProviders input {capacityProviders, cluster, include, maxResults, nextToken} (all optional), output {capacityProviders, failures, nextToken}; CapacityProvider {capacityProviderArn, name, cluster, status, autoScalingGroupProvider, managedInstancesProvider, updateStatus, updateStatusReason, tags, type}; errors include ClientException (400).

## Steps to Reproduce
1. Replay the wire corpus (replay_wire_corpus) and read the ecs capacity-provider clusters.
2. DescribeCapacityProviders with no arguments -> emulator 400 instead of 200 listing FARGATE and FARGATE_SPOT.
3. DescribeCapacityProviders capacityProviders=[FARGATE,FARGATE_SPOT] -> entries lack type and tags.
4. DescribeCapacityProviders capacityProviders=[<name-that-does-not-exist>] -> no failures entry is returned.
5. CreateCapacityProvider with autoScalingGroupProvider.autoScalingGroupArn naming a nonexistent ASG -> emulator 200 instead of 400 ClientException.
