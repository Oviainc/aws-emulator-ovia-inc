// SPDX-License-Identifier: CC0-1.0

//! The envelope a secret version's value is stored under, and the KMS calls that
//! seal and open it.
//!
//! A stored value is not held as it arrived. "Secrets Manager uses the Amazon Web
//! Services managed key `aws/secretsmanager`" unless the secret names a key of its
//! own, in which case the value is "encrypted using a customer managed key"
//! (`CreateSecretRequest$KmsKeyId`, this service's Smithy model) — and a value
//! encrypted under a key is a value the service cannot produce, or replace, without
//! that key. So for a secret naming a customer managed key:
//!
//! * a WRITE asks that key for a fresh data key (`kms:GenerateDataKey`), seals the
//!   value under it with AES-256-GCM, and stores the wrapped data key beside the
//!   ciphertext — the plaintext is never stored;
//! * a READ asks that key to unwrap the data key the version was sealed with
//!   (`kms:Decrypt`) and opens the ciphertext with it.
//!
//! That is why the key's own state decides both paths: a key KMS will not perform a
//! cryptographic operation with cannot produce a data key and cannot unwrap one, so
//! the write fails with `EncryptionFailure` and the read with `DecryptionFailure`,
//! each relaying the KMS refusal it met. Nothing here inspects the key's state
//! itself: KMS owns that judgement, and this module asks it.
//!
//! A secret left on the service-managed key names no key a caller can act on — it
//! cannot be disabled, scheduled for deletion or deleted — so there is no refusal
//! for this module to meet and the value is held as sent.
//!
//! KMS is reached the way the rest of this emulator reaches another service: the AWS
//! SDK driven over the emulator's own in-process transport, through the operation
//! callback ([`smithy_server_runtime::OperationContext::callback`]), so no socket is
//! opened and this crate names no other service's crate. An emulator assembled
//! WITHOUT KMS is the one case a value under a customer managed key is held as sent:
//! the transport itself refuses the call, nothing is said about the key, and a
//! service that refused every write of a value would be unusable in such an
//! emulator. Every other failure leaves the write refused and nothing stored, and a
//! READ of a value that IS sealed has no way to open it and is refused.
//!
//! The AEAD and the nonce come from `ring` (`ring::aead::AES_256_GCM`,
//! `ring::rand::SystemRandom`); the data key itself is KMS's, never this service's.

use std::collections::HashMap;
use std::fmt;

use ::aws_sdk_kms::error::{ProvideErrorMetadata, SdkError};
use ::aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use ::ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ::ring::digest::{digest, SHA256};
use ::ring::rand::{SecureRandom, SystemRandom};

/// How the `Message` of `DecryptionFailure` introduces the KMS refusal a read met.
///
/// This is the clause the service answers WITH THE RELAY on the wire: the recorded
/// refusal of a read of a secret whose customer managed key is disabled is
/// "Secrets Manager can't decrypt the secret value: &lt;the KMS refusal&gt;". The
/// model's own sentence for the error class ([`DECRYPTION_FAILURE_MESSAGE`]) is this
/// service's DESCRIPTION of `DecryptionFailure` rather than that wire message, so it
/// is what the refusal states when there is no KMS answer for the clause to
/// introduce.
const DECRYPTION_RELAY_PREFIX: &str = "Secrets Manager can't decrypt the secret value:";

/// How the `Message` of `EncryptionFailure` introduces the KMS refusal a write met,
/// which is the mirror of [`DECRYPTION_RELAY_PREFIX`] — the same clause for the
/// operation the key could not perform.
const ENCRYPTION_RELAY_PREFIX: &str = "Secrets Manager can't encrypt the secret value:";

/// The `Message` of `DecryptionFailure` when there is no KMS answer to relay:
/// "Secrets Manager can't decrypt the protected secret text using the provided KMS
/// key." (this service's Smithy model).
pub const DECRYPTION_FAILURE_MESSAGE: &str =
    "Secrets Manager can't decrypt the protected secret text using the provided KMS key.";

