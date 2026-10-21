Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the Payment Cryptography Data emulator

_Filed: 2026-10-21T23:18:11Z_
_Operation: service-com-amazonaws-paymentcryptographydata-paymentcryptographydataplane | Class: stub_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 15 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. This issue defines the FIRST minimal, buildable slice: the symmetric data-protection core loop of the Payment Cryptography **data plane** (restJson1, sigv4 name `payment-cryptography`, endpoint prefix `dataplane.payment-cryptography`).

### Happy path to implement (in order)

1. **EncryptData** — `POST /keys/{KeyIdentifier}/encrypt`. Input: `KeyIdentifier` (@httpLabel, key ARN or alias), `PlainText` (hex, 2..4096), `EncryptionAttributes` (EncryptionDecryptionAttributes union — support at least the `Symmetric`/TDES or AES variant), optional `WrappedKey`. Output: `KeyArn` (resolved full key ARN), `CipherText` (hex, 2..4224), `KeyCheckValue`. The ciphertext must be produced by a real deterministic transform over the stored key material and recorded in state so step 2 can invert it.
2. **DecryptData** — `POST /keys/{KeyIdentifier}/decrypt`. Input: `KeyIdentifier` (@httpLabel), `CipherText` (hex), `DecryptionAttributes` (required). Output: `KeyArn`, `PlainText`, `KeyCheckValue` (all three `@required`). `PlainText` MUST equal the `PlainText` passed to EncryptData for that ciphertext under the SAME key and the same attributes/mode — not an echo of `CipherText`. Decrypting with a different key ARN or a mismatched mode must fail (`ValidationException` 400) rather than silently returning the plaintext.
3. **GenerateMac** — `POST /mac/generate`. Input: `KeyIdentifier`, `MessageData` (hex, 2..8192), `GenerationAttributes` (MacAttributes union — at least `Algorithm` ISO9797_ALGORITHM1/3 or CMAC), optional `MacLength` (4..32). Output: `KeyArn`, `KeyCheckValue`, `Mac` (hex, 4..128). The MAC must be a deterministic function of (key material, MessageData, algorithm, MacLength).
4. **VerifyMac** — `POST /mac/verify`. Input: `KeyIdentifier`, `MessageData`, `Mac`, `VerificationAttributes`, optional `MacLength`. On a MAC that matches what GenerateMac produced for the same key/message/attributes: 200 with `KeyArn` + `KeyCheckValue`. On any mismatch: **`VerificationFailedException` (HTTP 400)** — this operation must be able to FAIL; a handler that always returns 200 is a failure of this issue.
5. **GenerateCardValidationData** — `POST /cardvalidationdata/generate`. Input: `KeyIdentifier`, `PrimaryAccountNumber` (12..19 digits), `GenerationAttributes` (CardGenerationAttributes union — at least `CardVerificationValue1`/`CardVerificationValue2` with `CardExpiryDate`/`ServiceCode`), optional `ValidationDataLength` (3..5, default 3). Output: `KeyArn`, `KeyCheckValue`, `ValidationData` (3..5 digits).
6. **VerifyCardValidationData** — `POST /cardvalidationdata/verify`. Input: `KeyIdentifier`, `PrimaryAccountNumber`, `VerificationAttributes`, `ValidationData`. 200 (`KeyArn`, `KeyCheckValue`) when `ValidationData` matches what step 5 generated for the same PAN + attributes; `VerificationFailedException` (400) otherwise.

### Rules for this slice

- **No canned or echoed responses.** Every field above must be computed from persisted key material and recorded operation state. Returning the input hex back as `CipherText`/`PlainText`, a constant `KeyCheckValue`, a random MAC that is never stored, or an unconditional 200 from the two Verify operations all count as NOT implemented.
- **Key resolution is stateful and shared.** `KeyIdentifier` accepts either a key ARN (`arn:aws:payment-cryptography:<region>:<acct>:key/<id>`) or an alias (`alias/...`); both must resolve to the same registry entry, and the output `KeyArn` must always be the canonical full key ARN (70..150 chars, `key/...` form), never the alias the caller passed. An unknown identifier must return **`ResourceNotFoundException` (HTTP 404)**.
- **KeyCheckValue must be stable per key** across all six operations (same key identifier ⇒ same KCV), since that is the client-visible proof the emulator is using one persisted key.
- Minimal validation to honour: hex-only / even-length `PlainText`, `CipherText`, `MessageData`, `Mac`; numeric `PrimaryAccountNumber` of length 12..19; `ValidationDataLength` in 3..5; `MacLength` in 4..32 — each violation is `ValidationException` (HTTP 400), not a 500.

### State Model

Add to `ComAmazonawsPaymentcryptographydataPaymentcryptographydataplaneState` (currently an empty `TODO` struct in `src/state.rs`):

