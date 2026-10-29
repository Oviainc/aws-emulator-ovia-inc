Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.create-alias-reserved-prefix-fails kms.op-alias-without-prefix-rejected kms.encrypt-max-plaintext-size kms.op-encrypt-oversized-plaintext-rejected kms.error-sign-with-encrypt-key kms.sign-verify-ecc-p384-signs-only-with-ecdsa-sha-384 kms.import-key-material-validto-out-of-range-rejected kms.replicate-key-invalid-arn-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.create-alias-reserved-prefix-fails|run#11|AssertionError|assert got == expected, f"expected {expected!r}, got {got!r}"
Issue-Key: kms.op-alias-without-prefix-rejected|run#13|AssertionError|assert code == "ValidationException", (
Issue-Key: kms.encrypt-max-plaintext-size|run#11|AssertionError|assert message == (
Issue-Key: kms.op-encrypt-oversized-plaintext-rejected|run#9|AssertionError|assert message == (
Issue-Key: kms.error-sign-with-encrypt-key|run#12|AssertionError|assert message == (
Issue-Key: kms.sign-verify-ecc-p384-signs-only-with-ecdsa-sha-384|run#40|AssertionError|assert (code, msg) == ("InvalidKeyUsageException", incompatible), (
Issue-Key: kms.import-key-material-validto-out-of-range-rejected|run#13|AssertionError|assert message == "ValidTo must be in the future", message
Issue-Key: kms.replicate-key-invalid-arn-rejected|run#7|AssertionError|assert code == "InvalidArnException", code
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# kms: 7 net-new refusal-fidelity root causes — CreateAlias prefix refusals use the wrong code, Encrypt's plaintext-ceiling message echoes the sensitive value, Sign's key-usage/key-spec messages are emulator-authored, ImportKeyMaterial's ValidTo message diverges, ReplicateKey misclassifies an alias ARN

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlias

## Description
SEVEN distinct net-new root causes in the KMS emulator's refusal classification and refusal messages. Every one is grounded in a failing behavioral check of this suite that the report marked neither ALREADY FILED nor OWNED. A fixer must close all seven before submitting.

Related OPEN issues that do NOT carry these checks' replay keys (named for context only): 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md (its Cause 4 describes CreateAlias's missing-prefix code for a DIFFERENT check id, kms.alias-format-validation, and its Cause 5 describes the empty-Message contract for other operations) and 20261024T221823Z-kms-grant-authorization-grant-refusal-contracts-listretirablegrants-validation.md.

Cause 1 of 7 — CreateAlias on a RESERVED alias/aws/ name is refused under the wrong error code and with an emulator-authored message.
AWS: creating a customer alias whose name begins with 'alias/aws/' is refused NotAuthorizedException, HTTP 400, with the fixed message text 'The alias name for a customer managed CMK cannot begin with alias/aws/.' (the whole sentence is fixed — it interpolates nothing from the request). Smithy: CreateAlias models AlreadyExistsException/DependencyTimeoutException/InvalidAliasNameException/KMSInternalException/KMSInvalidStateException/LimitExceededException/NotFoundException; NotAuthorizedException is NOT among them, so this is the service-common authorization fault — recorded behavior only for the code choice.
Emulator: InvalidAliasNameException with 'Aliases beginning with "alias/aws/" are reserved for Amazon Web Services managed keys. Found: <alias name>' — a different code and a message that echoes the requested alias name.
Why: AWS distinguishes 'this name is syntactically not an alias' (InvalidAliasName) from 'this namespace is not yours' (NotAuthorized). A client that branches on NotAuthorizedException to tell a reserved-namespace attempt from a malformed name takes the wrong branch, and IAM-style retry/escalation logic keyed on authorization faults never fires.
Fix: the CreateAlias handler's reserved-prefix check and its error classification.
Traces: kms.create-alias-reserved-prefix-fails.

