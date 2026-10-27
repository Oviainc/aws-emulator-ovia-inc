Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguru-security.update-account-configuration-kms-round-trip codeguru-security.update-account-configuration-nonexistent-kms-key-rejected codeguru-security.update-account-configuration-invalid-kms-pattern-invalid codeguru-security.tag-resource-unknown-arn-internal-error codeguru-security.untag-resource-unknown-arn-internal-error codeguru-security.list-tags-for-resource-rejected-feature-unavailable
Issue-Shape-Id: com.amazonaws.codegurusecurity#AwsCodeGuruSecurity
Issue-Key: codeguru-security|UpdateAccountConfiguration|ResourceNotFoundException
Issue-Key: codeguru-security.update-account-configuration-nonexistent-kms-key-rejected|run#8|AssertionError|assert message == NO_CONFIG_MESSAGE, f"expected {NO_CONFIG_MESSAGE!r}, got {message!r}"
Issue-Key: codeguru-security.update-account-configuration-invalid-kms-pattern-invalid|run#8|AssertionError|assert control_message == NO_CONFIG_MESSAGE, (
Issue-Key: codeguru-security.tag-resource-unknown-arn-internal-error|run#10|AssertionError|assert code == "InternalServerErrorException", f"CODE={code!r}"
Issue-Key: codeguru-security.untag-resource-unknown-arn-internal-error|run#10|AssertionError|assert code == "InternalServerErrorException", f"CODE={code!r}"
Issue-Key: codeguru-security.list-tags-for-resource-rejected-feature-unavailable|run#9|AssertionError|assert code2 == "InternalServerErrorException", f"CODE2={code2!r}"
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# CodeGuru Security: UpdateAccountConfiguration 404s every call and never resolves the KMS key; scan tag ops report 404 not-found where AWS 500s

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccountConfiguration

## Description
Three distinct root causes in the codeguru-security emulator, found by replaying the behavioral suite. All three live in this one service; they are numbered so each is closed on its own.

