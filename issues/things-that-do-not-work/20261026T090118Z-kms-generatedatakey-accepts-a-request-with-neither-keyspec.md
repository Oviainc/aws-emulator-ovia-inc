Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.generate-data-key-spec-and-bytes-exclusivity kms.import-key-material-token-single-use-rejected kms.import-token-ttl-window-enforced kms.imported-rsa-private-key-signs-identically-to-the-local-key-it-came-from kms.invalid-arn-consistent-across-metadata-ops kms.sep-kms-key-scope-context-map-is-order-independent-on-decrypt kms.sep-kms-key-scope-identical-imported-material-does-not-make-keys-interchangeable-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.generate-data-key-spec-and-bytes-exclusivity|_err_code#11|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: kms.import-key-material-token-single-use-rejected|run#21|AssertionError|assert len(successes) == 1, f"expected exactly 1 success, got {len(successes)}: {outcomes}"
Issue-Key: kms.import-token-ttl-window-enforced|run#17|AssertionError|assert message == "", f"expected an empty InvalidImportTokenException message, got {message!r}"
Issue-Key: kms|ImportKeyMaterial|NotImplemented
Issue-Key: kms.invalid-arn-consistent-across-metadata-ops|run#13|AssertionError|assert message_last_usage == _denied("GetKeyLastUsage"), message_last_usage
Issue-Key: kms.sep-kms-key-scope-context-map-is-order-independent-on-decrypt|run#17|AssertionError|assert code == ("InvalidCiphertextException", ""), (
Issue-Key: kms.sep-kms-key-scope-identical-imported-material-does-not-make-keys-interchangeable-rejected|run#13|AssertionError|assert code_ab == ("IncorrectKeyException", _INCORRECT_KEY_MESSAGE), (
Issue-Commit: bd2652b5ddba3816e4efa759e4736f1b7b87af71

# KMS: GenerateDataKey accepts a request with neither KeySpec nor NumberOfBytes, an import token is replayable, an asymmetric ImportKeyMaterial branch is a stub, and four refusal message templates diverge (7 causes)

