Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.asymmetric-decrypt-requires-key-id kms.default-key-policy-grants-root-account-access kms.aws-managed-key-reference-catalogue kms.op-schedule-deletion-window-out-of-range-rejected kms.put-key-policy-rejection-leaves-policy-intact
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.asymmetric-decrypt-requires-key-id|run#14|AssertionError|assert no_keyid == (ERR_VALIDATION, MSG_KEYID_NULL), (
Issue-Key: kms.default-key-policy-grants-root-account-access|_error#26|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Key: kms.aws-managed-key-reference-catalogue|run#4|AssertionError|assert managed, (
Issue-Key: kms.op-schedule-deletion-window-out-of-range-rejected|run#7|AssertionError|assert low_msg == out_of_range, f"6 days -> unexpected message {low_msg!r}"
Issue-Key: kms.put-key-policy-rejection-leaves-policy-intact|_err#17|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: cb1771c3c2a1df51218899fe99979bb9da68f0b3

# kms: 5 net-new root causes — Decrypt's KeyId/EncryptionAlgorithm combination rule is not checked before the blob, PutKeyPolicy has no lockout check and resolves no principals, the AWS-managed key catalogue is still empty, and ScheduleKeyDeletion's range refusal lost its message

> _Note: this issue was identified against the code base at commit `cb1771c3c2a1df51218899fe99979bb9da68f0b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutKeyPolicy

## Description
REOPENS 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md: its fix left the call refused. That resolved issue claimed all five of the checks below (its Causes 2, 3, 5 and 6); the asymmetric Decrypt path it claimed to implement is still refused, now with the wrong error class, and the other four contracts it named remain divergent (one of them, the ScheduleKeyDeletion range message, regressed from wrong text to no text). Five distinct net-new root causes follow, all in the KMS crate, each with its own failing check ids.

Cause 1 of 5 — Decrypt does not enforce the KeyId/EncryptionAlgorithm parameter-combination rule before it parses the ciphertext.
AWS: when Decrypt carries a non-default EncryptionAlgorithm, KeyId becomes mandatory, and that request-validation rule is applied BEFORE the ciphertext blob is examined at all: Decrypt(CiphertextBlob=<any>, EncryptionAlgorithm=RSAES_OAEP_SHA_256) with no KeyId is refused ValidationException with the message 'KeyId must not be null' (fixed text; no variable part). Only when neither KeyId nor EncryptionAlgorithm is given is the request well-formed (algorithm defaults to SYMMETRIC_DEFAULT) and the blob then parsed, which is where an RSA block earns InvalidCiphertextException with an empty message. Smithy: Decrypt.KeyId is KeyIdType and NOT required, Decrypt.EncryptionAlgorithm is EncryptionAlgorithmSpec, and InvalidCiphertextException is modeled (httpError 400); the KeyId-required-with-a-non-default-algorithm rule is recorded behavior only, as is the service-common ValidationException.
Emulator: the no-KeyId + EncryptionAlgorithm=RSAES_OAEP_SHA_256 request is refused ('InvalidCiphertextException', '') — i.e. the handler skipped the parameter-combination check and went straight to blob parsing, producing the refusal that belongs to the DIFFERENT request shape (no KeyId, no algorithm).
Why: the two refusals are how a client tells a malformed request apart from a ciphertext/key mismatch. A client that retries on InvalidCiphertextException (re-encrypt, re-fetch the blob) will loop forever on what is actually a missing-parameter bug, and SDK/validation-layer tests that assert request validation precedes payload handling pass against AWS and fail here.
Fix: the Decrypt handler's input validation, ahead of ciphertext decoding (reject before any blob parse when EncryptionAlgorithm is present and KeyId is absent).
Traces: kms.asymmetric-decrypt-requires-key-id.

Cause 2 of 5 — PutKeyPolicy still applies no policy-lockout safety check.
AWS: a replacement key policy that leaves the calling principal no path to kms:PutKeyPolicy (e.g. dropping the account-root 'Enable IAM User Permissions' statement) is refused MalformedPolicyDocumentException, HTTP 400, with the fixed message 'The new key policy will not allow you to update the key policy in the future.' (no variable part), and the stored document is left untouched — unless BypassPolicyLockoutSafetyCheck is set. Smithy: PutKeyPolicy errors include MalformedPolicyDocumentException (httpError 400) and the input member BypassPolicyLockoutSafetyCheck has default=false, so the safety check is the modeled default.
Emulator: the call returns HTTP 200 with an empty body (content-length 2) and stores the locked-out policy.
Why: a client can irrecoverably lock itself out of a key in the emulator, and the single guardrail AWS users rely on when scripting key policies is unverifiable — a deployment pipeline that is supposed to fail its safety test passes.
Fix: PutKeyPolicy's validation path — lockout evaluation of the submitted document, honouring BypassPolicyLockoutSafetyCheck.
Traces: kms.default-key-policy-grants-root-account-access.

Cause 3 of 5 — PutKeyPolicy does not resolve the principals a policy names.
AWS: KMS resolves every Principal at write time. A syntactically valid document whose statement names an IAM role ARN in the account that does not exist is refused MalformedPolicyDocumentException, HTTP 400, with the fixed message 'Policy contains a statement with one or more invalid principals.' (no variable part — the offending ARN is NOT echoed), and the stored policy is unchanged. This is the same error CODE as a bad-syntax refusal but the only one of the two that carries message text, which is how principal resolution is told apart from malformed JSON. Smithy: PutKeyPolicy errors include MalformedPolicyDocumentException (httpError 400); the principal-resolution rule itself is recorded behavior only.
Emulator: the call SUCCEEDS with HTTP 200 and an empty body, storing a policy that grants kms:Decrypt to a nonexistent role.
Why: typo'd or not-yet-created role ARNs in key policies are accepted silently, so the emulator reports a working grant where AWS refuses the write; infrastructure code that depends on the write failing (ordering a role's creation before the key policy that references it) is never corrected.
Fix: PutKeyPolicy's validation path — principal resolution against IAM state before the document is persisted.
Traces: kms.put-key-policy-rejection-leaves-policy-intact.

Cause 4 of 5 — ScheduleKeyDeletion's out-of-range refusal carries no message text.
AWS: PendingWindowInDays outside 7..30 is refused ValidationException with the message 'PendingWindowInDays must be between 7 and 30' — fixed text whose only variable parts are the two limits (7 and 30, the service's enforced window), identical for the low (6) and high (31) rejection, and the key's state is untouched. Smithy: ScheduleKeyDeletion.PendingWindowInDays is PendingWindowInDaysType with range 1..365, so the 7..30 window is a service rule the model does not express; the message is recorded behavior.
Emulator: the error CODE is right (ValidationException) but the message is the empty string '' for both 6 and 31.
Why: the message is the only thing that tells a caller WHICH parameter and which bounds were violated — ValidationException with no text is indistinguishable from any other input refusal, so a client surfacing the error to an operator shows nothing actionable. Note this is a regression of the prior fix, which made these messages empty on the premise that AWS sends them empty; the recording shows the text above.
Fix: ScheduleKeyDeletion's validation / the KMS error serializer for this refusal.
Traces: kms.op-schedule-deletion-window-out-of-range-rejected.

Cause 5 of 5 — the AWS-managed key catalogue still does not exist.
AWS: every account already has AWS-managed keys with alias/aws/* aliases (alias/aws/s3, alias/aws/secretsmanager, alias/aws/dynamodb, ...) that nobody created; they appear in ListAliases and each resolves to a key whose KeyMetadata.KeyManager is AWS. Smithy: ListAliases returns AliasList with Marker/NextMarker/Truncated paging; DescribeKey's KeyMetadata.KeyManager distinguishes AWS from CUSTOMER. The catalogue's existence is reference data (recorded behavior only).
Emulator: ListAliases over a fresh account returns 0 aliases in total and no alias/aws/* entry at all.
Why: every integration that defaults to an AWS-managed key (an S3 aws:kms bucket default with no KMSMasterKeyID, a Secrets Manager rekey to alias/aws/secretsmanager, DynamoDB's alias/aws/dynamodb) has nothing to resolve to, so a client that reads the key ARN back off such a resource gets nothing; this cause is upstream of several cross-service failures in this suite.
Fix: KMS state seeding — the AWS-managed key/alias reference set, materialised per service on first use, with KeyManager=AWS.
Traces: kms.aws-managed-key-reference-catalogue.

## Evidence
Cause 1: kms.asymmetric-decrypt-requires-key-id — "AssertionError: Decrypt without KeyId but WITH EncryptionAlgorithm=RSAES_OAEP_SHA_256 must raise ValidationException 'KeyId must not be null' (the parameter-combination rule is checked before the blob), got ('InvalidCiphertextException', '')". Expected: ValidationException / 'KeyId must not be null'. Got: InvalidCiphertextException with an empty message.

Cause 2: kms.default-key-policy-grants-root-account-access — the default policy's root kms:* statement is present, but PutKeyPolicy(KeyId=<key>, PolicyName='default', Policy=<document granting the caller only kms:GetKeyPolicy/DescribeKey/ScheduleKeyDeletion plus logs.amazonaws.com kms:Decrypt, and NO path to kms:PutKeyPolicy>) returned "{'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}}}". Expected: MalformedPolicyDocumentException, HTTP 400, Message 'The new key policy will not allow you to update the key policy in the future.'

Cause 3: kms.put-key-policy-rejection-leaves-policy-intact — case 1 (malformed JSON '{not json' -> MalformedPolicyDocumentException with an empty message) now passes; case 2 fails: PutKeyPolicy with a document whose second statement names arn:aws:iam::<account>:role/sim2real-behav-kms-no-such-role returned "{'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2' ...}}" — "AssertionError: expected a rejection, but the call SUCCEEDED". Expected: MalformedPolicyDocumentException, HTTP 400, Message 'Policy contains a statement with one or more invalid principals.', stored policy unchanged.

Cause 4: kms.op-schedule-deletion-window-out-of-range-rejected — ScheduleKeyDeletion(KeyId=<enabled symmetric key>, PendingWindowInDays=6): "AssertionError: 6 days -> unexpected message ''". Code ValidationException is correct; expected Message 'PendingWindowInDays must be between 7 and 30' (the same text is expected for PendingWindowInDays=31).

Cause 5: kms.aws-managed-key-reference-catalogue — "AssertionError: no alias/aws/* aliases in ListAliases (0 aliases total) — the AWS-managed key catalogue is reference data every account has; an emulator that knows only customer-created keys fails here". The check fully paginates ListAliases (Marker/Truncated discipline asserted) and sees an empty list.

## Steps to Reproduce
Replay the behavioral suite and re-run the five check ids in trace_ids. Per cause:
1. CreateKey(KeySpec=RSA_2048, KeyUsage=ENCRYPT_DECRYPT); Encrypt(KeyId=<arn>, Plaintext=..., EncryptionAlgorithm='RSAES_OAEP_SHA_256'); then Decrypt(CiphertextBlob=<that blob>, EncryptionAlgorithm='RSAES_OAEP_SHA_256') with NO KeyId — observe InvalidCiphertextException with an empty message instead of ValidationException 'KeyId must not be null'. Contrast with Decrypt(CiphertextBlob=<blob>) alone, which must be InvalidCiphertextException.
2. CreateKey(); PutKeyPolicy(PolicyName='default', Policy=<a document that grants the caller everything except kms:PutKeyPolicy and drops the account-root statement>) with BypassPolicyLockoutSafetyCheck unset — observe HTTP 200.
3. CreateKey(); PutKeyPolicy(PolicyName='default', Policy=<valid JSON keeping the root kms:* statement plus a statement whose Principal.AWS is arn:aws:iam::<account>:role/<a role that does not exist>>) — observe HTTP 200 and GetKeyPolicy showing the new document.
4. CreateKey(); ScheduleKeyDeletion(KeyId=<key>, PendingWindowInDays=6) and again with 31 — inspect Error.Message, which is empty.
5. kms:ListAliases on a fresh account, fully paginated — observe 0 aliases and no alias/aws/* entry; DescribeKey('alias/aws/s3') has nothing to resolve.
