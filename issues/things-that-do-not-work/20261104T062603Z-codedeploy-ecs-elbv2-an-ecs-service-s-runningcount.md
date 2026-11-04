Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy__ecs__elbv2.ecs-blue-green-deployment-moves-listener-from-blue-to-green
Issue-Key: codedeploy__ecs__elbv2.ecs-blue-green-deployment-moves-listener-from-blue-to-green|_poll_until#6|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# codedeploy__ecs__elbv2: an ECS service's runningCount never reaches desiredCount, so a CodeDeploy blue/green deployment never shifts the listener (1 cause)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: the ECS service that backs a blue/green deployment never converges, so the deployment cannot progress.

AWS: a CODE_DEPLOY-controlled ECS service with desiredCount=1 reaches runningCount=1; CodeDeploy then creates the green task set, waits for its targets to become healthy and shifts the listener's default action from the blue target group to the green one.

Emulator: the check's wait for `service <name> runningCount==1` exhausts the run budget — the service never reports a running task, so the listener is never moved and the deployment is never graded.

Why: the whole blue/green path (task sets, traffic shifting, rollback on an unhealthy green) is untestable; a client using CodeDeploy for ECS gets a deployment that is accepted and never advances.

Fix: the ECS service scheduler/task runtime (runningCount must converge to desiredCount) and, once it does, the CodeDeploy ECS deployment's listener-shift step.

Traces: codedeploy__ecs__elbv2.ecs-blue-green-deployment-moves-listener-from-blue-to-green.

## Evidence
Expected (AWS): DescribeServices reports runningCount=1, then the listener's forward action moves from the blue target group to the green one.
Actual (emulator): `TimeoutError: service sim2real-xsvc-svc-<suffix> runningCount==1: the run budget ended first; not graded` (the service name is per-run).

## Steps to Reproduce
1. Create an ALB with blue and green target groups and a listener forwarding to blue.
2. CreateService with deploymentController.type=CODE_DEPLOY, desiredCount=1, loadBalancers pointing at blue.
3. Poll DescribeServices for runningCount==1 -> never reached; the subsequent CodeDeploy CreateDeployment never shifts the listener.
