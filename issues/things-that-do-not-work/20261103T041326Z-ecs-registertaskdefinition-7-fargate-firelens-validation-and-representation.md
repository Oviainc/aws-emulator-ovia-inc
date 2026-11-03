Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.ecsnrt-awsfirelens-driver-without-router-rejected ecs.ecsnrt-fargate-definition-with-gpu-or-host-path-rejected ecs.ecsnt-fargate-awslogs-task-definition-without-an-execution-role-rejected ecs.fargate-task-definition-with-awslogs-and-no-execution-role-rejected ecs.fargate-task-definition-pidmode-host-rejected ecs.escc-windows-fargate-task-definition-rejects-a-quarter-vcpu ecs.fargate-task-definition-vcpu-and-gb-strings-register-as-units ecs.fargate-task-definition-with-network-mode-omitted-derives-no-fargate-compatibility
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.ecsnrt-awsfirelens-driver-without-router-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ecs.ecsnrt-fargate-definition-with-gpu-or-host-path-rejected|_error#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: ecs.ecsnt-fargate-awslogs-task-definition-without-an-execution-role-rejected|_error#4|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ecs.fargate-task-definition-with-awslogs-and-no-execution-role-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ecs.fargate-task-definition-pidmode-host-rejected|_error#4|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: ecs.escc-windows-fargate-task-definition-rejects-a-quarter-vcpu|_error#3|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ecs.fargate-task-definition-vcpu-and-gb-strings-register-as-units|run#5|AssertionError|assert (ok["cpu"], ok["memory"], ok["revision"]) == ("1024", "2048", 1), (
Issue-Key: ecs.fargate-task-definition-with-network-mode-omitted-derives-no-fargate-compatibility|run#11|AssertionError|assert "networkMode" not in read_plain, (
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# ECS RegisterTaskDefinition: 7 Fargate/FireLens validation and representation divergences — invalid definitions register with 200 and declared values are rewritten

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskDefinition

## Description
RegisterTaskDefinition in the ECS emulator skips seven independent service-side rules: five cross-field refusals that real ECS answers with HTTP 400 ClientException, and two representation rules that decide what the stored revision reads back as. Each is a separate missing rule in the RegisterTaskDefinition handler's validation/normalisation, bundled here because they share one service. Seven causes, numbered below — all seven must be closed before this issue is done.

Cause 1 of 7: a container whose logConfiguration.logDriver is `awsfirelens` registers even when no container in the definition carries a firelensConfiguration (no log router).
AWS: Smithy: RegisterTaskDefinition.containerDefinitions -> ContainerDefinition.logConfiguration / .firelensConfiguration are both optional members, so the pairing is enforced by the service, not the model; the modeled client errors are ClientException (httpError 400) and InvalidParameterException (400). Recorded behavior: HTTP 400 ClientException with the message `When awsfirelens log driver is specified in log configuration, a firelens configuration object must be configured in one of the containers.` — that text is wholly fixed, it interpolates nothing from the request. The refusal also mints no revision: the family's latest revision stays where it was.
Emulator: returns 200 and registers the revision, so the check's `_error` helper raises "the call succeeded; real AWS rejects it, so there is no error code".
Why: a client that relies on ECS to catch the missing router ships a definition whose containers can emit no logs at all, and the failure only appears at task launch; a deploy pipeline asserting the refusal passes wrongly and the family's revision counter advances past what AWS would have stored.
Fix: the RegisterTaskDefinition handler's validation (the cross-container FireLens check), services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_task_definition.rs. NOTE: open issue 20261024T081305Z-ecs-is-overly-permissive-ten-validations-authorizations-aws.md describes the same rule as its Cause 3 under a different check (ecs.ecsdeploy-awsfirelens-log-driver-without-router-rejected); this check's replay key is not carried by that issue, so it is filed here — fixing one should close both.
Traces: ecs.ecsnrt-awsfirelens-driver-without-router-rejected.

Cause 2 of 7: a definition with requiresCompatibilities=["FARGATE"] registers although it declares a GPU resource requirement, and likewise registers when it mounts a host-path volume.
AWS: Smithy: RegisterTaskDefinition.requiresCompatibilities -> CompatibilityList, containerDefinitions -> ContainerDefinition.resourceRequirements -> ResourceRequirement.type (enum, includes GPU), volumes -> Volume.host -> HostVolumeProperties.sourcePath; errors ClientException 400 / InvalidParameterException 400. Recorded behavior: two distinct HTTP 400 ClientException refusals with fixed texts `Tasks using the Fargate launch type do not support GPU resource requirements.` and `Fargate compatible task definitions do not support sourcePath` — neither interpolates a request value. Both refusals leave the family with no revisions, while the identical containers requiring EC2 register as revision 1 with requiresCompatibilities ["EC2"].
Emulator: both registrations return 200.
Why: a Fargate definition that Fargate can never place is accepted, so capacity planning and negative tests both break and the error surfaces only when a task is started.
Fix: RegisterTaskDefinition validation (the Fargate feature-compatibility checks over resourceRequirements and volumes), same handler.
Traces: ecs.ecsnrt-fargate-definition-with-gpu-or-host-path-rejected.