- `keys: HashMap<String /* canonical key ARN */, KeyEntry>` where `KeyEntry { arn: String, alias: Option<String>, material: Vec<u8> /* deterministic bytes derived from the ARN when the key was not created through the control plane */, key_algorithm: String, key_usage: String /* TR31_P0_PIN_ENCRYPTION_KEY, TR31_M3_ISO_9797_3_MAC_KEY, TR31_C0_CARD_VERIFICATION_KEY, ... */, kcv: String /* 6 hex chars, computed once from `material` and cached */ }`.
- `aliases: HashMap<String /* alias/... */, String /* canonical key ARN */>` so alias and ARN resolve identically.
- `ciphertexts: HashMap<String /* lowercase ciphertext hex */, CipherRecord>` where `CipherRecord { key_arn: String, plaintext_hex: String, mode: String /* ECB | CBC | CFB | ... from EncryptionDecryptionAttributes */, initialization_vector: Option<String> }` — this is what makes DecryptData/ReEncryptData invertible and key-bound.
- `macs: HashMap<MacKey, String /* mac hex */>` with `MacKey { key_arn: String, message_data: String, algorithm: String, mac_length: u32 }`, used by VerifyMac.
- `card_validation: HashMap<CvKey, String /* validation data digits */>` with `CvKey { key_arn: String, pan: String, attributes_digest: String, length: u32 }`, used by VerifyCardValidationData.

Key material may be lazily auto-provisioned for a well-formed-but-unknown ARN ONLY if the project decides the data plane should work without the control plane; if so it must be inserted into `keys` and reused on every later call, never regenerated per request (that would break KCV stability and decrypt round-trips).

### Out of scope (stay 501, filed separately later)

GeneratePinData, VerifyPinData, TranslatePinData, ReEncryptData, GenerateMacEmvPinChange, GenerateAuthRequestCryptogram, VerifyAuthRequestCryptogram, TranslateKeyMaterial, GenerateAs2805KekValidation.

### Done when

An integration test drives encrypt→decrypt round-trip, MAC generate→verify (pass AND tampered-message fail), and CVV generate→verify (pass AND wrong-value fail) against a single running emulator, asserting the response BODIES (plaintext equality, stable KeyArn/KeyCheckValue, VerificationFailedException on mismatch), not just 2xx.

## Evidence

OPERATIONS.md at the crate root lists all 15 operations with their routes, required input/output members and errors: EncryptData `POST /keys/{KeyIdentifier}/encrypt` (required output `KeyArn`, `CipherText`), DecryptData `POST /keys/{KeyIdentifier}/decrypt` (required output `KeyArn`, `KeyCheckValue`, `PlainText`), GenerateMac `POST /mac/generate` (required output `Mac`), VerifyMac `POST /mac/verify` (declares `VerificationFailedException (HTTP 400)`), GenerateCardValidationData `POST /cardvalidationdata/generate` (required output `ValidationData`), VerifyCardValidationData `POST /cardvalidationdata/verify` (declares `VerificationFailedException (HTTP 400)`). All five services' errors include `ResourceNotFoundException (HTTP 404)` for an unresolvable `KeyIdentifier` and `ValidationException (HTTP 400)` for malformed input; the model's length/pattern constraints (hex, even-length ciphertext/plaintext, 12..19 digit PAN, 3..5 validation data, key ARN 70..150 with `key/` form vs the 7..322 identifier that also accepts `alias/...`) are quoted in the per-operation entries above.

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 15 files in `services/service-com-amazonaws-paymentcryptographydata-paymentcryptographydataplane/src/ops/` (encrypt_data.rs, decrypt_data.rs, generate_mac.rs, verify_mac.rs, generate_card_validation_data.rs, verify_card_validation_data.rs, …), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` ⇒ HTTP 501. `src/state.rs` still contains only `// TODO: add service-specific state fields`, proving zero state is persisted today. Well-established service semantics grounding the round-trip requirement: an AWS Payment Cryptography decrypt of a ciphertext returns the original plaintext under the same key, KeyCheckValue is a fixed property of a key, and the Verify* operations raise VerificationFailedException on mismatch.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `payment-cryptography-data` client at it (restJson1, endpoint prefix `dataplane.payment-cryptography`).
2. Call EncryptData with KeyIdentifier=arn:aws:payment-cryptography:us-east-1:123456789012:key/aaaaaaaaaaaaaaaa, PlainText="31323334313233343132333431323334", EncryptionAttributes={Symmetric:{Mode:"CBC"}} — today: HTTP 501 NotImplemented; expected: 200 with a hex CipherText, the canonical KeyArn and a KeyCheckValue.
3. Call DecryptData with the SAME KeyIdentifier, the CipherText from step 2 and the matching DecryptionAttributes — expected: 200 and PlainText exactly equal to the step-2 PlainText, with the same KeyArn/KeyCheckValue.
4. Repeat DecryptData with a DIFFERENT key ARN — expected: ValidationException (400) or ResourceNotFoundException (404) for an unknown key, never the plaintext.
5. Call GenerateMac with KeyIdentifier, MessageData="3132333435363738", GenerationAttributes={Algorithm:"ISO9797_ALGORITHM3"} — expected: 200 with a deterministic hex Mac.
6. Call VerifyMac with the same key/message/attributes and that Mac — expected: 200; then flip one nibble of MessageData and call VerifyMac again — expected: VerificationFailedException (HTTP 400).
7. Call GenerateCardValidationData with PrimaryAccountNumber="4111111111111111", GenerationAttributes={CardVerificationValue2:{CardExpiryDate:"0123"}} — expected: 200 with 3-digit ValidationData; then VerifyCardValidationData with that value (expected 200) and with "000" (expected VerificationFailedException 400).
8. Call EncryptData again with the same key identifier and confirm the returned KeyArn and KeyCheckValue are identical to every earlier call (state is persisted, not regenerated per request).
