Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.authorization-token-default-duration-12h codeartifact.codeartifact-archived-version-tarball-download-rejected-while-published-sibling-serves codeartifact.codeartifact-repository-policy-denying-read-blocks-the-endpoint-and-a-sibling-still-serves codeartifact.codeartifact-scoped-npm-package-round-trips-under-its-url-encoded-namespace codeartifact.codeartifact-unlisted-version-is-hidden-from-the-packument-but-still-downloadable codeartifact.codeartifact-upstream-chain-resolves-through-two-hops-and-caches-in-the-front-repository codeartifact.e2e-external-connection-upstream-fetch-then-disassociate-blocks codeartifact.package-version-details-are-extracted-from-the-manifest-and-latest-decides-the-display-version codeartifact.npm-client-installs-a-semver-range-and-verifies-integrity-against-the-repository codeartifact.list-package-version-dependencies-preserves-declared-order codeartifact.zero-duration-token-expires-with-the-role-session-while-a-long-duration-outlives-it codeartifact.authorization-token-needs-sts-bearer-permission-that-a-domain-policy-cannot-grant codeartifact.ingested-package-defaults-to-publish-block-and-origin-type-partitions-the-versions
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.authorization-token-default-duration-12h|run#19|AssertionError|assert status == 200, (
Issue-Key: codeartifact.codeartifact-archived-version-tarball-download-rejected-while-published-sibling-serves|run#13|AssertionError|assert status == 200, f"publishing {version!r} returned {status!r}: {body[:300]!r}"
Issue-Key: codeartifact.codeartifact-repository-policy-denying-read-blocks-the-endpoint-and-a-sibling-still-serves|run#14|AssertionError|assert put_status == 200, (
Issue-Key: codeartifact.codeartifact-scoped-npm-package-round-trips-under-its-url-encoded-namespace|run#17|AssertionError|assert put_status == 200, (
Issue-Key: codeartifact.codeartifact-unlisted-version-is-hidden-from-the-packument-but-still-downloadable|run#12|AssertionError|assert status == 200, f"publishing {version!r} returned {status!r}: {body[:300]!r}"
Issue-Key: codeartifact.codeartifact-upstream-chain-resolves-through-two-hops-and-caches-in-the-front-repository|run#21|AssertionError|assert put_status == 200, f"publish into the back repository returned {put_status!r}: {put_body[:400]!r}"
Issue-Key: codeartifact.e2e-external-connection-upstream-fetch-then-disassociate-blocks|_http_get_bytes#11|HTTPError|with urllib.request.urlopen(req, timeout=timeout_s) as resp: # noqa: S310
Issue-Key: codeartifact.package-version-details-are-extracted-from-the-manifest-and-latest-decides-the-display-version|run#12|AssertionError|assert status == 200, f"publishing {plain}@{version} returned {status!r}: {body[:300]!r}"
Issue-Key: codeartifact.npm-client-installs-a-semver-range-and-verifies-integrity-against-the-repository|run#20|AssertionError|assert published.returncode == 0, (
Issue-Key: codeartifact.list-package-version-dependencies-preserves-declared-order|run#15|AssertionError|raise AssertionError(
Issue-Key: codeartifact.zero-duration-token-expires-with-the-role-session-while-a-long-duration-outlives-it|run#28|AssertionError|assert abs(zero_offset) <= slack, (
Issue-Key: codeartifact.authorization-token-needs-sts-bearer-permission-that-a-domain-policy-cannot-grant|_poll#22|Timeout|time.sleep(interval_s)
Issue-Key: codeartifact.ingested-package-defaults-to-publish-block-and-origin-type-partitions-the-versions|run#16|AssertionError|assert absent == expected_absent, (
Issue-Commit: afc21d5623f1a8f7411b21bbb87a0dff6c2e7024

# CodeArtifact repository endpoints serve nothing (npm registry 404 UnknownService), durationSeconds=0 ignored, GetAuthorizationToken never gated on sts:GetServiceBearerToken, DescribePackage not-found omits the format (4 causes)

> _Note: this issue was identified against the code base at commit `afc21d5623f1a8f7411b21bbb87a0dff6c2e7024`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepositoryEndpoint

