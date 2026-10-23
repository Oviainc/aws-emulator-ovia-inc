// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-securityagent-securityagent service.
//!
//! The Application records every application operation reads and writes live here, so
//! `CreateApplication`, `GetApplication`, `ListApplications`, `UpdateApplication` and
//! `DeleteApplication` all see one copy of each resource. The private connection records
//! `CreatePrivateConnection`, `DescribePrivateConnection` and `ListPrivateConnections`
//! share live here for the same reason, and so do the target domain records:
//! `CreateTargetDomain` writes one and `BatchGetTargetDomains` reads that same record
//! back.
//!
//! The agent space records sit beside them in the same struct, so `CreateAgentSpace`
//! and `BatchGetAgentSpaces` — and every agent space operation implemented after them —
//! read ONE copy of each agent space.
//!
//! The security requirement packs live here for the same reason:
//! `CreateSecurityRequirementPack`, `GetSecurityRequirementPack`,
//! `ListSecurityRequirementPacks` — and every pack operation implemented after them —
//! read and write ONE copy of each pack record.
//!
//! The helpers below (identity minting, the IdC application ARN, the account's network,
//! the modelled projections and the pagination cursor codec) are shared by those
//! handlers rather than duplicated per operation.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AWSResources, AgentSpace, ApplicationSummary, CodeReviewSettings, DnsVerification,
    HttpVerification, IdCConfiguration, PrivateConnectionSummary,
    SecurityRequirementPackSummary, TargetDomain, VerificationDetails,
};

/// The region every value this service derives from the request's region carries when
/// the request arrived without a SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many summary entries one listing page holds when the caller asks for no page
/// size. Shared by every paginated listing of this service (`ListApplications`,
/// `ListSecurityRequirementPacks`, `ListPrivateConnections`), which all take the one
/// modelled `MaxResults` shape.
///
/// CHOSEN, not cited: `MaxResults` carries neither `@default` nor `@range` in the
/// model, so nothing pins this value. It only bounds the size of one response.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// The most summary entries one listing page can hold, whatever the caller asks for.
///
/// CHOSEN, not cited, for the same reason as [`DEFAULT_MAX_RESULTS`]: `MaxResults`
/// carries no `@range`, so nothing is refused at the boundary and a caller could
/// otherwise ask for the whole store in one response. A larger request is clamped to
/// this ceiling and the page simply carries a `nextToken`, so no record becomes
/// unreachable — only the work and the response size one call can demand are bounded.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most applications this emulator stores for the account before
/// `CreateApplication` answers the service's `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited: the real service's application quota is not in the model. It is
/// set far above any plausible client's application count, so normal use never meets
/// it, while the store a caller can grow stays bounded.
pub const MAX_APPLICATIONS: usize = 1_000;

/// The most target domains this emulator stores for the account before
/// `CreateTargetDomain` answers the service's `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited, for the same reason as [`MAX_APPLICATIONS`]: the real service's
/// target domain quota is not in the model. It is set far above any plausible client's
/// domain count, so normal use never meets it, while the store a caller can grow stays
/// bounded.
pub const MAX_TARGET_DOMAINS: usize = 1_000;

/// The longest a `targetDomainName` may be.
///
/// NOT chosen: `targetDomainName` carries no `@length` in the model, but the operation
/// registers a web domain, and a DNS name is at most 253 characters in presentation
/// form (RFC 1035 §2.3.4 with RFC 1123 §2.1).
pub const MAX_DOMAIN_NAME_LEN: usize = 253;

/// The longest one dot-separated label of a `targetDomainName` may be — 63 characters
/// (RFC 1035 §2.3.4).
pub const MAX_DOMAIN_LABEL_LEN: usize = 63;

/// The `DomainVerificationMethod` that verifies ownership through a DNS TXT record
/// (source: `smithy_query(query="shape",shape="DomainVerificationMethod")`).
pub const VERIFICATION_METHOD_DNS_TXT: &str = "DNS_TXT";

/// The `DomainVerificationMethod` that verifies ownership through an HTTP route.
pub const VERIFICATION_METHOD_HTTP_ROUTE: &str = "HTTP_ROUTE";

/// The `DomainVerificationMethod` that verifies ownership by IP from a private VPC.
pub const VERIFICATION_METHOD_PRIVATE_VPC: &str = "PRIVATE_VPC";

/// Every value the `DomainVerificationMethod` enum admits, in the model's order
/// (source: `smithy_query(query="shape",shape="DomainVerificationMethod")`), so a
/// value outside the enum is refused rather than stored and read back as a method
/// with no challenge.
pub const VERIFICATION_METHOD_VALUES: [&str; 3] = [
    VERIFICATION_METHOD_DNS_TXT,
    VERIFICATION_METHOD_HTTP_ROUTE,
    VERIFICATION_METHOD_PRIVATE_VPC,
];

/// The `TargetDomainStatus` a registered target domain carries until its ownership is
/// verified (source: the `PENDING` value's own doc, "Domain verification is pending").
///
/// A target domain does NOT leave this status on a timer: the model documents that
/// ownership is proven by a record the domain's owner publishes, which nothing inside
/// this emulator can observe, so the status moves only when a verification call finds
/// that proof.
pub const TARGET_DOMAIN_STATUS_PENDING: &str = "PENDING";

/// The label prefixed onto the domain to name the TXT record a `DNS_TXT` target
/// domain's owner publishes.
///
/// CHOSEN, not cited: the model pins the record TYPE (`DNSRecordType` admits only
/// `TXT`) and says the details carry "the name of the DNS record to create for
/// verification", but not which label the real service picks. An underscore-prefixed
/// label cannot collide with a hostname the domain already serves.
const DNS_CHALLENGE_LABEL: &str = "_securityagent-challenge";

/// The type of that record. NOT chosen: `DNSRecordType` admits exactly one value,
/// `TXT` ("Currently, only TXT is supported").
const DNS_CHALLENGE_RECORD_TYPE: &str = "TXT";

