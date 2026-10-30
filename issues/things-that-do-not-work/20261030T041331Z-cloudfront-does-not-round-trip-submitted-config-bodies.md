Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-field-level-encryption-profile-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-field-level-encryption-profile-lifecycle cloudfront-content-type-profile-config-lifecycle cloudfront-invalidation-create-get-list cloudfront-invalidation-lifecycle cloudfront-field-level-encryption-duplicate-and-ifmatch-errors cloudfront-field-level-encryption-lifecycle cloudfront-query-arg-profile-config-lifecycle
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateFieldLevelEncryptionProfile|2xx->2xx|$.FieldLevelEncryptionProfile.FieldLevelEncryptionProfileConfig.EncryptionEntities.Items.EncryptionEntity.FieldPatterns.Items.FieldPattern[*]
Issue-Key: cloudfront|ListFieldLevelEncryptionProfiles|2xx->2xx|$.FieldLevelEncryptionProfileList.Items.FieldLevelEncryptionProfileSummary[Name="sim2real-probe-cloudfront-fleprofile-aee4bdb8"].EncryptionEntities.Items.EncryptionEntity.FieldPatterns.Items.FieldPattern[*]
Issue-Key: cloudfront|GetFieldLevelEncryptionProfile|2xx->2xx|$.FieldLevelEncryptionProfile.FieldLevelEncryptionProfileConfig.EncryptionEntities.Items.EncryptionEntity.FieldPatterns.Items.FieldPattern[*]
Issue-Key: cloudfront|CreateInvalidation|2xx->2xx|$.Invalidation.InvalidationBatch.Paths.Items.Path[*]
Issue-Key: cloudfront|GetInvalidation|2xx->2xx|$.Invalidation.InvalidationBatch.Paths.Items.Path[*]
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryption.FieldLevelEncryptionConfig.ContentTypeProfileConfig
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Key: cloudfront|GetFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryptionConfig.ContentTypeProfileConfig
Issue-Key: cloudfront|GetFieldLevelEncryptionConfig|2xx->2xx|$.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Key: cloudfront|ListFieldLevelEncryptionConfigs|2xx->2xx|$.FieldLevelEncryptionList.Items.FieldLevelEncryptionSummary.ContentTypeProfileConfig
Issue-Key: cloudfront|ListFieldLevelEncryptionConfigs|2xx->2xx|$.FieldLevelEncryptionList.Items.FieldLevelEncryptionSummary.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown
Issue-Commit: e7818f8c3a6adfb142c95bd82ae3863b54254e49

# CloudFront does not round-trip submitted config bodies: list item order reversed and optional sub-configs dropped

