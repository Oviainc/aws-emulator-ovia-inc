Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs.deploy-ecs-capacity-provider-round-trip-then-delete cloudformation__ecs.deploy-ecs-capacity-provider-with-nonexistent-asg-rejected cloudformation__ecs.deploy-ecs-cluster-and-capacity-provider-associations-round-trip-then-stack-delete cloudformation__ecs.deploy-ecs-cluster-capacity-provider-associations-unassociated-strategy-rejected cloudformation__ecs.deploy-ecs-service-round-trip-then-stack-delete cloudformation__ecs.deploy-ecs-task-definition-invalid-fargate-cpu-rejected cloudformation__ecs.deploy-ecs-taskset-and-primary-taskset-round-trip-then-stack-delete
Issue-Key: cloudformation__ecs.deploy-ecs-capacity-provider-round-trip-then-delete|run#8|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Key: cloudformation__ecs.deploy-ecs-capacity-provider-with-nonexistent-asg-rejected|run#10|AssertionError|assert status == "CREATE_COMPLETE", f"control stack ended {status!r}"
Issue-Key: cloudformation__ecs.deploy-ecs-cluster-and-capacity-provider-associations-round-trip-then-stack-delete|run#10|AssertionError|assert out["CPAssocClusterOut"] == cluster_name, (
Issue-Key: cloudformation__ecs.deploy-ecs-cluster-capacity-provider-associations-unassociated-strategy-rejected|run#10|AssertionError|assert got_good == ("ACTIVE", ["FARGATE"],
Issue-Key: cloudformation__ecs.deploy-ecs-service-round-trip-then-stack-delete|run#20|AssertionError|assert listed == [service_arn], f"ListServices(cluster={cluster_arn!r}) returned {listed!r}"
Issue-Key: cloudformation__ecs.deploy-ecs-task-definition-invalid-fargate-cpu-rejected|run#8|AssertionError|assert bad_stack["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__ecs.deploy-ecs-taskset-and-primary-taskset-round-trip-then-stack-delete|run#18|AssertionError|assert out["TaskSetRefOut"] == f"{cluster_name}|{service_name}|{taskset_id}", (
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# cloudformation__ecs: 5 net-new causes — ECS capacity-provider stacks roll back, cluster capacity-provider associations are inert with a wrong Ref, CFN-created services are invisible to ECS, an invalid Fargate cpu is not rejected, and TaskSet Ref is wrong

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 5: a stack whose only resource is a valid AWS::ECS::CapacityProvider ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE.
AWS: recorded behavior only for the CFN resource support; Smithy (ecs) CreateCapacityProvider accepts an autoScalingGroupProvider with an existing ASG ARN and answers 200 with status ACTIVE, so a stack declaring it reaches CREATE_COMPLETE and DescribeStacks reports StackStatus 'CREATE_COMPLETE'. The negative variant's *control* stack (a capacity provider over an ASG that does exist) must also reach CREATE_COMPLETE — only the nonexistent-ASG stack may roll back.
Emulator: `AssertionError: stack 'sim2real-cfncp-<id>' ended 'ROLLBACK_COMPLETE', expected 'CREATE_COMPLETE'` and `AssertionError: control stack ended 'ROLLBACK_COMPLETE'`. The provisioning of AWS::ECS::CapacityProvider fails during CREATE for a well-formed template, so the positive path is unreachable and the negative test has no control.
Why: a client cannot provision ECS capacity providers through CloudFormation at all, and because the control also rolls back, a rollback no longer means 'my template was wrong'.
Fix: the CloudFormation resource provider for AWS::ECS::CapacityProvider (property mapping onto ECS CreateCapacityProvider and the create/rollback path in the CFN engine).
Traces: cloudformation__ecs.deploy-ecs-capacity-provider-round-trip-then-delete, cloudformation__ecs.deploy-ecs-capacity-provider-with-nonexistent-asg-rejected.