/// The `Message` of `EncryptionFailure` when there is no KMS answer to relay:
/// "Secrets Manager can't encrypt the protected secret text using the provided KMS
/// key. Check that the KMS key is available, enabled, and not in an invalid state."
/// (this service's Smithy model).
pub const ENCRYPTION_FAILURE_MESSAGE: &str =
    "Secrets Manager can't encrypt the protected secret text using the provided KMS \
     key. Check that the KMS key is available, enabled, and not in an invalid state.";

/// The service name the relayed KMS refusal names itself by, which is KMS's own in
/// the wrapped failure Secrets Manager relays.
const KMS_SERVICE_NAME: &str = "AWSKMS";

/// The status a relayed KMS refusal reports for a request KMS rejected, and the one
/// for a fault on its side: KMS's JSON protocol answers a client-side refusal of a
/// cryptographic operation with 400 and a fault of its own with 500, whichever
/// exception shape carries it, so the status the relay states is the one for the
/// refusal's CLASS rather than the status line of one answer.
const KMS_CLIENT_STATUS: u16 = 400;
const KMS_FAULT_STATUS: u16 = 500;

/// The entry of the encryption context naming the secret the value belongs to.
const CONTEXT_SECRET_ARN: &str = "SecretARN";

/// The entry of the encryption context naming the version the value belongs to.
const CONTEXT_SECRET_VERSION_ID: &str = "SecretVersionId";

/// Which member of the request a version's value arrived in, and therefore which
/// member of the response reports it: "The decrypted secret value, if the secret
/// value was originally provided as binary data … this field is omitted. The secret
/// value appears in SecretString instead."
/// (`GetSecretValueResponse$SecretBinary`, this service's Smithy model.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// `SecretString`.
    Text,
    /// `SecretBinary`.
    Binary,
}

/// A secret value in the clear: the bytes, and the member they travel in.
///
/// Produced from a request on the way in and from an opened envelope on the way
/// out; never stored, which is what [`StoredValue`] is for.
#[derive(Clone)]
pub struct SecretValue {
    kind: ValueKind,
    bytes: Vec<u8>,
}

/// The value is `@sensitive`, so it is withheld from the formatter: a derived
/// `Debug` would carry the plaintext into every log and trace that formats a
/// request or a stored version.
impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretValue")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl SecretValue {
    /// The value a request carried, or `None` for one that carried neither member —
    /// "You must include SecretBinary or SecretString, but not both", which the
    /// operations refuse before reaching here.
    pub fn from_request(
        secret_string: Option<&String>,
        secret_binary: Option<&Vec<u8>>,
    ) -> Option<Self> {
        match (secret_string, secret_binary) {
            (Some(text), _) => Some(Self {
                kind: ValueKind::Text,
                bytes: text.as_bytes().to_vec(),
            }),
            (None, Some(blob)) => Some(Self {
                kind: ValueKind::Binary,
                bytes: blob.clone(),
            }),
            (None, None) => None,
        }
    }

    /// The value for the response, in the one member it travels in.
    ///
    /// A text value was handed to this service as a `String`, so its bytes are
    /// UTF-8; the AEAD's authentication tag is what guarantees the bytes read back
    /// are the bytes sealed.
    pub fn into_members(self) -> (Option<String>, Option<Vec<u8>>) {
        match self.kind {
            ValueKind::Text => (
                Some(String::from_utf8_lossy(&self.bytes).into_owned()),
                None,
            ),
            ValueKind::Binary => (None, Some(self.bytes)),
        }
    }
}

/// A secret version's value as the store holds it.
#[derive(Clone)]
pub struct StoredValue {
    kind: ValueKind,
    content: Content,
    /// The digest of the value, which is what an `@idempotencyToken` replay is
    /// judged on: a sealed value cannot be compared as bytes, because each seal
    /// draws its own data key and nonce.
    fingerprint: Vec<u8>,
}

/// The value is withheld from the formatter for the reason [`SecretValue`]'s own
/// `Debug` withholds it; what a log may say about a stored value is its member and
/// whether a customer managed key sealed it.
impl fmt::Debug for StoredValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredValue")
            .field("kind", &self.kind)
            .field("sealed", &self.is_sealed())
            .finish_non_exhaustive()
    }
}

