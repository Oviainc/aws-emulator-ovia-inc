Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.exec-protection-agent-ops-on-missing-cluster-not-found ecs.container-instance-agent-connection-timeout ecs.container-state-change-sequencing ecs.escc-assign-public-ip-value-is-case-sensitive autoscaling__ec2__ecs.ecsnrt-capacity-provider-name-and-group-reuse-rejected autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it ecs.update-service-deployment-controller-change-rejected ecs.ecsnt-create-service-with-a-negative-desired-count-rejected ecs.escc-describe-services-rejects-more-than-ten-names ecs.op-contract-put-account-setting ecs.op-contract-put-account-setting-default ecs.concurrent-account-setting-writes ecs.register-taskdef-duplicate-container-name-rejected ecs.run-task-nonexistent-taskdef-fails ec2__ecs.ecsnt-exec-command-service-without-a-task-role-rejected
Issue-Key: ecs.exec-protection-agent-ops-on-missing-cluster-not-found|run#12|AssertionError|assert code == "InvalidParameterException", f"ExecuteCommand control: got {code}"
Issue-Key: ecs.container-instance-agent-connection-timeout|run#4|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: ecs.container-state-change-sequencing|run#7|AssertionError|assert code == "ClientException", f"expected ClientException, got {code}"
Issue-Key: ecs.escc-assign-public-ip-value-is-case-sensitive|run#7|AssertionError|assert code == "InvalidParameterException", f"got {code!r}"
Issue-Key: autoscaling__ec2__ecs|CreateCapacityProvider|ClientException
Issue-Key: autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it|run#11|AssertionError|assert code_msg == want, f"RunTask on the unmanaged empty provider said {code_msg!r}, expected {want!r}"
Issue-Key: ecs.update-service-deployment-controller-change-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ecs.ecsnt-create-service-with-a-negative-desired-count-rejected|run#8|AssertionError|assert msg == want, f"desiredCount -1 refusal message: expected {want!r}, got {msg!r}"
Issue-Key: ecs.escc-describe-services-rejects-more-than-ten-names|run#10|AssertionError|assert msg == want, f"eleven names refusal message: expected {want!r}, got {msg!r}"
Issue-Key: ecs.op-contract-put-account-setting|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ecs.op-contract-put-account-setting-default|run#13|AssertionError|assert message == "Invalid setting 'value'", message
Issue-Key: ecs.concurrent-account-setting-writes|run#5|AssertionError|assert message == "Invalid setting 'value'", f"unexpected message: {message!r}"
Issue-Key: ecs.register-taskdef-duplicate-container-name-rejected|run#6|AssertionError|assert message == "Container names must be unique within a task definition.", f"refusal message {message!r}"
Issue-Key: ecs.run-task-nonexistent-taskdef-fails|run#7|AssertionError|assert message == "TaskDefinition not found.", f"refusal message {message!r}"
Issue-Key: ec2__ecs.ecsnt-exec-command-service-without-a-task-role-rejected|run#10|AssertionError|assert (code, msg) == want, f"exec-command without a task role: {(code, msg)!r}, expected {want!r}"
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# ECS agent-plane operations are unimplemented stubs, and several refusals carry the wrong error code, wrong reason or emulator-internal message text

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteCommand

## Description
Seven causes in the ECS error surface: three agent-plane operations answer NotImplemented, three refusals carry a wrong code or wrong reason, and one cluster of refusals carries emulator-internal message text. Seven causes, numbered below — all seven must be closed before this issue is done.

Cause 1 of 7: ExecuteCommand answers NotImplemented.
AWS: Smithy: ExecuteCommand is a modeled operation (input cluster/container/command/interactive/task) whose errors are ClientException 400, ClusterNotFoundException 400, InvalidParameterException 400, TargetNotConnectedException 400, AccessDeniedException 403, ServerException 500 — NotImplemented is not among them. For a live cluster and an unknown task, AWS answers 400 InvalidParameterException (recorded message example: "The specified task was not found. Specify a valid task ID and try again."); for an unknown cluster, 400 ClusterNotFoundException ("Cluster not found.").
Emulator: error code NotImplemented ("operation ... is scaffolded but not implemented in this emulator").
Why: ECS Exec cannot be exercised at all, and the cluster-vs-task error discrimination a client branches on is unavailable.
Fix: implement the ExecuteCommand handler (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/execute_command.rs).
Traces: ecs.exec-protection-agent-ops-on-missing-cluster-not-found.

Cause 2 of 7: UpdateContainerAgent answers NotImplemented.
AWS: Smithy: UpdateContainerAgent errors are ClientException 400, ClusterNotFoundException 400, InvalidParameterException 400, MissingVersionException 400, NoUpdateAvailableException 400, UpdateInProgressException 400, AccessDeniedException 403, ServerException 500. Against a nonexistent cluster AWS answers InvalidParameterException/ClusterNotFoundException (recorded: ClusterNotFoundException "Cluster not found."; for a real cluster and unknown instance, ClientException "Referenced container instance <id> not found.").
Emulator: NotImplemented.
Why: container-instance agent lifecycle (and its negative contract) cannot be emulated.
Fix: implement services/.../src/ops/update_container_agent.rs.
Traces: ecs.container-instance-agent-connection-timeout.

