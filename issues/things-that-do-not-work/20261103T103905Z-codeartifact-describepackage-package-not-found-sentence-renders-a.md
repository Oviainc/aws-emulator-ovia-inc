Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.package-delete-cascades-to-versions
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.package-delete-cascades-to-versions|run#19|AssertionError|assert refusal == expected, (
Issue-Commit: 286c0019b2a6d6ba24e260ebac56f0f3f96c1e4a

# CodeArtifact DescribePackage package-not-found sentence renders a namespace= component AWS omits (behavioral replay of package-delete-cascades-to-versions)

> _Note: this issue was identified against the code base at commit `286c0019b2a6d6ba24e260ebac56f0f3f96c1e4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePackage

## Description
ONE root cause, found by replaying the simparity-behavioral-aws-v1 behavioral suite against the emulator booted from this checkout. A fixer must close this single cause before submitting. (All domain, repository, namespace and package names quoted below are one run's values filling a template — only the fixed text and the rule for the variable parts are the requirement.)

NOTE on prior art: the open issue 20261103T055241Z-codeartifact-describepackage-not-found-message-includes-a-namespace.md describes the same emulator behavior, but it was raised from the wire corpus (simparity-wire-aws-v1, key codeartifact|DescribePackage|4xx->4xx|$.message) and does NOT carry this behavioral check's replay key, so the behavioral replay of it is not covered by it. If that issue is fixed, this check should pass with it; it is filed here so the behavioral check id is bound to the cause and gets re-replayed. No other open CodeArtifact issue covers this sentence: 20261029T203338Z-codeartifact-control-plane-refusal-messages-are-the-emulator.md and 20261031T213622Z-codeartifact-data-plane-refusals-diverge-digest-mismatch-answered.md cover other operations' (GetPackageVersionAsset) asset- and package-VERSION-not-found templates.

=== Cause 1 of 1: DescribePackage's package-not-found message inserts the request's namespace into the coordinate brace list ===
AWS: Smithy DescribePackage (GET /v1/package, success 200) binds domain (required), domainOwner, repository (required), format (PackageFormat, required), namespace (PackageNamespace, optional) and package (PackageName, required); it models ResourceNotFoundException with httpError 404 (plus AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400). The model does not document the refusal text, so the message is recorded behavior only: for a package the addressed repository does not hold — including a package just removed by DeletePackage — the service answers ResourceNotFoundException / HTTP 404 with message "Package not found in repository. Package = {format=<format>, name=<package>}". Fixed text (the requirement): "Package not found in repository. Package = {format=", ", name=", "}". Variable parts, taken from the request: the request's format value (e.g. generic, npm) and the request's package name. No namespace= component appears in the brace list even when the request addresses a namespaced (generic/maven/swift) coordinate — this sentence renders format+name only, unlike the richer package-VERSION/asset coordinate renderings of this service. Corroborated inside this same suite run by another check, where the emulator's own npm sentence reads "Package = {format=npm, name=lodash}" (no namespace supplied, so no divergence).
Emulator: correct error class (ResourceNotFoundException), correct status (404) and correct fixed text, but it appends the request's namespace, emitting "Package not found in repository. Package = {format=generic, namespace=<namespace>, name=<package>}" — in the failing run: expected ('ResourceNotFoundException', 'Package not found in repository. Package = {format=generic, name=sim2real-behav-pkg-5e6ed228c3a9}'), got ('ResourceNotFoundException', 'Package not found in repository. Package = {format=generic, namespace=sim2real-behav-ns-3a13ddc860c4, name=sim2real-behav-pkg-5e6ed228c3a9}'). The divergence is namespace-conditional: a namespace-less coordinate matches AWS, a namespaced one does not.
Why: the refusal message is part of the wire contract. A client, test or operator tool that matches, parses or surfaces CodeArtifact's package-not-found sentence diverges on every namespaced coordinate, and anything parsing the brace list sees a key AWS never emits there. Because it only appears when a namespace is supplied, it passes a smoke test on pypi/npm coordinates and breaks on generic/maven/swift ones. It also blocks the rest of this check from running at all, so the DeletePackage cascade assertions after it (ListPackageVersions' own not-found sentence "The generic package '<package>' with namespace '<namespace>' does not exist in repository '<repository>'." and DeletePackage's idempotent second call returning a bare 200 with no deletedPackage member) are still unverified behind it.
Fix: the package-not-found message construction reached from the DescribePackage handler — the shared helper crate::state::package_not_found_message in this service crate's src/state.rs, called from src/ops/describe_package.rs. The sentence must render format and name only, leaving the namespace out of the brace list regardless of whether the request supplied one; the package-VERSION and asset not-found sentences use a different coordinate rendering and must not be changed by this fix.
Traces: codeartifact.package-delete-cascades-to-versions (the only member of its cluster in this suite run).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator booted from this checkout: 82/95 passed (86.3%), 9 mismatched, 4 errored, 0 skipped, 0 unloadable; all failures under service heading `codeartifact`. Twelve of the thirteen failures are marked OWNED by existing issues; this is the one surviving failure.

Verdict verbatim from the replay output:

[mismatch] codeartifact.package-delete-cascades-to-versions
  File "check_codeartifact_package_delete_cascades_to_versions.py", line 78, in run
    assert refusal == expected, (
  AssertionError: expected DescribePackage on a deleted package to raise ('ResourceNotFoundException', 'Package not found in repository. Package = {format=generic, name=sim2real-behav-pkg-5e6ed228c3a9}'), got ('ResourceNotFoundException', 'Package not found in repository. Package = {format=generic, namespace=sim2real-behav-ns-3a13ddc860c4, name=sim2real-behav-pkg-5e6ed228c3a9}')

Expected vs actual:
- HTTP status: 404 vs 404 (matches).
- Error code: ResourceNotFoundException vs ResourceNotFoundException (matches).
- Message: fixed text "Package not found in repository. Package = {format=" ... ", name=" ... "}" with format and package name taken from the request; the emulator additionally renders ", namespace=<request namespace>" between them, which is not part of the template. The names sim2real-behav-pkg-5e6ed228c3a9 / sim2real-behav-ns-3a13ddc860c4 are one run's generated values, not a requirement.

Source of the extra component: services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/describe_package.rs:226 calls crate::state::package_not_found_message(&input.format, input.namespace.as_deref(), &input.package) — the helper is handed the namespace and renders it.

## Steps to Reproduce
Boot the emulator from this checkout and replay the behavioral suite; the failing check is codeartifact.package-delete-cascades-to-versions.

By hand, with a codeartifact client pointed at the emulator:
1. create_domain(domain=D)
2. create_repository(domain=D, repository=R)
3. publish_package_version(domain=D, repository=R, format='generic', namespace=N, package=P, packageVersion='1.0.0', assetName=A, assetContent=C, assetSHA256=sha256(C).hexdigest()) for versions 1.0.0 and 2.0.0
4. delete_package(domain=D, repository=R, format='generic', namespace=N, package=P)
5. describe_package(domain=D, repository=R, format='generic', namespace=N, package=P)
   -> expect ResourceNotFoundException / HTTP 404, message "Package not found in repository. Package = {format=generic, name=P}" (no namespace= component)
   -> observe "Package not found in repository. Package = {format=generic, namespace=N, name=P}"
6. Control: repeat step 5 omitting namespace (publish a namespace-less pypi/npm coordinate first) — the emulator's sentence then matches AWS, confirming the extra component is emitted only when the request supplies a namespace.
7. Behind this assertion, the check then expects ListPackageVersions on the deleted package to raise ResourceNotFoundException with "The generic package 'P' with namespace 'N' does not exist in repository 'R'." and a second delete_package to return a bare 200 with no deletedPackage member; verify these once step 5 matches.