/// How the bytes of a stored value are held.
#[derive(Clone)]
enum Content {
    /// Held as sent: the secret is on the Amazon Web Services managed key, which no
    /// caller can act on, so no KMS refusal stands between a read and the value.
    AsSent(Vec<u8>),
    /// Sealed under a data key the secret's customer managed key wrapped.
    Sealed(Sealed),
}

/// A value sealed under a KMS data key: the wrapped key, the nonce and the
/// ciphertext with its authentication tag.
#[derive(Clone)]
struct Sealed {
    /// The `CiphertextBlob` KMS wrapped the data key as. Only the key that wrapped
    /// it can unwrap it, and the blob names that key, so a version is opened under
    /// the key it was sealed with whatever key the secret names now — which is what
    /// keeps a version the secret's key change did not re-encrypt readable (see
    /// [`crate::ops::update_secret`]).
    wrapped_data_key: Vec<u8>,
    nonce: [u8; NONCE_LEN],
    body: Vec<u8>,
}

impl StoredValue {
    /// The value held as it arrived, for a secret on the service-managed key.
    pub fn as_sent(value: &SecretValue) -> Self {
        Self {
            kind: value.kind,
            fingerprint: fingerprint(value.kind, &value.bytes),
            content: Content::AsSent(value.bytes.clone()),
        }
    }

    /// Whether a customer managed key sealed this value.
    pub fn is_sealed(&self) -> bool {
        matches!(self.content, Content::Sealed(_))
    }

    /// Whether this holds exactly `value` — the test an `@idempotencyToken` replay
    /// turns on: "If you call this operation with a ClientRequestToken that matches
    /// an existing version's VersionId, and you specify the same secret data, the
    /// operation succeeds but does nothing." (`PutSecretValue`, this service's
    /// Smithy model.)
    pub fn holds(&self, value: &SecretValue) -> bool {
        self.kind == value.kind && self.fingerprint == fingerprint(value.kind, &value.bytes)
    }
}

/// The encryption context the KMS calls for one secret version carry: the secret and
/// the version the value belongs to, which is what binds a sealed value to the place
/// it is stored — a data key wrapped for one version cannot be unwrapped for
/// another.
#[derive(Debug, Clone)]
pub struct ValueContext {
    secret_arn: String,
    version_id: String,
}

impl ValueContext {
    /// The context for the version `version_id` of the secret `secret_arn`.
    pub fn new(secret_arn: &str, version_id: &str) -> Self {
        Self {
            secret_arn: secret_arn.to_string(),
            version_id: version_id.to_string(),
        }
    }

    /// The `EncryptionContext` of the KMS call.
    fn kms(&self) -> HashMap<String, String> {
        HashMap::from([
            (CONTEXT_SECRET_ARN.to_string(), self.secret_arn.clone()),
            (
                CONTEXT_SECRET_VERSION_ID.to_string(),
                self.version_id.clone(),
            ),
        ])
    }

    /// The same two entries as the additional authenticated data of the local AEAD,
    /// so the ciphertext is bound to its version as well as the data key is.
    fn aad(&self) -> Vec<u8> {
        format!(
            "{CONTEXT_SECRET_ARN}={}&{CONTEXT_SECRET_VERSION_ID}={}",
            self.secret_arn, self.version_id
        )
        .into_bytes()
    }
}

/// A KMS call the envelope needed did not produce a result, so the value could not
/// be sealed or opened.
#[derive(Debug, Clone)]
pub struct KmsRefusal {
    /// The KMS refusal to relay, when KMS answered with one; `None` when KMS did not
    /// answer at all, which is nothing to report about the key.
    relayed: Option<String>,
}

impl KmsRefusal {
    /// A failure KMS stated nothing about.
    fn unrelayed() -> Self {
        Self { relayed: None }
    }

    /// The `DecryptionFailure` message for this refusal.
    pub fn decryption_failure_message(&self) -> String {
        self.worded(DECRYPTION_RELAY_PREFIX, DECRYPTION_FAILURE_MESSAGE)
    }

