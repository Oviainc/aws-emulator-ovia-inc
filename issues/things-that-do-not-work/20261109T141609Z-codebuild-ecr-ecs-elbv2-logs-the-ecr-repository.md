Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served
Issue-Key: codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image|run#2|AssertionError|assert not (p := ctx.published_url_mismatch(f"https://{repo_uri}")), p
Issue-Key: codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served|run#2|AssertionError|assert not (p := ctx.published_url_mismatch(f"https://{repo_uri}")), p
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# codebuild__ecr__ecs__elbv2__logs: the ECR repository URI announced to the CI-CD chain is a real-AWS dkr.ecr host the emulator does not serve, so no image can be pushed or pulled by address

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
Cause 1 of 1: ECR publishes the repository's pull/push address on a host that is not the emulator's.
AWS: Repository$repositoryUri is the address a client (docker, a build, an ECS task) uses to push and pull that repository's images, and it resolves to the registry endpoint actually serving the account/region. Smithy (ECR): Repository$repositoryUri; the data-plane behavior is recorded only.
Emulator: the announced URI is '<accountId>.dkr.ecr.<region>.amazonaws.com/<repository>' — a real-AWS hostname that is not one of the emulator's own hosts, so the harness refuses the published address (a customer of the emulator needs http(s)://<own host>:<port>/<path>). Nothing in the CodeBuild→ECR→ECS/ALB chain can address the registry.
Why: every consumer that uses the address the API published — a docker push in a build, an ECS task definition image, a manifest GET — is sent to AWS's DNS name instead of the emulator; the image never lands, so the whole deploy chain is unobservable.
Fix: ECR's repository URI/registry endpoint serializer (announce the emulator's own registry listener host and port).
Traces: codebuild__ecr__ecs__elbv2__logs.cicdchain-alb-body-tracks-the-latest-codebuild-image, codebuild__ecr__ecs__elbv2__logs.cicdchain-unhealthy-build-rollout-rejected-and-old-bytes-still-served.

## Evidence
AssertionError: the emulator published https://000000000000.dkr.ecr.us-west-2.amazonaws.com/<repository>: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

(identical for both checks, with different repositories and listener ports)

Expected: repositoryUri names a host:port the emulator serves, so a push/pull against it reaches the emulator's registry.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay the two checks in trace_ids (service codebuild__ecr__ecs__elbv2__logs).
3. Or: CreateRepository and read repository.repositoryUri — the host is <accountId>.dkr.ecr.<region>.amazonaws.com, which the emulator does not listen on.