## Description
Four distinct root causes in the CodeArtifact emulator, all reached through the codeartifact service. Every control-plane request below is answered (none of these is a missing operation implementation; the operations the suite flagged as scaffold stubs are filed separately). Numbered so each can be closed individually; a fixer must close all four.

Already-open issues that do NOT cover any cause below: 20261022T171312Z-codeartifact-no-iam-authorization-on-reads-publishpackageversion-digest.md (its Cause 1 is identity-policy evaluation on DescribeRepository reads; Cause 3 below is a different decision point — the cross-service STS action that gates token minting) and 20261024T061931Z-codeartifact-cfn-provisioned-domains-absent-resource-policies-never.md (its Cause 7 is the same not-found message helper reached through CopyPackageVersions; Cause 4 below is the same omission reached through DescribePackage and is listed here only so the failing check id is carried by some work item).

Cause 1 of 4: the repository endpoint returned by GetRepositoryEndpoint serves nothing — the whole CodeArtifact package data plane is unreachable.
AWS: GetRepositoryEndpoint (Smithy: GET /v1/repository/endpoint, required domain + repository + format:PackageFormat, output repositoryEndpoint) returns the URL of a repository endpoint that SPEAKS THE NATIVE PACKAGE-MANAGER PROTOCOL for that format. For npm that endpoint is a real npm registry (recorded behavior only — the Smithy model describes only the control plane; the endpoint protocol is AWS's documented npm/pypi/maven endpoint behavior, https://docs.aws.amazon.com/codeartifact/latest/ug/npm-auth.html): GET /-/ping?write=true answers 200 for a valid bearer token, PUT /<package> (or PUT /@scope%2fname for a scoped package) publishes a tarball and answers 200, GET /<package> returns the synthesised packument and GET of the tarball path returns the asset bytes. `npm publish` and `npm install` against an npmrc of the documented shape therefore succeed.
Emulator: nothing is served there. Reached by host, the endpoint's host label is not registered in the emulator's request router and every request answers HTTP 404 with the body {"__type":"UnknownService","message":"no emulated service is registered for '<domain>-<accountId>'"} — this is the emulator announcing an endpoint it does not serve, for publishes, packument reads, tarball downloads and /-/ping alike. Reached by path on the emulator's own address, the same traffic answers a bare HTTP 404 (npm renders it as `npm error 404 Not Found - PUT https://<host>/npm/<repo>/<package>`). Note GetRepositoryEndpoint itself answers 200 with a well-formed URL, so the divergence is entirely in what that URL resolves to.
Why: every CodeArtifact use that matters is unexercisable. A client cannot publish or install a package with its real package manager, cannot verify that a token actually authenticates, and cannot observe any of the data-plane semantics layered on the endpoint (unlisted versions hidden from the packument but still downloadable, archived versions refusing tarball downloads while a published sibling serves, upstream chains resolving and caching in the front repository, external-connection fetch then disassociate, scoped packages under their URL-encoded namespace, manifest-extracted version details). Because the refusal is a 404 UnknownService rather than an HTTP-level auth or not-found answer from the repository, a client cannot even tell that it reached CodeArtifact.
Fix: two layers, both in the CodeArtifact service: (a) the endpoint/host routing table must register the repository-endpoint host form that GetRepositoryEndpoint hands out (and the /<format>/<repository>/ path form) so those requests dispatch into CodeArtifact instead of falling through to the unknown-service responder; (b) the package-format data-plane handlers (npm registry protocol first: /-/ping, PUT package, packument GET, tarball GET), backed by the same package/asset state the control-plane PublishPackageVersion and GetPackageVersionAsset handlers use, so a publish over the endpoint and a publish over the control plane are the same version.
Traces: codeartifact.authorization-token-default-duration-12h (the 12h expiration assertion PASSED; it fails proving the minted token authenticates: /-/ping?write=true answered 404), codeartifact.codeartifact-archived-version-tarball-download-rejected-while-published-sibling-serves, codeartifact.codeartifact-repository-policy-denying-read-blocks-the-endpoint-and-a-sibling-still-serves, codeartifact.codeartifact-scoped-npm-package-round-trips-under-its-url-encoded-namespace, codeartifact.codeartifact-unlisted-version-is-hidden-from-the-packument-but-still-downloadable, codeartifact.codeartifact-upstream-chain-resolves-through-two-hops-and-caches-in-the-front-repository, codeartifact.e2e-external-connection-upstream-fetch-then-disassociate-blocks, codeartifact.package-version-details-are-extracted-from-the-manifest-and-latest-decides-the-display-version, codeartifact.npm-client-installs-a-semver-range-and-verifies-integrity-against-the-repository, codeartifact.list-package-version-dependencies-preserves-declared-order. The same cause also blocks the check id codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event, filed separately because its id carries a different leading service token.

Cause 2 of 4: GetAuthorizationToken ignores durationSeconds=0.
AWS: Smithy, GetAuthorizationTokenRequest.durationSeconds (range 0..43200): "The time, in seconds, that the generated authorization token is valid. Valid values are 0 and any number between 900 (15 minutes) and 43200 (12 hours). A value of 0 will set the expiration of the authorization token to the same expiration of the user's role's temporary credentials." So when the caller is an assumed-role session, durationSeconds=0 must yield an `expiration` equal to that session's own credential Expiration, not a default lifetime.
Emulator: the member is accepted (HTTP 200) and then discarded — under a 900s AssumeRole session the token came back with an expiration one hour from issuance, 2700s PAST the session's reported Expiration (observed: session Expiration 02:51:40Z, token expiration 03:36:40Z). The default-duration path is right (the 12h default is honoured in Cause 1's check), so only the 0 case is unimplemented.
Why: 0 is the documented way to make a repository credential die with the session that minted it. A client that asks for it gets a token that outlives the session, so credential-lifetime assumptions (and any test asserting the token cannot be used after the session ends) are validated green against an emulator that hands out a longer-lived credential than AWS would.
Fix: the durationSeconds handling in the GetAuthorizationToken handler (services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/get_authorization_token.rs) — the 0 case must resolve the expiration from the calling session's temporary-credential expiry rather than applying a default.
Traces: codeartifact.zero-duration-token-expires-with-the-role-session-while-a-long-duration-outlives-it.

