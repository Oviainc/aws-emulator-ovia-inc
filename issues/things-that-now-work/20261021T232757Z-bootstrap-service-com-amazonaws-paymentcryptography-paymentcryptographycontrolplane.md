Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: e44f583f966e818f9abdd86a5b0ab9a8f2f88230

# Bootstrap the AWS Payment Cryptography Control Plane emulator

_Filed: 2026-10-21T23:27:57Z_
_Operation: service-com-amazonaws-paymentcryptography-paymentcryptographycontrolplane | Class: missing_

> _Note: this issue was identified against the code base at commit `e44f583f966e818f9abdd86a5b0ab9a8f2f88230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 32 handlers under `src/ops/` is an untouched scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL slice: the key lifecycle plus alias binding, backed by real persisted state in `ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState`.

Happy path to implement, in order (awsJson1_0, `POST /` with `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`):

1. **CreateKey** — input `KeyAttributes` (required: `KeyUsage`, `KeyClass`, `KeyAlgorithm`, `KeyModesOfUse`), `Exportable` (required), `Enabled`, `KeyCheckValueAlgorithm`, `Tags`, `DeriveKeyUsage`, `ReplicationRegions`. Must mint a new key record and return `Key` with all required members populated from what was stored: `KeyArn` (`arn:aws:payment-cryptography:<region>:<account>:key/<16-64 alnum>`), the echoed-but-stored `KeyAttributes`, a deterministic hex `KeyCheckValue` (4..16 hex chars) derived from the generated key material, `KeyCheckValueAlgorithm` (default per algorithm: `ANSI_X9_24` for TDES, `CMAC` for AES, `SHA_1` for RSA/ECC when the caller omits it), `Enabled` (default true when omitted), `Exportable`, `KeyState = CREATE_COMPLETE`, `KeyOrigin = AWS_PAYMENT_CRYPTOGRAPHY`, `CreateTimestamp`, and `UsageStartTimestamp`.
2. **GetKey** — input `KeyIdentifier`, which may be a key ARN **or** an alias (`alias/...` name or alias ARN). Must resolve the identifier against stored state and return the SAME `Key` structure CreateKey returned (same ARN, KCV, attributes, state, timestamps). Unknown identifier -> `ResourceNotFoundException` (HTTP 404).
3. **CreateAlias** — input `AliasName` (`^alias/[a-zA-Z0-9/_-]+$`), optional `KeyArn`. Must store the alias, reject a second create of the same name with `ConflictException` (409), reject a `KeyArn` that names no stored key with `ResourceNotFoundException`, and return `Alias{AliasName, KeyArn}`. **GetAlias** returns the stored binding; 404 when absent.
4. **ListKeys** — input optional `KeyState`, `NextToken`, `MaxResults` (1..100). Must return `Keys` as `KeySummary` entries (`KeyArn`, `KeyState`, `KeyAttributes`, `KeyCheckValue`, `Exportable`, `Enabled`) built from stored keys, filtered by `KeyState` when supplied, truncated to `MaxResults` with an opaque `NextToken` that resumes exactly where the previous page stopped. (Same contract for **ListAliases**, optionally filtered by `KeyArn`.)
5. **DeleteKey** — input `KeyIdentifier`, optional `DeleteKeyInDays` (default 7). Must NOT drop the record: set `KeyState = DELETE_PENDING` and `DeletePendingTimestamp = now + DeleteKeyInDays` on the stored key and return the mutated `Key`. A following GetKey must show `DELETE_PENDING`; ListKeys with `KeyState=DELETE_PENDING` must include it, and with `CREATE_COMPLETE` must not.
6. **RestoreKey** — input `KeyIdentifier`. Must flip a `DELETE_PENDING` key back to `CREATE_COMPLETE`, clear `DeletePendingTimestamp`, and return the restored `Key`; calling it on a key that is not pending deletion is a `ConflictException` (409).

Every one of these must compute its response from persisted state — no canned bodies, no echoing the input back, no minting an ARN or KCV that is never stored. A create followed by a read must return exactly what was created, and a delete must be observable by the next reader.

## State Model

- `keys: BTreeMap<String /* KeyArn */, StoredKey>` — insertion-ordered so pagination is stable.
  - `StoredKey { key_arn: String, key_attributes: KeyAttributes /* key_usage, key_class, key_algorithm, key_modes_of_use */, key_check_value: String /* hex */, key_check_value_algorithm: KeyCheckValueAlgorithm, enabled: bool, exportable: bool, key_state: KeyState /* CREATE_COMPLETE | DELETE_PENDING | DELETE_COMPLETE */, key_origin: KeyOrigin, create_timestamp: SystemTime, usage_start_timestamp: Option<SystemTime>, usage_stop_timestamp: Option<SystemTime>, delete_pending_timestamp: Option<SystemTime>, delete_timestamp: Option<SystemTime>, derive_key_usage: Option<DeriveKeyUsage>, replication_regions: Vec<String>, tags: BTreeMap<String, String>, material: Vec<u8> /* generated secret, never returned; source of the KCV */ }`
- `aliases: BTreeMap<String /* AliasName */, Option<String> /* KeyArn */>`
- `key_counter: u64` — feeds the generated key id so ARNs are unique and reproducible.

Helpers the slice needs: `resolve_key_identifier(&str) -> Option<KeyArn>` (accepts key ARN, bare key id, `alias/<name>`, and alias ARN, following `aliases` to the key) and an opaque-token paginator shared by ListKeys/ListAliases/ListTagsForResource.

Out of scope for this first slice (stays 501, filed later as individual gap issues): ImportKey/ExportKey, GetParametersForImport/Export, GetCertificateSigningRequest, GetPublicKeyCertificate, Put/Get/DeleteResourcePolicy, Tag/Untag/ListTagsForResource, Update/DeleteAlias, StartKeyUsage/StopKeyUsage, the key-replication-region operations and the MPA-team operations.

## Evidence

`services/service-com-amazonaws-paymentcryptography-paymentcryptographycontrolplane/OPERATIONS.md` lists all 32 operations with these exact input/output members (CreateKey -> `Key`; GetKey/DeleteKey/RestoreKey keyed on `KeyIdentifier`; ListKeys `@paginated(inputToken=NextToken, outputToken=NextToken, items=Keys)` with `MaxResults` range 1..100; CreateAlias/GetAlias -> `Alias`; errors `ResourceNotFoundException (HTTP 404)` and `ConflictException (HTTP 409)` on each). `smithy_query shape=Key,KeySummary,KeyAttributes,Alias` gives the required response members cited above (`KeyArn`, `KeyAttributes`, `KeyCheckValue`, `KeyCheckValueAlgorithm`, `Enabled`, `Exportable`, `KeyState`, `KeyOrigin`, `CreateTimestamp` all `@required` on `Key`). `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 32 handler files, including `create_key.rs`, `get_key.rs`, `list_keys.rs`, `delete_key.rs`, `restore_key.rs`, `create_alias.rs` and `get_alias.rs` — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and touches no state. No integration test exercises any of this.

