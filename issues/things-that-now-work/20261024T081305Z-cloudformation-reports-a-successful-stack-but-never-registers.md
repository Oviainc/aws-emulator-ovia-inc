Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__iam__s3.e2e-eventbridge-ecs-task-trigger-debugging-lifecycle cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api
Issue-Key: ecs__events__iam__s3|DescribeRule|ResourceNotFoundException
Issue-Key: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api|_cdk#15|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# CloudFormation reports a successful stack but never registers the declared AWS::Events::Rule or AWS::SSM::Parameter, breaking EventBridge estates and the CDK bootstrap path

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two resource types declared in a stack are reported as provisioned but do not exist in the owning service afterwards. Two causes; both must be closed before this issue is done. (Service attribution: these checks' ids carry composite service groups — ecs__events__iam__s3 and cloudformation__ecs — and the diverging operation in both is CloudFormation's resource provisioning, so the issue is filed under cloudformation.)

Cause 1 of 2: an AWS::Events::Rule declared in a stack is never created in EventBridge.
AWS: CreateStack provisions each declared resource before the stack reaches CREATE_COMPLETE and before its Outputs resolve; a stack that declares an AWS::Events::Rule (here a scheduled rule with an ECS RunTask target) leaves a rule that events:DescribeRule returns on the `default` bus with the declared ScheduleExpression and State ENABLED (recorded behavior: ScheduleExpression "cron(05 23 27 * ? *)", State "ENABLED").
Emulator: the stack settles with all five expected Outputs present (including EventBridgeRuleName), but DescribeRule on that name answers HTTP 400 ResourceNotFoundException "Rule <name> does not exist on event bus default."
Why: a client that deploys an event-driven estate by template believes the trigger exists; nothing ever fires, and every later phase (fault injection, debugging, teardown) operates on a resource that was never created. A stack status of CREATE_COMPLETE stops meaning "the declared resources exist".
Fix: the CloudFormation resource provisioner's AWS::Events::Rule handler (the CFN crate's resource-type dispatch — it must call the EventBridge PutRule/PutTargets path and fail the resource when it cannot).
Traces: ecs__events__iam__s3.e2e-eventbridge-ecs-task-trigger-debugging-lifecycle (phase 1/4 deploy).

Cause 2 of 2: the AWS::SSM::Parameter declared by the CDK bootstrap (CDKToolkit) template is never created, so every subsequent `cdk deploy` fails.
AWS: `cdk bootstrap` deploys the CDKToolkit stack, which declares /cdk-bootstrap/<qualifier>/version as an AWS::SSM::Parameter; after a successful bootstrap ssm:GetParameter on that name returns the version, which `cdk deploy` reads to validate the environment.
Emulator: `cdk bootstrap ... --qualifier <q> --toolkit-stack-name <name>` exits 0 (the check asserts rc==0 and passes), but the following `cdk deploy` exits 1 with "SSM parameter /cdk-bootstrap/<q>/version not found in aws://<account>/<region>. Has the environment been bootstrapped?" / ParameterNotFound — the stack reported success without materialising the parameter.
Why: no CDK-based workflow can run against the emulator: bootstrap claims success and deploy then refuses, so the failure is reported as a user error ("has the environment been bootstrapped?") rather than as the emulator's.
Fix: same CloudFormation resource provisioner, AWS::SSM::Parameter handler (create the parameter in the SSM store, and fail the resource rather than reporting success).
Traces: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api.

Related open issues, not duplicates: 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md and 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md cover Logs/CloudWatch/ECS and ELBv2/ECS resource types; AWS::Events::Rule and AWS::SSM::Parameter are not named in either, and both checks above still fail after those filings.

## Evidence
Replay output:
- ecs__events__iam__s3.e2e-eventbridge-ecs-task-trigger-debugging-lifecycle [error] — _PhaseError: "[phase 1/4 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeRule operation: Rule sim2real-ebecs-e2e-<id>-MonthlyRule-<id> does not exist on event bus default." The stack-outputs assertion immediately before it passed, i.e. the stack settled with EventBridgeRuleName in its Outputs.
- cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api [mismatch] — after `_bootstrap()` asserted `cdk bootstrap` rc==0, "`cdk deploy` rc=1, expected 0: ... SsmParameterNotFound: SSM parameter /cdk-bootstrap/<qualifier>/version not found in aws://000000000000/us-west-2. Has the environment been bootstrapped? ... ParameterNotFound: Parameter /cdk-bootstrap/<qualifier>/version not found".
Expected: after a stack reaches a successful terminal state, every declared resource is readable through its own service API (events:DescribeRule returns the rule; ssm:GetParameter returns the bootstrap version parameter).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-run the two checks in trace_ids.
2. Cause 1: CreateStack with a template declaring an AWS::Events::Rule (ScheduleExpression cron(...), State ENABLED, an ECS RunTask target) and an Output exposing its name; wait for the stack to settle, then call events:DescribeRule with that name — ResourceNotFoundException.
3. Cause 2: run `cdk bootstrap aws://<account>/<region> --qualifier <q> --toolkit-stack-name <name> --bootstrap-kms-key-id AWS_MANAGED_KEY --ci` (exits 0), then `ssm:GetParameter /cdk-bootstrap/<q>/version` — ParameterNotFound; any `cdk deploy` with that qualifier then fails.
