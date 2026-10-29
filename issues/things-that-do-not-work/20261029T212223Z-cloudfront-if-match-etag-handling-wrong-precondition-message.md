Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-cache-policy-full-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-cache-policy-full-lifecycle cloudfront-cache-policy-lifecycle cfetag-connection-group-etag-chain cloudfront-origin-request-policy-lifecycle cfetag-field-level-encryption-config-etag-chain cloudfront-cloudfront-function-get-workflow cloudfront-distribution-create-invalidate-monitor-webacl cloudfront-distribution-webacl-monitoring-lifecycle cloudfront-web-acl-association-error-path cloudfront-web-acl-association-workflow cloudfront-distribution-tenant-webacl-association cloudfront-distribution-webacl-association cloudfront-field-level-encryption-config-workflow cloudfront-fle-config-lifecycle-with-profile-reference cloudfront-fle-config-not-found-and-invalid-ifmatch-errors
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|DeleteCachePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteConnectionGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteOriginRequestPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|UpdateFieldLevelEncryptionConfig|2xx->4xx|$.FieldLevelEncryption
Issue-Key: cloudfront|UpdateFieldLevelEncryptionConfig|2xx->4xx|$headers.etag
Issue-Key: cloudfront|UpdateFieldLevelEncryptionConfig|2xx->4xx|status
Issue-Key: cloudfront|DeleteFunction|2xx->4xx|status
Issue-Key: cloudfront|DisassociateDistributionWebACL|2xx->4xx|$.DisassociateDistributionWebACLResult
Issue-Key: cloudfront|DisassociateDistributionWebACL|2xx->4xx|$headers.etag
Issue-Key: cloudfront|DisassociateDistributionWebACL|2xx->4xx|status
Issue-Key: cloudfront|DisassociateDistributionTenantWebACL|2xx->4xx|$.DisassociateDistributionTenantWebACLResult
Issue-Key: cloudfront|DisassociateDistributionTenantWebACL|2xx->4xx|$headers.etag
Issue-Key: cloudfront|DisassociateDistributionTenantWebACL|2xx->4xx|status
Issue-Key: cloudfront|DeleteDistribution|2xx->4xx|status
Issue-Key: cloudfront|DeleteFieldLevelEncryptionProfile|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|DeleteFieldLevelEncryptionProfile|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteFieldLevelEncryptionProfile|4xx->4xx|status
Issue-Key: cloudfront|DeleteFieldLevelEncryptionProfile|2xx->4xx|status
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront If-Match/ETag handling: wrong precondition message and spurious 412 on valid ETags

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCachePolicy

## Description
Ten causes, one shared fault: the emulator's conditional-request layer is wrong in both directions. When a precondition genuinely fails it emits a generic HTTP-flavoured sentence instead of AWS's PreconditionFailed message, and when the caller supplies the CURRENT ETag it still answers 412, so correct conditional updates and deletes are impossible.

Fix for every cause: the shared If-Match/ETag comparison used by the mutating CloudFront handlers - it must compare against the entity's current ETag (the value last returned in the ETag response header) and, on a real mismatch, raise the modelled PreconditionFailed with AWS's wording.

Causes 1-3 (wrong PreconditionFailed message).
AWS: 412 `PreconditionFailed` with the fixed message `The request failed because it didn't meet the preconditions for this operation.` (Smithy: PreconditionFailed -> 412 on DeleteCachePolicy, DeleteOriginRequestPolicy and the connection-group/FLE mutators). No variable part.
Emulator: 412 with `The request failed because it didn't meet the preconditions in one or more request-header fields.` - an HTTP-generic sentence AWS does not use.
Why: clients that match AWS's sentence (or surface it to users) get unrecognised text for every stale-ETag retry.
Cause 1 of 10: DeleteCachePolicy - traces cloudfront-cache-policy-full-lifecycle#task#7 (also cloudfront-cache-policy-lifecycle#task#7).
Cause 2 of 10: DeleteConnectionGroup - AWS's message here is `Your request is missing requirements for this operation. Update your request and try again.`; emulator sends the same generic header sentence. Traces cfetag-connection-group-etag-chain#task#5.
Cause 3 of 10: DeleteOriginRequestPolicy - traces cloudfront-origin-request-policy-lifecycle#task#8.