Cause 3 of 4: GetAuthorizationToken is not gated on the caller's identity permissions, so removing sts:GetServiceBearerToken never produces a refusal.
AWS: minting a CodeArtifact token requires BOTH codeartifact:GetAuthorizationToken and the cross-service sts:GetServiceBearerToken in the CALLER'S IDENTITY policy (https://docs.aws.amazon.com/codeartifact/latest/ug/tokens-authentication.html); a session holding codeartifact:GetAuthorizationToken but not sts:GetServiceBearerToken is refused with AccessDeniedException (Smithy: GetAuthorizationToken errors -> AccessDeniedException, httpError 403). The 403/AccessDeniedException classification is the requirement; the failing check is an unblessed DRAFT, so its exact message wording ("...is not authorized to perform: sts:GetServiceBearerToken...") is that check's hypothesis, not an observation, and is not part of the requirement.
Emulator: the token mints unconditionally. With the positive control in place the token is issued (correct); after the inline policy is rewritten to drop sts:GetServiceBearerToken the identical call KEEPS returning HTTP 200 with a token, so the check's bounded poll for the refusal never observes one and the harness kills the check at its 60s ceiling (reported verdict: "Check timed out after 60.0s"). The timeout is the absence of the refusal, not a slow response.
Why: the emulator hands a working repository credential to a principal real AWS refuses, and the discriminating leg behind this check — that sts:GetServiceBearerToken granted in a CodeArtifact DOMAIN resource policy has no effect and must NOT substitute for the identity grant — can never be reached, so a generically-implemented policy evaluator that would wrongly allow it goes undetected. Least-privilege tests for token minting pass vacuously.
Fix: the authorization decision ahead of the GetAuthorizationToken handler in the CodeArtifact request pipeline — it must evaluate the caller's identity policy for BOTH codeartifact:GetAuthorizationToken and sts:GetServiceBearerToken and return the modeled AccessDeniedException (403), and must not treat a domain/repository resource-policy grant of the STS action as satisfying it.
Traces: codeartifact.authorization-token-needs-sts-bearer-permission-that-a-domain-policy-cannot-grant.

