// SPDX-License-Identifier: CC0-1.0

//! @op Decrypt
//! IMPLEMENTED: returns the plaintext of a ciphertext this service can open, which is
//! one of two kinds. This is the ONLY file to touch for this operation.
//!
//! A ciphertext a SYMMETRIC encryption key sealed is the envelope `Encrypt` produced:
//! it names the key that sealed it, so `Decrypt` needs no `KeyId`; when one is given it
//! must name that same key. The AES-GCM tag is what distinguishes a ciphertext this
//! service produced from a foreign or altered one, and a matching encryption context
//! from a different one — both failures are `InvalidCiphertextException`. An encryption
//! context that is not a legal context at all never reaches that tag: it is a malformed
//! request, refused by the validation every operation taking one shares
//! ([`crate::state::check_encryption_context`]).
//!
//! A ciphertext an ASYMMETRIC encryption key opens was produced OUTSIDE KMS, by the
//! public half `GetPublicKey` published, with one of the `EncryptionAlgorithms` that key
//! advertises: it carries no metadata, so the request's `KeyId` is what names the key
//! and its `EncryptionAlgorithm` what names the scheme, and the private half this store
//! holds opens it ([`crate::state::decrypt_asymmetric`]). The algorithm is a
//! cryptographic parameter, so the same bytes declared under the other advertised digest
//! do not open and are refused `InvalidCiphertextException` like any ciphertext this key
//! did not receive. An algorithm the key's spec does not advertise, and a key whose
//! usage is not encryption and decryption, are `InvalidKeyUsageException`.
//!
//! Key material does not cross Regions. A ciphertext whose key this endpoint's Region
//! holds no record of — and a `KeyId` that is an ARN of another Region — names material
//! this endpoint cannot authorize, which is an authorization refusal
//! ([`crate::state::foreign_key_material`]) and not a report that the key is missing.
//! The material the keys of a multi-Region set share is held by every member, so a
//! replica opens its primary's ciphertext.
//!
//! `Recipient` (returning the plaintext encrypted to a Nitro enclave's attested
//! public key) is answered with the scaffold's 501: the capability is not
//! implemented, and no response here may carry a plaintext in its place.
//!
//! Permission is the service's one authorization decision
//! ([`crate::state::authorization`]): a request that presents NO `GrantTokens` is judged
//! against the key policy of the key that sealed the ciphertext, the calling principal's
//! identity-based policies and the grants on that key issued to that principal, and is
//! refused when none of them allows it.
//!
//! One deliberate divergence, the same one `Encrypt` carries and which
//! [`crate::state::authorization::authorize`] states once for every handler that shares
//! it: a request that DOES present a grant token draws its permission from the grants
//! those tokens name, so an otherwise-permitted call carrying an unrelated or stale token
//! is refused here, where the real service treats a grant token as additive.
//!
//! A request that presents NO grant token draws its permission from the policy of the key
//! that sealed the ciphertext, and the one thing that policy decides without a caller
//! identity is judged: [`crate::state::authorization`] refuses the request when every
//! statement granting this action is gated on a condition the request does not satisfy.
//!
//! ## `KeyOrigin`
//!
//! The response reports the ORIGIN of the key that opened the ciphertext, beside the
//! key's ARN and the id of the material used: AWS answers `Decrypt` with that member
//! (the `simparity-wire-aws-v1` recording of `kms-symmetric-encrypt-decrypt` carries
//! `KeyOrigin: AWS_KMS` for a symmetric-default CMK), while THIS model snapshot
//! declares it only on `DeriveSharedSecretResponse`
//! (`smithy_query(query="doc",shape="DeriveSharedSecretResponse",member="KeyOrigin")`,
//! whose wording it reuses). So the member is declared HERE, beside
//! [`DecryptResponse`], rather than by editing the vendored snapshot: the generated
//! shape writes every modelled member of `DecryptResponse` and this one is written
//! after it. The value is read from the key record that backs `KeyMetadata.Origin`, so
//! `Decrypt` and `DescribeKey` report one origin for one key.
//! `tests/all/decrypt_key_origin.rs` pins both halves: the member on the wire, and the
//! snapshot still not declaring it — if a refreshed snapshot ever does, that test
//! fails and this local declaration should give way to the modelled member.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `Decrypt::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `Decrypt`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json")
//! ```
//!
//! Decrypts ciphertext that was encrypted by a KMS key using any of the following
//! operations: Encrypt GenerateDataKey GenerateDataKeyPair GenerateDataKeyWithoutPlaintext
//! GenerateDataKeyPairWithoutPlaintext You can use this operation to decrypt ciphertext
//! that was encrypted under a symmetric encryption KMS key or an asymmetric encryption KMS
//! key. When the KMS key is asymmetric, you must specify the KMS key and the encryption
//! algorithm that was used to encrypt the ciphertext. For information about asymmetric KMS
//! keys, see Asymmetric KMS keys in the Key Management Service Developer Guide. The Decrypt
//! operation also decrypts ciphertext that was encrypted outside of KMS by the public key
//! in an KMS asymmetric KMS key. However, it cannot decrypt symmetric ciphertext produced
//! by other libraries, such as the Amazon Web Services Encryption SDK or Amazon S3
//! client-side encryption. These libraries return a ciphertext format that is incompatible
//! with KMS. If the ciphertext was encrypted under a symmetric encryption KMS key, the
//! KeyId parameter is optional. KMS can get this information from metadata that it adds to
//! the symmetric ciphertext blob. This feature adds durability to your implementation by
//! ensuring that authorized users can decrypt ciphertext decades after it was encrypted,
//! even if they've lost track of the key ID. However, specifying the KMS key is always
//! recommended as a best practice. When you use the KeyId parameter to specify a KMS key,
//! KMS only uses the KMS key you specify. If the ciphertext was encrypted under a different
//! KMS key, the Decrypt operation fails. This practice ensures that you use the KMS key
//! that you intend. Whenever possible, use key policies to give users permission to call
//! the Decrypt operation on a particular KMS key, instead of using IAM policies. Otherwise,
//! you might create an IAM policy that gives the user Decrypt permission on all KMS keys.
//! This user could decrypt ciphertext that was encrypted by KMS keys in other accounts if
//! the key policy for the cross-account KMS key permits it. If you must use an IAM policy
//! for Decrypt permissions, limit the user to particular KMS keys or particular trusted
//! accounts. For details, see Best practices for IAM policies in the Key Management Service
//! Developer Guide. Decrypt also supports Amazon Web Services Nitro Enclaves and NitroTPM,
//! which provide attested environments in Amazon EC2. To call Decrypt for a Nitro enclave
//! or NitroTPM, use the Amazon Web Services Nitro Enclaves SDK or any Amazon Web Services
//! SDK. Use the Recipient parameter to provide the attestation document for the attested
//! environment. Instead of the plaintext data, the response includes the plaintext data
//! encrypted with the public key from the attestation document (CiphertextForRecipient).
//! For information about the interaction between KMS and Amazon Web Services Nitro Enclaves
//! or Amazon Web Services NitroTPM, see Cryptographic attestation support in KMS in the Key
//! Management Service Developer Guide. The KMS key that you …
//!
//! ## Input
//!
//! * `CiphertextBlob` (CiphertextType, len 1..6144) — Ciphertext to be decrypted. The blob
//!   includes metadata. This parameter is required in all cases except when DryRun is true
//!   and DryRunModifiers is set to IGNORE_CIPHERTEXT.
//! * `DryRun` (NullableBooleanType) — Checks if your request will succeed. DryRun is an
//!   optional parameter. To learn more about how to use this parameter, see Testing your
//!   permissions in the Key Management Service Developer Guide.
//! * `DryRunModifiers` (DryRunModifierList) — Specifies the modifiers to apply to the dry run
//!   operation. DryRunModifiers is an optional parameter that only applies when DryRun is set
//!   to true. When set to IGNORE_CIPHERTEXT, KMS performs only authorization validation
//!   without ciphertext validation. This allows you to test permissions without requiring a
//!   valid ciphertext blob. To learn more about how to use this parameter, see Testing your
//!   permissions in the Key Management Service Developer Guide.
//! * `EncryptionAlgorithm` (EncryptionAlgorithmSpec, one of: RSAES_OAEP_SHA_1 |
//!   RSAES_OAEP_SHA_256 | SM2PKE | SYMMETRIC_DEFAULT) — Specifies the encryption algorithm
//!   that will be used to decrypt the ciphertext. Specify the same algorithm that was used to
//!   encrypt the data. If you specify a different algorithm, the Decrypt operation fails.
//!   This parameter is required only when the ciphertext was encrypted under an asymmetric
//!   KMS key. The default value, SYMMETRIC_DEFAULT, represents the only supported algorithm
//!   that is valid for symmetric encryption KMS keys.
//! * `EncryptionContext` (EncryptionContextType) — Specifies the encryption context to use
//!   when decrypting the data. An encryption context is valid only for cryptographic
//!   operations with a symmetric encryption KMS key. The standard asymmetric encryption
//!   algorithms and HMAC algorithms that KMS uses do not support an encryption context. An
//!   encryption context is a collection of non-secret key-value pairs that represent
//!   additional authenticated data. When you use an encryption context to encrypt data, you
//!   must specify the same (an exact case-sensitive match) encryption context to decrypt the
//!   data. An encryption context is supported only on operations with symmetric encryption
//!   KMS keys. On operations with symmetric encryption KMS keys, an …
//! * `GrantTokens` (GrantTokenList, len 0..10) — A list of grant tokens. Use a grant token
//!   when your permission to call this operation comes from a new grant that has not yet
//!   achieved eventual consistency. For more information, see Grant token and Using a grant
//!   token in the Key Management Service Developer Guide.
//! * `KeyId` (KeyIdType, len 1..2048) — Specifies the KMS key that KMS uses to decrypt the
//!   ciphertext. Enter a key ID of the KMS key that was used to encrypt the ciphertext. If
//!   you identify a different KMS key, the Decrypt operation throws an IncorrectKeyException.
//!   This parameter is required only when the ciphertext was encrypted under an asymmetric
//!   KMS key or when DryRun is true and DryRunModifiers is set to IGNORE_CIPHERTEXT. If you
//!   used a symmetric encryption KMS key, KMS can get the KMS key from metadata that it adds
//!   to the symmetric ciphertext blob. However, it is always recommended as a best practice.
//!   This practice ensures that you use the KMS key that you intend. To specify a KMS key,
//!   use its key ID, key ARN, alias name, …
//! * `Recipient` (RecipientInfo) — A signed attestation document from an Amazon Web Services
//!   Nitro enclave or NitroTPM, and the encryption algorithm to use with the public key in
//!   the attestation document. The only valid encryption algorithm is RSAES_OAEP_SHA_256.
//!   This parameter supports the Amazon Web Services Nitro Enclaves SDK or any Amazon Web
//!   Services SDK for Amazon Web Services Nitro Enclaves. It supports any Amazon Web Services
//!   SDK for Amazon Web Services NitroTPM. When you use this parameter, instead of returning
//!   the plaintext data, KMS encrypts the plaintext data with the public key in the
//!   attestation document, and returns the resulting ciphertext in the CiphertextForRecipient
//!   field in the response. This ciphertext …
//!
//! ## Output
//!
//! * `CiphertextForRecipient` (CiphertextType, len 1..6144) — The plaintext data encrypted
//!   with the public key from the attestation document. This ciphertext can be decrypted only
//!   by using a private key from the attested environment. This field is included in the
//!   response only when the Recipient parameter in the request includes a valid attestation
//!   document from an Amazon Web Services Nitro enclave or NitroTPM. For information about
//!   the interaction between KMS and Amazon Web Services Nitro Enclaves or Amazon Web
//!   Services NitroTPM, see Cryptographic attestation support in KMS in the Key Management
//!   Service Developer Guide.
//! * `EncryptionAlgorithm` (EncryptionAlgorithmSpec, one of: RSAES_OAEP_SHA_1 |
//!   RSAES_OAEP_SHA_256 | SM2PKE | SYMMETRIC_DEFAULT) — The encryption algorithm that was
//!   used to decrypt the ciphertext.
//! * `KeyId` (KeyIdType, len 1..2048) — The Amazon Resource Name (key ARN) of the KMS key
//!   that was used to decrypt the ciphertext.
//! * `KeyMaterialId` (BackingKeyIdType, len 64..64, pattern ^[a-f0-9]+$) — The identifier of
//!   the key material used to decrypt the ciphertext. This field is present only when the
//!   operation uses a symmetric encryption KMS key. This field is omitted if the request
//!   includes the Recipient parameter.
//!
//! Beside those, the response carries `KeyOrigin` (OriginType), which this model snapshot
//! does not declare for `Decrypt` — see "## `KeyOrigin`" above.
//! * `Plaintext` (PlaintextType, len 1..4096) — Decrypted plaintext data. When you use the
//!   HTTP API or the Amazon Web Services CLI, the value is Base64-encoded. Otherwise, it is
//!   not Base64-encoded. If the response includes the CiphertextForRecipient field, the
//!   Plaintext field is null or empty.
//!
//! ## Errors
//!
//! * `DependencyTimeoutException` (HTTP 503) — The system timed out while trying to fulfill
//!   the request. You can retry the request.
//! * `DisabledException` (HTTP 409) — The request was rejected because the specified KMS key
//!   is not enabled.
//! * `DryRunOperationException` (HTTP 412) — The request was rejected because the DryRun
//!   parameter was specified.
//! * `IncorrectKeyException` (HTTP 400) — The request was rejected because the specified KMS
//!   key cannot decrypt the data. The KeyId in a Decrypt request and the SourceKeyId in a
//!   ReEncrypt request must identify the same KMS key that was used to encrypt the
//!   ciphertext.
//! * `InvalidCiphertextException` (HTTP 400) — From the Decrypt or ReEncrypt operation, the
//!   request was rejected because the specified ciphertext, or additional authenticated data
//!   incorporated into the ciphertext, such as the encryption context, is corrupted, missing,
//!   or otherwise invalid. From the ImportKeyMaterial operation, the request was rejected
//!   because KMS could not decrypt the encrypted (wrapped) key material.
//! * `InvalidGrantTokenException` (HTTP 400) — The request was rejected because the specified
//!   grant token is not valid.
//! * `InvalidKeyUsageException` (HTTP 400) — The request was rejected for one of the
//!   following reasons: The KeyUsage value of the KMS key is incompatible with the API
//!   operation. The encryption algorithm or signing algorithm specified for the operation is
//!   incompatible with the type of key material in the KMS key (KeySpec). For encrypting,
//!   decrypting, re-encrypting, and generating data keys, the KeyUsage must be
//!   ENCRYPT_DECRYPT. For signing and verifying messages, the KeyUsage must be SIGN_VERIFY.
//!   For generating and verifying message authentication codes (MACs), the KeyUsage must be
//!   GENERATE_VERIFY_MAC. For deriving key agreement secrets, the KeyUsage must be
//!   KEY_AGREEMENT. To find the KeyUsage of a KMS key, use the DescribeKey …
//! * `KeyUnavailableException` (HTTP 500) — The request was rejected because the specified
//!   KMS key was not available. You can retry the request.
//! * `KMSInternalException` (HTTP 500) — The request was rejected because an internal
//!   exception occurred. The request can be retried.
//! * `KMSInvalidStateException` (HTTP 409) — The request was rejected because the state of
//!   the specified resource is not valid for this request. This exceptions means one of the
//!   following: The key state of the KMS key is not compatible with the operation. To find
//!   the key state, use the DescribeKey operation. For more information about which key
//!   states are compatible with each KMS operation, see Key states of KMS keys in the Key
//!   Management Service Developer Guide . For cryptographic operations on KMS keys in custom
//!   key stores, this exception represents a general failure with many possible causes. To
//!   identify the cause, see the error message that accompanies the exception.
//! * `NotFoundException` (HTTP 404) — The request was rejected because the specified entity
//!   or resource could not be found.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DecryptError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kms-trentservice.json");

