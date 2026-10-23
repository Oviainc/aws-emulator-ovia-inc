# Local additions to the Smithy model snapshot: NONE

`com-amazonaws-kms-trentservice.json` in this directory is a snapshot of the
published KMS model and is kept EXACTLY as shipped. Nothing in this crate edits
it, and nothing should: it is a third-party artefact consumed at build time by
`#[smithy_macros::smithy]`, so an edit here would be invisible to a reader of the
crate's source and lost on the next refresh.

One consequence is worth recording, because it is easy to mistake for a missing
feature. AWS answers `Decrypt` with a `KeyOrigin` member — the origin of the key
that opened the ciphertext, recorded in the `simparity-wire-aws-v1` wire fixture
`kms-symmetric-encrypt-decrypt` as `KeyOrigin: AWS_KMS` — while this snapshot
declares that member only on `DeriveSharedSecretResponse`. Rather than adding it
here, `src/ops/decrypt.rs` declares the member schema beside its own response
type and writes it after the generated shape's modelled members; the value comes
from the same stored field `KeyMetadata.Origin` projects.

`tests/all/decrypt_key_origin.rs` pins both halves: the member on the wire, and
this snapshot still not declaring it. If the snapshot is ever refreshed with a
`DecryptResponse$KeyOrigin` member, that test fails — and the local declaration
in `src/ops/decrypt.rs` should then give way to the modelled member.
