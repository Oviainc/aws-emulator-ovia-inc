// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-auditmanager-bedrockassessmentmanagerlambda service.
//!
//! This is the one store every handler of this service reads and writes, plus the
//! projections and helpers they share: the assessment record, the account's
//! standard-framework catalog, the request's region, and the opaque page-token
//! encoding every `@paginated` list in this service uses.

use std::collections::HashMap;

use crate::ops::types::{
    AWSAccount, Assessment, AssessmentFramework, AssessmentFrameworkMetadata, AssessmentMetadata,
    AssessmentMetadataItem, AssessmentReportsDestination, Delegation, FrameworkMetadata, Role,
    Scope,
};

/// The region an ARN carries when the request names none (no SigV4 credential
/// scope to read it from). Matches `emulator_arn`'s documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `auditmanager` ARN namespace, as the model's `aws.api#service` declares it.
pub const ARN_SERVICE: &str = "auditmanager";

/// `AssessmentStatus` as the model enumerates it (`ACTIVE`, `INACTIVE`).
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// `RoleType` value the model's `Role$roleType` doc permits in `CreateAssessment`:
/// "In `CreateAssessment`, `roleType` can only be `PROCESS_OWNER`."
pub const ROLE_TYPE_PROCESS_OWNER: &str = "PROCESS_OWNER";

/// `FrameworkType` as the model enumerates it (`Standard`, `Custom`).
pub const FRAMEWORK_TYPE_STANDARD: &str = "Standard";

/// The largest page any list in this service returns, which is the upper bound of
/// the model's `MaxResults` range (`range(min: 1, max: 1000)`). It is also the
/// page size used when a request names no `maxResults`.
pub const MAX_PAGE_SIZE: i32 = 1000;

/// How many assessments the store holds before `CreateAssessment` refuses with
/// the modelled `ServiceQuotaExceededException`.
///
/// Audit Manager publishes an adjustable per-account assessment quota, and this
/// emulator models neither Service Quotas nor an account's adjustments — so this
/// is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It exists because the store lives for the life of the process
/// and every create request may add a caller-sized record to it. Every record it
/// holds is released by `DeleteAssessment`, and it sits far above any workflow an
/// emulator serves.
pub const ASSESSMENT_QUOTA: usize = 1_000;

/// One entry of the account's standard-framework catalog.
#[derive(Debug, Clone, Copy)]
pub struct StandardFramework {
    /// The framework's identifier, which satisfies the model's `UUID` pattern
    /// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
    pub id: &'static str,
    /// The framework's name.
    pub name: &'static str,
    /// The compliance standard the framework covers, which an assessment created
    /// from it reports as `AssessmentMetadata$complianceType`.
    pub compliance_type: &'static str,
}

/// The frameworks Audit Manager provides to a registered account, which
/// `CreateAssessment` resolves `frameworkId` against and
/// `ListAssessmentFrameworks` publishes under `frameworkType=Standard`.
///
/// Source: the framework names and compliance standards in the "Supported
/// frameworks in Audit Manager" list of the AWS Audit Manager User Guide. AWS
/// mints its own identifier for each one per account, so no published identifier
/// exists to copy; this emulator seeds a stable identifier per framework instead,
/// which is what `ListAssessmentFrameworks` hands a client to create an
/// assessment from.
pub const STANDARD_FRAMEWORKS: &[StandardFramework] = &[
    StandardFramework {
        id: "1b2c3d4e-5f6a-4b7c-8d9e-1f2a3b4c5d6e",
        name: "AWS Control Tower Guardrails",
        compliance_type: "AWS Control Tower",
    },
    StandardFramework {
        id: "2c3d4e5f-6a7b-4c8d-8e9f-2a3b4c5d6e7f",
        name: "AWS Foundational Security Best Practices",
        compliance_type: "AWS Foundational Security Best Practices",
    },
    StandardFramework {
        id: "3d4e5f6a-7b8c-4d9e-8f0a-3b4c5d6e7f80",
        name: "CIS AWS Foundations Benchmark v1.4.0 Level 1",
        compliance_type: "CIS",
    },
    StandardFramework {
        id: "4e5f6a7b-8c9d-4e0f-8a1b-4c5d6e7f8091",
        name: "FedRAMP Moderate Baseline",
        compliance_type: "FedRAMP",
    },
    StandardFramework {
        id: "5f6a7b8c-9d0e-4f1a-8b2c-5d6e7f8091a2",
        name: "General Data Protection Regulation",
        compliance_type: "GDPR",
    },
    StandardFramework {
        id: "6a7b8c9d-0e1f-4a2b-8c3d-6e7f8091a2b3",
        name: "HIPAA Security Rule: Feb 2003",
        compliance_type: "HIPAA",
    },
    StandardFramework {
        id: "7b8c9d0e-1f2a-4b3c-8d4e-7f8091a2b3c4",
        name: "NIST SP 800-53 Rev 5",
        compliance_type: "NIST",
    },
    StandardFramework {
        id: "8c9d0e1f-2a3b-4c4d-8e5f-8091a2b3c4d5",
        name: "PCI DSS V3.2.1",
        compliance_type: "PCI DSS",
    },
    StandardFramework {
        id: "9d0e1f2a-3b4c-4d5e-8f6a-91a2b3c4d5e6",
        name: "SOC 2",
        compliance_type: "SOC",
    },
];