Cause 2 of 5: AWS::ECS::ClusterCapacityProviderAssociations neither applies the association nor returns the documented Ref.
AWS: recorded behavior only. Ref on AWS::ECS::ClusterCapacityProviderAssociations returns the cluster name (the resource's physical id IS the cluster), and after CREATE_COMPLETE ECS DescribeClusters shows the named providers in `capacityProviders` and the declared `defaultCapacityProviderStrategy`. A strategy naming a provider that is not in the associations list is refused (Smithy: PutClusterCapacityProviders errors InvalidParameterException), so that stack must roll back and the cluster must stay unmodified.
Emulator: `AssertionError: Ref CPAssoc 'sim2real-cfnecs-cl-<id>-CPAssoc-<id>' != 'sim2real-cfnecs-cl-<id>'` — Ref yields a generated logical-id-derived name instead of the cluster name; and `AssertionError: control cluster reads ('ACTIVE', [], [])` — the cluster's capacityProviders and defaultCapacityProviderStrategy are both empty, so the association was never applied to ECS.
Why: a template that wires capacity providers to a cluster silently does nothing, and any Fn::Ref/Fn::Sub that depends on the association's physical id produces a name that is not a cluster.
Fix: the CloudFormation resource provider for AWS::ECS::ClusterCapacityProviderAssociations — physical-id selection (cluster name) and the call through to ECS PutClusterCapacityProviders.
Traces: cloudformation__ecs.deploy-ecs-cluster-and-capacity-provider-associations-round-trip-then-stack-delete, cloudformation__ecs.deploy-ecs-cluster-capacity-provider-associations-unassociated-strategy-rejected.

Cause 3 of 5: an AWS::ECS::Service that reached CREATE_COMPLETE is unknown to ECS.
AWS: Smithy (ecs) ListServices returns the serviceArns of the cluster's services; a service created by a CREATE_COMPLETE stack is one of them, and DescribeServices returns it.
Emulator: `AssertionError: ListServices(cluster='arn:aws:ecs:<region>:<acct>:cluster/sim2real-cfnecs-svc-cl-<id>') returned []` — the stack claims success but ECS has no such service.
Why: CREATE_COMPLETE is not a truthful signal. Every post-deploy step a client performs (scale, update, describe, delete by service name) fails with ServiceNotFoundException against a stack the emulator says succeeded.
Fix: the CloudFormation resource provider for AWS::ECS::Service must create the service in ECS state (or fail the stack), rather than recording a resource the owning service never sees.
Traces: cloudformation__ecs.deploy-ecs-service-round-trip-then-stack-delete.

Cause 4 of 5: an AWS::ECS::TaskDefinition with a Fargate-invalid cpu value is accepted and the stack reaches CREATE_COMPLETE.
AWS: Smithy (ecs) RegisterTaskDefinition refuses a FARGATE-compatible definition whose cpu is not one of the supported task CPU values with ClientException/InvalidParameterException (HTTP 400); cpu='100' is not a valid Fargate task size, so the stack rolls back and DescribeStacks reports 'ROLLBACK_COMPLETE'.
Emulator: `AssertionError: a Fargate task definition with cpu='100' ended 'CREATE_COMPLETE', expected 'ROLLBACK_COMPLETE'` — the invalid size passes validation end to end.
Why: a client gets CREATE_COMPLETE for a template real AWS rejects, so the invalid task size is discovered only in production. Validation that exists to fail fast is absent.
Fix: RegisterTaskDefinition's Fargate cpu/memory validation in the ECS crate, which the CFN task-definition provider must surface as a failed CREATE.
Traces: cloudformation__ecs.deploy-ecs-task-definition-invalid-fargate-cpu-rejected.

Cause 5 of 5: Ref on AWS::ECS::TaskSet returns a logical-id-derived name instead of the task set's identifier.
AWS: recorded behavior only. Ref on AWS::ECS::TaskSet returns the task set id, and the stack's own outputs join cluster|service|id; the check compares Ref against the Cluster|Service|Id triple it reads back from ECS DescribeTaskSets.
Emulator: `AssertionError: Ref TaskSet 'sim2real-cfnecs-ts-<id>-TaskSet-<id>' != Cluster|Service|Id sim2real-cfnecs-ts-cl-<id>|sim2real-cfnecs-ts-s-<id>|sim2real-cfnecs-ts-<id>-TaskSet-<id>` — the Ref value is a generated resource name, not the task set id ECS knows.
Why: any template that feeds a task set Ref into another resource (a CODE_DEPLOY primary task set, an output consumed by a deploy tool) passes an identifier ECS cannot resolve.
Fix: the CloudFormation resource provider for AWS::ECS::TaskSet — physical id must be the ECS task set id returned by CreateTaskSet.
Traces: cloudformation__ecs.deploy-ecs-taskset-and-primary-taskset-round-trip-then-stack-delete.

## Evidence
Cause 1: `AssertionError: stack 'sim2real-cfncp-<id>' ended 'ROLLBACK_COMPLETE', expected 'CREATE_COMPLETE'`; `AssertionError: control stack ended 'ROLLBACK_COMPLETE'`.
Cause 2: `AssertionError: Ref CPAssoc 'sim2real-cfnecs-cl-<id>-CPAssoc-<id>' != 'sim2real-cfnecs-cl-<id>'`; `AssertionError: control cluster reads ('ACTIVE', [], [])` (status, capacityProviders, defaultCapacityProviderStrategy).
Cause 3: `AssertionError: ListServices(cluster='arn:aws:ecs:...:cluster/sim2real-cfnecs-svc-cl-<id>') returned []` after CREATE_COMPLETE.
Cause 4: `AssertionError: a Fargate task definition with cpu='100' ended 'CREATE_COMPLETE', expected 'ROLLBACK_COMPLETE'`.
Cause 5: `AssertionError: Ref TaskSet 'sim2real-cfnecs-ts-<id>-TaskSet-<id>' != Cluster|Service|Id <cluster>|<service>|<id>`.

## Steps to Reproduce
Cause 1: CreateStack with a single AWS::ECS::CapacityProvider over an existing ASG; poll DescribeStacks.
Cause 2: CreateStack with AWS::ECS::Cluster + AWS::ECS::ClusterCapacityProviderAssociations; compare Ref to the cluster name and read ecs DescribeClusters include=['ATTACHMENTS'] for capacityProviders/defaultCapacityProviderStrategy.
Cause 3: CreateStack with AWS::ECS::Cluster + AWS::ECS::TaskDefinition + AWS::ECS::Service; after CREATE_COMPLETE call ecs ListServices(cluster=<stack cluster>).
Cause 4: CreateStack with an AWS::ECS::TaskDefinition declaring RequiresCompatibilities=[FARGATE], Cpu='100', Memory='512'; poll DescribeStacks.
Cause 5: CreateStack with AWS::ECS::TaskSet (+ PrimaryTaskSet); compare Ref against ecs DescribeTaskSets id.
