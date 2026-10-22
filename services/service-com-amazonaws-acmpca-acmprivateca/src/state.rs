// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-acmpca-acmprivateca service.
//!
//! Everything the private-CA lifecycle persists lives here, beside the helpers the
//! operation handlers share: ARN parsing, the clock-owed status transitions, validity
//! resolution and the PEM bodies the service hands back. Handlers hold no state of
//! their own — they reach this store through [`service_state`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    ASN1Subject, CertificateAuthority, CertificateAuthorityConfiguration, RevocationConfiguration,
    Tag, Validity,
};

/// The ARN namespace / sigv4 signing name of this service (`awsService.arnNamespace`).
pub const ARN_SERVICE: &str = "acm-pca";

/// The region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

// `CertificateAuthorityStatus` members this slice uses (see
// `CERTIFICATE_AUTHORITY_STATUS_VALUES` in `ops::types` for the whole enum).
/// Private CA is being created; its CSR is not available yet.
pub const STATUS_CREATING: &str = "CREATING";
/// Private CA exists and waits for its signed CA certificate to be imported.
pub const STATUS_PENDING_CERTIFICATE: &str = "PENDING_CERTIFICATE";
/// Private CA holds its certificate and may issue end-entity certificates.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// Default `UsageMode` — "The default value is GENERAL_PURPOSE."
/// (`CreateCertificateAuthorityRequest$UsageMode` docs).
pub const DEFAULT_USAGE_MODE: &str = "GENERAL_PURPOSE";

/// Default `KeyStorageSecurityStandard` — "Default: FIPS_140_2_LEVEL_3_OR_HIGHER"
/// (`CreateCertificateAuthorityRequest$KeyStorageSecurityStandard` docs).
pub const DEFAULT_KEY_STORAGE_SECURITY_STANDARD: &str = "FIPS_140_2_LEVEL_3_OR_HIGHER";

/// `SHORT_LIVED_CERTIFICATE` usage mode: "Short-lived certificate validity is limited to
/// seven days." (`CreateCertificateAuthorityRequest$UsageMode` docs).
pub const SHORT_LIVED_MAX_VALIDITY: Duration = Duration::from_secs(7 * 86_400);

/// How long a `CreateCertificateAuthority` / `IssueCertificate` idempotency token is
/// honoured: "Idempotency tokens for CreateCertificateAuthority time out after five
/// minutes." (`CreateCertificateAuthorityRequest$IdempotencyToken` docs; the
/// `IssueCertificateRequest$IdempotencyToken` docs say the same for IssueCertificate).
pub const IDEMPOTENCY_WINDOW: Duration = Duration::from_secs(5 * 60);

/// How long the CA creation the clock owes takes to settle (`CREATING` ->
/// `PENDING_CERTIFICATE`), and likewise an issued certificate becoming retrievable.
///
/// A span this emulator CHOOSES, not one AWS documents: it only has to be non-zero, so a
/// caller that acts immediately after the starting call still observes the transient
/// status and the state errors that go with it (`RequestInProgressException`).
pub const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// Validity window recorded for a CA certificate the caller imports, in calendar months.
///
/// CHOSEN, not documented: Amazon Web Services Private CA copies `NotBefore`/`NotAfter`
/// out of the imported certificate's ASN.1, which this emulator does not decode, so the
/// window is recorded as the import instant plus this span.
pub const IMPORTED_CA_CERTIFICATE_VALIDITY_MONTHS: i64 = 120;

/// Most private CAs this emulator holds at once.
///
/// A BOUND, in the shape of the per-region quota Amazon Web Services Private CA
/// publishes for private CAs: every CA is held in memory for the life of the process, so
/// without a ceiling an unauthenticated caller could grow the store without limit. A
/// create past it answers the modelled `LimitExceededException`, which names the quota.
pub const MAX_CERTIFICATE_AUTHORITIES: usize = 200;

/// Most certificates one CA holds at once, bounded for the same reason.
pub const MAX_CERTIFICATES_PER_CA: usize = 1_000;

