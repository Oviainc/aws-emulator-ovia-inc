Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-authorization-token-and-repository-endpoint
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-authorization-token-and-repository-endpoint codeartifact-repository-endpoint-formats
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|GetRepositoryEndpoint|2xx->2xx|$.repositoryEndpoint
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# CodeArtifact GetRepositoryEndpoint announces a real-AWS d.codeartifact host the emulator does not serve

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepositoryEndpoint

## Description
Cause 1 of 1: GetRepositoryEndpoint returns a repositoryEndpoint whose authority is a real AWS production hostname rather than an address on the emulator's own listener, so the repository data plane it advertises is not reachable on this emulator.

AWS: Smithy (codeartifact control plane): GetRepositoryEndpoint, GET /v1/repository/endpoint, success 200; required query inputs domain, repository, format (PackageFormat enum), optional domain-owner and endpointType; output GetRepositoryEndpointResult$repositoryEndpoint (String), documented as "A string that specifies the URL of the returned endpoint." Modeled errors: ValidationException 400, ResourceNotFoundException 404, AccessDeniedException 403, ThrottlingException 429, InternalServerException 500. The recorded response is 200 carrying a URL of the shape https://<domain>-<domainOwner>.d.codeartifact.<region>.amazonaws.com/<format>/<repository>/ — on real AWS that host is served by AWS itself, so the URL is the address a package client is meant to publish to and read from. The requirement being asserted here is therefore not the literal string but the invariant the replay checks (verdict host_not_own): the announced endpoint must be an authority the responder actually serves. The variable parts of the URL are taken from the request and state (domain name, domain owner account, region, package format, repository name); the fixed parts are the scheme, the ".d.codeartifact.<region>.amazonaws.com" suffix shape and the trailing "/<format>/<repository>/" path.

Emulator: for every GetRepositoryEndpoint call in the corpus the emulator echoes the AWS-shaped production host verbatim (host_not_own on $.repositoryEndpoint; status is 200 as expected, only the announced authority diverges). It publishes a hostname outside the address it is being reached on, instead of rewriting the authority to its own data listener while keeping the AWS path shape.

Why: a client that does what the API exists for — take repositoryEndpoint and point npm/pip/twine or a generic PublishPackageVersion/GetPackageVersionAsset request at it — leaves the emulator entirely. The name either does not resolve, or resolves to real AWS and answers an unrelated credential/authorization failure. Every publish-and-read and upstream/external-connection workflow that is driven through the advertised endpoint is unreachable, and the failure surfaces as a network or signature error rather than as anything the client can act on.

Fix: the GetRepositoryEndpoint handler's response construction in the codeartifact service crate (endpoint/URL synthesis), which must emit an authority the emulator serves — the host and port the request arrived on — while preserving the modeled path shape /<format>/<repository>/ and honouring endpointType. Related open issues describe what breaks downstream of this address but do not own it: 20261029T095746Z-codeartifact-npm-external-connection-ingests-nothing.md and 20261027T083906Z-codeartifact-reopens-codeartifact-pypi-repository-endpoint-still-ingests-nothing.md (both about nothing being resolved THROUGH the repository endpoint, not about the announced host not being the emulator's).

Traces: codeartifact-authorization-token-and-repository-endpoint#task#3 and #task#4; codeartifact-repository-endpoint-formats#task#3, #task#4 and #task#5 (5 steps, one cluster, all GetRepositoryEndpoint).

## Evidence
Replay cluster service=codeartifact|GetRepositoryEndpoint|status=200->200|keys=$.repositoryEndpoint, verdict mismatch, affected_count 5.

Expected (recorded): HTTP 200, $.repositoryEndpoint = a URL on the CodeArtifact domain's own served host, e.g. 'https://<domain>-<domainOwner>.d.codeartifact.us-west-2.amazonaws.com/generic/<repository>/'.
Actual (emulator): HTTP 200, $.repositoryEndpoint = the same AWS-production authority, which this emulator does not serve — replay verdict 'host_not_own' on $.repositoryEndpoint (the announced host is not the one the emulator is reached on).

Affected steps: codeartifact-authorization-token-and-repository-endpoint#task#3, codeartifact-authorization-token-and-repository-endpoint#task#4, codeartifact-repository-endpoint-formats#task#3, codeartifact-repository-endpoint-formats#task#4, codeartifact-repository-endpoint-formats#task#5.

No stub blocks this cluster (stub_path empty) and no open issue owns it (owned_by empty).

## Steps to Reproduce
1. Replay fixture codeartifact-authorization-token-and-repository-endpoint against the emulator and stop at step task#3 (GetRepositoryEndpoint), or replay codeartifact-repository-endpoint-formats steps task#3..task#5 which exercise several package formats.
2. CreateDomain + CreateRepository, then call GET /v1/repository/endpoint?domain=<domain>&repository=<repo>&format=generic.
3. Observe the 200 response body's repositoryEndpoint: its authority is <domain>-<owner>.d.codeartifact.<region>.amazonaws.com, not the host:port the request was sent to.
4. Issue any package-plane request (e.g. PublishPackageVersion or GetPackageVersionAsset) against that returned URL: it never reaches the emulator.