> _Note: this issue was identified against the code base at commit `bd2652b5ddba3816e4efa759e4736f1b7b87af71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateDataKey

## Description
Seven independent root causes in the KMS crate, all grounded in failing behavioral checks that are neither blocked by a scaffold stub nor carried by an open issue's replay key. Causes 1-2 are OVERLY PERMISSIVE (the emulator returns 200 where AWS refuses); causes 3, 5, 6 and 7 are refusal-message-template divergences; cause 4 is a per-key-spec unimplemented branch of an operation that is otherwise implemented.

Cause 1 of 7: GenerateDataKey accepts a request that supplies NEITHER KeySpec nor NumberOfBytes and silently defaults to a 32-byte key.
AWS:      Smithy: GenerateDataKey.KeyId is required (KeyIdType, @length 1..2048); KeySpec (DataKeySpec enum) and NumberOfBytes (NumberOfBytesType, @range 1..1024) are BOTH optional in the model, so the service — not the model — enforces that exactly one of them must be present. Modeled errors are DisabledException(409, awsQueryError Disabled), KMSInvalidStateException(409), NotFoundException(404, NotFound), InvalidGrantTokenException(400), InvalidKeyUsageException(400), KeyUnavailableException(500), DependencyTimeoutException(503), DryRunOperationException(412) — none of which covers request validation. Recorded behavior only: a GenerateDataKey carrying neither member is refused with Error.Code ValidationException (HTTP 400), the service's generic request-validation refusal (ValidationException is not a modeled KMS error shape; the corpus already holds a replay-passing golden for it on CreateAlias). The message for this class is AWS's request-validation text naming the offending members: its fixed text is the validation wording, and its variable parts are the member names (KeySpec / NumberOfBytes) and, for a range violation, the rejected value — all taken from the request. No single run's literal value is part of the requirement.
Emulator: GenerateDataKey(KeyId=<key ARN>) with neither member returns HTTP 200 and a complete payload (CiphertextBlob, Plaintext of 32 bytes, KeyId, KeyMaterialId), i.e. it defaults to AES_256 instead of refusing. The both-members-supplied leg of the same check already passes (the emulator does return ValidationException there), so the defect is specifically the "neither supplied" branch. Because the check aborts at that assertion, three further legs of the same rule were never exercised and must be verified by the fix: NumberOfBytes=1025 (above the modeled max of 1024) on GenerateDataKey, and both the both-supplied and neither-supplied cases on GenerateDataKeyWithoutPlaintext (same optional KeySpec/NumberOfBytes pair, same @range 1..1024).
Why:      Envelope encryption is the dominant KMS usage pattern and every SDK encryption client is built on GenerateDataKey. A caller that omits the key-size parameter entirely develops successfully against the emulator and then fails in production on a ValidationException it never saw; conversely a caller that branches on that refusal can never reach the branch.
Fix:      Request validation in the GenerateDataKey handler and the matching validation in the GenerateDataKeyWithoutPlaintext handler, in the KMS crate.
Traces:   kms.generate-data-key-spec-and-bytes-exclusivity (asserts at check_kms_generate_data_key_spec_and_bytes_exclusivity.py:86).

Cause 2 of 7: an ImportKeyMaterial {ImportToken, EncryptedKeyMaterial} pair is replayable — the same pair imports twice, both times with HTTP 200.
AWS:      Smithy: ImportKeyMaterial requires KeyId, ImportToken (CiphertextType blob, @length 1..6144) and EncryptedKeyMaterial (CiphertextType blob, @length 1..6144), and models IncorrectKeyMaterialException (client, httpError 400, awsQueryError IncorrectKeyMaterialException), ExpiredImportTokenException(400), InvalidImportTokenException(400), InvalidCiphertextException(400), UnsupportedOperationException(400), KMSInvalidStateException(409), NotFoundException(404). Recorded behavior: a single GetParametersForImport pair is SINGLE-USE for ImportType=NEW_KEY_MATERIAL — two back-to-back ImportKeyMaterial calls reusing the identical (ImportToken, EncryptedKeyMaterial) pair yield exactly ONE success and ONE IncorrectKeyMaterialException, with no duplicate PENDING_ROTATION material created, and the key stays manageable via a fresh legitimate reimport. The message AWS words for this error class is fixed text with no variable parts: "expected new key material but the key material being imported is already associated with the KMS key".
Emulator: BOTH calls succeed with HTTP 200 and return the identical KeyId and KeyMaterialId; zero refusals are produced, so the token is consumed by neither call.
Why:      A client driving on-demand rotation by importing new material cannot detect an accidental double-import; a retried request that AWS would reject as a duplicate instead appears to install material twice, hiding exactly the bug the single-use rule exists to surface, and the key-material identity a caller audits (KeyMaterialId) is indistinguishable between a genuine new import and a replay.
Fix:      ImportKeyMaterial's import-token consumption and key-material-identity precondition, plus the PENDING_ROTATION material state it writes, in the KMS crate.
Traces:   kms.import-key-material-token-single-use-rejected (asserts at check_kms_import_key_material_token_single_use_rejected.py:112 — "expected exactly 1 success, got 2").

Cause 3 of 7: InvalidImportTokenException is returned with an invented message where AWS returns an empty one.
AWS:      Smithy: ImportKeyMaterial models InvalidImportTokenException (client, httpError 400, awsQueryError InvalidImportTokenException). Recorded behavior: an ImportToken issued by GetParametersForImport for key A, presented to ImportKeyMaterial targeting a DIFFERENT EXTERNAL key B, is refused with Error.Code InvalidImportTokenException, HTTP 400, and an EMPTY Error.Message — the token/key binding carries no diagnostic text at all. There are no variable parts because there is no text.
Emulator: the same call returns InvalidImportTokenException with Error.Message "The import token is not valid for <key ARN>. Call GetParametersForImport for a public key and import token to use with it." — an invented template that also echoes the target key's ARN back to the caller.
Why:      A client that logs or string-matches the refusal sees text AWS never emits, and the emulator teaches a diagnostic the real service does not provide; the echoed ARN additionally differs from AWS's deliberately information-free refusal on this path.
Fix:      The InvalidImportTokenException construction in the ImportKeyMaterial handler / the KMS error serializer, in the KMS crate.
Traces:   kms.import-token-ttl-window-enforced (asserts at check_kms_import_token_ttl_window_enforced.py:88). The same check also pins the declared 24-hour ParametersValidTo window; that leg was not reached.

Cause 4 of 7: ImportKeyMaterial has no implementation for an asymmetric (RSA_2048) EXTERNAL-origin key, although the symmetric branch of the same operation works.
AWS:      Smithy: ImportKeyMaterial's KeyId is an unconstrained KeyIdType and EncryptedKeyMaterial is a CiphertextType blob @length 1..6144 — nothing in the model restricts the key spec, and GetParametersForImport supports RSA_AES_KEY_WRAP_SHA_256, documented as "Supported for wrapping RSA and ECC key material". So an RSA_2048 / SIGN_VERIFY CMK created with Origin=EXTERNAL must accept RSA-AES-wrapped PKCS#8 private-key material and then be usable for Sign, producing signatures identical to the local private key the material came from.
Emulator: the call is refused with Error.Code NotImplemented and message "operation ImportKeyMaterial for a RSA_2048 KMS key is scaffolded but not implemented in this emulator". This is a per-key-spec hole, not a whole-operation stub: cause 2 above shows ImportKeyMaterial answering 200 for SYMMETRIC_DEFAULT material, so no "implement the stub" issue exists for it and the replay report did not mark this check as stub-blocked.
Why:      Importing an externally generated RSA private key (BYOK for signing) is unavailable entirely; a client gets an unrecognized error code on a modeled, documented path and cannot develop or test any key-import signing flow.
Fix:      The asymmetric/RSA key-material branch of the ImportKeyMaterial handler (and the RSA_AES_KEY_WRAP unwrap it needs), in the KMS crate.
Traces:   kms.imported-rsa-private-key-signs-identically-to-the-local-key-it-came-from. Note this check is an unblessed DRAFT whose RSA_AES wrapping construction and byte order are still hypotheses; the NotImplemented refusal recorded here is independent of those hypotheses, since the emulator refuses the operation before any payload interpretation.

Cause 5 of 7: the AccessDeniedException message for a malformed-account key ARN uses the wrong template and echoes the rejected resource.
AWS:      Smithy: GetKeyLastUsage.KeyId is KeyIdType @length 1..2048 with NO pattern, and the operation models InvalidArnException(400), NotFoundException(404, NotFound), KMSInternalException(500), DependencyTimeoutException(503) — an authorization refusal surfaces as the service-common AccessDeniedException. Recorded behavior: a key ARN carrying the real region/resource-type/key-id but an account-id segment that is not the required 12-digit numeric shape is refused CONSISTENTLY by GetKeyLastUsage, ListKeyPolicies and UpdatePrimaryRegion with Error.Code AccessDeniedException and the message template:
            "User: <caller ARN> is not authorized to perform: kms:<Action> on this resource because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access"
          Fixed text: everything except the two variable parts — <caller ARN>, taken from the caller's identity (sts:GetCallerIdentity), and <Action>, the operation name. Notably AWS says "on this resource" and NEVER echoes the rejected ARN, because authorization is evaluated before any ARN-content validation so existence must not leak.
Emulator: the same call returns AccessDeniedException with "User: <caller ARN> is not authorized to perform: kms:GetKeyLastUsage on resource: <the malformed key ARN> because no resource-based policy allows the kms:GetKeyLastUsage action" — a different fixed text ("on resource: <arn> ... because no resource-based policy allows the <action> action") that also interpolates the rejected resource ARN.
Why:      A client matching AWS's documented denial text does not match; and echoing the resource ARN on an authorization denial reverses the property this check exists to pin (authorization precedes ARN parsing precisely so a cross-account or malformed ARN never discloses anything about the resource).
Fix:      The AccessDeniedException message construction in the shared KMS authorization path used by the metadata read operations (GetKeyLastUsage, ListKeyPolicies) and UpdatePrimaryRegion. The error CODE on this path was addressed by the resolved issue 20261024T140708Z-kms-error-semantics-checks-are-blocked-by-five.md (its Cause 2); the code is now correct and the message is not, which is the new defect recorded here.
Traces:   kms.invalid-arn-consistent-across-metadata-ops (asserts at check_kms_invalid_arn_consistent_across_metadata_ops.py:56). The check's later legs — the same template for ListKeyPolicies and UpdatePrimaryRegion, the effect-absent DescribeKey, and the UnsupportedOperationException "<key ARN> is not a multi-region key." positive control — were not reached and must be verified by the fix.

Cause 6 of 7: InvalidCiphertextException is returned with the documentation blurb as its message where AWS returns an empty message.
AWS:      Smithy: Decrypt models InvalidCiphertextException (client, httpError 400, awsQueryError InvalidCiphertext). Recorded behavior: Decrypt presented with an EncryptionContext that differs in any VALUE from the context bound at encrypt time is refused with Error.Code InvalidCiphertextException, HTTP 400, and an EMPTY Error.Message. (The order-independence half of the contract — a reordered but value-identical context map decrypts successfully — already passes.)
Emulator: the refusal carries Error.Message "The request was rejected because the specified ciphertext, or additional authenticated data incorporated into the ciphertext, such as the encryption context, is corrupted, missing, or otherwise invalid." — the API-reference prose for the exception shape, which the wire never carries.
Why:      A caller that distinguishes KMS refusals by message, or asserts the absence of a message on this path, diverges; the emulator makes an information-free refusal look diagnostic.
Fix:      The InvalidCiphertextException construction in the Decrypt handler / the KMS error serializer, in the KMS crate.
Traces:   kms.sep-kms-key-scope-context-map-is-order-independent-on-decrypt (asserts at check_kms_sep_kms_key_scope_context_map_is_order_independent_on_decrypt.py:90).

Cause 7 of 7: IncorrectKeyException is returned with an invented message that discloses the wrapping key's ARN.
AWS:      Smithy: Decrypt models IncorrectKeyException (client, httpError 400, awsQueryError IncorrectKeyException). Recorded behavior: Decrypt(CiphertextBlob=<blob wrapped by key A>, KeyId=<key B>) is refused with Error.Code IncorrectKeyException, HTTP 400, and the message "The key ID in the request does not identify a CMK that can perform this operation." — entirely fixed text with NO variable parts; neither key is named. This holds even when A and B were given byte-identical imported key material, which is the separation property the check pins.
Emulator: the refusal carries "The key <key B ARN> cannot decrypt the ciphertext, which was encrypted by <key A ARN>." — an invented template that interpolates both key ARNs and, in particular, discloses the WRAPPING key's ARN to a caller who only ever presented the wrong key.
Why:      A client matching AWS's fixed refusal text does not match, and the emulator leaks the identity of a key the caller did not name — the opposite of AWS's deliberately key-agnostic wording on this path.
Fix:      The IncorrectKeyException construction in the Decrypt handler / the KMS error serializer, in the KMS crate.
Traces:   kms.sep-kms-key-scope-identical-imported-material-does-not-make-keys-interchangeable-rejected (asserts at check_kms_sep_kms_key_scope_identical_imported_material_does_not_make_keys_interchangeable_rejected.py:87). The sibling check kms.sep-kms-key-scope-cross-key-decrypt-fails-while-keyless-decrypt-names-the-wrapping-key hits the same template at line 65 but is already OWNED by 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md, so it is not re-listed here; fixing this template will also clear its line-65 assertion.

## Evidence
Replay output, verbatim assertion tails (behavioral suite simparity-behavioral-ga-prod-v1):

1. [mismatch] kms.generate-data-key-spec-and-bytes-exclusivity — check_kms_generate_data_key_spec_and_bytes_exclusivity.py:86
   neither = _err_code(lambda: kms.generate_data_key(KeyId=key_arn))
   AssertionError: real AWS rejects this call, but it SUCCEEDED and returned
     {'CiphertextBlob': b'...', 'Plaintext': b'<32 bytes>', 'KeyId': 'arn:aws:kms:<region>:<account>:key/<id>',
      'KeyMaterialId': '<64 hex>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
   Request:  GenerateDataKey(KeyId=<key ARN>)            # neither KeySpec nor NumberOfBytes
   Expected: HTTP 400, Error.Code = ValidationException
   Actual:   HTTP 200 with a 32-byte Plaintext data key
   (The preceding leg — GenerateDataKey(KeyId, KeySpec=AES_256, NumberOfBytes=32) — already returned ValidationException, so only the "neither" branch diverges. The NumberOfBytes=1025 leg and both GenerateDataKeyWithoutPlaintext legs were never reached.)

2. [mismatch] kms.import-key-material-token-single-use-rejected — check_kms_import_key_material_token_single_use_rejected.py:112
   AssertionError: expected exactly 1 success, got 2:
     [(None, {'KeyId': 'arn:aws:kms:<region>:<account>:key/<id>', 'KeyMaterialId': '<64 hex>', 'HTTPStatusCode': 200}),
      (None, {'KeyId': 'arn:aws:kms:<region>:<account>:key/<id>', 'KeyMaterialId': '<same 64 hex>', 'HTTPStatusCode': 200})]
   Request:  ImportKeyMaterial(KeyId, ImportToken=T, EncryptedKeyMaterial=M) twice with the identical (T, M) pair
   Expected: first call HTTP 200; second call HTTP 400, Error.Code = IncorrectKeyMaterialException,
             Error.Message = "expected new key material but the key material being imported is already associated with the KMS key" (fixed text, no variable parts)
   Actual:   both calls HTTP 200 with identical KeyId/KeyMaterialId

3. [mismatch] kms.import-token-ttl-window-enforced — check_kms_import_token_ttl_window_enforced.py:88
   assert message == "", f"expected an empty InvalidImportTokenException message, got {message!r}"
   AssertionError: expected an empty InvalidImportTokenException message, got 'The import token is not valid for arn:aws:kms:<region>:<account>:key/<id>. Call GetParametersForImport for a public key and import token to use with it.'
   Request:  ImportKeyMaterial(KeyId=<EXTERNAL key B>, ImportToken=<token issued for key A>, EncryptedKeyMaterial=...)
   Expected: HTTP 400, Error.Code = InvalidImportTokenException, Error.Message = "" (empty)
   Actual:   HTTP 400, Error.Code = InvalidImportTokenException, Error.Message = the invented sentence above

4. [error] kms.imported-rsa-private-key-signs-identically-to-the-local-key-it-came-from
   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ImportKeyMaterial operation: operation ImportKeyMaterial for a RSA_2048 KMS key is scaffolded but not implemented in this emulator
   Request:  CreateKey(KeySpec=RSA_2048, KeyUsage=SIGN_VERIFY, Origin=EXTERNAL) -> GetParametersForImport(WrappingAlgorithm=RSA_AES_KEY_WRAP_SHA_256, WrappingKeySpec=RSA_2048) -> ImportKeyMaterial(<wrapped PKCS#8 DER private key>)
   Expected: HTTP 200 (KeyId + KeyMaterialId), key becomes Enabled and Sign-able
   Actual:   Error.Code = NotImplemented (per-key-spec branch; the SYMMETRIC_DEFAULT branch of the same operation answers 200 — see item 2)

5. [mismatch] kms.invalid-arn-consistent-across-metadata-ops — check_kms_invalid_arn_consistent_across_metadata_ops.py:56
   assert message_last_usage == _denied("GetKeyLastUsage"), message_last_usage
   AssertionError: User: arn:aws:iam::<account>:user/<caller> is not authorized to perform: kms:GetKeyLastUsage on resource: arn:aws:kms:<region>:notanumericaccount:key/<id> because no resource-based policy allows the kms:GetKeyLastUsage action
   Request:  GetKeyLastUsage(KeyId=<real key ARN with its account segment replaced by "notanumericaccount">)
   Expected: Error.Code = AccessDeniedException, Error.Message =
             "User: <caller ARN> is not authorized to perform: kms:GetKeyLastUsage on this resource because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access"
             (fixed text everywhere except <caller ARN> from the caller identity and the kms:<Action> token from the operation; the resource ARN is NOT echoed)
   Actual:   Error.Code = AccessDeniedException (correct) with the "on resource: <arn> ... because no resource-based policy allows the <action> action" template, echoing the malformed ARN

6. [mismatch] kms.sep-kms-key-scope-context-map-is-order-independent-on-decrypt — check_kms_sep_kms_key_scope_context_map_is_order_independent_on_decrypt.py:90
   assert code == ("InvalidCiphertextException", ""), ...
   AssertionError: a reordered context with one value changed must raise InvalidCiphertextException with an empty message, got ('InvalidCiphertextException', 'The request was rejected because the specified ciphertext, or additional authenticated data incorporated into the ciphertext, such as the encryption context, is corrupted, missing, or otherwise invalid.')
   Expected: HTTP 400, Error.Code = InvalidCiphertextException, Error.Message = "" (empty)
   Actual:   same code, Error.Message = the API-reference prose for the exception shape

7. [mismatch] kms.sep-kms-key-scope-identical-imported-material-does-not-make-keys-interchangeable-rejected — check_..._interchangeable_rejected.py:87
   assert code_ab == ("IncorrectKeyException", _INCORRECT_KEY_MESSAGE), ...
   where _INCORRECT_KEY_MESSAGE = "The key ID in the request does not identify a CMK that can perform this operation."
   AssertionError: key A's blob pinned to key B (same imported material) must raise IncorrectKeyException, got ('IncorrectKeyException', 'The key arn:aws:kms:<region>:<account>:key/<B> cannot decrypt the ciphertext, which was encrypted by arn:aws:kms:<region>:<account>:key/<A>.')
   Expected: HTTP 400, Error.Code = IncorrectKeyException, Error.Message = "The key ID in the request does not identify a CMK that can perform this operation." (entirely fixed text; no key is named)
   Actual:   same code, an invented template interpolating both key ARNs

Smithy citations used (services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json):
- GenerateDataKey: KeyId required (KeyIdType @length 1..2048); KeySpec (DataKeySpec enum) and NumberOfBytes (NumberOfBytesType @range 1..1024) both OPTIONAL; errors DisabledException(409, Disabled), KMSInvalidStateException(409), NotFoundException(404, NotFound), InvalidGrantTokenException(400), InvalidKeyUsageException(400), KeyUnavailableException(500), DependencyTimeoutException(503), DryRunOperationException(412). ValidationException is NOT modeled — recorded behavior only.
- GenerateDataKeyWithoutPlaintext: same optional KeySpec/NumberOfBytes pair, same @range 1..1024, same error set.
- ImportKeyMaterial: KeyId/ImportToken/EncryptedKeyMaterial required (blobs CiphertextType @length 1..6144); errors IncorrectKeyMaterialException(400), ExpiredImportTokenException(400), InvalidImportTokenException(400), InvalidCiphertextException(400), UnsupportedOperationException(400, UnsupportedOperation), KMSInvalidStateException(409), NotFoundException(404), InvalidArnException(400).
- GetKeyLastUsage: KeyId required, KeyIdType @length 1..2048 with no pattern; errors InvalidArnException(400, InvalidArn), NotFoundException(404, NotFound), KMSInternalException(500), DependencyTimeoutException(503). AccessDeniedException is service-common.
- Decrypt: errors InvalidCiphertextException(400, InvalidCiphertext), IncorrectKeyException(400, IncorrectKeyException), DisabledException(409), KMSInvalidStateException(409), NotFoundException(404), InvalidKeyUsageException(400), InvalidGrantTokenException(400).

## Steps to Reproduce
Boot the emulator and replay these seven behavioral checks:

  kms.generate-data-key-spec-and-bytes-exclusivity
  kms.import-key-material-token-single-use-rejected
  kms.import-token-ttl-window-enforced
  kms.imported-rsa-private-key-signs-identically-to-the-local-key-it-came-from
  kms.invalid-arn-consistent-across-metadata-ops
  kms.sep-kms-key-scope-context-map-is-order-independent-on-decrypt
  kms.sep-kms-key-scope-identical-imported-material-does-not-make-keys-interchangeable-rejected

Manual repro per cause, any AWS SDK pointed at the emulator's KMS endpoint:

Cause 1: k = CreateKey(KeyUsage=ENCRYPT_DECRYPT, KeySpec=SYMMETRIC_DEFAULT).
         GenerateDataKey(KeyId=k)                                  -> expect ValidationException (400); observe HTTP 200 + 32-byte Plaintext.
         GenerateDataKey(KeyId=k, NumberOfBytes=1025)               -> expect ValidationException (400).
         GenerateDataKeyWithoutPlaintext(KeyId=k)                   -> expect ValidationException (400).
         GenerateDataKeyWithoutPlaintext(KeyId=k, KeySpec=AES_256, NumberOfBytes=32) -> expect ValidationException (400).
         Controls that must keep working: GenerateDataKey(KeyId=k, KeySpec=AES_128) returns exactly 16 bytes, NumberOfBytes=64 returns exactly 64 bytes, and each CiphertextBlob decrypts to exactly its own Plaintext; GenerateDataKeyWithoutPlaintext omits Plaintext entirely rather than returning it empty.

Cause 2: k = CreateKey(KeyUsage=ENCRYPT_DECRYPT, KeySpec=SYMMETRIC_DEFAULT, Origin=EXTERNAL); import initial material; then
         p = GetParametersForImport(KeyId=k, WrappingAlgorithm=RSAES_OAEP_SHA_256, WrappingKeySpec=RSA_2048);
         M = RSA-OAEP-SHA256(p.PublicKey, <32 random bytes>);
         call ImportKeyMaterial(KeyId=k, ImportToken=p.ImportToken, EncryptedKeyMaterial=M) TWICE with the identical pair.
         Expect exactly one HTTP 200 and one IncorrectKeyMaterialException (400); observe two HTTP 200s.

Cause 3: a = CreateKey(Origin=EXTERNAL); b = CreateKey(Origin=EXTERNAL); pa = GetParametersForImport(KeyId=a, ...);
         ImportKeyMaterial(KeyId=b, ImportToken=pa.ImportToken, EncryptedKeyMaterial=<material wrapped with pa.PublicKey>).
         Expect InvalidImportTokenException (400) with an EMPTY Error.Message; observe the invented sentence.

Cause 4: k = CreateKey(KeySpec=RSA_2048, KeyUsage=SIGN_VERIFY, Origin=EXTERNAL);
         p = GetParametersForImport(KeyId=k, WrappingAlgorithm=RSA_AES_KEY_WRAP_SHA_256, WrappingKeySpec=RSA_2048);
         ImportKeyMaterial(KeyId=k, ImportToken=p.ImportToken, EncryptedKeyMaterial=<RSA_AES-wrapped PKCS#8 DER private key>).
         Expect HTTP 200 and a Sign-able key; observe Error.Code=NotImplemented naming the RSA_2048 key spec.

Cause 5: k = CreateKey(); take its Arn and replace the 12-digit account segment with "notanumericaccount"; call
         GetKeyLastUsage / ListKeyPolicies / UpdatePrimaryRegion with that ARN.
         Expect AccessDeniedException from all three with the "... on this resource because the resource does not exist in this Region, no resource-based policies allow access, or a resource-based policy explicitly denies access" template and NO resource ARN echoed; observe the "on resource: <arn> ... because no resource-based policy allows the <action> action" template.

Cause 6: k = CreateKey(); c = Encrypt(KeyId=k, Plaintext=..., EncryptionContext={several pairs});
         Decrypt(CiphertextBlob=c, KeyId=k, EncryptionContext=<same pairs reordered with ONE value changed>).
         Expect InvalidCiphertextException (400) with an EMPTY Error.Message; observe the API-reference prose.
         Control: the same context reordered but value-identical must still decrypt successfully.

Cause 7: a = CreateKey(Origin=EXTERNAL); b = CreateKey(Origin=EXTERNAL); import byte-IDENTICAL material into both;
         ca = Encrypt(KeyId=a, ...); Decrypt(CiphertextBlob=ca, KeyId=b).
         Expect IncorrectKeyException (400) with Error.Message exactly "The key ID in the request does not identify a CMK that can perform this operation."; observe the two-ARN template.
         Control: keyless Decrypt(CiphertextBlob=ca) must still succeed and name key a.