Cause 2 of 7 — CreateAlias on a name WITHOUT the 'alias/' prefix is refused InvalidAliasNameException where AWS returns ValidationException.
AWS: an AliasName with no 'alias/' prefix is refused ValidationException, HTTP 400 (recorded behavior only — the model does not say which fault covers a missing prefix). Its message's fixed text is 'Alias must start with the prefix "alias/". Please see https://docs.aws.amazon.com/kms/latest/developerguide/kms-alias.html' and interpolates nothing. Smithy: AliasName is AliasNameType, length 1..256, pattern ^[a-zA-Z0-9:/_-]+$ — the pattern admits a prefix-less name, so the prefix rule is a handler responsibility.
Emulator: InvalidAliasNameException with 'Alias must start with the prefix "alias/". Found: <name>' (the emulator appends the rejected value).
Why: the same code-branch divergence as Cause 1, on the opposite input class; a client cannot distinguish the two refusals at all because the emulator answers both with InvalidAliasNameException.
Fix: the CreateAlias handler's input validation / error classification.
Traces: kms.op-alias-without-prefix-rejected.

Cause 3 of 7 — Encrypt's oversized-plaintext ValidationException message interpolates the rejected value, which AWS withholds because Plaintext is sensitive.
AWS: Encrypt with 4097 plaintext bytes is refused ValidationException, HTTP 400, with the message '1 validation error detected: Value at 'plaintext' failed to satisfy constraint: Member must have length less than or equal to 4096'. Everything in that sentence is fixed except the member name and the limit, which come from the model; there is NO 'Value ...' operand, because Smithy marks Encrypt$Plaintext @sensitive (PlaintextType, blob, length 1..4096). Exactly 4096 bytes succeeds (the at-limit leg of both checks passed).
Emulator: ValidationException (code and status correct) with '1 validation error detected: Value '4097 bytes' at 'plaintext' failed to satisfy constraint: Member must have length less than or equal to 4096' — it inserts a Value operand derived from the sensitive member.
Why: the message is the only observable distinguishing the constraint that failed, and emitting a Value operand for a @sensitive member is a disclosure AWS deliberately avoids; clients matching AWS's exact constraint-violation template fail to parse it.
Fix: the KMS validation/error serializer for sensitive members — omit the Value operand when the offending member is @sensitive.
Traces: kms.encrypt-max-plaintext-size, kms.op-encrypt-oversized-plaintext-rejected.

Cause 4 of 7 — Sign on an ENCRYPT_DECRYPT key is refused with the right code but an emulator-authored message.
AWS: Sign against a SYMMETRIC_DEFAULT/ENCRYPT_DECRYPT key is refused InvalidKeyUsageException (Smithy: modeled on Sign, httpError 400), HTTP 400, with the message template '<key ARN> key usage is <KeyUsage> which is not valid for <operation name>.' — the key ARN, the key's KeyUsage and the operation name are the only variable parts; the rest is fixed. The positive control (a SIGN_VERIFY RSA_2048 key signing the same digest) passed, so only the refusal text diverges.
Emulator: InvalidKeyUsageException with 'The KeyUsage value of the KMS key is incompatible with the API operation. <key ARN> has KeyUsage ENCRYPT_DECRYPT; for signing and verifying messages, the KeyUsage must be SIGN_VERIFY.'
Why: clients and test suites assert on this message to prove the refusal is about key usage rather than a blanket Sign failure; the emulator's sentence shares no fixed text with AWS's, so every such assertion fails.
Fix: the Sign handler's InvalidKeyUsageException construction (KMS refusal-message templates).
Traces: kms.error-sign-with-encrypt-key.