Cause 3 of 7: SubmitContainerStateChange answers NotImplemented.
AWS: Smithy: SubmitContainerStateChange errors are AccessDeniedException 403, ClientException 400, ClusterNotFoundException 400, InvalidParameterException 400, ServerException 500. A bogus cluster yields ClientException (recorded message example "Cluster not found."); a referenced-but-missing task yields InvalidParameterException ("The referenced task was not found").
Emulator: NotImplemented.
Why: the agent-facing state-change plane is unusable, so container state sequencing (including stale/out-of-order replays) cannot be tested.
Fix: implement services/.../src/ops/submit_container_state_change.rs.
Traces: ecs.container-state-change-sequencing.

Cause 4 of 7: a case-variant assignPublicIp value is refused with an error code ECS does not model.
AWS: Smithy: AwsVpcConfiguration.assignPublicIp -> enum AssignPublicIp [ENABLED, DISABLED] (values are case-sensitive); CreateService's modeled client errors are ClientException, ClusterNotFoundException, InvalidParameterException, NamespaceNotFoundException, PlatformTaskDefinitionIncompatibilityException, PlatformUnknownException, UnsupportedFeatureException, AccessDeniedException — there is no ValidationException in ECS at all. Recorded: the refusal is InvalidParameterException with HTTP 400.
Emulator: HTTP 400 with error code ValidationException.
Why: clients that catch ecs.exceptions.InvalidParameterException (the modeled type) do not catch this error at all; botocore surfaces an unmodeled code.
Fix: CreateService input validation error mapping (services/.../src/ops/create_service.rs).
Traces: ecs.escc-assign-public-ip-value-is-case-sensitive.

Cause 5 of 7: CreateCapacityProvider is not idempotent for an identical repeat.
AWS: Smithy: CreateCapacityProvider carries the `idempotent` trait. Repeating the call with the SAME name and the SAME autoScalingGroupProvider returns the existing provider (same capacityProviderArn) rather than an error; only a repeat that changes the configuration (e.g. the same name over a different Auto Scaling group) is refused with ClientException 400 (recorded message example: "The specified capacity provider already exists. To change the configuration of an existing capacity provider, update the capacity provider.").
Emulator: the identical repeat is refused with ClientException "A capacity provider with the specified name already exists. To change the configuration of an existing capacity provider, use UpdateCapacityProvider.", which also aborts the check before the real negative legs run.
Why: any retry-safe client (SDK retries, CFN/CDK re-apply, Terraform) that re-sends an unchanged CreateCapacityProvider now fails where AWS succeeds.
Fix: CreateCapacityProvider handler idempotency (services/.../src/ops/create_capacity_provider.rs) — compare the requested configuration before refusing.
Traces: autoscaling__ec2__ecs.ecsnrt-capacity-provider-name-and-group-reuse-rejected (check-id group is composite; the diverging operation is ECS CreateCapacityProvider).

Cause 6 of 7: RunTask against a capacity provider that holds no container instances reports a cluster-level reason instead of naming the provider.
AWS: the refusal identifies the capacity provider from the request's capacityProviderStrategy (recorded message, built from the request: "The specified capacity provider '<name>' contains no container instances.").
Emulator: answers the generic cluster-level reason "No Container Instances were found in your cluster." — the provider named in the request is not consulted.
Why: a caller cannot tell which provider in a multi-provider strategy is empty, so autoscaling/parking logic keyed on the provider cannot act.
Fix: RunTask placement refusal path (services/.../src/ops/run_task.rs) — resolve the strategy's provider before reporting.
Traces: autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it (composite check-id group; the operation is ECS RunTask).

