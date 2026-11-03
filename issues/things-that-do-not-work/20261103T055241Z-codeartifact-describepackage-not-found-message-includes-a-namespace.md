Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-package-origin-configuration-and-version-delete
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-package-origin-configuration-and-version-delete
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|DescribePackage|4xx->4xx|$.message
Issue-Commit: 0f012c5ca8dd92dc033d983a8fae9e32b37a8ccd

# CodeArtifact DescribePackage not-found message includes a namespace component AWS omits

> _Note: this issue was identified against the code base at commit `0f012c5ca8dd92dc033d983a8fae9e32b37a8ccd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePackage

## Description
ONE root cause, found by replaying the simparity-wire-aws-v1 wire corpus against the emulator booted from this checkout. A fixer must close this single cause before submitting. (All domain, repository, namespace and package names quoted below are one run's values filling a template — only the fixed text and the rule for the variable parts are the requirement.)

=== Cause 1 of 1: DescribePackage's package-not-found sentence renders a namespace= component that AWS's sentence does not carry ===
AWS: Smithy DescribePackage (GET /v1/package, success 200) binds domain, domainOwner, repository, format (PackageFormat, required), namespace (PackageNamespace, optional) and package (PackageName, required) as httpQuery members, and models ResourceNotFoundException with httpError 404 (also AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400). The model does not document the refusal text, so the message is recorded behavior only: for a package that the addressed repository does not hold, the service answers ResourceNotFoundException / HTTP 404 with "Package not found in repository. Package = {format=<format>, name=<package>}". The fixed text is "Package not found in repository. Package = {format=", ", name=", "}"; the two variable parts are the request's own format (lower-cased, as the request sent it) and the request's package name. The recorded response carries NO namespace= component in the brace list even though the request addressed a namespaced generic coordinate — i.e. the coordinate AWS renders here is format+name only, unlike the richer PackageVersionCoordinateEntity rendering used by the package-VERSION and asset not-found sentences of this service.
Emulator: ResourceNotFoundException / HTTP 404 — correct error class, correct status, correct fixed text — but it inserts the request's namespace into the brace list, emitting "Package not found in repository. Package = {format=generic, namespace=sim2real, name=sim2real-probe-pkg-fbfc55f2}" where the service emits "Package not found in repository. Package = {format=generic, name=sim2real-probe-pkg-fbfc55f2}". Hypothesis: the shared package-not-found message helper renders every member of the resolved coordinate it holds (format, namespace, name) rather than the format+name pair this particular sentence uses.
Why: the refusal message is part of the wire contract. A client or test that matches, parses or surfaces the service's package-not-found sentence diverges on every namespaced coordinate, and tooling that parses the brace list sees a key AWS never emits there. Because the extra component appears only when a namespace was supplied, the divergence is also namespace-conditional: the same emulator sentence matches AWS for a namespace-less coordinate and diverges for a namespaced one, which is exactly the shape of bug that passes a smoke test and breaks a real client.
Fix: the package-not-found message construction in the DescribePackage handler (services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/describe_package.rs) and the shared package-not-found message helper it calls in that crate's src/state.rs — this sentence must render format and name only, leaving the namespace out of the brace list regardless of whether the request supplied one. The package-VERSION and asset not-found sentences use a different coordinate rendering and must not be changed by this fix.
Traces: codeartifact-package-origin-configuration-and-version-delete#task#11 (the only member of its cluster). Related open issues describe other CodeArtifact refusal sentences but not this one and do not carry this replay key: 20261029T203338Z-codeartifact-control-plane-refusal-messages-are-the-emulator.md (its Cause 7 covers GetPackageVersionAsset's ASSET-not-found sentence) and 20261031T213622Z-codeartifact-data-plane-refusals-diverge-digest-mismatch-answered.md (its Cause 3 covers GetPackageVersionAsset's package-VERSION-not-found sentence). Both are different operations, different branches and different templates.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator booted from this checkout: 34 fixtures, 344 steps, 290 passed, 54 mismatched, 0 errored, 0 blocked, score 84.3%, 27 divergence clusters — all on service "codeartifact". This cluster is the only one carrying neither a stub_path nor an owned_by, i.e. no open issue carries its replay key. Verdict verbatim from the replay output:

[mismatch] codeartifact-package-origin-configuration-and-version-delete#task#11 DescribePackage, status 404 -> 404, affected_count 1:
  $.message: value_mismatch (expected 'Package not found in repository. Package = {format=generic, name=sim2real-probe-pkg-fbfc55f2}', got 'Package not found in repository. Package = {format=generic, namespace=sim2real, name=sim2real-probe-pkg-fbfc55f2}')

Fixed text (the requirement): "Package not found in repository. Package = {format=" ... ", name=" ... "}". Values from the request: format=generic, name=sim2real-probe-pkg-fbfc55f2 in this run. The emulator's additional ", namespace=sim2real" component is not part of the template. HTTP status (404) and error code (ResourceNotFoundException, x-amzn-errortype) match AWS; only the message text diverges.

## Steps to Reproduce
Boot the emulator from this checkout and replay simparity-wire-aws-v1; the failing step is codeartifact-package-origin-configuration-and-version-delete#task#11.

By hand, with a codeartifact client pointed at the emulator:
1. create_domain(domain=D).
2. create_repository(domain=D, repository=R).
3. publish_package_version(domain=D, repository=R, format='generic', namespace=N, package=P, packageVersion='1.0.0', assetName=A, assetContent=C, assetSHA256=sha256(C).hexdigest()), then delete the package (delete_package / delete_package_versions so the package is no longer held by R) — or simply address a package name R never held.
4. describe_package(domain=D, repository=R, format='generic', namespace=N, package=P)
   -> expect ResourceNotFoundException / HTTP 404 with message "Package not found in repository. Package = {format=generic, name=P}" (no namespace= component).
   -> observe "Package not found in repository. Package = {format=generic, namespace=N, name=P}".
5. Control: repeat step 4 with no namespace argument (a namespace-less coordinate) — the emulator's sentence then matches AWS, confirming the extra component is emitted only when the request supplies a namespace.