/// A framework held by this account: a catalog entry plus the instants the
/// account acquired it.
#[derive(Debug, Clone)]
pub struct FrameworkRecord {
    pub id: String,
    pub name: String,
    /// `FrameworkType`: `Standard` for a framework Audit Manager provides,
    /// `Custom` for one the account created.
    pub framework_type: String,
    pub compliance_type: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub last_updated_at: ::aws_smithy_types::DateTime,
}

/// An assessment's ARN. The model constrains `Assessment$arn` to
/// `^arn:.*:auditmanager:`.
pub fn assessment_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("assessment/{id}"))
}

/// A framework's ARN. The model constrains `AssessmentFramework$arn` to
/// `^arn:.*:auditmanager:`.
pub fn framework_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("assessmentFramework/{id}"))
}

impl FrameworkRecord {
    /// This framework's ARN, in the region the reading request addressed.
    pub fn arn(&self, region: &str) -> String {
        framework_arn(region, &self.id)
    }

    /// The `AssessmentFrameworkMetadata` projection `ListAssessmentFrameworks`
    /// returns. The control counts are zero because this service holds no
    /// controls yet (control CRUD is still scaffolded), not as a placeholder.
    pub fn to_framework_metadata(&self, region: &str) -> AssessmentFrameworkMetadata {
        AssessmentFrameworkMetadata {
            arn: Some(self.arn(region)),
            id: Some(self.id.clone()),
            r#type: Some(self.framework_type.clone()),
            name: Some(self.name.clone()),
            description: None,
            logo: None,
            compliance_type: Some(self.compliance_type.clone()),
            controls_count: 0,
            control_sets_count: 0,
            created_at: Some(self.created_at),
            last_updated_at: Some(self.last_updated_at),
        }
    }

    /// This framework's position in the stable `ListAssessmentFrameworks`
    /// ordering: name first, then id.
    ///
    /// The two parts are joined with NUL, which orders below every other byte, so
    /// comparing two keys byte-wise orders them exactly as comparing the
    /// `(name, id)` pairs does — which a printable separator would not, since a
    /// name may contain characters below it.
    pub fn page_key(&self) -> String {
        format!("{}\u{0}{}", self.name, self.id)
    }
}

/// An assessment, as this service stores it. Every assessment-shaped response is
/// a projection of this record, never of a request.
#[derive(Debug, Clone)]
pub struct AssessmentRecord {
    pub id: String,
    pub arn: String,
    /// The region the creating request addressed, so every ARN this record
    /// renders stays the one minted at creation.
    pub region: String,
    pub name: String,
    pub description: Option<String>,
    /// Copied from the framework the assessment was created from.
    pub compliance_type: String,
    /// `AssessmentStatus`: `ACTIVE` or `INACTIVE`.
    pub status: String,
    pub assessment_reports_destination: AssessmentReportsDestination,
    pub scope: Scope,
    pub roles: Vec<Role>,
    pub delegations: Vec<Delegation>,
    pub framework_id: String,
    pub tags: HashMap<String, String>,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub last_updated: ::aws_smithy_types::DateTime,
}

