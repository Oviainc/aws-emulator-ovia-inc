Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.update-image-storage-class-unsupported-transition-rejected ecr.image-scan-on-archived-image-rejected ecr.oci-manifest-media-type-negotiation ecr.create-repository-invalid-name-rejected ecr.e2e-image-push-scan-replicate-cleanup ecr.image-storage-class-archival-blocks-mutating-ops ecr.start-image-scan-on-archived-image-rejected ecr.scan-not-found-before-scan-started
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.update-image-storage-class-unsupported-transition-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: ecr.image-scan-on-archived-image-rejected|run#9|AssertionError|assert control_result == {
Issue-Key: ecr.oci-manifest-media-type-negotiation|run#31|AssertionError|assert converted["imageManifestMediaType"] == DOCKER_MANIFEST_V2, (
Issue-Key: ecr.create-repository-invalid-name-rejected|run#13|AssertionError|assert message == invalid_name, f"expected message {invalid_name!r}, got {message!r}"
Issue-Key: ecr.e2e-image-push-scan-replicate-cleanup|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: ecr.image-storage-class-archival-blocks-mutating-ops|run#17|AssertionError|assert message == (
Issue-Key: ecr.start-image-scan-on-archived-image-rejected|run#9|AssertionError|assert message == (
Issue-Key: ecr.scan-not-found-before-scan-started|run#8|AssertionError|assert message == (
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# ECR: image storage-class transitions, scan evaluation, media-type negotiation, replication status and error-message construction diverge (6 causes)

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateImageStorageClass

## Description
Six distinct root causes, all in ECR, found by the behavioral suite. Related open issues that do NOT cover these checks (the replay report did not mark any of the checks below as owned by them): 20261024T014332Z-ecr-asynchronous-effects-never-occur-and-several-enforcement.md (its cause list stops at a DescribeImageScanFindings NotImplemented stub and an artifactMediaType KeyError, both of which now get past that point), 20261022T234922Z-ecr-completelayerupload-is-a-501-stub-cfn-provisioned.md, 20261023T153036Z-ecr-lifecycle-policy-text-is-reformatted-on-read.md, 20261024T014332Z-ecr-updateimagestorageclass-stub.md (UpdateImageStorageClass now answers, so the stub issue no longer describes what these checks hit).

Cause 1 of 6: UpdateImageStorageClass accepts re-archiving an image that is already in the ARCHIVE storage class.
AWS: Smithy: UpdateImageStorageClass(repositoryName, imageId, targetStorageClass: TargetStorageClass = STANDARD|ARCHIVE) declares ImageStorageClassUpdateNotSupportedException (client error, HTTP 400, doc: "The requested image storage class update is not supported."); output imageStatus is ImageStatus = ACTIVE|ARCHIVED|ACTIVATING. Recorded behavior: targetStorageClass=ARCHIVE on an image whose imageStatus is already ARCHIVED is refused with HTTP 400 ImageStorageClassUpdateNotSupportedException (recorded message, as an example: "Image cannot be updated to ARCHIVE storage class when it is in ARCHIVED state"), and the image's status is left unchanged.
Emulator: the second ARCHIVE transition returns HTTP 200 with {imageStatus: 'ARCHIVED'} — no storage-class state machine is consulted, so the no-op/unsupported transition is indistinguishable from the first legitimate ACTIVE->ARCHIVE transition.
Why: a client that uses the exception to detect "already archived" (idempotency guards, archival reconciliation loops) never sees it and concludes it performed a transition that never happened; the emulator is overly permissive on a modelled error path.
Fix: the UpdateImageStorageClass handler's precondition check against the image's stored storage-class/status state.
Traces: ecr.update-image-storage-class-unsupported-transition-rejected.

Cause 2 of 6: basic image scanning always reports COMPLETE — the scan never evaluates the image.
AWS: Smithy: StartImageScan/DescribeImageScanFindings return ImageScanStatus{status: ScanStatus, description}, ScanStatus = IN_PROGRESS|COMPLETE|FAILED|UNSUPPORTED_IMAGE|ACTIVE|PENDING|SCAN_ELIGIBILITY_EXPIRED|FINDINGS_UNAVAILABLE|LIMIT_EXCEEDED|IMAGE_ARCHIVED; StartImageScan also declares UnsupportedImageTypeException. Recorded behavior: a synthetic single-layer image with no recognizable OS/package manager reaches the terminal status FAILED with description "UnsupportedImageError: The operating system and/or package manager are not supported."
Emulator: DescribeImageScanFindings settles at {status: 'COMPLETE', description: 'The scan was completed successfully.'} for that same image, i.e. the scan outcome is a fixed success constant rather than a function of the scanned image.
Why: a client cannot distinguish an image that was genuinely scanned from one that cannot be scanned; CI gates that branch on FAILED/UNSUPPORTED_IMAGE take the "clean scan" path for images AWS never scanned, and findings-based policies are never exercised.
Fix: the scan state machine behind StartImageScan / the DescribeImageScanFindings projection — derive the terminal status (and its description) from the stored image's manifest/config instead of emitting a constant COMPLETE.
Traces: ecr.image-scan-on-archived-image-rejected.