Cause 3 of 7: a FARGATE definition whose container logs via the `awslogs` driver registers with no executionRoleArn.
AWS: Smithy: RegisterTaskDefinition.executionRoleArn is an optional String, so the requirement is service-side; errors ClientException 400 / InvalidParameterException 400. Recorded behavior: HTTP 400 ClientException with the fixed message `Fargate requires task definition to have execution role ARN to support log driver awslogs.` (no request values interpolated). No revision is minted — ListTaskDefinitions on the family stays empty — and the control cases prove the refusal is about the pairing: the same container with requiresCompatibilities=["EC2"]/networkMode=bridge registers revision 1 carrying the awslogs logConfiguration, and the same FARGATE definition WITH an executionRoleArn registers the next revision echoing that ARN.
Emulator: returns 200 for the FARGATE definition with no execution role.
Why: the task definition is accepted and then no container can write logs; worse, because the refused call is persisted, the subsequent legitimate registration lands on the wrong revision number, so clients pinning `family:N` resolve to the wrong definition.
Fix: RegisterTaskDefinition validation (the Fargate execution-role requirement for log drivers), same handler. Related open issue 20261102T091623Z-ecs-registertaskdefinition-does-not-expand-bare-role-names.md covers the execution-role requirement for ECR *images*, a different rule; this is the awslogs log-driver rule.
Traces: ecs.ecsnt-fargate-awslogs-task-definition-without-an-execution-role-rejected, ecs.fargate-task-definition-with-awslogs-and-no-execution-role-rejected.

Cause 4 of 7: a FARGATE/awsvpc definition declaring pidMode=host registers.
AWS: Smithy: RegisterTaskDefinition.pidMode -> PidMode enum (host|task); the Fargate restriction is service-side, with errors ClientException 400 / InvalidParameterException 400. Recorded behavior: HTTP 400 ClientException with the fixed message `Tasks using the Fargate launch type do not support pidMode 'host'. The supported value for pidMode is 'task'.` — the quoted `'host'` is the rejected request value, the rest is fixed text. No revision is minted; the identical definition with pidMode omitted registers as revision 1 and reads back with no pidMode member at all.
Emulator: returns 200 and mints the revision.
Why: a client gets a definition Fargate will refuse to run, and the revision numbering diverges from AWS.
Fix: RegisterTaskDefinition validation (the Fargate pidMode/ipcMode restriction), same handler.
Traces: ecs.fargate-task-definition-pidmode-host-rejected.

Cause 5 of 7: a Windows Fargate definition (runtimePlatform.operatingSystemFamily WINDOWS_SERVER_*) registers at cpu="256" / memory="512".
AWS: Smithy: RegisterTaskDefinition.cpu and .memory are free-form Strings and runtimePlatform -> RuntimePlatform.operatingSystemFamily is an enum, so the valid size table is enforced by the service; errors ClientException 400 / InvalidParameterException 400. The Fargate task-size table is per operating system — the Linux minimum (256/512) is not a valid Windows size. Recorded behavior: HTTP 400 ClientException with `No Fargate configuration exists for given values: <cpu> CPU, <memory> memory. See the Amazon ECS documentation for the valid values.` where `<cpu>` and `<memory>` are the cpu/memory values from the request and the surrounding text is fixed. No revision is left behind, and the 1024/2048 Windows control registers and round-trips cpu "1024", memory "2048", runtimePlatform as declared.
Emulator: returns 200 for 256/512 on WINDOWS_SERVER_2019_FULL.
Why: a Windows Fargate service built on that definition can never be placed, and a client validating sizes against the emulator learns the wrong table. Note the related open issue 20261024T081305Z (Cause 4) covers an unsupported *Linux* cpu/memory pair; the per-OS Windows table is a separate rule and is unenforced.
Fix: RegisterTaskDefinition validation (the per-operatingSystemFamily Fargate task-size table), same handler.
Traces: ecs.escc-windows-fargate-task-definition-rejects-a-quarter-vcpu.

Cause 6 of 7: cpu/memory supplied in unit form are stored verbatim instead of being normalised to CPU units and MiB.
AWS: Smithy: RegisterTaskDefinition.cpu/.memory are Strings; ECS accepts the vCPU/GB spellings and normalises them, so the registered revision reads back the numeric unit form. Recorded behavior: a definition sent with the vCPU and GB spellings registers as revision 1 whose taskDefinition.cpu is the CPU-unit string and .memory the MiB string (1 vCPU -> "1024", 2 GB -> "2048").
Emulator: echoes the request strings unchanged — got ('1 vCPU', '2 GB', 1) where ('1024', '2048', 1) is required.
Why: every consumer that does arithmetic on taskDefinition.cpu/.memory (autoscaling math, capacity planning, CloudFormation drift) parses a non-numeric string and either errors or silently reads 0; two definitions that AWS considers identical compare unequal.
Fix: RegisterTaskDefinition's input normalisation before the revision is stored (and the serializer that echoes it), same handler.
Traces: ecs.fargate-task-definition-vcpu-and-gb-strings-register-as-units.