/// Most live idempotency tokens one map holds. Expired entries are swept on every
/// record, and the oldest live entry is evicted at the ceiling, so a caller sending a
/// fresh token per request cannot grow the map without limit.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 128;

/// One issued end-entity certificate, held under the CA that issued it.
#[derive(Debug, Clone)]
pub struct IssuedCertificate {
    /// `<ca arn>/certificate/<serial>`.
    pub arn: String,
    /// Hex serial allocated by the issuing CA.
    pub serial: String,
    /// The certificate body `GetCertificate` returns.
    pub certificate_pem: String,
    /// The CA certificate (plus the CA's own chain) this certificate chains up through.
    pub certificate_chain_pem: Option<String>,
    /// The `SigningAlgorithm` the request asked the CA to sign with.
    pub signing_algorithm: String,
    /// The `TemplateArn` the request named, when it named one.
    pub template_arn: Option<String>,
    /// Start of the certificate's validity window (`ValidityNotBefore`, else issuance).
    pub not_before: SystemTime,
    /// End of the certificate's validity window (`Validity`).
    pub not_after: SystemTime,
    /// When `IssueCertificate` accepted the request.
    pub issued_at: SystemTime,
    /// When issuance is due to complete; `GetCertificate` answers
    /// `RequestInProgressException` before it.
    pub available_at: SystemTime,
}

impl IssuedCertificate {
    /// Whether issuance has completed as of `now`.
    pub fn is_available(&self, now: SystemTime) -> bool {
        now >= self.available_at
    }
}

/// One private certificate authority.
#[derive(Debug, Clone)]
pub struct CertificateAuthorityRecord {
    /// `arn:aws:acm-pca:<region>:<account>:certificate-authority/<id>`.
    pub arn: String,
    /// Account that owns the CA.
    pub owner_account: String,
    /// `CertificateAuthorityType`: ROOT or SUBORDINATE.
    pub ca_type: String,
    /// `CertificateAuthorityStatus`.
    pub status: String,
    /// Serial number of the private CA.
    pub serial: String,
    /// When `CreateCertificateAuthority` accepted the request.
    pub created_at: SystemTime,
    /// When the CA last changed status.
    pub last_state_change_at: SystemTime,
    /// When `CREATING` is due to settle into `PENDING_CERTIFICATE`.
    pub settles_at: SystemTime,
    /// Start of the CA certificate's validity window; set on import.
    pub not_before: Option<SystemTime>,
    /// End of the CA certificate's validity window; set on import.
    pub not_after: Option<SystemTime>,
    /// Why creating the CA failed, when it did.
    pub failure_reason: Option<String>,
    /// The configuration the create request supplied.
    pub configuration: CertificateAuthorityConfiguration,
    /// The revocation configuration the create request supplied.
    pub revocation_configuration: Option<RevocationConfiguration>,
    /// `CertificateAuthorityUsageMode`.
    pub usage_mode: String,
    /// `KeyStorageSecurityStandard`.
    pub key_storage_security_standard: String,
    /// The CSR this CA hands out, derived once at create time and never re-derived.
    pub csr_pem: String,
    /// The CA certificate imported through `ImportCertificateAuthorityCertificate`.
    pub ca_certificate_pem: Option<String>,
    /// The chain imported alongside that certificate.
    pub ca_certificate_chain_pem: Option<String>,
    /// How long a deleted CA can still be restored.
    pub restorable_until: Option<SystemTime>,
    /// Tags attached to the CA.
    pub tags: Vec<Tag>,
    /// Counter the next issued serial is minted from.
    pub next_serial: u64,
    /// Certificates issued by this CA, keyed by certificate ARN.
    pub certificates: HashMap<String, IssuedCertificate>,
    /// `IssueCertificate` idempotency tokens -> the certificate ARN they minted.
    pub issue_idempotency: HashMap<String, IdempotencyEntry>,
}