/// The directory of the HTTP route an `HTTP_ROUTE` target domain's owner serves its
/// token at; the token itself is the last segment, so two target domains never share
/// one path.
///
/// CHOSEN, not cited: the model says the details carry "the HTTP route path where the
/// verification token must be served" without pinning the path. It sits under the
/// `/.well-known/` prefix reserved for exactly this kind of challenge (RFC 8615).
const HTTP_CHALLENGE_ROUTE_PREFIX: &str = "/.well-known/securityagent-domain-verification";

/// The domain-separation prefix hashed into every verification token, so a token can
/// only ever stand for a target domain of this service.
const VERIFICATION_TOKEN_CONTEXT: &str = "securityagent-domain-verification:";

/// The most agent spaces this emulator stores for the account before
/// `CreateAgentSpace` answers the service's `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited, for the same reason as [`MAX_APPLICATIONS`]: the real service's
/// agent space quota is not in the model. It is set far above any plausible client's
/// agent space count, so normal use never meets it, while the store a caller can grow
/// stays bounded.
pub const MAX_AGENT_SPACES: usize = 1_000;

/// The most security requirement packs this emulator stores for the account before
/// `CreateSecurityRequirementPack` answers the service's
/// `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited: the real service's pack quota is not in the model. It is set far
/// above any plausible client's pack count, so normal use never meets it, while the
/// store a caller can grow stays bounded.
pub const MAX_SECURITY_REQUIREMENT_PACKS: usize = 1_000;

/// The `managementType` every pack this API creates carries.
///
/// Source: `CreateSecurityRequirementPack`'s own documentation — "Creates a *customer
/// managed* security requirement pack" — and the `ManagementType` enum
/// (`AWS_MANAGED | CUSTOMER_MANAGED`), whose other member describes packs AWS ships
/// rather than ones a caller creates.
pub const MANAGEMENT_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// The `status` a pack created without one carries.
///
/// Source: the `status` member's documentation on `CreateSecurityRequirementPackInput`
/// — "The status of the pack. Defaults to ENABLED if not provided."
pub const PACK_STATUS_ENABLED: &str = "ENABLED";

/// The other member of the `SecurityRequirementPackStatus` enum.
pub const PACK_STATUS_DISABLED: &str = "DISABLED";

/// Every value the `SecurityRequirementPackStatus` enum admits.
///
/// Source: `smithy_query(query="shape", shape="SecurityRequirementPackStatus")` —
/// `["ENABLED", "DISABLED"]`.
pub const PACK_STATUS_VALUES: [&str; 2] = [PACK_STATUS_ENABLED, PACK_STATUS_DISABLED];

/// The prefix every `ListApplications` pagination cursor carries, so a token this
/// service did not mint is refused instead of being read as a resume point.
const PAGE_TOKEN_PREFIX: &str = "app-v1:";

/// The prefix every `ListSecurityRequirementPacks` pagination cursor carries. Distinct
/// from [`PAGE_TOKEN_PREFIX`], so a cursor one list minted is not read as a resume
/// point by the other.
const PACK_PAGE_TOKEN_PREFIX: &str = "pack-v1:";

/// The same, for `ListPrivateConnections`: a cursor minted for one listing names a
/// resume point in another listing's key space, so each listing carries its own prefix
/// and a cursor taken from the wrong one is refused.
const PRIVATE_CONNECTION_PAGE_TOKEN_PREFIX: &str = "privconn-v1:";

/// The IAM Identity Center instance id an `idcInstanceArn` that names none falls back
/// to, so the minted `idcApplicationArn` is always well formed.
const FALLBACK_IDC_INSTANCE_ID: &str = "ssoins-emulator";

/// The IAM Identity Center configuration stored for an application.
///
/// Mirrors the modelled [`IdCConfiguration`] shape: `idcInstanceArn` is what the
/// caller supplied, `idcApplicationArn` is the ARN this emulator minted for the
/// Identity Center application it stands up alongside the application record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdcConfig {
    pub idc_application_arn: Option<String>,
    pub idc_instance_arn: Option<String>,
}

impl IdcConfig {
    /// This configuration projected onto the modelled shape the API returns.
    pub fn modelled(&self) -> IdCConfiguration {
        IdCConfiguration {
            idc_application_arn: self.idc_application_arn.clone(),
            idc_instance_arn: self.idc_instance_arn.clone(),
        }
    }
}

/// One stored application: the top-level organizational unit every agent space, code
/// review, pentest, threat model and finding hangs off.
#[derive(Debug, Clone)]
pub struct Application {
    pub application_id: String,
    pub application_name: String,
    pub domain: String,
    pub role_arn: Option<String>,
    pub default_kms_key_id: Option<String>,
    pub idc_configuration: Option<IdcConfig>,
    /// The tags supplied at create time, persisted so the tagging operations have
    /// state to read once they are implemented.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
}

impl Application {
    /// This application projected onto the `ApplicationSummary` shape
    /// `ListApplications` returns: the stored record's own fields, never request input.
    pub fn summary(&self) -> ApplicationSummary {
        ApplicationSummary {
            application_id: self.application_id.clone(),
            application_name: self.application_name.clone(),
            domain: self.domain.clone(),
            default_kms_key_id: self.default_kms_key_id.clone(),
        }
    }
}

/// One stored agent space: "a dedicated workspace for securing a specific
/// application" (source: the `AgentSpace` shape doc), carrying the members the caller
/// supplied plus the identity and the timestamps the service mints.
///
/// The modelled shapes are stored as they arrived rather than flattened into local
/// mirrors, so a read projects the stored record and never reconstructs one from
/// request text.
#[derive(Debug, Clone)]
pub struct AgentSpaceRecord {
    pub agent_space_id: String,
    pub name: String,
    pub description: Option<String>,
    pub aws_resources: Option<AWSResources>,
    pub target_domain_ids: Option<Vec<String>>,
    pub code_review_settings: Option<CodeReviewSettings>,
    pub kms_key_id: Option<String>,
    /// The tags supplied at create time, persisted so the tagging operations have
    /// state to read once they are implemented. `tags` is not a member of any agent
    /// space response shape, so it is never projected below.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AgentSpaceRecord {
    /// This agent space projected onto the modelled [`AgentSpace`] shape the batch read
    /// returns: the stored record's own fields, never request input.
    pub fn modelled(&self) -> AgentSpace {
        AgentSpace {
            agent_space_id: self.agent_space_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            aws_resources: self.aws_resources.clone(),
            target_domain_ids: self.target_domain_ids.clone(),
            code_review_settings: self.code_review_settings.clone(),
            kms_key_id: self.kms_key_id.clone(),
            created_at: Some(timestamp(self.created_at)),
            updated_at: Some(timestamp(self.updated_at)),
        }
    }
}

