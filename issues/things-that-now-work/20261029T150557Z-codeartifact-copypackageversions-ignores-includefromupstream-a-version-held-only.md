Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.copy-from-upstream-without-include-flag-rejected
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|CopyPackageVersions|ResourceNotFoundException
Issue-Commit: da71a5d688a59bd4c749ede677d7a21d9a10da22

# CodeArtifact CopyPackageVersions ignores includeFromUpstream: a version held only by the source repository's upstream is refused 404 instead of copied (1 cause)

> _Note: this issue was identified against the code base at commit `da71a5d688a59bd4c749ede677d7a21d9a10da22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyPackageVersions

## Description
REOPENS 20261023T134651Z-codeartifact-removals-and-copies-are-not-reflected-in.md: its fix left the call refused. That issue's Cause 4 made a same-repository CopyPackageVersions materialize the versions in the destination, and that leg now works; the upstream-resolving leg of the SAME operation is still refused HTTP 404, so this is the operation's remaining half, not a new area.

Cause 1 of 1: CopyPackageVersions never resolves the source package through the source repository's upstream chain, so `includeFromUpstream=true` is a no-op and the call is refused with ResourceNotFoundException exactly as if the flag had not been sent.

AWS: Smithy (com.amazonaws.codeartifact#CopyPackageVersions, POST /v1/package/versions/copy, success 200) models `includeFromUpstream` (BooleanOptional) as "Set to true to copy packages from repositories that are upstream from the source repository to the destination repository. The default setting is false.", and models the per-version outcome maps `successfulVersions` (SuccessfulPackageVersionInfoMap) and `failedVersions` (PackageVersionErrorMap) plus `ResourceNotFoundException` (httpError 404). So the operation has two distinct behaviours for a version the source repository does not itself hold but one of its upstreams does:
  * with `includeFromUpstream` absent or false, AWS refuses the WHOLE call with `ResourceNotFoundException`, HTTP 404, message "The <format> package '<package>' with namespace '<namespace>' does not exist in repository '<sourceRepository>'." — the fixed text is "The ", " package '", "' with namespace '", "' does not exist in repository '", "'."; `<format>`, `<package>`, `<namespace>` and `<sourceRepository>` are the request's own values. The emulator already matches this leg, and the refusal correctly lands nothing in the destination.
  * with `includeFromUpstream=true`, the identical call resolves the version through the source repository's upstream repositories and copies it: HTTP 200 with `successfulVersions` carrying one entry per requested version, `{"<version>": {"revision": <the version's revision>, "status": "Published"}}`, and an empty `failedVersions`. The copy is complete, so GetPackageVersionAsset against the DESTINATION repository then returns the upstream asset's exact bytes (recorded behavior only for the asset-byte follow-on; the 200 + successfulVersions shape is modeled).

Emulator: both legs answer the same way. The upstream-including call is refused `ResourceNotFoundException` (404) with "The generic package '<package>' with namespace '<namespace>' does not exist in repository '<downstream repo>'." — the flag is not consulted at all. The handler states this as intentional in its own module doc: "`includeFromUpstream` changes nothing here, and deliberately: this emulator holds a package version only in the repository it was published into, and no operation resolves a version through a repository's upstreams". Nothing ever lands in the destination, so no follow-on read can succeed either.

Why: a client cannot distinguish "this version is nowhere in the domain" from "this version is one upstream hop away", which is the exact distinction the flag exists to express. The standard CodeArtifact promotion pattern — a downstream repo that upstreams to a shared/build repo, promoting a validated version into a release repo with `includeFromUpstream=true` — fails with a hard 404 on the emulator. Worse, the refusal is indistinguishable from the correct refusal of the no-flag call, so a client's retry-with-the-flag recovery path (the documented remedy for that 404) also 404s and the pipeline dead-ends with an error that looks like a legitimate not-found.

Fix: the CopyPackageVersions handler's source resolution and the state lookup behind it — when `includeFromUpstream` is true, resolve the package/version through the source repository's `upstreams` chain (depth-first through the domain's repositories, as the repository record already records them) before deciding `ResourceNotFoundException`, and copy the resolved version's record and assets into the destination with the same per-version `successfulVersions` reporting the in-repository copy already produces. The intra-domain upstream walk already exists on the data-plane side (`src/service/npm_registry.rs`, `retain_from_upstreams`, as named by 20261029T095746Z-codeartifact-npm-external-connection-ingests-nothing.md), so this is control-plane reuse of an existing traversal, not new pull-through: it needs no outbound network access and is unrelated to the external-connection platform blocker carried by that issue and by 20261027T083906Z-codeartifact-reopens-codeartifact-pypi-repository-endpoint-still-ingests-nothing.md.

Traces: codeartifact.copy-from-upstream-without-include-flag-rejected.

## Evidence
codeartifact.copy-from-upstream-without-include-flag-rejected (verdict: error)

Setup, all answered 200 by the emulator: CreateDomain(D) -> CreateRepository(D, UP) -> PublishPackageVersion(D, UP, format=generic, namespace=sim2real, package=P, packageVersion=1.0.0, asset payload.bin) -> CreateRepository(D, DOWN, upstreams=[{repositoryName: UP}]) -> CreateRepository(D, DEST).

Leg 1 (matches AWS): CopyPackageVersions(domain=D, sourceRepository=DOWN, destinationRepository=DEST, format=generic, namespace=sim2real, package=P, versions=["1.0.0"]) with no includeFromUpstream -> ResourceNotFoundException, "The generic package '<P>' with namespace 'sim2real' does not exist in repository '<DOWN>'." The check's equality on (code, message) passed, and ListPackages against DEST was empty, so the refusal correctly left no effect.

Leg 2 (the divergence): the SAME call with includeFromUpstream=True ->
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the CopyPackageVersions operation: The generic package 'sim2real-upstream-only' with namespace 'sim2real' does not exist in repository 'sim2real-behav-ca-dn-a0377c81f8df'.
Expected HTTP 200 with successfulVersions["1.0.0"]["status"] == "Published" (and an empty failedVersions), followed by GetPackageVersionAsset(domain=D, repository=DEST, format=generic, namespace=sim2real, package=P, packageVersion=1.0.0, asset=payload.bin) returning the upstream asset's exact bytes. Neither is reached: the copy is refused 404 and DEST stays empty.

(The domain/repository/package names and the 1.0.0 version in the quoted message are this run's values; the requirement is the fixed sentence template with the request's own format, package, namespace and sourceRepository substituted — and, on the includeFromUpstream=true leg, that no such refusal is issued at all.)

Source of the behaviour, for the fixer: services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/copy_package_versions.rs module doc lines 29-33 declare the omission deliberate ("`includeFromUpstream` changes nothing here"), and the 404 is raised by the `not_found` helper at approx. line 664 of the same file after the source package lookup fails.

## Steps to Reproduce
1. CreateDomain(domain=D).
2. CreateRepository(domain=D, repository=UP); PublishPackageVersion(domain=D, repository=UP, format=generic, namespace=sim2real, package=P, packageVersion=1.0.0, assetName=payload.bin, assetContent=<bytes>, assetSHA256=<sha256 of those bytes>) -> 200.
3. CreateRepository(domain=D, repository=DOWN, upstreams=[{repositoryName: UP}]) -> DOWN holds nothing itself; P/1.0.0 is reachable only through its upstream UP.
4. CreateRepository(domain=D, repository=DEST).
5. CopyPackageVersions(domain=D, sourceRepository=DOWN, destinationRepository=DEST, format=generic, namespace=sim2real, package=P, versions=["1.0.0"]) with includeFromUpstream omitted -> expect ResourceNotFoundException 404 "The generic package '<P>' with namespace 'sim2real' does not exist in repository '<DOWN>'." and ListPackages(domain=D, repository=DEST) empty. This leg already passes; keep it passing.
6. Repeat step 5 with includeFromUpstream=True -> expect HTTP 200 with successfulVersions["1.0.0"].status == "Published" and an empty failedVersions; observe ResourceNotFoundException 404 with the same message as step 5.
7. GetPackageVersionAsset(domain=D, repository=DEST, format=generic, namespace=sim2real, package=P, packageVersion=1.0.0, asset=payload.bin) -> expect the bytes published in step 2; observe that DEST never received the version.
