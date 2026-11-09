Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr.repository-pull-count-counts-the-registry-v2-pulls
Issue-Key: cloudwatch__ecr.repository-pull-count-counts-the-registry-v2-pulls|_get#10|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# cloudwatch__ecr: the ECR registry data-plane address is published as a real-AWS <account>.dkr.ecr.<region>.amazonaws.com host, so no registry v2 pull can be made against the emulator

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRepositories

## Description
Cause 1 of 1: ECR publishes a repository/registry URI on a host the emulator does not serve.
AWS: a repository's repositoryUri is the registry host that actually answers the Docker Registry v2 API for that account and region, so a client can GET /v2/<name>/manifests/<tag> at the address the API handed it. Smithy (ECR): Repository$repositoryUri is the URI a client uses to pull/push the repository's images; recorded behavior for the data plane.
Emulator: the published URI is '<accountId>.dkr.ecr.<region>.amazonaws.com/<repository>' — a real-AWS hostname that is not one of the emulator's own hosts. The replay harness refuses the announced address: a customer of the emulator needs the registry on the emulator's own listener (http(s)://<own host>:<port>/<path>). The manifest GET that would record a pull therefore never reaches the emulator.
Why: every client that follows the address the API published (docker pull, a registry v2 client, anything deriving the pull URL from repositoryUri) is sent to AWS's real DNS name instead of the emulator; the pull never happens, so the repository's pull count and all pull-driven metrics stay empty.
Fix: ECR's repository/registry URI serializer (publish the emulator's own registry listener host and port in repositoryUri and in any registry endpoint it announces).
Traces: cloudwatch__ecr.repository-pull-count-counts-the-registry-v2-pulls.

## Evidence
simparity.published.PublishedUrlRejected: endpoint_url was given https://000000000000.dkr.ecr.us-west-2.amazonaws.com/v2/<repository>/manifests/v1 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

Expected: the announced registry address is a host/port the emulator listens on, and the registry v2 manifest GET succeeds and is counted as a pull.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay cloudwatch__ecr.repository-pull-count-counts-the-registry-v2-pulls.
3. Or: CreateRepository, read repository.repositoryUri, and issue GET /v2/<repository>/manifests/<tag> against that host — the host is an amazonaws.com name the emulator does not serve.