## Steps to Reproduce

1. POST / with X-Amz-Target: PaymentCryptographyControlPlane.CreateKey and body {"KeyAttributes":{"KeyUsage":"TR31_P0_PIN_ENCRYPTION_KEY","KeyClass":"SYMMETRIC_KEY","KeyAlgorithm":"TDES_3KEY","KeyModesOfUse":{"Encrypt":true,"Decrypt":true,"Wrap":true,"Unwrap":true}},"Exportable":true} — expect 200 with Key.KeyArn, Key.KeyCheckValue (hex), Key.KeyState=CREATE_COMPLETE, Key.KeyOrigin=AWS_PAYMENT_CRYPTOGRAPHY; today 501 NotImplemented
2. GetKey with {"KeyIdentifier":"<KeyArn from step 1>"} — expect the identical Key structure (same ARN, KCV, attributes, timestamps)
3. CreateAlias with {"AliasName":"alias/pin-key-1","KeyArn":"<KeyArn>"} then GetAlias {"AliasName":"alias/pin-key-1"} — expect Alias.KeyArn to equal the created key's ARN
4. GetKey with {"KeyIdentifier":"alias/pin-key-1"} — expect the alias to resolve to the same Key
5. ListKeys with {"MaxResults":1} after creating two keys — expect one KeySummary plus a NextToken, and the second page to return the other key with no NextToken
6. DeleteKey with {"KeyIdentifier":"<KeyArn>","DeleteKeyInDays":7} — expect Key.KeyState=DELETE_PENDING and a DeletePendingTimestamp ~7 days out; a following GetKey must still return the key in DELETE_PENDING
7. RestoreKey with {"KeyIdentifier":"<KeyArn>"} — expect Key.KeyState back to CREATE_COMPLETE with DeletePendingTimestamp cleared; GetKey confirms
8. GetKey with {"KeyIdentifier":"arn:aws:payment-cryptography:us-east-1:123456789012:key/doesnotexist0000"} — expect ResourceNotFoundException with HTTP 404, not 501

