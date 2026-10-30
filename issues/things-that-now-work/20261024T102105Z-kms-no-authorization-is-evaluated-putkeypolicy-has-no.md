Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.decrypt-blocked-without-kms-decrypt-permission kms.grant-token-bypasses-listing-eventual-consistency kms.grant-token-enables-immediate-use-before-propagation kms.grant-token-immediate-use-bypasses-propagation-delay kms.grant-token-immediate-visibility kms.grant-encryption-context-equals-and-subset-are-enforced-differently kms.key-policy-encryption-context-condition-gates-decrypt kms.default-key-policy-grants-root-account-access kms.aws-managed-key-reference-catalogue kms.alias-format-validation kms.put-key-policy-malformed-json-rejected kms.put-key-policy-rejection-leaves-policy-intact kms.op-schedule-deletion-window-out-of-range-rejected kms.sep-kms-key-scope-bit-flipped-ciphertext-never-yields-different-plaintext-rejected kms.sep-kms-key-scope-context-absent-at-encrypt-present-at-decrypt-rejected kms.encryption-context-aad-binding-integrity kms.alias-retarget-does-not-rebind-ciphertext kms.sep-kms-key-scope-cross-key-decrypt-fails-while-keyless-decrypt-names-the-wrapping-key kms.asymmetric-decrypt-requires-key-id kms.rsa-oaep-plaintext-ceiling-by-modulus kms.kms-reencrypt-with-a-mismatched-source-encryption-context-rejected kms.invalid-arn-rejected-across-read-ops kms.list-retirable-grants-invalid-marker-rejected kms.create-custom-key-store-requires-active-cluster-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.decrypt-blocked-without-kms-decrypt-permission|_error#19|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Key: kms.grant-token-bypasses-listing-eventual-consistency|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: kms.grant-token-enables-immediate-use-before-propagation|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: kms.grant-token-immediate-use-bypasses-propagation-delay|run#15|AssertionError|raise AssertionError("Decrypt succeeded for a caller with no key-policy Decrypt grant")
Issue-Key: kms.grant-token-immediate-visibility|_error#13|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: kms.grant-encryption-context-equals-and-subset-are-enforced-differently|run#10|AssertionError|assert blob is None and code == ERR_ACCESS_DENIED, (
Issue-Key: kms.key-policy-encryption-context-condition-gates-decrypt|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: kms.default-key-policy-grants-root-account-access|_retry_put_key_policy_expect_error#17|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Key: kms.aws-managed-key-reference-catalogue|run#4|AssertionError|assert managed, (
Issue-Key: kms.alias-format-validation|run#10|AssertionError|assert got == expected, f"expected {expected!r}, got {got!r}"
Issue-Key: kms.put-key-policy-malformed-json-rejected|run#10|AssertionError|assert message == "", f"expected an empty message, got {message!r}"
Issue-Key: kms.put-key-policy-rejection-leaves-policy-intact|run#14|AssertionError|assert msg == "", f"AWS sends this refusal with no message text, got {msg!r}"
Issue-Key: kms.op-schedule-deletion-window-out-of-range-rejected|run#7|AssertionError|assert low_msg == out_of_range, f"6 days -> unexpected message {low_msg!r}"
Issue-Key: kms.sep-kms-key-scope-bit-flipped-ciphertext-never-yields-different-plaintext-rejected|run#16|AssertionError|assert len(tag_codes) == _TAG_BYTES and bad_tag == {}, (
Issue-Key: kms.sep-kms-key-scope-context-absent-at-encrypt-present-at-decrypt-rejected|run#13|AssertionError|assert code == ("InvalidCiphertextException", ""), (
Issue-Key: kms.encryption-context-aad-binding-integrity|run#11|AssertionError|assert message == MSG_INVALID_CIPHERTEXT, (
Issue-Key: kms.alias-retarget-does-not-rebind-ciphertext|run#27|AssertionError|assert got == (ERR_INCORRECT_KEY, MSG_INCORRECT_KEY), (
Issue-Key: kms.sep-kms-key-scope-cross-key-decrypt-fails-while-keyless-decrypt-names-the-wrapping-key|run#12|AssertionError|assert code_ab == ("IncorrectKeyException", _INCORRECT_KEY_MESSAGE), (
Issue-Key: kms|Encrypt|NotImplemented
Issue-Key: kms.kms-reencrypt-with-a-mismatched-source-encryption-context-rejected|run#14|AssertionError|assert code_wrong == ERR_INVALID_CIPHERTEXT, (
Issue-Key: kms.invalid-arn-rejected-across-read-ops|run#7|AssertionError|assert code_retirable == "InvalidArnException", (
Issue-Key: kms.list-retirable-grants-invalid-marker-rejected|run#5|AssertionError|assert code == "InvalidMarkerException", (
Issue-Key: kms|CreateCluster|AmbiguousService
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# KMS: no authorization is evaluated, PutKeyPolicy has no lockout check, the AWS-managed key catalogue is missing, asymmetric Encrypt/ReEncrypt/ListRetirableGrants are stubs, refusal codes/messages diverge, and 'cloudhsm' is double-registered (9 causes)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Decrypt