/// A stored instant on the wire, as the modelled timestamp members of an agent space
/// carry it (`smithy_query(query="shape",shape="AgentSpace")`).
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// One stored security requirement pack: the named collection of security
/// requirements `CreateSecurityRequirementPack` mints and every other pack operation
/// reads.
///
/// `management_type`, `vendor_name` and `import_status` are the service's to choose,
/// not the caller's: a pack this API creates is customer managed, so it carries no
/// vendor, and it holds no import until `ImportSecurityRequirements` starts one.
#[derive(Debug, Clone)]
pub struct SecurityRequirementPack {
    pub pack_id: String,
    pub name: String,
    pub description: Option<String>,
    /// `None` for a customer managed pack: the member documents it as "The vendor name
    /// for AWS managed packs".
    pub vendor_name: Option<String>,
    /// One of the `ManagementType` enum values; always
    /// [`MANAGEMENT_TYPE_CUSTOMER_MANAGED`] for a pack created through this API.
    pub management_type: String,
    /// One of [`PACK_STATUS_VALUES`].
    pub status: String,
    /// The `SecurityRequirementPackImportStatus` of the pack's last import, or `None`
    /// while no import has been started for it.
    pub import_status: Option<String>,
    pub kms_key_id: Option<String>,
    /// The tags supplied at create time, persisted so the tagging operations have
    /// state to read once they are implemented.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl SecurityRequirementPack {
    /// This pack projected onto the `SecurityRequirementPackSummary` shape
    /// `ListSecurityRequirementPacks` returns: the stored record's own fields, never
    /// request input.
    pub fn summary(&self) -> SecurityRequirementPackSummary {
        SecurityRequirementPackSummary {
            pack_id: self.pack_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            vendor_name: self.vendor_name.clone(),
            management_type: self.management_type.clone(),
            status: self.status.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }
}

/// The identity minted for a new application: the three values that are the service's
/// to choose rather than the caller's to supply.
#[derive(Debug, Clone)]
pub struct ApplicationIdentity {
    pub application_id: String,
    pub application_name: String,
    pub domain: String,
    /// The monotonic sequence number behind this identity, used to mint the IAM
    /// Identity Center application ARN that goes with it.
    pub sequence: u64,
}

// ---------------------------------------------------------------------------
// Private connections
// ---------------------------------------------------------------------------

/// The two `PrivateConnectionType` values
/// (`smithy_query(query="shape",shape="PrivateConnectionType")`).
pub const TYPE_SERVICE_MANAGED: &str = "SERVICE_MANAGED";
/// See [`TYPE_SERVICE_MANAGED`].
pub const TYPE_SELF_MANAGED: &str = "SELF_MANAGED";

/// The `PrivateConnectionStatus` a connection carries while the clock still owes it its
/// `ACTIVE` status, and the one it carries from then on
/// (`smithy_query(query="shape",shape="PrivateConnectionStatus")`).
pub const STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
/// See [`STATUS_CREATE_IN_PROGRESS`].
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// How long after its create call a private connection still reports
/// `CREATE_IN_PROGRESS`.
///
/// A created private connection is answered in the non-terminal
/// `CREATE_IN_PROGRESS` status and reaches `ACTIVE` on its own, so the transition is
/// one the CLOCK owes. AWS documents no duration for it, so this is the emulator's
/// CHOSEN span, inside the 250–750 ms window this project fixes for such a
/// transition. It is never zero, so a caller that reads straight back still observes
/// the transient status; nothing sleeps for it — the due instant is stored and
/// compared on read.
pub const CREATE_TRANSITION: Duration = Duration::from_millis(500);

/// The `dnsResolution` a service-managed create that names none takes.
///
/// Cited: `ServiceManagedInput$dnsResolution` — "The DNS resolution mode for the
/// resource gateway. Defaults to PUBLIC when not set."
pub const DEFAULT_DNS_RESOLUTION: &str = "PUBLIC";

/// The most private connections this emulator stores for the account before
/// `CreatePrivateConnection` answers the service's `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited: the real service's private connection quota is not in the model.
/// It is set far above any plausible client's connection count, so normal use never
/// meets it, while the store a caller can grow stays bounded.
pub const MAX_PRIVATE_CONNECTIONS: usize = 100;

/// The ARN namespace (and SigV4 signing name) of VPC Lattice, the service that owns the
/// resource gateways and resource configurations a private connection is built on.
pub const VPC_LATTICE_ARN_SERVICE: &str = "vpc-lattice";

/// The ARN resource type of a VPC Lattice resource gateway, as
/// `com.amazonaws.vpclattice#ResourceGatewayArn`'s pattern spells it
/// (`…:resourcegateway/rgw-…`).
pub const KIND_RESOURCE_GATEWAY: &str = "resourcegateway";
/// The ARN resource type of a VPC Lattice resource configuration, as
/// `com.amazonaws.vpclattice#ResourceConfigurationArn`'s pattern spells it
/// (`…:resourceconfiguration/rcfg-…`).
pub const KIND_RESOURCE_CONFIGURATION: &str = "resourceconfiguration";

/// The id prefix of a VPC Lattice resource gateway
/// (`com.amazonaws.vpclattice#ResourceGatewayId`: `^rgw-[0-9a-z]{17}$`).
pub const RESOURCE_GATEWAY_ID_PREFIX: &str = "rgw-";
/// The id prefix of a VPC Lattice resource configuration
/// (`com.amazonaws.vpclattice#ResourceConfigurationId`: `^rcfg-[0-9a-z]{17}$`).
pub const RESOURCE_CONFIGURATION_ID_PREFIX: &str = "rcfg-";
/// How many `[0-9a-z]` characters follow the prefix of either id, from those same two
/// patterns.
pub const LATTICE_ID_SUFFIX_LEN: usize = 17;

/// The lowercase base-36 alphabet those id patterns admit (`[0-9a-z]`).
const BASE36_LOWER: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// One stored private connection: the record every private connection operation reads.
#[derive(Debug, Clone)]
pub struct PrivateConnection {
    /// The caller-chosen name, unique within the account — this record's key.
    pub name: String,
    /// `PrivateConnectionType`: [`TYPE_SERVICE_MANAGED`] or [`TYPE_SELF_MANAGED`].
    pub connection_type: String,
    /// The VPC Lattice resource gateway this connection reaches the provider through.
    /// Present for a service-managed connection, whose gateway the service creates;
    /// absent for a self-managed one, where the gateway is the caller's own and only
    /// the resource configuration is named to the service.
    pub resource_gateway_id: Option<String>,
    /// The VPC Lattice resource configuration: the one the service created beside the
    /// gateway for a service-managed connection, or the one the caller named for a
    /// self-managed one, as the account holds it (the bare id, not the request's form).
    pub resource_configuration_id: Option<String>,
    /// `hostAddress` as the service-managed create supplied it.
    pub host_address: Option<String>,
    /// The VPC the service-managed resource gateway was created in.
    pub vpc_id: Option<String>,
    /// The subnets that gateway spans, persisted so a later read reports the placement
    /// this connection was created with.
    pub subnet_ids: Vec<String>,
    /// The security groups attached to that gateway.
    pub security_group_ids: Vec<String>,
    /// `ipAddressType` as supplied, `None` when the create named none.
    pub ip_address_type: Option<String>,
    /// `ipv4AddressesPerEni` as supplied.
    pub ipv4_addresses_per_eni: Option<i32>,
    /// The TCP port ranges a consumer may reach the resource on, as supplied.
    pub port_ranges: Vec<String>,
    /// `ResourceConfigDnsResolution`, defaulted per [`DEFAULT_DNS_RESOLUTION`] for a
    /// service-managed connection; `None` for a self-managed one, whose resource
    /// configuration the caller owns and whose DNS mode this service does not choose.
    pub dns_resolution: Option<String>,
    /// The tags supplied at create time.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// The instant the clock owes this connection its `ACTIVE` status.
    pub active_at: SystemTime,
}

impl PrivateConnection {
    /// `PrivateConnectionStatus` at `now`: the transient create status until the stored
    /// due instant passes, [`STATUS_ACTIVE`] from then on. A pure function of stored
    /// state and the clock, so reading it changes nothing a later caller sees.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.active_at {
            STATUS_ACTIVE
        } else {
            STATUS_CREATE_IN_PROGRESS
        }
    }

    /// This connection projected onto the `PrivateConnectionSummary` shape, which
    /// carries every member `CreatePrivateConnection`, `DescribePrivateConnection` and
    /// `ListPrivateConnections` report — so the three answer ONE projection of the
    /// stored record rather than three hand-written ones.
    pub fn summary(&self, now: SystemTime) -> PrivateConnectionSummary {
        PrivateConnectionSummary {
            name: self.name.clone(),
            r#type: self.connection_type.clone(),
            status: self.status(now).to_string(),
            resource_gateway_id: self.resource_gateway_id.clone(),
            host_address: self.host_address.clone(),
            vpc_id: self.vpc_id.clone(),
            resource_configuration_id: self.resource_configuration_id.clone(),
            // The certificate a create supplies is the CALLER's own PEM chain, and the
            // expiry AWS reports here is that certificate's `notAfter`. This emulator
            // reads the chain's PEM envelope but does not decode the X.509 inside it,
            // so it reports no expiry rather than a value it would have had to invent;
            // a value that is not a PEM certificate chain is still refused at create
            // time.
            certificate_expiry_time: None,
            dns_resolution: self.dns_resolution.clone(),
            // No stored connection has failed: this emulator moves a connection from
            // `CREATE_IN_PROGRESS` to `ACTIVE` and never to a failed status, so there
            // is no failure to describe. The member stays absent rather than carrying
            // a message for a failure that did not happen.
            failure_message: None,
            // AWS reports no tag map for an untagged resource, so an empty map is
            // absent rather than present and empty.
            tags: (!self.tags.is_empty())
                .then(|| self.tags.clone().into_iter().collect()),
        }
    }
}

