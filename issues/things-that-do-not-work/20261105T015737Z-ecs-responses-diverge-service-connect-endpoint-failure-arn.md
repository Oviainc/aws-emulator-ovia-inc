Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop ecs-agent-apis-and-exec-error-paths ecs-cluster-setup-poll-endpoint-and-teardown ecs-run-task-and-task-protection
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeServices|0->None|$
Issue-Key: ecs|DescribeServices|0->None|status
Issue-Key: ecs|DiscoverPollEndpoint|2xx->2xx|$.serviceConnectEndpoint
Issue-Key: ecs|UpdateTaskProtection|2xx->2xx|$.failures[*]
Issue-Commit: a5885c3be3f986c1a94ab18540ac47db2748e09d

# ECS responses diverge: Service Connect endpoint, failure ARN re-synthesis, and a deployment poll that never settles

> _Note: this issue was identified against the code base at commit `a5885c3be3f986c1a94ab18540ac47db2748e09d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Three net-new ECS wire divergences, bundled because they all belong to the same service and none is covered by an open issue's replay key.

Cause 1 of 3: a circuit-breaker deployment never reaches a terminal state, so a DescribeServices poll never settles (operation DescribeServices; fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#15).
AWS: recorded behavior only for this step (DescribeServices is modeled read-only, output services/failures, errors ClientException/ClusterNotFoundException/InvalidParameterException/AccessDeniedException/ServerException). The recording drives a service whose new deployment keeps failing with deploymentCircuitBreaker enabled and then polls DescribeServices until the service's deployment reaches a terminal outcome (the failed deployment is rolled back / stopped and the service's deployments converge); on AWS that poll completes well inside the fixture's budget.
Emulator: the step produces NO response at all — the replay reports 'status: expected , got None' and '$: fixture budget 600s spent', i.e. the poll loop on DescribeServices never observed the terminal deployment state and the whole fixture was killed at its 600s budget. Every later step of that fixture is therefore unexercised.
Why: a client that waits for a deployment to settle (the normal way to drive a rollout, and what the AWS CLI's `ecs wait services-stable` and CDK/CodeDeploy integrations do) hangs forever against the emulator instead of observing the rollback and continuing. Hypothesis: the service-deployment state machine never transitions a failing deployment out of IN_PROGRESS, so DescribeServices keeps reporting the same non-terminal deployment forever.
Fix: the service scheduler / service-deployment state machine behind DescribeServices — the deployment lifecycle and circuit-breaker evaluation that must drive a failing deployment to a terminal (rolled-back/stopped) state, and the clock pass that materialises it. The already-open behavioral issue 20261104T062603Z-ecs-rolling-deployment-controls-are-not-enforced-the.md describes the same underlying non-convergence from the behavioral corpus (its cause 3), but it carries no wire replay key, so this wire step is net-new and must be re-replayed to confirm a fix.
Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#15.

Cause 2 of 3: UpdateTaskProtection re-synthesizes the failure entry's ARN with a cluster segment (operation UpdateTaskProtection; fixture ecs-run-task-and-task-protection, step task#7).
AWS: Smithy: UpdateTaskProtection output member `failures` (Failures -> Failure{arn,reason,detail}); Failure$arn is documented as 'The Amazon Resource Name (ARN) of the failed resource'. For a task identifier that resolves to no task, AWS reports the failure under the resource the request named — the task ARN formed from the request's own identifier — with reason 'MISSING'. The recorded response shows the failure arn in the form `arn:aws:ecs:<region>:<account>:task/<task-id>` (no cluster segment) for a request that named a bare task id.
Emulator: it returns the same reason 'MISSING' but rewrites the arn into the long, cluster-qualified form `arn:aws:ecs:<region>:<account>:task/<cluster-name>/<task-id>`, inserting the cluster the request was addressed to. (Recorded instance: expected `...:task/00000000-0000-0000-0000-000000000000`, got `...:task/<generated-cluster-name>/00000000-0000-0000-0000-000000000000`; the cluster name and the task id are per-run values, the inserted `/<cluster>/` segment is the defect.)
Why: clients correlate `failures[]` entries back to the identifiers they submitted. An arn the caller never supplied — and that names a cluster-qualified resource for a task that does not exist — breaks that match, so SDK helpers and retry logic cannot tell which requested task failed.
Fix: the UpdateTaskProtection handler's unresolved-failure construction — it must report the identifier the request sent rather than the ARN a stored task of that id would carry. See services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/update_task_protection.rs:311 (the `None` branch calling `crate::state::unresolved_failure`, which qualifies via `crate::state::task_arn_from_reference` in src/state.rs). This is the same class of defect as the already-open 20261030T130106Z-ecs-describetasksets-failure-entries-re-synthesize-task-set.md, which covers DescribeTaskSets and not this operation.
Traces: ecs-run-task-and-task-protection#task#7.

Cause 3 of 3: DiscoverPollEndpoint's serviceConnectEndpoint is derived from the agent-endpoint host template and does not match the value class AWS returns (operation DiscoverPollEndpoint; fixture ecs-agent-apis-and-exec-error-paths, step task#2).
AWS: Smithy: DiscoverPollEndpoint output {endpoint, telemetryEndpoint, serviceConnectEndpoint}; serviceConnectEndpoint is documented as 'The endpoint for the Amazon ECS agent to poll for Service Connect configuration'. Recorded behavior only for its concrete value: the recording asserts this member against a shape class rather than a literal (the per-environment parts are not asserted), and the agent and telemetry endpoints of the same response satisfy their shape while this member does not — so AWS's Service Connect configuration endpoint is not a member of the same host family/form the emulator emits.
Emulator: it emits `https://ecs-sc.<region>.amazonaws.com/`, built by the same per-region service-host template used for `endpoint` and `telemetryEndpoint`, and the replay reports `$.serviceConnectEndpoint: value_mismatch (expected '<shaped-not-asserted>', got 'https://ecs-sc.us-west-2.amazonaws.com/')`. Status is 200 on both sides; only this member diverges. Hypothesis: AWS serves the Service Connect configuration endpoint under a different domain family than the agent-communication/telemetry endpoints, so re-using the one template cannot produce a conforming value.
Why: the ECS agent reads this member to decide where to poll Service Connect configuration; a value of the wrong form sends an agent (or any client asserting the documented endpoint form) at an address that is not a Service Connect configuration endpoint.
Fix: the DiscoverPollEndpoint response construction and the endpoint derivation it calls — services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/discover_poll_endpoint.rs:258 and `crate::state::agent_service_connect_endpoint` in src/state.rs. The host itself is environment-specific and is not the requirement; the form/family of the value is.
Traces: ecs-agent-apis-and-exec-error-paths#task#2 (also ecs-cluster-setup-poll-endpoint-and-teardown#task#2).

