Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served
Issue-Key: codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image|run#2|AssertionError|assert done1["buildStatus"] == "SUCCEEDED", _build_failure_detail(est, done1)
Issue-Key: codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served|run#2|AssertionError|assert done1["buildStatus"] == "SUCCEEDED", _build_failure_detail(est, done1)
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# codebuild__ecr__ecs__elbv2__logs: every CodeBuild build in the container CI-CD chain ends buildStatus FAULT at phase PROVISIONING, so the chain never produces an image to roll out (1 cause)

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
Cause 1 of 1: a build started against a project whose buildspec builds and docker-pushes an image never executes. BatchGetBuilds reports the build terminating with buildStatus 'FAULT', its PROVISIONING phase carrying phaseStatus 'FAULT' and the emulator-authored reason 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'. Both checks of this service stop at the first assertion that reads the build outcome (`assert done1["buildStatus"] == "SUCCEEDED"`), so no step downstream of the build — the pushed ECR image/digest, the task definition pinned to that digest, the ECS rollout, the ALB-served body, the circuit-breaker rollback — is reachable.

AWS: CodeBuild models the build lifecycle as com.amazonaws.codebuild#BuildPhaseType {SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE, INSTALL, PRE_BUILD, BUILD, POST_BUILD, UPLOAD_ARTIFACTS, FINALIZING, COMPLETED} and the outcome as com.amazonaws.codebuild#StatusType {SUCCEEDED, FAILED, FAULT, TIMED_OUT, IN_PROGRESS, STOPPED}. StartBuild of a project with a valid environment and a buildspec that succeeds walks all of those phases with each phaseStatus 'SUCCEEDED' and BatchGetBuilds then reports buildStatus 'SUCCEEDED'. FAULT is AWS's code for an internal CodeBuild failure, not the outcome of a well-formed build request. Per the model there is no condition under which a valid StartBuild of a healthy project is answered with a PROVISIONING FAULT.

Emulator: StartBuild is accepted (buildStatus 'IN_PROGRESS'), but the build container is never run: the build settles at phases [SUBMITTED/SUCCEEDED, QUEUED/SUCCEEDED, PROVISIONING/FAULT, COMPLETED] with terminal buildStatus 'FAULT'. The fixed text of the divergence is the enum vocabulary itself (buildStatus 'FAULT' where 'SUCCEEDED' is required, phase 'PROVISIONING' with phaseStatus 'FAULT'); the project name and build id in any one run ('sim2real-xsvc-cb-<suffix>:<build-uuid>') are values taken from the request and the emulator's state and are not part of the requirement.

Why: a client running the standard container CI/CD topology (CodeBuild builds the image -> ECR holds it by digest -> ECS/Fargate rolls it out behind an ALB) gets no working chain. Every build is reported as an internal CodeBuild fault, so the client cannot distinguish an emulator limitation from a real infrastructure failure, no image digest is ever produced, and all deploy-by-digest and circuit-breaker-rollback logic downstream is unreachable and untestable.

Fix: the CodeBuild build-execution path behind StartBuild/BatchGetBuilds — the build phase state machine and whatever executes the buildspec — must advance a valid build past PROVISIONING and drive a successful buildspec to a terminal buildStatus 'SUCCEEDED', writing the image the buildspec pushes into the ECR repository so the ECS/ELBv2 half of the chain can be exercised. Related open issues record this same PROVISIONING FAULT for other service groupings (20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md, 20261029T174654Z-codebuild-codepipeline-ecr-build-actions-are-never-executed.md), and 20261104T040200Z-codebuild-ecr-ecs-elbv2-logs-the-codebuild-ecr.md plus 20261105T082918Z-codebuild-ecr-ecs-elbv2-logs-the-circuit-breaker.md cover this service grouping — but all of those carry replay keys for a 60s harness timeout, whereas these two checks now run far enough to capture the FAULT verdict itself, so the suite reports them as owned by none of them.

Traces: codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image, codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served.

## Evidence
Suite: service codebuild__ecr__ecs__elbv2__logs scored 0/2 (0%), 2 failed.

[mismatch] codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image
  check_..._cicdchain_alb_body_tracks_the_latest_codebuild_image.py line 80: assert done1["buildStatus"] == "SUCCEEDED"
  AssertionError: build '<project>:<build-id>' ended 'FAULT'; phase 'PROVISIONING'='FAULT' this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed

[mismatch] codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served
  check_..._cicdchain_unhealthy_build_rollout_rejected_and_old_bytes_still_served.py line 77: same assertion, same 'FAULT' / PROVISIONING='FAULT' detail.

Expected (AWS): BatchGetBuilds on the started build reports buildStatus 'SUCCEEDED' with phases SUBMITTED, QUEUED, PROVISIONING, DOWNLOAD_SOURCE, INSTALL, PRE_BUILD, BUILD, POST_BUILD, UPLOAD_ARTIFACTS, FINALIZING, COMPLETED each 'SUCCEEDED'; the image the buildspec pushes is then addressable in ECR by a 'sha256:' digest, and the ECS service pinned to that digest serves the baked body through the ALB (rolloutState COMPLETED), with the broken revision reaching rolloutState FAILED and rolling back in the second check.

Actual (emulator): buildStatus 'FAULT' with phases [SUBMITTED/SUCCEEDED, QUEUED/SUCCEEDED, PROVISIONING/FAULT, COMPLETED], and the non-AWS reason text 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'. Nothing downstream (digest, task definition revision, target health, ALB body, rolloutState) is produced.

## Steps to Reproduce
1. Create an ECR repository and two CloudWatch log groups (build logs, task logs).
2. Create an IAM role trusted by codebuild.amazonaws.com with ECR push/pull + logs permissions, and a task-execution role trusted by ecs-tasks.amazonaws.com with ECR pull permissions.
3. CreateProject with a buildspec that builds a tiny HTTP-serving image (baking a token, the build id and the tag into the served body) and docker-pushes it to that repository.
4. StartBuild with environmentVariablesOverride carrying the token, image tag and Dockerfile; the response reports buildStatus 'IN_PROGRESS'.
5. Poll BatchGetBuilds until the build leaves IN_PROGRESS.
   -> AWS: buildStatus 'SUCCEEDED', every phase 'SUCCEEDED'.
   -> Emulator: buildStatus 'FAULT' with phase PROVISIONING='FAULT' and the reason 'this emulator does not run CodeBuild build containers, ...'.
6. (Unreachable on the emulator) DescribeImages for the pushed tag, register a task definition pinning repoUri@digest, create a Fargate service behind the ALB target group, and assert the ALB serves the baked body; then deploy a second build by digest and assert rolloutState COMPLETED, and a health-check-failing revision reaches rolloutState FAILED and rolls back while the ALB keeps serving the previous bytes.