/// One registered target domain: a web domain that must be verified before a pentest
/// may target it.
#[derive(Debug, Clone)]
pub struct TargetDomainRecord {
    pub target_domain_id: String,
    /// The domain name the caller registered, as supplied.
    pub domain_name: String,
    /// The `DomainVerificationMethod` ownership is to be proven with.
    pub verification_method: String,
    /// The record's `TargetDomainStatus`.
    pub verification_status: String,
    pub verification_status_reason: Option<String>,
    /// The token this service minted for the owner to publish. Stored, not recomputed
    /// per read, so every projection of the record carries the same token.
    pub verification_token: String,
    /// The tags supplied at create time, persisted so the tagging operations have
    /// state to read once they are implemented.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// When ownership was proven; `None` while the record is not `VERIFIED`.
    pub verified_at: Option<SystemTime>,
}

impl TargetDomainRecord {
    /// A newly registered target domain: `PENDING`, carrying the challenge its chosen
    /// verification method requires and no verification time yet.
    pub fn pending(
        identity: TargetDomainIdentity,
        domain_name: String,
        verification_method: String,
        tags: BTreeMap<String, String>,
    ) -> Self {
        let mut record = Self {
            target_domain_id: identity.target_domain_id,
            domain_name,
            verification_method,
            verification_status: TARGET_DOMAIN_STATUS_PENDING.to_string(),
            verification_status_reason: None,
            verification_token: identity.verification_token,
            tags,
            created_at: SystemTime::now(),
            verified_at: None,
        };
        // The reason names the challenge, which is built from the minted token, so it
        // is computed once the rest of the record exists.
        record.verification_status_reason = Some(record.pending_status_reason());
        record
    }