Cause 3 of 6: BatchGetImage ignores acceptedMediaTypes — an OCI manifest is returned to a client that accepts only the Docker v2 schema.
AWS: Smithy: BatchGetImageRequest.acceptedMediaTypes (MediaTypeList) is documented as "The accepted media types for the request. Valid values: application/vnd.docker.distribution.manifest.v1+json | application/vnd.docker.distribution.manifest.v2+json | application/vnd.oci.image.manifest.v1+json". Recorded behavior: for an image stored as application/vnd.oci.image.manifest.v1+json, a BatchGetImage whose acceptedMediaTypes is only [application/vnd.docker.distribution.manifest.v2+json] is answered with the manifest converted into the accepted schema, and images[].imageManifestMediaType reports the accepted (Docker v2) media type.
Emulator: returns imageManifestMediaType 'application/vnd.oci.image.manifest.v1+json' regardless of acceptedMediaTypes — the stored manifest is echoed unconverted and the negotiation input is dropped.
Why: a Docker-v2-only client (older daemons, tooling that validates the manifest media type) is handed a manifest schema it cannot parse, while AWS would have served it a usable one; media-type negotiation is untestable.
Fix: the BatchGetImage handler's manifest negotiation/conversion step (honour acceptedMediaTypes, convert or report a failure, and set imageManifestMediaType from the served manifest).
Traces: ecr.oci-manifest-media-type-negotiation.

Cause 4 of 6: CreateRepository's name-validation error names the wrong input member ('repositoryNames').
AWS: Smithy: CreateRepositoryRequest.repositoryName @required, @length min 2 max 256, @pattern ^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$; error InvalidParameterException (client, HTTP 400). The message is built from the request: it names the violated member and its constraint, i.e. "Invalid parameter at 'repositoryName' failed to satisfy constraint: 'must satisfy regular expression '<the pattern>''" for a name containing uppercase characters.
Emulator: returns the right code (InvalidParameterException) but names member 'repositoryNames' — the plural member of DescribeRepositories — for a CreateRepository call that has no such member.
Why: the request-derived part of the message is wrong, so a client (or a human reading a failed deploy log) is pointed at a parameter it never sent; validators that key on the reported member name mis-attribute the failure.
Fix: the CreateRepository input validation path — the member name in the InvalidParameterException must come from the member actually being validated.
Traces: ecr.create-repository-invalid-name-rejected.

Cause 5 of 6: with a replication configuration in place before the push, DescribeImageReplicationStatus never reaches a terminal status for the destination Region.
AWS: Smithy: DescribeImageReplicationStatus returns replicationStatuses[]{region, registryId, status, failureCode}. Recorded behavior: after PutReplicationConfiguration adds a rule whose repositoryFilters PREFIX_MATCH the repository, an image pushed afterwards gains a replicationStatuses entry for the destination Region that settles to COMPLETE (or FAILED).
Emulator: the configuration is accepted and the push succeeds, but the destination-Region entry never reports COMPLETE/FAILED; the check's replication poll (180 s budget, 10 s interval) is still polling when the harness's 60 s cap cuts the check off, so no terminal status is ever observed. The scan leg of the same check completes immediately (see cause 2), so the remaining time is spent in the replication poll.
Why: no client can confirm that configured replication happened; a promotion pipeline that waits for COMPLETE before deploying from the destination registry blocks forever.
Fix: the replication engine driving DescribeImageReplicationStatus for images pushed under an active replication rule. The related open issue 20261024T014332Z-ecr-asynchronous-effects-never-occur-and-several-enforcement.md describes the same engine for other checks but does not carry this check's replay key.
Traces: ecr.e2e-image-push-scan-replicate-cleanup.

Cause 6 of 6: the request-derived messages of ImageArchivedException and ScanNotFoundException are built from a different template than AWS's.
AWS: Smithy: ImageArchivedException (client, HTTP 400, doc "The specified image is archived and cannot be scanned.") and ScanNotFoundException (client, HTTP 400). Recorded behavior, independently encoded by three checks: the message interpolates the request's imageId, repository name and registry id in a fixed form — ImageArchivedException: "The image with imageId {imageDigest:'<d>', imageTag:'<t>'} is archived within the repository with name '<repo>' in the registry with id '<account>'"; ScanNotFoundException: "Image scan does not exist for the image with '{imageDigest:'<d>', imageTag:'<t>'}' in the repository with name '<repo>' in the registry with id '<account>'".
Emulator: the HTTP status and the error codes are correct, but the templates differ — ImageArchivedException is rendered as "The image with 'imageId {imageDigest:'<d>', imageTag:'<t>'}' in the repository with name '<repo>' in the registry with id '<account>' is archived and cannot be scanned" (clause order changed, the imageId brace-expression wrapped in quotes), and ScanNotFoundException inserts a literal "imageId " inside the quoted brace-expression. The interpolated request values themselves are correct.
Why: clients and operators that parse or match these messages (the facts are request-derived, so the rendering is deterministic, not environment-specific) see a shape AWS never emits; three independent checks assert the AWS form.
Fix: the error-message construction for ImageArchivedException and ScanNotFoundException in the StartImageScan / DescribeImageScanFindings handlers. Lowest-severity cause in this bundle: status and error code already match.
Traces: ecr.image-storage-class-archival-blocks-mutating-ops, ecr.start-image-scan-on-archived-image-rejected, ecr.scan-not-found-before-scan-started.

