Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-invalidation-create-get-list
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-invalidation-create-get-list cloudfront-invalidation-lifecycle cloudfront-content-type-profile-config-lifecycle cloudfront-field-level-encryption-profile-lifecycle cloudfront-field-level-encryption-duplicate-and-ifmatch-errors cloudfront-field-level-encryption-lifecycle cloudfront-query-arg-profile-config-lifecycle
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateInvalidation|2xx->2xx|$.Invalidation.InvalidationBatch.Paths.Items.Path[*]
Issue-Key: cloudfront|GetInvalidation|2xx->2xx|$.Invalidation.InvalidationBatch.Paths.Items.Path[*]
Issue-Key: cloudfront|GetFieldLevelEncryptionProfile|2xx->2xx|$.FieldLevelEncryptionProfile.FieldLevelEncryptionProfileConfig.EncryptionEntities.Items.EncryptionEntity.FieldPatterns.Items.FieldPattern[*]
Issue-Key: cloudfront|CreateFieldLevelEncryptionProfile|2xx->2xx|$.FieldLevelEncryptionProfile.FieldLevelEncryptionProfileConfig.EncryptionEntities.Items.EncryptionEntity.FieldPatterns.Items.FieldPattern[*]
Issue-Key: cloudfront|ListFieldLevelEncryptionProfiles|2xx->2xx|$.FieldLevelEncryptionProfileList.Items.FieldLevelEncryptionProfileSummary[Name="sim2real-probe-cloudfront-fleprofile-aee4bdb8"].EncryptionEntities.Items.EncryptionEntity.FieldPatterns.Items.FieldPattern[*]
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryption.FieldLevelEncryptionConfig.ContentTypeProfileConfig
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Key: cloudfront|GetFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryptionConfig.ContentTypeProfileConfig
Issue-Key: cloudfront|GetFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Key: cloudfront|ListFieldLevelEncryptionConfigs|2xx->2xx|$.FieldLevelEncryptionList.Items.FieldLevelEncryptionSummary.ContentTypeProfileConfig
Issue-Key: cloudfront|ListFieldLevelEncryptionConfigs|2xx->2xx|$.FieldLevelEncryptionList.Items.FieldLevelEncryptionSummary.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront does not preserve request-supplied list order or FLE config values in responses

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvalidation

## Description
Eight causes, one shared fault: the values a request supplies are not reflected faithfully in what the emulator stores and serves back. Two symptoms: ordered list members come back in the opposite order from the one AWS returns, and an FLE config's supplied profile settings are replaced by defaults.

Causes 1-5 (list member order reversed).
AWS: these element lists are returned in a stable, recorded order - for every one of the five clusters the emulator's sequence is the exact REVERSE of AWS's, over a two-element list, on both the create and the read/list paths. The order is therefore part of the response, not an incidental detail.
Emulator: emits the members in reverse order.
Why: FieldPatterns order determines which pattern a field matches first, and an invalidation's path order is what a client diffs against its request; a client comparing its request to the response concludes the service reordered its input.
Fix: the serializers/state for InvalidationBatch.Paths.Items and EncryptionEntity.FieldPatterns.Items - preserve insertion order instead of reversing it.
Cause 1 of 8: CreateInvalidation - Paths.Items.Path reversed. Traces cloudfront-invalidation-create-get-list#task#2 (also cloudfront-invalidation-lifecycle#task#2).
Cause 2 of 8: GetInvalidation - same reversal on read. Traces cloudfront-invalidation-create-get-list#task#3 (also cloudfront-invalidation-lifecycle#task#3).
Cause 3 of 8: GetFieldLevelEncryptionProfile - FieldPatterns.Items.FieldPattern reversed. Traces cloudfront-content-type-profile-config-lifecycle#teardown#7 (also cloudfront-field-level-encryption-profile-lifecycle#task#3).
Cause 4 of 8: CreateFieldLevelEncryptionProfile - FieldPatterns reversed in the create response. Traces cloudfront-field-level-encryption-profile-lifecycle#task#2.
Cause 5 of 8: ListFieldLevelEncryptionProfiles - FieldPatterns reversed in the summary. Traces cloudfront-field-level-encryption-profile-lifecycle#task#4.