Cause 4 of 4: the DescribePackage not-found refusal does not identify the package format it looked for.
AWS: DescribePackage (Smithy: GET /v1/package, required domain + repository + format + package, errors -> ResourceNotFoundException httpError 404) refuses an absent package with ResourceNotFoundException whose message is BUILT FROM THE REQUEST and names the addressed format alongside the package name, e.g. "Package not found in repository. Package = {format=npm, name=left-pad}" (the failing check is an unblessed DRAFT, so that phrasing is its expectation; the requirement is the rule that the refusal identifies the addressed format/namespace/package — the same rule already accepted for CopyPackageVersions as Cause 7 of open issue 20261024T061931Z-codeartifact-cfn-provisioned-domains-absent-resource-policies-never.md).
Emulator: same code and status, message 'Package left-pad not found in repository <repo>.' — the bare name and repository only, with the format (and namespace) dropped. This is the same shared package-not-found message helper that issue's Cause 7 reports from the copy path, so fixing that helper should close this too; it is recorded here because that issue's trace list does not carry this check.
Why: a package is addressed by its (format, namespace, name) triple, so a refusal naming one third of the key cannot tell a caller whether it addressed the wrong format. Here it also masks a deeper check: the assertion is a precondition for grading that an INGESTED package defaults to {publish: BLOCK, upstream: ALLOW}, which the emulator was never asked.
Fix: the shared package-not-found message builder in the CodeArtifact state module, as used by the DescribePackage handler (src/ops/describe_package.rs) — render the full addressed triple.
Traces: codeartifact.ingested-package-defaults-to-publish-block-and-origin-type-partitions-the-versions.

## Evidence
Cause 1 — the endpoint answers 404 UnknownService for every data-plane verb (values are examples; the shape is constant):
  codeartifact.codeartifact-archived-version-tarball-download-rejected-while-published-sibling-serves:
    AssertionError: publishing '1.0.0' returned 404: b'{"__type":"UnknownService","message":"no emulated service is registered for \'sim2real-ca-1ec8fb6a301b-000000000000\'"}'
  codeartifact.codeartifact-scoped-npm-package-round-trips-under-its-url-encoded-namespace:
    AssertionError: publishing the scoped package to the encoded path returned 404: b'{"__type":"UnknownService",...}'
  codeartifact.codeartifact-unlisted-version-is-hidden-from-the-packument-but-still-downloadable: same 404 body on the publish.
  codeartifact.codeartifact-upstream-chain-resolves-through-two-hops-and-caches-in-the-front-repository:
    AssertionError: publish into the back repository returned 404: b'{"__type":"UnknownService",...}'
  codeartifact.codeartifact-repository-policy-denying-read-blocks-the-endpoint-and-a-sibling-still-serves:
    AssertionError: publishing into 'sim2real-denied-10104b9e0a8a' returned 404: b'{"__type":"UnknownService",...}'
  codeartifact.package-version-details-are-extracted-from-the-manifest-and-latest-decides-the-display-version: same 404 body on the publish.
  codeartifact.e2e-external-connection-upstream-fetch-then-disassociate-blocks:
    urllib.error.HTTPError: HTTP Error 404: Not Found (GET against the repository endpoint).
  codeartifact.authorization-token-default-duration-12h:
    AssertionError: expected 200 authenticating with the default-duration token, got 404 — GET <repositoryEndpoint>/-/ping?write=true with Authorization: Bearer <token>. The 43200s expiration assertion before it passed.
  Reached by path with the real npm client:
  codeartifact.list-package-version-dependencies-preserves-declared-order:
    npm notice Publishing to https://<emulator>/npm/sim2real-behav-repo-255ec5575323/ with tag latest
    npm error code E404 / npm error 404 Not Found - PUT https://<emulator>/npm/sim2real-behav-repo-255ec5575323/sim2real-behav-pkg-d573c28bd069
  codeartifact.npm-client-installs-a-semver-range-and-verifies-integrity-against-the-repository:
    AssertionError: `npm publish` for sim2real-npmcli-82ce100f4bec@1.0.0 exited 1; stderr: npm error 404 ... 'sim2real-npmcli-82ce100f4bec@1.0.0' is not in this registry.
  In every case GetRepositoryEndpoint itself returned 200 and a URL of the form https://<domain>-<accountId>.d.codeartifact.<region>.amazonaws.com/npm/<repository>/.

