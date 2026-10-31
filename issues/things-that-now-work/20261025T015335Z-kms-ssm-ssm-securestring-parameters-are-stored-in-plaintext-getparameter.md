Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__ssm.ssm-securestring-parameter-is-encrypted-at-rest-and-decrypts-only-with-permission
Issue-Key: kms__ssm.ssm-securestring-parameter-is-encrypted-at-rest-and-decrypts-only-with-permission|run#21|AssertionError|assert raw != _PLAINTEXT, (
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# SSM SecureString parameters are stored in plaintext: GetParameter(WithDecryption=False) returns the secret instead of a KMS ciphertext (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutParameter

## Description
Cause 1 of 1: a parameter written with Type='SecureString' and KeyId=<customer-managed CMK> is stored as the literal plaintext, so the undecrypted read hands the secret back.

AWS: PutParameter with Type='SecureString' encrypts the value with KMS (the key named by KeyId) before storing it, and the two read forms of that parameter differ. Smithy (ssm): PutParameter.Value is @sensitive; Type targets the ParameterType enum that includes SecureString; KeyId (ParameterKeyId) selects the KMS key; GetParameter.WithDecryption is documented 'Return decrypted values for secure string parameters. This flag is ignored for String and StringList parameter types.' — so WithDecryption=True returns the plaintext and WithDecryption=False (the default) returns the stored KMS ciphertext, base64-encoded, never the plaintext. Recorded behavior (the check's measured oracle, us-west-2 2026-09-10): the WithDecryption=False bytes are a real KMS ciphertext bound to the parameter with EncryptionContext {"PARAMETER_ARN": <parameter ARN>} — kms:Decrypt with that context returns the exact plaintext and reports KeyId = the CMK's ARN, while kms:Decrypt with no EncryptionContext is refused with InvalidCiphertextException; and because the decrypt is performed by KMS on SSM's behalf, a second principal in the same account that the key policy explicitly denies kms:Decrypt is refused the plaintext read with AccessDeniedException (HTTP 400) while still reading byte-identical ciphertext with WithDecryption=False.

Emulator: GetParameter(WithDecryption=False) returns the exact plaintext that was written ('…securestring-plaintext-…'), i.e. the value is labelled SecureString (Type and the recorded KeyId do come back correctly from GetParameter/GetParameterHistory) but never encrypted. No KMS ciphertext exists, so nothing downstream of that read can hold either: the bytes cannot be handed to kms:Decrypt, the PARAMETER_ARN encryption-context binding does not exist, and the key-policy-deny leg (the AccessDeniedException the check is named for) is unreachable.

Why: a client that reads a SecureString without WithDecryption — the normal way to list/copy/replicate parameters without being entitled to their contents, and the way Parameter Store is audited — receives the cleartext secret. Encryption-at-rest for Parameter Store is absent, KMS key policies and grants have no effect on SecureString contents (anyone who can call ssm:GetParameter gets the secret without any kms:Decrypt entitlement), and no envelope/rewrap or cross-account-key workflow built on the ciphertext can be developed against the emulator.

Fix: the SecureString path in the SSM crate's parameter store and its PutParameter/GetParameter handlers — PutParameter(Type=SecureString) must call KMS Encrypt under the resolved KeyId (defaulting to alias/aws/ssm) with EncryptionContext {PARAMETER_ARN: <arn>} and persist the ciphertext; GetParameter/GetParameters/GetParametersByPath must return that ciphertext base64-encoded when WithDecryption is false or absent, and call KMS Decrypt (same encryption context, so the key policy and its conditions are evaluated) when it is true, relaying a KMS refusal as AccessDeniedException.

Traces: kms__ssm.ssm-securestring-parameter-is-encrypted-at-rest-and-decrypts-only-with-permission (failing assertion at the first of the two read forms; everything after it in that check is blocked behind this cause).

## Evidence
kms__ssm.ssm-securestring-parameter-is-encrypted-at-rest-and-decrypts-only-with-permission [mismatch]: AssertionError: "WithDecryption=False returned the plaintext 'sim2real-behav-securestring-plaintext-a7f3'; the value is labelled SecureString but is not encrypted".
Expected (AWS): GetParameter(Name, WithDecryption=False) -> Parameter.Value is a base64 KMS ciphertext != the written plaintext; kms:Decrypt(CiphertextBlob=base64decode(Value), EncryptionContext={'PARAMETER_ARN': <parameter ARN>}) -> Plaintext == the written value and KeyId == the CMK ARN; kms:Decrypt of the same blob with no EncryptionContext -> InvalidCiphertextException; GetParameter(WithDecryption=True) by a principal the key policy denies kms:Decrypt -> HTTP 400 AccessDeniedException whose message implicates kms:Decrypt on that key, while that same principal's WithDecryption=False read still succeeds with byte-identical ciphertext.
Actual (emulator): GetParameter(WithDecryption=False) -> Parameter.Value == the plaintext. (Type == 'SecureString' and GetParameterHistory's KeyId == the CMK were returned correctly, so only the encryption itself is missing.)

## Steps to Reproduce
1. kms:CreateKey (SYMMETRIC_DEFAULT, ENCRYPT_DECRYPT) with the default account-root key policy. 2. ssm:PutParameter(Name='/sim2real-secure-x', Value='plaintext-marker', Type='SecureString', KeyId=<that key id>). 3. ssm:GetParameter(Name='/sim2real-secure-x', WithDecryption=False) — AWS returns a base64 KMS ciphertext; the emulator returns 'plaintext-marker'. 4. Confirm the stronger half is unreachable: base64-decode the value from step 3 and call kms:Decrypt(CiphertextBlob=<those bytes>, EncryptionContext={'PARAMETER_ARN': <ARN from GetParameter>}) — on AWS this returns the plaintext and the key ARN; in the emulator there is no ciphertext to decrypt. 5. Optional: add an explicit Deny of kms:Decrypt for a second role in the key policy, assume it and call ssm:GetParameter(WithDecryption=True) — AWS answers 400 AccessDeniedException naming kms:Decrypt; the emulator answers 200 with the value.
