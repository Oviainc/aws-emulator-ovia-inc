Issue-Source: trace-sweep-divergence
Issue-Trace: api-image-push-via-layer-upload
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-image-push-via-layer-upload api-image-scanning-and-findings ecr-archive-and-restore-image-storage-class ecr-archive-and-restore-storage-class ecr-multi-image-isolation-in-shared-repository
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|DescribeImages|2xx->2xx|$.imageDetails[*]
Issue-Key: ecr|BatchGetImage|2xx->2xx|$.images[*].imageManifestSize
Issue-Key: ecr|DescribeImages|2xx->2xx|$.imageDetails[*].artifactMediaType
Issue-Key: ecr|DescribeImages|2xx->2xx|$.imageDetails[*].imageSizeInBytes
Issue-Key: ecr|DescribeImages|2xx->2xx|$.imageDetails[*].lastActivatedAt
Issue-Key: ecr|DescribeImages|2xx->2xx|$.imageDetails[*].lastArchivedAt
Issue-Key: ecr|DescribeImages|2xx->2xx|$.imageDetails[*].imageStatus
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECR image metadata fields are missing or wrong in image read operations

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
This issue bundles 4 DISTINCT root causes in ECR's image-read surface: modeled members that are never populated, a size that is computed from the wrong thing, and an image status that never leaves ACTIVE. All four must be closed before submitting; each carries its own fixture and step.

Cause 1 of 4: DescribeImages never returns artifactMediaType, and imageSizeInBytes reports the manifest's own byte length instead of the image size.
AWS: Smithy ecr#ImageDetail declares artifactMediaType (MediaType, doc: "The artifact media type of the image") alongside imageManifestMediaType, and imageSizeInBytes (long, doc: "The size, in bytes, of the image in the repository"). For a v2 Docker manifest the recording shows artifactMediaType = the config descriptor's media type (example: application/vnd.docker.container.image.v1+json) and imageSizeInBytes = the summed size of the image's layers, not the size of the manifest document.
Emulator: omits artifactMediaType entirely from every ImageDetail and reports imageSizeInBytes as the manifest JSON length (recorded example: 440, i.e. the manifest bytes, where AWS reported 256, the layer total).
Why: artifactMediaType is how clients tell a container image from a non-image OCI artifact (SBOM, signature, Helm chart) without fetching the manifest; absent, every artifact looks like an image. A size that tracks the manifest rather than the content makes storage accounting, quota and cost reporting wrong by orders of magnitude.
Fix: in the DescribeImages handler/serializer, derive artifactMediaType from the stored manifest's config descriptor and compute imageSizeInBytes from the registered layer sizes in image state.
Traces: api-image-push-via-layer-upload#task#8 (representative); same cluster: api-image-scanning-and-findings#task#7, ecr-archive-and-restore-image-storage-class#task#10, ecr-archive-and-restore-storage-class#task#10.

Cause 2 of 4: BatchGetImage omits the manifest size member.
AWS: recorded behavior only — the on-disk ecr#Image shape in this checkout declares registryId, repositoryName, imageId, imageManifest and imageManifestMediaType and has no imageManifestSize member, but the recording shows AWS returning images[].imageManifestSize carrying the byte length of the returned manifest.
Emulator: the member is absent from the BatchGetImage response.
Why: clients that stream or verify the manifest use the advertised size to allocate and to check they received the whole document; absent, they must trust an unbounded read. The service model shipped with the emulator also needs the member before the serializer can emit it.
Fix: add imageManifestSize to the Image output in the BatchGetImage serializer (and to the ecr Smithy model in this checkout if it is to be generated), populated from the stored manifest's byte length.
Traces: api-image-push-via-layer-upload#task#7.

Cause 3 of 4: archive/restore transitions are not timestamped, so lastArchivedAt and lastActivatedAt are never returned.
AWS: Smithy ecr#ImageDetail declares lastArchivedAt ("when the image was last transitioned to Amazon ECR archive") and lastActivatedAt ("when the image was last restored from Amazon ECR archive to Amazon ECR standard"). After an archive-then-restore cycle both members are present on the image's detail.
Emulator: both members are missing from the ImageDetail after the cycle (the same response also carries the Cause 1 artifactMediaType/imageSizeInBytes defects).
Why: these timestamps are the only record of a storage-class transition; lifecycle tooling and cost reporting that decide when to re-archive or when a restore completed have nothing to read.
Fix: record the transition instants in the image's storage-class state and emit them from the DescribeImages serializer.
Traces: ecr-archive-and-restore-image-storage-class#task#12 (representative; ecr-archive-and-restore-storage-class#task#12 is in the same cluster).

Cause 4 of 4: an archived image keeps reporting imageStatus ACTIVE.
AWS: Smithy ecr#ImageDetail.imageStatus targets the ecr#ImageStatus enum with values ACTIVE, ARCHIVED, ACTIVATING. After the image's storage class is moved to archive, DescribeImages reports imageStatus = ARCHIVED for that image and leaves its neighbours in the shared repository ACTIVE.
Emulator: reports imageStatus = ACTIVE for the archived image, so the archive request has no visible effect on the image it named.
Why: ARCHIVED is the signal that a pull will need a restore first; a client told ACTIVE will attempt a direct pull and never issue the restore, and per-image isolation inside a repository cannot be verified at all.
Fix: have the archive path mutate the image's stored status (and the DescribeImages serializer read it) per image rather than leaving the default ACTIVE.
Traces: ecr-multi-image-isolation-in-shared-repository#task#20.

## Evidence
Cause 1 — fixture api-image-push-via-layer-upload, step task#8, status 200 both sides: expected $.imageDetails[0] to include 'artifactMediaType': 'application/vnd.docker.container.image.v1+json' and 'imageSizeInBytes': 256; got an ImageDetail with no artifactMediaType and 'imageSizeInBytes': 440 (the manifest length).
Cause 2 — fixture api-image-push-via-layer-upload, step task#7, status 200 both sides: $.images[0].imageManifestSize missing (expected 440, got None).
Cause 3 — fixture ecr-archive-and-restore-image-storage-class, step task#12, status 200 both sides: $.imageDetails[0].lastArchivedAt missing, $.imageDetails[0].lastActivatedAt missing, $.imageDetails[0].artifactMediaType missing, $.imageDetails[0].imageSizeInBytes expected 120, got 439.
Cause 4 — fixture ecr-multi-image-isolation-in-shared-repository, step task#20, status 200 both sides: $.imageDetails[0].imageStatus expected 'ARCHIVED', got 'ACTIVE'.

## Steps to Reproduce
Replay the wire corpus and re-run the fixture steps listed per cause. Manually:
1. CreateRepository R, upload a layer, PutImage a v2 Docker manifest, then DescribeImages(R) -> check artifactMediaType is present and imageSizeInBytes equals the summed layer sizes, not the manifest length.
2. BatchGetImage(R, imageIds=[{imageTag:'v1'}]) -> check images[0].imageManifestSize.
3. Archive the image's storage class, restore it, then DescribeImages(R) -> check lastArchivedAt and lastActivatedAt are present.
4. In a repository holding several images, archive exactly one, then DescribeImages for it -> check imageStatus == 'ARCHIVED' while the others stay ACTIVE.
