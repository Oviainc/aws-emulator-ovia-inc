Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr.repository-pull-count-counts-the-registry-v2-pulls
Issue-Key: cloudwatch__ecr.repository-pull-count-counts-the-registry-v2-pulls|_get#10|AssertionError|raise AssertionError(
Issue-Commit: b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7

# cloudwatch__ecr: the ECR Docker Registry v2 data-plane endpoint is not routed — a /v2 manifest GET answers 404 UnknownService, so a registry pull (and RepositoryPullCount) is unreachable

> _Note: this issue was identified against the code base at commit `b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDownloadUrlForLayer

## Description
ONE net-new root cause.

Cause 1 of 1 — requests to the ECR registry host's Docker Registry v2 paths are not dispatched to the ECR service at all.
AWS: recorded behavior — ECR serves the Docker Registry v2 HTTP API on the registry host <accountId>.dkr.ecr.<region>.amazonaws.com: GET /v2/<repositoryName>/manifests/<reference> with the GetAuthorizationToken Basic credential returns the manifest (200) and GET /v2/<repositoryName>/blobs/<digest> returns the blob; these pulls are what `docker pull` performs, and ECR counts them (AWS/ECR RepositoryPullCount, dimension RepositoryName, one-minute periods). The check's metric assertions (Sum, StandardUnit membership, and the never-pulled twin repository publishing no datapoint) are hypotheses it records; the wire behavior above is not.
Emulator: the request reaches the emulator and is answered HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — the front door cannot attribute a registry-v2 request (which carries Basic auth and no X-Amz-Target) to ECR, so no manifest or blob is ever served and no pull is ever counted. This is the emulator's own routing decision, not a transport or name-resolution failure of the runner: the call completed and the emulator authored the body.
Why: no Docker-compatible client can pull from the emulated registry — the one data path `docker pull`/containerd uses returns a service-discovery error. Everything downstream (image pulls by ECS/EKS/CodeBuild over the registry protocol, pull-count metrics, pull-time bookkeeping) is unreachable, and the error a client gets names an unknown service rather than anything about the repository or its credentials.
Fix: the request-routing front door must resolve host/path-shaped registry traffic (<account>.dkr.ecr.<region>.* and /v2/** with Basic authorization) to the ECR service, and ECR must serve the registry-v2 manifest/blob GETs behind it; the pull must then increment the AWS/ECR RepositoryPullCount datapoint for the repository.
Traces: cloudwatch__ecr.repository-pull-count-counts-the-registry-v2-pulls.

## Evidence
[mismatch] cloudwatch__ecr.repository-pull-count-counts-the-registry-v2-pulls, raised in the check's _get helper (line 306) from _pull_image (line 321):
"AssertionError: Docker Registry v2 GET https://000000000000.dkr.ecr.us-west-2.amazonaws.com/v2/sim2real-cwecr-pulled-<id>/manifests/v1 failed: HTTP 404 b'{\"__type\":\"UnknownService\",\"message\":\"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)\"}'".
The repository was created and pushed to successfully through the ECR API immediately before this request, and the request was issued through the harness's own endpoint routing, so the host reached the emulator. Account id, region, repository name and tag are per-run values; the requirement is that /v2/<repo>/manifests/<ref> on the registry host is served by ECR rather than refused as an unknown service.

## Steps to Reproduce
1) ecr CreateRepository(<repo>) and push an image (InitiateLayerUpload/UploadLayerPart/CompleteLayerUpload/PutImage). 2) ecr GetAuthorizationToken and build the Basic authorization header from the token. 3) GET https://<accountId>.dkr.ecr.<region>.amazonaws.com/v2/<repo>/manifests/<tag> with that Authorization header and Accept: application/vnd.docker.distribution.manifest.v2+json. The emulator answers 404 {"__type":"UnknownService",...} instead of the manifest. 4) cloudwatch GetMetricStatistics for AWS/ECR RepositoryPullCount, dimension RepositoryName=<repo>: no datapoint exists.
