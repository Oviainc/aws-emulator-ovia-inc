Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.authorization-token-custom-duration-sets-expiration codeartifact.authorization-token-default-duration-12h codeartifact.authorization-token-duration-shapes-expiry codeartifact.codeartifact-archived-version-tarball-download-rejected-while-published-sibling-serves codeartifact.codeartifact-external-connection-upstream-fetch-then-disassociate-blocks codeartifact.codeartifact-npm-publish-and-install-through-repository-endpoint codeartifact.codeartifact-package-origin-configuration-blocks-upstream-ingestion codeartifact.codeartifact-pypi-simple-index-serves-a-published-wheel codeartifact.codeartifact-repository-policy-denying-read-blocks-the-endpoint-and-a-sibling-still-serves codeartifact.codeartifact-scoped-npm-package-round-trips-under-its-url-encoded-namespace codeartifact.codeartifact-unlisted-version-is-hidden-from-the-packument-but-still-downloadable codeartifact.codeartifact-upstream-chain-resolves-through-two-hops-and-caches-in-the-front-repository codeartifact.e2e-external-connection-upstream-fetch-then-disassociate-blocks codeartifact.external-connection-caches-fetched-package codeartifact.ingested-package-defaults-to-publish-block-and-origin-type-partitions-the-versions codeartifact.list-package-version-dependencies-preserves-declared-order codeartifact.npm-client-installs-a-semver-range-and-verifies-integrity-against-the-repository codeartifact.package-origin-configuration-blocks-upstream-ingestion codeartifact.package-version-details-are-extracted-from-the-manifest-and-latest-decides-the-display-version codeartifact.package-version-readme-unicode-round-trip codeartifact.repository-endpoint-unauthenticated-fetch-rejected codeartifact.repository-permissions-policy-enforced-for-cross-principal-access codeartifact.zero-duration-token-expires-with-the-role-session-while-a-long-duration-outlives-it
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.authorization-token-custom-duration-sets-expiration|_routed_request#19|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.authorization-token-default-duration-12h|_routed_request#18|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.authorization-token-duration-shapes-expiry|_routed_request#16|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.codeartifact-archived-version-tarball-download-rejected-while-published-sibling-serves|_routed_request#13|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.codeartifact-external-connection-upstream-fetch-then-disassociate-blocks|_routed_request#11|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.codeartifact-npm-publish-and-install-through-repository-endpoint|_routed_request#13|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.codeartifact-package-origin-configuration-blocks-upstream-ingestion|_routed_request#14|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.codeartifact-pypi-simple-index-serves-a-published-wheel|_routed_request#18|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.codeartifact-repository-policy-denying-read-blocks-the-endpoint-and-a-sibling-still-serves|_routed_request#14|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.codeartifact-scoped-npm-package-round-trips-under-its-url-encoded-namespace|_routed_request#16|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.codeartifact-unlisted-version-is-hidden-from-the-packument-but-still-downloadable|_routed_request#12|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.codeartifact-upstream-chain-resolves-through-two-hops-and-caches-in-the-front-repository|_routed_request#20|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.e2e-external-connection-upstream-fetch-then-disassociate-blocks|_routed_request#11|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.external-connection-caches-fetched-package|_routed_request#16|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.ingested-package-defaults-to-publish-block-and-origin-type-partitions-the-versions|_routed_request#17|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.list-package-version-dependencies-preserves-declared-order|run#11|PublishedUrlRejected|endpoint = ctx.endpoint_url(
Issue-Key: codeartifact.npm-client-installs-a-semver-range-and-verifies-integrity-against-the-repository|run#11|PublishedUrlRejected|endpoint = ctx.endpoint_url(ca.get_repository_endpoint(domain=domain, repository=repository,
Issue-Key: codeartifact.package-origin-configuration-blocks-upstream-ingestion|run#12|PublishedUrlRejected|endpoint = ctx.endpoint_url(
Issue-Key: codeartifact.package-version-details-are-extracted-from-the-manifest-and-latest-decides-the-display-version|_routed_request#12|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: codeartifact.package-version-readme-unicode-round-trip|_npm_publish#13|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(endpoint.rstrip("/") + f"/{package_name}")
Issue-Key: codeartifact.repository-endpoint-unauthenticated-fetch-rejected|run#9|AssertionError|assert not (p := ctx.published_url_mismatch(endpoint)), p
Issue-Key: codeartifact.repository-permissions-policy-enforced-for-cross-principal-access|run#20|AssertionError|assert not (p := ctx.published_url_mismatch(endpoint)), p
Issue-Key: codeartifact.zero-duration-token-expires-with-the-role-session-while-a-long-duration-outlives-it|_routed_request#42|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Commit: 0cee419aadfb4868f9ccd3aa23bc5045ac08a29b