## Resolution notes

The slice above is implemented and answered from persisted state
(`src/state.rs` plus `src/ops/{create_key,get_key,create_alias,get_alias,list_keys,list_aliases,delete_key,restore_key}.rs`),
covered by `tests/all/key_lifecycle.rs`. Two rules were settled differently from this
issue's text, deliberately, and the implementation follows the settled rule:

- **CreateAlias on an identical retry answers 200, not 409.** `CreateAlias` carries
  `@idempotent` (`smithy_query(query="operation",operation="CreateAlias")` reports
  `"idempotent": true`; this crate's OPERATIONS.md lists it under
  `## CreateAlias / Traits: @idempotent`), and an idempotent operation must let the
  retry an SDK performs after a lost response succeed. So re-creating the SAME
  `AliasName` bound to the SAME `KeyArn` answers the stored binding, while rebinding an
  existing name to a DIFFERENT key — the inconsistent state `ConflictException`
  documents — is refused with `ConflictException` (409). The issue text above asked for
  409 on any second create of the name; that would break the modelled idempotency.
- **DeleteKey on a key already `DELETE_PENDING` answers it unchanged when the retry asks
  for the SAME waiting period**, for the same reason (`DeleteKey` is `@idempotent`): the
  retry an SDK performs after a lost response neither moves the stored
  `DeletePendingTimestamp` nor fails. A second request naming a DIFFERENT
  `DeleteKeyInDays` is not that retry — it would move a deletion date the caller was
  already promised — so it is refused with `ConflictException` (409) and the stored
  schedule is left alone; the key must be restored first. A key whose waiting period has
  already elapsed (`DELETE_COMPLETE`) is a `ConflictException` too. The waiting period the
  scheduling call asked for is persisted on the key record (not reported on the wire,
  which models no such member) because it is what distinguishes the two cases: the
  deletion instant is computed from the clock, so an identical retry a second later would
  not match it.

- **A list request that names no `MaxResults` is served fifty items, not a hundred.**
  That member's own documentation says so on every list operation of this service
  ("If you do not include a value, it defaults to 50"), so `ListKeys` and
  `ListAliases` serve a default page of 50 with a resuming `NextToken`, while 100
  stays the maximum the modelled range admits. Pinned by
  `a_list_request_with_no_max_results_serves_the_documented_default_page`.

Still 501 and left to their own gap issues, as this issue scoped them: ImportKey,
ExportKey, GetParametersForImport/Export, GetCertificateSigningRequest,
GetPublicKeyCertificate, Put/Get/DeleteResourcePolicy, Tag/Untag/ListTagsForResource,
UpdateAlias, DeleteAlias, Start/StopKeyUsage, the key-replication-region operations and
the MPA-team operations. Because nothing replicates a key or runs an approval yet,
`Key.MultiRegionKeyType`, `PrimaryRegion`, `ReplicationStatus` and `MpaStatus` are
reported absent rather than filled with a status no work produced.