impl AssessmentRecord {
    /// The full `Assessment` projection, with the framework it was created from
    /// resolved from the same store.
    ///
    /// The assessment's own ARN is the one minted in the region it was created
    /// in, because an assessment belongs to that region. The framework's ARN is
    /// rendered in `region`, the region the READING request addressed, because a
    /// framework is an account-wide catalog entry — the same rule
    /// `ListAssessmentFrameworks` reports it under, so one framework never
    /// surfaces two ARNs.
    pub fn to_assessment(
        &self,
        framework: Option<&FrameworkRecord>,
        region: &str,
    ) -> Assessment {
        Assessment {
            arn: Some(self.arn.clone()),
            aws_account: Some(AWSAccount {
                id: Some(::engine::emulator_account_id().to_string()),
                email_address: None,
                name: None,
            }),
            metadata: Some(AssessmentMetadata {
                name: Some(self.name.clone()),
                id: Some(self.id.clone()),
                description: self.description.clone(),
                compliance_type: Some(self.compliance_type.clone()),
                status: Some(self.status.clone()),
                assessment_reports_destination: Some(self.assessment_reports_destination.clone()),
                scope: Some(self.scope.clone()),
                roles: Some(self.roles.clone()),
                delegations: Some(self.delegations.clone()),
                creation_time: Some(self.creation_time),
                last_updated: Some(self.last_updated),
            }),
            framework: Some(AssessmentFramework {
                id: Some(self.framework_id.clone()),
                arn: Some(framework_arn(region, &self.framework_id)),
                metadata: framework.map(|f| FrameworkMetadata {
                    name: Some(f.name.clone()),
                    description: None,
                    logo: None,
                    compliance_type: Some(f.compliance_type.clone()),
                }),
                // No control sets: this service holds no controls yet, so there
                // are none to report for the framework.
                control_sets: Some(Vec::new()),
            }),
            tags: Some(self.tags.clone()),
        }
    }

    /// The `AssessmentMetadataItem` projection `ListAssessments` returns.
    pub fn to_metadata_item(&self) -> AssessmentMetadataItem {
        AssessmentMetadataItem {
            name: Some(self.name.clone()),
            id: Some(self.id.clone()),
            compliance_type: Some(self.compliance_type.clone()),
            status: Some(self.status.clone()),
            roles: Some(self.roles.clone()),
            delegations: Some(self.delegations.clone()),
            creation_time: Some(self.creation_time),
            last_updated: Some(self.last_updated),
        }
    }

    /// The role of the caller for this assessment: the owning persona the
    /// assessment records (`PROCESS_OWNER`), else whatever role it does record.
    pub fn user_role(&self) -> Option<Role> {
        self.roles
            .iter()
            .find(|r| r.role_type == ROLE_TYPE_PROCESS_OWNER)
            .or_else(|| self.roles.first())
            .cloned()
    }

    /// This record's position in the stable `ListAssessments` ordering: creation
    /// instant first, then id, rendered so a byte-wise comparison orders two keys
    /// the same way the pair does.
    pub fn page_key(&self) -> String {
        format!(
            "{:020}.{:09}#{}",
            self.creation_time.secs(),
            self.creation_time.subsec_nanos(),
            self.id
        )
    }
}

/// Per-service state for com-amazonaws-auditmanager-bedrockassessmentmanagerlambda. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsAuditmanagerBedrockassessmentmanagerlambdaState {
    /// Every assessment this account holds, keyed by its `UUID` identifier.
    pub assessments: HashMap<String, AssessmentRecord>,
    /// Every framework this account holds, keyed by its `UUID` identifier. Seeded
    /// with [`STANDARD_FRAMEWORKS`], the frameworks Audit Manager provides;
    /// custom frameworks join it when `CreateAssessmentFramework` is implemented.
    pub frameworks: HashMap<String, FrameworkRecord>,
    /// How many identifiers this store has minted. It is what makes every mint
    /// input distinct; see [`ComAmazonawsAuditmanagerBedrockassessmentmanagerlambdaState::next_uuid`].
    pub minted: u64,
}

impl Default for ComAmazonawsAuditmanagerBedrockassessmentmanagerlambdaState {
    fn default() -> Self {
        let now = now();
        Self {
            assessments: HashMap::new(),
            frameworks: STANDARD_FRAMEWORKS
                .iter()
                .map(|f| {
                    (
                        f.id.to_string(),
                        FrameworkRecord {
                            id: f.id.to_string(),
                            name: f.name.to_string(),
                            framework_type: FRAMEWORK_TYPE_STANDARD.to_string(),
                            compliance_type: f.compliance_type.to_string(),
                            created_at: now,
                            last_updated_at: now,
                        },
                    )
                })
                .collect(),
            minted: 0,
        }
    }
}

impl ComAmazonawsAuditmanagerBedrockassessmentmanagerlambdaState {
    /// A fresh identifier for a resource this service mints, in the lowercase-hex
    /// 8-4-4-4-12 form the model's `UUID` shape requires
    /// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), with
    /// the version (4) and variant bits RFC 4122 sets for that form.
    ///
    /// The sixteen bytes are the leading half of a SHA-256 digest (through the
    /// crate's `sha2` dependency, so nothing here hand-rolls a hash) over this
    /// store's own mint counter and the instant of the mint. The counter is
    /// incremented first and never repeats for the life of the store, so no two
    /// mints digest the same input and a repeated identifier is not reachable;
    /// `CreateAssessment` refuses an occupied slot rather than overwriting one,
    /// so even a digest collision cannot return another caller's assessment.
    pub fn next_uuid(&mut self, at: ::aws_smithy_types::DateTime) -> String {
        use ::sha2::Digest as _;
        self.minted = self.minted.wrapping_add(1);
        let mut digest = ::sha2::Sha256::new();
        digest.update(b"auditmanager/mint/");
        digest.update(self.minted.to_be_bytes());
        digest.update(at.secs().to_be_bytes());
        digest.update(at.subsec_nanos().to_be_bytes());
        let digest = digest.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        format_uuid(&bytes)
    }
}

