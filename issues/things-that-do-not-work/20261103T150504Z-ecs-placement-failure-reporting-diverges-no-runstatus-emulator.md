Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsnrt-traj-ec2-service-scaled-up-without-capacity-events
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsnrt-traj-ec2-service-scaled-up-without-capacity-events ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.availabilityZoneRebalancing
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.currentServiceRevisions[*].requestedTaskCount
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].desiredCount
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.events[*]
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.runStatus
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[*].autoScalingGroupProvider.managedDraining
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[*].autoScalingGroupProvider.managedScaling.instanceWarmupPeriod
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[*].autoScalingGroupProvider.managedScaling.maximumScalingStepSize
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[*].autoScalingGroupProvider.managedScaling.minimumScalingStepSize
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[*].autoScalingGroupProvider.managedScaling.scalingApproach
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[*].autoScalingGroupProvider.managedScaling.targetCapacity
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[*].tags
Issue-Key: ecs|DescribeCapacityProviders|2xx->2xx|$.capacityProviders[*].type
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.currentServiceRevisions[*]
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.deployments[*].launchType
Issue-Key: ecs|UpdateService|2xx->2xx|$.service.version
Issue-Related: 20261103T150504Z-ecs-listings-and-describes-never-reflect-deletion-or.md
Issue-Related: 20261103T150504Z-ecs-service-responses-drop-deployment-configuration-task-sets.md
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# ECS placement-failure reporting diverges: no runStatus, emulator-worded events, wrong service defaults

