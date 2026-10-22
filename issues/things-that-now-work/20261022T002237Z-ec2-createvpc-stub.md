Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecr__lambda__logs.refarch-batch-job-fails-before-any-container-on-empty-repository cloudformation__cloudwatch__ecr__ecs__elbv2__logs.cfn-nested-stack-microservice-platform-workflow cloudwatch__ecr__ecs.cloudwatch-metric-filter-on-ecs-log-group-triggers-alarm cloudwatch__ecr__ecs.deploy-cfn-alarm-on-ecs-service-created-and-active cloudwatch__ecr__ecs.deploy-cfn-ecs-cluster-service-with-ecr-image-and-cloudwatch-logs cloudwatch__ecr__ecs.deploy-cfn-ecs-service-fails-without-iam-trust-policy-rejected cloudwatch__ecr__ecs.deploy-cfn-ecs-task-role-trust-policy-correctly-attached cloudwatch__ecr__ecs__logs.cli-ecs-task-log-delivery-blocked-without-permission-rejected cloudwatch__ecr__ecs__logs.cloudwatch-logs-metric-filter-creates-alarm-from-ecs-logs cloudwatch__ecr__ecs__logs.deploy-cloudwatch-alarm-on-ecs-service-from-template cloudwatch__ecr__ecs__logs.deploy-ecs-fargate-stack-with-ecr-and-log-group cloudwatch__ecr__ecs__logs.deploy-stack-fails-without-log-group-resource-policy cloudwatch__ecr__ecs__logs.ecs-service-cpu-memory-metrics-published-to-cloudwatch ecr__ecs__elbv2.ecs-execution-role-trust-policy-allows-ecs-tasks ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch ecr__ecs__elbv2.ecs-service-create-rejects-nonexistent-role-arn ecr__ecs__elbv2.ecs-task-launch-fails-without-ecs-tasks-trust ecr__ecs__elbv2.elbv2-deregisters-targets-on-ecs-task-stop ecr__ecs__elbv2.elbv2-health-check-reflects-ecs-task-status ecr__ecs__elbv2.elbv2-listener-routes-only-to-registered-ecs-targets ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks ecr__ecs__logs.ecr-pull-through-cache-image-pulled-by-ecs-task ecr__ecs__logs.ecs-exec-role-without-ecr-auth-cannot-pull ecr__eks__logs.cli-eks-update-logging-config-workflow ecr__eks__logs.deploy-eks-cluster-without-logging-config-no-log-groups ecr__eks__logs.deploy-eks-nodegroup-with-ecr-readonly-policy ecr__eks__logs.eks-audit-log-streams-delivered-to-cloudwatch ecr__eks__logs.eks-cluster-log-group-naming-follows-convention-per-cluster ecr__eks__logs.eks-container-insights-metrics-logs-delivery ecr__eks__logs.eks-container-insights-without-agent-permission-no-delivery ecr__eks__logs.eks-disabled-log-type-produces-no-log-group ecr__eks__logs.eks-enable-cluster-logging-creates-log-group ecr__eks__logs.eks-fargate-execution-role-pulls-ecr-image ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch
Issue-Key: ec2|CreateVpc|NotImplemented
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# Implement ec2 CreateVpc: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpc

## Description
`CreateVpc` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

36 recorded unit(s) fail on their `CreateVpc` call, so each one unblocks only when `CreateVpc` works: `batch__ecr__lambda__logs.refarch-batch-job-fails-before-any-container-on-empty-repository`, `cloudformation__cloudwatch__ecr__ecs__elbv2__logs.cfn-nested-stack-microservice-platform-workflow`, `cloudwatch__ecr__ecs.cloudwatch-metric-filter-on-ecs-log-group-triggers-alarm`, `cloudwatch__ecr__ecs.deploy-cfn-alarm-on-ecs-service-created-and-active`, `cloudwatch__ecr__ecs.deploy-cfn-ecs-cluster-service-with-ecr-image-and-cloudwatch-logs`, `cloudwatch__ecr__ecs.deploy-cfn-ecs-service-fails-without-iam-trust-policy-rejected`, `cloudwatch__ecr__ecs.deploy-cfn-ecs-task-role-trust-policy-correctly-attached`, `cloudwatch__ecr__ecs__logs.cli-ecs-task-log-delivery-blocked-without-permission-rejected`, `cloudwatch__ecr__ecs__logs.cloudwatch-logs-metric-filter-creates-alarm-from-ecs-logs`, `cloudwatch__ecr__ecs__logs.deploy-cloudwatch-alarm-on-ecs-service-from-template`, `cloudwatch__ecr__ecs__logs.deploy-ecs-fargate-stack-with-ecr-and-log-group`, `cloudwatch__ecr__ecs__logs.deploy-stack-fails-without-log-group-resource-policy`, `cloudwatch__ecr__ecs__logs.ecs-service-cpu-memory-metrics-published-to-cloudwatch`, `ecr__ecs__elbv2.ecs-execution-role-trust-policy-allows-ecs-tasks`, `ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch`, `ecr__ecs__elbv2.ecs-service-create-rejects-nonexistent-role-arn`, `ecr__ecs__elbv2.ecs-task-launch-fails-without-ecs-tasks-trust`, `ecr__ecs__elbv2.elbv2-deregisters-targets-on-ecs-task-stop`, `ecr__ecs__elbv2.elbv2-health-check-reflects-ecs-task-status`, `ecr__ecs__elbv2.elbv2-listener-routes-only-to-registered-ecs-targets` (+16 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc.rs`. Replaying the corpus, the emulator refused the `CreateVpc` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVpc operation: operation CreateVpc is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call CreateVpc and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