/// The `KeyOrigin` response member: "The source of the key material for the specified
/// KMS key. When this value is `AWS_KMS`, KMS created the key material. When this
/// value is `EXTERNAL`, the key material was imported or the KMS key doesn't have any
/// key material."
/// (`smithy_query(query="doc",shape="DeriveSharedSecretResponse",member="KeyOrigin")` —
/// the one shape this snapshot declares the member on).
///
/// It is declared here because the snapshot does not declare it on `DecryptResponse`
/// while AWS answers `Decrypt` with it. The member schema carries the wire name, which
/// is what the JSON codec emits the key from; the index follows the five modelled
/// members of `DecryptResponse`.
static KEY_ORIGIN_MEMBER: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
    ::aws_smithy_schema::ShapeId::from_static(
        "com.amazonaws.kms#DecryptResponse$KeyOrigin",
        "com.amazonaws.kms",
        "KeyOrigin",
    ),
    ::aws_smithy_schema::ShapeType::String,
    "KeyOrigin",
    5,
);

/// What `Decrypt` answers: every modelled member of `DecryptResponse`, plus
/// `KeyOrigin`.
///
/// The modelled half IS the generated shape, serialized by its own generated
/// implementation against the generated schema, so no modelled member is re-described
/// here; this type only adds the one member the snapshot lacks.
pub struct DecryptResponse {
    modelled: DecryptOutput,
    /// The `OriginType` of the key that opened the ciphertext — the same value
    /// `DescribeKey` reports as `KeyMetadata.Origin`.
    key_origin: Option<String>,
}

