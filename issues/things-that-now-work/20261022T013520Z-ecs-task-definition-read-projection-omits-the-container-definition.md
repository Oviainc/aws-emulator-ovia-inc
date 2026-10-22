Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__servicediscovery.ecs-task-definition-derives-compatibilities-beyond-what-was-required
Issue-Key: ecs__servicediscovery.ecs-task-definition-derives-compatibilities-beyond-what-was-required|run#15|KeyError|assert definition[field] == [], (
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Task-definition read projection omits the container-definition collections AWS materialises as empty lists (environment, mountPoints, volumesFrom, systemControls)

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTaskDefinition

## Description
Cause 1 of 1 — a registered container definition comes back without the empty collection members real AWS always materialises.
AWS: recorded behavior only. The ECS Smithy model declares ContainerDefinition.environment, mountPoints, volumesFrom and systemControls as plain lists with no @default, so the model does not state the projection; the recording does: RegisterTaskDefinition with a single container that declares none of these, read back through DescribeTaskDefinition, returns each of the four members PRESENT as an empty list ([]) rather than omitting the key. The same recording shows logConfiguration stays ABSENT when none was declared (no default awslogs driver is invented), and the derived members in the same response are already correct in this emulator: requiresCompatibilities stays exactly ['EC2'], compatibilities is derived as ['EC2','EXTERNAL'] with FARGATE correctly absent for bridge networking, networkMode='bridge', revision=1, status='ACTIVE', the ARN ends ':<family>:1', and portMappings preserves hostPort 0.
Emulator: the containerDefinitions[0] structure returned by DescribeTaskDefinition has no 'environment' key at all (the check raises KeyError: 'environment' on the first of the four), i.e. empty collections are dropped from the response instead of serialized as [].
Why: clients and IaC tools read these members unconditionally — td['containerDefinitions'][0]['environment'] is a KeyError against the emulator and a valid [] against AWS, so a register-then-describe round trip crashes callers or produces false drift on every diff of a task definition that declares no env vars, mounts, volumesFrom or sysctls.
Fix: the task-definition read/serialization projection for ECS (the DescribeTaskDefinition handler and the shared container-definition projection it shares with RegisterTaskDefinition) must materialise these list members as empty lists rather than omitting them, while leaving absent-by-design members such as logConfiguration absent.
Traces: ecs__servicediscovery.ecs-task-definition-derives-compatibilities-beyond-what-was-required (the check's own SERVICES is ['ecs']; it fails only on this ECS read projection, after every other assertion in it passes).

## Evidence
Suite: simparity-behavioral-aws-v1.
[error] ecs__servicediscovery.ecs-task-definition-derives-compatibilities-beyond-what-was-required
  File check_ecs__servicediscovery_ecs_task_definition_derives_compatibilities_beyond_what_was_required.py, line 124, in run
    assert definition[field] == [], (...)   # for field in ('environment','mountPoints','volumesFrom','systemControls')
  KeyError: 'environment'

The failure occurs AFTER the assertions on requiresCompatibilities, compatibilities ([EC2, EXTERNAL], no FARGATE), networkMode, revision/status, taskDefinitionArn suffix and portMappings (hostPort 0) have all passed, so the only divergence is the omission of the four empty collection members. Expected: each member present and equal to []. Actual: key absent from the returned container definition.

## Steps to Reproduce
1. Boot the emulator and point a botocore ecs client at it.
2. ecs.register_task_definition(family=F, networkMode='bridge', requiresCompatibilities=['EC2'], containerDefinitions=[{'name':'app','image':'public.ecr.aws/docker/library/nginx:latest','essential':True,'cpu':128,'memory':128,'portMappings':[{'containerPort':80,'hostPort':0,'protocol':'tcp'}]}]) — declare no environment, mountPoints, volumesFrom or systemControls.
3. td = ecs.describe_task_definition(taskDefinition=F)['taskDefinition'].
4. Read td['containerDefinitions'][0]['environment'] (then 'mountPoints', 'volumesFrom', 'systemControls'): the emulator raises KeyError; real AWS returns [] for each.
5. Confirm logConfiguration is still absent (it is, and must stay so).
6. Re-replay the check id in trace_ids to confirm the fix.