impl CertificateAuthorityRecord {
    /// Materialise the status change the clock already owed at `now`.
    ///
    /// A reporting operation may call this: the transition was due at `settles_at`
    /// whether or not anybody called, so applying it changes nothing a later caller
    /// could have seen differently.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == STATUS_CREATING && now >= self.settles_at {
            self.status = STATUS_PENDING_CERTIFICATE.to_string();
            self.last_state_change_at = self.settles_at;
        }
    }

    /// The modelled `CertificateAuthority` structure, read straight off this record.
    pub fn to_wire(&self) -> CertificateAuthority {
        CertificateAuthority {
            arn: Some(self.arn.clone()),
            owner_account: Some(self.owner_account.clone()),
            created_at: Some(to_date_time(self.created_at)),
            last_state_change_at: Some(to_date_time(self.last_state_change_at)),
            r#type: Some(self.ca_type.clone()),
            serial: Some(self.serial.clone()),
            status: Some(self.status.clone()),
            not_before: self.not_before.map(to_date_time),
            not_after: self.not_after.map(to_date_time),
            failure_reason: self.failure_reason.clone(),
            certificate_authority_configuration: Some(self.configuration.clone()),
            revocation_configuration: self.revocation_configuration.clone(),
            restorable_until: self.restorable_until.map(to_date_time),
            key_storage_security_standard: Some(self.key_storage_security_standard.clone()),
            usage_mode: Some(self.usage_mode.clone()),
        }
    }

    /// The chain an end-entity certificate issued by this CA presents: the CA's own
    /// certificate followed by whatever chain was imported with it.
    pub fn issued_chain(&self) -> Option<String> {
        let ca_certificate = self.ca_certificate_pem.as_ref()?;
        Some(match self.ca_certificate_chain_pem.as_ref() {
            Some(chain) => format!("{ca_certificate}{chain}"),
            None => ca_certificate.clone(),
        })
    }
}

/// An idempotency token's record: what it minted and when it stops being honoured.
#[derive(Debug, Clone)]
pub struct IdempotencyEntry {
    /// The ARN the first call with this token returned.
    pub arn: String,
    /// End of the five-minute window the token is honoured for.
    pub expires_at: SystemTime,
    /// Insertion order, so eviction at the ceiling is the oldest entry rather than an
    /// arbitrary one of several recorded in the same clock tick.
    pub sequence: u64,
}

impl IdempotencyEntry {
    /// A token recorded at `now`, honoured for [`IDEMPOTENCY_WINDOW`].
    pub fn new(arn: &str, now: SystemTime) -> Self {
        Self {
            arn: arn.to_string(),
            expires_at: now + IDEMPOTENCY_WINDOW,
            sequence: next_sequence(),
        }
    }
}

/// Look `token` up in an idempotency map, ignoring (and pruning) an expired entry.
pub fn replay_idempotent(
    tokens: &mut HashMap<String, IdempotencyEntry>,
    token: Option<&String>,
    now: SystemTime,
) -> Option<String> {
    let token = token?;
    match tokens.get(token) {
        Some(entry) if now < entry.expires_at => Some(entry.arn.clone()),
        Some(_) => {
            tokens.remove(token);
            None
        }
        None => None,
    }
}

/// Remember that `token` minted `arn`, keeping the map bounded.
///
/// The five-minute window is enforced as a real eviction policy, not only when the same
/// token comes back: every record sweeps the entries that have expired, and at
/// [`MAX_IDEMPOTENCY_TOKENS`] live entries the oldest INSERTED entry (the lowest
/// `sequence`) makes room. A caller sending a fresh token per request therefore cannot
/// grow this map without limit, and an absent token is simply not remembered.
pub fn record_idempotent(
    tokens: &mut HashMap<String, IdempotencyEntry>,
    token: Option<String>,
    arn: &str,
    now: SystemTime,
) {
    let Some(token) = token else {
        return;
    };
    tokens.retain(|_, entry| now < entry.expires_at);
    while tokens.len() >= MAX_IDEMPOTENCY_TOKENS {
        let Some(oldest) = tokens
            .iter()
            .min_by_key(|(_, entry)| entry.sequence)
            .map(|(token, _)| token.clone())
        else {
            break;
        };
        tokens.remove(&oldest);
    }
    tokens.insert(token, IdempotencyEntry::new(arn, now));
}