impl ::aws_smithy_schema::serde::SerializableStruct for DecryptResponse {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        ::aws_smithy_schema::serde::SerializableStruct::serialize_members(
            &self.modelled,
            serializer,
        )?;
        if let Some(key_origin) = &self.key_origin {
            serializer.write_string(&KEY_ORIGIN_MEMBER, key_origin)?;
        }
        Ok(())
    }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DecryptError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.kms#Decrypt")]
#[derive(Debug)]
pub enum DecryptErrorModeled {
    DependencyTimeoutException(DependencyTimeoutException),
    DisabledException(DisabledException),
    DryRunOperationException(DryRunOperationException),
    IncorrectKeyException(IncorrectKeyException),
    InvalidCiphertextException(InvalidCiphertextException),
    InvalidGrantTokenException(InvalidGrantTokenException),
    InvalidKeyUsageException(InvalidKeyUsageException),
    KeyUnavailableException(KeyUnavailableException),
    KMSInternalException(KMSInternalException),
    KMSInvalidStateException(KMSInvalidStateException),
    NotFoundException(NotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::decrypt::DecryptError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DecryptError = crate::scaffolded::Scaffolded<DecryptErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DecryptErrorModeled> for DecryptError {
    fn from(modeled: DecryptErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `Decrypt` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct Decrypt<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> Decrypt<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for Decrypt<Eng>
{
    type Input = DecryptInput;
    type Output = DecryptResponse;
    type Error = DecryptError;

    const NAME: &'static str = "Decrypt";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DECRYPT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DECRYPT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DecryptErrorModeled::KMSInternalException(KMSInternalException {
                message: Some("the KMS state store is unavailable".to_string()),
            })
            .into());
        };

