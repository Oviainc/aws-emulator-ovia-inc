Issue-Source: trace-sweep-divergence
Issue-Trace: api-image-layer-upload-and-push-workflow
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-image-layer-upload-and-push-workflow api-repository-deletion-and-non-empty-error-paths api-repository-creation-catalog-lifecycle api-resource-tagging-workflow
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public|DescribeImages|2xx->2xx|$.imageDetails[*]
Issue-Key: ecr-public|BatchDeleteImage|2xx->2xx|$.imageIds[*].imageTag
Issue-Key: ecr-public|CreateRepository|2xx->2xx|$.catalogData.marketplaceCertified
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# ECR Public response field fidelity: wrong imageSizeInBytes, dropped imageTag, injected marketplaceCertified

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Three independent response-serialization/state defects in the ecr-public emulator, all of them successful (HTTP 200) responses whose body does not match what Amazon ECR Public returns. All three must be closed before this issue is resolved.

Cause 1 of 3: DescribeImages reports the wrong imageSizeInBytes for a pushed image.
AWS: Smithy ImageDetail.imageSizeInBytes (long) — "The size, in bytes, of the image in the repository. If the image is a manifest list, this is the max size of all manifests in the list." The doc note explains the value tracks the COMPRESSED layer content the client pushed, which is why the recorded 200 response for the probe image reports 209 after PutImage of a manifest whose layer blobs total 209 bytes. Recorded behavior: imageSizeInBytes = 209 for the single-manifest image sha256:a8b4056...50efe2.
Emulator: returns imageSizeInBytes = 439 for the same image in the same DescribeImages 200 response (every other member of imageDetails[0] — registryId, repositoryName, imageDigest, imageTags, imageManifestMediaType, artifactMediaType — matches). Hypothesis: the size is accumulated over the manifest document and/or the config blob in addition to the layer blobs, rather than over the layers the manifest references.
Why: clients that display or budget on image size (gallery UIs, quota/告 accounting, pull-size estimation, "did my layer dedupe work" assertions) read a number roughly 2x the real one, and any test that pushes a known blob set and asserts the resulting size fails against the emulator while passing against AWS.
Fix: the image-size computation that backs ImageDetail — the PutImage/state layer that records an image's size when the manifest is stored, and the DescribeImages handler that projects it. It must sum only the blob sizes the manifest's layers reference (max across manifests for a manifest list), excluding the manifest bytes themselves.
Traces: api-image-layer-upload-and-push-workflow#task#10 (also seen in api-repository-deletion-and-non-empty-error-paths#task#10).

Cause 2 of 3: BatchDeleteImage omits imageTag from the returned imageIds.
AWS: Smithy BatchDeleteImage output imageIds -> ImageIdentifierList of ImageIdentifier{imageDigest, imageTag}; ImageIdentifier.imageTag is "The tag that's used for the image." The recorded 200 response echoes the deleted image fully identified — imageIds[0].imageTag = 'probe-tag' alongside its digest — so a caller learns which tag was actually removed.
Emulator: returns imageIds[0] with imageTag absent/None in an otherwise matching 200 response.
Why: a client that deletes by digest cannot learn which tags went away, and a client that deletes by tag cannot confirm its own request was the thing honored; code that reconciles the returned ImageIdentifier against the requested one (or re-tags afterwards) sees an unidentified entry and treats the delete as partially unacknowledged.
Fix: the BatchDeleteImage handler/serializer — when resolving each requested image it must carry the tag(s) it detached into the returned ImageIdentifier, not only the digest.
Traces: api-image-layer-upload-and-push-workflow#task#15.

Cause 3 of 3: CreateRepository emits catalogData.marketplaceCertified when AWS omits it.
AWS: Smithy RepositoryCatalogData.marketplaceCertified (boolean, not required, no @default) — "Indicates whether the repository is certified by Amazon Web Services Marketplace." A freshly created repository is not Marketplace-certified and the recorded 200 CreateRepository response carries no marketplaceCertified key in catalogData at all.
Emulator: serializes "marketplaceCertified": false into catalogData on the 200 response.
Why: the member is a tri-state on the wire (absent = AWS has made no certification statement, false = explicitly not certified). Emitting false for an unset member makes absence unobservable, and clients/SDK round-trips that diff catalog data or feed it back into PutRepositoryCatalogData see a field the service never set.
Fix: the CreateRepository response serializer for RepositoryCatalogData — omit non-required, non-defaulted members that are unset rather than writing a zero value.
Traces: api-repository-creation-catalog-lifecycle#setup#1 (also seen in api-resource-tagging-workflow#setup#1).

## Evidence
Cause 1 — DescribeImages, fixture api-image-layer-upload-and-push-workflow step task#10, expected 200 / actual 200: $.imageDetails[0] expected imageSizeInBytes 209, got 439 (imagePushedAt differs only as a per-run timestamp and is not part of this finding). Cluster also covers api-repository-deletion-and-non-empty-error-paths#task#10.
Cause 2 — BatchDeleteImage, fixture api-image-layer-upload-and-push-workflow step task#15, expected 200 / actual 200: $.imageIds[0].imageTag missing (expected 'probe-tag', got None).
Cause 3 — CreateRepository, fixture api-repository-creation-catalog-lifecycle step setup#1, expected 200 / actual 200: $.catalogData.marketplaceCertified extra (expected absent, got False). Cluster also covers api-resource-tagging-workflow#setup#1.
Replay context: trace set simparity-wire-aws-v1, emulator_up true, state_reset_failures 0, so none of these ran against leftover state.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Cause 1: replay fixture api-image-layer-upload-and-push-workflow through step task#10 — create the public repository, InitiateLayerUpload/UploadLayerPart/CompleteLayerUpload the probe blob, PutImage the v2 manifest, then DescribeImages the repository and read imageDetails[0].imageSizeInBytes: expect 209 (sum of the manifest's layer blobs), emulator returns 439.
3. Cause 2: continue the same fixture to step task#15 — BatchDeleteImage the pushed image and read imageIds[0]: expect both imageDigest and imageTag 'probe-tag', emulator returns the digest with no imageTag.
4. Cause 3: replay fixture api-repository-creation-catalog-lifecycle step setup#1 — CreateRepository with catalogData and inspect the 200 body's catalogData: AWS has no marketplaceCertified key, emulator emits "marketplaceCertified": false.