/// Per-service state for com-amazonaws-acmpca-acmprivateca. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAcmpcaAcmprivatecaState {
    /// Every private CA, keyed by its ARN.
    pub certificate_authorities: HashMap<String, CertificateAuthorityRecord>,
    /// `CreateCertificateAuthority` idempotency tokens -> the CA ARN they minted.
    pub create_idempotency: HashMap<String, IdempotencyEntry>,
}

impl ComAmazonawsAcmpcaAcmprivatecaState {
    /// The CA named by `arn`, with any status change the clock owed already applied.
    pub fn certificate_authority_mut(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&mut CertificateAuthorityRecord> {
        let record = self.certificate_authorities.get_mut(arn)?;
        record.settle(now);
        Some(record)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAcmpcaAcmprivatecaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAcmpcaAcmprivatecaState>)
}

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

/// The CA ARN a request named, when it is well formed for THIS service.
///
/// A segment of an AWS ARN never contains `:`, so splitting on it is exact:
/// `arn:<partition>:acm-pca:<region>:<account>:certificate-authority/<id>`.
pub fn parse_certificate_authority_arn(arn: &str) -> Option<CertificateAuthorityArn> {
    let fields: Vec<&str> = arn.split(':').collect();
    let [prefix, partition, service, region, account, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" || partition.is_empty() || *service != ARN_SERVICE {
        return None;
    }
    // `AccountId` is twelve digits (len 12..12, pattern ^[0-9]+$), so an ARN with an
    // empty or non-numeric account segment names no resource of this service.
    if account.len() != 12 || !account.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let segments: Vec<&str> = resource.split('/').collect();
    let ["certificate-authority", id] = segments.as_slice() else {
        return None;
    };
    if id.is_empty() {
        return None;
    }
    Some(CertificateAuthorityArn {
        region: (*region).to_string(),
        account: (*account).to_string(),
        id: (*id).to_string(),
    })
}

/// The parts of a well-formed certificate-authority ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateAuthorityArn {
    /// Region segment (empty for a partition-global ARN, which the pattern allows).
    pub region: String,
    /// Account segment.
    pub account: String,
    /// The CA's id, the last resource segment.
    pub id: String,
}

/// Split a certificate ARN into the ARN of the CA that issued it and its serial.
///
/// `IssueCertificate` returns `<ca arn>/certificate/<serial>`, so the CA ARN is a
/// prefix of the certificate ARN — a certificate ARN minted by one CA therefore never
/// resolves against another.
pub fn parse_certificate_arn(arn: &str) -> Option<(String, String)> {
    let (ca_arn, serial) = arn.rsplit_once("/certificate/")?;
    if serial.is_empty() || serial.contains('/') {
        return None;
    }
    parse_certificate_authority_arn(ca_arn)?;
    Some((ca_arn.to_string(), serial.to_string()))
}

/// The region this request was signed for, which is the region of every ARN it mints.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// Minted identifiers
// ---------------------------------------------------------------------------