## Evidence
Replay of trace set simparity-wire-aws-v1 (117 fixtures, 1356 steps, 1111 passed, score 81.9%); these are the only three divergence clusters that are neither blocked by an unimplemented-operation stub nor owned by an existing open issue.

Cause 1 — DescribeServices, fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop step task#15, verdict mismatch:
  expected_status: 0 (recorded step), actual_status: null
  detail: "status: expected , got None" / "$: fixture budget 600s spent (expected None, got None)"
  Expected: the DescribeServices poll observes the failing deployment reach a terminal rolled-back/stopped outcome and the fixture continues.
  Actual: no response is ever produced for the step; the fixture is killed after exhausting its 600s budget, so the rest of the fixture never runs.

Cause 2 — UpdateTaskProtection, fixture ecs-run-task-and-task-protection step task#7, verdict mismatch, status 200 on both sides:
  detail: "$.failures[0]: missing (expected {'arn': 'arn:aws:ecs:us-west-2:000000000000:task/00000000-0000-0000-0000-000000000000', 'reason': 'MISSING'}, got {'arn': 'arn:aws:ecs:us-west-2:000000000000:task/sim2real-probe-ecs-ecs-cluster-13b8980e/00000000-0000-0000-0000-000000000000', 'reason': 'MISSING'})"
  Expected arn form: arn:aws:ecs:<region>:<account>:task/<task-id>  (the identifier the request named)
  Actual arn form:   arn:aws:ecs:<region>:<account>:task/<cluster-name>/<task-id>
  `reason` is correct ('MISSING'); the cluster name and task id shown are per-run probe values, the injected cluster segment is the requirement breach.

Cause 3 — DiscoverPollEndpoint, fixture ecs-agent-apis-and-exec-error-paths step task#2 (cluster also covers ecs-cluster-setup-poll-endpoint-and-teardown#task#2), verdict mismatch, status 200 on both sides:
  detail: "$.serviceConnectEndpoint: value_mismatch (expected '<shaped-not-asserted>', got 'https://ecs-sc.us-west-2.amazonaws.com/')"
  `endpoint` and `telemetryEndpoint` of the same response satisfy their shape assertions; only serviceConnectEndpoint fails, so the divergence is in the form of that member's value, not in the environment's host.

## Steps to Reproduce
Cause 1: CreateService with deploymentCircuitBreaker {enable:true, rollback:true}, then UpdateService to a revision whose tasks cannot start, then poll DescribeServices for the service until its deployment reaches a terminal state -> the poll never terminates (replay fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop, step task#15, which exhausts the 600s fixture budget).

Cause 2: CreateCluster, then call UpdateTaskProtection with cluster=<that cluster> and tasks=[<a bare task id that does not exist>] -> the response's failures[0].arn comes back as arn:aws:ecs:<region>:<account>:task/<cluster>/<id> instead of arn:aws:ecs:<region>:<account>:task/<id> with reason MISSING (replay fixture ecs-run-task-and-task-protection, step task#7).

Cause 3: CreateCluster, RegisterContainerInstance, then call DiscoverPollEndpoint for that cluster/instance -> serviceConnectEndpoint is returned as https://ecs-sc.<region>.amazonaws.com/, which does not match the value class the recording asserts for that member (replay fixture ecs-agent-apis-and-exec-error-paths, step task#2; also ecs-cluster-setup-poll-endpoint-and-teardown, step task#2).
