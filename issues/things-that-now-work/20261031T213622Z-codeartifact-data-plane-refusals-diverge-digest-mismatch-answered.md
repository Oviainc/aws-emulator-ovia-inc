Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.publish-asset-sha256-mismatch-rejected codeartifact.disposed-package-version-is-terminal codeartifact.codeartifact-package-origin-configuration-blocks-direct-publish
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.publish-asset-sha256-mismatch-rejected|run#18|AssertionError|assert refusal == expected, (
Issue-Key: codeartifact.disposed-package-version-is-terminal|run#32|AssertionError|assert refusal == expected, (
Issue-Key: codeartifact.codeartifact-package-origin-configuration-blocks-direct-publish|run#25|AssertionError|assert absent == expected_absent, (
Issue-Commit: f5bcb0a946788170dc0fa0f931c1e9072e8c121c

# CodeArtifact data-plane refusals diverge: digest mismatch answered 403 SignatureDoesNotMatch, and the package-version conflict/not-found sentences are the emulator's own

> _Note: this issue was identified against the code base at commit `f5bcb0a946788170dc0fa0f931c1e9072e8c121c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishPackageVersion

## Description
This issue bundles THREE distinct net-new root causes on CodeArtifact's package-version write and read paths, all grounded in failing checks of the simparity-behavioral-aws-v1 replay against this checkout. A fixer must close all three before submitting; each cause names its own failing check ids. All domain, repository, package, namespace, version and digest values quoted below are one run's values filling a template — only the fixed text and the rule for the variable parts are the requirement.

=== Cause 1 of 3: a mismatched assetSHA256 on PublishPackageVersion is answered 403 SignatureDoesNotMatch instead of 400 ValidationException ===
AWS: Smithy PublishPackageVersion (POST /v1/package/version/publish) binds assetSHA256 -> SHA256 @length(min 64, max 64) @pattern(^[0-9a-f]+$) to the HTTP header `x-amz-content-sha256`, and assetContent -> Asset is the required streaming @httpPayload; the operation models ValidationException with httpError 400 (Smithy: PublishPackageVersion errors -> AccessDeniedException 403, ConflictException 409, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400 — SignatureDoesNotMatch is not modeled at all). The declared digest disagreeing with the uploaded bytes is therefore an APPLICATION-layer refusal, not an authentication failure: the request authenticates, the service hashes the body, compares, and refuses with ValidationException / HTTP 400 and the message "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '<declared>', Actual hash '<computed>'". Fixed text: "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '", "', Actual hash '", "'"; the two hex digests are values — <declared> is the assetSHA256 the request sent on x-amz-content-sha256 and <computed> is the SHA-256 the service computed over the uploaded bytes — and there is no trailing period.
Emulator: HTTP 403 with error code SignatureDoesNotMatch and message 'The request signature does not match'. Hypothesis: because assetSHA256 rides on x-amz-content-sha256, the generic SigV4 payload-hash verification in the request pipeline treats that member-bound header as the authoritative payload hash, recomputes the body hash and rejects the request before PublishPackageVersion is dispatched, so no content-hash comparison is ever performed or reported.
Why: the whole digest-integrity contract of this operation is unreachable. SDKs and tools treat SignatureDoesNotMatch as a credential/clock problem and re-sign, refresh credentials or alarm on auth failures, so a corrupted upload is diagnosed as a signing failure; the 400/ValidationException branch a client needs in order to recompute its digest and retry the UPLOAD is never taken, and the refusal never names the computed digest, which is the only evidence that the service hashed the payload rather than trusting the header.
Fix: the SigV4 payload-hash verification in the request pipeline must not re-derive and enforce the body hash for an operation whose model binds a member to x-amz-content-sha256 with a streaming payload; the digest comparison and the modeled ValidationException belong in the PublishPackageVersion handler's validation (services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/publish_package_version.rs).
Traces: codeartifact.publish-asset-sha256-mismatch-rejected. The open issues 20261022T171312Z-codeartifact-no-iam-authorization-on-reads-publishpackageversion-digest.md (its Cause 2) and 20261029T203338Z-codeartifact-package-version-publish-status-and-read-paths.md (its Cause 1) describe this same symptom, but neither carries this behavioral check's replay key, so this check is still unowned and is filed here.

=== Cause 2 of 3: re-publishing over a Disposed version is refused with the emulator's own sentence instead of AWS's update-rule sentence ===
AWS: PublishPackageVersion models ConflictException httpError 409 and documents that only an Unfinished package version may be added to. A publish onto a coordinate that already exists with a non-Unfinished status is refused with ConflictException / HTTP 409 and the message "Cannot update existing generic package version '<namespace>/<package>/<version>' with status '<status>'. Only 'Unfinished' package versions can be updated." Fixed text: "Cannot update existing ", " package version '", "' with status '", "'. Only 'Unfinished' package versions can be updated."; the format word, the slash-joined namespace/package/version coordinate from the request, and <status> (the stored version's current PackageVersionStatus, here Disposed) are the variable parts.
Emulator: ConflictException / 409 — the right class and status — with its own sentence 'Package version <package> <version> is Disposed and cannot be published to.': no coordinate (namespace and format dropped), the status unquoted, and no pointer to the Unfinished rule.
Why: AWS's sentence is what tells a publisher the version must be created unfinished (or deleted) before it can be written to; a client that matches or surfaces the service sentence, and any test asserting the terminality of Disposed via the refusal text, diverges. It also makes the Disposed-terminal refusal indistinguishable from an unrelated conflict.
Fix: the already-exists / status-guard branch of the PublishPackageVersion handler (src/ops/publish_package_version.rs) and the shared package-version conflict message helper in src/state.rs — one template covering every non-Unfinished stored status, not a per-status sentence.
Traces: codeartifact.disposed-package-version-is-terminal. Cause 2 of the open issue 20261029T203338Z-codeartifact-package-version-publish-status-and-read-paths.md describes the same template for a 'Published' stored status from a wire step; it does not carry this behavioral check's replay key.

=== Cause 3 of 3: GetPackageVersionAsset's package-version-not-found sentence omits the repository id and the package coordinate entity ===
AWS: GetPackageVersionAsset (GET /v1/package/version/asset) models ResourceNotFoundException httpError 404 (Smithy: GetPackageVersionAsset errors -> ResourceNotFoundException 404). When the addressed package VERSION does not exist in the repository (e.g. a publish that package origin configuration blocked, so the version was never stored), the refusal is ResourceNotFoundException / 404 with the message "Package version not found in repo. repoId = '<repoId>', packageCoordinate = 'PackageVersionCoordinateEntity{name=<package>, type=<FORMAT upper-cased>, namespace=<namespace>, version=<version>, displayName=<namespace>/<package>/<version>}'." Fixed text: "Package version not found in repo. repoId = '", "', packageCoordinate = 'PackageVersionCoordinateEntity{name=", ", type=", ", namespace=", ", version=", ", displayName=", "}'."; the variable parts are the resolved internal repository id and the request's own coordinate (format upper-cased, displayName composed as namespace/package/version). Note this is the version-absent sentence and is distinct from the asset-absent sentence ("Asset not found in repo. RepositoryName = ...").
Emulator: ResourceNotFoundException / 404 — right class and status — with 'Package version <package> <version> not found in repository <repository>.': the repoId clause and the whole PackageVersionCoordinateEntity are missing, and the repository is named by name rather than id.
Why: the coordinate entity is the part that tells a caller WHICH coordinate was resolved (format and namespace included), so a client cannot tell a wrong-namespace or wrong-format lookup from a genuinely absent version; tests that assert the service sentence, and tooling that parses the coordinate out of it, both fail. It also makes a blocked-publish read indistinguishable from an unrelated miss.
Fix: the version-lookup failure message in the GetPackageVersionAsset handler (src/ops/get_package_version_asset.rs) and the shared package-version-not-found message helper in src/state.rs, which must render the repoId + PackageVersionCoordinateEntity template.
Traces: codeartifact.codeartifact-package-origin-configuration-blocks-direct-publish. Cause 7 of the open issue 20261029T203338Z-codeartifact-control-plane-refusal-messages-are-the-emulator.md covers the ASSET-not-found sentence of the same operation, a different message and a different branch; it does not cover the version-not-found sentence or this check.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator booted from this checkout: 72/95 passed (75.8%), 15 mismatched, 8 errored; all failures on service codeartifact. The three clusters below carry no stub_path and no owned_by marker — no open issue carries their replay keys. Verdicts verbatim from the replay output:

Cause 1 — [mismatch] codeartifact.publish-asset-sha256-mismatch-rejected
  File "check_codeartifact_publish_asset_sha256_mismatch_rejected.py", line 78, in run
  AssertionError: a publish with a mismatched assetSHA256 returned ('SignatureDoesNotMatch', 'The request signature does not match'), expected ('ValidationException', "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash 'e082bb3059bd78f769564587b8fee9fe31739c1be0b8f396c73beeddefd908bc', Actual hash '06ce2f7126c9297b17c3f7d527cbe2a37452179f0d86625beb9d6f27903fabab'"): the refusal names the declared digest and the digest it COMPUTED from the bytes, which is what proves the service hashed the payload rather than trusting the header.
  Request: publish_package_version(format='generic', namespace='sim2real', package='sim2real-integrity', assetContent=<payload bytes>, assetSHA256=sha256(<different bytes>)). Status diverges too: the modeled refusal is HTTP 400, the emulator answers 403.

Cause 2 — [mismatch] codeartifact.disposed-package-version-is-terminal
  File "check_codeartifact_disposed_package_version_is_terminal.py", line 132, in run
  AssertionError: re-publishing over a Disposed version returned ('ConflictException', 'Package version sim2real-disposal 1.0.0 is Disposed and cannot be published to.'), expected ('ConflictException', "Cannot update existing generic package version 'sim2real/sim2real-disposal/1.0.0' with status 'Disposed'. Only 'Unfinished' package versions can be updated.")
  Error code and HTTP status (409) match; only the message text diverges.

Cause 3 — [mismatch] codeartifact.codeartifact-package-origin-configuration-blocks-direct-publish
  File "check_codeartifact_codeartifact_package_origin_configuration_blocks_direct_publish.py", line 113, in run
  AssertionError: the blocked version's asset is readable or refused otherwise: GetPackageVersionAsset answered ('ResourceNotFoundException', 'Package version sim2real-blocked-1f41740a9b84 2.0.0 not found in repository sim2real-repo-59e8fdcf5323.'), expected ('ResourceNotFoundException', "Package version not found in repo. repoId = '<id>', packageCoordinate = 'PackageVersionCoordinateEntity{name=sim2real-blocked-1f41740a9b84, type=GENERIC, namespace=sim2real-ns, version=2.0.0, displayName=sim2real-ns/sim2real-blocked-1f41740a9b84/2.0.0}'.")
  Error code and HTTP status (404) match; only the message text diverges.

All names, repository ids, versions and digests above are one run's values filling the templates described in the causes.

## Steps to Reproduce
Boot the emulator from this checkout and replay the three checks listed in trace_ids. By hand, with a codeartifact client pointed at the emulator:

Cause 1:
1. create_domain(domain=D); create_repository(domain=D, repository=R).
2. payload = b'the-bytes-that-are-actually-sent\n'; other = b'the-bytes-whose-digest-was-declared\n'.
3. publish_package_version(domain=D, repository=R, format='generic', namespace='sim2real', package=P, packageVersion='2.0.0', assetName='payload.bin', assetContent=payload, assetSHA256=sha256(other).hexdigest()).
   -> expect ValidationException / HTTP 400 with "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '<declared>', Actual hash '<computed over payload>'"; observe HTTP 403 SignatureDoesNotMatch / 'The request signature does not match'.
4. Confirm the positive control: the same bytes published with their true digest lands and reads back, and version 2.0.0 does not exist after the rejected publish.

Cause 2:
1. create_domain(domain=D); create_repository(domain=D, repository=R).
2. publish_package_version(domain=D, repository=R, format='generic', namespace='sim2real', package=P, packageVersion='1.0.0', assetContent=C, assetName=A, assetSHA256=sha256(C).hexdigest()).
3. dispose_package_versions(domain=D, repository=R, format='generic', namespace='sim2real', package=P, versions=['1.0.0']) and wait for describe_package_version status == 'Disposed'.
4. Re-publish the identical coordinate -> expect ConflictException / 409 "Cannot update existing generic package version 'sim2real/P/1.0.0' with status 'Disposed'. Only 'Unfinished' package versions can be updated."; observe 'Package version P 1.0.0 is Disposed and cannot be published to.'

Cause 3:
1. create_domain(domain=D); create_repository(domain=D, repository=R); publish version 1.0.0 of package P (namespace N, format generic).
2. put_package_origin_configuration(domain=D, repository=R, format='generic', namespace=N, package=P, restrictions={'publish': 'BLOCK', 'upstream': 'ALLOW'}) and confirm the restriction is in effect.
3. Attempt publish_package_version of version 2.0.0 of P -> refused, nothing stored.
4. get_package_version_asset(domain=D, repository=R, format='generic', namespace=N, package=P, packageVersion='2.0.0', asset=<asset name>) -> expect ResourceNotFoundException / 404 "Package version not found in repo. repoId = '<repoId>', packageCoordinate = 'PackageVersionCoordinateEntity{name=P, type=GENERIC, namespace=N, version=2.0.0, displayName=N/P/2.0.0}'."; observe 'Package version P 2.0.0 not found in repository R.'
