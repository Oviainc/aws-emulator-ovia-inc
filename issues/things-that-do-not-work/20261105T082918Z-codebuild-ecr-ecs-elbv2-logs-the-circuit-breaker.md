Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served
Issue-Key: codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served|_delete_net#2|Timeout|_time.sleep(2)
Issue-Commit: 506f51337b02a9b5673b5526bcfdc40d4e5c4821

# codebuild__ecr__ecs__elbv2__logs: the circuit-breaker rollback scenario never reaches an observable step — the CodeBuild→ECR→ECS/ALB chain produces no build outcome, no rolloutState and no served bytes

> _Note: this issue was identified against the code base at commit `506f51337b02a9b5673b5526bcfdc40d4e5c4821`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
One net-new root cause, grounded in the single surviving failing check of this service (its sibling check is OWNED by an already-filed issue).

Cause 1 of 1: the cross-service container CI/CD chain never advances to its first assertable observation, so the ECS deployment-circuit-breaker rollback behaviour — a failing-health-check revision being rolled back while the load balancer keeps serving the previous revision's bytes — is wholly unobservable. The check is cut off by the harness budget with no step completed and no response captured ('Check timed out after 60.0s'): no build status, no pushed image digest, no task placement, no target health, no ALB body and no deployment rolloutState is ever produced.