> _Note: this issue was identified against the code base at commit `e7818f8c3a6adfb142c95bd82ae3863b54254e49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFieldLevelEncryptionProfile

## Description
This bundle carries EIGHT distinct causes of one shared fault: the CloudFront emulator does not round-trip the configuration body a caller submitted. Two failure modes recur across create, get and list for the same resources: (a) the member order of an XML list is not preserved — the emulator serves the list members in reversed submission order; (b) an optional sub-structure of the submitted config (ContentTypeProfileConfig) is dropped entirely and a required boolean inside another sub-structure (QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown) is replaced by a server-chosen value rather than the submitted one. A fixer must close all EIGHT causes below before submitting.

=== Cause 1 of 8: CreateFieldLevelEncryptionProfile reverses FieldPatterns.Items ===
AWS: Smithy: CreateFieldLevelEncryptionProfile -> FieldLevelEncryptionProfile (httpPayload); FieldLevelEncryptionProfileConfig.EncryptionEntities.Items.EncryptionEntity.FieldPatterns.Items targets FieldPatternList, a `list` of `string` with member xmlName FieldPattern. A Smithy list is an ORDERED sequence, so the response serializes the members in the order the request supplied them; AWS echoes the submitted profile config verbatim in the 201 response. Status 201.
Emulator: 201 with the correct members but in reversed order — the recording's first FieldPattern comes back second and vice versa (replay: FieldPattern[0] expected the first submitted pattern, got the second; FieldPattern[1] expected the second, got the first).
Why: a client that reads back its own profile to confirm what it created, or that renders/diffs FieldPatterns positionally (index-addressed config management, idempotent re-apply, template drift detection), sees a config it never submitted and re-writes it, flapping forever. Ordering is also semantically load-bearing for pattern matching.
Fix: the CloudFront FieldLevelEncryptionProfile create handler's state write and/or the FieldPatternList XML serializer — whichever reverses the sequence (a push-front/prepend accumulation, or a reversed iterator) must preserve submission order end to end.
Traces: cloudfront-field-level-encryption-profile-lifecycle#task#2.

=== Cause 2 of 8: ListFieldLevelEncryptionProfiles reverses FieldPatterns.Items ===
AWS: Smithy: ListFieldLevelEncryptionProfiles -> FieldLevelEncryptionProfileList.Items.FieldLevelEncryptionProfileSummary[].EncryptionEntities.Items.EncryptionEntity.FieldPatterns.Items (FieldPatternList, ordered). The summary reproduces the stored profile's field patterns in submission order. Status 200.
Emulator: 200, same two patterns in reversed order as in Cause 1.
Why: the same positional read-back breakage, now on the list path, so even a client that never re-fetches the single profile is affected.
Fix: same component as Cause 1 — the stored order is already wrong, so the list projection inherits it; fix the write path and the list serializer's ordering together.
Traces: cloudfront-field-level-encryption-profile-lifecycle#task#4.

=== Cause 3 of 8: GetFieldLevelEncryptionProfile reverses FieldPatterns.Items ===
AWS: Smithy: GetFieldLevelEncryptionProfile -> FieldLevelEncryptionProfile with the same FieldPatternList ordering contract. Status 200.
Emulator: 200 with the two submitted patterns transposed (replay: FieldPattern[0]/[1] each got the other's value).
Why: Get is the canonical read-back; a config-management client comparing desired vs actual state never converges.
Fix: same ordering defect in the profile state/serializer, confirmed on the Get path too.
Traces: cloudfront-content-type-profile-config-lifecycle#teardown#7, cloudfront-field-level-encryption-profile-lifecycle#task#3.

=== Cause 4 of 8: CreateInvalidation reverses InvalidationBatch.Paths.Items ===
AWS: Smithy: CreateInvalidation input InvalidationBatch (httpPayload, required) -> output Invalidation (httpPayload), http POST /2020-05-31/distribution/{DistributionId}/invalidation, code 201. Paths.Items targets PathList (ordered list) and Paths.Quantity is required. AWS echoes the submitted InvalidationBatch, paths in submitted order.
Emulator: 201 with the submitted paths reversed — the first path submitted is serialized second (replay: Path[0] expected the first submitted path, got the second; Path[1] the converse).
Why: invalidation batches are commonly built and audited positionally, and callers diff the echoed batch against the one they sent to confirm the request was accepted intact; a reversed echo reads as the service having mangled the batch.
Fix: the CreateInvalidation handler's InvalidationBatch/PathList persistence and its XML serializer — preserve the submitted sequence.
Traces: cloudfront-invalidation-create-get-list#task#2, cloudfront-invalidation-lifecycle#task#2.

=== Cause 5 of 8: GetInvalidation reverses InvalidationBatch.Paths.Items ===
AWS: Smithy: GetInvalidation -> Invalidation containing the InvalidationBatch as submitted, Paths.Items in order. Status 200.
Emulator: 200 with the two paths transposed, same as Cause 4.
Why: the stored invalidation is what later audits and reconciliations read; it misreports which objects were invalidated in which order.
Fix: same ordering defect, on the invalidation read path/state.
Traces: cloudfront-invalidation-create-get-list#task#3, cloudfront-invalidation-lifecycle#task#3.

=== Cause 6 of 8: CreateFieldLevelEncryptionConfig drops ContentTypeProfileConfig and ignores the submitted ForwardWhenQueryArgProfileIsUnknown ===
AWS: Smithy: CreateFieldLevelEncryptionConfig input FieldLevelEncryptionConfig (httpPayload, required) -> output FieldLevelEncryption (httpPayload), http POST /2020-05-31/field-level-encryption, code 201. FieldLevelEncryptionConfig has members CallerReference (required), Comment, QueryArgProfileConfig and ContentTypeProfileConfig. QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown is a REQUIRED boolean, and ContentTypeProfileConfig.ForwardWhenContentTypeIsUnknown is a REQUIRED boolean with ContentTypeProfiles (Quantity + Items/ContentTypeProfile{Format,ProfileId,ContentType}). AWS echoes BOTH sub-configs exactly as submitted, including a submitted `false` boolean and a ContentTypeProfiles list with Quantity 1.
Emulator: 201 but the response payload omits FieldLevelEncryption.FieldLevelEncryptionConfig.ContentTypeProfileConfig entirely (expected the whole submitted structure, got nothing), and serializes QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown as 'true' when the request submitted 'false'.
Why: a client that submits content-type-profile mapping gets a 201 that silently says the mapping does not exist, and gets the inverse of the forwarding flag it chose — so field-level encryption is configured differently from what the caller asked for, and the "forward when the profile is unknown" decision (fail-open vs fail-closed) is flipped. This is a security-relevant default inversion, not a cosmetic omission.
Fix: the CreateFieldLevelEncryptionConfig handler's request deserialization/state write — it must retain the submitted ContentTypeProfileConfig and the submitted boolean instead of substituting a server default — and the FieldLevelEncryptionConfig serializer must emit ContentTypeProfileConfig whenever it is set. Related but distinct open issue, which does NOT cover these steps: 20261029T212223Z-cloudfront-omits-server-defaulted-and-empty-optional-members.md.
Traces: cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#4, cloudfront-field-level-encryption-lifecycle#task#7, cloudfront-query-arg-profile-config-lifecycle#task#4.

=== Cause 7 of 8: GetFieldLevelEncryptionConfig drops ContentTypeProfileConfig and reports the wrong ForwardWhenQueryArgProfileIsUnknown ===
AWS: Smithy: GetFieldLevelEncryptionConfig -> FieldLevelEncryptionConfig (httpPayload), status 200, reproducing the stored config including ContentTypeProfileConfig and the stored required boolean.
Emulator: 200 with $.FieldLevelEncryptionConfig.ContentTypeProfileConfig missing and ForwardWhenQueryArgProfileIsUnknown 'true' where the stored config was created with 'false'.
Why: read-back of the FLE config misrepresents the active encryption policy; a client cannot discover its own content-type profile mapping at all, and sees the forwarding flag inverted.
Fix: same component as Cause 6, on the Get path (the state is already missing the sub-config, so the write fix is the primary one; the serializer must also stop suppressing it).
Traces: cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#6, cloudfront-field-level-encryption-lifecycle#task#8, cloudfront-query-arg-profile-config-lifecycle#task#5.

=== Cause 8 of 8: ListFieldLevelEncryptionConfigs summaries drop ContentTypeProfileConfig and report the wrong ForwardWhenQueryArgProfileIsUnknown ===
AWS: Smithy: ListFieldLevelEncryptionConfigs -> FieldLevelEncryptionList.Items.FieldLevelEncryptionSummary[], each summary carrying QueryArgProfileConfig and ContentTypeProfileConfig as stored. Status 200.
Emulator: 200 with FieldLevelEncryptionSummary.ContentTypeProfileConfig missing and the summary's ForwardWhenQueryArgProfileIsUnknown 'true' instead of the stored 'false'.
Why: enumeration — the path used by inventory and compliance tooling — reports every FLE config as having no content-type profile mapping and the opposite forwarding behavior.
Fix: same component as Cause 6; the list projection must carry both members through.
Traces: cloudfront-field-level-encryption-lifecycle#task#9, cloudfront-query-arg-profile-config-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, 1143 steps, 761 passed, score 66.6%). Eight net-new cloudfront divergence clusters, none owned by an open issue and none blocked by a stub:

1) CreateFieldLevelEncryptionProfile, 201->201, cloudfront-field-level-encryption-profile-lifecycle#task#2:
   $...FieldPatterns.Items.FieldPattern[0]: expected 'ssn', got 'credit-card'
   $...FieldPatterns.Items.FieldPattern[1]: expected 'credit-card', got 'ssn'
   (the two recorded values are example data; the requirement is that the order equals the submitted order)
2) ListFieldLevelEncryptionProfiles, 200->200, cloudfront-field-level-encryption-profile-lifecycle#task#4: same two FieldPattern entries transposed in the summary.
3) GetFieldLevelEncryptionProfile, 200->200, cloudfront-content-type-profile-config-lifecycle#teardown#7 (also cloudfront-field-level-encryption-profile-lifecycle#task#3): expected 'dob','ssn', got 'ssn','dob'.
4) CreateInvalidation, 201->201, cloudfront-invalidation-create-get-list#task#2 (also cloudfront-invalidation-lifecycle#task#2): $.Invalidation.InvalidationBatch.Paths.Items.Path[0] expected '/images/*', got '/index.html'; Path[1] expected '/index.html', got '/images/*'.
5) GetInvalidation, 200->200, cloudfront-invalidation-create-get-list#task#3 (also cloudfront-invalidation-lifecycle#task#3): identical transposition.
6) CreateFieldLevelEncryptionConfig, 201->201, cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#4 (also cloudfront-field-level-encryption-lifecycle#task#7, cloudfront-query-arg-profile-config-lifecycle#task#4):
   $.FieldLevelEncryption.FieldLevelEncryptionConfig.QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown: expected 'false', got 'true'
   $.FieldLevelEncryption.FieldLevelEncryptionConfig.ContentTypeProfileConfig: missing (expected the submitted structure {ForwardWhenContentTypeIsUnknown:'false', ContentTypeProfiles:{Quantity:'1', Items:{ContentTypeProfile:{Format:'URLEncoded', ProfileId:<submitted profile id>, ContentType:'application/x-www-form-urlencoded'}}}}, got None)
7) GetFieldLevelEncryptionConfig, 200->200, cloudfront-field-level-encryption-duplicate-and-ifmatch-errors#task#6 (also cloudfront-field-level-encryption-lifecycle#task#8, cloudfront-query-arg-profile-config-lifecycle#task#5): $.FieldLevelEncryptionConfig.ContentTypeProfileConfig missing; ForwardWhenQueryArgProfileIsUnknown expected 'false', got 'true'.
8) ListFieldLevelEncryptionConfigs, 200->200, cloudfront-field-level-encryption-lifecycle#task#9 (also cloudfront-query-arg-profile-config-lifecycle#task#6): $.FieldLevelEncryptionList.Items.FieldLevelEncryptionSummary.ContentTypeProfileConfig missing; ForwardWhenQueryArgProfileIsUnknown expected 'false', got 'true'.

Model references used: Smithy CreateFieldLevelEncryptionConfig (input FieldLevelEncryptionConfig httpPayload required, POST /2020-05-31/field-level-encryption, code 201); FieldLevelEncryptionConfig members CallerReference(required)/Comment/QueryArgProfileConfig/ContentTypeProfileConfig; QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown boolean required; ContentTypeProfileConfig.ForwardWhenContentTypeIsUnknown boolean required; FieldPatternList = list<string> member xmlName FieldPattern; Paths{Quantity required, Items: PathList}; CreateInvalidation POST /2020-05-31/distribution/{DistributionId}/invalidation code 201.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay trace set simparity-wire-aws-v1.
3. Ordering causes: replay cloudfront-field-level-encryption-profile-lifecycle (task#2, task#3, task#4) and cloudfront-invalidation-create-get-list (task#2, task#3) — i.e. CreateFieldLevelEncryptionProfile with an EncryptionEntity whose FieldPatterns.Items has two distinct patterns, then Get and List it; and CreateInvalidation with two distinct Paths.Items entries, then GetInvalidation. Compare the serialized member order with the submitted order: the emulator serves them reversed.
4. Dropped sub-config causes: replay cloudfront-field-level-encryption-duplicate-and-ifmatch-errors (task#4, task#6) and cloudfront-field-level-encryption-lifecycle#task#9 — i.e. CreateFieldLevelEncryptionConfig with a FieldLevelEncryptionConfig carrying BOTH QueryArgProfileConfig (ForwardWhenQueryArgProfileIsUnknown=false) and ContentTypeProfileConfig (one ContentTypeProfile), then GetFieldLevelEncryptionConfig and ListFieldLevelEncryptionConfigs. All three responses omit ContentTypeProfileConfig and report ForwardWhenQueryArgProfileIsUnknown=true.
5. Also replay cloudfront-content-type-profile-config-lifecycle#teardown#7 and cloudfront-query-arg-profile-config-lifecycle (task#4, task#5, task#6) to confirm the same two modes on those fixtures.
