Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.cloudwatch-alarm-on-ecs-service-running-task-count cloudwatch__ecr__ecs.ecs-cluster-container-insights-metrics-visible-in-cloudwatch cloudwatch__ecr__ecs.ecs-metrics-absent-without-container-insights-enabled cloudwatch__ecr__ecs.ecs-service-cpu-utilization-published-to-cloudwatch cloudwatch__ecr__ecs.ecs-service-memory-utilization-published-to-cloudwatch cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing ecr__ecs__elbv2.deploy-cfn-negative-missing-service-target-group-binding ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret ecr__eks__logs.eks-nodegroup-pulls-private-ecr-image
Issue-Key: ec2|DescribeVpcs|NotImplemented
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# Implement ec2 DescribeVpcs: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
`DescribeVpcs` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpcs.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `DescribeVpcs` call, so each one unblocks only when `DescribeVpcs` works: `cloudwatch__ecr__ecs.cloudwatch-alarm-on-ecs-service-running-task-count`, `cloudwatch__ecr__ecs.ecs-cluster-container-insights-metrics-visible-in-cloudwatch`, `cloudwatch__ecr__ecs.ecs-metrics-absent-without-container-insights-enabled`, `cloudwatch__ecr__ecs.ecs-service-cpu-utilization-published-to-cloudwatch`, `cloudwatch__ecr__ecs.ecs-service-memory-utilization-published-to-cloudwatch`, `cloudwatch__ecr__ecs__logs.deploy-ecs-service-rollback-on-missing-ecr-image`, `ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing`, `ecr__ecs__elbv2.deploy-cfn-negative-missing-service-target-group-binding`, `ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change`, `ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull`, `ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag`, `ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret`, `ecr__eks__logs.eks-nodegroup-pulls-private-ecr-image`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpcs.rs`. Replaying the corpus, the emulator refused the `DescribeVpcs` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeVpcs operation: operation DescribeVpcs is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call DescribeVpcs and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
