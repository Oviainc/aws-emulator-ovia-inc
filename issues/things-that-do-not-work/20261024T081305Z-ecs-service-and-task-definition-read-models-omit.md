Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.escc-platform-family-is-derived-from-the-task-definition-os ecs.escc-custom-deployment-and-network-settings-round-trip ecs.escc-latest-platform-version-resolves-per-os-on-the-deployment ecs.escc-ec2-service-omits-fargate-only-fields ecs.escc-fargate-service-omitted-settings-read-back-as-defaults ecs.escc-service-tags-only-returned-with-include-tags ecs.ecsnrt-gpu-host-volume-task-definition-derives-no-fargate-compatibility ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults ecs.default-capacity-providers-active ecs.escc-network-config-update-starts-a-new-primary-deployment
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.escc-platform-family-is-derived-from-the-task-definition-os|run#7|AssertionError|assert got == {k: (v, v) for k, v in want.items()}, (
Issue-Key: ecs.escc-custom-deployment-and-network-settings-round-trip|run#15|AssertionError|assert s.get("platformFamily") == "Windows-Server-2019-Full", (
Issue-Key: ecs.escc-latest-platform-version-resolves-per-os-on-the-deployment|run#5|AssertionError|assert got == {"linux": ("LATEST", "1.4.0"), "windows": ("LATEST", "1.0.0")}, (
Issue-Key: ecs.escc-ec2-service-omits-fargate-only-fields|run#11|AssertionError|assert present_fg == list(keys), (
Issue-Key: ecs.escc-fargate-service-omitted-settings-read-back-as-defaults|run#14|AssertionError|assert awsvpc.get("assignPublicIp") == "DISABLED", (
Issue-Key: ecs.escc-service-tags-only-returned-with-include-tags|run#14|AssertionError|assert "tags" not in untagged, (
Issue-Key: ecs.ecsnrt-gpu-host-volume-task-definition-derives-no-fargate-compatibility|run#16|AssertionError|assert omitted == [], f"task-level keys the caller omitted came back: {omitted!r} -> {[gpu[k] for k in omitted]!r}"
Issue-Key: ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults|run#10|AssertionError|assert router.get("user") == "0", (
Issue-Key: ecs.default-capacity-providers-active|run#4|AssertionError|assert name in providers, (
Issue-Key: ecs.escc-network-config-update-starts-a-new-primary-deployment|run#12|AssertionError|assert sorted(deps) == ["ACTIVE", "PRIMARY"], (
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# ECS service and task-definition read models omit derived fields and server-side defaults (platformFamily, platformVersion, assignPublicIp, tags, requiresCompatibilities, built-in capacity providers)

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
ECS derives a number of members server-side and materializes defaults before it answers a read; the emulator echoes only what the caller sent. Nine causes, numbered below — all nine must be closed before this issue is done.

Cause 1 of 9: Service.platformFamily / Deployment.platformFamily are never populated.
AWS: Smithy: Service.platformFamily (String) — "The operating system that your tasks in the service run on". ECS derives it from the task definition's runtimePlatform and returns the display spelling on both the service and its deployment (recorded: Linux -> "Linux", WINDOWS_SERVER_2022_CORE -> "Windows-Server-2022-Core", WINDOWS_SERVER_2019_FULL -> "Windows-Server-2019-Full").
Emulator: platformFamily is absent/None on both the service and the deployment for every OS.
Why: a client that selects behavior by platformFamily (Windows vs Linux task handling) sees nothing and cannot tell the two apart.
Fix: CreateService/DescribeServices service state derivation (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs + the Service/Deployment serializer).
Traces: ecs.escc-platform-family-is-derived-from-the-task-definition-os, ecs.escc-custom-deployment-and-network-settings-round-trip.

