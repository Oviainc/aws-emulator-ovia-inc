Issue-Source: trace-sweep-divergence
Issue-Trace: cfdeep-edge-attachment-inuse-conflicts
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfdeep-edge-attachment-inuse-conflicts cfdeep-origin-access-inuse-conflicts cfetag-field-level-encryption-config-etag-chain cfval-origin-oai-and-oac-both-set cloudfront-connection-group-errors cloudfront-connection-group-lifecycle cloudfront-distribution-tenant-delete-requires-disabled
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|DeleteFunction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteRealtimeLogConfig|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteCloudFrontOriginAccessIdentity|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteOriginAccessControl|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteOriginAccessControl|2xx->4xx|status
Issue-Key: cloudfront|DeleteCloudFrontOriginAccessIdentity|2xx->4xx|status
Issue-Related: 20261029T212223Z-cloudfront-if-match-etag-handling-wrong-precondition-message.md
Issue-Related: 20261029T212223Z-cloudfront-resolves-the-entity-before-validating-id-format.md
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront in-use and must-be-disabled delete conflicts are mis-worded or raised spuriously

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFunction

## Description
Nine causes, one shared fault: the emulator's reference/disabled bookkeeping for deletes is wrong. Where a reference genuinely exists it reports the conflict with its own prose (often leaking the referencing resource's id or a count), and where no reference exists any more it still raises 409, so a correctly-disassociated resource can never be deleted.

Fix for every cause: the delete-time reference/disabled check in that operation's handler and the state it consults - it must count only live references and must emit AWS's fixed sentence for the error class.

Causes 1-6 (conflict detected, message wrong). AWS's still-in-use messages are FIXED text that names neither the resource nor a count.
Cause 1 of 9: DeleteFunction while attached to a cache behavior. AWS: 409 with `Cannot delete function sim2real-probe-... , it is in use by 1 distributions` is NOT AWS's wording - AWS's recorded message is the one in the fixture: the attachment sentence that names the function and the number of distributions holding it, with the function name taken from the request and the count from state. Emulator: "Cannot delete the function because it's attached to one or more cache behaviors." - a fixed sentence with the name and count dropped. Why: the caller cannot tell WHICH function or HOW MANY distributions block the delete. Fix: DeleteFunction's in-use check/message. Traces cfdeep-edge-attachment-inuse-conflicts#task#9.
Cause 2 of 9: DeleteRealtimeLogConfig while referenced. AWS: 400 with the fixed message `The CloudFront real-time log configuration is still being used.` Emulator: 'Cannot delete the real-time log configuration because it is attached to one or more cache behaviors.' Fix: DeleteRealtimeLogConfig's in-use check. Traces cfdeep-edge-attachment-inuse-conflicts#task#11.
Cause 3 of 9: DeleteCloudFrontOriginAccessIdentity while referenced. AWS: 409 with the fixed message `The CloudFront origin access identity is still being used.` Emulator: 'The Origin Access Identity specified is already in use: <Id>' - different sentence, leaks the distribution id. Fix: DeleteCloudFrontOriginAccessIdentity's in-use check. Traces cfdeep-origin-access-inuse-conflicts#task#5.
Cause 4 of 9: DeleteOriginAccessControl while referenced. AWS: 409 with the fixed message `The CloudFront origin access control is still being used.` Emulator: "Cannot delete the origin access control because it's in use by one or more distributions: <Id>". Fix: DeleteOriginAccessControl's in-use check. Traces cfdeep-origin-access-inuse-conflicts#task#6.
Cause 5 of 9: DeleteConnectionGroup on an enabled connection group. AWS: 409 with the fixed message `You must disable the specified connection group before you can delete it.` Emulator: "The specified CloudFront resource hasn't been disabled yet." - a generic sentence that does not say which resource kind. Fix: DeleteConnectionGroup's disabled check. Traces cloudfront-connection-group-errors#task#2 (also cloudfront-connection-group-lifecycle#task#2).
Cause 6 of 9: DeleteDistributionTenant on an enabled tenant. AWS: 409 with the fixed message `You must disable the distribution tenant before you can delete it.` Emulator: 'The specified distribution tenant is not disabled. You must disable the distribution tenant before you can delete it.' - AWS's sentence with an extra leading sentence prepended. Fix: DeleteDistributionTenant's disabled check. Traces cloudfront-distribution-tenant-delete-requires-disabled#task#3.

Causes 7-9 (409 raised when nothing references the resource any more).
AWS: 204 No Content - the delete succeeds.
Emulator: 409, so the resource is undeletable and leaks for the life of the account.
Why: reference counts are never decremented when the referencing resource is deleted or disassociated, so teardown of a legitimately-released resource is impossible.
Cause 7 of 9: DeleteFieldLevelEncryptionProfile after its config was deleted - traces cfetag-field-level-encryption-config-etag-chain#teardown#11.
Cause 8 of 9: DeleteOriginAccessControl after the referencing distribution was deleted - traces cfval-origin-oai-and-oac-both-set#teardown#7.
Cause 9 of 9: DeleteCloudFrontOriginAccessIdentity after the referencing distribution was deleted - traces cfval-origin-oai-and-oac-both-set#teardown#8.

## Evidence
Causes 1-6 (status matches, $.ErrorResponse.Error.Message diverges):
1. DeleteFunction 409: expected 'Cannot delete function <function-name>, it is in use by <n> distributions' (name from the request, count from state), got "Cannot delete the function because it's attached to one or more cache behaviors."
2. DeleteRealtimeLogConfig 400: expected 'The CloudFront real-time log configuration is still being used.', got 'Cannot delete the real-time log configuration because it is attached to one or more cache behaviors.'
3. DeleteCloudFrontOriginAccessIdentity 409: expected 'The CloudFront origin access identity is still being used.', got 'The Origin Access Identity specified is already in use: EZGEITDQ1HLFY0'.
4. DeleteOriginAccessControl 409: expected 'The CloudFront origin access control is still being used.', got "Cannot delete the origin access control because it's in use by one or more distributions: EZGEITDQ1HLFY0".
5. DeleteConnectionGroup 409: expected 'You must disable the specified connection group before you can delete it.', got "The specified CloudFront resource hasn't been disabled yet."
6. DeleteDistributionTenant 409: expected 'You must disable the distribution tenant before you can delete it.', got 'The specified distribution tenant is not disabled. You must disable the distribution tenant before you can delete it.'

Causes 7-9 (status diverges):
7. DeleteFieldLevelEncryptionProfile: expected 204, got 409.
8. DeleteOriginAccessControl: expected 204, got 409.
9. DeleteCloudFrontOriginAccessIdentity: expected 204, got 409.

EZGEITDQ1HLFY0 and the probe function name are per-run values; in causes 3-4 AWS's message contains no id at all, while in cause 1 the name/count ARE the message's variable parts and the emulator drops them.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Causes 1-4: attach a function / real-time log config / OAI / OAC to a distribution cache behavior or origin, then call the matching Delete* and read Error.Message.
3. Causes 5-6: call DeleteConnectionGroup / DeleteDistributionTenant while the resource is still enabled.
4. Causes 7-9: delete the referencing resource (the FLE config, the distribution) first, then call DeleteFieldLevelEncryptionProfile / DeleteOriginAccessControl / DeleteCloudFrontOriginAccessIdentity with the current ETag; each answers 409 instead of 204.
5. Representative step: fixture cfdeep-edge-attachment-inuse-conflicts, step task#9 (DeleteFunction).
