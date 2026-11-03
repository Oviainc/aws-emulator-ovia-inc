Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-lls-s3-published-layer-reference-walk
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-lls-s3-published-layer-reference-walk lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling lambda-lls-layer-reference-forms-name-arn-and-version-arn lambda-lls-version-number-mistakes-then-list-first lambda-layer-filter-by-runtime-and-architecture lambda-layer-lifecycle-and-permissions lambda-layer-list-with-filters lambda-layer-multi-version-lifecycle lambda-layer-publish-and-inspect lambda-layer-publish-and-permissions lambda-layer-publish-multi-version-lifecycle lambda-layer-runtime-architecture-filter lambda-list-layers-filter-by-runtime-and-arch
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|PublishLayerVersion|2xx->2xx|$.Content.CodeSha256
Issue-Key: lambda|PublishLayerVersion|2xx->2xx|$.Content.CodeSize
Issue-Key: lambda|PublishLayerVersion|2xx->2xx|$.Content.Location
Issue-Key: lambda|GetLayerVersion|2xx->2xx|$.Content.CodeSha256
Issue-Key: lambda|GetLayerVersion|2xx->2xx|$.Content.CodeSize
Issue-Key: lambda|GetLayerVersion|2xx->2xx|$.Content.Location
Issue-Key: lambda|GetLayerVersionByArn|2xx->2xx|$.Content.CodeSha256
Issue-Key: lambda|GetLayerVersionByArn|2xx->2xx|$.Content.CodeSize
Issue-Key: lambda|GetLayerVersionByArn|2xx->2xx|$.Content.Location
Issue-Key: lambda|ListLayers|2xx->2xx|$.Layers[*]
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# Lambda layer version payloads omit content metadata and an unpresigned location

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishLayerVersion

## Description
The LayerVersionContentOutput the emulator builds is not AWS's: it never measures or hashes the layer archive when the content came from S3, and its Location is a bare object path rather than the short-lived presigned URL the model documents. The layer listing additionally emits a non-AWS timestamp format. 7 causes, all in the layer-content projection and the layer serializers.