Cause 5 of 7 — Sign with a SigningAlgorithm incompatible with the key's KeySpec carries a different message template.
AWS: Sign(SigningAlgorithm=ECDSA_SHA_256) on an ECC_NIST_P384 key is refused InvalidKeyUsageException, HTTP 400, with the message template 'Algorithm <SigningAlgorithm> is incompatible with key spec <KeySpec>.' — the algorithm comes from the request, the key spec from the key; the rest is fixed. Smithy: Sign$SigningAlgorithm is required (SigningAlgorithmSpec) and InvalidKeyUsageException is modeled.
Emulator: the CODE is right, the message is not: 'The signing algorithm specified for the operation is incompatible with the type of key material in the KMS key (KeySpec). <key ARN> has KeySpec ECC_NIST_P384, which supports ECDSA_SHA_384.'
Why: the two incompatibility refusals (wrong KeyUsage, Cause 4, vs wrong algorithm for the KeySpec) are told apart by their message text alone — both are InvalidKeyUsageException — so a client cannot tell which input to correct.
Fix: the Sign handler's algorithm/key-spec compatibility refusal construction.
Traces: kms.sign-verify-ecc-p384-signs-only-with-ecdsa-sha-384.

Cause 6 of 7 — ImportKeyMaterial's past-ValidTo refusal carries a different message.
AWS: ImportKeyMaterial(ExpirationModel=KEY_MATERIAL_EXPIRES, ValidTo=<a past timestamp>) is refused ValidationException, HTTP 400, with the fixed message 'ValidTo must be in the future' (no interpolated operands). The identical (token, wrapped material) pair with a future ValidTo succeeds, so only the refusal text is at issue. Smithy: ValidTo is DateType and ExpirationModel is ExpirationModelType; the range rule is a handler responsibility.
Emulator: the code is ValidationException, the message is 'ValidTo must be a future date and time, at most 365 days from the request date.'
Why: the emulator asserts an additional 365-day rule in text AWS does not send here, so a client that renders or matches the refusal shows a constraint AWS did not state.
Fix: the ImportKeyMaterial handler's ValidTo validation message.
Traces: kms.import-key-material-validto-out-of-range-rejected.

