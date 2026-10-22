// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-securityagent-securityagent service.
//!
//! The Application records every application operation reads and writes live here, so
//! `CreateApplication`, `GetApplication`, `ListApplications`, `UpdateApplication` and
//! `DeleteApplication` all see one copy of each resource. The helpers below (identity
//! minting, the IdC application ARN, the modelled projections and the pagination cursor
//! codec) are shared by those handlers rather than duplicated per operation.
//!
//! The agent space records sit beside them in the same struct, so `CreateAgentSpace`
//! and `BatchGetAgentSpaces` — and every agent space operation implemented after them —
//! read ONE copy of each agent space.
//!
//! The security requirement packs live here for the same reason:
//! `CreateSecurityRequirementPack` and `GetSecurityRequirementPack` — and every pack
//! operation implemented after them — read and write ONE copy of each pack record.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    AWSResources, AgentSpace, ApplicationSummary, CodeReviewSettings, IdCConfiguration,
};

/// The region every value this service derives from the request's region carries when
/// the request arrived without a SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many `ApplicationSummary` entries one `ListApplications` page holds when the
/// caller asks for no page size.
///
/// CHOSEN, not cited: `MaxResults` carries neither `@default` nor `@range` in the
/// model, so nothing pins this value. It only bounds the size of one response.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// The most `ApplicationSummary` entries one `ListApplications` page can hold,
/// whatever the caller asks for.
///
/// CHOSEN, not cited, for the same reason as [`DEFAULT_MAX_RESULTS`]: `MaxResults`
/// carries no `@range`, so nothing is refused at the boundary and a caller could
/// otherwise ask for the whole store in one response. A larger request is clamped to
/// this ceiling and the page simply carries a `nextToken`, so no application becomes
/// unreachable — only the work and the response size one call can demand are bounded.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most applications this emulator stores for the account before
/// `CreateApplication` answers the service's `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited: the real service's application quota is not in the model. It is
/// set far above any plausible client's application count, so normal use never meets
/// it, while the store a caller can grow stays bounded.
pub const MAX_APPLICATIONS: usize = 1_000;

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

/// Encode the `applicationId` a `ListApplications` page resumes at as an opaque
/// `nextToken`.
///
/// The cursor names the record to resume AT, not an offset, so deleting a record
/// between two pages cannot make the next page skip or repeat an application.
pub fn encode_page_token(application_id: &str) -> String {
    format!("{PAGE_TOKEN_PREFIX}{}", hex::encode(application_id))
}

/// The `applicationId` a `nextToken` resumes at, or `None` when the token is not one
/// this service minted (the caller answers that with a `ValidationException`).
pub fn decode_page_token(token: &str) -> Option<String> {
    let body = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let bytes = hex::decode(body).ok()?;
    let application_id = String::from_utf8(bytes).ok()?;
    (!application_id.is_empty()).then_some(application_id)
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