Cause 1 of 7 - PublishLayerVersion reports CodeSize 0 and no CodeSha256 for an S3-sourced layer.
AWS: Smithy: PublishLayerVersion output Content -> LayerVersionContentOutput.CodeSize ('The size of the layer archive in bytes') and .CodeSha256 ('The SHA-256 hash of the layer archive'). AWS reads the S3 object named by Content.S3Bucket/S3Key and reports its real byte size and its base64 SHA-256.
Emulator: reports Content.CodeSize=0 and omits Content.CodeSha256 altogether, i.e. it never reads the object. (The recording's 219-byte size and its hash are this probe's archive; the requirement is that both members report the published archive's own size and hash.)
Why: a client that records CodeSha256 to detect whether a layer changed, or that checks CodeSize against the layer-size ceiling, has a zero and a missing hash to work with, so every comparison reports 'unchanged / empty'.
Fix: the PublishLayerVersion handler's S3 content resolution (it must fetch and measure the object) and the LayerVersionContentOutput projection.
Traces: lambda-lls-s3-published-layer-reference-walk#task#3.

Cause 2 of 7 - GetLayerVersion repeats the zero size and missing hash.
AWS: Smithy: GetLayerVersion output Content -> the same LayerVersionContentOutput members.
Emulator: CodeSize=0 and no CodeSha256 on the read path too, because the state the publish wrote has neither.
Fix: as cause 1 - the stored layer-version record must carry size and hash.
Traces: lambda-lls-s3-published-layer-reference-walk#task#5.

Cause 3 of 7 - GetLayerVersionByArn repeats it.
AWS: as cause 2, reached by LayerVersionArn.
Emulator: as cause 2.
Fix: as cause 1.
Traces: lambda-lls-s3-published-layer-reference-walk#task#6.

Cause 4 of 7 - PublishLayerVersion's Content.Location is not a presigned URL.
AWS: Smithy: LayerVersionContentOutput.Location is documented as 'A link to the layer archive in Amazon S3 that is valid for 10 minutes', i.e. a presigned S3 GET URL carrying the SigV4 query parameters (X-Amz-Algorithm, X-Amz-Credential, X-Amz-Date, X-Amz-Expires, X-Amz-SignedHeaders, X-Amz-Signature).
Emulator: returns a bare path of the form https://awslambda-<region>-layers.s3.<region>.amazonaws.com/snapshots/<account>/<layer-name>-<version> with no query string, failing the recorded shape assertion on that member.
Why: a client that downloads the layer archive with a plain GET against the returned link - the documented use - gets an access-denied response instead of the archive.
Fix: the layer-content serializer's Location builder (presign the object).
Traces: lambda-lls-s3-published-layer-reference-walk#task#7 (2 steps affected).

Cause 5 of 7 - GetLayerVersion's Content.Location is likewise unsigned.
AWS: as cause 4.
Emulator: as cause 4, on the read path.
Fix: as cause 4.
Traces: lambda-lls-layer-reference-forms-name-arn-and-version-arn#task#3 (3 steps affected).

Cause 6 of 7 - GetLayerVersionByArn's Content.Location is likewise unsigned.
AWS: as cause 4.
Emulator: as cause 4.
Fix: as cause 4.
Traces: lambda-lls-layer-reference-forms-name-arn-and-version-arn#task#5.

Cause 7 of 7 - ListLayers emits a non-AWS CreatedDate format.
AWS: Smithy: ListLayers output Layers -> LayersListItem.LatestMatchingVersion -> LayerVersionsListItem.CreatedDate, documented as 'the date that the version was created, in ISO 8601 format. For example, 2018-11-27T15:10:45.123+0000' - a numeric +0000 offset, not a 'Z' designator.
Emulator: emits the 'Z'-suffixed form (<date>T<time>.<millis>Z), which fails the recorded shape assertion for this member.
Why: a client parsing the field with AWS's documented ISO-8601-with-offset format rejects the value, so layer creation dates cannot be read or sorted.
Fix: the layer-version timestamp serializer used by the ListLayers projection.
Traces: lambda-layer-filter-by-runtime-and-architecture#task#3 (10 steps affected across the layer fixtures).

Note: the open issue 20261103T031416Z-lambda-s3-6-net-new-causes-in-s3.md describes S3-sourced layer causes in the same area but does not carry any of these clusters' replay keys.

## Evidence
Causes 1-3: $.Content.CodeSize expected the published archive's byte size (219 in this recording), got 0; $.Content.CodeSha256 expected present, got absent - on PublishLayerVersion (201 vs 201), GetLayerVersion (200 vs 200) and GetLayerVersionByArn (200 vs 200).
Causes 4-6: $.Content.Location expected a presigned-URL shape, got 'https://awslambda-us-west-2-layers.s3.us-west-2.amazonaws.com/snapshots/<account>/<layer>-<version>' with no query string (5 steps across the three operations).
Cause 7: $.Layers[0] value_mismatch on ListLayers - the LatestMatchingVersion.CreatedDate was expected in AWS's +0000-offset ISO 8601 form and came back as '2026-11-03T16:25:17.085Z' (10 steps affected).

## Steps to Reproduce
1. Replay lambda-lls-s3-published-layer-reference-walk and inspect task#3 (PublishLayerVersion from S3), task#5 (GetLayerVersion), task#6 (GetLayerVersionByArn) and task#7 (PublishLayerVersion from an inline ZipFile).
2. Replay lambda-lls-layer-reference-forms-name-arn-and-version-arn task#3 and task#5 and check Content.Location for SigV4 query parameters.
3. Replay lambda-layer-filter-by-runtime-and-architecture task#3 and compare LatestMatchingVersion.CreatedDate's format.