AWS: CodeBuild models the build state machine as StatusType {SUCCEEDED, FAILED, FAULT, TIMED_OUT, IN_PROGRESS, STOPPED} and the phase vocabulary as BuildPhaseType {SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE, INSTALL, PRE_BUILD, BUILD, POST_BUILD, UPLOAD_ARTIFACTS, FINALIZING, COMPLETED} (Smithy: com.amazonaws.codebuild#StatusType, com.amazonaws.codebuild#BuildPhaseType). On AWS a StartBuild of a project whose buildspec succeeds walks those phases and BatchGetBuilds reports buildStatus 'SUCCEEDED'; the image the buildspec pushes is then addressable in ECR by digest, an ECS service on a task definition pinning that digest reaches one RUNNING task, the ELBv2 target group reports that task's address 'healthy', and a GET through the listener returns 200 with the bytes the image serves. For a revision whose image fails the target group's health check and whose service has deploymentCircuitBreaker enabled, the documented behaviour is that that deployment reaches rolloutState FAILED with a circuit-breaker rolloutStateReason, the service rolls back to the last COMPLETED revision, and the load balancer keeps serving the previous revision's bytes throughout (AWS docs: ECS deployment circuit breaker). Recorded behavior only for the rest: the scenario's own recording is the only statement of the intermediate values.

Emulator: nothing in the chain is answered observably. The only captured result is the harness cutting the check off with no completed step — unlike the other failures in this run no status code, error code or mismatched field exists, because the flow produced none. Hypothesis (neither the model nor any captured response in this run states it): the first blocker is the CodeBuild build-execution gap this run records elsewhere for other service groupings, where a build walks [(SUBMITTED, SUCCEEDED), (QUEUED, SUCCEEDED), (PROVISIONING, FAULT), (COMPLETED, None)] and ends buildStatus FAULT rather than SUCCEEDED, so no image digest is ever produced and nothing downstream can exist; a second candidate is a provisioning poll in the networking/teardown setup (load balancer, ECS service or route-table teardown never settling) consuming the whole budget before StartBuild's outcome is read.

Note on existing issues: open issue 20261104T040200Z-codebuild-ecr-ecs-elbv2-logs-the-codebuild-ecr.md describes this service's chain and even lists this check among its trace ids, but it does not carry this check's replay key (its recorded key for this check is a teardown step, '_drop_rtb#2', while this run cuts the check off elsewhere), so this check is not covered by it. Related CodeBuild provisioning-FAULT issues (20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md, 20261029T174654Z-codebuild-codepipeline-ecr-build-actions-are-never-executed.md) likewise carry none of this service's keys.

Why: building a container image in CodeBuild, pushing it to ECR and rolling it out to an ECS service behind an ALB is the standard container CI/CD topology, and the safety property a client depends on is exactly the one this scenario asserts — a bad build must not take the service down, because the circuit breaker rejects it and the old bytes keep being served. Against the emulator the client gets no chain at all: the build never reports a terminal successful outcome, no digest exists, no task becomes healthy, no deployment ever reports a rolloutState, and code that polls for these documented steady states hangs instead of completing or failing, which is strictly worse than a wrong answer.

Fix: the CodeBuild build-execution state machine behind StartBuild/BatchGetBuilds (phase progression and terminal buildStatus) must drive a successful buildspec to SUCCEEDED so the chain's first observation exists; then the downstream steady states this flow waits on must each settle rather than remain pending — the ECR image/digest written by the build, the ECS service's task placement plus its deployment rolloutState (including the circuit-breaker FAILED/rollback path and its rolloutStateReason), and the ELBv2 target-group health plus the listener data path.

Traces: codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served.

## Evidence
Suite run (201/347 passed): service codebuild__ecr__ecs__elbv2__logs scored 0/2 (0%), 2 failed; the sibling check cicdchain-alb-body-tracks-the-latest-codebuild-image is OWNED by an already-filed issue, this one is not.

[mismatch] codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served
  Error: Check timed out after 60.0s

Expected (AWS): StartBuild returns buildStatus 'IN_PROGRESS'; BatchGetBuilds polls it to 'SUCCEEDED' with each phase phaseStatus 'SUCCEEDED'; DescribeImages for the pushed tag returns a digest beginning 'sha256:' with non-zero size; an ECS Fargate service on a task definition pinning repoUri@digest reaches exactly one RUNNING task whose address the target group reports 'healthy'; a GET through the ALB returns HTTP 200 with the bytes the image bakes. Then a revision whose image fails the health check makes that deployment reach rolloutState 'FAILED' with the circuit-breaker rolloutStateReason, the service rolls back to the last COMPLETED revision, and the ALB keeps serving the previous build's bytes throughout.

Actual (emulator): none of these assertions was reached. No build status, image digest, task, target-health entry, ALB response or rolloutState was captured — the harness cut the check off with no completed step. The requirement here is the enum vocabulary and the transitions themselves (IN_PROGRESS -> SUCCEEDED; rolloutState COMPLETED / FAILED; target health 'healthy'); the project name, build id, image tag, digest and addresses in any one run are values minted by that run and are not part of the requirement.

## Steps to Reproduce
1. Create an ECR repository and two CloudWatch log groups (one for the build, one for the ECS tasks).
2. Create a CodeBuild service role trusted by codebuild.amazonaws.com with ECR push/pull and logs permissions, and an execution role trusted by ecs-tasks.amazonaws.com with ECR pull permissions.
3. CreateProject with a buildspec that builds a tiny HTTP-serving image (baking a token, the build id and the tag into the body it serves) and docker-pushes it to that repository under a tag.
4. StartBuild with environmentVariablesOverride carrying the token, the tag and the Dockerfile; assert the returned buildStatus is 'IN_PROGRESS'.
5. Create a public VPC with subnets, an ALB security group open on tcp/80, a task security group admitting tcp/80 only from the ALB group, a target group, an ALB and an HTTP listener forwarding to it; create an ECS cluster.
6. Poll BatchGetBuilds until the build leaves IN_PROGRESS and assert buildStatus == 'SUCCEEDED'.
   -> AWS: the build SUCCEEDS and the image is addressable by digest. Emulator: the chain never reaches an observable outcome and the check is cut off at the harness budget with nothing captured.
7. Register a task definition pinning repoUri@digest, create a Fargate service on it behind the target group with deploymentCircuitBreaker enabled, and poll for one RUNNING task, a 'healthy' target and an HTTP 200 through the ALB whose body equals the baked body.
8. Build and deploy a revision whose image fails the target group's health check; poll for that deployment's rolloutState 'FAILED' with the circuit-breaker rolloutStateReason, for the service to roll back to the last COMPLETED revision, and for the ALB to have kept serving the previous build's bytes throughout.
