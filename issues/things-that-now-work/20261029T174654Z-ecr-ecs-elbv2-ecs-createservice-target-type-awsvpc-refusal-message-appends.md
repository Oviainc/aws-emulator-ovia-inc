Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch
Issue-Key: ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7

# ECS CreateService target-type/awsvpc refusal message appends a remediation sentence AWS does not emit

> _Note: this issue was identified against the code base at commit `b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
ONE net-new root cause.

Cause 1 of 1 — CreateService with an "instance"-type target group and an awsvpc task definition is correctly refused InvalidParameterException, but the message carries an extra trailing sentence.
AWS: ecs CreateService models InvalidParameterException for this incompatibility; the message text is recorded behavior only. The recording's template is "The provided target group <targetGroupArn> has target type instance, which is incompatible with the awsvpc network mode specified in the task definition." — fixed text apart from the target group ARN, which is taken from the request's loadBalancers[].targetGroupArn. The sentence ends there.
Emulator: the same sentence followed by " Use a target group whose target type is ip." — a remediation sentence that is not part of AWS's message for this error class.
Why: the refusal decision and status are right, but the message is a contract surface: a client or test comparing the message to AWS's text (exact match, or a trailing-anchor match) sees a mismatch, and operators learn a sentence AWS never emits. The effect-absent leg of the check (no service created) passed, so this is purely message construction.
Fix: the ECS CreateService validation path that builds the target-group/network-mode incompatibility message — emit AWS's single sentence with no appended advice.
Traces: ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch.

## Evidence
Replay output, [mismatch] ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch, assertion at check line 46:
expected message 'The provided target group arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/sim2real-tg-.../... has target type instance, which is incompatible with the awsvpc network mode specified in the task definition.'
got 'The provided target group arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/sim2real-tg-.../... has target type instance, which is incompatible with the awsvpc network mode specified in the task definition. Use a target group whose target type is ip.'
The error code InvalidParameterException matched. The ARN is a per-run value from the request; the divergence is the appended fixed sentence.

## Steps to Reproduce
Create a VPC/subnets/security group, an ECS cluster, and a task definition with networkMode=awsvpc. Create an ALB target group with TargetType=instance. Call ecs CreateService(cluster, serviceName, taskDefinition=<awsvpc task def>, desiredCount=0, launchType=FARGATE, networkConfiguration.awsvpcConfiguration=..., loadBalancers=[{targetGroupArn:<instance-type tg>, containerName:"app", containerPort:80}]) and read Error.Message.
