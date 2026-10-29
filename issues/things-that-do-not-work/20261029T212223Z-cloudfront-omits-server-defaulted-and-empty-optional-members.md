Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-cache-policy-duplicate-name-error
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-cache-policy-duplicate-name-error cfdeep-request-local-limits cfdeep-edge-attachment-inuse-conflicts cloudfront-content-type-profile-config-lifecycle cfetag-field-level-encryption-config-etag-chain cloudfront-fle-delete-profile-in-use-error cloudfront-fle-profile-duplicate-caller-reference-error cloudfront-fle-list-pagination-params cloudfront-key-group-precondition-and-inuse-errors cloudfront-public-key-duplicate-and-notfound-errors
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateCachePolicy|2xx->2xx|$.CachePolicy.CachePolicyConfig.DefaultTTL
Issue-Key: cloudfront|CreateCachePolicy|2xx->2xx|$.CachePolicy.CachePolicyConfig.MaxTTL
Issue-Key: cloudfront|CreateCachePolicy|2xx->2xx|$.CachePolicy.CachePolicyConfig.ParametersInCacheKeyAndForwardedToOrigin.EnableAcceptEncodingBrotli
Issue-Key: cloudfront|GetFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryptionConfig.QueryArgProfileConfig
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig
Issue-Key: cloudfront|CreateFieldLevelEncryptionProfile|2xx->2xx|$.FieldLevelEncryptionProfile.FieldLevelEncryptionProfileConfig.Comment
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryption.FieldLevelEncryptionConfig.Comment
Issue-Key: cloudfront|GetFieldLevelEncryptionProfile|2xx->2xx|$.FieldLevelEncryptionProfile.FieldLevelEncryptionProfileConfig.Comment
Issue-Key: cloudfront|ListFieldLevelEncryptionConfigs|2xx->2xx|$.FieldLevelEncryptionList.Items.FieldLevelEncryptionSummary.QueryArgProfileConfig
Issue-Key: cloudfront|CreatePublicKey|2xx->2xx|$.PublicKey.PublicKeyConfig.Comment
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront omits server-defaulted and empty optional members from serialized configs

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCachePolicy

## Description
Nine causes, one shared fault: when a request omits an optional config member, AWS still materialises it in the response - either with its documented server-side default or as an empty element - while the emulator leaves the member out of the XML entirely. The config AWS echoes back is therefore not the config the emulator echoes back, on both the create and read paths.

Fix for every cause: the config serializer for that resource type (plus the create-time default in state for the TTL/flag cases) must emit the member rather than skipping it when unset.

Cause 1 of 9: CreateCachePolicy drops DefaultTTL, MaxTTL and EnableAcceptEncodingBrotli.
AWS: when the request omits them, AWS fills in DefaultTTL=86400 and MaxTTL=31536000 (Smithy: CachePolicyConfig.DefaultTTL - 'The default value for this field is 86400 seconds (one day)'; MaxTTL - 'The default value for this field is 31536000 seconds (one year)') and serializes EnableAcceptEncodingBrotli as false.
Emulator: all three members absent from the returned CachePolicyConfig.
Why: a client reading the policy's TTLs gets null instead of the effective values AWS applies, so it cannot tell what caching the policy actually implements.
Fix: CreateCachePolicy state defaults + the cache-policy serializer.
Traces: cloudfront-cache-policy-duplicate-name-error#task#1.

Cause 2 of 9: CreateCachePolicy drops EnableAcceptEncodingBrotli when TTLs were supplied.
AWS: ParametersInCacheKeyAndForwardedToOrigin.EnableAcceptEncodingBrotli is serialized as false when not requested.
Emulator: member absent.
Why/Fix: as cause 1, in the ParametersInCacheKeyAndForwardedToOrigin serializer.
Traces: cfdeep-request-local-limits#task#6.

Cause 3 of 9: GetFieldLevelEncryptionConfig drops QueryArgProfileConfig.
AWS: returns QueryArgProfileConfig with ForwardWhenQueryArgProfileIsUnknown=true and QueryArgProfiles{Quantity=0, empty Items} even when the request did not set one (Smithy: QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown is required, so the structure cannot be half-present).
Emulator: the whole QueryArgProfileConfig element is absent.
Why: the required member of a present structure is missing, so a strict XML/SDK parse of the FLE config fails or yields a null required field.
Fix: the field-level-encryption config serializer.
Traces: cfdeep-edge-attachment-inuse-conflicts#task#13 (also cloudfront-content-type-profile-config-lifecycle#task#4).