    /// The `EncryptionFailure` message for this refusal.
    pub fn encryption_failure_message(&self) -> String {
        self.worded(ENCRYPTION_RELAY_PREFIX, ENCRYPTION_FAILURE_MESSAGE)
    }

    /// What Secrets Manager could not do, and then the KMS refusal it wraps — so
    /// the caller learns which key state or permission refused the call, and
    /// therefore whether it can change. With no KMS answer to introduce there is
    /// nothing to relay, and the modelled sentence for the failure stands alone.
    fn worded(&self, prefix: &str, unrelayed: &str) -> String {
        match &self.relayed {
            Some(relayed) => format!("{prefix} {relayed}"),
            None => unrelayed.to_string(),
        }
    }
}

/// Seal `value` for the version `context` names, under the key `kms_key_id` names.
///
/// A secret on the service-managed key (`None`, the empty string, or the managed
/// key's own alias) has no customer managed key to call, so its value is held as
/// sent.
pub async fn seal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kms_key_id: Option<&str>,
    context: &ValueContext,
    value: &SecretValue,
) -> Result<StoredValue, KmsRefusal> {
    let Some(key_id) = customer_managed_key(kms_key_id) else {
        return Ok(StoredValue::as_sent(value));
    };

    let generated = match kms_client(ctx, region)
        .generate_data_key()
        .key_id(key_id)
        .key_spec(::aws_sdk_kms::types::DataKeySpec::Aes256)
        .set_encryption_context(Some(context.kms()))
        .send()
        .await
    {
        Ok(generated) => generated,
        // An emulator that holds no KMS cannot seal anything, and refusing every
        // write of a value under a customer managed key would leave this service
        // unusable without one — see this module's header. Every other failure
        // means the value was not sealed, so it is not stored: a refusal KMS
        // stated, and a fault that reached no answer, are both the write failing.
        Err(error) => {
            return match call_failure(&error) {
                CallFailure::NoKms => Ok(StoredValue::as_sent(value)),
                CallFailure::Failed(refusal) => Err(refusal),
            };
        }
    };

    let (Some(data_key), Some(wrapped_data_key)) =
        (generated.plaintext(), generated.ciphertext_blob())
    else {
        return Err(KmsRefusal::unrelayed());
    };
    let Some((nonce, body)) = seal_under(data_key.as_ref(), &context.aad(), &value.bytes) else {
        return Err(KmsRefusal::unrelayed());
    };
    Ok(StoredValue {
        kind: value.kind,
        fingerprint: fingerprint(value.kind, &value.bytes),
        content: Content::Sealed(Sealed {
            wrapped_data_key: wrapped_data_key.as_ref().to_vec(),
            nonce,
            body,
        }),
    })
}

/// Open `stored`, the value of the version `context` names.
///
/// A value held as sent is answered as it is; a sealed one is answered only once KMS
/// unwraps the data key it was sealed under. The unwrap names no key: the wrapped
/// data key carries the identity of the key that wrapped it, which is what keeps a
/// version readable after the secret is pointed at another key.
pub async fn open(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    context: &ValueContext,
    stored: &StoredValue,
) -> Result<SecretValue, KmsRefusal> {
    let sealed = match &stored.content {
        Content::AsSent(bytes) => {
            return Ok(SecretValue {
                kind: stored.kind,
                bytes: bytes.clone(),
            });
        }
        Content::Sealed(sealed) => sealed,
    };

    let opened = match kms_client(ctx, region)
        .decrypt()
        .ciphertext_blob(::aws_smithy_types::Blob::new(
            sealed.wrapped_data_key.clone(),
        ))
        .set_encryption_context(Some(context.kms()))
        .send()
        .await
    {
        Ok(opened) => opened,
        // A sealed value cannot be opened without KMS, whichever way the call
        // failed, so every failure refuses this read.
        Err(error) => {
            return Err(match call_failure(&error) {
                CallFailure::NoKms => KmsRefusal::unrelayed(),
                CallFailure::Failed(refusal) => refusal,
            });
        }
    };

    let Some(data_key) = opened.plaintext() else {
        return Err(KmsRefusal::unrelayed());
    };
    let Some(bytes) = open_under(data_key.as_ref(), &context.aad(), sealed) else {
        return Err(KmsRefusal::unrelayed());
    };
    Ok(SecretValue {
        kind: stored.kind,
        bytes,
    })
}