--- Cause 1 of 3: UpdateAccountConfiguration rejects EVERY call on an account that has no stored configuration record, so the happy path is unreachable. Failing check: codeguru-security.update-account-configuration-kms-round-trip.
AWS: UpdateAccountConfiguration (Smithy: PUT /updateAccountConfiguration, input EncryptionConfig#kmsKeyArn @length 1..2048 @pattern ^arn:aws:kms:[\S]+:[\d]{12}:key\/((uuid)|(mrk-...))$, output encryptionConfig required; modeled errors AccessDenied 403 / InternalServer 500 / ResourceNotFound 404 / Throttling 429 / Validation 400). Recorded behavior: a real, Enabled, symmetric customer-managed CMK is ACCEPTED (HTTP 200) and the switch is independently observable through GetAccountConfiguration, which then reports encryptionConfig.kmsKeyArn equal to the submitted key ARN; reverting to the account's original encryptionConfig is likewise accepted and observable.
Emulator: the call fails with ResourceNotFoundException (HTTP 404) regardless of the key, e.g. message 'no existing account configuration found for account <acct>: CodeGuru Security holds no configuration for this account, so there is none to update' (text quoted as an example only). This happens even though GetAccountConfiguration in the same session answers 200 with an encryptionConfig — the read path reports a configuration the write path says does not exist.
Why: a client that sets or clears its CodeGuru Security encryption key can never succeed against this emulator; every configuration write is a hard 404, and the value GetAccountConfiguration reports can never be changed. The read/write disagreement also makes the emulator self-inconsistent.
Fix: the UpdateAccountConfiguration handler and the account-configuration state it reads — the operation must apply the requested encryptionConfig (creating/onboarding the account's configuration record if the read path already behaves as if one exists) and return it, rather than gating on a pre-existing record.
Traces: codeguru-security.update-account-configuration-kms-round-trip (services/service-com-amazonaws-codegurusecurity-awscodegurusecurity/src/ops/update_account_configuration.rs lines 193-210 hold the unconditional refusal).

--- Cause 2 of 3: UpdateAccountConfiguration never resolves kmsKeyArn against KMS, so a well-formed but nonexistent key is not rejected for being absent — the 404 it currently returns comes from the unrelated condition in Cause 1 and will disappear once Cause 1 is fixed, leaving the call overly permissive. Failing checks: codeguru-security.update-account-configuration-nonexistent-kms-key-rejected, codeguru-security.update-account-configuration-invalid-kms-pattern-invalid.
AWS: recorded behavior only — a syntactically valid key ARN (arn:aws:kms:<region>:<acct>:key/00000000-0000-0000-0000-000000000000) that names no key in the account is REJECTED with ResourceNotFoundException (HTTP 404, recorded message 'No existing account configuration found.' — the text is an example, the 404 + ResourceNotFoundException code are the requirement), and the account's encryptionConfig is left untouched. The model lists ResourceNotFoundException (404) for this operation but documents no reason text.
Emulator: the handler reads only input.encryption_config.kms_key_arn and writes it; it never checks that the key exists. The 404 observed today is emitted before any key resolution, for 'no configuration record' (see Cause 1), and the recorded diverging detail is the message: expected 'No existing account configuration found.', got 'no existing account configuration found for account 000000000000: CodeGuru Security holds no configuration for this account, so there is none to update'. The rejection is therefore for the wrong reason and is not conditioned on the key at all.
Why: once Cause 1 is fixed naively, a client pointing its account at a nonexistent or deleted CMK gets HTTP 200 and a stored configuration real AWS would never accept — the emulator silently accepts an unusable encryption key, and tests that assert the rejection pass today only by accident.
Fix: UpdateAccountConfiguration's validation — resolve the submitted kmsKeyArn against the KMS state before committing, and reject an absent key with ResourceNotFoundException (404) while leaving the stored encryptionConfig unchanged; keep the Cause-1 gating out of the key-existence path so the two rejections are distinguishable.
Note: check update-account-configuration-invalid-kms-pattern-invalid also asserts that a malformed kmsKeyArn ('not-a-valid-kms-arn') is rejected with ValidationException (400) carrying the standard constraint message naming 'encryptionConfig.kmsKeyArn' and the EncryptionConfig#kmsKeyArn @pattern (a request-derived message, so that rule IS a requirement). That assertion was never reached because the control assertion above it failed first; it must be re-replayed after this cause is fixed.
Traces: codeguru-security.update-account-configuration-nonexistent-kms-key-rejected, codeguru-security.update-account-configuration-invalid-kms-pattern-invalid.

--- Cause 3 of 3: the scan tagging surface reports a clean not-found for a well-formed but unknown scan ARN, where this service answers with a server error. Failing checks: codeguru-security.tag-resource-unknown-arn-internal-error, codeguru-security.untag-resource-unknown-arn-internal-error, codeguru-security.list-tags-for-resource-rejected-feature-unavailable.
AWS: recorded behavior only. For a resourceArn that satisfies ScanNameArn (@length 1..300, @pattern ^arn:aws:codeguru-security:[\S]+:[\d]{12}:scans\/[a-zA-Z0-9-_$:.]*$) but names no existing scan, TagResource, UntagResource and ListTagsForResource all fail with error code InternalServerErrorException at HTTP 500 (recorded message 'Internal server error' — an example), not with the ResourceNotFoundException (404) the model lists: the scan-lookup backing store errors instead of reporting not-found. Note the recorded code InternalServerErrorException is not even a shape in this service's model (the model names InternalServerException, server/500/retryable), so the wire error code must be emitted as recorded. The malformed-ARN control (resourceArn='not-an-arn-at-all' -> BadRequestException 400) already matches and is unaffected.
Emulator: all three operations look the scan up in state and, when absent, return ResourceNotFoundException (HTTP 404) — e.g. tag_resource.rs returns ResourceNotFoundException with 'no scan of this account is identified by <arn>'. Replay shows CODE='ResourceNotFoundException' where 'InternalServerErrorException' was expected, on all three operations.
Why: a client distinguishes a retryable server fault (5xx, retried by the SDK's standard retry policy) from a terminal client error (4xx, not retried). The emulator turns a retryable 500 into a terminal 404, so SDK retry/backoff paths and any error handling keyed on InternalServerErrorException are never exercised, and code that treats 404 as 'resource confirmed absent' draws a conclusion real AWS does not support (the read path cannot confirm absence here at all).
Fix: the shared scan-ARN lookup used by the TagResource / UntagResource / ListTagsForResource handlers (services/service-com-amazonaws-codegurusecurity-awscodegurusecurity/src/ops/tag_resource.rs, untag_resource.rs, list_tags_for_resource.rs) and its error channel — a well-formed ARN with no matching scan must surface the 500 InternalServerErrorException, while malformed ARNs keep the existing 400 BadRequestException boundary refusal.
Traces: codeguru-security.tag-resource-unknown-arn-internal-error, codeguru-security.untag-resource-unknown-arn-internal-error, codeguru-security.list-tags-for-resource-rejected-feature-unavailable (tag_resource.rs lines 171-184 is the representative not-found return).

## Evidence
Suite: 0/7 codeguru-security checks passed.

Cause 1 — codeguru-security.update-account-configuration-kms-round-trip [error]:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the UpdateAccountConfiguration operation: no existing account configuration found for account 000000000000: CodeGuru Security holds no configuration for this account, so there is none to update
  Expected: the call succeeds for an Enabled symmetric CMK and GetAccountConfiguration then reports encryptionConfig.kmsKeyArn == that key ARN.

Cause 2 — codeguru-security.update-account-configuration-nonexistent-kms-key-rejected [mismatch]:
  AssertionError: expected 'No existing account configuration found.', got 'no existing account configuration found for account 000000000000: CodeGuru Security holds no configuration for this account, so there is none to update'
  (code ResourceNotFoundException matched, but was produced by the unconditional no-config gate, not by resolving the key)
  codeguru-security.update-account-configuration-invalid-kms-pattern-invalid [mismatch]: same control assertion; the subsequent ValidationException/pattern assertions were never reached.

Cause 3 —
  codeguru-security.tag-resource-unknown-arn-internal-error [mismatch]: AssertionError: CODE='ResourceNotFoundException' (expected 'InternalServerErrorException', HTTP 500)
  codeguru-security.untag-resource-unknown-arn-internal-error [mismatch]: AssertionError: CODE='ResourceNotFoundException'
  codeguru-security.list-tags-for-resource-rejected-feature-unavailable [mismatch]: AssertionError: CODE2='ResourceNotFoundException'
  In all three, the malformed-ARN control (BadRequestException 400, 'Invalid input resource arn: bad syntax') passed, so the divergence is specific to the well-formed-but-unknown scan ARN.

## Steps to Reproduce
1. Boot the emulator and point a botocore codeguru-security client at it.
2. Cause 1: csec.get_account_configuration() -> succeeds with encryptionConfig. Create an Enabled SYMMETRIC_DEFAULT CMK via kms.create_key, then csec.update_account_configuration(encryptionConfig={'kmsKeyArn': <key arn>}) -> observe ResourceNotFoundException (404) instead of 200 + a GetAccountConfiguration that reflects the key.
3. Cause 2: csec.update_account_configuration(encryptionConfig={'kmsKeyArn': 'arn:aws:kms:<region>:<acct>:key/00000000-0000-0000-0000-000000000000'}) -> observe the rejection is emitted by the no-config gate (message naming the account configuration), not by resolving the key; after Cause 1 is fixed, the same call will be ACCEPTED. Also csec.update_account_configuration(encryptionConfig={'kmsKeyArn': 'not-a-valid-kms-arn'}) must give ValidationException (400) naming encryptionConfig.kmsKeyArn and the EncryptionConfig#kmsKeyArn pattern, with the stored config unchanged.
4. Cause 3: with no scan created, call csec.tag_resource(resourceArn='arn:aws:codeguru-security:<region>:<acct>:scans/nosuchscan', tags={'Environment':'test'}), csec.untag_resource(resourceArn=<same>, tagKeys=['Environment']) and csec.list_tags_for_resource(resourceArn=<same>) -> each returns ResourceNotFoundException (404) instead of InternalServerErrorException (500). Control: the same calls with resourceArn='not-an-arn-at-all' correctly return BadRequestException (400).
