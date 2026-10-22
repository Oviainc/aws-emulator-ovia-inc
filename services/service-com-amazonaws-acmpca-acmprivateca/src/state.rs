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
use x509_cert::attr::AttributeTypeAndValue;
use x509_cert::der::asn1::{Any, BitString, SetOfVec};
use x509_cert::der::{Decode, Encode, Tag as DerTag};
use x509_cert::name::{Name, RdnSequence, RelativeDistinguishedName};
use x509_cert::request::{CertReq, CertReqInfo};
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{AlgorithmIdentifierOwned, ObjectIdentifier, SubjectPublicKeyInfoOwned};
use x509_cert::time::{Time, Validity as CertificateValidity};
use x509_cert::{Certificate, TbsCertificate, Version};

use crate::ops::types::{
    ASN1Subject, CertificateAuthority, CertificateAuthorityConfiguration, Permission,
    RevocationConfiguration, Tag, Validity,
};

/// A PKCS#10 certification request, as this service reads and writes one.
pub type CertificationRequest = CertReq;

/// The content of a certification request: the subject and the public key a certificate
/// issued for it carries.
pub type CertificationRequestInfo = CertReqInfo;

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

// `ResourceOwner` members a listing filters on (see `RESOURCE_OWNER_VALUES` in
// `ops::types` for the whole enum).
/// The CAs the calling account owns — "The default is SELF."
/// (`ListCertificateAuthoritiesRequest$ResourceOwner` docs).
pub const RESOURCE_OWNER_SELF: &str = "SELF";
/// The CAs another account owns and shared with the calling account.
pub const RESOURCE_OWNER_OTHER_ACCOUNTS: &str = "OTHER_ACCOUNTS";

/// What Amazon Web Services Private CA answers while a CA's creation is still in
/// progress — one sentence, the same for every operation that refuses on `CREATING`
/// rather than one built from the request.
pub const CREATE_IN_PROGRESS_MESSAGE: &str =
    "The request to create certificate authority is still in progress. Try again later.";

/// What Amazon Web Services Private CA answers when a request carries a tag key in the
/// reserved `aws:` namespace: the service's own sentence for the refusal, not one built
/// from the request.
pub const RESERVED_TAG_MESSAGE: &str = "Tags starting with 'aws' are categorized as AWS \
     internal tags and cannot be used for tagging in ACM Private CA service.";

/// What Amazon Web Services Private CA answers for an Amazon Resource Name that is not
/// one of its own: one sentence about the ARN, the same for every operation that reads
/// one.
pub const INVALID_ARN_MESSAGE: &str = "Not a valid arn.";

/// What Amazon Web Services Private CA answers for a certificate body it cannot read:
/// one sentence about the certificate, the same for the certificate and for the chain.
pub const MALFORMED_CERTIFICATE_MESSAGE: &str = "Malformed certificate.";

/// What Amazon Web Services Private CA answers when a CA is asked to issue a certificate
/// in a state that does not allow issuance: one sentence about the CA, naming no request
/// of its own.
pub const ISSUANCE_INVALID_STATE_MESSAGE: &str =
    "The certificate authority is not in a valid state for issuing certificates";

/// What Amazon Web Services Private CA answers for a certificate ARN no certificate of
/// the CA matches: one sentence about the certificate, naming no ARN of its own.
pub const CERTIFICATE_NOT_FOUND_MESSAGE: &str = "Could not find specified certificate.";

/// The sentence this service answers a permission-not-found refusal with: one sentence
/// about the permission, in the form the service's other not-found refusals take
/// ([`CERTIFICATE_NOT_FOUND_MESSAGE`]) and naming no principal of its own.
///
/// An EXAMPLE, not a requirement: the `ResourceNotFoundException` code and its HTTP 400
/// are what the refusal has to be, and no rule for this sentence's wording is documented
/// by AWS, stated by the model or derivable from the request.
pub const PERMISSION_NOT_FOUND_MESSAGE: &str = "Could not find specified permission.";