Cause 2 of 9: platformVersion "LATEST" is neither kept on the service nor resolved per OS on the deployment.
AWS: Smithy: Service.platformVersion — "If one isn't specified, the LATEST platform version is used". The service echoes LATEST while the deployment reports the resolved version (recorded: Linux 1.4.0, Windows 1.0.0).
Emulator: both are None for Linux and Windows.
Why: a caller cannot discover which Fargate platform version its deployment actually runs.
Fix: service/deployment state + serializer (same handler as Cause 1).
Traces: ecs.escc-latest-platform-version-resolves-per-os-on-the-deployment.

Cause 3 of 9: a Fargate service omits the Fargate-only control members.
AWS: a Fargate (awsvpc) service carries platformFamily, platformVersion, networkConfiguration and roleArn (Smithy: Service.platformFamily/.platformVersion/.networkConfiguration/.roleArn).
Emulator: carries only networkConfiguration.
Why: EC2-vs-Fargate discrimination by presence of these members is impossible, and roleArn (the ECS service-linked role) is invisible.
Fix: service state + Service serializer.
Traces: ecs.escc-ec2-service-omits-fargate-only-fields.

Cause 4 of 9: an omitted assignPublicIp is not materialized as DISABLED.
AWS: Smithy: AwsVpcConfiguration.assignPublicIp -> enum AssignPublicIp [ENABLED, DISABLED]; the member doc states "When you use create-service or update-service, the default is DISABLED", and the read-back carries it.
Emulator: the service reports awsvpcConfiguration {'subnets': ['subnet-...']} with no assignPublicIp member at all.
Why: a client reading the service back cannot tell whether public IPs are assigned, and an idempotent re-apply of the read value changes meaning.
Fix: CreateService default materialization for NetworkConfiguration.
Traces: ecs.escc-fargate-service-omitted-settings-read-back-as-defaults.

Cause 5 of 9: an untagged service returns an empty `tags` list instead of omitting the member.
AWS: with include=['TAGS'] on DescribeServices, a service with no tags omits the key entirely (recorded behavior).
Emulator: returns tags: [].
Why: clients that test key presence to decide "tags were requested and exist" mis-branch.
Fix: DescribeServices serializer (omit empty optional lists).
Traces: ecs.escc-service-tags-only-returned-with-include-tags.

Cause 6 of 9: task-level keys the caller omitted come back as empty values.
AWS: Smithy: RegisterTaskDefinition.requiresCompatibilities is an optional list; when the caller omits it on a GPU/host-volume (EC2-only) definition, the registered definition does not surface it — AWS derives `compatibilities` instead and omits `requiresCompatibilities` (recorded behavior).
Emulator: returns requiresCompatibilities: [].
Why: a round-trip of the described definition now declares an explicit empty compatibility list the caller never sent.
Fix: RegisterTaskDefinition/DescribeTaskDefinition serializer (omit members the request did not set).
Traces: ecs.ecsnrt-gpu-host-volume-task-definition-derives-no-fargate-compatibility.

Cause 7 of 9: FireLens router defaults are not injected.
AWS: when a container carries firelensConfiguration, ECS injects `user: "0"` on the router container in the registered definition (recorded behavior).
Emulator: the router container's user is None.
Why: the registered definition differs from what ECS would run; tooling that diffs desired vs registered state reports a permanent drift.
Fix: RegisterTaskDefinition FireLens default injection.
Traces: ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults.

Cause 8 of 9: the account's built-in FARGATE and FARGATE_SPOT capacity providers do not exist.
AWS: every account/region has the ECS-managed FARGATE and FARGATE_SPOT capacity providers; DescribeCapacityProviders(capacityProviders=["FARGATE","FARGATE_SPOT"]) returns them ACTIVE.
Emulator: returns capacityProviders: [] and failures [{'arn': 'arn:aws:ecs:...:capacity-provider/FARGATE', 'reason': 'MISSING'}, {... FARGATE_SPOT ... 'MISSING'}] with HTTP 200.
Why: any service or cluster using a FARGATE capacity-provider strategy cannot be validated or described; a client sees the managed providers as absent.
Fix: ECS state seeding for the two managed capacity providers (services/.../src/state.rs / capacity-provider store) and DescribeCapacityProviders.
Traces: ecs.default-capacity-providers-active.

