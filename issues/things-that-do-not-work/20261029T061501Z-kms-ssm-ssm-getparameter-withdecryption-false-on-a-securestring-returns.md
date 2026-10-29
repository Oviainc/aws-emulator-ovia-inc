Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__ssm.encryption-context-parameter-arn-condition-scopes-decryption
Issue-Key: kms__ssm.encryption-context-parameter-arn-condition-scopes-decryption|run#37|Error|denied_blob = base64.b64decode(
Issue-Commit: 6cada3bea20bb0dfad4860907220259726a5f31e

# SSM GetParameter(WithDecryption=False) on a SecureString returns the plaintext instead of a base64 KMS ciphertext, so the encryption-context-scoped kms:Decrypt leg cannot run (1 cause)

> _Note: this issue was identified against the code base at commit `6cada3bea20bb0dfad4860907220259726a5f31e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
Cause 1 of 1: a parameter written with Type='SecureString' and KeyId=<customer-managed CMK> is handed back verbatim by the undecrypted read, so the value a client reads with WithDecryption=False is the cleartext secret rather than a base64-encoded KMS ciphertext, and nothing that is built on that ciphertext (handing it to kms:Decrypt under the parameter's encryption context) is reachable.

NOTE — a similar open issue exists, 20261025T015335Z-kms-ssm-ssm-securestring-parameters-are-stored-in-plaintext-getparameter.md ("SSM SecureString parameters are stored in plaintext"), which carries only the replay key of kms__ssm.ssm-securestring-parameter-is-encrypted-at-rest-and-decrypts-only-with-permission. The failing check named here is NOT covered by that issue's replay key, so it is filed separately and should be re-replayed alongside it; a fixer closing one should verify both check ids.

AWS: PutParameter with Type='SecureString' encrypts Value with KMS (the key named by KeyId, defaulting to alias/aws/ssm) before storing it, using the encryption context {"PARAMETER_ARN": <parameter ARN>}; the two read forms of such a parameter therefore differ. Smithy (ssm): PutParameter.Value targets PSParameterValue and is @sensitive, Type targets the ParameterType enum including SecureString, KeyId targets ParameterKeyId; GetParameter.WithDecryption is documented 'Return decrypted values for secure string parameters. This flag is ignored for String and StringList parameter types.' — so WithDecryption=True yields the plaintext and WithDecryption=False (the default) yields the stored KMS ciphertext, base64-encoded, never the plaintext. Recorded behavior: the WithDecryption=False value base64-decodes to a KMS ciphertext blob that kms:Decrypt accepts when given EncryptionContext {"PARAMETER_ARN": <that parameter's ARN>}, which is what makes a role whose kms:Decrypt is conditioned on kms:EncryptionContext:PARAMETER_ARN provable directly against KMS with the same denial text SSM relays ('User: <assumed-role ARN> is not authorized to perform: kms:Decrypt on resource: <key ARN> because no identity-based policy allows the kms:Decrypt action' — fixed text, with the principal ARN, the action and the key ARN taken from the request and state).

Emulator: ssm:GetParameter(Name=<SecureString parameter>, WithDecryption=False) answers 200 with Parameter.Value set to the literal plaintext that was written. The check's base64 decode of that value raises binascii.Error: Incorrect padding, i.e. the returned value is not base64 at all, let alone a KMS ciphertext. The legs before it pass: the emulator does enforce the kms:EncryptionContext:PARAMETER_ARN condition on the relayed GetParameter(WithDecryption=True) read (AccessDeniedException with the expected relayed KMS denial text), so only the at-rest encryption and the ciphertext it should expose are missing.

Why: a client that reads a SecureString without WithDecryption — the normal way to list, copy, replicate or audit parameters without being entitled to their contents — receives the cleartext secret from the emulator, so encryption-at-rest for Parameter Store is absent and any KMS entitlement check on the contents is bypassed by simply omitting the flag. Conversely, every workflow that takes the ciphertext out of Parameter Store and calls kms:Decrypt itself (envelope re-wrap, cross-account key checks, verifying an EncryptionContext-scoped key policy directly against KMS) fails against the emulator with a decode error rather than working, because there is no ciphertext to decode.

Fix: the SecureString path of the SSM crate's parameter store and its PutParameter/GetParameter(+GetParameters/GetParametersByPath/GetParameterHistory) handlers — PutParameter(Type=SecureString) must encrypt through KMS under the resolved KeyId with EncryptionContext {PARAMETER_ARN: <parameter ARN>} and persist the resulting ciphertext; the read path must return that ciphertext base64-encoded when WithDecryption is false or absent, and decrypt through KMS with the same encryption context when it is true (so the key policy and its conditions are evaluated), relaying a KMS refusal as AccessDeniedException. The ciphertext handed out must be one the emulator's own kms:Decrypt accepts with that encryption context.

Traces: kms__ssm.encryption-context-parameter-arn-condition-scopes-decryption (errors at base64.b64decode of the WithDecryption=False value).

## Evidence
kms__ssm.encryption-context-parameter-arn-condition-scopes-decryption [error]: base64.b64decode(ssm.get_parameter(Name=<denied SecureString>, WithDecryption=False)['Parameter']['Value']) -> binascii.Error: Incorrect padding.
Expected (AWS): GetParameter(Name, WithDecryption=False) -> Parameter.Value is a base64-encoded KMS ciphertext (it decodes, and it is not the written plaintext); base64-decoding it and calling kms:Decrypt(CiphertextBlob=<those bytes>, EncryptionContext={'PARAMETER_ARN': <parameter ARN>}) as a role whose kms:Decrypt is conditioned on a DIFFERENT parameter ARN is refused with HTTP 400 AccessDeniedException and the exact message 'User: <assumed-role ARN> is not authorized to perform: kms:Decrypt on resource: <key ARN> because no identity-based policy allows the kms:Decrypt action' (fixed text; the principal ARN and key ARN come from the request and the key's state, with no relay suffix and no request id).
Actual (emulator): Parameter.Value == the written plaintext, which is not valid base64, so the direct kms:Decrypt leg never happens. The preceding relayed-denial assertions on GetParameter(WithDecryption=True) passed, so the divergence is confined to the stored/returned ciphertext.

## Steps to Reproduce
1. kms:CreateKey (SYMMETRIC_DEFAULT, ENCRYPT_DECRYPT) with an account-root key policy.
2. ssm:PutParameter(Name='/sim2real-ctxcond-denied', Value='sim2real-denied-plaintext', Type='SecureString', KeyId=<that key id>).
3. ssm:GetParameter(Name='/sim2real-ctxcond-denied', WithDecryption=False) — AWS returns a base64 KMS ciphertext; the emulator returns 'sim2real-denied-plaintext'.
4. base64-decode the value from step 3 — on AWS it decodes to ciphertext bytes; against the emulator it raises 'Incorrect padding' (the value is plaintext).
5. Call kms:Decrypt(CiphertextBlob=<decoded bytes>, EncryptionContext={'PARAMETER_ARN': <ARN from ssm:GetParameter>}) with a role whose kms:Decrypt is allowed only when kms:EncryptionContext:PARAMETER_ARN equals a DIFFERENT parameter's ARN — AWS answers 400 AccessDeniedException naming kms:Decrypt on the key ARN; against the emulator step 4 already made the call impossible.