/// What Amazon Web Services Private CA answers when the resource-based policy a request
/// asks for is not there.
///
/// `GetPolicy`'s docs make the two absences one refusal — "If either the private CA
/// resource or the policy cannot be found, this action returns a
/// ResourceNotFoundException" — so the sentence is about the POLICY, the thing the
/// operation was asked for, and names no resource of the request's own.
pub const POLICY_NOT_FOUND_MESSAGE: &str = "Could not find policy for specified resource.";

/// `SigningAlgorithm`'s members in the order the model DECLARES them, which is the order
/// Amazon Web Services Private CA lists the usable algorithms in when it refuses a
/// key/signing-algorithm pairing.
///
/// `ops::types::SIGNING_ALGORITHM_VALUES` is the same set sorted alphabetically, so it
/// cannot be read for this; the order below is the enum's, from
/// `smithy_query(query="shape",shape="SigningAlgorithm")`. A test pins that the two hold
/// the same members, so a member added to the model cannot go unnoticed here.
pub const SIGNING_ALGORITHMS_IN_MODEL_ORDER: &[&str] = &[
    "SHA256WITHECDSA",
    "SHA384WITHECDSA",
    "SHA512WITHECDSA",
    "SHA256WITHRSA",
    "SHA384WITHRSA",
    "SHA512WITHRSA",
    "SHA256WITHRSA_PSS",
    "SHA384WITHRSA_PSS",
    "SHA512WITHRSA_PSS",
    "SM3WITHSM2",
    "ML_DSA_44",
    "ML_DSA_65",
    "ML_DSA_87",
];

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

/// Most permissions one CA holds at once.
///
/// A BOUND, in the shape of the quota Amazon Web Services Private CA publishes for the
/// permissions attached to one private CA: a permission is held in memory for the life of
/// the process, so without a ceiling a caller naming a fresh principal per call could grow
/// the store without limit. A grant past it answers the modelled `LimitExceededException`,
/// which names the quota.
pub const MAX_PERMISSIONS_PER_CA: usize = 100;

/// Most live idempotency tokens one map holds. Expired entries are swept on every
/// record, and the oldest live entry is evicted at the ceiling, so a caller sending a
/// fresh token per request cannot grow the map without limit.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 128;

/// Most items one page of a listing carries, whatever `MaxResults` asks for: "Although
/// the maximum value is 1000, the action only returns a maximum of 100 items."
/// (`ListCertificateAuthoritiesRequest$MaxResults` docs). It is also what a request that
/// names no `MaxResults` gets.
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// How many items the page of a listing asked for by `max_results` holds: what the
/// caller asked for, never more than the ceiling the operation publishes, and the
/// ceiling when the caller asked for nothing.
///
/// The modelled `@range(1..1000)` is refused before any handler runs, so a value that
/// reaches here is at least 1.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(MAX_LIST_PAGE_SIZE);
    requested.clamp(1, MAX_LIST_PAGE_SIZE) as usize
}

/// The listing `ListCertificateAuthorities` pages over, as its cursors name it.
pub const LISTING_CERTIFICATE_AUTHORITIES: &str = "certificate-authorities";

/// What Amazon Web Services Private CA answers for a `NextToken` it did not issue for
/// the listing being paged — the modelled exception's own sentence, naming no value out
/// of the request: a token is opaque, so a caller cannot act on its text.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str =
    "The token specified in the NextToken argument is not valid. Use the token returned \
     from your previous call to ListCertificateAuthorities.";

/// The separator between a cursor's listing and the key it resumes at: an ASCII unit
/// separator, which no ARN and no listing name contains.
const PAGE_TOKEN_SEPARATOR: char = '\u{1f}';

/// Encode a pagination cursor: the `listing` it belongs to and the key of the first item
/// of the NEXT page.
///
/// Hex-encoded so the token is opaque to the caller and carries an ARN's `:` and `/`
/// safely, and stamped with the listing so a token one listing issued cannot resume
/// another.
pub fn encode_page_token(listing: &str, key: &str) -> String {
    ::hex::encode(format!("{listing}{PAGE_TOKEN_SEPARATOR}{key}"))
}