    /// The name of the TXT record a `DNS_TXT` target domain's owner publishes.
    pub fn dns_record_name(&self) -> String {
        format!("{DNS_CHALLENGE_LABEL}.{}", self.domain_name)
    }

    /// The path an `HTTP_ROUTE` target domain's owner serves the token at.
    pub fn http_route_path(&self) -> String {
        format!("{HTTP_CHALLENGE_ROUTE_PREFIX}/{}", self.verification_token)
    }

    /// Why this record is still `PENDING`: what the domain's owner has to do before a
    /// verification call can find the proof.
    fn pending_status_reason(&self) -> String {
        match self.verification_method.as_str() {
            VERIFICATION_METHOD_DNS_TXT => format!(
                "Awaiting domain ownership verification: publish a {DNS_CHALLENGE_RECORD_TYPE} \
                 record named {} whose value is the verification token",
                self.dns_record_name()
            ),
            VERIFICATION_METHOD_HTTP_ROUTE => format!(
                "Awaiting domain ownership verification: serve the verification token at \
                 https://{}{}",
                self.domain_name,
                self.http_route_path()
            ),
            // The remaining enum member, `PRIVATE_VPC`, proves ownership by IP from
            // inside the VPC that reaches the domain, so there is nothing to publish
            // on the domain itself. No other value reaches a stored record:
            // `verification_method_violation` refuses anything outside the enum before
            // the record is created.
            _ => "Awaiting domain ownership verification from the private VPC that reaches \
                  this domain"
                .to_string(),
        }
    }

    /// The challenge this record's verification method requires, as the modelled
    /// [`VerificationDetails`] shape.
    ///
    /// Exactly one of the per-method members is present, and only for a method that
    /// has a challenge to publish: `PRIVATE_VPC` is verified by IP from the VPC, so it
    /// carries the method alone.
    pub fn verification_details(&self) -> VerificationDetails {
        let mut details = VerificationDetails {
            method: Some(self.verification_method.clone()),
            dns_txt: None,
            http_route: None,
        };
        match self.verification_method.as_str() {
            VERIFICATION_METHOD_DNS_TXT => {
                details.dns_txt = Some(DnsVerification {
                    token: Some(self.verification_token.clone()),
                    dns_record_name: Some(self.dns_record_name()),
                    dns_record_type: Some(DNS_CHALLENGE_RECORD_TYPE.to_string()),
                })
            }
            VERIFICATION_METHOD_HTTP_ROUTE => {
                details.http_route = Some(HttpVerification {
                    token: Some(self.verification_token.clone()),
                    route_path: Some(self.http_route_path()),
                })
            }
            // `PRIVATE_VPC` (the only other enum member a stored record can carry)
            // publishes no challenge, so it keeps the method alone.
            _ => {}
        }
        details
    }

    /// This target domain projected onto the modelled [`TargetDomain`] shape the reads
    /// return: the stored record's own fields, never request input.
    pub fn modelled(&self) -> TargetDomain {
        TargetDomain {
            target_domain_id: self.target_domain_id.clone(),
            domain_name: self.domain_name.clone(),
            verification_status: Some(self.verification_status.clone()),
            verification_status_reason: self.verification_status_reason.clone(),
            verification_details: Some(self.verification_details()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            verified_at: self.verified_at.map(::aws_smithy_types::DateTime::from),
        }
    }
}

/// The identity minted for a new private connection: the VPC Lattice resources the
/// service creates for it, which are the service's to choose rather than the caller's
/// to supply.
#[derive(Debug, Clone)]
pub struct PrivateConnectionIdentity {
    /// The resource gateway id, in VPC Lattice's own `rgw-<17 of [0-9a-z]>` form.
    pub resource_gateway_id: String,
    /// The resource configuration id, in VPC Lattice's own `rcfg-<17 of [0-9a-z]>` form.
    pub resource_configuration_id: String,
}

/// The identity minted for a new target domain: the two values that are the service's
/// to choose rather than the caller's to supply.
#[derive(Debug, Clone)]
pub struct TargetDomainIdentity {
    pub target_domain_id: String,
    pub verification_token: String,
}

/// Per-service state for com-amazonaws-securityagent-securityagent. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSecurityagentSecurityagentState {
    /// Every stored application, keyed by its `applicationId`. A `BTreeMap` so
    /// `ListApplications` walks the applications in one stable order and a cursor
    /// taken on one page resumes at the right place on the next — including after a
    /// neighbouring record was deleted.
    pub applications: BTreeMap<String, Application>,
    /// Monotonic counter behind every minted `applicationId`, so two
    /// `CreateApplication` calls are two distinct records.
    pub next_application_seq: u64,
    /// Every stored private connection, keyed by its name — which the API itself makes
    /// the identifier ("A unique name for the private connection within your
    /// account"). A `BTreeMap` for the same reason as `applications`.
    pub private_connections: BTreeMap<String, PrivateConnection>,
    /// Monotonic counter behind the VPC Lattice ids minted per private connection, so
    /// two creates are two distinct gateways and configurations.
    pub next_private_connection_seq: u64,
    /// Every registered target domain, keyed by its `targetDomainId`. A `BTreeMap` for
    /// the same reason as [`Self::applications`]: one stable order for the reads that
    /// walk the whole set.
    pub target_domains: BTreeMap<String, TargetDomainRecord>,
    /// Monotonic counter behind every minted `targetDomainId`, so two
    /// `CreateTargetDomain` calls are two distinct records.
    pub next_target_domain_seq: u64,
    /// Every stored agent space, keyed by its `agentSpaceId`. A `BTreeMap` for the
    /// same reason as [`Self::applications`]: one stable order for a list read to walk
    /// and to resume a cursor in.
    pub agent_spaces: BTreeMap<String, AgentSpaceRecord>,
    /// Monotonic counter behind every minted `agentSpaceId`, so two `CreateAgentSpace`
    /// calls are two distinct records whatever they were called with.
    pub next_agent_space_seq: u64,
    /// Every stored security requirement pack, keyed by its `packId`. A `BTreeMap` for
    /// the same reason as [`Self::applications`]: one stable order for
    /// `ListSecurityRequirementPacks` to walk and to resume a cursor in.
    pub security_requirement_packs: BTreeMap<String, SecurityRequirementPack>,
    /// Monotonic counter behind every minted `packId`, so two
    /// `CreateSecurityRequirementPack` calls are two distinct records even when they
    /// carry byte-identical input.
    pub next_security_requirement_pack_seq: u64,
}