Cause 7 of 7: a definition registered with networkMode omitted reads back carrying networkMode `bridge`.
AWS: Smithy: RegisterTaskDefinition.networkMode -> NetworkMode enum, not required and carrying no @default; an omitted networkMode stays absent from TaskDefinition on read even though ECS derives bridge-mode compatibility (EC2, EXTERNAL) from it. Recorded behavior: DescribeTaskDefinition/RegisterTaskDefinition on a definition with neither requiresCompatibilities nor networkMode returns a taskDefinition with NO networkMode member.
Emulator: materialises the default, so the member is present with value 'bridge' — "an omitted networkMode must stay omitted on read, got 'bridge'".
Why: clients distinguish "unset" from "explicitly bridge" (CloudFormation and CDK drift detection, and ECS's own compatibility derivation keyed on absence); inventing the member makes an unset definition look explicitly configured and produces spurious drift.
Fix: the RegisterTaskDefinition/DescribeTaskDefinition serializer and the stored revision's representation (do not default networkMode when it is absent), same handler plus the task-definition state shape.
Traces: ecs.fargate-task-definition-with-network-mode-omitted-derives-no-fargate-compatibility.

## Evidence
Replay output (all eight checks, verdict mismatch):
- ecs.ecsnrt-awsfirelens-driver-without-router-rejected — `register_task_definition(family=<fam>, containerDefinitions=[app_with_awsfirelens_only])` -> AssertionError "the call succeeded; real AWS rejects it, so there is no error code". Expected 400 ClientException `When awsfirelens log driver is specified in log configuration, a firelens configuration object must be configured in one of the containers.`
- ecs.ecsnrt-fargate-definition-with-gpu-or-host-path-rejected — `register_task_definition(requiresCompatibilities=["FARGATE"], networkMode="awsvpc", cpu="1024", memory="4096", containerDefinitions=[{resourceRequirements:[{type:GPU,value:1}]}])` -> "the call succeeded; real AWS rejects it, so there is no error". Expected ('ClientException', 'Tasks using the Fargate launch type do not support GPU resource requirements.'); the host-path variant expects ('ClientException', 'Fargate compatible task definitions do not support sourcePath').
- ecs.ecsnt-fargate-awslogs-task-definition-without-an-execution-role-rejected and ecs.fargate-task-definition-with-awslogs-and-no-execution-role-rejected — `register_task_definition(requiresCompatibilities=["FARGATE"], networkMode="awsvpc", cpu="256", memory="512", containerDefinitions=[{logConfiguration:{logDriver:"awslogs", ...}}])` with no executionRoleArn -> "the call succeeded; real AWS rejects it, so there is no error code". Expected ('ClientException', 'Fargate requires task definition to have execution role ARN to support log driver awslogs.') and `list_task_definitions(familyPrefix=<fam>)["taskDefinitionArns"] == []`.
- ecs.fargate-task-definition-pidmode-host-rejected — `register_task_definition(cpu="1024", memory="2048", networkMode="awsvpc", pidMode="host", requiresCompatibilities=["FARGATE"], ...)` -> "the call succeeded; real AWS rejects it, so there is no error". Expected ('ClientException', "Tasks using the Fargate launch type do not support pidMode 'host'. The supported value for pidMode is 'task'.").
- ecs.escc-windows-fargate-task-definition-rejects-a-quarter-vcpu — `register_task_definition(networkMode="awsvpc", requiresCompatibilities=["FARGATE"], cpu="256", memory="512", runtimePlatform={"operatingSystemFamily": "WINDOWS_SERVER_2019_FULL"}, ...)` -> "the call succeeded; real AWS rejects it, so there is no error code". Expected ('ClientException', 'No Fargate configuration exists for given values: 256 CPU, 512 memory. See the Amazon ECS documentation for the valid values.') with no revision left.
- ecs.fargate-task-definition-vcpu-and-gb-strings-register-as-units — AssertionError: "1 vCPU / 2 GB must register as revision 1 with 1024/2048; got ('1 vCPU', '2 GB', 1)".
- ecs.fargate-task-definition-with-network-mode-omitted-derives-no-fargate-compatibility — AssertionError: "an omitted networkMode must stay omitted on read, got 'bridge'".
In every refusal case the emulator answered HTTP 200 with the revision registered; expected HTTP 400 with a modeled ECS client error (ClientException / InvalidParameterException per the Smithy error list for RegisterTaskDefinition).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the eight checks listed in trace_ids (all under the `ecs` service heading).
3. Per cause, issue the RegisterTaskDefinition call shown in Evidence against the emulator and observe HTTP 200 with a revision minted instead of HTTP 400 with the quoted ClientException message; for causes 6 and 7, register the definition and DescribeTaskDefinition it, observing the un-normalised cpu/memory strings and the invented networkMode=bridge member.