Cause 2 — codeartifact.zero-duration-token-expires-with-the-role-session-while-a-long-duration-outlives-it:
  AssertionError: durationSeconds=0 produced expiration 2026-10-25T03:36:40.448061+00:00, which is 2700.4s from the assumed session's own reported Expiration 2026-10-25T02:51:40+00:00 (tolerance 30.0s). The documented rule is that 0 means the same expiration as the caller's role's temporary credentials.
  (The offset is an example of the general defect: the response's expiration is computed without reference to the session; here it is issuance + 3600s.)

Cause 3 — codeartifact.authorization-token-needs-sts-bearer-permission-that-a-domain-policy-cannot-grant:
  Error: Check timed out after 60.0s.
  Beat 1 (both codeartifact:GetAuthorizationToken and sts:GetServiceBearerToken granted) minted a token — the positive control works. Beat 2 rewrites the role's inline policy to drop sts:GetServiceBearerToken and polls GetAuthorizationToken for a refusal; every probe kept returning HTTP 200 with a token, so no (code, message) tuple was ever observed and the poll ran until the harness killed the check. Expected: AccessDeniedException, HTTP 403.

Cause 4 — codeartifact.ingested-package-defaults-to-publish-block-and-origin-type-partitions-the-versions:
  got      ('ResourceNotFoundException', 'Package left-pad not found in repository sim2real-ca-repo-ddb33cc58430.')
  expected ('ResourceNotFoundException', 'Package not found in repository. Package = {format=npm, name=left-pad}')
  — code and HTTP status (404) agree; the message omits the request-derived format.

## Steps to Reproduce
Cause 1:
1. create_domain(domain=D); create_repository(domain=D, repository=R).
2. endpoint = get_repository_endpoint(domain=D, repository=R, format='npm')['repositoryEndpoint'] -> 200, a URL under <D>-<account>.d.codeartifact.<region>.amazonaws.com/npm/<R>/.
3. token = get_authorization_token(domain=D)['authorizationToken'].
4. GET <endpoint>/-/ping?write=true with header 'Authorization: Bearer <token>' -> observe HTTP 404 and body {"__type":"UnknownService","message":"no emulated service is registered for '<D>-<account>'"} instead of 200.
5. PUT <endpoint><package> with an npm publish document (gzipped tar under package/), same Authorization header -> same 404 instead of 200; GET <endpoint><package> (packument) and the tarball path -> same 404.
6. Equivalent with the real client: write an npmrc with registry=<endpoint>, //<host><path>:_authToken=<token>, //<host><path>:always-auth=true and run `npm publish` -> npm error code E404 on the PUT.

Cause 2:
1. sts.assume_role(..., DurationSeconds=900) and note Credentials.Expiration.
2. Build a codeartifact client from that session; create_domain(domain=D).
3. get_authorization_token(domain=D, durationSeconds=0) -> observe an `expiration` that is NOT the session's Expiration (here ~2700s later).
4. Control: get_authorization_token(domain=D, durationSeconds=43200) must still expire 12h out.

Cause 3:
1. iam.create_role trusted by the account root, inline policy granting codeartifact:GetAuthorizationToken, codeartifact:DescribeRepository AND sts:GetServiceBearerToken; sts.assume_role into it.
2. create_domain/create_repository with the privileged client; call get_authorization_token(domain=D) from the assumed session -> 200 (positive control).
3. Rewrite the inline policy with sts:GetServiceBearerToken REMOVED (everything else unchanged).
4. Call get_authorization_token(domain=D) from the same session -> observe HTTP 200 with a token instead of AccessDeniedException (403).
5. Follow-on leg that is currently unreachable: put_domain_permissions_policy(domain=D, policyDocument=<Allow codeartifact:DescribeDomain + codeartifact:GetAuthorizationToken + sts:GetServiceBearerToken to that role>) -> once DescribeDomain starts working for the role, get_authorization_token must STILL be refused 403.

Cause 4:
1. create_domain(domain=D); create_repository(domain=D, repository=R).
2. describe_package(domain=D, repository=R, format='npm', package='left-pad') -> observe ResourceNotFoundException whose message names only the package name and the repository, with the addressed format dropped.