> _Note: this issue was identified against the code base at commit `34977689197ee7e7ec3a117f2c4937da4e6f0436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateService

## Description
Six net-new causes, one fault: when a service cannot place its tasks, the emulator reports the failure in its own words, omits the runStatus summary AWS sets, and gets the service's own defaults and counters wrong. Open issue 20261029T220901Z-ecs-capacity-provider-surface-omits-type-and-tags describes a related capacity-provider gap but carries none of these replay keys.

Cause 1 of 6: UpdateService omits runStatus and mis-words the placement-failure event (empty cluster). AWS: service.runStatus = 'Error.ClusterEmpty' and events[0].message is the fixed sentence '(service ' + the service name + ') was unable to place a task because no container instance met all of its requirements. The reason for failure is ' + the reason + '. For more information, see the Troubleshooting section of the Amazon ECS Developer Guide.' — the service name and the reason clause are the variable parts; 'The reason for failure is' is AWS's fixed connective. Smithy: Service.runStatus is a modeled member. Emulator: runStatus is absent and the event reads '... all of its requirements. Reason: No Container Instances were found in your cluster. ...' — 'Reason:' instead of 'The reason for failure is'. Why: runStatus is the machine-readable way to classify why a service is stuck; without it a client must parse the event text, which the emulator also rewords, so both routes fail. Fix: the service state's placement-failure reporting (runStatus) and the event message template. Traces: ecs-ecsnrt-traj-ec2-service-scaled-up-without-capacity-events#task#11.

Cause 2 of 6: UpdateService reports the wrong availabilityZoneRebalancing default and the wrong desired/requested counts, and emits an event AWS has not emitted yet. AWS: availabilityZoneRebalancing 'DISABLED' when the client did not request it (Smithy: AvailabilityZoneRebalancing = [ENABLED, DISABLED]; the recording shows DISABLED for this service), deployments[0].desiredCount and currentServiceRevisions[0].requestedTaskCount 0 at this point, and no events entry. Emulator: 'ENABLED', counts 1, and a placement-failure event already present. Why: a client reading availabilityZoneRebalancing believes rebalancing is on when it is off, and one tracking desiredCount through a scale-up sees the counts move a step early. Fix: the service state's defaults and the scale accounting in UpdateService. Traces: ecs-ecsnrt-traj-ec2-service-scaled-up-without-capacity-events#task#5.

Cause 3 of 6: DescribeCapacityProviders omits most of the provider record. AWS: capacityProviders[0] carries type 'EC2_AUTOSCALING', tags [], and autoScalingGroupProvider.managedDraining 'ENABLED' plus managedScaling {instanceWarmupPeriod 300, maximumScalingStepSize 10000, minimumScalingStepSize 1, scalingApproach 'SCALING_POLICY', targetCapacity 100} — the AWS defaults applied when the client supplies no managedScaling block. Smithy: CapacityProvider.type -> CapacityProviderType, CapacityProvider.tags -> Tags. Emulator: type, tags and the whole managedScaling/managedDraining set are absent. Why: a client cannot tell an EC2 Auto Scaling provider from a managed-instances one, and reads 'no managed scaling' where AWS has defaults in force. Fix: the capacity-provider state's defaulting plus DescribeCapacityProviders' serializer. Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#task#12.

Cause 4 of 6: UpdateService onto a capacity-provider strategy loses a service revision, mis-counts, and bumps version. AWS: currentServiceRevisions holds two entries (the new and prior revision) with requestedTaskCount 0, deployments[1].launchType 'EC2' present, service.version 0, and no events entry yet. Emulator: one revision, counts 1, deployments[1].launchType absent, version 1, and a capacity-provider placement event already emitted whose text again uses its own wording. Why: a client diffing service revisions across a deployment sees only one, and version-based optimistic concurrency is off by one. Fix: the service-revision state and UpdateService's accounting. Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#task#15.

Cause 5 of 6: DescribeServices omits runStatus for the capacity-provider case and mis-words the event. AWS: runStatus 'Error.CapacityProviderEmpty' and the event message's fixed connective 'The reason for failure is No Container Instances were found in your capacity provider.' Emulator: no runStatus, and a message reading "Reason: The specified capacity provider '<name>' contains no container instances." — different connective and a different reason clause. The currentServiceDeployment ARN's trailing id is also a zero-padded counter rather than AWS's generated base62 id. Why: as in cause 1, neither the code nor the text lets a client classify the stall. Fix: the service state's runStatus/event derivation and the service-deployment id generation. Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#task#16.

Cause 6 of 6: the follow-up UpdateService repeats cause 5's divergences (no runStatus, reworded event, availabilityZoneRebalancing 'ENABLED' vs 'DISABLED', version 1 vs 0, requestedTaskCount/desiredCount 0 vs 1). Fix: as cause 5. Traces: ecs-ecsnrt-traj-zero-capacity-provider-strategy-repair#task#17.

## Evidence
Replay of trace set simparity-wire-aws-v1; all six clusters have empty owned_by and empty stub_path, status 200 on both sides.
Cause 1 — $.service.runStatus expected 'Error.ClusterEmpty', got absent; $.service.events[0].message expected '... requirements. The reason for failure is No Container Instances were found in your cluster. For more information, ...', got '... requirements. Reason: No Container Instances were found in your cluster. For more information, ...'.
Cause 2 — $.service.availabilityZoneRebalancing expected 'DISABLED', got 'ENABLED'; currentServiceRevisions[0].requestedTaskCount expected 0, got 1; deployments[0].desiredCount expected 0, got 1; events[0] 'extra (expected None)'.
Cause 3 — $.capacityProviders[0].type expected 'EC2_AUTOSCALING', tags expected [], autoScalingGroupProvider.managedDraining expected 'ENABLED', managedScaling.{instanceWarmupPeriod 300, maximumScalingStepSize 10000, minimumScalingStepSize 1, scalingApproach 'SCALING_POLICY', targetCapacity 100} — all 'missing'.
Cause 4 — $.service.currentServiceRevisions[1] missing; deployments[1].launchType expected 'EC2', got absent; version expected 0, got 1; events[0] 'extra'.
Cause 5 — $.services[0].runStatus expected 'Error.CapacityProviderEmpty', got absent; events[0] message connective differs; currentServiceDeployment trailing id expected a generated base62 id, got '0000000000000000002'; version expected 0, got 1.
Cause 6 — $.service.runStatus expected 'Error.CapacityProviderEmpty', got absent; availabilityZoneRebalancing expected 'DISABLED', got 'ENABLED'; requestedTaskCount/desiredCount expected 1, got 0; version expected 0, got 1.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Representative: fixture ecs-ecsnrt-traj-ec2-service-scaled-up-without-capacity-events, step task#11 — scale an EC2 service in an empty cluster and diff service.runStatus and service.events[0].message.
3. Replay each fixture#step in trace_ids and diff the service / capacityProviders body against the recorded response.