/// What KMS says about the key `kms_key_id` that stops it being used at all, as the
/// `EncryptionFailure` message a write under that key would be refused with;
/// `None` when the key can be used.
///
/// A secret is encrypted under the key it names, so a key KMS will not even describe
/// cannot hold one — which is what a replica whose Region names such a key reports
/// as the reason its creation failed, and it reports it as what the service could
/// not do with that key: the same message, relayed KMS answer and all, that a
/// write refused under it carries. The judgement is KMS's, as everywhere else in
/// this module: the key's own state and the permissions on it are KMS's to state,
/// and this asks it (`kms:DescribeKey`).
///
/// `None`, for the same two cases the sealing path treats as nothing said about the
/// key: the Amazon Web Services managed key, which names no key a caller can act on,
/// and an emulator assembled without KMS, where the request reached no KMS at all.
pub async fn key_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kms_key_id: Option<&str>,
) -> Option<String> {
    let key_id = customer_managed_key(kms_key_id)?;
    match kms_client(ctx, region)
        .describe_key()
        .key_id(key_id)
        .send()
        .await
    {
        Ok(_) => None,
        Err(error) => match call_failure(&error) {
            CallFailure::NoKms => None,
            // A fault KMS stated nothing about says nothing about the key, so
            // there is nothing to report: the modelled sentence for the failure
            // class stands for a refused write, not for a judgement of the key.
            CallFailure::Failed(refusal) => refusal
                .relayed
                .is_some()
                .then(|| refusal.encryption_failure_message()),
        },
    }
}

/// The customer managed key a secret's `KmsKeyId` names, or `None` for a secret left
/// on the Amazon Web Services managed key: "If you don't specify this value, then
/// Secrets Manager uses the Amazon Web Services managed key `aws/secretsmanager`"
/// (`CreateSecretRequest$KmsKeyId`, this service's Smithy model), which an empty
/// string names too (`UpdateSecretRequest$KmsKeyId`).
pub fn customer_managed_key(kms_key_id: Option<&str>) -> Option<&str> {
    kms_key_id.filter(|key| !key.is_empty() && !super::names_default_kms_key(key))
}

/// Seal `plaintext` under the data key `data_key`, binding `aad` into the
/// ciphertext, and answer the nonce drawn for it with the ciphertext and its tag.
///
/// `None` only when the AEAD itself refuses the inputs — a data key that is not
/// AES-256 material, or a plaintext larger than AES-GCM can carry, which the
/// modelled 65536-byte bound on a secret value already excludes.
fn seal_under(data_key: &[u8], aad: &[u8], plaintext: &[u8]) -> Option<([u8; NONCE_LEN], Vec<u8>)> {
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, data_key).ok()?);
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).ok()?;
    // `seal_in_place_append_tag` leaves `body` holding the ciphertext followed by
    // the authentication tag.
    let mut body = plaintext.to_vec();
    cipher
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad),
            &mut body,
        )
        .ok()?;
    Some((nonce, body))
}

/// Open `sealed` with the data key `data_key` under `aad`.
///
/// `None` when the tag does not verify: the ciphertext was sealed under another data
/// key or for another version, or has been altered.
fn open_under(data_key: &[u8], aad: &[u8], sealed: &Sealed) -> Option<Vec<u8>> {
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, data_key).ok()?);
    let mut body = sealed.body.clone();
    let plaintext = cipher
        .open_in_place(
            Nonce::assume_unique_for_key(sealed.nonce),
            Aad::from(aad),
            &mut body,
        )
        .ok()?;
    Some(plaintext.to_vec())
}