/// Distinguishes two mints that land in the same nanosecond, and orders idempotency
/// records for eviction.
static MINT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The next value of the process-wide sequence.
fn next_sequence() -> u64 {
    MINT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

/// A fresh RFC 4122 version-4-shaped id, the form Private CA gives a CA resource
/// (`...:certificate-authority/12345678-1234-1234-1234-123456789012`).
pub fn mint_resource_id() -> String {
    let digest = mint_digest("certificate-authority");
    let hex = ::hex::encode(&digest[..16]);
    // Version nibble 4 and a variant nibble from `8..=b`, so the id is shaped like the
    // random uuid AWS mints rather than merely being 32 hex digits.
    let variant = match digest[8] & 0x03 {
        0 => '8',
        1 => '9',
        2 => 'a',
        _ => 'b',
    };
    format!(
        "{}-{}-4{}-{variant}{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

/// A serial number for a CA or for one of its certificates, minted from `seed`.
///
/// Private CA reports serials as hex, so this is 16 bytes of hex digits.
pub fn mint_serial(seed: &str) -> String {
    ::hex::encode(&mint_digest(seed)[..16])
}

/// A digest over `seed`, the current instant and a process-wide sequence, so two mints
/// with the same seed never collide.
fn mint_digest(seed: &str) -> [u8; 32] {
    let sequence = next_sequence();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let digest = Sha256::digest(format!("{seed}|{sequence}|{nanos}").as_bytes());
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    bytes
}

// ---------------------------------------------------------------------------
// PEM bodies
// ---------------------------------------------------------------------------

/// PEM label of a PKCS#10 certification request (RFC 7468 section 7).
pub const PEM_LABEL_CSR: &str = "CERTIFICATE REQUEST";
/// PEM label of an X.509 certificate (RFC 7468 section 5).
pub const PEM_LABEL_CERTIFICATE: &str = "CERTIFICATE";

/// Render `body` as a PEM block of `label`: base64 (the audited
/// `aws_smithy_types::base64` encoder, the one the wire serializer uses) in lines of 64
/// characters with LF endings, as RFC 7468 section 2 specifies.
pub fn encode_pem(label: &str, body: &[u8]) -> String {
    let encoded = ::aws_smithy_types::base64::encode(body);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in encoded.as_bytes().chunks(64) {
        // base64 is ASCII, so every chunk is valid UTF-8.
        out.push_str(&String::from_utf8_lossy(line));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

/// The single PEM block `label` frames, re-encoded canonically, when `text` is exactly
/// one well-formed non-empty block of that label. `None` for anything else — the caller
/// refuses that as malformed.
pub fn parse_pem_block(text: &str, label: &str) -> Option<String> {
    match parse_pem_blocks_as(text, label)?.as_slice() {
        [one] => Some(encode_pem(label, one)),
        _ => None,
    }
}

/// Every PEM block of `label` in `text`, re-encoded canonically, or `None` if any of it
/// is not a well-formed non-empty block of that label.
pub fn parse_pem_blocks(text: &str, label: &str) -> Option<String> {
    let blocks = parse_pem_blocks_as(text, label)?;
    (!blocks.is_empty()).then(|| {
        blocks
            .iter()
            .map(|body| encode_pem(label, body))
            .collect::<String>()
    })
}

/// The contents of every PEM block in `text`, requiring the text to be nothing but
/// well-formed non-empty blocks of `label`.
///
/// Only the framing is read here — the body is decoded by the audited
/// `aws_smithy_types::base64` decoder, and the walk is bounded by the input, which the
/// model already caps (`CertificateBodyBlob` 32 KiB, `CertificateChainBlob` 2 MiB).
fn parse_pem_blocks_as(text: &str, label: &str) -> Option<Vec<Vec<u8>>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut bodies: Vec<Vec<u8>> = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let (body, remainder) = rest
            .strip_prefix(begin.as_str())?
            .split_once(end.as_str())?;
        // Line endings inside the block are not part of the base64 (RFC 7468), so CRLF
        // and LF framing decode alike.
        let base64: String = body.split_whitespace().collect();
        if base64.is_empty() {
            return None;
        }
        bodies.push(::aws_smithy_types::base64::decode(&base64).ok()?);
        rest = remainder.trim_start();
    }
    Some(bodies)
}

/// The certification request this CA hands out, derived once at create time.
///
/// The emulator holds no HSM, so the body is not a signed PKCS#10 structure: it is the
/// request's own content — the CA's ARN, key and signing algorithms and subject
/// distinguished name — rendered deterministically inside the PEM envelope the modelled
/// `CsrBody` carries, so the same CA always answers with the same CSR.
pub fn derive_csr_pem(arn: &str, configuration: &CertificateAuthorityConfiguration) -> String {
    let subject = distinguished_name(&configuration.subject);
    let descriptor = format!(
        "aws-emulator acm-pca certification request\n\
         certificate-authority: {arn}\n\
         key-algorithm: {}\n\
         signing-algorithm: {}\n\
         subject: {subject}\n",
        configuration.key_algorithm, configuration.signing_algorithm,
    );
    encode_pem(PEM_LABEL_CSR, descriptor.as_bytes())
}

/// The certificate this CA issues for `csr`, derived from what the CA persisted and
/// from the request.
///
/// As with the CSR above there is no HSM to sign with, so the body is the certificate's
/// own content — issuer, serial, validity window, signing algorithm, template and the
/// digest of the certification request it answers — rendered deterministically inside
/// the PEM envelope `CertificateBody` carries.
#[allow(clippy::too_many_arguments)]
pub fn derive_certificate_pem(
    ca: &CertificateAuthorityRecord,
    serial: &str,
    csr: &[u8],
    signing_algorithm: &str,
    template_arn: Option<&String>,
    not_before: SystemTime,
    not_after: SystemTime,
) -> String {
    let descriptor = format!(
        "aws-emulator acm-pca certificate\n\
         serial: {serial}\n\
         issuer: {}\n\
         certificate-authority: {}\n\
         signing-algorithm: {signing_algorithm}\n\
         template: {}\n\
         not-before: {}\n\
         not-after: {}\n\
         csr-sha256: {}\n",
        distinguished_name(&ca.configuration.subject),
        ca.arn,
        template_arn.map_or("-", String::as_str),
        format_timestamp(not_before),
        format_timestamp(not_after),
        ::hex::encode(Sha256::digest(csr)),
    );
    encode_pem(PEM_LABEL_CERTIFICATE, descriptor.as_bytes())
}

/// The subject's X.500 distinguished name, most specific relative distinguished name
/// first, as RFC 4514 renders one.
pub fn distinguished_name(subject: &ASN1Subject) -> String {
    let mut rdns: Vec<String> = Vec::new();
    let mut push = |key: &str, value: &Option<String>| {
        if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
            rdns.push(format!("{key}={value}"));
        }
    };
    push("CN", &subject.common_name);
    push("SERIALNUMBER", &subject.serial_number);
    push("OU", &subject.organizational_unit);
    push("O", &subject.organization);
    push("L", &subject.locality);
    push("ST", &subject.state);
    push("C", &subject.country);
    push("TITLE", &subject.title);
    push("SN", &subject.surname);
    push("GIVENNAME", &subject.given_name);
    push("INITIALS", &subject.initials);
    push("PSEUDONYM", &subject.pseudonym);
    push("GENERATIONQUALIFIER", &subject.generation_qualifier);
    push("DNQ", &subject.distinguished_name_qualifier);
    for attribute in subject.custom_attributes.iter().flatten() {
        rdns.push(format!(
            "{}={}",
            attribute.object_identifier, attribute.value
        ));
    }
    rdns.join(",")
}

/// Whether `subject` carries at least one relative distinguished name.
pub fn subject_is_empty(subject: &ASN1Subject) -> bool {
    distinguished_name(subject).is_empty()
}

/// The key family a `KeyAlgorithm` or a `SigningAlgorithm` member belongs to, or `None`
/// for a value neither enum declares.
///
/// Derived from the two value lists in `ops::types` (`KEY_ALGORITHM_VALUES` and
/// `SIGNING_ALGORITHM_VALUES`), so a member added to the model keeps its family: every
/// RSA key is `RSA_<bits>` and every RSA signature names `WITHRSA`, every EC key is
/// `EC_<curve>` and every ECDSA signature names `WITHECDSA`, the lattice members share
/// the `ML_DSA_` prefix, and SM2 keys pair with the single SM3WITHSM2 signature.
pub fn algorithm_family(algorithm: &str) -> Option<&'static str> {
    if algorithm.starts_with("ML_DSA_") {
        Some("ML_DSA")
    } else if algorithm.starts_with("RSA_") || algorithm.contains("WITHRSA") {
        Some("RSA")
    } else if algorithm.starts_with("EC_") || algorithm.contains("WITHECDSA") {
        Some("EC")
    } else if algorithm == "SM2" || algorithm == "SM3WITHSM2" {
        Some("SM2")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// `SystemTime` as the modelled `TStamp`.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(
        time.duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|since| i64::try_from(since.as_millis()).ok())
            .unwrap_or(0),
    )
}

