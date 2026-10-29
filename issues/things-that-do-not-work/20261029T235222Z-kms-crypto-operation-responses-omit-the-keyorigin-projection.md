Issue-Source: trace-sweep-divergence
Issue-Trace: kms-asymmetric-key-sign-verify
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-asymmetric-key-sign-verify kms-asymmetric-sign-verify kms-derive-shared-secret kms-sign-digest-length-mismatch-rejected kms-data-key-generation kms-mac-generate-verify kms-symmetric-encrypt-decrypt
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|GetPublicKey|2xx->2xx|$.KeyOrigin
Issue-Key: kms|Sign|2xx->2xx|$.KeyOrigin
Issue-Key: kms|Verify|2xx->2xx|$.KeyOrigin
Issue-Key: kms|GenerateDataKey|2xx->2xx|$.KeyOrigin
Issue-Key: kms|GenerateDataKeyWithoutPlaintext|2xx->2xx|$.KeyOrigin
Issue-Key: kms|GenerateDataKeyPair|2xx->2xx|$.KeyOrigin
Issue-Key: kms|GenerateDataKeyPairWithoutPlaintext|2xx->2xx|$.KeyOrigin
Issue-Key: kms|GenerateMac|2xx->2xx|$.KeyOrigin
Issue-Key: kms|VerifyMac|2xx->2xx|$.KeyOrigin
Issue-Key: kms|ReEncrypt|2xx->2xx|$.DestinationKeyOrigin
Issue-Key: kms|ReEncrypt|2xx->2xx|$.SourceKeyOrigin
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# KMS crypto-operation responses omit the KeyOrigin projection the recorded wire carries

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPublicKey

## Description
Across the whole KMS cryptographic surface the emulator's 200 response body is missing the key-origin projection that every recorded success response carries. The recorded responses report the Origin of the KMS key that served the request (value AWS_KMS for a KMS-generated key); the emulator emits no such member on any of these operations, so the member is absent from each body. 10 causes, one per operation, all the same defect in the same place.

Smithy: KeyOrigin is NOT a modeled member of GetPublicKeyResponse, SignResponse, VerifyResponse, GenerateDataKeyResponse, GenerateDataKeyWithoutPlaintextResponse, GenerateDataKeyPairResponse, GenerateDataKeyPairWithoutPlaintextResponse, GenerateMacResponse, VerifyMacResponse or ReEncryptResponse (checked against services/service-com-amazonaws-kms-trentservice/smithy-model). So for every cause below the AWS requirement is RECORDED BEHAVIOR ONLY: the recorded 200 body carries the member, with the string value of the serving key's Origin. Hypothesis: the member reports KeyMetadata.Origin of the key resolved for the call, which is AWS_KMS for any key whose material KMS generated and EXTERNAL / AWS_CLOUDHSM / EXTERNAL_KEY_STORE for the other origins — the value is read from the key's stored state, it is not a constant.

Cause 1 of 10: GetPublicKey.
AWS: recorded behavior only — the 200 body carries KeyOrigin, whose value is the Origin of the key named by the request's KeyId ('AWS_KMS' in the recording).
Emulator: answers 200 with KeyOrigin absent ($.KeyOrigin: missing (expected 'AWS_KMS', got None)).
Why: a client that reads the serving key's origin off the crypto response (to decide whether key material is customer-supplied before re-importing or re-wrapping) sees nothing and cannot distinguish an AWS_KMS key from an EXTERNAL one.
Fix: the GetPublicKey handler's response serializer — project the resolved key's Origin onto the output alongside KeyId.
Traces: kms-asymmetric-key-sign-verify#task#2 (also kms-asymmetric-sign-verify#task#2, kms-derive-shared-secret#task#3).

Cause 2 of 10: Sign.
AWS: recorded behavior only — the 200 body carries KeyOrigin for the signing key.
Emulator: 200 with $.KeyOrigin missing (expected 'AWS_KMS', got None).
Why: same as cause 1, on the signing path.
Fix: the Sign handler's response serializer.
Traces: kms-asymmetric-key-sign-verify#task#3 (also kms-asymmetric-sign-verify#task#3, kms-sign-digest-length-mismatch-rejected#task#7 and #task#10).

Cause 3 of 10: Verify.
AWS: recorded behavior only — the 200 body carries KeyOrigin for the verifying key.
Emulator: 200 with $.KeyOrigin missing (expected 'AWS_KMS', got None).
Fix: the Verify handler's response serializer.
Traces: kms-asymmetric-key-sign-verify#task#4 (also kms-asymmetric-sign-verify#task#4, kms-sign-digest-length-mismatch-rejected#task#8).

