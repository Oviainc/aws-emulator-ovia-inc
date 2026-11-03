Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecr__ecs.ecsdeploy-fargate-ephemeral-storage-outside-21-to-200-gib-rejected ec2__ecr__ecs.e2e-ecsdeploy-service-deployment-failure-diagnosis-lifecycle ec2__ecr__ecs.e2e-ecsnrt-diagnose-ecs-service-no-running-tasks-lifecycle ec2__ecr__ecs.ecsnrt-ec2-service-moved-to-fargate-and-back-by-an-empty-strategy-restores-its-launch-type
Issue-Key: ec2__ecr__ecs.ecsdeploy-fargate-ephemeral-storage-outside-21-to-200-gib-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ec2__ecr__ecs.e2e-ecsdeploy-service-deployment-failure-diagnosis-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: ec2__ecr__ecs.e2e-ecsnrt-diagnose-ecs-service-no-running-tasks-lifecycle|phase#8|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: ec2__ecr__ecs.ecsnrt-ec2-service-moved-to-fargate-and-back-by-an-empty-strategy-restores-its-launch-type|_poll#19|Timeout|time.sleep(interval_s)
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# ECS: Fargate ephemeral-storage bounds unenforced, services are not discoverable from their cluster, cluster capacity is never reported, and an empty capacity-provider strategy does not restore the EC2 launch type (4 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Four net-new root causes in the ECS control plane, replayed under the `ec2__ecr__ecs` suite heading.

Cause 1 of 4: Fargate ephemeral storage outside the permitted range is accepted.
AWS: recorded behavior only. A Fargate task definition's EphemeralStorage.SizeInGiB must be between 21 and 200 GiB; a value outside that range is refused and no revision is registered.
Emulator: the call SUCCEEDS and the revision is stored.
Why: a task definition AWS will never accept becomes the account's active revision, so the first real deployment fails at a point the emulator reported as healthy; and the bound is the only guard against a value the Fargate platform cannot provision.
Fix: the EphemeralStorage range validation in the RegisterTaskDefinition handler (Fargate compatibility branch).
Traces: ec2__ecr__ecs.ecsdeploy-fargate-ephemeral-storage-outside-21-to-200-gib-rejected

Cause 2 of 4: a created service is not discoverable through its cluster.
AWS: a service created with CreateService is listed by ListServices/DescribeServices on the cluster it was created in, and the cluster's activeServicesCount reflects it.
Emulator: after the deploy phase, no cluster in the account holds a service with the name that CreateService accepted - the service exists nowhere the API can reach it.
Why: every deployment workflow re-reads the service to watch its deployment roll out. A service that cannot be found after a successful create makes the whole observe-and-wait pattern impossible, and a client cannot tell "not created" from "created but invisible".
Fix: the cluster association written by the CreateService handler and the cluster scoping in ListServices/DescribeServices.
Traces: ec2__ecr__ecs.e2e-ecsdeploy-service-deployment-failure-diagnosis-lifecycle

Cause 3 of 4: a cluster never reports its capacity providers, default strategy or registered instances.
AWS: DescribeClusters reports the cluster's capacityProviders, defaultCapacityProviderStrategy and registeredContainerInstancesCount, and they change as providers are attached and instances register.
Emulator: the cluster reads back as `([], [], 0)` after the deploy phase that attached a provider and registered instances.
Why: these three fields are how a client decides where a task can be placed. All-empty means every capacity decision and every "why is nothing running?" diagnosis reads the same regardless of what was configured.
Fix: capacity-provider and container-instance registration in cluster state, plus the DescribeClusters serializer.
Traces: ec2__ecr__ecs.e2e-ecsnrt-diagnose-ecs-service-no-running-tasks-lifecycle

Cause 4 of 4: an empty capacity-provider strategy on UpdateService does not restore the EC2 launch type.
AWS: recorded behavior only. Moving a service to Fargate via a capacity-provider strategy and then calling UpdateService with an EMPTY capacityProviderStrategy restores the service's original launchType (EC2); the empty list is the documented way to clear the strategy.
Emulator: the service does not return to its EC2 launch type.
Why: clearing the strategy is the only path back from a Fargate migration. A client that follows it is left with a service whose launch type does not match the capacity it has, and tasks are placed against the wrong platform.
Fix: the capacityProviderStrategy / launchType reconciliation in the UpdateService handler (empty list must clear, not be ignored).
Traces: ec2__ecr__ecs.ecsnrt-ec2-service-moved-to-fargate-and-back-by-an-empty-strategy-restores-its-launch-type

## Evidence
Replayed failing checks under `### ec2__ecr__ecs` (none marked ALREADY FILED / OWNED by):

1. ec2__ecr__ecs.ecsdeploy-fargate-ephemeral-storage-outside-21-to-200-gib-rejected -> AssertionError: the call succeeded; real AWS rejects it, so there is no error code
2. ec2__ecr__ecs.e2e-ecsdeploy-service-deployment-failure-diagnosis-lifecycle -> AssertionError: [phase 2/4 solve] no cluster in the account holds a service named 'sim2real-ecsdeploy-svc-8182a9afc64b'
3. ec2__ecr__ecs.e2e-ecsnrt-diagnose-ecs-service-no-running-tasks-lifecycle -> AssertionError: [phase 1/5 deploy] the app cluster is (providers, default, instances) ([], [], 0)
4. ec2__ecr__ecs.ecsnrt-ec2-service-moved-to-fargate-and-back-by-an-empty-strategy-restores-its-launch-type -> mismatch verdict in the same group

For corroboration, the neighbouring `ec2__ecs__elbv2` group's cluster assertion (OWNED by 20261102T085002Z-ec2-ecs-elbv2-3-net-new-ecs-root.md) shows the same all-zero cluster shape: registeredContainerInstancesCount 0, activeServicesCount 0, capacityProviders [], defaultCapacityProviderStrategy [].

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; the four checks appear under `### ec2__ecr__ecs`, unmarked.
2. Standalone repros:
   - RegisterTaskDefinition with requiresCompatibilities=['FARGATE'] and ephemeralStorage={'sizeInGiB': 10} (and again with 250): both must be refused.
   - CreateCluster, CreateService in it, then ListServices(cluster=<cluster>) and DescribeServices: the service must be returned and activeServicesCount must include it.
   - CreateCluster with capacityProviders and a defaultCapacityProviderStrategy, register container instances, then DescribeClusters: capacityProviders, defaultCapacityProviderStrategy and registeredContainerInstancesCount must all be non-empty/non-zero.
   - Create an EC2-launch-type service, UpdateService with a Fargate capacityProviderStrategy, then UpdateService with capacityProviderStrategy=[]: DescribeServices must report launchType EC2 again.