Cause 9 of 9: an UpdateService network-configuration change does not start a new deployment.
AWS: changing networkConfiguration rolls the service — the new deployment becomes PRIMARY and the previous one stays ACTIVE until it drains (Smithy: Service.deployments -> Deployments).
Emulator: after the update there is exactly one deployment, [('ecs-svc/0000000000000000001', 'PRIMARY')] — the original one, with no new PRIMARY and no ACTIVE predecessor.
Why: deployment-progress tooling (and circuit-breaker/rollback logic) has nothing to observe; a client cannot tell a rolled service from an untouched one.
Fix: UpdateService deployment state transition (services/.../src/ops/update_service.rs).
Traces: ecs.escc-network-config-update-starts-a-new-primary-deployment.

Related open issues, not duplicates: 20261023T153036Z-ecs-service-read-and-write-responses-omit-recorded.md (omitted Service/Deployment members) and 20261023T153036Z-ecs-deleteservice-returns-status-inactive-instead-of-draining.md describe other members/transitions; the nine causes above were replayed as failing after those were filed.

## Evidence
Replay output (all verdict mismatch):
- ecs.escc-platform-family-is-derived-from-the-task-definition-os — "service and deployment platformFamily must both be the derived display spelling {'Linux-default': 'Linux', 'Linux-arm64': 'Linux', 'Windows-Server-2022-Core': 'Windows-Server-2022-Core'}; got {'Linux-default': (None, None), 'Linux-arm64': (None, None), 'Windows-Server-2022-Core': (None, None)}".
- ecs.escc-custom-deployment-and-network-settings-round-trip — "a WINDOWS_SERVER_2019_FULL task definition must surface as platformFamily 'Windows-Server-2019-Full'; got None".
- ecs.escc-latest-platform-version-resolves-per-os-on-the-deployment — "the service must keep 'LATEST' and the deployment must resolve it per OS (Linux 1.4.0, Windows 1.0.0); got {'linux': (None, None), 'windows': (None, None)}".
- ecs.escc-ec2-service-omits-fargate-only-fields — "the Fargate control must carry ['platformFamily', 'platformVersion', 'networkConfiguration', 'roleArn']; it carries ['networkConfiguration']".
- ecs.escc-fargate-service-omitted-settings-read-back-as-defaults — "an omitted assignPublicIp must default to DISABLED; the service reports {'subnets': ['subnet-00000000000000020']}".
- ecs.escc-service-tags-only-returned-with-include-tags — "an untagged service must omit the key even with include=['TAGS']; got []".
- ecs.ecsnrt-gpu-host-volume-task-definition-derives-no-fargate-compatibility — "task-level keys the caller omitted came back: ['requiresCompatibilities'] -> [[]]".
- ecs.ecsdeploy-firelens-task-definition-round-trips-with-router-defaults — "ECS injects user '0' on a FireLens router container; got None".
- ecs.default-capacity-providers-active — "built-in FARGATE capacity provider missing: {'capacityProviders': [], 'failures': [{'arn': 'arn:aws:ecs:us-west-2:000000000000:capacity-provider/FARGATE', 'reason': 'MISSING'}, {'arn': '...capacity-provider/FARGATE_SPOT', 'reason': 'MISSING'}], ... 'HTTPStatusCode': 200}".
- ecs.escc-network-config-update-starts-a-new-primary-deployment — "the update must leave one PRIMARY and the old deployment ACTIVE; got [('ecs-svc/0000000000000000001', 'PRIMARY')]".

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the ten checks in trace_ids (service token `ecs`).
3. Spot-check by hand, e.g.: RegisterTaskDefinition with runtimePlatform operatingSystemFamily=WINDOWS_SERVER_2019_FULL, CreateService on it with launchType FARGATE and an awsvpc networkConfiguration omitting assignPublicIp, then DescribeServices(include=['TAGS']) — platformFamily/platformVersion/roleArn are absent, assignPublicIp is absent, tags is []; and DescribeCapacityProviders(capacityProviders=['FARGATE','FARGATE_SPOT']) reports both MISSING.