Cause 4 of 10: GenerateDataKey.
AWS: recorded behavior only — the 200 body carries KeyOrigin for the wrapping CMK.
Emulator: 200 with $.KeyOrigin missing (expected 'AWS_KMS', got None).
Fix: the GenerateDataKey handler's response serializer.
Traces: kms-data-key-generation#task#2.

Cause 5 of 10: GenerateDataKeyWithoutPlaintext.
AWS: recorded behavior only — same projection as cause 4.
Emulator: 200 with $.KeyOrigin missing.
Fix: the GenerateDataKeyWithoutPlaintext handler's response serializer.
Traces: kms-data-key-generation#task#3.

Cause 6 of 10: GenerateDataKeyPair.
AWS: recorded behavior only — same projection.
Emulator: 200 with $.KeyOrigin missing.
Fix: the GenerateDataKeyPair handler's response serializer.
Traces: kms-data-key-generation#task#4.

Cause 7 of 10: GenerateDataKeyPairWithoutPlaintext.
AWS: recorded behavior only — same projection.
Emulator: 200 with $.KeyOrigin missing.
Fix: the GenerateDataKeyPairWithoutPlaintext handler's response serializer.
Traces: kms-data-key-generation#task#5.

Cause 8 of 10: GenerateMac.
AWS: recorded behavior only — the 200 body carries KeyOrigin for the HMAC key.
Emulator: 200 with $.KeyOrigin missing.
Fix: the GenerateMac handler's response serializer.
Traces: kms-mac-generate-verify#task#2 (also kms-mac-generate-verify#task#8).

Cause 9 of 10: VerifyMac.
AWS: recorded behavior only — same projection as cause 8.
Emulator: 200 with $.KeyOrigin missing.
Fix: the VerifyMac handler's response serializer.
Traces: kms-mac-generate-verify#task#3.

Cause 10 of 10: ReEncrypt.
AWS: recorded behavior only — the 200 body carries TWO origin members, SourceKeyOrigin for the key that decrypted the ciphertext and DestinationKeyOrigin for the key that re-wrapped it.
Emulator: 200 with both absent ($.SourceKeyOrigin: missing (expected 'AWS_KMS', got None); $.DestinationKeyOrigin: missing (expected 'AWS_KMS', got None)).
Why: a re-encrypt caller cannot tell which side of the hop is on customer-supplied material.
Fix: the ReEncrypt handler's response serializer — project both the resolved source key's and the destination key's Origin.
Traces: kms-symmetric-encrypt-decrypt#task#6.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, 10 mismatch clusters, all expected_status 200 / actual_status 200 with a body-key difference:
- GetPublicKey, kms-asymmetric-key-sign-verify#task#2: $.KeyOrigin: missing (expected 'AWS_KMS', got None); 3 affected steps.
- Sign, kms-asymmetric-key-sign-verify#task#3: $.KeyOrigin: missing (expected 'AWS_KMS', got None); 4 affected steps.
- Verify, kms-asymmetric-key-sign-verify#task#4: $.KeyOrigin: missing (expected 'AWS_KMS', got None); 3 affected steps.
- GenerateDataKey, kms-data-key-generation#task#2: $.KeyOrigin: missing (expected 'AWS_KMS', got None).
- GenerateDataKeyWithoutPlaintext, kms-data-key-generation#task#3: $.KeyOrigin: missing.
- GenerateDataKeyPair, kms-data-key-generation#task#4: $.KeyOrigin: missing.
- GenerateDataKeyPairWithoutPlaintext, kms-data-key-generation#task#5: $.KeyOrigin: missing.
- GenerateMac, kms-mac-generate-verify#task#2: $.KeyOrigin: missing; 2 affected steps.
- VerifyMac, kms-mac-generate-verify#task#3: $.KeyOrigin: missing.
- ReEncrypt, kms-symmetric-encrypt-decrypt#task#6: $.DestinationKeyOrigin: missing (expected 'AWS_KMS', got None) and $.SourceKeyOrigin: missing (expected 'AWS_KMS', got None).
Expected in every case: the member present, carrying the serving key's Origin. Actual: the member absent from the emulator's body.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Inspect fixture kms-asymmetric-key-sign-verify step task#2 (GetPublicKey): the response is 200 but has no KeyOrigin member, while the recorded response carries KeyOrigin='AWS_KMS'.
4. Repeat for each fixture#step listed under Traces (Sign, Verify, GenerateDataKey*, GenerateMac, VerifyMac) and for kms-symmetric-encrypt-decrypt#task#6, where both SourceKeyOrigin and DestinationKeyOrigin are absent.
5. Equivalent by hand: CreateKey (Origin AWS_KMS), then call any of the operations above against it and diff the response body against the recorded one.
