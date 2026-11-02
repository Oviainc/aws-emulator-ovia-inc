Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle
Issue-Key: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle|phase#8|_PhaseError|raise _PhaseError(
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# cloudwatch__ec2__elbv2__opensearch: a deployed stack's Outputs entries carry no OutputValue, so the ALB/OpenSearch diagnosis lifecycle cannot start

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: CloudFormation stack Outputs are returned without their values.
AWS: once a stack reaches CREATE_COMPLETE, DescribeStacks returns each declared output as an Output structure carrying BOTH OutputKey and OutputValue (plus optional Description/ExportName); a resolved output never lacks OutputValue. Recorded behavior only — the check reads the stack's outputs into a dict and fails on the missing member.
Emulator: the deploy phase raises KeyError 'OutputValue' while mapping the stack's outputs, i.e. at least one Output element is returned with an OutputKey and no OutputValue. Nothing downstream can run: the lifecycle's remaining four phases (setup, solve, cleanup, destroy) never execute, because every one of them addresses the estate through the stack's outputs (AlarmName, AlbDnsName, VpcId, the domain's subnet).
Why: a client that deploys infrastructure and then reads the stack's outputs to find what it just created gets a key with no value and crashes; stack outputs are the only contract between a template and its consumers.
Fix: the CloudFormation DescribeStacks output projection / stack-output resolution (populate OutputValue when the resource attribute resolves, and do not emit a resolved output without one). Adjacent to the open 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never, which reports stacks completing without creating their resources; this issue is the Output serialization itself and is not keyed to this check.
Traces: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle (phase 1/5 deploy).

## Evidence
Expected: every Output of the CREATE_COMPLETE stack to carry OutputKey and OutputValue, so the check's outputs mapping yields AlarmName/AlbDnsName/VpcId etc.
Actual: "[phase 1/5 deploy] KeyError: 'OutputValue'" — the deploy phase aborts while reading DescribeStacks outputs, and phases setup/solve/cleanup/destroy never run.

## Steps to Reproduce
Replay the behavioral suite and re-run cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle. Equivalently: CreateStack with the check's deployments/alb-opensearch-stale-targets/template.json, wait for CREATE_COMPLETE, then call DescribeStacks and inspect Stacks[0].Outputs — each element must have both OutputKey and OutputValue.
