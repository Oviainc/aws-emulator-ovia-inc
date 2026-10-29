Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-generic-publish-validation-envelopes
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-generic-publish-validation-envelopes codeartifact-package-origin-configuration-and-version-delete codeartifact-package-version-status-transitions codeartifact-copy-package-versions-between-repositories codeartifact-write-ops-on-missing-resources codeartifact-generic-package-publish-and-reads
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|PublishPackageVersion|4xx->4xx|$.message
Issue-Key: codeartifact|PublishPackageVersion|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: codeartifact|PublishPackageVersion|4xx->4xx|status
Issue-Key: codeartifact|UpdatePackageVersionsStatus|4xx->4xx|$.message
Issue-Key: codeartifact|CopyPackageVersions|2xx->2xx|$.failedVersions.9.9.9.errorMessage
Issue-Key: codeartifact|UpdatePackageVersionsStatus|2xx->2xx|$.failedVersions.1.0.0.errorMessage
Issue-Key: codeartifact|DisposePackageVersions|2xx->2xx|$.failedVersions.1.0.0.errorMessage
Issue-Key: codeartifact|CopyPackageVersions|2xx->2xx|$.failedVersions.1.0.0.errorMessage
Issue-Key: codeartifact|ListPackageVersions|2xx->2xx|$.versions[*].publishedTime
Issue-Key: codeartifact|GetPackageVersionAsset|2xx->2xx|$headers.content-type
Issue-Key: codeartifact|DescribePackage|4xx->2xx|$.message
Issue-Key: codeartifact|DescribePackage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: codeartifact|DescribePackage|4xx->2xx|status
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodeArtifact package-version publish, status and read paths diverge on status, payload and message

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishPackageVersion

## Description
This issue bundles SEVEN distinct net-new root causes on CodeArtifact's package-version write and read paths, all grounded in failing steps of the simparity-wire-aws-v1 wire replay. Two change the HTTP status a client sees, one changes the response's Content-Type, one drops a modeled member, one changes the batch per-version error payload, and two re-word a refusal. A fixer must close all seven before submitting; each cause names its own failing fixture step. (Names, versions, digests and accounts quoted are one run's values filling a template, never part of the requirement.)

=== Cause 1 of 7: a mismatched assetSHA256 is answered 403 SignatureDoesNotMatch instead of 400 ValidationException ===
AWS: Smithy PublishPackageVersion (POST /v1/package/version/publish) binds assetSHA256 -> SHA256 @length(64) @pattern(^[0-9a-f]+$) to the header x-amz-content-sha256 and assetContent -> Asset as the streaming @httpPayload; the operation models ValidationException httpError 400. The service HASHES the uploaded bytes and compares them with the declared digest; a mismatch is ValidationException, HTTP 400, x-amzn-errortype "ValidationException:http://internal.amazon.com/coral/com.amazonaws.codeartifact.controlplane/", message "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '<the assetSHA256 the request declared>', Actual hash '<the SHA-256 the service computed over the uploaded bytes>'" — the two hex digests are values (from the request header and from the body), the rest is fixed text, and there is no trailing period.
Emulator: HTTP 403 with errortype SignatureDoesNotMatch and message 'The request signature does not match' — an error this operation does not model at all. Hypothesis: because assetSHA256 rides on x-amz-content-sha256, the generic SigV4 request-authentication middleware compares that header against the body and rejects the request before the handler runs, so no content-hash comparison is ever performed or reported.
Why: SDKs and tools treat SignatureDoesNotMatch as a credential/clock problem and re-sign, refresh credentials or alarm on authorization failures, so a corrupted upload is diagnosed as a signing failure; and the 400/ValidationException branch a client needs in order to retry the UPLOAD is never taken.
Fix: the payload-digest comparison must happen inside src/ops/publish_package_version.rs and raise the modeled ValidationException with the sentence above; the signature-verification middleware must not treat this operation's x-amz-content-sha256 payload digest as a SigV4 signature mismatch. The open issue 20261022T171312Z-codeartifact-no-iam-authorization-on-reads-publishpackageversion-digest.md names this symptom and the open behavioral issue 20261029T180849Z-...-10-fidelity-divergences-in-getauthorizationtoken.md repeats it as its Cause 5, but neither carries these wire steps.
Traces: codeartifact-generic-publish-validation-envelopes#task#5 (cluster also covers codeartifact-package-origin-configuration-and-version-delete#task#6).

=== Cause 2 of 7: re-publishing an existing Published generic version is refused with the emulator's own sentence ===
AWS: PublishPackageVersion models ConflictException httpError 409 and documents that only an Unfinished version may be added to. A publish onto a version that already exists and is not Unfinished is refused with ConflictException/409 and message "Cannot update existing generic package version '<namespace>/<package>/<version>' with status '<status>'. Only 'Unfinished' package versions can be updated." — fixed text "Cannot update existing generic package version '", "' with status '", "'. Only 'Unfinished' package versions can be updated."; the coordinate is the slash-joined namespace/package/version from the request and <status> is the stored version's current PackageVersionStatus.
Emulator: ConflictException/409 (correct class) with 'Package version <package> <version> is Published and cannot be published to.' — its own sentence: no coordinate, status unquoted, and no pointer to the Unfinished rule.
Why: the AWS sentence is what tells a publisher the version must be created as unfinished (or disposed/deleted first); a client matching or surfacing it diverges.
Fix: the already-exists branch of src/ops/publish_package_version.rs (and the shared package-version conflict helper in src/state.rs).
Traces: codeartifact-generic-publish-validation-envelopes#task#6.

=== Cause 3 of 7: UpdatePackageVersionsStatus refuses targetStatus=Disposed with a modeled-enum constraint sentence instead of the service's rule ===
AWS: UpdatePackageVersionsStatus.targetStatus targets the whole PackageVersionStatus enum (Published | Unfinished | Unlisted | Archived | Disposed | Deleted), so Disposed is a WIRE-VALID value; the service refuses it by its own rule (that transition belongs to DisposePackageVersions) with ValidationException, HTTP 400, message "Target status is not allowed." — entirely fixed text, naming neither the member nor a value set.
Emulator: ValidationException/400 with "1 validation error detected: Value 'Disposed' at 'targetStatus' failed to satisfy constraint: Member must satisfy enum value set: [Archived, Published, Unlisted]" — the shared constraint-violation sentence, which advertises a narrower enum than the model declares.
Why: a client validating against the model is told a modeled value is structurally invalid instead of being routed to the operation that owns the transition; any tooling that distinguishes modeled-constraint failures from service-rule failures mis-classifies this one.
Fix: the targetStatus guard in src/ops/update_package_versions_status.rs must raise ValidationException with the fixed sentence rather than calling the constraint-violation message helper in src/state.rs. Same wording as Cause 6 of the open behavioral issue 20261029T180849Z-...-10-fidelity-divergences-in-getauthorizationtoken.md, which carries no wire step for it.
Traces: codeartifact-package-version-status-transitions#task#9.

=== Cause 4 of 7: the per-version PackageVersionError in failedVersions does not match AWS's errorMessage ===
AWS: CopyPackageVersions, UpdatePackageVersionsStatus and DisposePackageVersions all answer 200 with successfulVersions + failedVersions, a map of version -> PackageVersionError { errorCode (ALREADY_EXISTS | MISMATCHED_REVISION | MISMATCHED_STATUS | NOT_ALLOWED | NOT_FOUND | SKIPPED), errorMessage }. Recorded behavior only: for the failing version in each of these four steps, the recording's errorMessage member does NOT agree with the emulator's — the replay reports $.failedVersions.<version>.errorMessage as a value_mismatch while rendering BOTH sides as None, because the version key ("9.9.9", "1.0.0") contains dots and the reporter's dotted path cannot address inside it. Hypothesis: the divergence is the member's presence/serialization (the recording omits errorMessage for this error class where the emulator serializes a member for it, or the reverse) rather than its text; the authoritative expected value is in the recorded step itself.
Emulator: each failure branch fills in a sentence of the emulator's own — e.g. src/ops/copy_package_versions.rs builds NOT_FOUND via crate::state::package_version_not_found_message(package, version, source_repository), MISMATCHED_REVISION via "Package version <pkg> <v> is at revision <held>, not the specified revision <pinned>.", ALREADY_EXISTS via "Package version <pkg> <v> already exists in repository <repo>.", NOT_ALLOWED via "Copying into package <pkg> is blocked by its package origin configuration in repository <repo>." — none of which is quoted from the service.
Why: failedVersions is the ONLY channel these batch operations have for a per-version failure; clients log, surface and sometimes match that payload, so a body that differs from AWS's in a member's presence or text makes a partially-failed batch report differently than on AWS.
Fix: the PackageVersionError construction shared by src/ops/copy_package_versions.rs, src/ops/update_package_versions_status.rs and src/ops/dispose_package_versions.rs (the version_error() helper and the message helpers it calls in src/state.rs) — serialize the entry exactly as the recorded step does for each errorCode class.
Traces: codeartifact-copy-package-versions-between-repositories#task#8 (CopyPackageVersions, version 9.9.9), codeartifact-package-version-status-transitions#task#6 (UpdatePackageVersionsStatus, version 1.0.0; cluster also covers codeartifact-write-ops-on-missing-resources#task#8), codeartifact-write-ops-on-missing-resources#task#7 (DisposePackageVersions, version 1.0.0), codeartifact-write-ops-on-missing-resources#task#9 (CopyPackageVersions, version 1.0.0).

=== Cause 5 of 7: ListPackageVersions drops publishedTime from every version summary ===
AWS: ListPackageVersions answers 200 with a versions list whose entries carry the version's publish timestamp (publishedTime, an epoch-seconds-with-millis JSON number in the recorded response) beside version/revision/status/origin. (The Smithy model checked in here defines PackageVersionSummary with only version, revision, status and origin — the on-disk model predates the member — so this is recorded behavior: AWS returns publishedTime per summary.)
Emulator: the member is absent from every summary entry. src/state.rs PackageVersion::summary() builds PackageVersionSummary from version, revision, status and origin only, although the SAME record's description() does report published_time, so the data exists and is simply not projected into the listing.
Why: a client that sorts or filters a listing by publish time (release tooling, retention jobs, "newest version" resolution) has to issue one DescribePackageVersion per version against the emulator, and code that reads versions[i].publishedTime gets a missing-key failure where AWS always supplies it.
Fix: PackageVersion::summary() in src/state.rs (and the PackageVersionSummary shape in the generated types if the member is missing there) — project the stored publish timestamp into the listing projection.
Traces: codeartifact-generic-package-publish-and-reads#task#7 (cluster also covers codeartifact-generic-publish-validation-envelopes#task#7 and codeartifact-package-version-status-transitions#task#5).

=== Cause 6 of 7: GetPackageVersionAsset serves the asset with Content-Type application/octet-stream ===
AWS: GetPackageVersionAsset (GET /v1/package/version/asset) returns the asset as the streaming @httpPayload blob (Asset, no @mediaType) plus the X-AssetName / X-PackageVersion / X-PackageVersionRevision headers. Recorded behavior: the response's Content-Type is "application/json" on every successful asset read, for the whole response regardless of the asset's own bytes.
Emulator: Content-Type: application/octet-stream — the generic default a REST-JSON serializer applies to an un-annotated streaming blob payload.
Why: Content-Type is a wire-visible header clients and proxies branch on (SDK response handling, caching layers, test fixtures that assert the header), so a differing media type changes how the body is treated even though the bytes match.
Fix: the response Content-Type for the asset payload in src/ops/get_package_version_asset.rs / the streaming-payload content-type rule in the service's REST-JSON serializer.
Traces: codeartifact-copy-package-versions-between-repositories#task#7 (cluster also covers codeartifact-generic-package-publish-and-reads#task#9).

=== Cause 7 of 7: DescribePackage still answers 200 for a package whose last version was deleted ===
AWS: when the only version of a package is deleted, the package itself no longer exists in that repository, and DescribePackage is refused ResourceNotFoundException, HTTP 404, with an x-amzn-errortype header and message "Package not found in repository. Package = {format=<format>, name=<package>}" — fixed text "Package not found in repository. Package = {format=", ", name=", "}", with the format and name taken from the request (a namespaced package also reports its namespace component in that brace list).
Emulator: HTTP 200 with a package description and no message/errortype at all — the package record survives the deletion of its last version, so a deleted package keeps answering reads.
Why: 200-vs-404 is a control-flow branch, not a cosmetic one: clean-up and existence checks conclude the package is still there, reconciling IaC re-creates nothing, and a client that treats a successful DescribePackage as proof of content gets an empty shell.
Fix: the version-removal paths (src/ops/delete_package_versions.rs / src/ops/dispose_package_versions.rs and the package record in src/state.rs) must drop the package record once it holds no versions, or src/ops/describe_package.rs must treat a package with no versions as absent and raise the modeled ResourceNotFoundException. The earlier issue 20261023T134651Z-codeartifact-removals-and-copies-are-not-reflected-in.md is RESOLVED and does not cover this step.
Traces: codeartifact-package-origin-configuration-and-version-delete#task#11.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator booted from this checkout: 344 steps, 272 passed, 72 mismatched, score 79.1%, 30 divergence clusters, all service "codeartifact". No cluster below carries stub_path or owned_by. Verdicts verbatim:

Cause 1 — [mismatch] codeartifact-generic-publish-validation-envelopes#task#5 PublishPackageVersion, status: expected 400, got 403; $.message: value_mismatch (expected "The provided content hashes do not match the calculated content hashes. Hash algorithm 'SHA-256', Expected hash '0feaf5309dafd2a0254c72595bfe9a4f7ca44106ace6fcd771ac0556ed4c68e2', Actual hash '8b4b7c18a4c6d84ee8d5b349495483c0a38ab030e5afa3a33cceb8ba35e87f96'", got 'The request signature does not match'); $headers.x-amzn-errortype: shape_mismatch (expected 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.codeartifact.controlplane/', got 'SignatureDoesNotMatch'). 2 steps in cluster.

Cause 2 — [mismatch] codeartifact-generic-publish-validation-envelopes#task#6 PublishPackageVersion, 409 -> 409: $.message: value_mismatch (expected "Cannot update existing generic package version 'sim2real/<pkg>/1.0.0' with status 'Published'. Only 'Unfinished' package versions can be updated.", got 'Package version <pkg> 1.0.0 is Published and cannot be published to.')

Cause 3 — [mismatch] codeartifact-package-version-status-transitions#task#9 UpdatePackageVersionsStatus, 400 -> 400: $.message: value_mismatch (expected 'Target status is not allowed.', got "1 validation error detected: Value 'Disposed' at 'targetStatus' failed to satisfy constraint: Member must satisfy enum value set: [Archived, Published, Unlisted]")

Cause 4 — [mismatch] codeartifact-copy-package-versions-between-repositories#task#8 CopyPackageVersions, 200 -> 200: $.failedVersions.9.9.9.errorMessage: value_mismatch (expected None, got None); [mismatch] codeartifact-package-version-status-transitions#task#6 UpdatePackageVersionsStatus, 200 -> 200: $.failedVersions.1.0.0.errorMessage: value_mismatch (expected None, got None), 2 steps in cluster; [mismatch] codeartifact-write-ops-on-missing-resources#task#7 DisposePackageVersions, 200 -> 200: same key, same verdict; [mismatch] codeartifact-write-ops-on-missing-resources#task#9 CopyPackageVersions, 200 -> 200: same key, same verdict. Both sides render as None because the reporter's dotted path cannot address a map key containing dots — the recorded step holds the authoritative expected entry.

Cause 5 — [mismatch] codeartifact-generic-package-publish-and-reads#task#7 ListPackageVersions, 200 -> 200: $.versions[0].publishedTime: missing (expected 1789080921.643, got None). 3 steps in cluster.

Cause 6 — [mismatch] codeartifact-copy-package-versions-between-repositories#task#7 GetPackageVersionAsset, 200 -> 200: $headers.content-type: value_mismatch (expected 'application/json', got 'application/octet-stream'). 2 steps in cluster.

Cause 7 — [mismatch] codeartifact-package-origin-configuration-and-version-delete#task#11 DescribePackage, status: expected 404, got 200; $.message: missing (expected 'Package not found in repository. Package = {format=generic, name=<pkg>}', got None); $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None).

All package names, versions, digests and timestamps above are one run's values.

## Steps to Reproduce
Boot the emulator from this checkout and replay simparity-wire-aws-v1 (each cause's fixture#step is listed under Traces / Issue-Trace-Ids). By hand:
Cause 1: create domain D and repository R; publish_package_version(domain=D, repository=R, format='generic', namespace=N, package=P, packageVersion='1.0.0', assetName='payload.bin', assetContent=B, assetSHA256=sha256(other bytes)) -> expect ValidationException/400 naming Expected hash (declared) and Actual hash (computed over B); observe 403 SignatureDoesNotMatch.
Cause 2: publish 1.0.0 successfully (finished), then publish the same coordinate again -> expect ConflictException/409 "Cannot update existing generic package version 'N/P/1.0.0' with status 'Published'. Only 'Unfinished' package versions can be updated."
Cause 3: update_package_versions_status(domain=D, repository=R, format='generic', package=P, versions=['1.0.0'], targetStatus='Disposed') -> expect ValidationException/400 'Target status is not allowed.'
Cause 4: copy_package_versions(..., versions=['9.9.9']) for a version the source repository does not hold, update_package_versions_status(..., versions=['1.0.0']) and dispose_package_versions(..., versions=['1.0.0']) against missing resources -> each answers 200; compare failedVersions['<version>'] member-for-member with the recorded step (errorCode plus the presence/text of errorMessage).
Cause 5: publish 1.0.0, then list_package_versions(domain=D, repository=R, format='generic', package=P) -> every versions[] entry must carry publishedTime.
Cause 6: get_package_version_asset(domain=D, repository=R, format='generic', namespace=N, package=P, packageVersion='1.0.0', asset='payload.bin') -> response Content-Type must be application/json.
Cause 7: publish 1.0.0 as the package's only version, delete that version, then describe_package(domain=D, repository=R, format='generic', package=P) -> expect ResourceNotFoundException/404 "Package not found in repository. Package = {format=generic, name=P}"; observe 200 with a package description.
