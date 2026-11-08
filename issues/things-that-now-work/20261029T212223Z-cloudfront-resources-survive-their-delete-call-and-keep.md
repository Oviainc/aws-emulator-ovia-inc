Issue-Source: trace-sweep-divergence
Issue-Trace: cfetag-public-key-etag-chain
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfetag-public-key-etag-chain cloudfront-content-type-profile-config-lifecycle cloudfront-field-level-encryption-profile-lifecycle cloudfront-fle-config-lifecycle-with-profile-reference cloudfront-key-group-lifecycle-with-members cloudfront-public-key-and-key-group-lifecycle cloudfront-public-key-lifecycle cloudfront-public-key-lifecycle-and-errors cloudfront-query-arg-profile-config-lifecycle cfetag-field-level-encryption-config-etag-chain cfetag-key-group-etag-chain cloudfront-key-group-lifecycle cfetag-response-headers-policy-etag-chain cloudfront-response-headers-policy-lifecycle cloudfront-cloudfront-function-get-workflow cloudfront-key-group-precondition-and-inuse-errors cloudfront-public-key-basic-lifecycle cloudfront-public-key-error-paths cloudfront-response-headers-policy-errors
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetPublicKey|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|GetPublicKey|4xx->2xx|status
Issue-Key: cloudfront|GetFieldLevelEncryption|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|GetFieldLevelEncryption|4xx->2xx|status
Issue-Key: cloudfront|GetKeyGroup|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|GetKeyGroup|4xx->2xx|status
Issue-Key: cloudfront|GetResponseHeadersPolicy|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|GetResponseHeadersPolicy|4xx->2xx|status
Issue-Key: cloudfront|GetFunction|4xx->2xx|$
Issue-Key: cloudfront|GetFunction|4xx->2xx|$headers.content-type
Issue-Key: cloudfront|GetFunction|4xx->2xx|status
Issue-Key: cloudfront|GetFieldLevelEncryptionProfile|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|GetFieldLevelEncryptionProfile|4xx->2xx|status
Issue-Key: cloudfront|GetKeyGroup|4xx->2xx|$
Issue-Key: cloudfront|GetPublicKey|4xx->2xx|$
Issue-Key: cloudfront|GetResponseHeadersPolicy|4xx->2xx|$
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront resources survive their Delete call and keep answering Get with 200

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPublicKey

## Description
Ten causes, one shared fault: after a successful Delete*, the resource is still present in emulator state, so the following Get* answers 200 with the full resource instead of the modelled NotFound error. Two of the causes are polled to exhaustion (29 and 56 attempts), confirming this is not eventual consistency - the delete never removes the entity.

AWS contract shared by all ten: once Delete* succeeds (204), the corresponding Get* returns the operation's modelled 404 for that resource type, with the fixed message for that error class. Fix for every cause: the Delete* handler must remove the entity from state (or the Get* lookup must stop resolving tombstoned entities) - this is a state-layer defect, not a serializer one.

Cause 1 of 10: GetPublicKey after DeletePublicKey.
AWS: 404 `NoSuchPublicKey` with the fixed message `The specified CloudFront public key does not exist.` (Smithy: GetPublicKey errors = AccessDenied 403, NoSuchPublicKey 404).
Emulator: 200 with the full PublicKey body.
Why: a client that deletes a key and then confirms its absence sees it still present; key-rotation flows never converge.
Fix: DeletePublicKey state removal.
Traces: cfetag-public-key-etag-chain#task#8 (also cloudfront-content-type-profile-config-lifecycle#teardown#12, cloudfront-field-level-encryption-profile-lifecycle#teardown#8, cloudfront-fle-config-lifecycle-with-profile-reference#teardown#11, cloudfront-key-group-lifecycle-with-members#teardown#11, cloudfront-public-key-and-key-group-lifecycle#task#17, cloudfront-public-key-lifecycle#teardown#7, cloudfront-public-key-lifecycle-and-errors#task#5, cloudfront-query-arg-profile-config-lifecycle#teardown#14).

Cause 2 of 10: GetFieldLevelEncryption after DeleteFieldLevelEncryptionConfig.
AWS: 404 `NoSuchFieldLevelEncryptionConfig`, fixed message `The specified field level encryption config does not exist.`
Emulator: 200 with the config.
Fix: DeleteFieldLevelEncryptionConfig state removal.
Traces: cfetag-field-level-encryption-config-etag-chain#task#9.

