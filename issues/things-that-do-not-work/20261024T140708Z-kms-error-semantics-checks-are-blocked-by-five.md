Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.disable-enable-key kms.get-key-last-usage-invalid-arn-rejected kms.invalid-arn-consistent-across-metadata-ops kms.get-parameters-for-import-native-origin-key-rejected kms.op-import-apis-on-kms-origin-rejected kms.rotate-key-on-demand-unsupported-key-type-rejected kms.update-custom-key-store-reconfigures-cluster
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.disable-enable-key|run#19|AssertionError|assert gdk_err.get("Code") == "DisabledException", (
Issue-Key: kms.get-key-last-usage-invalid-arn-rejected|run#5|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: kms.invalid-arn-consistent-across-metadata-ops|run#12|AssertionError|assert code_last_usage == "AccessDeniedException", f"unexpected error code: {code_last_usage!r}"
Issue-Key: kms.get-parameters-for-import-native-origin-key-rejected|run#9|AssertionError|assert code == "UnsupportedOperationException", f"GetParametersForImport on AWS_KMS-origin key -> got {code}"
Issue-Key: kms.op-import-apis-on-kms-origin-rejected|run#10|AssertionError|assert code == "UnsupportedOperationException", f"GetParametersForImport -> got {code}"
Issue-Key: kms.rotate-key-on-demand-unsupported-key-type-rejected|run#4|AssertionError|assert code == "UnsupportedOperationException", f"unexpected error code: {code!r}"
Issue-Key: kms.update-custom-key-store-reconfigures-cluster|run#3|AssertionError|assert absent == "CustomKeyStoreNotFoundException", (
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# KMS error-semantics checks are blocked by five scaffold stubs that launder into wrong error codes: GenerateDataKey, GetKeyLastUsage, GetParametersForImport, RotateKeyOnDemand, UpdateCustomKeyStore all answer NotImplemented (5 causes)

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateDataKey

## Description
Five distinct KMS operations answer `NotImplemented` instead of their modeled behavior. Each of these failing checks wraps its call in a helper that catches `ClientError` and asserts on `Error.Code`, so the stub's `NotImplemented` is laundered into an error-code mismatch rather than a blocked call — which is why the replay report did not auto-mark them as blocked by an unimplemented operation. Related open issues exist per operation (`20261024T102105Z-kms-generatedatakey-stub.md`, `20261024T102105Z-kms-getkeylastusage-stub.md`, `20261024T102105Z-kms-getparametersforimport-stub.md`, `20261024T102105Z-kms-rotatekeyondemand-stub.md`, `20261024T102105Z-kms-updatecustomkeystore-stub.md`); this issue records the specific error-semantics contracts these checks pin, which a bare "implement the stub" fix will not satisfy on its own.

