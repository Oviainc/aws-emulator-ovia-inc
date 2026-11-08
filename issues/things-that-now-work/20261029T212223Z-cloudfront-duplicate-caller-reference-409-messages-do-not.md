Issue-Source: trace-sweep-divergence
Issue-Trace: cfetag-field-level-encryption-config-etag-chain
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfetag-field-level-encryption-config-etag-chain cfetag-origin-access-identity-etag-chain cloudfront-fle-profile-duplicate-caller-reference-error cloudfront-public-key-error-paths cloudfront-public-key-lifecycle-and-errors
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateFieldLevelEncryptionConfig|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateCloudFrontOriginAccessIdentity|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateFieldLevelEncryptionProfile|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreatePublicKey|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront duplicate caller-reference 409 messages do not use AWS wording

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFieldLevelEncryptionConfig

## Description
Four causes, one shared fault: when a Create* call reuses a CallerReference that is already bound to another resource, the emulator returns the right 409 status and the right error code but its `Error.Message` is its own sentence with the colliding resource id appended, instead of AWS's fixed caller-reference sentence. AWS's caller-reference message is fixed text with no variable part - it never names the existing id.

Fix for every cause: the caller-reference collision branch of each operation's validation must emit AWS's fixed sentence and must not append the colliding resource id.

Cause 1 of 4: CreateFieldLevelEncryptionConfig with a reused CallerReference.
AWS: 409 `FieldLevelEncryptionConfigAlreadyExists` with the fixed message `The caller reference that you are using to create a field level encryption config is associated with another field level encryption config in your account.` (Smithy: FieldLevelEncryptionConfig.CallerReference is required and is the replay-protection key).
Emulator: 409 with `The specified configuration for field-level encryption already exists: <Id>` - different sentence, and it leaks the generated config id.
Why: a client that matches AWS's documented sentence to distinguish "caller reference reused" from other conflicts cannot, and the id makes the message vary per run.
Fix: CreateFieldLevelEncryptionConfig's caller-reference uniqueness check.
Traces: cfetag-field-level-encryption-config-etag-chain#task#4.

Cause 2 of 4: CreateCloudFrontOriginAccessIdentity with a reused CallerReference.
AWS: 409 `CloudFrontOriginAccessIdentityAlreadyExists`, fixed message `The caller reference you are using to create the CloudFront origin access identity is associated with another identity.`
Emulator: 409 with `The caller reference that you are using to create an origin access identity is associated with another identity. Already exists: <Id>` - the sentence is reworded and a second sentence carrying the id is appended.
Why: same as cause 1.
Fix: CreateCloudFrontOriginAccessIdentity's caller-reference uniqueness check.
Traces: cfetag-origin-access-identity-etag-chain#task#3.

Cause 3 of 4: CreateFieldLevelEncryptionProfile with a reused CallerReference.
AWS: 409 `FieldLevelEncryptionProfileAlreadyExists`, fixed message `The caller reference that you are using to create a field level encryption profile is associated with another field level encryption profile in your account.`
Emulator: 409 with `The specified profile for field-level encryption already exists: <Id>`.
Why: same as cause 1.
Fix: CreateFieldLevelEncryptionProfile's caller-reference uniqueness check.
Traces: cloudfront-fle-profile-duplicate-caller-reference-error#task#3.

Cause 4 of 4: CreatePublicKey with a reused CallerReference.
AWS: 409 `PublicKeyAlreadyExists`, fixed message `The caller reference that you are using to create a CloudFront public key is associated with another public key in your account.` (Smithy: PublicKeyConfig.CallerReference is required - 'A string included in the request to help make sure that the request can't be replayed').
Emulator: 409 with `The specified public key already exists: <Id>`.
Why: same as cause 1; also indistinguishable from the duplicate-Name conflict, which AWS words differently.
Fix: CreatePublicKey's caller-reference uniqueness check.
Traces: cloudfront-public-key-error-paths#task#3 (also cloudfront-public-key-lifecycle-and-errors#task#3).

## Evidence
All four clusters match on status (409) and code and diverge only on $.ErrorResponse.Error.Message.

1. CreateFieldLevelEncryptionConfig: expected 'The caller reference that you are using to create a field level encryption config is associated with another field level encryption config in your account.', got 'The specified configuration for field-level encryption already exists: CVBB9JKPE6KWE0'.
2. CreateCloudFrontOriginAccessIdentity: expected 'The caller reference you are using to create the CloudFront origin access identity is associated with another identity.', got 'The caller reference that you are using to create an origin access identity is associated with another identity. Already exists: E7IYCGPKTANSF3'.
3. CreateFieldLevelEncryptionProfile: expected 'The caller reference that you are using to create a field level encryption profile is associated with another field level encryption profile in your account.', got 'The specified profile for field-level encryption already exists: PJZ50Z92T9DP30'.
4. CreatePublicKey: expected 'The caller reference that you are using to create a CloudFront public key is associated with another public key in your account.', got 'The specified public key already exists: K7IYCGPKTANSF3'.

The ids shown (CVBB9JKPE6KWE0, E7IYCGPKTANSF3, PJZ50Z92T9DP30, K7IYCGPKTANSF3) are per-run generated values; AWS's message contains no id, so the whole trailing clause is unmodelled.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Create the resource with CallerReference R; then call the same Create* again with CallerReference R and a different Name.
3. Observe 409 with the emulator's own sentence and the existing resource id appended, instead of AWS's fixed caller-reference sentence.
4. Representative step: fixture cfetag-field-level-encryption-config-etag-chain, step task#4 (CreateFieldLevelEncryptionConfig).