        // `Recipient` asks KMS to return the plaintext encrypted under the public key
        // in a Nitro enclave's attestation document. Nothing here can verify an
        // attestation, so that capability is reported as not implemented rather than
        // as a malformed request — and never answered with a plaintext the caller did
        // not ask for.
        if input.recipient.is_some() {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "Decrypt with a Recipient (enclave attestation)",
            ));
        }

        // The encryption context is INPUT, judged before anything is looked up: a
        // context that is not a legal encryption context is a malformed request
        // (`crate::state::check_encryption_context`), not a ciphertext that failed to
        // authenticate — the distinction a caller needs, because one is fixed in the
        // caller and the other is a ciphertext that is not its own.
        crate::state::check_encryption_context(input.encryption_context.as_ref())?;

        // The model does not mark `CiphertextBlob` required, so an absent one reaches
        // the handler; it is a malformed request rather than an invalid ciphertext.
        let Some(blob) = input.ciphertext_blob.as_deref() else {
            return Err(crate::scaffolded::Scaffolded::validation(
                "CiphertextBlob is required and was not provided",
            ));
        };

        // Who the request speaks for, resolved BEFORE this service's store is locked, so
        // the IAM and STS stores the resolution reads are never reached while this
        // service's guard is held.
        let caller = crate::state::authorization::caller(&*self.engine, ctx);
        let region = crate::state::request_region(ctx);
        let mut state = state.lock();

        // Permission a request draws from a grant is judged below, once the key is
        // known and its key state has been judged.
        let tokens = input.grant_tokens.clone().unwrap_or_default();

        // WHICH SCHEME produced this ciphertext follows from the KEY, not from the
        // bytes. A ciphertext an ASYMMETRIC key opens was produced outside KMS, under
        // the public half `GetPublicKey` published, and carries no metadata at all —
        // "the standard format for asymmetric key ciphertext does not include
        // configurable fields" — so nothing in it names a key and the request must:
        // `KeyId` "is required only when the ciphertext was encrypted under an
        // asymmetric KMS key"
        // (`smithy_query(query="doc",shape="DecryptRequest",member="KeyId")`). A
        // ciphertext a SYMMETRIC encryption key sealed carries "metadata that it adds
        // to the symmetric ciphertext blob", which is this service's envelope, and so
        // names its own key.
        let asymmetric = match input.key_id.as_deref() {
            Some(requested) => match state.resolve(&region, requested) {
                crate::state::KeyRef::Found(requested_key) if requested_key.is_asymmetric() => {
                    Some(requested_key)
                }
                _ => None,
            },
            None => None,
        };

        // The envelope of a ciphertext this service's `Encrypt` sealed — `None` for a
        // raw ciphertext an asymmetric key opens, which is not an envelope at all.
        let mut envelope = None;
        let key = match asymmetric {
            Some(key) => key,
            None => {
                let Some(parsed) = crate::state::parse_envelope(blob) else {
                    return Err(invalid_ciphertext());
                };

                // The envelope names the key whose material sealed it, and this endpoint
                // can open the ciphertext only with a key of its OWN Region:
                // single-Region key material does not travel, so a ciphertext whose key
                // is not held here names material this endpoint cannot authorize. That is
                // an authorization decision and not a lookup — KMS does not disclose
                // whether that key exists — so it is refused with `AccessDeniedException`
                // rather than reported as a missing resource. Material the keys of a
                // multi-Region set SHARE is held by every member, so a replica opens its
                // primary's ciphertext here as usual.
                let Some(key) = state.key_in(&region, &parsed.key_id) else {
                    return Err(crate::state::foreign_key_material());
                };

                // "Specify the key ID ... of the KMS key" only when the caller wants KMS
                // to check its assumption. A `KeyId` that names an EXISTING key other than
                // the one which sealed the ciphertext is an `IncorrectKeyException`; one
                // that names no key at all is the same `NotFoundException` any other
                // operation answers for it (`Decrypt` models no `InvalidArnException`, so
                // a malformed ARN is reported the same way).
                if let Some(requested) = input.key_id.as_deref() {
                    match state.resolve(&region, requested) {
                        crate::state::KeyRef::Found(requested_key)
                            if requested_key.key_id == key.key_id => {}
                        crate::state::KeyRef::Found(_) => {
                            return Err(incorrect_key());
                        }
                        crate::state::KeyRef::UnauthorizedAccount => {
                            return Err(crate::state::unauthorized_account(Self::NAME));
                        }
                        // A `KeyId` that is an ARN of another Region names a key this
                        // endpoint cannot authorize for the ciphertext either, so it is
                        // refused the same way the ciphertext's own foreign key is: on
                        // authorization, not as a lookup miss.
                        crate::state::KeyRef::NotFound | crate::state::KeyRef::MalformedArn
                            if crate::state::names_another_region(&region, requested) =>
                        {
                            return Err(crate::state::foreign_key_material());
                        }
                        crate::state::KeyRef::NotFound | crate::state::KeyRef::MalformedArn => {
                            return Err(DecryptErrorModeled::NotFoundException(
                                NotFoundException {
                                    message: Some(crate::state::key_not_found_message(
                                        &region, requested,
                                    )),
                                },
                            )
                            .into());
                        }
                    }
                }
                envelope = Some(parsed);
                key
            }
        };

        if !key.is_usable() {
            if key.key_state == crate::state::KEY_STATE_DISABLED {
                return Err(DecryptErrorModeled::DisabledException(DisabledException {
                    message: Some(crate::state::invalid_state_message(key)),
                })
                .into());
            }
            return Err(DecryptErrorModeled::KMSInvalidStateException(
                KMSInvalidStateException {
                    message: Some(crate::state::invalid_state_message(key)),
                },
            )
            .into());
        }

        // A grant token is handed out by `CreateGrant` and names the grant it was
        // minted for. A token that is not one this service minted names no grant at
        // all, so the TOKEN "is not valid".
        //
        // Everything else about permission is the service's one authorization decision
        // (`crate::state::authorization`): the key's own key policy, the calling
        // principal's identity policies, and the grants on this key the caller holds or
        // presented a token for, judged together. A request presenting no token is
        // judged too — a grant propagates to its grantee principal without one, and a
        // caller the key policy does not reach has no permission whether or not it
        // presents a token.
        let named = match state.grants_for_tokens(&region, &tokens) {
            Ok(named) => named,
            Err(crate::state::GrantRefusal::MalformedToken) => {
                return Err(DecryptErrorModeled::InvalidGrantTokenException(
                    InvalidGrantTokenException {
                        message: Some("The specified grant token is not valid.".to_string()),
                    },
                )
                .into());
            }
            // Every token is well formed but names a grant the store no longer holds,
            // which carries no permission; what else the caller holds is judged below.
            Err(crate::state::GrantRefusal::NotAuthorized) => Vec::new(),
        };
        let presented = (!tokens.is_empty()).then_some(named.as_slice());
        if let Err(message) = crate::state::authorization::authorize(
            &caller,
            &state,
            &region,
            key,
            Self::NAME,
            input.encryption_context.as_ref(),
            presented,
        ) {
            return Err(crate::scaffolded::Scaffolded::access_denied(message));
        }

        // "Specify the same algorithm that was used to encrypt the data", whose
        // default "SYMMETRIC_DEFAULT represents the only supported algorithm that is
        // valid for symmetric encryption KMS keys"
        // (`smithy_query(query="doc",shape="DecryptRequest",member="EncryptionAlgorithm")`).
        let algorithm = input
            .encryption_algorithm
            .clone()
            .unwrap_or_else(|| crate::state::ENCRYPTION_ALGORITHM_SYMMETRIC_DEFAULT.to_string());

        // Which algorithms can open a ciphertext of this key is exactly the key's own
        // `EncryptionAlgorithms` — the list `DescribeKey` and `GetPublicKey` report for
        // it, which follows from its key spec — and a key with no such list cannot
        // decrypt at all: "for encrypting, decrypting, re-encrypting, and generating
        // data keys, the KeyUsage must be ENCRYPT_DECRYPT"
        // (`smithy_query(query="doc",shape="InvalidKeyUsageException")`).
        let Some(supported) = key.algorithms_under(crate::state::KEY_USAGE_ENCRYPT_DECRYPT) else {
            return Err(invalid_key_usage(crate::state::wrong_key_usage_message(
                key,
                Self::NAME,
            )));
        };
        if !supported.contains(&algorithm) {
            return Err(invalid_key_usage(
                crate::state::incompatible_algorithm_message(&algorithm, &key.key_spec),
            ));
        }

        if input.dry_run == Some(true) {
            return Err(DecryptErrorModeled::DryRunOperationException(
                DryRunOperationException {
                    message: Some(
                        "The request was rejected because the DryRun parameter was specified."
                            .to_string(),
                    ),
                },
            )
            .into());
        }

        let (plaintext, key_material_id) = match &envelope {
            // A ciphertext this service's `Encrypt` sealed. The encryption context is
            // authenticated, not stored: a context that differs from the one `Encrypt`
            // was given fails the tag, exactly as a tampered ciphertext does. Which
            // VERSION of the key's material sealed the ciphertext is read from the same
            // tag, so a ciphertext sealed before a rotation opens under the material it
            // was sealed with.
            Some(envelope) => {
                let Some(opened) =
                    crate::state::unseal(key, envelope, input.encryption_context.as_ref())
                else {
                    return Err(invalid_ciphertext());
                };
                (opened.plaintext, opened.key_material_id)
            }
            // A ciphertext produced OUTSIDE KMS under the public half of this key's key
            // pair: "the Decrypt operation also decrypts ciphertext that was encrypted
            // outside of KMS by the public key in an KMS asymmetric KMS key"
            // (`smithy_query(query="doc",shape="Decrypt")`). It is opened with the
            // private half this store holds, under the parameters the declared algorithm
            // names — so a ciphertext produced under a DIFFERENT advertised algorithm
            // than the one declared does not open, which is what makes the algorithm a
            // cryptographic parameter and not a label. No `KeyMaterialId` accompanies
            // it: that member "is present only when the operation uses a symmetric
            // encryption KMS key".
            None => match crate::state::decrypt_asymmetric(key, &algorithm, blob) {
                Ok(plaintext) => (plaintext, None),
                Err(crate::state::AsymmetricRefusal::NotThisCiphertext) => {
                    return Err(invalid_ciphertext());
                }
                Err(crate::state::AsymmetricRefusal::Unsupported) => {
                    return Err(crate::scaffolded::Scaffolded::not_implemented(&format!(
                        "Decrypt with the {algorithm} encryption algorithm"
                    )));
                }
                // The scheme is implemented and the library refused the operation itself,
                // which says nothing about the request: a server fault, not a refusal.
                Err(crate::state::AsymmetricRefusal::Failed) => {
                    return Err(DecryptErrorModeled::KMSInternalException(
                        KMSInternalException {
                            message: Some("the ciphertext could not be decrypted".to_string()),
                        },
                    )
                    .into());
                }
            },
        };
        let key_arn = key.arn.clone();
        let key_origin = key.origin.clone();
        let key_id = key.key_id.clone();

        // The key was just used in a successful cryptographic operation, which is what
        // `GetKeyLastUsage` reports once the tracking has produced the record.
        state.record_last_usage(&region, &key_id, Self::NAME);

        Ok(DecryptResponse {
            modelled: DecryptOutput {
                key_id: Some(key_arn),
                plaintext: Some(plaintext),
                encryption_algorithm: Some(algorithm),
                // Only a `Recipient` request has a ciphertext for one, and those are
                // refused above.
                ciphertext_for_recipient: None,
                // "The identifier of the key material used to decrypt the ciphertext.
                // This field is present only when the operation uses a symmetric
                // encryption KMS key" (`smithy_query(shape="DecryptResponse")`), which
                // is exactly the material that carries an id — the version that
                // actually opened this ciphertext, not whichever version is current.
                key_material_id,
            },
            // Where that key's material comes from — the same field `DescribeKey`
            // reports as `KeyMetadata.Origin`, read from the one key record so the two
            // answers can never disagree. A client branches on it to decide how the
            // backing material is held (`AWS_KMS`, `EXTERNAL`, a CloudHSM or external
            // key store), so a null here is not the same answer as a value.
            key_origin: Some(key_origin),
        })
    }
}

