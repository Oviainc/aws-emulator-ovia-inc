Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served
Issue-Key: codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image|_drop_vpc#2|Timeout|time.sleep(10)
Issue-Key: codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served|_drop_vpc#2|Timeout|time.sleep(10)
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# CodeBuild -> ECR -> ECS/ALB CI-CD chain never converges: neither the rollout nor the rollback end-to-end check ever completes, so the ALB never serves build-baked bytes

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
Cause 1 of 1: the full container CI/CD chain (CodeBuild builds an image -> pushes it to ECR -> ECS/Fargate runs it behind an ELBv2 ALB -> an unauthenticated HTTP GET returns the bytes the build baked in) never reaches the point where any of its assertions can be evaluated against this emulator. Both end-to-end checks in this suite were cut off by the harness with no AWS-level assertion failure recorded.
  AWS: recorded behavior only (these checks are data-plane/e2e; no single modeled response carries the claim). The recordings show the happy path converging — a build reaches a terminal SUCCEEDED, PutImage mints a digest in ECR, the ECS service's new task revision becomes healthy behind the target group, and a plain GET to the ALB DNS name returns a body containing both the token the build invented and the CODEBUILD_BUILD_ID only CodeBuild could know. The rejection recording shows the deployment-level verdict instead: the bad revision reports rolloutState FAILED, ECS rolls back, and the ALB keeps serving the previous revision's bytes.
  Emulator: neither check completes. Both report only 'Check timed out after 60.0s' — the chain never converges to any observable state, good or bad, and no step-level divergence is attributable from the replay output.
  Why: a client cannot use this emulator to exercise any container CI/CD workflow: nothing an image build produces ever becomes reachable through the registry, the service or the load balancer, and the deployment-circuit-breaker rollback behavior that the second check asserts has no observable outcome at all. A test suite built on this chain hangs rather than failing a specific assertion.
  Hypothesis: the first hop is the blocker — this emulator's CodeBuild builds never execute (every build FAULTs at PROVISIONING, which open issue 20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md records for the single-service checks, and src/ops/start_sandbox.rs documents the same "nothing is actually provisioned" design for sandboxes), so no image is ever built or pushed and every later hop waits on an artifact that never appears. That attribution is NOT established by this run's output, which carries only the timeouts; the ECR/ECS/ELBv2/Logs hops of the chain are unverified and may hide further divergences once builds execute.
  Fix: the CodeBuild build execution/state machine for the first hop (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build.rs and the build settle logic in src/state.rs), then re-replay these two checks to expose whatever the ECR PutImage / ECS deployment / ELBv2 target-health / CloudWatch Logs hops do once an image exists.
  Traces: codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image, codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served.

## Evidence
Expected (recorded): cicdchain-alb-body-tracks-the-latest-codebuild-image — a GET to the ALB DNS name returns a body containing the check's token and the build's CODEBUILD_BUILD_ID; cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served — the unhealthy revision's deployment reports rolloutState FAILED, ECS rolls back, and the ALB still returns the previous build's bytes. Actual: both checks produced no assertion result — the only detail recorded is 'Error: Check timed out after 60.0s' for each, i.e. the chain never converged within the harness window.

## Steps to Reproduce
Replay the two container CI/CD-chain checks (SERVICES = codebuild, ecr, ecs, elbv2, logs): create an ECR repository, a CodeBuild project whose buildspec builds and pushes an image carrying a unique token plus CODEBUILD_BUILD_ID, an ECS/Fargate service for that image behind an ELBv2 ALB target group, run StartBuild, wait for the service to settle, then issue a plain HTTP GET to the ALB DNS name and compare the body. Against this emulator the sequence never reaches the GET assertion and is cut off at the harness timeout.

## Resolution

PARTIAL, and deliberately so — read this before assuming the whole chain converges.

What was fixed is the cause both of this report's `Issue-Key` lines attribute the
divergence to: `_drop_vpc#2|Timeout|time.sleep(10)`, the checks' EC2 VPC teardown. EC2's
`CreateSubnet`, `DeleteSubnet`, `DeleteSecurityGroup` and `DeleteVpc` answered HTTP 501, so
a VPC these checks created could never be given a subnet and could never be removed; a
teardown that retries its delete and sleeps between attempts therefore consumed the whole
60 s harness window and the check died inside that sleep, with no AWS-level assertion
reached and the body's own divergence masked. Those four operations now implement the
VPC-scoped network resource lifecycle (create into the named VPC, read back through the
describe, delete only while nothing depends on it, `DependencyViolation` otherwise), so the
teardown completes and a check of this chain reports its own outcome instead of a timeout.

What is DECLINED, and tracked elsewhere:

* The first hop — CodeBuild executing a build that bakes a token into a container image and
  pushes it to ECR — needs a container runtime this emulator does not have. A build ends in
  CodeBuild's own `FAULT` with a phase context saying exactly that, which is the modelled
  failure state rather than a fabricated success. Tracked by
  `issues/things-that-do-not-work/20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md`.
* Because of that, neither check's end-state assertion (the ALB serving the bytes a build
  baked in; the rollout verdict for a revision that never ran) can pass in this emulator,
  and the ECR / ECS / ELBv2 / Logs hops of the chain remain unverified.
* The EC2 operations the chain's SETUP also wants still answer 501, each with its own open
  stub issue: `DescribeNetworkInterfaces`, `ModifyVpcAttribute`, `DescribeVpcAttribute`,
  `CreateInternetGateway` and the route-table operations — this crate models no ENI, no VPC
  DNS attribute, no gateway and no route table at all, so a 501 ("not written yet") is the
  honest answer for them.