Cause 4 of 9: CreateFieldLevelEncryptionConfig drops QueryArgProfileConfig.
AWS/Emulator/Why/Fix: as cause 3, on the create path.
Traces: cfetag-field-level-encryption-config-etag-chain#task#3 (also cloudfront-content-type-profile-config-lifecycle#task#3).

Cause 5 of 9: CreateFieldLevelEncryptionProfile drops an unset Comment.
AWS: serializes Comment as an empty element (Smithy: FieldLevelEncryptionProfileConfig.Comment is optional; AWS echoes it empty).
Emulator: member absent.
Why: round-tripping the profile config through Update* loses the member's presence.
Fix: the FLE profile config serializer.
Traces: cloudfront-fle-delete-profile-in-use-error#setup#2 (also cloudfront-fle-profile-duplicate-caller-reference-error#setup#2).

Cause 6 of 9: CreateFieldLevelEncryptionConfig drops both an unset Comment and QueryArgProfileConfig.
AWS: Comment empty element present, QueryArgProfileConfig present as in cause 3 (Smithy: FieldLevelEncryptionConfig.Comment - 'An optional comment about the configuration').
Emulator: both absent.
Fix: the FLE config serializer.
Traces: cloudfront-fle-delete-profile-in-use-error#setup#3.

Cause 7 of 9: GetFieldLevelEncryptionProfile drops an unset Comment.
AWS/Emulator/Why/Fix: as cause 5, on the read path.
Traces: cloudfront-fle-delete-profile-in-use-error#task#5.

Cause 8 of 9: ListFieldLevelEncryptionConfigs drops QueryArgProfileConfig from each summary.
AWS: each FieldLevelEncryptionSummary carries QueryArgProfileConfig as in cause 3.
Emulator: absent from the summary.
Fix: the FLE summary serializer used by ListFieldLevelEncryptionConfigs.
Traces: cloudfront-fle-list-pagination-params#task#5.

Cause 9 of 9: CreatePublicKey drops an unset Comment.
AWS: serializes PublicKeyConfig.Comment as an empty element (Smithy: PublicKeyConfig.Comment - 'A comment to describe the public key').
Emulator: member absent.
Fix: the public-key config serializer.
Traces: cloudfront-key-group-precondition-and-inuse-errors#task#1 (also cloudfront-public-key-duplicate-and-notfound-errors#task#1).

## Evidence
All nine clusters match on status and diverge only on members reported `missing (expected <value>, got None)`.

1. CreateCachePolicy: DefaultTTL missing (expected 86400), MaxTTL missing (expected 31536000), ParametersInCacheKeyAndForwardedToOrigin.EnableAcceptEncodingBrotli missing (expected false).
2. CreateCachePolicy: EnableAcceptEncodingBrotli missing (expected false).
3. GetFieldLevelEncryptionConfig: $.FieldLevelEncryptionConfig.QueryArgProfileConfig missing (expected {ForwardWhenQueryArgProfileIsUnknown: true, QueryArgProfiles: {Quantity: 0, Items: ''}}).
4. CreateFieldLevelEncryptionConfig: $.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig missing, same expected value.
5. CreateFieldLevelEncryptionProfile: ...FieldLevelEncryptionProfileConfig.Comment missing (expected '').
6. CreateFieldLevelEncryptionConfig: ...FieldLevelEncryptionConfig.Comment missing (expected '') and QueryArgProfileConfig missing.
7. GetFieldLevelEncryptionProfile: ...FieldLevelEncryptionProfileConfig.Comment missing (expected '').
8. ListFieldLevelEncryptionConfigs: ...FieldLevelEncryptionSummary.QueryArgProfileConfig missing.
9. CreatePublicKey: $.PublicKey.PublicKeyConfig.Comment missing (expected '').

The expected values here are AWS's documented defaults (86400, 31536000, false, ForwardWhenQueryArgProfileIsUnknown=true, Quantity=0) and empty elements - not values taken from any one recording.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Cause 1/2: CreateCachePolicy omitting DefaultTTL/MaxTTL/EnableAcceptEncodingBrotli; the response CachePolicyConfig has no such members.
3. Causes 3/4/6/8: create an FLE config without QueryArgProfileConfig (and without Comment) and read it back via Get/List; the elements are absent.
4. Causes 5/7: create an FLE profile without Comment and read it back. Cause 9: CreatePublicKey without Comment.
5. Representative step: fixture cloudfront-cache-policy-duplicate-name-error, step task#1 (CreateCachePolicy).
