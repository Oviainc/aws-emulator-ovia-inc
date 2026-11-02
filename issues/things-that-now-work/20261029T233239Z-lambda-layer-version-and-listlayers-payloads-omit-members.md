Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-layer-delete-and-notfound-errors
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-layer-delete-and-notfound-errors lambda-layer-error-paths lambda-layer-filter-by-runtime-and-architecture lambda-layer-lifecycle-and-permissions lambda-layer-multi-version-lifecycle lambda-layer-multiple-versions lambda-layer-not-found-and-idempotent-delete lambda-layer-permission-account-grant lambda-layer-permission-error-paths lambda-layer-permission-lifecycle lambda-layer-permission-organization-grant lambda-layer-permission-public-and-org lambda-layer-permission-revision-precondition lambda-layer-publish-and-inspect lambda-layer-publish-and-permissions lambda-layer-publish-multi-version-lifecycle lambda-layer-runtime-architecture-filter lambda-layer-version-permission-lifecycle lambda-layer-version-permission-policy lambda-layer-version-permission-sharing lambda-list-layers-filter-by-runtime-and-arch lambda-layer-list-with-filters
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|PublishLayerVersion|2xx->2xx|$.Content.UncompressedCodeSize
Issue-Key: lambda|GetLayerVersion|2xx->2xx|$.Content.UncompressedCodeSize
Issue-Key: lambda|GetLayerVersionByArn|2xx->2xx|$.Content.UncompressedCodeSize
Issue-Key: lambda|ListLayers|2xx->2xx|$.Layers[*]
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda layer version and ListLayers payloads omit members the recorded responses carry

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishLayerVersion

## Description
Shared fault: the layer-version content serializer and the ListLayers serializer omit members that the recorded AWS responses carry. Four net-new clusters.

AWS: recorded behavior only for the first member — the on-disk Smithy LayerVersionContentOutput declares Location/CodeSha256/CodeSize/SigningProfileVersionArn/SigningJobArn/ResolvedS3Object, and the recordings additionally show `Content.UncompressedCodeSize` present on PublishLayerVersion, GetLayerVersion and GetLayerVersionByArn (0 for the probe archives). For ListLayers, the recorded Layers entry carries the `Description` and `State` members and its `LatestMatchingVersion` carries `LicenseInfo` and `Description`, rendered explicitly even when unset; the emulator's entry has those keys absent.

Emulator: `Content.UncompressedCodeSize` is never emitted; the ListLayers entry is a shorter object missing the members above.

Why: a client reading the uncompressed size of a layer archive (the dimension against which Lambda's unzipped-size quota is measured) gets nothing back, and a client comparing a ListLayers entry against the recorded shape sees a different member set.

Fix: the layer-version content serializer must render UncompressedCodeSize from layer state, and the ListLayers item serializer must render the full recorded member set.

Cause 1 of 4: PublishLayerVersion, 201 — fixture lambda-layer-delete-and-notfound-errors step setup#1; `$.Content.UncompressedCodeSize` absent.
Cause 2 of 4: GetLayerVersion, 200 — fixture lambda-layer-lifecycle-and-permissions step task#2; same member absent.
Cause 3 of 4: GetLayerVersionByArn, 200 — fixture lambda-layer-lifecycle-and-permissions step task#3; same member absent.
Cause 4 of 4: ListLayers, 200 — fixture lambda-layer-filter-by-runtime-and-architecture step task#3; `$.Layers[0]` lacks the `Description`/`State` members and `LatestMatchingVersion` lacks `LicenseInfo`/`Description`.

## Evidence
Cluster details: three clusters `$.Content.UncompressedCodeSize: missing (expected 0, got None)` (expected status == actual status 201/200/200); ListLayers cluster `$.Layers[0]: value_mismatch` where the expected object contains `Description`, `State` and `LatestMatchingVersion.{LicenseInfo,Description}` and the emulator's object contains only LayerName/LayerArn/LatestMatchingVersion{LayerVersionArn,Version,CreatedDate,CompatibleArchitectures,CompatibleRuntimes}. Smithy: LayerVersionContentOutput does not declare UncompressedCodeSize, so this is recorded behavior only.

## Steps to Reproduce
Replay the corpus. Fixture lambda-layer-delete-and-notfound-errors step setup#1: PublishLayerVersion and read `Content` in the 201 body. Fixture lambda-layer-filter-by-runtime-and-architecture step task#3: ListLayers with runtime/architecture filters and compare the entry's member set.