Cause 7 of 7 — ReplicateKey given an ALIAS ARN is refused NotFoundException instead of InvalidArnException.
AWS: ReplicateKey(KeyId=<a syntactically valid alias ARN in the caller's own account>) is refused InvalidArnException (Smithy: modeled on ReplicateKey, httpError 400) with the fixed message 'Key Aliases are not supported for this operation.' — ARN-grammar/kind validation runs BEFORE any key lookup. The positive control (a well-formed key ARN for a real single-Region key) correctly reaches the business rule, so only the alias-ARN branch diverges.
Emulator: NotFoundException — the handler resolves the identifier first and reports the missing alias, so the ARN-kind rule is never applied.
Why: the two refusals mean opposite things to a client — 'this identifier kind is not accepted here' (never retry, fix the call) versus 'this resource does not exist' (create it, or retry). Ordering lookup before ARN validation also leaks whether an alias exists.
Fix: the ReplicateKey handler's KeyId validation ordering (reject alias-kind ARNs before resolution).
Traces: kms.replicate-key-invalid-arn-rejected.

## Evidence
Cause 1 — kms.create-alias-reserved-prefix-fails: "AssertionError: expected ('NotAuthorizedException', 'The alias name for a customer managed CMK cannot begin with alias/aws/.'), got ('InvalidAliasNameException', 'Aliases beginning with \"alias/aws/\" are reserved for Amazon Web Services managed keys. Found: alias/aws/reserved-2d091dfa31ce')". The 'alias/aws/reserved-<hex>' suffix is this run's generated name, not part of the requirement.

Cause 2 — kms.op-alias-without-prefix-rejected: "AssertionError: 'sim2real-behav-noprefix-3a402b315bb8' -> expected ValidationException, got InvalidAliasNameException".

Cause 3 — kms.encrypt-max-plaintext-size: "AssertionError: unexpected message \"1 validation error detected: Value '4097 bytes' at 'plaintext' failed to satisfy constraint: Member must have length less than or equal to 4096\""; the check asserts code=='ValidationException' (which passed) and message=="1 validation error detected: Value at 'plaintext' failed to satisfy constraint: Member must have length less than or equal to 4096". kms.op-encrypt-oversized-plaintext-rejected fails on the identical difference. Both checks' Encrypt of exactly 4096 bytes succeeded.

Cause 4 — kms.error-sign-with-encrypt-key: "AssertionError: unexpected message 'The KeyUsage value of the KMS key is incompatible with the API operation. arn:aws:kms:<region>:<account>:key/<id> has KeyUsage ENCRYPT_DECRYPT; for signing and verifying messages, the KeyUsage must be SIGN_VERIFY.'"; expected '<key ARN> key usage is ENCRYPT_DECRYPT which is not valid for Sign.' Code InvalidKeyUsageException matched; the SIGN_VERIFY positive control signed successfully.

Cause 5 — kms.sign-verify-ecc-p384-signs-only-with-ecdsa-sha-384: "AssertionError: Sign with ECDSA_SHA_256 on a P-384 key must be refused InvalidKeyUsageException 'Algorithm ECDSA_SHA_256 is incompatible with key spec ECC_NIST_P384.', got 'InvalidKeyUsageException': 'The signing algorithm specified for the operation is incompatible with the type of key material in the KMS key (KeySpec). arn:aws:kms:<region>:<account>:key/<id> has KeySpec ECC_NIST_P384, which supports ECDSA_SHA_384.'"

Cause 6 — kms.import-key-material-validto-out-of-range-rejected: the check asserts code=='ValidationException' (passed) then message=='ValidTo must be in the future'; got 'ValidTo must be a future date and time, at most 365 days from the request date.'

Cause 7 — kms.replicate-key-invalid-arn-rejected: "assert code == \"InvalidArnException\" ... AssertionError: NotFoundException" for ReplicateKey(KeyId='arn:aws:kms:<region>:<account>:alias/sim2real-nonexistent', ReplicaRegion=<other region>); the expected message is 'Key Aliases are not supported for this operation.'

## Steps to Reproduce
Replay the behavioral suite and re-run the eight check ids in trace_ids. Per cause:
1. CreateKey; CreateAlias(AliasName='alias/aws/<anything>', TargetKeyId=<key>) — observe InvalidAliasNameException instead of NotAuthorizedException / 'The alias name for a customer managed CMK cannot begin with alias/aws/.'
2. CreateAlias(AliasName='<name with no alias/ prefix>', TargetKeyId=<key>) — observe InvalidAliasNameException instead of ValidationException.
3. CreateKey(KeyUsage=ENCRYPT_DECRYPT, KeySpec=SYMMETRIC_DEFAULT); Encrypt(Plaintext=b'x'*4096) (succeeds); Encrypt(Plaintext=b'x'*4097) — inspect Error.Message for the extra "Value '4097 bytes'" operand.
4. CreateKey(KeyUsage=SIGN_VERIFY, KeySpec=RSA_2048) and Sign a SHA-256 digest with RSASSA_PKCS1_V1_5_SHA_256 (succeeds); CreateKey(KeyUsage=ENCRYPT_DECRYPT, KeySpec=SYMMETRIC_DEFAULT) and Sign the same digest — inspect Error.Message.
5. CreateKey(KeySpec=ECC_NIST_P384, KeyUsage=SIGN_VERIFY); Sign(MessageType=DIGEST, SigningAlgorithm='ECDSA_SHA_256') — inspect Error.Message.
6. CreateKey(Origin=EXTERNAL); GetParametersForImport; ImportKeyMaterial(ExpirationModel='KEY_MATERIAL_EXPIRES', ValidTo=<yesterday>) — inspect Error.Message.
7. ReplicateKey(KeyId='arn:aws:kms:<region>:<your account>:alias/does-not-exist', ReplicaRegion=<another region>) — observe NotFoundException instead of InvalidArnException.