/// The digest an idempotent replay compares a value by: the member it travels in,
/// then its bytes, so the same bytes in `SecretString` and in `SecretBinary` are not
/// one value.
fn fingerprint(kind: ValueKind, bytes: &[u8]) -> Vec<u8> {
    let mut input = Vec::with_capacity(bytes.len() + 1);
    input.push(match kind {
        ValueKind::Text => b'S',
        ValueKind::Binary => b'B',
    });
    input.extend_from_slice(bytes);
    digest(&SHA256, &input).as_ref().to_vec()
}

/// The error types that are the in-process TRANSPORT's own refusal rather than an
/// answer from KMS: this emulator holds no KMS at the endpoint that was called, or
/// more than one of its services claims it. Either way the request reached no KMS,
/// so it says nothing about the key — the same reading every service that calls
/// another in process makes of these two codes.
const TRANSPORT_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// The response headers a request identifier is read from, so the relay reports the
/// identifier of the answer it wraps whenever that answer carries one.
const REQUEST_ID_HEADERS: [&str; 2] = ["x-amzn-requestid", "x-amz-request-id"];

/// How the relayed clause states a field the wrapped answer did not carry.
const ABSENT_FIELD: &str = "null";

/// Why a KMS call did not produce a result.
enum CallFailure {
    /// The request reached no KMS at all: this emulator was assembled without one,
    /// so the TRANSPORT refused it ([`TRANSPORT_REFUSALS`]) and nothing was said
    /// about the key.
    NoKms,
    /// The call did not do its work: a refusal KMS stated, which is relayed, or a
    /// fault that reached no answer, which has nothing to relay.
    Failed(KmsRefusal),
}

/// Which of the two [`CallFailure`]s an SDK error is.
///
/// A refusal KMS stated is relayed as it was answered: the message KMS stated, the
/// service and error code it stated it under, the status its class is answered with,
/// and the identifier of that answer — a field the answer did not carry is stated as
/// absent rather than invented.
fn call_failure<E: ProvideErrorMetadata + ::std::error::Error + 'static>(
    error: &SdkError<E, HttpResponse>,
) -> CallFailure {
    let SdkError::ServiceError(context) = error else {
        ::tracing::debug!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
            "KMS did not answer the envelope call for this secret version"
        );
        return CallFailure::Failed(KmsRefusal::unrelayed());
    };
    let refused = context.err();
    let code = refused.code();
    if code.is_some_and(|code| TRANSPORT_REFUSALS.contains(&code)) {
        ::tracing::debug!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
            "no KMS answered the envelope call for this secret version"
        );
        return CallFailure::NoKms;
    }
    let answered = context.raw();
    let status = if answered.status().as_u16() >= KMS_FAULT_STATUS {
        KMS_FAULT_STATUS
    } else {
        KMS_CLIENT_STATUS
    };
    let request_id = REQUEST_ID_HEADERS
        .into_iter()
        .find_map(|header| answered.headers().get(header))
        .map(str::to_string)
        // A request identifier belongs to the ANSWER, and the refusal is relayed
        // with the one that answer carried. An answer that carries none is still an
        // answer to one request, so the relay states an identifier of that form for
        // it rather than leaving the field out of a clause that states it.
        .unwrap_or_else(|| ::uuid::Uuid::new_v4().to_string());
    let clause = format!(
        "(Service: {KMS_SERVICE_NAME}; Status Code: {status}; Error Code: {}; \
         Request ID: {request_id}; Proxy: {ABSENT_FIELD})",
        code.unwrap_or(ABSENT_FIELD)
    );
    let wrapped = match refused.message() {
        Some(message) => format!("{message} {clause}"),
        None => clause,
    };
    CallFailure::Failed(KmsRefusal {
        relayed: Some(wrapped),
    })
}

/// A KMS client over the in-process callback into this same emulator.
///
/// `region` is the request's own signing region, so the key is used where the secret
/// that names it lives. The transport is in-process, so there is nothing to retry,
/// time out or watch for a stalled stream — and each of those would need a sleep
/// implementation this client is not given. The same goes for caching the identity:
/// the credentials the callback hands over are used directly.
fn kms_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}