Causes 6-8 (FLE config values replaced by defaults).
AWS: the config is stored as supplied - ForwardWhenQueryArgProfileIsUnknown keeps the requested value (false here), and the supplied ContentTypeProfileConfig (ForwardWhenContentTypeIsUnknown plus its ContentTypeProfiles items) is returned on every path (Smithy: FieldLevelEncryptionConfig.ContentTypeProfileConfig and QueryArgProfileConfig; QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown is required).
Emulator: ForwardWhenQueryArgProfileIsUnknown comes back as true regardless of the request, and ContentTypeProfileConfig is absent entirely.
Why: the field-level-encryption behaviour the client configured (do NOT forward on an unknown profile; use this content-type profile) is silently inverted and dropped, and the client cannot see that it was.
Fix: CreateFieldLevelEncryptionConfig's handler/state must persist both members, and the FLE config/summary serializers must emit ContentTypeProfileConfig.
Cause 6 of 8: CreateFieldLevelEncryptionConfig - traces cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#4 (also cloudfront-field-level-encryption-lifecycle#task#7, cloudfront-query-arg-profile-config-lifecycle#task#4).
Cause 7 of 8: GetFieldLevelEncryptionConfig - traces cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#6 (also cloudfront-field-level-encryption-lifecycle#task#8, cloudfront-query-arg-profile-config-lifecycle#task#5).
Cause 8 of 8: ListFieldLevelEncryptionConfigs - traces cloudfront-field-level-encryption-lifecycle#task#9 (also cloudfront-query-arg-profile-config-lifecycle#task#6).

## Evidence
Causes 1-5 (status matches; element order reversed):
1. CreateInvalidation: Paths.Items.Path[0] expected '/images/*' got '/index.html'; Path[1] expected '/index.html' got '/images/*'.
2. GetInvalidation: identical reversal.
3. GetFieldLevelEncryptionProfile: FieldPattern[0] expected 'dob' got 'ssn'; FieldPattern[1] expected 'ssn' got 'dob'.
4. CreateFieldLevelEncryptionProfile: FieldPattern[0] expected 'ssn' got 'credit-card'; FieldPattern[1] expected 'credit-card' got 'ssn'.
5. ListFieldLevelEncryptionProfiles: same reversal inside the FieldLevelEncryptionProfileSummary.
In all five the actual sequence is the exact reverse of the expected one, which is why this is an ordering defect and not a value defect; the particular path/pattern strings are the probes' own inputs.

Causes 6-8 (status matches; values diverge):
6. CreateFieldLevelEncryptionConfig: QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown expected 'false', got 'true'; ContentTypeProfileConfig missing (expected ForwardWhenContentTypeIsUnknown 'false' with one ContentTypeProfile of Format 'URLEncoded' and ContentType 'application/x-www-form-urlencoded').
7. GetFieldLevelEncryptionConfig: same two divergences on the read path.
8. ListFieldLevelEncryptionConfigs: same two divergences inside FieldLevelEncryptionSummary.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Causes 1-2: CreateInvalidation with Paths.Items = ['/images/*', '/index.html'], then GetInvalidation; the response lists them reversed.
3. Causes 3-5: CreateFieldLevelEncryptionProfile with two FieldPatterns in a known order, then Get and List; the order is reversed on all three paths.
4. Causes 6-8: CreateFieldLevelEncryptionConfig with ForwardWhenQueryArgProfileIsUnknown=false and a ContentTypeProfileConfig, then Get and List; the flag reads back true and ContentTypeProfileConfig is gone.
5. Representative step: fixture cloudfront-invalidation-create-get-list, step task#2 (CreateInvalidation).