/// The key a cursor of `listing` resumes at, or `None` when `token` is not one this
/// service issued for that listing — which the caller answers with
/// `InvalidNextTokenException` rather than by restarting the listing.
pub fn decode_page_token(listing: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (stamped, key) = decoded.split_once(PAGE_TOKEN_SEPARATOR)?;
    (stamped == listing && !key.is_empty()).then(|| key.to_string())
}

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

/// One permission granted on a private CA to a service principal.
///
/// Identified, as Amazon Web Services Private CA identifies it, by the CA it was granted
/// on together with the principal that holds it and the account that assigned it: those
/// three are what `CreatePermission` keys a grant by and what `DeletePermission` names to
/// revoke one.
#[derive(Debug, Clone)]
pub struct PermissionRecord {
    /// The service or identity that holds the permission.
    pub principal: String,
    /// "The ID of the account that assigned the permission." (`Permission$SourceAccount`).
    pub source_account: String,
    /// The `ActionType` members the principal may call on the CA, in the order granted.
    pub actions: Vec<String>,
    /// When `CreatePermission` accepted the grant.
    pub created_at: SystemTime,
}

impl PermissionRecord {
    /// What identifies this grant among the ones its CA holds.
    pub fn key(&self) -> (&str, &str) {
        (self.principal.as_str(), self.source_account.as_str())
    }