## Evidence
Cause 1 — AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'registryId': '000000000000', 'repositoryName': 'sim2real-ecr-cabd32f654ef', 'imageId': {'imageDigest': 'sha256:8eb53882...'}, 'imageStatus': 'ARCHIVED', 'ResponseMetadata': {'HTTPStatusCode': 200}} for a second targetStorageClass='ARCHIVE' on an already-ARCHIVED image; expected HTTP 400 ImageStorageClassUpdateNotSupportedException.
Cause 2 — AssertionError on the scan control: got {'status': 'COMPLETE', 'description': 'The scan was completed successfully.'}, expected {'status': 'FAILED', 'description': 'UnsupportedImageError: The operating system and/or package manager are not supported.'} for a synthetic OS-less single-layer image.
Cause 3 — AssertionError: BatchGetImage reported imageManifestMediaType 'application/vnd.oci.image.manifest.v1+json' for a client that accepts only 'application/vnd.docker.distribution.manifest.v2+json'; the manifest was not converted into the accepted schema.
Cause 4 — AssertionError: expected message "Invalid parameter at 'repositoryName' failed to satisfy constraint: 'must satisfy regular expression '[a-z0-9]+((\\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\\.|_|__|-+)[a-z0-9]+)*)*''", got the same text with member 'repositoryNames' (code InvalidParameterException matched).
Cause 5 — check cut off: "Check timed out after 60.0s" while polling describe_image_replication_status(repositoryName, imageId) for an entry with region == the destination Region and status in (COMPLETE, FAILED) (poll budget 180 s, interval 10 s), after put_replication_configuration succeeded and the image pushed.
Cause 6 — StartImageScan on an archived image returned code ImageArchivedException with message "The image with 'imageId {imageDigest:'sha256:3ecc739e...', imageTag:'null'}' in the repository with name 'sim2real-ecr-behav-83edf7561284' in the registry with id '000000000000' is archived and cannot be scanned"; expected "The image with imageId {imageDigest:'sha256:3ecc739e...', imageTag:'null'} is archived within the repository with name '<repo>' in the registry with id '<account>'". DescribeImageScanFindings on a never-scanned image returned code ScanNotFoundException with "Image scan does not exist for the image with 'imageId {imageDigest:'sha256:0347998c...', imageTag:'null'}' in the repository ..."; expected the same sentence without the literal 'imageId ' inside the quoted brace-expression.

## Steps to Reproduce
All against the emulator's ECR endpoint with basic scanning enabled on the registry.
Cause 1: create_repository; push a single-layer image; update_image_storage_class(repositoryName, imageId={'imageDigest': d}, targetStorageClass='ARCHIVE'); poll describe_images until imageStatus == 'ARCHIVED'; call the identical update_image_storage_class again and inspect the response/error.
Cause 2: push a minimal synthetic image (empty gzip tar layer + config, no OS); start_image_scan on its digest; poll describe_image_scan_findings(...)['imageScanStatus'] to a terminal status and inspect status/description.
Cause 3: push an image whose manifest mediaType is application/vnd.oci.image.manifest.v1+json; call batch_get_image(repositoryName, imageIds=[{'imageDigest': d}], acceptedMediaTypes=['application/vnd.docker.distribution.manifest.v2+json']) and read images[0]['imageManifestMediaType'].
Cause 4: create_repository(repositoryName='<valid-lowercase-name>-INVALID-Upper') and read the InvalidParameterException message's member name.
Cause 5: describe_registry to capture the existing replication rules; put_replication_configuration adding a rule with destinations=[{'region': <other region>, 'registryId': <account>}] and repositoryFilters=[{'filter': <repo>, 'filterType': 'PREFIX_MATCH'}]; push an image to <repo>; poll describe_image_replication_status(repositoryName=<repo>, imageId={'imageDigest': d}) for an entry for the destination Region with status COMPLETE or FAILED.
Cause 6: (a) archive an image via update_image_storage_class then call start_image_scan on it and read the error message; (b) push an image and call describe_image_scan_findings on it before any scan was started and read the error message.
