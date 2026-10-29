Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.ingested-package-defaults-to-publish-block-and-origin-type-partitions-the-versions codeartifact.e2e-external-connection-upstream-fetch-then-disassociate-blocks
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.ingested-package-defaults-to-publish-block-and-origin-type-partitions-the-versions|run#18|AssertionError|assert pm_status == 200, (
Issue-Key: codeartifact.e2e-external-connection-upstream-fetch-then-disassociate-blocks|_http_get_bytes#11|HTTPError|with urllib.request.urlopen(req, timeout=timeout_s) as resp: # noqa: S310
Issue-Commit: d1b341c944550181b7ac630538402367c666ba11

# CodeArtifact: the npm repository endpoint resolves nothing through an associated external connection, so no EXTERNAL-origin version is ever ingested (1 cause)

> _Note: this is Cause 3 of 20261029T095746Z-codeartifact-three-fidelity-divergences-getauthorizationtoken-bearer-denial-omits.md, carried forward on its own. That issue's Cause 1 (the IAM implicit-deny sentence for GetAuthorizationToken) and Cause 2 (the not-found sentence the version-addressing operations state) are fixed and their checks pass; this cause is not, and it owns the two checks listed above so a later scan does not have to re-derive it._

## Operation
AssociateExternalConnection

## Description
Cause 1 of 1: a repository with an npm external connection associated (`public:npmjs`) never resolves or ingests a package the connection's public registry holds, so every behaviour that depends on an ingested EXTERNAL-origin version is unobservable.

AWS: `AssociateExternalConnection` (POST /v1/repository/external-connection) attaches a public upstream such as `public:npmjs` to a repository. The npm registry wire protocol on the endpoint `GetRepositoryEndpoint(format='npm')` hands out is recorded behaviour only (it is not in the Smithy model): with the `GetAuthorizationToken` token as the bearer credential, `GET <endpoint>/<package>` for a package held only by the public registry answers HTTP 200 with the packument, its `dist.tarball` rewritten to the repository's own endpoint, and fetching that tarball ingests the version into the repository. The control plane then shows it: `ListPackageVersions` reports the version with `originType` EXTERNAL and partitions the package's versions by that member, and `DescribePackage` reports `originConfiguration.restrictions` {publish: BLOCK, upstream: ALLOW}. After `DisassociateExternalConnection`, an already-cached version stays downloadable through `GetPackageVersionAsset` while a version that was never fetched while the connection was live can no longer be retrieved.

Emulator: the endpoint is served and answers in CodeArtifact's own envelope, and UPSTREAM REPOSITORIES inside the domain do resolve and retain (`src/service/npm_registry.rs`, `retain_from_upstreams`) — but an external connection resolves nothing: `GET <endpoint>/<package>` for a package no repository of the domain holds answers HTTP 404 with the package-not-found body. Nothing is ingested, so no EXTERNAL-origin version appears, the `originType` partition is never exercised, and the post-disassociation cached-versus-never-fetched distinction cannot be observed. The parts of this cause that need no public registry ARE implemented: an ingestion-created package record defaults to {publish: BLOCK, upstream: ALLOW} (`crate::state::Package::ingested`), the package's `upstream` restriction is consulted before anything is ingested, and a publish into a publish-BLOCK package is refused HTTP 400 with `{"success":false,"error":"This package is configured to block new versions from being published."}`.

Why: external connections are the primary reason to run CodeArtifact. With no pull-through, every upstream behaviour is a silent no-op that looks correct — a blocked package has no versions and an allowed package also has no versions — so a client or CI pipeline validating dependency-substitution protection, origin controls or upstream caching passes against an emulator that enforces nothing, and `npm install` of any public dependency simply 404s.

## Known blocker (read this before picking the issue up)
Serving a package held only by `public:npmjs` means the emulator reaching that registry over the network. This emulator serves inbound requests and dials nothing off-process: an outbound connection to a third-party registry is refused by the project's own rules and by the merge gate's security review, so an HTTP client to `registry.npmjs.org` cannot land. The other offline route — answering with hand-written packuments and tarballs for the particular package names and version numbers a recording happens to mention — is fabricated content fitted to one run, not an emulation of the behaviour, and is not an acceptable fix either.

So this issue is open and UNRESOLVED rather than fixable as it stands: it needs either a platform capability that does not exist yet (an outbound fetch, however confined, or a local stand-in registry the platform itself provides and the emulator may resolve against), or the honest exit — `mark_wont_fix(platform_blocker)` on `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/service/npm_registry.rs`, naming the no-outbound-connection rule. The same gap for the pypi format is carried by 20261027T083906Z-codeartifact-reopens-codeartifact-pypi-repository-endpoint-still-ingests-nothing.md.

## Evidence
codeartifact.ingested-package-defaults-to-publish-block-and-origin-type-partitions-the-versions (mismatch)
  The control plane answers 200 throughout: create_domain -> create_repository -> associate_external_connection(externalConnection='public:npmjs') -> get_authorization_token -> get_repository_endpoint(format='npm').
  AssertionError "the packument fetch for 'left-pad' through public:npmjs returned 404: b'{\"error\":\"Package not found in repository. Package = {format=npm, name=left-pad}\"}'". Expected HTTP 200 with a packument whose `versions[<version>].dist.tarball` is rewritten to this repository's endpoint; fetching it must ingest that version, after which DescribePackage reports restrictions {publish: BLOCK, upstream: ALLOW}, a PUT of a substituted version is refused HTTP 400 with the publish-blocked body, and ListPackageVersions partitions the versions by originType. None of those legs are reached. (The package name and the version number in any single run are that run's values, not part of the requirement.)

codeartifact.e2e-external-connection-upstream-fetch-then-disassociate-blocks (error)
  urllib.error.HTTPError: HTTP Error 404: Not Found on the packument GET through the live `public:npmjs` connection. Expected HTTP 200 advertising the upstream's versions, a tarball fetch that caches one of them, and — after disassociate_external_connection — the cached version still downloadable via GetPackageVersionAsset while a version never fetched while the connection was live is refused.

## Steps to Reproduce
1. create_domain(domain=D); create_repository(domain=D, repository=R); associate_external_connection(domain=D, repository=R, externalConnection='public:npmjs') -> all 200.
2. token = get_authorization_token(domain=D)['authorizationToken']; ep = get_repository_endpoint(domain=D, repository=R, format='npm')['repositoryEndpoint'].
3. HTTP GET ep + '<a package only the public registry holds>' with the token as bearer credential -> expect HTTP 200 packument with dist.tarball rewritten to ep; observe HTTP 404 with the package-not-found body.
4. GET the advertised tarball, then list_package_versions(domain=D, repository=R, format='npm', package=<that package>) -> expect the ingested version with originType EXTERNAL; observe that nothing is ever ingested.
5. After caching one version through the connection, disassociate_external_connection(...) -> the cached version must stay downloadable via get_package_version_asset while a version never fetched while the connection was live must be refused.