/// Render sixteen bytes in the model's `UUID` form: lowercase hex in 8-4-4-4-12
/// groups.
fn format_uuid(bytes: &[u8; 16]) -> String {
    use ::std::fmt::Write as _;
    let mut rendered = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            rendered.push('-');
        }
        // Writing to a String cannot fail.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAuditmanagerBedrockassessmentmanagerlambdaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAuditmanagerBedrockassessmentmanagerlambdaState>)
}

/// The current instant, as the modelled `Timestamp` type.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
}

/// The region the request addressed, read from its SigV4 credential scope, or
/// [`DEFAULT_REGION`] when it carries none.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The scope an assessment stores for a requested one.
///
/// `Scope$awsServices` is modelled `@deprecated`: "If you use this parameter to
/// specify one or more Amazon Web Services services, Audit Manager ignores this
/// input. Instead, the value for `awsServices` will show as empty." So the
/// requested services are dropped and the stored list is empty.
pub fn stored_scope(requested: &Scope) -> Scope {
    Scope {
        aws_accounts: requested.aws_accounts.clone(),
        aws_services: Some(Vec::new()),
    }
}

/// A page token that does not decode to a cursor this service issued for the
/// listing that was asked for.
#[derive(Debug, Clone, Copy)]
pub struct InvalidPageToken;

/// One page of a list, plus the token that resumes after it.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// The marker every page token this service mints carries, before its listing
/// name and resume key. It is what makes a token verifiable: a string that merely
/// happens to be base64 is not a resume point, and a token one listing issued is
/// not a resume point for another.
const PAGE_TOKEN_PREFIX: &str = "auditmanager/v1/";

/// The listing name `ListAssessments` mints and accepts tokens for.
pub const LISTING_ASSESSMENTS: &str = "assessments";

/// The listing name `ListAssessmentFrameworks` mints and accepts tokens for.
pub const LISTING_ASSESSMENT_FRAMEWORKS: &str = "assessmentFrameworks";

/// Encode a listing's resume point as the opaque token the model's `Token` shape
/// allows (`^[A-Za-z0-9+/=]*$`, which base64 satisfies).
fn encode_page_token(listing: &str, cursor: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{PAGE_TOKEN_PREFIX}{listing}/{cursor}"))
}

/// The resume point a token carries, or `InvalidPageToken` when it was not minted
/// by `listing`.
fn decode_page_token(listing: &str, token: &str) -> Result<String, InvalidPageToken> {
    let bytes = ::aws_smithy_types::base64::decode(token).map_err(|_| InvalidPageToken)?;
    let decoded = String::from_utf8(bytes).map_err(|_| InvalidPageToken)?;
    decoded
        .strip_prefix(PAGE_TOKEN_PREFIX)
        .and_then(|rest| rest.strip_prefix(listing))
        .and_then(|rest| rest.strip_prefix('/'))
        .map(str::to_string)
        .ok_or(InvalidPageToken)
}

/// Cut one page out of `sorted`, a list of `(ordering key, item)` pairs sorted by
/// key ascending, for the listing named `listing`.
///
/// The token carries the key of the last item returned, so resuming returns every
/// item ordered after it: no item is served twice, and removing an item between
/// two pages does not shift the rest. The last page carries no token.
pub fn paginate<T>(
    listing: &str,
    sorted: Vec<(String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<T>, InvalidPageToken> {
    let start = match next_token {
        Some(token) => {
            let cursor = decode_page_token(listing, token)?;
            sorted
                .iter()
                .position(|(key, _)| key.as_str() > cursor.as_str())
                .unwrap_or(sorted.len())
        }
        None => 0,
    };
    let page_size = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let end = sorted.len().min(start.saturating_add(page_size));
    let next_token = if end < sorted.len() && end > start {
        Some(encode_page_token(listing, &sorted[end - 1].0))
    } else {
        None
    };
    let items = sorted
        .into_iter()
        .skip(start)
        .take(end - start)
        .map(|(_, item)| item)
        .collect();
    Ok(Page { items, next_token })
}