/// Whole seconds between the Unix epoch and `time`.
pub fn epoch_seconds(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// The instant `seconds` after the Unix epoch, or `None` when the platform's clock
/// cannot represent it. Fallible because the value can come straight off the wire.
pub fn from_epoch_seconds(seconds: i64) -> Option<SystemTime> {
    let offset = Duration::from_secs(seconds.unsigned_abs());
    if seconds >= 0 {
        UNIX_EPOCH.checked_add(offset)
    } else {
        UNIX_EPOCH.checked_sub(offset)
    }
}

/// `time` rendered as RFC 3339 by `aws-smithy-types` — the same `DateTime` the wire
/// serializer uses, so there is no second date implementation here. `None` when the
/// instant is not a date that type can express.
fn rfc3339(seconds: i64) -> Option<String> {
    ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// The instant an RFC 3339 `YYYY-MM-DDTHH:MM:SSZ` string names, parsed by the same
/// `aws-smithy-types` date type: month lengths, leap years, the hour/minute/second
/// ranges and the range of representable dates are its rules, not restated here.
fn instant_from_rfc3339(text: &str) -> Option<SystemTime> {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    from_epoch_seconds(parsed.secs())
}

/// The instant `seconds` after the Unix epoch, when it is also a date this service can
/// report. Rendering it IS the range check: the date type refuses a timestamp it cannot
/// express, which is what a caller-chosen `Validity.Value` of absurd magnitude becomes.
fn calendar_instant(seconds: i64) -> Option<SystemTime> {
    rfc3339(seconds)?;
    from_epoch_seconds(seconds)
}

/// `time` as RFC 3339 at second granularity.
pub fn format_timestamp(time: SystemTime) -> String {
    rfc3339(epoch_seconds(time)).unwrap_or_default()
}

/// `start` advanced by `months` calendar months, keeping the time of day and clamping the
/// day of month to the length of the target month (2026-01-31 + 1 month = 2026-02-28).
///
/// Only the month count is arithmetic here; every calendar rule (which days a month has,
/// leap years, the representable range) comes from the `aws-smithy-types` date type,
/// which refuses a date that does not exist. `None` when the result is not such a date —
/// what a caller-chosen `Validity.Value` of absurd magnitude resolves to.
pub fn add_months(start: SystemTime, months: i64) -> Option<SystemTime> {
    let rendered = rfc3339(epoch_seconds(start))?;
    let (date, clock) = rendered.split_once('T')?;
    let mut fields = date.split('-');
    let year: i64 = fields.next()?.parse().ok()?;
    let month: i64 = fields.next()?.parse().ok()?;
    let day: i64 = fields.next()?.parse().ok()?;
    let total = year
        .checked_mul(12)?
        .checked_add(month - 1)?
        .checked_add(months)?;
    let target_year = total.div_euclid(12);
    let target_month = total.rem_euclid(12) + 1;
    // RFC 3339 carries a four-digit year, so anything wider is not a date this service
    // can report.
    if !(1..=9999).contains(&target_year) {
        return None;
    }
    // A day the target month does not have fails to parse, so stepping back from the
    // starting day of month clamps to that month's last day.
    let mut target_day = day;
    loop {
        let candidate = format!("{target_year:04}-{target_month:02}-{target_day:02}T{clock}");
        if let Some(instant) = instant_from_rfc3339(&candidate) {
            return Some(instant);
        }
        // 28 is the shortest month, so a day above it is the only clampable case.
        if target_day > 28 {
            target_day -= 1;
        } else {
            return None;
        }
    }
}

// ---------------------------------------------------------------------------
// Validity
// ---------------------------------------------------------------------------

/// Resolve a modelled `Validity` against `reference` (the start of the validity period).
///
/// The five `ValidityPeriodType` members, exactly as the `Validity$Type` docs define
/// them: `DAYS`/`MONTHS`/`YEARS` are relative to issuance, `ABSOLUTE` is seconds since
/// the Unix epoch, and `END_DATE` is `YYMMDDHHMMSS` (UTCTime — a year field of 50 or
/// more means 19YY, below 50 means 20YY) or `YYYYMMDDHHMMSS` (GeneralizedTime).
///
/// Total for every `i64` the wire can carry: a value that resolves outside the
/// representable range of dates is refused rather than wrapped or panicked on.
///
/// `Err` carries the message of the `InvalidArgsException` the caller must answer with.
pub fn resolve_validity(reference: SystemTime, validity: &Validity) -> Result<SystemTime, String> {
    let value = validity.value;
    let period = validity.r#type.as_str();
    let out_of_range = || {
        format!(
            "Validity.Value {value} with Validity.Type {period} resolves outside the range of \
             dates this service represents"
        )
    };
    match period {
        "DAYS" => value
            .checked_mul(86_400)
            .and_then(|seconds| epoch_seconds(reference).checked_add(seconds))
            .and_then(calendar_instant)
            .ok_or_else(out_of_range),
        "MONTHS" => add_months(reference, value).ok_or_else(out_of_range),
        "YEARS" => value
            .checked_mul(12)
            .and_then(|months| add_months(reference, months))
            .ok_or_else(out_of_range),
        "ABSOLUTE" => calendar_instant(value).ok_or_else(out_of_range),
        "END_DATE" => parse_end_date(value),
        other => Err(format!(
            "Validity.Type '{other}' is not one of ABSOLUTE, DAYS, END_DATE, MONTHS, YEARS"
        )),
    }
}

/// An `END_DATE` validity value: UTCTime `YYMMDDHHMMSS` or GeneralizedTime
/// `YYYYMMDDHHMMSS`. The digits are rearranged into RFC 3339 and handed to the
/// `aws-smithy-types` date parser, so the calendar date and the time of day are
/// validated by that date type and no month-length or leap-year rule is restated here.
fn parse_end_date(value: i64) -> Result<SystemTime, String> {
    let malformed = || {
        format!(
            "Validity.Value {value} is not a UTCTime YYMMDDHHMMSS or GeneralizedTime \
             YYYYMMDDHHMMSS date and time, which Validity.Type END_DATE requires"
        )
    };
    if value <= 0 {
        return Err(malformed());
    }
    let digits = value.to_string();
    let (year, rest) = match digits.len() {
        12 => {
            let two = digits[0..2].parse::<i64>().map_err(|_| malformed())?;
            // UTCTime: "if the year field (YY) is greater than or equal to 50, the year
            // is interpreted as 19YY. If the year field is less than 50, the year is
            // interpreted as 20YY." (`Validity$Type` docs)
            (
                if two >= 50 { 1900 + two } else { 2000 + two },
                &digits[2..],
            )
        }
        14 => (
            digits[0..4].parse::<i64>().map_err(|_| malformed())?,
            &digits[4..],
        ),
        _ => return Err(malformed()),
    };
    // `rest` is exactly the ten digits MMDDHHMMSS, so every slice below is in bounds.
    let candidate = format!(
        "{year:04}-{}-{}T{}:{}:{}Z",
        &rest[0..2],
        &rest[2..4],
        &rest[4..6],
        &rest[6..8],
        &rest[8..10]
    );
    instant_from_rfc3339(&candidate).ok_or_else(malformed)
}