# codeartifact: GetRepositoryEndpoint publishes a real-AWS d.codeartifact host the emulator does not serve, so the whole repository data plane (npm/pypi publish+install, upstreams, external connections, repository policy, auth tokens) is unreachable

> _Note: this issue was identified against the code base at commit `0cee419aadfb4868f9ccd3aa23bc5045ac08a29b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepositoryEndpoint

## Description
Cause 1 of 1: GetRepositoryEndpoint answers 200 with a repositoryEndpoint whose authority is a real AWS production hostname instead of the address the emulator is actually reached on, so every behavioral check that does what the API exists for — take repositoryEndpoint and drive the package plane through it — never reaches the emulator at all. 23 failing checks in this service are blocked at exactly this point, before any package-plane behavior can be observed.

AWS: Smithy (com.amazonaws.codeartifact#CodeArtifactControlPlaneService) GetRepositoryEndpoint — GET /v1/repository/endpoint, success code 200; required httpQuery inputs `domain` (DomainName, length 2–50, pattern ^[a-z][a-z0-9\-]{0,48}[a-z0-9]$), `repository` (RepositoryName, length 2–100, pattern ^[A-Za-z0-9][A-Za-z0-9._\-]{1,99}$) and `format` (PackageFormat enum), optional `domain-owner` (AccountId, 12 digits) and `endpointType` (EndpointType enum); output GetRepositoryEndpointResult$repositoryEndpoint (String), documented as "A string that specifies the URL of the returned endpoint". Modeled errors: ValidationException 400, ResourceNotFoundException 404, AccessDeniedException 403, ThrottlingException 429, InternalServerException 500. On real AWS that URL's host is served by AWS itself, which is the whole point of the operation: the returned URL is the address an npm/pip/twine client publishes to and installs from. The requirement is therefore not a literal string — the path shape /<format>/<repository>/ and the variable parts (domain, domain owner, region, format, repository) come from the request and state — but the invariant that the announced authority is one the responder serves. Recorded behavior only for the data plane itself, which these checks then exercise: an authenticated GET of a packument returns 200 JSON, an unknown package returns 404 with a "not found" error body, an Archived version's tarball returns 400 naming the status, an unauthenticated fetch is refused, and a repository policy denying read blocks the endpoint while a sibling repository still serves.

Emulator: repositoryEndpoint is returned verbatim in AWS production form, e.g. https://<domain>-<domainOwner>.d.codeartifact.<region>.amazonaws.com/<format>/<repository>/ . The harness rejects it before issuing a request — "simparity.published.PublishedUrlRejected: endpoint_url was given https://<domain>-<account>.d.codeartifact.<region>.amazonaws.com/npm/<repo>/... (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener" — and two checks assert on it directly ("the emulator published https://<domain>-<account>.d.codeartifact.<region>.amazonaws.com/npm/<repo>/: its host is not one of the emulator's own hosts"). The authority is the only divergence; the status is the expected 200 and the path shape is right. (Note: 127.0.0.1 and the port differ per run and are not part of the requirement — the requirement is "the host and port the request arrived on".)

Why: a client that follows the documented flow (GetAuthorizationToken, then point a package manager at repositoryEndpoint) leaves the emulator. The name either does not resolve or resolves to real AWS and answers an unrelated credential failure, surfacing as a network/signature error the client cannot act on. Nothing behind the endpoint can be exercised: npm and pypi publish/install, scoped (URL-encoded namespace) packages, the pypi simple index, unlisted/archived version visibility, external-connection pull-through and its disassociation refusal, two-hop upstream resolution and front-repository caching, package-origin publish blocks, manifest-derived version detail and readme round-trips, dependency ordering, repository-permissions-policy enforcement, and all three authorization-token lifetime checks (which prove the token works by authenticating the endpoint).

Fix: the GetRepositoryEndpoint handler's response construction in the codeartifact service crate (the endpoint/URL synthesis), which must emit an authority the emulator serves — the host and port the request arrived on, or the per-resource listener it stands up — while preserving the modeled path shape /<format>/<repository>/ and honouring endpointType; and the repository data-plane listener/routing that must answer on that authority. Related open issue, which does NOT own these checks (it is a wire-corpus finding keyed codeartifact|GetRepositoryEndpoint|2xx->2xx|$.repositoryEndpoint and the suite reported none of these checks as owned): 20261109T130849Z-codeartifact-getrepositoryendpoint-announces-a-real-aws-d-codeartifact.md — it describes the same announced-host defect from the wire replay; these 23 behavioral checks are the downstream data-plane behavior that stays unverifiable until the announced authority is the emulator's.

Traces: the 23 failing check ids listed in trace_ids, every one of which fails at its first use of the published repository endpoint.

## Evidence
Representative failures (all under ### codeartifact):

[mismatch] codeartifact.authorization-token-custom-duration-sets-expiration
  File "context.py", line 572, in endpoint_route -> _published_target
  simparity.published.PublishedUrlRejected: endpoint_url was given https://sim2real-behav-ca-<id>-000000000000.d.codeartifact.us-west-2.amazonaws.com/npm/sim2real-behav-ca-repo-<id>/-/ping?write=true (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:36977/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

[mismatch] codeartifact.codeartifact-pypi-simple-index-serves-a-published-wheel
  PublishedUrlRejected: endpoint_url was given https://sim2real-ca-<id>-000000000000.d.codeartifact.us-west-2.amazonaws.com/pypi/sim2real-repo-<id>/ ... its host is not one of the emulator's own hosts

[mismatch] codeartifact.repository-endpoint-unauthenticated-fetch-rejected
  AssertionError: the emulator published https://sim2real-behav-ca-dom-<id>-000000000000.d.codeartifact.us-west-2.amazonaws.com/npm/sim2real-behav-ca-npm-<id>/: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:39377/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

[mismatch] codeartifact.repository-permissions-policy-enforced-for-cross-principal-access
  AssertionError: the emulator published https://sim2real-carepopol-<id>-000000000000.d.codeartifact.us-west-2.amazonaws.com/npm/repo-<id>/: its host is not one of the emulator's own hosts

Expected: GetRepositoryEndpoint -> 200, repositoryEndpoint = <scheme>://<host:port the emulator serves>/<format>/<repository>/ , and that URL answers the package plane (authenticated packument GET 200; absent package 404 with a "not found" error body; Archived version tarball 400 naming the status; unauthenticated fetch refused; policy-denied read blocked while a sibling repository still serves 200).
Actual: 200 with repositoryEndpoint on the AWS production authority <domain>-<domainOwner>.d.codeartifact.<region>.amazonaws.com — an address the emulator does not serve — so no request is ever made and every dependent assertion is unreachable.

Identifiers, ports, domain/repository names and the account id above are per-run values, not requirements.

## Steps to Reproduce
1. Boot the emulator and point a codeartifact client at it.
2. CreateDomain(domain=<d>), CreateRepository(domain=<d>, repository=<r>).
3. GetAuthorizationToken(domain=<d>) and note authorizationToken/expiration.
4. GetRepositoryEndpoint(domain=<d>, repository=<r>, format=npm) (and format=pypi).
5. Observe the 200 body: repositoryEndpoint's authority is <d>-<account>.d.codeartifact.<region>.amazonaws.com, not the host:port the request was sent to.
6. Send any package-plane request to the returned URL (e.g. GET <endpoint>/<package> with the token, or `npm publish`/`pip install --index-url <endpoint>simple/`): it never reaches the emulator. Every check in trace_ids stops here.
