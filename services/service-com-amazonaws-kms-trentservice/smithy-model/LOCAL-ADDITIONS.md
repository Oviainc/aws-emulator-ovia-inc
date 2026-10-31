# Local additions to the Smithy model snapshot: NONE

`com-amazonaws-kms-trentservice.json` in this directory is a snapshot of the
published KMS model and is kept EXACTLY as shipped. Nothing in this crate edits
it, and nothing should: it is a third-party artefact consumed at build time by
`#[smithy_macros::smithy]`, so an edit here would be invisible to a reader of the
crate's source and lost on the next refresh.

One consequence is worth recording, because it is easy to mistake for a missing
feature: AWS answers some operations with members this snapshot does not declare.
Each is declared next to the response it belongs to and written after the
generated shape's modelled members, rather than being added here.

- `Decrypt` reports `KeyOrigin` — the origin of the key that opened the
  ciphertext, recorded in the `simparity-wire-aws-v1` wire fixture
  `kms-symmetric-encrypt-decrypt` as `KeyOrigin: AWS_KMS` — while this snapshot
  declares that member only on `DeriveSharedSecretResponse`. `src/ops/decrypt.rs`
  declares the member schema beside its own response type; the value comes from
  the same stored field `KeyMetadata.Origin` projects.
- `CreateGrant` reports `KeyOrigin` for the key the grant was created on, while
  this snapshot declares `CreateGrantResponse` with `GrantId` and `GrantToken`
  only. `src/ops/create_grant.rs` declares it the same way, from the same stored
  field.
- `ListGrants` and `ListRetirableGrants` report `GrantCount`, the number of
  grants the answered `Grants` list holds (including `0` beside an empty list),
  while this snapshot declares `ListGrantsResponse` with `Grants`, `NextMarker`
  and `Truncated` only. Both readers answer that one shape, so the member is
  declared once, beside it, in `src/ops/types/shared_types.rs`.

`tests/all/decrypt_key_origin.rs` and `tests/all/grant_read_projection.rs` pin
both halves: the members on the wire, and this snapshot still not declaring them.
If the snapshot is ever refreshed with any of them, those tests fail — and the
local declaration should then give way to the modelled member.
