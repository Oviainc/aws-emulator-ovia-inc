Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served
Issue-Key: codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image|_delete_net#2|Timeout|_time.sleep(2)
Issue-Key: codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served|_drop_rtb#2|Timeout|raise RuntimeError(f"route table {rtb_id} could not be deleted")
Issue-Commit: 165722d258ec8cbd86766e596a6f87959b068084

# codebuild__ecr__ecs__elbv2__logs: the CodeBuild→ECR→ECS/ALB CI-CD chain never reaches an observable build/rollout outcome, so both container-chain checks never complete (1 cause)

> _Note: this issue was identified against the code base at commit `165722d258ec8cbd86766e596a6f87959b068084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
Cause 1 of 1: the cross-service container CI-CD chain never advances to the first assertable observation. Both checks in this service follow the same shape — create an ECR repository and two log groups, create IAM roles, CreateProject, StartBuild, build a public VPC/subnets/security groups, create a target group, an ALB and a listener, create an ECS cluster, then BatchGetBuilds-poll the build out of IN_PROGRESS and assert buildStatus == 'SUCCEEDED' before registering a task definition pinned to the pushed image digest and rolling it out behind the ALB. Against the emulator neither check ever produces a verdict on any of those assertions: each is cut off by the harness budget with no response captured (`Check timed out after 60.0s`), so no step of the chain — build outcome, pushed image digest, task placement, target health, ALB body, rolloutState — is ever observable.

AWS: CodeBuild's build state machine is modeled by StatusType {SUCCEEDED, FAILED, FAULT, TIMED_OUT, IN_PROGRESS, STOPPED} and BuildPhaseType {SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE, INSTALL, PRE_BUILD, BUILD, POST_BUILD, UPLOAD_ARTIFACTS, FINALIZING, COMPLETED} (Smithy: com.amazonaws.codebuild#StatusType, com.amazonaws.codebuild#BuildPhaseType). On real AWS a StartBuild of a project whose buildspec runs successfully walks every phase and BatchGetBuilds reports buildStatus 'SUCCEEDED' with each phase's phaseStatus 'SUCCEEDED'; the image the buildspec pushes is then addressable in ECR by digest, an ECS service on a task definition pinning that digest reaches one RUNNING task, the ALB target group reports that task's ENI address 'healthy', and a GET through the load balancer returns 200 with the bytes the image serves. The surrounding progressions (ECS deployment rolloutState reaching COMPLETED, the circuit breaker reaching FAILED and rolling back) are the documented steady-state behaviors these checks assert.

Emulator: the suite never gets far enough to record any of those values. The only observed result for both checks is the harness cutting them off with no completed step (`Error: Check timed out after 60.0s`); unlike the other failures in this run, no status code, error code or mismatched field was captured, because the flow produced none. Hypothesis (not stated by the model or by any captured response in this run): the same build-execution gap this run records elsewhere is the first blocker — in the very same suite run, cloudwatch__codebuild's build check captured a build whose phases were [('SUBMITTED','SUCCEEDED'), ('QUEUED','SUCCEEDED'), ('PROVISIONING','FAULT'), ('COMPLETED', None)] and whose buildStatus was 'FAULT' rather than 'SUCCEEDED', so a build here would never SUCCEED and nothing downstream (the ECR digest, the pinned task definition, the ALB body) would ever exist; a second candidate blocker is a provisioning poll in the networking setup (load balancer or ECS service never leaving its provisioning state) consuming the whole budget before StartBuild's outcome is even read.

Why: a client that builds a container image in CodeBuild, pushes it to ECR and rolls it out to an ECS service behind an ALB — the standard container CI/CD topology — gets no working chain at all against the emulator. Nothing it waits on ever settles: the build never reports a terminal successful outcome, so no image digest is produced, no task definition can be pinned, no target becomes healthy and the load balancer never serves the built bytes. Code that polls for these documented steady states hangs instead of completing or failing, which is strictly worse than a wrong answer.

Fix: the CodeBuild build-execution state machine behind StartBuild/BatchGetBuilds (the build's phase progression and terminal buildStatus) must drive a successful buildspec to SUCCEEDED so the chain's first observation exists; then the downstream steady-state transitions these flows wait on — the ECR image/digest written by the build, the ECS service's task placement and deployment rolloutState, and the ELBv2 target-group health and listener data path — must each settle rather than remaining pending. Related open issues describe the CodeBuild provisioning FAULT for other service groupings (20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md, 20261025T014438Z-codebuild-s3-builds-fault-at-provisioning-so-no.md, 20261029T174654Z-codebuild-codepipeline-ecr-build-actions-are-never-executed.md) but none of them carries these two checks' replay keys, so the codebuild__ecr__ecs__elbv2__logs chain is unverified by any of them.

Traces: both failing checks of this service, listed under trace_ids.

## Evidence
Suite run (79/190 passed): service codebuild__ecr__ecs__elbv2__logs scored 0/2 (0%), 2 failed.

[mismatch] codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image
  Error: Check timed out after 60.0s

[mismatch] codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served
  Error: Check timed out after 60.0s

Expected (AWS): the first check's StartBuild of tag v1 reaches BatchGetBuilds buildStatus 'SUCCEEDED'; DescribeImages for that tag returns a digest beginning 'sha256:' with a non-zero size; the ECS service reaches exactly one RUNNING task whose ENI address lies in the check's VPC CIDR and which the target group reports 'healthy'; a GET through the ALB returns HTTP 200 whose body is the token+build-id+tag the image bakes; a second build deployed by digest makes the ALB serve build #2's body and the PRIMARY deployment reach rolloutState 'COMPLETED' on revision 2 with runningCount == desiredCount == 1. The second check additionally expects the broken revision's deployment to reach rolloutState 'FAILED' with the documented circuit-breaker rolloutStateReason, the service to roll back to the last COMPLETED revision, and the ALB to keep serving the previous build's bytes throughout.

Actual (emulator): neither check reached any of these assertions. No build outcome, image digest, task, target-health entry, ALB response, or rolloutState was captured for either check — the harness cut both off with no completed step. In the SAME suite run the CodeBuild build state machine was captured directly by cloudwatch__codebuild.build-outcome-splits-succeeded-and-failed-builds-in-the-codebuild-namespace: 'ended with buildStatus FAULT, expected SUCCEEDED; Phases were [(SUBMITTED, SUCCEEDED), (QUEUED, SUCCEEDED), (PROVISIONING, FAULT), (COMPLETED, None)]'. The fixed text of that captured divergence is the phase/status vocabulary itself (PROVISIONING, FAULT, SUCCEEDED, IN_PROGRESS — all Smithy enum members); the project name, build id and tag in any one run are values taken from the request and the emulator's state and are not part of the requirement.

## Steps to Reproduce
1. Create an ECR repository and two CloudWatch log groups (one for the build, one for the ECS tasks).
2. Create a CodeBuild service role trusted by codebuild.amazonaws.com with ECR push/pull and logs permissions, and a task-execution role trusted by ecs-tasks.amazonaws.com with ECR pull permissions.
3. CreateProject with a buildspec that builds a tiny HTTP-serving image (baking a token, the build id and the tag into the served body) and docker-pushes it to that ECR repository under a tag.
4. StartBuild with environmentVariablesOverride carrying the token, the image tag and the Dockerfile; assert the returned buildStatus is 'IN_PROGRESS'.
5. Create a public VPC with subnets, an ALB security group open on tcp/80, a task security group admitting tcp/80 only from the ALB group, a target group, an ALB and an HTTP listener forwarding to the target group; create an ECS cluster.
6. Poll BatchGetBuilds until the build leaves IN_PROGRESS and assert buildStatus == 'SUCCEEDED'.
   -> On AWS the build SUCCEEDS and the image is addressable by digest. On the emulator the chain never reaches an observable outcome: the flow produces no verdict and is cut off by the harness budget (the same run records CodeBuild builds ending 'FAULT' with PROVISIONING FAULT).
7. Register a task definition pinning repoUri@digest, create a Fargate service on it behind the target group, and poll for one RUNNING task, a 'healthy' target and an HTTP 200 through the ALB whose body equals the baked body.
8. Start a second build with a new tag, register the next revision pinning its digest, UpdateService to it, and poll for the ALB to serve build #2's body and for the PRIMARY deployment to reach rolloutState 'COMPLETED' on that revision.
9. For the rollback check, deploy a revision whose image fails the ALB health check and poll for that deployment's rolloutState 'FAILED' with the circuit-breaker rolloutStateReason, for the service to roll back to the last COMPLETED revision, and for the ALB to have kept serving the previous build's bytes.
