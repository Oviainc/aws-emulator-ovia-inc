Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 85378260976efce74d968b045ae62c2fe2a03011

# Bootstrap the com-amazonaws-kms-trentservice (AWS KMS) emulator

_Filed: 2026-10-21T22:28:25Z_
_Operation: service-com-amazonaws-kms-trentservice | Class: missing_

> _Note: this issue was identified against the code base at commit `85378260976efce74d968b045ae62c2fe2a03011`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate is a fresh scaffold: all 54 operations in `src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answer HTTP 501 `NotImplemented`, holding zero state. Implement a FIRST SMALL SLICE: the KMS key lifecycle plus one crypto round-trip, end-to-end and statefully, over awsJson1_1 (`POST /`, `X-Amz-Target: TrentService.<Operation>`).

Happy path to implement, in order:

1. **CreateKey** — input `Description`, `KeyUsage` (default `ENCRYPT_DECRYPT`), `KeySpec` (default `SYMMETRIC_DEFAULT`), `Origin` (default `AWS_KMS`), `Policy`, `Tags`, `MultiRegion`, `BypassPolicyLockoutSafetyCheck`. Must mint a fresh UUID key id and ARN (`arn:aws:kms:<region>:<account>:key/<key-id>`), persist a key record, and return `KeyMetadata` computed from it: `KeyId`, `Arn`, `AWSAccountId`, `CreationDate`, `Enabled: true`, `KeyState: Enabled`, `Description`, `KeyUsage`, `KeySpec` (+ matching deprecated `CustomerMasterKeySpec`), `Origin`, `KeyManager: CUSTOMER`, `EncryptionAlgorithms: [SYMMETRIC_DEFAULT]` for ENCRYPT_DECRYPT, `MultiRegion`.
2. **Encrypt** — input `KeyId` (required), `Plaintext` (required, 1..4096), `EncryptionContext`, `EncryptionAlgorithm`. Must resolve `KeyId` against stored keys (accepting raw key id, key ARN, `alias/<name>` and alias ARN), reject an unknown key with `NotFoundException` (404) and a disabled / pending-deletion key with `DisabledException` / `KMSInvalidStateException` (409). Returns `CiphertextBlob` that embeds the resolved key id, the encryption context and the plaintext (reversible by Decrypt — a real cipher is not required, but the blob must be opaque and NOT be the plaintext), plus `KeyId` (the ARN) and `EncryptionAlgorithm`.
3. **Decrypt** — input `CiphertextBlob`, optional `KeyId`, `EncryptionContext`, `EncryptionAlgorithm`. Must decode the blob produced by Encrypt/GenerateDataKey and return the ORIGINAL `Plaintext`, the `KeyId` ARN recorded in the blob, and `EncryptionAlgorithm`. An undecodable/foreign blob → `InvalidCiphertextException` (400); a mismatched `EncryptionContext` → `InvalidCiphertextException`; an explicit `KeyId` that disagrees with the blob → `IncorrectKeyException` (400); a key that has been deleted → `NotFoundException`.
4. **DescribeKey** — input `KeyId` (required), must return exactly the stored `KeyMetadata` (same `Arn`, `CreationDate`, `Description`, `KeyState`, `Enabled`, `KeySpec`) that CreateKey returned, reflecting any later DisableKey/ScheduleKeyDeletion state change; unknown id → `NotFoundException` (404).
5. **ListKeys** — `Limit`/`Marker` paginated: return one `KeyListEntry{KeyId, KeyArn}` per persisted key, honour `Limit` by truncating and setting `Truncated: true` + `NextMarker`, and resume from `Marker`; an unparsable marker → `InvalidMarkerException` (400).
6. **ScheduleKeyDeletion** — input `KeyId` (required), `PendingWindowInDays` (default 30, range 1..365). Must flip the stored key to `KeyState: PendingDeletion`, `Enabled: false`, set `DeletionDate = now + PendingWindowInDays`, and return `KeyId` (ARN), `DeletionDate`, `KeyState`, `PendingWindowInDays`. A subsequent DescribeKey must show `PendingDeletion`, and a subsequent Encrypt on that key must fail with `KMSInvalidStateException` (409). Scheduling an already-pending key → `KMSInvalidStateException`.

Every one of these must compute its response from persisted state — no canned responses, no echoing the input back, no random ids that are never stored. A `CreateKey` followed by `DescribeKey`/`ListKeys` must return what was created, and `Decrypt` must return the bytes a prior `Encrypt` was given.

## State Model

In `src/state.rs` (`ComAmazonawsKmsTrentserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `keys: HashMap<String /* key-id uuid */, Cmk>` where
  `Cmk { key_id: String, arn: String, aws_account_id: String, region: String, creation_date: SystemTime, enabled: bool, key_state: KeyState, description: String, key_usage: KeyUsageType, key_spec: KeySpec, origin: OriginType, key_manager: KeyManagerType, multi_region: bool, policy: String, deletion_date: Option<SystemTime>, pending_window_in_days: Option<i32>, tags: HashMap<String, String>, material: Vec<u8> /* 32 random bytes backing the key */, current_key_material_id: String /* 64 hex chars */ }`