Cause 1 of 5: GenerateDataKey does not honor key state (and is not implemented at all).
AWS:      Smithy: GenerateDataKey models `DisabledException` (client, httpError 409, awsQueryError code `Disabled`) and `KMSInvalidStateException` (409). A key whose KeyState is `Disabled` must refuse GenerateDataKey with `DisabledException` while the metadata plane stays readable. In the same check, Encrypt and Decrypt on the same disabled key already return `DisabledException` correctly, so only GenerateDataKey diverges. The recorded message is of the documented form `<key ARN> is disabled.` (built from the request's key, so the rule is the requirement; the literal text is an example).
Emulator: GenerateDataKey returns error code `NotImplemented` ("operation GenerateDataKey is scaffolded but not implemented in this emulator") for every call, disabled key or not.
Why:      Envelope-encryption clients cannot obtain a data key at all, and any client that branches on `DisabledException` to re-enable a key or fail over sees an unrecognized code instead, so key-state enforcement is unobservable.
Fix:      The GenerateDataKey handler plus its key-state precondition check in the KMS crate.
Traces:   kms.disable-enable-key (asserts at check_kms_disable_enable_key.py:63). Handler: services/service-com-amazonaws-kms-trentservice/src/ops/generate_data_key.rs.

Cause 2 of 5: GetKeyLastUsage has no error semantics for malformed/degenerate key ARNs.
AWS:      Smithy: GetKeyLastUsage.KeyId is `KeyIdType` with @length 1..2048 and NO pattern, so the service — not the model — classifies bad shapes; GetKeyLastUsage models `InvalidArnException` (400), `NotFoundException` (404, awsQueryError `NotFound`) and `KMSInternalException`. Recorded behavior shows two distinct classifications: (a) an ARN-shaped KeyId carrying the caller's own account but a mangled body (extra empty colon-delimited field) is answered `NotFoundException` with message `Invalid arn <keyid>`; (b) an ARN whose account-id segment is not the required 12-digit numeric shape is answered `AccessDeniedException` — authorization is evaluated before any ARN-content validation, and the same malformed-account ARN is refused identically by ListKeyPolicies and UpdatePrimaryRegion.
Emulator: GetKeyLastUsage returns `NotImplemented` for both inputs, so neither classification exists and the cross-operation consistency cannot hold.
Why:      A client cannot distinguish "no such key" from "not permitted" from "malformed ARN", and the deliberate AWS property that authorization precedes ARN parsing (so a cross-account ARN never leaks existence) is absent.
Fix:      The GetKeyLastUsage handler, its KeyId resolution/ARN validation, and the shared authorization-before-lookup ordering used by the metadata read path.
Traces:   kms.get-key-last-usage-invalid-arn-rejected (check_kms_get_key_last_usage_invalid_arn_rejected.py:32, expected NotFoundException, got NotImplemented); kms.invalid-arn-consistent-across-metadata-ops (check_kms_invalid_arn_consistent_across_metadata_ops.py:55, expected AccessDeniedException, got NotImplemented). Handler: services/service-com-amazonaws-kms-trentservice/src/ops/get_key_last_usage.rs.

Cause 3 of 5: the import-material path does not reject an AWS_KMS-origin key.
AWS:      Smithy: both GetParametersForImport and DeleteImportedKeyMaterial model `UnsupportedOperationException` (client, httpError 400, awsQueryError code `UnsupportedOperation`). Import material only applies to `Origin=EXTERNAL` keys, so both operations must refuse a key created with `Origin=AWS_KMS` with `UnsupportedOperationException` and leave that key untouched (still `Origin=AWS_KMS`, still `Enabled`, still usable for Encrypt/Decrypt). The message is built from the request's key and origin — form `<key ARN> origin is AWS_KMS which is not valid for this operation.` — so that rule is the requirement, the literal text an example. Positive control: the identical call against an `Origin=EXTERNAL` key must succeed, returning a DER-encoded wrapping PublicKey (first byte 0x30), an ImportToken, and ParametersValidTo.
Emulator: GetParametersForImport returns `NotImplemented` regardless of the key's Origin, so neither the native-origin refusal nor the EXTERNAL-origin happy path exists, and the whole import/expire lifecycle is unreachable.
Why:      A client importing key material gets an unrecognized code instead of `UnsupportedOperationException`, and an emulator that later implements the call without the origin gate would silently accept an operation real KMS refuses — the "overly-permissive" failure class this check exists to catch.
Fix:      The GetParametersForImport handler (and the matching origin precondition in DeleteImportedKeyMaterial / ImportKeyMaterial) in the KMS crate.
Traces:   kms.get-parameters-for-import-native-origin-key-rejected (check_kms_get_parameters_for_import_native_origin_key_rejected.py:42); kms.op-import-apis-on-kms-origin-rejected (check_kms_op_import_apis_on_kms_origin_rejected.py:65). Handler: services/service-com-amazonaws-kms-trentservice/src/ops/get_parameters_for_import.rs.

Cause 4 of 5: RotateKeyOnDemand does not reject an unsupported key type.
AWS:      Smithy: RotateKeyOnDemand models `UnsupportedOperationException` (client, 400, awsQueryError `UnsupportedOperation`). On-demand rotation applies only to symmetric encryption KMS keys, so an asymmetric `SIGN_VERIFY`/`RSA_2048` key must be refused with `UnsupportedOperationException` (recorded with an empty message) and must gain no rotation history; GetKeyRotationStatus must still succeed on that key reporting `KeyRotationEnabled=false`, and ListKeyRotations must return an empty `Rotations` list. Positive control: the same call on a symmetric key succeeds and the rotation becomes observable through ListKeyRotations with `RotationType=ON_DEMAND`.
Emulator: RotateKeyOnDemand returns `NotImplemented` for every key, so the key-type gate and the rotation record are both absent.
Why:      Key-rotation automation cannot tell "this key type can never rotate" from "the service is broken", and once implemented without the gate the emulator would report a rotation for a key real KMS refuses to rotate.
Fix:      The RotateKeyOnDemand handler, its key-spec/key-usage precondition, and the rotation-history state it writes (read back by GetKeyRotationStatus/ListKeyRotations).
Traces:   kms.rotate-key-on-demand-unsupported-key-type-rejected (check_kms_rotate_key_on_demand_unsupported_key_type_rejected.py:27, expected UnsupportedOperationException, got NotImplemented). Handler: services/service-com-amazonaws-kms-trentservice/src/ops/rotate_key_on_demand.rs.

Cause 5 of 5: custom-key-store target resolution is absent, so absent and unparseable ids are not collapsed onto one error.
AWS:      Smithy: UpdateCustomKeyStore models `CustomKeyStoreNotFoundException` (client, httpError 400) and CreateCustomKeyStore models `XksProxyUriUnreachableException` (client, 400). UpdateCustomKeyStore resolves its target first and answers BOTH a well-formed-but-absent `CustomKeyStoreId` and a value that is not a key-store id at all with the SAME `CustomKeyStoreNotFoundException` (recorded with an empty message) — it does NOT split malformed from absent, so an emulator that raises a validation error for the unparseable id diverges. DescribeCustomKeyStores with the absent id must agree (same exception), and neither rejected update may bring a key store into existence. Separately, CreateCustomKeyStore with `CustomKeyStoreType=EXTERNAL_KEY_STORE` contacts the XKS proxy synchronously and fails with `XksProxyUriUnreachableException` for a well-formed but unreachable endpoint, rather than deferring to ConnectCustomKeyStore.
Emulator: UpdateCustomKeyStore returns `NotImplemented`, so no target resolution happens and the malformed/absent collapse cannot be observed; the same gap blocks the DescribeCustomKeyStores agreement leg and the synchronous XKS reachability probe.
Why:      A client managing a custom key store receives an unrecognized code for a plain "no such key store", and an emulator that later distinguishes malformed from absent would teach callers an error taxonomy KMS does not have.
Fix:      The UpdateCustomKeyStore handler's target resolution (shared with DescribeCustomKeyStores) and the CreateCustomKeyStore XKS-reachability precondition, in the KMS crate.
Traces:   kms.update-custom-key-store-reconfigures-cluster (check_kms_update_custom_key_store_reconfigures_cluster.py:58, expected CustomKeyStoreNotFoundException, got NotImplemented). Handler: services/service-com-amazonaws-kms-trentservice/src/ops/update_custom_key_store.rs.

## Evidence
Replay output, verbatim assertion tails:

1. kms.disable-enable-key [mismatch] — check_kms_disable_enable_key.py:63
   AssertionError: GenerateDataKey on a Disabled key raised 'NotImplemented', expected 'DisabledException'
   (Encrypt and Decrypt on the same Disabled key already returned DisabledException, so the check reached line 63.)

2. kms.get-key-last-usage-invalid-arn-rejected [mismatch] — check_kms_get_key_last_usage_invalid_arn_rejected.py:32
   AssertionError: expected NotFoundException, got NotImplemented
   Request: GetKeyLastUsage(KeyId="arn:aws:kms::<region>:<account>:key/00000000-0000-0000-0000-000000000000")
   Expected: Error.Code=NotFoundException (HTTP 404), Error.Message of the form "Invalid arn <keyid>"
   Actual:   Error.Code=NotImplemented

3. kms.invalid-arn-consistent-across-metadata-ops [mismatch] — check_kms_invalid_arn_consistent_across_metadata_ops.py:55
   AssertionError: unexpected error code: 'NotImplemented'
   Request: GetKeyLastUsage(KeyId=<real key ARN with the account segment replaced by "notanumericaccount">)
   Expected: Error.Code=AccessDeniedException, identical to ListKeyPolicies and UpdatePrimaryRegion on the same malformed ARN
   Actual:   Error.Code=NotImplemented

4. kms.get-parameters-for-import-native-origin-key-rejected [mismatch] — check_kms_get_parameters_for_import_native_origin_key_rejected.py:42
   AssertionError: GetParametersForImport on AWS_KMS-origin key -> got NotImplemented
   Request: CreateKey(Origin="AWS_KMS", KeySpec=SYMMETRIC_DEFAULT) then GetParametersForImport(KeyId=<that key>, WrappingAlgorithm=RSAES_OAEP_SHA_256, WrappingKeySpec=RSA_2048)
   Expected: Error.Code=UnsupportedOperationException (HTTP 400), message of the form "<key ARN> origin is AWS_KMS which is not valid for this operation."
   Actual:   Error.Code=NotImplemented

5. kms.op-import-apis-on-kms-origin-rejected [mismatch] — check_kms_op_import_apis_on_kms_origin_rejected.py:65
   AssertionError: GetParametersForImport -> got NotImplemented
   Same expected/actual as (4); the DeleteImportedKeyMaterial leg and the Origin=EXTERNAL positive control (DER PublicKey, ImportToken, ImportKeyMaterial, PendingImport revert) were never reached.

6. kms.rotate-key-on-demand-unsupported-key-type-rejected [mismatch] — check_kms_rotate_key_on_demand_unsupported_key_type_rejected.py:27
   AssertionError: unexpected error code: 'NotImplemented'
   Request: CreateKey(KeyUsage=SIGN_VERIFY, KeySpec=RSA_2048) then RotateKeyOnDemand(KeyId=<that key>)
   Expected: Error.Code=UnsupportedOperationException (HTTP 400), empty Error.Message
   Actual:   Error.Code=NotImplemented

7. kms.update-custom-key-store-reconfigures-cluster [mismatch] — check_kms_update_custom_key_store_reconfigures_cluster.py:58
   AssertionError: expected CustomKeyStoreNotFoundException for an absent key store id, got 'NotImplemented'
   Request: UpdateCustomKeyStore(CustomKeyStoreId="cks-0000000000000000d", NewCustomKeyStoreName=<unique>)
   Expected: Error.Code=CustomKeyStoreNotFoundException (HTTP 400), empty message; the same code for CustomKeyStoreId="not-a-cks"; the same code from DescribeCustomKeyStores(CustomKeyStoreId=absent); and XksProxyUriUnreachableException from CreateCustomKeyStore(CustomKeyStoreType=EXTERNAL_KEY_STORE) against an unreachable proxy
   Actual:   Error.Code=NotImplemented

Smithy citations used: GenerateDataKey -> DisabledException(409, awsQueryError Disabled), KMSInvalidStateException(409); GetKeyLastUsage.KeyId KeyIdType @length 1..2048 with no pattern, errors InvalidArnException(400)/NotFoundException(404); GetParametersForImport + DeleteImportedKeyMaterial -> UnsupportedOperationException(400, awsQueryError UnsupportedOperation); RotateKeyOnDemand -> UnsupportedOperationException(400); UpdateCustomKeyStore -> CustomKeyStoreNotFoundException(400); CreateCustomKeyStore -> XksProxyUriUnreachableException(400).

## Steps to Reproduce
Boot the emulator and replay these seven behavioral checks against it:

  kms.disable-enable-key
  kms.get-key-last-usage-invalid-arn-rejected
  kms.invalid-arn-consistent-across-metadata-ops
  kms.get-parameters-for-import-native-origin-key-rejected
  kms.op-import-apis-on-kms-origin-rejected
  kms.rotate-key-on-demand-unsupported-key-type-rejected
  kms.update-custom-key-store-reconfigures-cluster

Minimal manual repro per cause (any AWS SDK pointed at the emulator's KMS endpoint):

Cause 1: k = CreateKey(KeyUsage=ENCRYPT_DECRYPT, KeySpec=SYMMETRIC_DEFAULT); DisableKey(KeyId=k); GenerateDataKey(KeyId=k, KeySpec=AES_256).
         Expect DisabledException (409); observe NotImplemented.
Cause 2: GetKeyLastUsage(KeyId="arn:aws:kms::<region>:<your-account>:key/00000000-0000-0000-0000-000000000000").
         Expect NotFoundException (404); observe NotImplemented.
         Then GetKeyLastUsage / ListKeyPolicies / UpdatePrimaryRegion with a real key ARN whose account segment is "notanumericaccount".
         Expect AccessDeniedException from all three; observe NotImplemented from GetKeyLastUsage.
Cause 3: k = CreateKey(Origin=AWS_KMS, KeySpec=SYMMETRIC_DEFAULT); GetParametersForImport(KeyId=k, WrappingAlgorithm=RSAES_OAEP_SHA_256, WrappingKeySpec=RSA_2048).
         Expect UnsupportedOperationException (400); observe NotImplemented. Repeat with Origin=EXTERNAL for the positive control (must succeed).
Cause 4: k = CreateKey(KeyUsage=SIGN_VERIFY, KeySpec=RSA_2048); RotateKeyOnDemand(KeyId=k).
         Expect UnsupportedOperationException (400) with an empty message; observe NotImplemented.
Cause 5: UpdateCustomKeyStore(CustomKeyStoreId="cks-0000000000000000d", NewCustomKeyStoreName="x") and UpdateCustomKeyStore(CustomKeyStoreId="not-a-cks", NewCustomKeyStoreName="x").
         Expect CustomKeyStoreNotFoundException (400) from BOTH; observe NotImplemented.