## Description
Nine distinct root causes in the KMS emulator, all grounded in failing behavioral checks of this suite. NOTE: checks whose only divergence is 'got NotImplemented' from an operation the report already marked ALREADY FILED (GenerateDataKey, GetKeyLastUsage, GetParametersForImport, RotateKeyOnDemand, CreateCustomKeyStore, UpdateCustomKeyStore, UpdatePrimaryRegion) are deliberately NOT included here; their own stub issues cover them.

Cause 1 of 9 — KMS evaluates no identity policy, key policy or grant on its data plane.
AWS: Encrypt/Decrypt/GenerateDataKey are authorized against the key policy, the caller's identity policy and any grant; a caller with no matching permission is refused AccessDeniedException (HTTP 400) and no plaintext/ciphertext is returned. Smithy: Encrypt/Decrypt carry the service-common AccessDeniedException; DryRun/GrantTokens members exist precisely because authorization is a server-side decision. Recorded behavior: the negative control of every grant-token check is an AccessDeniedException for a principal without the grant.
Emulator: every negative control SUCCEEDS with HTTP 200. Decrypt returns Plaintext for a role whose policy does not allow kms:Decrypt; Encrypt returns a CiphertextBlob for a bare role with no policy at all (check reports 'got code None'); a Decrypt with no key-policy Decrypt grant succeeds. The key-policy encryption-context-condition check never converges and hits the 60 s cap because the refusal it polls for never appears.
Why: a client cannot test or demonstrate any least-privilege KMS design against the emulator — every deny-path test passes where production refuses, so the emulator hides exactly the failures it exists to surface, and grant/grant-token semantics (propagation, token bypass, EncryptionContextEquals vs Subset) are untestable.
Fix: an authorization gate in the KMS request pipeline (key-policy + identity-policy + grant evaluation) ahead of the Encrypt/Decrypt/GenerateDataKey handlers.
Traces: kms.decrypt-blocked-without-kms-decrypt-permission, kms.grant-token-bypasses-listing-eventual-consistency, kms.grant-token-enables-immediate-use-before-propagation, kms.grant-token-immediate-use-bypasses-propagation-delay, kms.grant-token-immediate-visibility, kms.grant-encryption-context-equals-and-subset-are-enforced-differently, kms.key-policy-encryption-context-condition-gates-decrypt (60 s timeout).

Cause 2 of 9 — PutKeyPolicy applies no policy-lockout safety check.
AWS: a replacement key policy that leaves no principal a path to kms:PutKeyPolicy is refused with MalformedPolicyDocumentException (HTTP 400) and the stored policy is left untouched, unless BypassPolicyLockoutSafetyCheck is set. Smithy: PutKeyPolicy errors include MalformedPolicyDocumentException (httpError 400) and the input has BypassPolicyLockoutSafetyCheck with default=false — the safety check is the modeled default.
Emulator: PutKeyPolicy with a lockout policy and no bypass returns HTTP 200 and stores it.
Why: a stack can irrecoverably lock itself out of a key in the emulator, and the guardrail AWS users rely on is unverifiable.
Fix: PutKeyPolicy's validation path (lockout evaluation honouring BypassPolicyLockoutSafetyCheck).
Traces: kms.default-key-policy-grants-root-account-access.