impl ComAmazonawsSecurityagentSecurityagentState {
    /// Mint the identity of a new application.
    ///
    /// The id is 32 lowercase hex characters drawn from the monotonic counter, so it
    /// is unique within the process and `ListApplications`' ordering is the creation
    /// order. The name and the domain are derived from that same identity, so a later
    /// read returns a real stored value rather than a placeholder, and the domain
    /// names the region the request was signed for.
    pub fn mint_application_identity(&mut self, region: &str) -> ApplicationIdentity {
        self.next_application_seq = self.next_application_seq.wrapping_add(1);
        let sequence = self.next_application_seq;
        let application_id = format!("{sequence:032x}");
        ApplicationIdentity {
            application_name: format!("application-{sequence}"),
            domain: format!("{application_id}.securityagent.{region}.on.aws"),
            application_id,
            sequence,
        }
    }

    /// Mint the VPC Lattice resources a service-managed private connection is created
    /// with: the resource gateway the service stands up in the caller's VPC, and the
    /// resource configuration it creates beside it.
    ///
    /// Both ids are drawn from the monotonic counter — which advances once per id, so
    /// the gateway and the configuration are two distinct resources — and rendered in
    /// the base-36 form VPC Lattice's own id patterns declare.
    pub fn mint_private_connection_identity(&mut self) -> PrivateConnectionIdentity {
        let gateway_sequence = self.next_private_connection_seq.wrapping_add(1);
        let configuration_sequence = self.next_private_connection_seq.wrapping_add(2);
        self.next_private_connection_seq = configuration_sequence;
        PrivateConnectionIdentity {
            resource_gateway_id: format!(
                "{RESOURCE_GATEWAY_ID_PREFIX}{}",
                base36_id(u128::from(gateway_sequence))
            ),
            resource_configuration_id: format!(
                "{RESOURCE_CONFIGURATION_ID_PREFIX}{}",
                base36_id(u128::from(configuration_sequence))
            ),
        }
    }