Cause 3 of 10: GetKeyGroup after DeleteKeyGroup.
AWS: 404 `NoSuchResource`, fixed message `The specified key group does not exist.` (Smithy: GetKeyGroup's only modelled error is NoSuchResource -> 404).
Emulator: 200 with the KeyGroup.
Fix: DeleteKeyGroup state removal.
Traces: cfetag-key-group-etag-chain#task#7 (also cloudfront-key-group-lifecycle#task#8, cloudfront-key-group-lifecycle-with-members#teardown#8, cloudfront-public-key-and-key-group-lifecycle#task#14).

Cause 4 of 10: GetResponseHeadersPolicy after DeleteResponseHeadersPolicy.
AWS: 404 `NoSuchResponseHeadersPolicy`, fixed message `The specified response headers policy does not exist.`
Emulator: 200 with the policy.
Fix: DeleteResponseHeadersPolicy state removal.
Traces: cfetag-response-headers-policy-etag-chain#task#6 (also cloudfront-response-headers-policy-lifecycle#task#10).

Cause 5 of 10: GetFunction after DeleteFunction - and the refusal is not even XML.
AWS: 404 `NoSuchFunctionExists` with the fixed message `The specified function does not exist.`, served as `Content-Type: text/xml`.
Emulator: 200 with the function code as the body and `Content-Type: application/octet-stream`.
Why: besides the deleted function still existing, an error-path content type of octet-stream means an SDK cannot even parse the refusal.
Fix: DeleteFunction state removal + the GetFunction error path's content-type/serializer.
Traces: cloudfront-cloudfront-function-get-workflow#teardown#7.

Cause 6 of 10: GetFieldLevelEncryptionProfile after DeleteFieldLevelEncryptionProfile.
AWS: 404 `NoSuchFieldLevelEncryptionProfile`, fixed message `The specified field level encryption profile does not exist.`
Emulator: 200 with the profile.
Fix: DeleteFieldLevelEncryptionProfile state removal.
Traces: cloudfront-fle-config-lifecycle-with-profile-reference#teardown#10.

Cause 7 of 10: GetKeyGroup after DeleteKeyGroup, polled 56 times.
AWS: 404 `NoSuchResource`, `The specified key group does not exist.`
Emulator: 200 with the KeyGroup for all 56 poll attempts.
Why: proves the divergence is permanent, not a propagation delay.
Fix: DeleteKeyGroup state removal.
Traces: cloudfront-key-group-precondition-and-inuse-errors#task#6.

Cause 8 of 10: GetPublicKey after DeletePublicKey, polled 29 times.
AWS: 404 `NoSuchPublicKey`, `The specified CloudFront public key does not exist.`
Emulator: 200 with the PublicKey for all 29 attempts.
Fix: DeletePublicKey state removal.
Traces: cloudfront-public-key-basic-lifecycle#teardown#4.

Cause 9 of 10: GetPublicKey after delete on the error-paths fixture (status-only divergence).
AWS: 404.
Emulator: 200.
Fix: DeletePublicKey state removal.
Traces: cloudfront-public-key-error-paths#teardown#7.

Cause 10 of 10: GetResponseHeadersPolicy after DeleteResponseHeadersPolicy, polled 29 times.
AWS: 404 `NoSuchResponseHeadersPolicy`, `The specified response headers policy does not exist.`
Emulator: 200 with the policy for all 29 attempts.
Fix: DeleteResponseHeadersPolicy state removal.
Traces: cloudfront-response-headers-policy-errors#teardown#6.

Note: open stub issues exist for DeleteKeyGroup, DeletePublicKey and DeleteResponseHeadersPolicy (20261029T175823Z-cloudfront-deletekeygroup-stub.md, -deletepublickey-stub.md, -deleteresponseheaderspolicy-stub.md). Those cover the operations being unimplemented; these causes are about the Get* path still returning 200 afterwards and must be re-replayed on their own fixtures.

## Evidence
Every cluster diverges on `status` (404 expected, 200 returned) with the whole $.ErrorResponse missing.

1. GetPublicKey: expected 404 NoSuchPublicKey 'The specified CloudFront public key does not exist.', got 200 (9 affected steps).
2. GetFieldLevelEncryption: expected 404 NoSuchFieldLevelEncryptionConfig 'The specified field level encryption config does not exist.', got 200.
3. GetKeyGroup: expected 404 NoSuchResource 'The specified key group does not exist.', got 200 (4 affected steps).
4. GetResponseHeadersPolicy: expected 404 NoSuchResponseHeadersPolicy 'The specified response headers policy does not exist.', got 200.
5. GetFunction: expected 404 NoSuchFunctionExists 'The specified function does not exist.' with content-type text/xml; got 200, body = the function source, content-type application/octet-stream.
6. GetFieldLevelEncryptionProfile: expected 404 NoSuchFieldLevelEncryptionProfile 'The specified field level encryption profile does not exist.', got 200.
7. GetKeyGroup: 'poll: status 200 not in [404] after 56 attempt(s)', body still the full KeyGroup.
8. GetPublicKey: 'poll: status 200 not in [404] after 29 attempt(s)', body still the full PublicKey including EncodedKey.
9. GetPublicKey: expected 404, got 200.
10. GetResponseHeadersPolicy: 'poll: status 200 not in [404] after 29 attempt(s)', body still the full policy.

The ids, timestamps and key material quoted in the poll bodies are per-run values; the requirement is the 404 and its fixed message.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. For each resource type (public key, FLE config, key group, response headers policy, CloudFront function, FLE profile): create it, call the matching Delete* with the current ETag as If-Match, then call the matching Get*.
3. The Get* answers 200 with the resource body (and for GetFunction, octet-stream) instead of the modelled 404. Polling does not change the outcome.
4. Representative step: fixture cfetag-public-key-etag-chain, step task#8 (GetPublicKey).