/// The ciphertext is not one this service can open: it is not an envelope and no key the
/// request names could have produced one, its authentication tag did not verify under the
/// key and encryption context given, or the private half of the asymmetric key named does
/// not open it under the declared encryption algorithm.
///
/// The refusal carries NO message. What it would have to say is which of the ciphertext,
/// the key and the encryption context did not agree, and a caller who presented a
/// ciphertext it cannot open is told none of that: the exception shape's own
/// documentation is API-reference prose, not a message the wire carries.
fn invalid_ciphertext() -> DecryptError {
    DecryptErrorModeled::InvalidCiphertextException(InvalidCiphertextException {
        message: None,
    })
    .into()
}

/// Refuse a key that cannot decrypt, or an algorithm its key spec does not support:
/// "the KeyUsage value of the KMS key is incompatible with the API operation" and "the
/// encryption algorithm ... specified for the operation is incompatible with the type of
/// key material in the KMS key (KeySpec)"
/// (`smithy_query(query="doc",shape="InvalidKeyUsageException")`) — the two reasons that
/// error names, which is why one refusal serves both.
fn invalid_key_usage(message: String) -> DecryptError {
    DecryptErrorModeled::InvalidKeyUsageException(InvalidKeyUsageException {
        message: Some(message),
    })
    .into()
}

/// The `KeyId` the request named is not the key whose material sealed the ciphertext.
///
/// The message is [`crate::state::INCORRECT_KEY_MESSAGE`], the one wording the whole
/// service gives this refusal: fixed text that names NEITHER key, because the caller
/// asked whether the key it named can perform this operation on this ciphertext and the
/// answer is that it cannot — naming the key that CAN would disclose a key the caller
/// never presented.
fn incorrect_key() -> DecryptError {
    DecryptErrorModeled::IncorrectKeyException(IncorrectKeyException {
        message: Some(crate::state::INCORRECT_KEY_MESSAGE.to_string()),
    })
    .into()
}