Causes 4-10 (412 returned for a valid, current ETag).
AWS: the operation succeeds - 200 with its result body and a fresh ETag header, or 204 for a delete.
Emulator: 412 PreconditionFailed, no result body and no ETag header, so the caller cannot recover: the only ETag it has is the one the emulator just rejected.
Why: every ETag-gated lifecycle is blocked. A client cannot update or delete these resources at all, and teardown of a correctly-created resource is impossible.
Cause 4 of 10: UpdateFieldLevelEncryptionConfig with the current ETag - AWS 200 with the updated FieldLevelEncryption and a new ETag; emulator 412 with neither. Traces cfetag-field-level-encryption-config-etag-chain#task#6.
Cause 5 of 10: DeleteFunction with the current ETag - AWS 204; emulator 412. Traces cloudfront-cloudfront-function-get-workflow#teardown#6.
Cause 6 of 10: DisassociateDistributionWebACL with the current ETag - AWS 200 with DisassociateDistributionWebACLResult.Id and a fresh ETag; emulator 412. Traces cloudfront-distribution-create-invalidate-monitor-webacl#task#13 (also cloudfront-distribution-webacl-monitoring-lifecycle#task#9, cloudfront-web-acl-association-error-path#task#4, cloudfront-web-acl-association-workflow#task#4).
Cause 7 of 10: DisassociateDistributionTenantWebACL with the current ETag - AWS 200 with DisassociateDistributionTenantWebACLResult.Id and a fresh ETag; emulator 412. Traces cloudfront-distribution-tenant-webacl-association#task#4.
Cause 8 of 10: DeleteDistribution with the current ETag on a disabled distribution - AWS 204; emulator 412. Traces cloudfront-distribution-webacl-association#teardown#6.
Cause 9 of 10: DeleteFieldLevelEncryptionProfile on a profile still referenced by an FLE config - AWS answers the *resource* conflict first: 409 `FieldLevelEncryptionProfileInUse` with `The field level encryption profile is currently associated with a field level encryption config. Please disassociate the profile before deleting.`; the emulator answers 412 PreconditionFailed with its generic header sentence, so the in-use reason is never reported. Traces cloudfront-field-level-encryption-config-workflow#task#6.
Cause 10 of 10: DeleteFieldLevelEncryptionProfile with the current ETag after the config is gone - AWS 204; emulator 412. Traces cloudfront-field-level-encryption-config-workflow#teardown#8 (also cloudfront-fle-config-lifecycle-with-profile-reference#teardown#7, cloudfront-fle-config-not-found-and-invalid-ifmatch-errors#teardown#7).

## Evidence
Causes 1-3 (status matches, message diverges):
- DeleteCachePolicy 412->412: expected "The request failed because it didn't meet the preconditions for this operation.", got "The request failed because it didn't meet the preconditions in one or more request-header fields."
- DeleteConnectionGroup 412->412: expected 'Your request is missing requirements for this operation. Update your request and try again.', got the same generic header sentence.
- DeleteOriginRequestPolicy 412->412: identical expected/actual pair as DeleteCachePolicy.

Causes 4-10 (success expected, 412 returned):
- UpdateFieldLevelEncryptionConfig: expected 200 with $.FieldLevelEncryption and an ETag header; got 412, both missing.
- DeleteFunction: expected 204, got 412.
- DisassociateDistributionWebACL: expected 200 with $.DisassociateDistributionWebACLResult.Id and an ETag header; got 412, both missing.
- DisassociateDistributionTenantWebACL: expected 200 with $.DisassociateDistributionTenantWebACLResult.Id and an ETag header; got 412.
- DeleteDistribution: expected 204, got 412.
- DeleteFieldLevelEncryptionProfile (in use): expected 409 FieldLevelEncryptionProfileInUse 'The field level encryption profile is currently associated with a field level encryption config. Please disassociate the profile before deleting.', got 412 PreconditionFailed with the generic header sentence.
- DeleteFieldLevelEncryptionProfile (teardown): expected 204, got 412 (3 affected steps).

The ETag values and resource ids in these steps are per-run; the requirement is that an If-Match carrying the ETag the emulator itself last returned is accepted.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Causes 1-3: call DeleteCachePolicy / DeleteConnectionGroup / DeleteOriginRequestPolicy with a deliberately stale If-Match and read Error.Message.
3. Causes 4-10: create the resource, read its ETag from the create/get response header, then call the mutator (UpdateFieldLevelEncryptionConfig, DeleteFunction, DisassociateDistributionWebACL, DisassociateDistributionTenantWebACL, DeleteDistribution, DeleteFieldLevelEncryptionProfile) with exactly that ETag as If-Match.
4. Each returns 412 with no body and no ETag header instead of the modelled success (or, in cause 9, instead of the 409 in-use refusal).
5. Representative step: fixture cloudfront-cache-policy-full-lifecycle, step task#7 (DeleteCachePolicy).