Cause 7 of 7: refusal messages diverge from ECS's text, and several expose emulator-internal (Rust) formatting.
AWS: for these refusals the message is a fixed service string or is built from the request naming the member and its constraint. Recorded examples: UpdateService deployment-controller change — "Updating the deployment controller requires a primary taskset in the service. Please ensure your service has a primary taskset and try again."; CreateService desiredCount -1 — "DesiredCount can not be lesser than 0"; DescribeServices with 11 names — "service names can have at most 10 items."; PutAccountSetting with an invalid value — "Invalid setting 'value'" (Smithy: PutAccountSetting.name -> enum SettingName, .value required String); RegisterTaskDefinition duplicate container names — "Container names must be unique within a task definition."; RunTask with an unregistered task definition — "TaskDefinition not found."; CreateService with a task definition lacking a task role for exec — "The service couldn't be created because a valid taskRoleArn is not being used. Specify a valid task role in your task definition and try again."
Emulator: emits its own wording, and in several cases internal Rust formatting leaks: 'desiredCount must not be negative; got Some(-1)'; 'services must name at most 10 services; got 11'; 'value for containerInsights must be one of enhanced, enabled, disabled; got "<value>"'; "container names must be unique within a task definition; 'app' is repeated"; 'Unable to describe task definition.'; 'the deployment controller of service "<name>" is EXTERNAL and cannot be changed to ECS' (a different reason from AWS's "requires a primary taskset"); 'Specify a valid task IAM role and try again.'
Why: the `Some(-1)` / Rust-debug text reveals the request was validated by a non-AWS code path, and clients or runbooks that match on ECS's message (and operators reading it) are misled; for UpdateService the stated REASON for the refusal is wrong, not just the wording.
Fix: the per-operation validation error constructors for UpdateService, CreateService, DescribeServices, PutAccountSetting/PutAccountSettingDefault, RegisterTaskDefinition and RunTask — emit ECS's message text and never Rust Debug output.
Traces: ecs.update-service-deployment-controller-change-rejected, ecs.ecsnt-create-service-with-a-negative-desired-count-rejected, ecs.escc-describe-services-rejects-more-than-ten-names, ecs.op-contract-put-account-setting, ecs.op-contract-put-account-setting-default, ecs.concurrent-account-setting-writes, ecs.register-taskdef-duplicate-container-name-rejected, ecs.run-task-nonexistent-taskdef-fails, ec2__ecs.ecsnt-exec-command-service-without-a-task-role-rejected.

## Evidence
Replay output:
- ecs.exec-protection-agent-ops-on-missing-cluster-not-found [mismatch] — `ecs.execute_command(cluster=<live>, task=<arn>, command="ls", interactive=True)` -> "ExecuteCommand control: got NotImplemented"; expected code InvalidParameterException.
- ecs.container-instance-agent-connection-timeout [mismatch] — `ecs.update_container_agent(cluster=<bogus>, containerInstance="x")` -> "expected InvalidParameterException, got NotImplemented".
- ecs.container-state-change-sequencing [mismatch] — `ecs.submit_container_state_change(cluster=<bogus>, task="0"*32, containerName="app", status="RUNNING")` -> "expected ClientException, got NotImplemented".
- ecs.escc-assign-public-ip-value-is-case-sensitive [mismatch] — "got 'ValidationException'" where InvalidParameterException was expected.
- autoscaling__ec2__ecs.ecsnrt-capacity-provider-name-and-group-reuse-rejected [error] — the identical repeat `create_capacity_provider(name=cp, autoScalingGroupProvider={autoScalingGroupArn: g1, managedTerminationProtection: DISABLED, managedScaling: {status: DISABLED, targetCapacity: 100}})` raised botocore.errorfactory.ClientException: "A capacity provider with the specified name already exists. To change the configuration of an existing capacity provider, use UpdateCapacityProvider."; AWS returns the existing provider's ARN (the check asserts again['capacityProviderArn'] == first).
- autoscaling__ec2__ecs.ecsnrt-empty-provider-run-task-rejected-unless-managed-scaling-parks-it [mismatch] — "RunTask on the unmanaged empty provider said 'No Container Instances were found in your cluster.', expected \"The specified capacity provider '<name>' contains no container instances.\"".
- ecs.update-service-deployment-controller-change-rejected [mismatch] — expected message 'Updating the deployment controller requires a primary taskset in the service. Please ensure your service has a primary taskset and try again.', got 'the deployment controller of service "<name>" is EXTERNAL and cannot be changed to ECS'.
- ecs.ecsnt-create-service-with-a-negative-desired-count-rejected [mismatch] — expected 'DesiredCount can not be lesser than 0', got 'desiredCount must not be negative; got Some(-1)'.
- ecs.escc-describe-services-rejects-more-than-ten-names [mismatch] — expected 'service names can have at most 10 items.', got 'services must name at most 10 services; got 11'.
- ecs.op-contract-put-account-setting [mismatch] — expected message "Invalid setting 'value'", got 'value for containerInsights must be one of enhanced, enabled, disabled; got "sim2real-not-a-real-value"'; ecs.op-contract-put-account-setting-default and ecs.concurrent-account-setting-writes report the same emulator string.
- ecs.register-taskdef-duplicate-container-name-rejected [mismatch] — expected "Container names must be unique within a task definition.", got "container names must be unique within a task definition; 'app' is repeated".
- ecs.run-task-nonexistent-taskdef-fails [mismatch] — expected "TaskDefinition not found.", got 'Unable to describe task definition.'.
- ec2__ecs.ecsnt-exec-command-service-without-a-task-role-rejected [mismatch] — code InvalidParameterException matched; message "... Specify a valid task IAM role and try again." instead of "... Specify a valid task role in your task definition and try again.".

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the checks in trace_ids.
3. By hand: (a) call ExecuteCommand / UpdateContainerAgent / SubmitContainerStateChange and observe error code NotImplemented; (b) CreateService with networkConfiguration.awsvpcConfiguration.assignPublicIp='enabled' (lowercase) and observe code ValidationException; (c) call CreateCapacityProvider twice with byte-identical input and observe the second call raising ClientException; (d) RunTask with a capacityProviderStrategy naming an empty provider and read the message; (e) trigger each refusal in Cause 7 and compare the message with ECS's recorded text.