    /// The VPC Lattice resource configuration `identifier` names in `region`, as the
    /// account holds it (its bare id), or `None` when the account holds none under that
    /// identifier — which a create answers with `ResourceNotFoundException`.
    ///
    /// `SelfManagedInput$resourceConfigurationId` is "The identifier or ARN of the
    /// resource configuration", so both forms resolve: the bare id, and the ARN
    /// `arn:aws:vpc-lattice:<region>:<account>:resourceconfiguration/<id>`
    /// (`com.amazonaws.vpclattice#ResourceConfigurationArn`). Rather than taking an ARN
    /// apart, each id the account holds is rendered as the ARN it has through
    /// [`lattice_arn`] — the one ARN BUILDER this emulator mints resources with — and
    /// the identifier has to equal one of the two forms. So an ARN naming another
    /// partition, vendor, Region, account or resource type simply is not an ARN of any
    /// resource configuration this request can reach, with no second ARN grammar
    /// written down to disagree with the builder's.
    ///
    /// The resource configurations the emulated account holds are the ones it CREATED:
    /// one per service-managed private connection, whose gateway and configuration are
    /// the service's to create and are persisted on the record. VPC Lattice's own
    /// `CreateResourceConfiguration` still answers 501 and keeps no store, so it
    /// contributes none yet; when it is implemented, THIS is the one place that has to
    /// start consulting that store beside these records.
    pub fn resolve_resource_configuration(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<String> {
        self.private_connections
            .values()
            .filter_map(|connection| connection.resource_configuration_id.as_deref())
            .find(|id| {
                identifier == *id
                    || identifier
                        == lattice_arn(region, KIND_RESOURCE_CONFIGURATION, id)
            })
            .map(str::to_string)
    }

    /// Mint the identity of a new target domain.
    ///
    /// The id is 32 lowercase hex characters drawn from the monotonic counter, so it is
    /// unique within the process and the reads' ordering is the registration order. The
    /// The token is derived from that id alone — never from request text — so it is
    /// unique per record and carries nothing the caller sent. It is DETERMINISTIC by
    /// design (a hash of the record's ordinal, so a test can recompute it); it is NOT
    /// an unpredictable secret the way the real service's challenge token is.
    pub fn mint_target_domain_identity(&mut self) -> TargetDomainIdentity {
        self.next_target_domain_seq = self.next_target_domain_seq.wrapping_add(1);
        let target_domain_id = format!("{:032x}", self.next_target_domain_seq);
        TargetDomainIdentity {
            verification_token: verification_token(&target_domain_id),
            target_domain_id,
        }
    }

    /// Mint the `agentSpaceId` of a new agent space.
    ///
    /// Opaque to the caller — `AgentSpaceId` carries neither a `@pattern` nor a
    /// `@length` in the model — and drawn from a monotonic counter, so it is unique
    /// within the process and the stored order is the creation order. 32 lowercase hex
    /// characters, the same form as a minted `applicationId`.
    pub fn mint_agent_space_id(&mut self) -> String {
        self.next_agent_space_seq = self.next_agent_space_seq.wrapping_add(1);
        format!("{:032x}", self.next_agent_space_seq)
    }

    /// Mint the `packId` of a new security requirement pack.
    ///
    /// Drawn from the store's own monotonic counter while the store is locked, so two
    /// calls — including two concurrent calls carrying the same `name` — get two
    /// distinct ids and neither overwrites the other. The id is 32 lowercase hex
    /// characters, the same opaque form as an `applicationId`: `SecurityRequirementPackId`
    /// carries no `@pattern` or `@length` in the model, so the shape is this
    /// emulator's to choose, and taking it from the counter makes the map's order the
    /// creation order.
    pub fn mint_security_requirement_pack_id(&mut self) -> String {
        self.next_security_requirement_pack_seq =
            self.next_security_requirement_pack_seq.wrapping_add(1);
        format!("{:032x}", self.next_security_requirement_pack_seq)
    }
}

/// `sequence` as the `<17 of [0-9a-z]>` suffix VPC Lattice's resource-gateway and
/// resource-configuration id patterns declare: base 36, lowercase, zero-padded to the
/// pattern's exact length.
fn base36_id(mut value: u128) -> String {
    let mut digits = [b'0'; LATTICE_ID_SUFFIX_LEN];
    for slot in digits.iter_mut().rev() {
        *slot = BASE36_LOWER[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8_lossy(&digits).into_owned()
}

/// The ARN a VPC Lattice resource of `kind` and id `id` has in `region`, built with the
/// ONE ARN builder this emulator mints resources with (`engine::emulator_arn`, which
/// carries the emulator's own partition and account). The form matches
/// `com.amazonaws.vpclattice#ResourceConfigurationArn` /
/// `#ResourceGatewayArn`: `arn:aws:vpc-lattice:<region>:<account>:<kind>/<id>`.
pub fn lattice_arn(region: &str, kind: &str, id: &str) -> String {
    ::engine::emulator_arn(VPC_LATTICE_ARN_SERVICE, region, &format!("{kind}/{id}"))
}

// ---------------------------------------------------------------------------
// The account's network
// ---------------------------------------------------------------------------

/// How many hex characters an EC2 subnet identifier carries in its long form
/// (`subnet-0bb1c79de3example`), which is the form [`region_subnets`] mints.
const SUBNET_ID_HEX_LEN: usize = 17;

/// How many digest BYTES go into an EC2 VPC identifier's short form (`vpc-1a2b3c4d`):
/// two hex characters each, so four make the 8 the form carries.
const VPC_ID_DIGEST_BYTES: usize = 4;

/// The Availability Zone suffixes a Region's default subnets sit in. A Region has at
/// least three Availability Zones, and a default VPC has one default subnet in each.
const AVAILABILITY_ZONE_SUFFIXES: [&str; 3] = ["a", "b", "c"];

/// A subnet of the account's network in one Region: the identifier a request names, the
/// VPC it belongs to and the Availability Zone it sits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubnetRecord {
    pub subnet_id: String,
    pub vpc_id: String,
    pub availability_zone: String,
}

/// The id of the account's default VPC in `region`.
///
/// AWS gives an account a default VPC in every Region and this emulator models no
/// other, so the id is DERIVED from the Region name — stable, so every read reports the
/// same VPC — in EC2's short `vpc-<8 hex>` form.
pub fn default_vpc_id(region: &str) -> String {
    format!(
        "vpc-{}",
        hex::encode(&Sha256::digest(region.as_bytes())[..VPC_ID_DIGEST_BYTES])
    )
}

/// The subnets that exist in `region`: the default VPC's default subnet in each
/// Availability Zone of that Region.
///
/// THE SAME derivation, constant for constant, as the one
/// `services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/state.rs` and
/// `services/service-com-amazonaws-dax-amazondaxv3/src/state.rs` apply, all three
/// written against the contract `services/common/src/network.rs` states, so no two
/// services answer "does this subnet exist" differently and a client that places an FSx
/// file system and a private connection's resource gateway in "the same" subnet is
/// talking to one account. The derivation itself moves into that shared module once one
/// change may carry every such service crate's manifest; keep them in step until then,
/// and when EC2's subnet operations stop answering 501, [`resolve_subnet`] is the one
/// place here that has to start consulting the stored subnets beside these derived ones.
pub fn region_subnets(region: &str) -> Vec<SubnetRecord> {
    let vpc_id = default_vpc_id(region);
    AVAILABILITY_ZONE_SUFFIXES
        .iter()
        .map(|suffix| {
            let availability_zone = format!("{region}{suffix}");
            let digest = hex::encode(Sha256::digest(availability_zone.as_bytes()));
            SubnetRecord {
                subnet_id: format!("subnet-{}", &digest[..SUBNET_ID_HEX_LEN]),
                vpc_id: vpc_id.clone(),
                availability_zone,
            }
        })
        .collect()
}

/// The subnet `subnet_id` names in `region`, or `None` when no subnet of that Region
/// carries that identifier — which is what lets a request naming it be refused instead
/// of stored.
pub fn resolve_subnet(region: &str, subnet_id: &str) -> Option<SubnetRecord> {
    region_subnets(region)
        .into_iter()
        .find(|subnet| subnet.subnet_id == subnet_id)
}

/// The verification token the owner of a target domain publishes: the SHA-256 of the
/// service-minted `targetDomainId` under a domain-separation prefix, hex-encoded.
///
/// Derived from the minted id rather than from the request, so it carries no caller
/// text, is unique per record and is the same on every projection of that record.
/// Because that id is the record's ordinal in this process, the token is a
/// deterministic, test-reproducible value — emulator-grade, not an unpredictable
/// secret: nothing here should be read as modelling the real challenge token's
/// unforgeability.
fn verification_token(target_domain_id: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(
        format!("{VERIFICATION_TOKEN_CONTEXT}{target_domain_id}").as_bytes(),
    );
    hex::encode(digest)
}

/// Why `name` is not a domain name this service can register as a target domain, or
/// `None` when it is one.
///
/// `targetDomainName` is a bare `String` in the model — no `@length`, no `@pattern` —
/// so this rule is a CHOSEN refusal of input the model admits: what the operation
/// registers is "a web domain", i.e. a DNS name, and a value that cannot name a domain
/// could never be verified, so it is refused at the door instead of being stored.
///
/// A name is one when, ignoring the optional single trailing dot of the rooted form,
/// it is at most [`MAX_DOMAIN_NAME_LEN`] bytes and made of non-empty dot-separated
/// labels of at most [`MAX_DOMAIN_LABEL_LEN`] bytes drawn from letters, digits and
/// hyphens, none of them starting or ending with a hyphen (RFC 1035 §2.3.1/§2.3.4 with
/// RFC 1123 §2.1, which is also why an underscore label — legal in a DNS zone but not
/// in a hostname — is not a web domain). Both bounds are measured in bytes, which is
/// the unit the RFCs state them in and, for a name this rule accepts, the same as
/// characters. The returned text is the constraint the value missed, for the refusal's
/// `fieldList` entry.
pub fn target_domain_name_violation(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("must not be empty".to_string());
    }
    // The rooted form `example.com.` names the same domain as `example.com`, so the
    // one trailing dot is not part of the name being checked.
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.len() > MAX_DOMAIN_NAME_LEN {
        return Some(format!(
            "must have length less than or equal to {MAX_DOMAIN_NAME_LEN}"
        ));
    }
    for label in name.split('.') {
        if label.is_empty() {
            return Some("must not carry an empty label".to_string());
        }
        if !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Some(
                "must be a domain name built from letters, digits, hyphens and dots".to_string(),
            );
        }
        if label.len() > MAX_DOMAIN_LABEL_LEN {
            return Some(format!(
                "must carry labels of at most {MAX_DOMAIN_LABEL_LEN} characters"
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Some("must not carry a label that starts or ends with a hyphen".to_string());
        }
    }
    None
}

/// Why `method` is not a `DomainVerificationMethod` this service can act on, or `None`
/// when it is one of [`VERIFICATION_METHOD_VALUES`].
///
/// The model declares the member as an enum, so a conforming client never sends
/// anything else; checking it here means an unmodelled value is refused rather than
/// stored and projected back as a method with no challenge to publish.
pub fn verification_method_violation(method: &str) -> Option<String> {
    (!VERIFICATION_METHOD_VALUES.contains(&method)).then(|| {
        format!(
            "must satisfy enum value set: [{}]",
            VERIFICATION_METHOD_VALUES.join(", ")
        )
    })
}

/// The ARN of the IAM Identity Center application this emulator stands up for an
/// application whose `idcInstanceArn` was supplied.
///
/// The instance segment is taken from the supplied instance ARN's own resource id
/// (`arn:aws:sso:::instance/<instance-id>`) so the two ARNs agree; an instance ARN
/// that names no usable id falls back to [`FALLBACK_IDC_INSTANCE_ID`]. The account
/// segment is the emulator's canonical account via `engine::emulator_arn`, and
/// `sso` ARNs carry no region, so the region passed is empty.
pub fn idc_application_arn(idc_instance_arn: &str, sequence: u64) -> String {
    let instance_id = idc_instance_arn
        .rsplit_once('/')
        .map(|(_, segment)| {
            segment
                .chars()
                .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
                .take(64)
                .collect::<String>()
        })
        .filter(|segment| !segment.is_empty())
        .unwrap_or_else(|| FALLBACK_IDC_INSTANCE_ID.to_string());
    ::engine::emulator_arn(
        "sso",
        "",
        &format!("application/{instance_id}/apl-{sequence:016x}"),
    )
}

/// Encode the record id a page resumes at as an opaque `nextToken` under `prefix`.
///
/// The cursor names the record to resume AT, not an offset, so deleting a record
/// between two pages cannot make the next page skip or repeat one.
fn encode_cursor(prefix: &str, record_id: &str) -> String {
    format!("{prefix}{}", hex::encode(record_id))
}

/// The record id a `nextToken` minted under `prefix` resumes at, or `None` when the
/// token is not one this service minted for that list.
fn decode_cursor(prefix: &str, token: &str) -> Option<String> {
    let body = token.strip_prefix(prefix)?;
    let bytes = hex::decode(body).ok()?;
    let record_id = String::from_utf8(bytes).ok()?;
    (!record_id.is_empty()).then_some(record_id)
}

/// Encode the `applicationId` a `ListApplications` page resumes at as an opaque
/// `nextToken`.
pub fn encode_page_token(application_id: &str) -> String {
    encode_cursor(PAGE_TOKEN_PREFIX, application_id)
}

/// The `applicationId` a `nextToken` resumes at, or `None` when the token is not one
/// this service minted (the caller answers that with a `ValidationException`).
pub fn decode_page_token(token: &str) -> Option<String> {
    decode_cursor(PAGE_TOKEN_PREFIX, token)
}

/// Encode the `packId` a `ListSecurityRequirementPacks` page resumes at as an opaque
/// `nextToken`.
pub fn encode_pack_page_token(pack_id: &str) -> String {
    encode_cursor(PACK_PAGE_TOKEN_PREFIX, pack_id)
}

/// The `packId` a `ListSecurityRequirementPacks` `nextToken` resumes at, or `None`
/// when the token is not one this service minted for that list (the caller answers
/// that with a `ValidationException`).
pub fn decode_pack_page_token(token: &str) -> Option<String> {
    decode_cursor(PACK_PAGE_TOKEN_PREFIX, token)
}

/// Encode the private connection name a `ListPrivateConnections` page resumes at as an
/// opaque `nextToken`, on [`encode_page_token`]'s resume-AT rule.
pub fn encode_connection_page_token(name: &str) -> String {
    encode_cursor(PRIVATE_CONNECTION_PAGE_TOKEN_PREFIX, name)
}

/// The private connection name a `ListPrivateConnections` `nextToken` resumes at, or
/// `None` when the token is not one this listing minted.
pub fn decode_connection_page_token(token: &str) -> Option<String> {
    decode_cursor(PRIVATE_CONNECTION_PAGE_TOKEN_PREFIX, token)
}

/// The region this request was signed for, from its SigV4 credential scope;
/// [`DEFAULT_REGION`] when it carries none.
///
/// The scope is caller-controlled text and the value lands inside a derived domain
/// name, so anything that is not a plausible region label is refused here and the
/// default used instead: a generated domain can never carry arbitrary request text.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSecurityagentSecurityagentState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSecurityagentSecurityagentState>)
}