Cause 3 of 9 — the AWS-managed key catalogue does not exist.
AWS: every account has AWS-managed keys with alias/aws/* aliases (alias/aws/s3, alias/aws/secretsmanager, alias/aws/dynamodb, ...), visible in ListAliases, each with KeyManager=AWS. Smithy: ListAliases returns AliasList; DescribeKey's KeyMetadata.KeyManager distinguishes AWS from CUSTOMER.
Emulator: ListAliases returns 0 aliases and no alias/aws/* entry exists at all.
Why: every service integration that defaults to an AWS-managed key (S3 aws:kms default, Secrets Manager rekey to alias/aws/secretsmanager, DynamoDB alias/aws/dynamodb) has nothing to resolve to, so those paths cannot work; this cause is upstream of several cross-service failures in this suite.
Fix: KMS state seeding — reference data for AWS-managed keys and their aliases, created on demand per service.
Traces: kms.aws-managed-key-reference-catalogue.

Cause 4 of 9 — CreateAlias rejects a name without the 'alias/' prefix under the wrong error code.
AWS (recorded behavior only — the model does not state which fault applies to a missing prefix): the refusal is ValidationException. Message example: 'Alias must start with the prefix "alias/". Please see https://docs.aws.amazon.com/kms/latest/developerguide/kms-alias.html'.
Emulator: InvalidAliasNameException (Smithy: modeled on CreateAlias, httpError 400) with its own message text.
Why: client code that branches on ValidationException for malformed alias input takes the wrong branch. (The message itself is an example, not the requirement; the error CODE is.)
Fix: CreateAlias input validation / error classification.
Traces: kms.alias-format-validation.

Cause 5 of 9 — refusal message contracts diverge: refusals AWS sends with NO message text carry emulator-authored text, and two documented constant messages are replaced with emulator prose.
AWS: the recordings measure that KMS sends these refusals with an EMPTY Message — PutKeyPolicy on malformed JSON (MalformedPolicyDocumentException), ScheduleKeyDeletion with PendingWindowInDays out of range, Decrypt of a tampered ciphertext or a mismatched encryption context (InvalidCiphertextException). For IncorrectKeyException the documented constant is 'The key ID in the request does not identify a CMK that can perform this operation.'
Emulator: each carries a generated message instead — e.g. 'The key policy is not valid JSON: EOF while parsing an object at line 1 column 62.', 'PendingWindowInDays must be between 7 and 30, inclusive, but was 6', the long 'The request was rejected because the specified ciphertext, or additional authenticated data ... is corrupted, missing, or otherwise invalid.' for InvalidCiphertextException, and 'The key <arn> cannot decrypt the ciphertext, which was encrypted by <arn>.' for IncorrectKeyException. The error CODES and statuses are correct in every one of these.
Why: the presence/absence of Message is a structural property of the response these checks measure; emulator-authored text also leaks internal parser positions and key ARNs that AWS does not disclose.
Fix: the KMS error serializer — do not synthesize a Message where the measured contract is an empty one; use the documented constant where AWS publishes one.
Traces: kms.put-key-policy-malformed-json-rejected, kms.put-key-policy-rejection-leaves-policy-intact, kms.op-schedule-deletion-window-out-of-range-rejected, kms.sep-kms-key-scope-bit-flipped-ciphertext-never-yields-different-plaintext-rejected, kms.sep-kms-key-scope-context-absent-at-encrypt-present-at-decrypt-rejected, kms.encryption-context-aad-binding-integrity, kms.alias-retarget-does-not-rebind-ciphertext, kms.sep-kms-key-scope-cross-key-decrypt-fails-while-keyless-decrypt-names-the-wrapping-key.

Cause 6 of 9 — Encrypt/Decrypt with an asymmetric KMS key is an unimplemented scaffold branch.
AWS: Encrypt against an RSA key with EncryptionAlgorithm RSAES_OAEP_SHA_256 succeeds up to the modulus-derived plaintext ceiling and fails above it; Decrypt of an asymmetric ciphertext requires KeyId. Smithy: Encrypt.EncryptionAlgorithm is EncryptionAlgorithmSpec (includes the RSAES_OAEP_* values) and Encrypt's modeled faults are InvalidKeyUsageException/DisabledException/NotFoundException — never NotImplemented.
Emulator: HTTP 501 'operation Encrypt with an asymmetric KMS key is scaffolded but not implemented in this emulator'.
Why: no asymmetric encryption path can be exercised at all; the modulus ceiling and the keyless-Decrypt rule are unreachable.
Fix: the Encrypt/Decrypt handlers' asymmetric branch.
Traces: kms.asymmetric-decrypt-requires-key-id, kms.rsa-oaep-plaintext-ceiling-by-modulus.

Cause 7 of 9 — ReEncrypt is an unimplemented scaffold stub.
AWS: ReEncrypt with a SourceEncryptionContext that does not match the one bound into the ciphertext is refused InvalidCiphertextException (Smithy: ReEncrypt errors include InvalidCiphertextException httpError 400, plus IncorrectKeyException/InvalidKeyUsageException).
Emulator: NotImplemented (HTTP 501).
Why: envelope re-wrapping and every ReEncrypt AAD-binding guarantee are unavailable.
Fix: services/service-com-amazonaws-kms-trentservice/src/ops/re_encrypt.rs.
Traces: kms.kms-reencrypt-with-a-mismatched-source-encryption-context-rejected.

Cause 8 of 9 — ListRetirableGrants is an unimplemented scaffold stub.
AWS: ListRetirableGrants rejects a structurally malformed RetiringPrincipal ARN with InvalidArnException and a corrupted Marker with InvalidMarkerException (Smithy: both modeled on the operation, httpError 400; Marker has pattern ^[\u0020-\u00FF]*$ and length 1..1024).
Emulator: NotImplemented (HTTP 501) for both.
Why: grant inventory by retiring principal is unavailable and its two input-validation contracts cannot be met.
Fix: services/service-com-amazonaws-kms-trentservice/src/ops/list_retirable_grants.rs.
Traces: kms.invalid-arn-rejected-across-read-ops, kms.list-retirable-grants-invalid-marker-rejected.

Cause 9 of 9 — two emulated services both claim the 'cloudhsm' endpoint, so every CloudHSM call is refused by the router.
AWS: cloudhsm CreateCluster is served normally; a KMS custom key store over an inactive cluster is then refused CloudHsmClusterNotActiveException.
Emulator: CreateCluster never reaches a handler — 'An error occurred (AmbiguousService) when calling the CreateCluster operation (reached max retries: 3): 2 emulated services all claim requests for "cloudhsm": cloudhsm, cloudhsm; the request is rejected rather than served by an arbitrary one'. This is a service-registration defect, not a fidelity one: the duplicate registration makes the whole cloudhsm surface unreachable.
Why: no CloudHSM-backed flow (including every KMS custom key store scenario) can run.
Fix: the emulator's service registry / endpoint-prefix routing table (two crates register the same 'cloudhsm' prefix: service-com-amazonaws-cloudhsm-cloudhsmfrontendservice and service-com-amazonaws-cloudhsmv2-baldrapiservice are both bootstrapped).
Traces: kms.create-custom-key-store-requires-active-cluster-rejected.

## Evidence
Cause 1: kms.decrypt-blocked-without-kms-decrypt-permission — 'AssertionError: expected a ClientError, but the call succeeded with {KeyId: arn:aws:kms:us-west-2:...:key/74aca4e6..., Plaintext: b"sim2real-behavioral-secret", ... HTTPStatusCode: 200}'. Expected: AccessDeniedException, HTTP 400. kms.grant-token-immediate-visibility / -bypasses-listing-eventual-consistency / -enables-immediate-use-before-propagation — 'expected a ClientError, but the call SUCCEEDED and returned {CiphertextBlob: ..., HTTPStatusCode: 200}'. kms.grant-token-immediate-use-bypasses-propagation-delay — 'Decrypt succeeded for a caller with no key-policy Decrypt grant'. kms.grant-encryption-context-equals-and-subset-are-enforced-differently — 'a bare IAM role with no policy must be refused Encrypt on an ungranted key with AccessDeniedException ...; got code None'. kms.key-policy-encryption-context-condition-gates-decrypt — 'Check timed out after 60.0s'.
Cause 2: kms.default-key-policy-grants-root-account-access — PutKeyPolicy(KeyId, PolicyName='default', Policy=<locked-out policy>) returned 'HTTPStatusCode: 200, content-length: 2'; expected MalformedPolicyDocumentException, HTTP 400.
Cause 3: kms.aws-managed-key-reference-catalogue — 'no alias/aws/* aliases in ListAliases (0 aliases total)'.
Cause 4: kms.alias-format-validation — "expected ('ValidationException', 'Alias must start with the prefix \"alias/\"...'), got ('InvalidAliasNameException', 'Alias must start with the prefix \"alias/\". Found: sim2real-behav-noprefix-d4181f9d561f')".
Cause 5: kms.put-key-policy-malformed-json-rejected — "expected an empty message, got 'The key policy is not valid JSON: EOF while parsing an object at line 1 column 62.'"; kms.put-key-policy-rejection-leaves-policy-intact — "AWS sends this refusal with no message text, got 'The key policy is not valid JSON: key must be a string at line 1 column 2.'"; kms.op-schedule-deletion-window-out-of-range-rejected — "6 days -> unexpected message 'PendingWindowInDays must be between 7 and 30, inclusive, but was 6'"; kms.sep-kms-key-scope-bit-flipped-ciphertext... — 'every flip in the trailing 16 authentication bytes must raise InvalidCiphertextException with an empty message; offsets that did not: {86: (InvalidCiphertextException, "The request was rejected because the specified ciphertext ..."), 87: ..., ...}'; kms.alias-retarget-does-not-rebind-ciphertext — expected IncorrectKeyException 'The key ID in the request does not identify a CMK that can perform this operation.', got IncorrectKeyException 'The key alias/... cannot decrypt the ciphertext, which was encrypted by arn:aws:kms:...:key/c0afd499...'.
Cause 6: kms.asymmetric-decrypt-requires-key-id and kms.rsa-oaep-plaintext-ceiling-by-modulus — 'An error occurred (NotImplemented) when calling the Encrypt operation: operation Encrypt with an asymmetric KMS key is scaffolded but not implemented in this emulator'.
Cause 7: kms.kms-reencrypt-with-a-mismatched-source-encryption-context-rejected — "ReEncrypt with SourceEncryptionContext {'stage': 'source', 'tenant': 'evil'} instead of {'stage': 'source', 'tenant': 'acme'} must raise InvalidCiphertextException, got 'NotImplemented'".
Cause 8: kms.invalid-arn-rejected-across-read-ops — "ListRetirableGrants with a structurally malformed principal ARN returned 'NotImplemented', expected InvalidArnException"; kms.list-retirable-grants-invalid-marker-rejected — "expected InvalidMarkerException for a corrupted Marker, got 'NotImplemented'".
Cause 9: kms.create-custom-key-store-requires-active-cluster-rejected — 'An error occurred (AmbiguousService) when calling the CreateCluster operation (reached max retries: 3): 2 emulated services all claim requests for "cloudhsm": cloudhsm, cloudhsm; the request is rejected rather than served by an arbitrary one'.

## Steps to Reproduce
Replay the behavioral suite and re-run the listed check ids. Per cause:
1. CreateKey; assume/scope a principal with no kms:Decrypt; Encrypt then Decrypt as that principal — observe 200 + Plaintext instead of AccessDeniedException.
2. CreateKey; PutKeyPolicy with a policy granting nobody kms:PutKeyPolicy and BypassPolicyLockoutSafetyCheck unset — observe 200.
3. kms:ListAliases on a fresh account — observe 0 aliases, no alias/aws/*.
4. CreateAlias(AliasName='no-prefix-name', TargetKeyId=<key>) — observe InvalidAliasNameException instead of ValidationException.
5. PutKeyPolicy with malformed JSON; ScheduleKeyDeletion(PendingWindowInDays=6); Decrypt a bit-flipped ciphertext — inspect Error.Message, which must be empty.
6. CreateKey(KeySpec=RSA_2048, KeyUsage=ENCRYPT_DECRYPT); Encrypt(EncryptionAlgorithm='RSAES_OAEP_SHA_256') — observe 501 NotImplemented.
7. Encrypt with an EncryptionContext, then ReEncrypt with a different SourceEncryptionContext — observe 501 NotImplemented.
8. ListRetirableGrants(RetiringPrincipal='<malformed arn>') and ListRetirableGrants(Marker='<corrupted>') — observe 501 NotImplemented.
9. cloudhsmv2 CreateCluster(...) against the emulator — observe AmbiguousService for 'cloudhsm'.