- `aliases: HashMap<String /* "alias/<name>" */, String /* key-id */>` — populated later by CreateAlias; the resolver must already consult it so `KeyId` accepts alias forms.
- A key-id resolver helper: `fn resolve_key_id(&self, id: &str) -> Option<&Cmk>` handling raw uuid, key ARN, `alias/<name>` and alias ARN.
- Ciphertext envelope: a serialized struct `{ key_id, encryption_context, plaintext }` (e.g. versioned, base64/binary-encoded, optionally XOR'd with `material`) so Decrypt is the exact inverse of Encrypt and ciphertexts are not interchangeable between keys.
- `list_marker` pagination derived deterministically from a stable key ordering (e.g. insertion order vector `key_order: Vec<String>`), with `Marker` encoding the index/last-seen key id.

Out of scope for this first slice (stay 501, filed separately later): grants, key policies, rotation, asymmetric Sign/Verify/GetPublicKey, HMAC Generate/VerifyMac, import of key material, custom key stores, replication, tagging and alias operations beyond the resolver hook.

## Evidence

`services/service-com-amazonaws-kms-trentservice/OPERATIONS.md`: 54 operations, protocol `awsJson1_1`, target prefix `TrentService.`, sdkId `KMS`. CreateKey (line 57) → output `KeyMetadata`; Encrypt (line 136) → required `KeyId`+`Plaintext`, output `CiphertextBlob`/`KeyId`/`EncryptionAlgorithm`, errors `NotFoundException` 404, `DisabledException` 409, `KMSInvalidStateException` 409; Decrypt (line 63) → output `KeyId`/`Plaintext`/`EncryptionAlgorithm`, errors `InvalidCiphertextException` 400, `IncorrectKeyException` 400; DescribeKey (line 100) → `KeyMetadata`, `NotFoundException`; ListKeys (line 242) → @paginated(inputToken=Marker, outputToken=NextMarker, items=Keys) with `Truncated` required, `InvalidMarkerException`; ScheduleKeyDeletion (line 299) → `KeyId`/`DeletionDate`/`KeyState`/`PendingWindowInDays`. `smithy_query shape=KeyMetadata` gives the required `KeyId` plus `Arn`, `AWSAccountId`, `CreationDate`, `Enabled`, `KeyState`, `DeletionDate`, `KeySpec`, `CustomerMasterKeySpec` (same value as KeySpec per its doc), `EncryptionAlgorithms`, `KeyManager`. Every handler, e.g. `src/ops/create_key.rs:4`, still carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, so no state exists today.

## Steps to Reproduce

1. POST / with X-Amz-Target: TrentService.CreateKey, body {"Description":"bootstrap key","KeyUsage":"ENCRYPT_DECRYPT","KeySpec":"SYMMETRIC_DEFAULT"} → expect 200 with KeyMetadata containing a fresh KeyId, matching Arn, KeyState "Enabled", Enabled true (currently 501)
2. POST TrentService.DescribeKey {"KeyId":"<key-id>"} → expect the identical KeyMetadata (same Arn, CreationDate, Description)
3. POST TrentService.ListKeys {} → expect the new key present as {KeyId, KeyArn}; repeat with {"Limit":1} and follow NextMarker to page through all keys
4. POST TrentService.Encrypt {"KeyId":"<key-arn>","Plaintext":"<base64 of 'hello kms'>","EncryptionContext":{"app":"demo"}} → expect 200 with an opaque CiphertextBlob (not equal to the plaintext) and KeyId = key ARN
5. POST TrentService.Decrypt {"CiphertextBlob":"<blob>","EncryptionContext":{"app":"demo"}} → expect Plaintext to base64-decode back to 'hello kms' and KeyId to equal the key ARN
6. POST TrentService.Decrypt with the same blob but EncryptionContext {"app":"other"} → expect 400 InvalidCiphertextException
7. POST TrentService.Encrypt with {"KeyId":"00000000-0000-0000-0000-000000000000",...} → expect 404 NotFoundException
8. POST TrentService.ScheduleKeyDeletion {"KeyId":"<key-id>","PendingWindowInDays":7} → expect KeyState "PendingDeletion", DeletionDate ≈ now+7d, PendingWindowInDays 7
9. POST TrentService.DescribeKey {"KeyId":"<key-id>"} again → expect KeyState "PendingDeletion", Enabled false, DeletionDate set
10. POST TrentService.Encrypt on the pending-deletion key → expect 409 KMSInvalidStateException