    /// The modelled `Permission` structure, read straight off this record.
    ///
    /// `Policy` is left out: it reports "the name of the policy that is associated with
    /// the permission" (`Permission$Policy`), which a grant made through
    /// `CreatePermission` does not carry — only a resource-based policy put through
    /// `PutPolicy` names one.
    pub fn to_wire(&self, certificate_authority_arn: &str) -> Permission {
        Permission {
            certificate_authority_arn: Some(certificate_authority_arn.to_string()),
            created_at: Some(to_date_time(self.created_at)),
            principal: Some(self.principal.clone()),
            source_account: Some(self.source_account.clone()),
            actions: Some(self.actions.clone()),
            policy: None,
        }
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
    /// The resource-based policy attached to the CA, when one is (`PutPolicy` attaches
    /// it, `GetPolicy` reads it, `DeletePolicy` removes it). It lives ON the CA, so it
    /// goes away with the CA rather than outliving it in a map of its own.
    pub resource_policy: Option<String>,
    /// Counter the next issued serial is minted from.
    pub next_serial: u64,
    /// Certificates issued by this CA, keyed by certificate ARN.
    pub certificates: HashMap<String, IssuedCertificate>,
    /// Permissions granted on this CA, one per (principal, source account) pair.
    pub permissions: Vec<PermissionRecord>,
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

    /// The permission this CA holds for `principal` and `source_account`, when it holds
    /// one: the grant those two name is unique among the CA's permissions.
    pub fn permission(
        &self,
        principal: &str,
        source_account: &str,
    ) -> Option<&PermissionRecord> {
        self.permissions
            .iter()
            .find(|permission| permission.key() == (principal, source_account))
    }

    /// Every permission this CA holds, ordered by the pair that identifies a grant.
    ///
    /// A total order over that pair is what lets one `ListPermissions` page resume where
    /// the previous one stopped: the position a `NextToken` names is the same on every
    /// call, whatever order the grants were made in and whether or not the grant the
    /// token names is still there.
    pub fn permissions_in_order(&self) -> Vec<&PermissionRecord> {
        let mut ordered: Vec<&PermissionRecord> = self.permissions.iter().collect();
        ordered.sort_by_key(|permission| permission.key());
        ordered
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
            revocation_configuration: Some(reported_revocation_configuration(
                self.revocation_configuration.as_ref(),
            )),
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

/// The `RevocationConfiguration` a reported `CertificateAuthority` carries.
///
/// Both sub-configurations are always present: `CrlConfiguration$Enabled` and
/// `OcspConfiguration$Enabled` are `@required` booleans, and "By default, both
/// certificate validation mechanisms are disabled."
/// (`CreateCertificateAuthorityRequest$RevocationConfiguration` docs), so a mechanism the
/// CA was not configured with is reported as the disabled configuration rather than left
/// out. A mechanism the CA WAS configured with is reported exactly as it was stored.
pub fn reported_revocation_configuration(
    configured: Option<&RevocationConfiguration>,
) -> RevocationConfiguration {
    RevocationConfiguration {
        crl_configuration: Some(
            configured
                .and_then(|revocation| revocation.crl_configuration.clone())
                .unwrap_or_default(),
        ),
        ocsp_configuration: Some(
            configured
                .and_then(|revocation| revocation.ocsp_configuration.clone())
                .unwrap_or_default(),
        ),
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

    /// Materialise every status change the clock owed at `now`, across every CA.
    ///
    /// What [`certificate_authority_mut`](Self::certificate_authority_mut) does for one
    /// CA, for the whole store: an operation that reports on more than one CA at a time
    /// applies the transitions that were due at the same instant whether or not anybody
    /// called, so no two readers disagree about a CA's current status.
    pub fn settle(&mut self, now: SystemTime) {
        for record in self.certificate_authorities.values_mut() {
            record.settle(now);
        }
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

/// Every private CA whose owner account `owned_by` accepts, in the order a listing
/// reports them.
///
/// The order is by ARN: a stable TOTAL order over the set, since a CA's ARN is unique.
/// That is what makes a cursor cut into the listing exact — a page resumes at the first
/// CA at or after the cursor's ARN, so no CA is reported twice or skipped even when CAs
/// are created between two pages. Amazon Web Services Private CA documents no order for
/// this listing, so any stable total order is faithful; an unstable one (a hash map's
/// iteration order) is not.
pub fn certificate_authorities_where(
    state: &ComAmazonawsAcmpcaAcmprivatecaState,
    owned_by: impl Fn(&str) -> bool,
) -> Vec<CertificateAuthority> {
    let mut records: Vec<&CertificateAuthorityRecord> = state
        .certificate_authorities
        .values()
        .filter(|record| owned_by(&record.owner_account))
        .collect();
    records.sort_by(|left, right| left.arn.cmp(&right.arn));
    records.into_iter().map(|record| record.to_wire()).collect()
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

/// The single PEM block `label` frames, when `text` is exactly one well-formed non-empty
/// block of that label. `None` for anything else — the caller refuses that as malformed.
pub fn parse_pem_block(text: &str, label: &str) -> Option<Vec<u8>> {
    match parse_pem_blocks(text, label)?.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// The body of every PEM block of `label` in `text`, or `None` if any of it is not a
/// well-formed non-empty block of that label.
pub fn parse_pem_blocks(text: &str, label: &str) -> Option<Vec<Vec<u8>>> {
    let blocks = parse_pem_blocks_as(text, label)?;
    (!blocks.is_empty()).then_some(blocks)
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

// ---------------------------------------------------------------------------
// X.509 / PKCS#10 objects
// ---------------------------------------------------------------------------

/// Object identifiers this service names in the DER it reads and writes.
///
/// A hand-written catalog of specification constants — no crate publishes the ML-DSA and
/// SM2 arcs this model's algorithm enums reach — each with its source:
///
/// * the X.500 attribute types a distinguished name carries: RFC 4519 section 2
///   (`2.5.4.*`), whose short names are the ones RFC 4514 section 3 prints;
/// * RSA keys and signatures: RFC 8017 appendix C (`1.2.840.113549.1.1.*`);
/// * EC public keys, the named curves and the ECDSA signatures: RFC 5480 sections 2.1.1
///   and 2.1.1.1 and RFC 5758 section 3.2 (`1.2.840.10045.*`, `1.3.132.0.*`);
/// * ML-DSA: the NIST CSOR arc for FIPS 204 (`2.16.840.1.101.3.4.3.17` ..
///   `2.16.840.1.101.3.4.3.19`);
/// * SM2 keys and the SM3-with-SM2 signature: GM/T 0006 (`1.2.156.10197.1.*`).
mod oid {
    use super::ObjectIdentifier;

    // RFC 4519 section 2 attribute types, in the order `distinguished_name` prints them.
    pub const COMMON_NAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.3");
    pub const SERIAL_NUMBER: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.5");
    pub const ORGANIZATIONAL_UNIT: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.11");
    pub const ORGANIZATION: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.10");
    pub const LOCALITY: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.7");
    pub const STATE_OR_PROVINCE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.8");
    pub const COUNTRY: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.6");
    pub const TITLE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.12");
    pub const SURNAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.4");
    pub const GIVEN_NAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.42");
    pub const INITIALS: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.43");
    pub const PSEUDONYM: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.65");
    pub const GENERATION_QUALIFIER: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.44");
    pub const DN_QUALIFIER: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.46");

    // Public key algorithms.
    pub const RSA_ENCRYPTION: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
    pub const EC_PUBLIC_KEY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
    pub const PRIME256V1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
    pub const SECP384R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.34");
    pub const SECP521R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.35");
    pub const SM2: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.156.10197.1.301");

    // Signature algorithms.
    pub const SHA256_WITH_RSA: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
    pub const SHA384_WITH_RSA: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.12");
    pub const SHA512_WITH_RSA: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.13");
    pub const RSASSA_PSS: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.10");
    pub const ECDSA_WITH_SHA256: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
    pub const ECDSA_WITH_SHA384: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.3");
    pub const ECDSA_WITH_SHA512: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.4");
    pub const SM3_WITH_SM2: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.156.10197.1.501");

    // ML-DSA names one algorithm for the key and the signature alike.
    pub const ML_DSA_44: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.3.17");
    pub const ML_DSA_65: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.3.18");
    pub const ML_DSA_87: ObjectIdentifier =
        ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.3.19");
}

/// Whether `der` is a well-formed X.509 certificate.
///
/// The structure RFC 5280 section 4.1 defines, decoded by `x509-cert`: a body that is not
/// one is what `MalformedCertificateException` answers. Nothing here verifies the
/// signature — the emulator holds no issuer key to verify it against.
pub fn is_x509_certificate(der: &[u8]) -> bool {
    Certificate::from_der(der).is_ok()
}

/// The PKCS#10 certification request `der` carries (RFC 2986 section 4), or `None` when
/// it is not one — what `MalformedCSRException` answers.
pub fn parse_certification_request(der: &[u8]) -> Option<CertificationRequest> {
    CertReq::from_der(der).ok()
}

/// The certification request this CA hands out, derived once at create time.
///
/// A real PKCS#10 `CertificationRequest` whose subject is the CA's own configured
/// subject and whose `SubjectPublicKeyInfo` names the algorithm the CA was created with,
/// so the body the CA hands out is the structure a signing authority expects — and the
/// certificate this service issues for it (below) can be imported back into a CA.
///
/// The emulator holds no HSM, so there is no key pair and no signature to make: the
/// public key and the signature are stand-ins of the right ASN.1 shape, each the SHA-256
/// digest of the content it belongs to (the CA's ARN, and the request body the signature
/// covers), so the same CA always answers with the same CSR. Nothing in the emulator
/// verifies either.
///
/// `None` when the configured subject cannot be encoded as an X.500 name.
pub fn derive_csr_pem(
    arn: &str,
    configuration: &CertificateAuthorityConfiguration,
) -> Option<String> {
    let info = CertReqInfo {
        version: x509_cert::request::Version::V1,
        subject: subject_name(&configuration.subject)?,
        public_key: public_key_info(&configuration.key_algorithm, arn)?,
        attributes: SetOfVec::new(),
    };
    let algorithm = signature_algorithm_identifier(&configuration.signing_algorithm)?;
    let signature = stand_in_signature(&info.to_der().ok()?)?;
    let request = CertReq {
        info,
        algorithm,
        signature,
    };
    Some(encode_pem(PEM_LABEL_CSR, &request.to_der().ok()?))
}

/// The certificate this CA issues for a certification request, built from what the CA
/// persisted and from the request it answers.
///
/// A real X.509 `Certificate` (RFC 5280 section 4.1): the serial the CA allocated, the
/// issuer the CA's own subject, the resolved validity window, and — as a CA does — the
/// subject and the public key copied out of the certification request being signed.
///
/// As with the CSR above there is no key to sign with, so the signature is a stand-in of
/// the right ASN.1 shape: the SHA-256 digest of the TBS certificate the signature would
/// cover. Nothing in the emulator verifies it.
///
/// `None` when the CA's subject, the allocated serial or the validity window cannot be
/// encoded as the certificate's DER demands.
pub fn derive_certificate_pem(
    ca: &CertificateAuthorityRecord,
    serial: &str,
    request: &CertificationRequestInfo,
    signing_algorithm: &str,
    not_before: SystemTime,
    not_after: SystemTime,
) -> Option<String> {
    let algorithm = signature_algorithm_identifier(signing_algorithm)?;
    let tbs_certificate = TbsCertificate {
        version: Version::V3,
        serial_number: serial_number(serial)?,
        signature: algorithm.clone(),
        issuer: subject_name(&ca.configuration.subject)?,
        validity: CertificateValidity {
            not_before: certificate_time(not_before)?,
            not_after: certificate_time(not_after)?,
        },
        subject: request.subject.clone(),
        subject_public_key_info: request.public_key.clone(),
        issuer_unique_id: None,
        subject_unique_id: None,
        extensions: None,
    };
    let signature = stand_in_signature(&tbs_certificate.to_der().ok()?)?;
    let certificate = Certificate {
        tbs_certificate,
        signature_algorithm: algorithm,
        signature,
    };
    Some(encode_pem(PEM_LABEL_CERTIFICATE, &certificate.to_der().ok()?))
}

/// The certificate serial `serial` names, as an X.509 `CertificateSerialNumber`.
///
/// The CA allocates its serials as hex ([`mint_serial`]), which is how they appear in a
/// certificate ARN; the DER integer the certificate carries is that same number, so the
/// two always agree. `SerialNumber` keeps it the positive integer RFC 5280 section
/// 4.1.2.2 requires by prepending a zero byte when the leading bit is set, which changes
/// no value.
fn serial_number(serial: &str) -> Option<SerialNumber> {
    SerialNumber::new(&::hex::decode(serial).ok()?).ok()
}

/// `time` as the certificate validity field RFC 5280 section 4.1.2.5 calls for: UTCTime
/// through 2049 and GeneralizedTime from 2050, which is what `Time::try_from` picks.
fn certificate_time(time: SystemTime) -> Option<Time> {
    Time::try_from(time).ok()
}

/// The `AlgorithmIdentifier` of a modelled `SigningAlgorithm`, or `None` for a value the
/// enum does not declare.
///
/// The three `*_RSA_PSS` members share the RSASSA-PSS identifier and this emitter leaves
/// its `RSASSA-PSS-params` out, so they are not told apart in the DER: the parameters
/// describe the digest and salt of a signature, and the emulator makes no signature to
/// describe (see [`stand_in_signature`]). The member the caller asked for is persisted on
/// the issued certificate either way.
fn signature_algorithm_identifier(signing_algorithm: &str) -> Option<AlgorithmIdentifierOwned> {
    let (algorithm, parameters) = match signing_algorithm {
        // RSASSA-PKCS1-v1_5 takes a NULL parameter (RFC 8017 appendix C).
        "SHA256WITHRSA" => (oid::SHA256_WITH_RSA, Some(null_parameters()?)),
        "SHA384WITHRSA" => (oid::SHA384_WITH_RSA, Some(null_parameters()?)),
        "SHA512WITHRSA" => (oid::SHA512_WITH_RSA, Some(null_parameters()?)),
        "SHA256WITHRSA_PSS" | "SHA384WITHRSA_PSS" | "SHA512WITHRSA_PSS" => {
            (oid::RSASSA_PSS, None)
        }
        // ECDSA and ML-DSA identifiers take no parameters (RFC 5758 section 3.2).
        "SHA256WITHECDSA" => (oid::ECDSA_WITH_SHA256, None),
        "SHA384WITHECDSA" => (oid::ECDSA_WITH_SHA384, None),
        "SHA512WITHECDSA" => (oid::ECDSA_WITH_SHA512, None),
        "SM3WITHSM2" => (oid::SM3_WITH_SM2, None),
        "ML_DSA_44" => (oid::ML_DSA_44, None),
        "ML_DSA_65" => (oid::ML_DSA_65, None),
        "ML_DSA_87" => (oid::ML_DSA_87, None),
        _ => return None,
    };
    Some(AlgorithmIdentifierOwned {
        oid: algorithm,
        parameters,
    })
}

/// The `SubjectPublicKeyInfo` of a CA created with `key_algorithm`.
///
/// The algorithm identifier is the real one for that member of `KeyAlgorithm`; the key
/// bits are a stand-in derived from `seed` (see [`derive_csr_pem`]), since the emulator
/// holds no key material. `None` for a value `KeyAlgorithm` does not declare.
fn public_key_info(key_algorithm: &str, seed: &str) -> Option<SubjectPublicKeyInfoOwned> {
    let (algorithm, parameters) = match key_algorithm {
        "RSA_2048" | "RSA_3072" | "RSA_4096" => (oid::RSA_ENCRYPTION, Some(null_parameters()?)),
        "EC_prime256v1" => (oid::EC_PUBLIC_KEY, Some(curve_parameters(oid::PRIME256V1)?)),
        "EC_secp384r1" => (oid::EC_PUBLIC_KEY, Some(curve_parameters(oid::SECP384R1)?)),
        "EC_secp521r1" => (oid::EC_PUBLIC_KEY, Some(curve_parameters(oid::SECP521R1)?)),
        "SM2" => (oid::EC_PUBLIC_KEY, Some(curve_parameters(oid::SM2)?)),
        "ML_DSA_44" => (oid::ML_DSA_44, None),
        "ML_DSA_65" => (oid::ML_DSA_65, None),
        "ML_DSA_87" => (oid::ML_DSA_87, None),
        _ => return None,
    };
    Some(SubjectPublicKeyInfoOwned {
        algorithm: AlgorithmIdentifierOwned {
            oid: algorithm,
            parameters,
        },
        subject_public_key: BitString::from_bytes(&Sha256::digest(seed.as_bytes())).ok()?,
    })
}

/// The named-curve `parameters` of an EC public key (RFC 5480 section 2.1.1).
fn curve_parameters(curve: ObjectIdentifier) -> Option<Any> {
    Any::new(DerTag::ObjectIdentifier, curve.as_bytes()).ok()
}

/// The explicit NULL an RSA algorithm identifier carries as its parameters.
fn null_parameters() -> Option<Any> {
    Any::new(DerTag::Null, Vec::new()).ok()
}

/// The signature bits of an object this service writes: the SHA-256 digest of the DER the
/// signature covers, standing in for a signature the emulator has no key to make.
fn stand_in_signature(signed: &[u8]) -> Option<BitString> {
    BitString::from_bytes(&Sha256::digest(signed)).ok()
}

/// The `SigningAlgorithm` members usable with `key_algorithm`, in the order the model
/// declares them: the ones whose algorithm family is the family of that key.
pub fn signing_algorithms_for_key(key_algorithm: &str) -> Vec<&'static str> {
    let Some(family) = algorithm_family(key_algorithm) else {
        return Vec::new();
    };
    SIGNING_ALGORITHMS_IN_MODEL_ORDER
        .iter()
        .copied()
        .filter(|signing| algorithm_family(signing) == Some(family))
        .collect()
}

/// The subject's relative distinguished names, most specific first: the X.500 attribute
/// type, the short name RFC 4514 section 3 prints for it, and the value.
///
/// The one ordered reading of an `ASN1Subject`, shared by the RFC 4514 string below and
/// by the DER `Name` the certificates carry, so the two can never disagree. An attribute
/// the request left out or left empty names nothing and is not a relative distinguished
/// name.
fn subject_attributes(subject: &ASN1Subject) -> Vec<(ObjectIdentifier, &'static str, &str)> {
    [
        (oid::COMMON_NAME, "CN", &subject.common_name),
        (oid::SERIAL_NUMBER, "SERIALNUMBER", &subject.serial_number),
        (
            oid::ORGANIZATIONAL_UNIT,
            "OU",
            &subject.organizational_unit,
        ),
        (oid::ORGANIZATION, "O", &subject.organization),
        (oid::LOCALITY, "L", &subject.locality),
        (oid::STATE_OR_PROVINCE, "ST", &subject.state),
        (oid::COUNTRY, "C", &subject.country),
        (oid::TITLE, "TITLE", &subject.title),
        (oid::SURNAME, "SN", &subject.surname),
        (oid::GIVEN_NAME, "GIVENNAME", &subject.given_name),
        (oid::INITIALS, "INITIALS", &subject.initials),
        (oid::PSEUDONYM, "PSEUDONYM", &subject.pseudonym),
        (
            oid::GENERATION_QUALIFIER,
            "GENERATIONQUALIFIER",
            &subject.generation_qualifier,
        ),
        (
            oid::DN_QUALIFIER,
            "DNQ",
            &subject.distinguished_name_qualifier,
        ),
    ]
    .into_iter()
    .filter_map(|(attribute, key, value)| {
        value
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| (attribute, key, value))
    })
    .collect()
}

/// The subject's X.500 distinguished name, most specific relative distinguished name
/// first, as RFC 4514 renders one.
pub fn distinguished_name(subject: &ASN1Subject) -> String {
    let mut rdns: Vec<String> = subject_attributes(subject)
        .into_iter()
        .map(|(_, key, value)| format!("{key}={value}"))
        .collect();
    for attribute in subject.custom_attributes.iter().flatten() {
        rdns.push(format!(
            "{}={}",
            attribute.object_identifier, attribute.value
        ));
    }
    rdns.join(",")
}

/// The subject as the DER `Name` a certificate or certification request carries: one
/// relative distinguished name per attribute the subject supplies, in the order
/// [`subject_attributes`] reads them, each a `SET OF` one `AttributeTypeAndValue` whose
/// value is a UTF8String (RFC 5280 section 4.1.2.4).
///
/// `None` when an attribute cannot be encoded — a `CustomAttribute` whose
/// `ObjectIdentifier` is not an object identifier, or a value the DER string type refuses.
fn subject_name(subject: &ASN1Subject) -> Option<Name> {
    let mut rdns: Vec<RelativeDistinguishedName> = Vec::new();
    for (attribute, _, value) in subject_attributes(subject) {
        rdns.push(relative_distinguished_name(attribute, value)?);
    }
    for attribute in subject.custom_attributes.iter().flatten() {
        let custom = ObjectIdentifier::new(&attribute.object_identifier).ok()?;
        rdns.push(relative_distinguished_name(custom, &attribute.value)?);
    }
    Some(Name::from(RdnSequence(rdns)))
}

/// One relative distinguished name: the `SET OF` holding the single attribute type and
/// its UTF8String value.
fn relative_distinguished_name(
    attribute: ObjectIdentifier,
    value: &str,
) -> Option<RelativeDistinguishedName> {
    let pair = AttributeTypeAndValue {
        oid: attribute,
        value: Any::new(DerTag::Utf8String, value.as_bytes()).ok()?,
    };
    Some(RelativeDistinguishedName::from(
        SetOfVec::try_from(vec![pair]).ok()?,
    ))
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
