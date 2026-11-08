// SPDX-License-Identifier: CC0-1.0

//! The external access the account's OWN resources grant: the one analysis every
//! external-access finding of this service is derived from.
//!
//! An external access analyzer monitors the resources of the account it was created in and
//! reports, for each of them, the access that resource's RESOURCE-BASED policy grants to a
//! principal outside the account. So a finding is not something a caller creates: it is a
//! projection of state another service already holds — a role's trust policy, a bucket's
//! bucket policy — and of the account the analyzer's own ARN names.
//!
//! Nothing of the analysis is stored here. The policies are read from the service that
//! OWNS each resource on every read ([`monitored_resources`]), so there is no second copy
//! of a policy to drift from the one that service reports, and the findings are derived
//! from them on every read ([`external_access_findings`]) — which is what makes a reporting
//! operation write nothing at all and two reads of one analyzer report the same findings
//! with the same ids: the id is DERIVED from what it identifies ([`super::derived_uuid`])
//! rather than minted.
//!
//! ## What is analyzed
//!
//! A resource is monitored when this emulator holds the resource-based policy that decides
//! who outside the account may reach it, and the account that owns it is the analyzer's
//! own. Each provider below reads one owning service's store:
//!
//! * `AWS::IAM::Role` — the role's trust policy, which is the resource-based policy of a
//!   role (it names the principals that may assume it);
//! * `AWS::S3::Bucket` — the bucket policy.
//!
//! A resource type whose policy no service of this emulator holds yet contributes nothing:
//! a finding is reported only for a policy that was actually read, never for a resource
//! type this emulator could merely have one for.
//!
//! ## Which grants are reported
//!
//! The rule is the one [`external_grants`] states, shared with the access-preview analysis
//! so the two cannot drift: one finding per principal OUTSIDE the zone of trust that an
//! `Allow` statement grants access to, where the zone of trust of an account analyzer is
//! the account that owns the resource. The two analyses differ in ONE answer, each
//! documented at its call site: an Amazon Web Services service principal is inside the zone
//! of trust of a deployed resource's analysis ([`ServicePrincipals`]).

use std::borrow::Cow;
use std::collections::HashMap;
use std::time::SystemTime;

use super::{derived_uuid, resource_owner_account};
use crate::ops::check_no_new_access::policy;
use crate::ops::types::{FindingSource, FindingSummary};

/// The `FindingStatus` a reported external-access finding carries: `ACTIVE`, the status of
/// a finding whose access the resource's policy still grants and which no archive rule has
/// archived. One of the model's [`crate::ops::types::FINDING_STATUS_VALUES`].
///
/// `ARCHIVED` is what an archive rule produces (`ApplyArchiveRule` is itself still
/// scaffolded, with an issue of its own) and `RESOLVED` what a finding whose access was
/// removed carries; the analysis holds no record of an access that is no longer granted, so
/// a grant this emulator cannot read in a stored policy any more reports no finding rather
/// than a resolved one.
pub const FINDING_STATUS_ACTIVE: &str = "ACTIVE";

/// The `ResourceType` of an IAM role's finding, from the model's
/// [`crate::ops::types::RESOURCE_TYPE_VALUES`].
pub const RESOURCE_TYPE_IAM_ROLE: &str = "AWS::IAM::Role";

/// The `ResourceType` of an Amazon S3 general purpose bucket's finding, from the same
/// enum.
pub const RESOURCE_TYPE_S3_BUCKET: &str = "AWS::S3::Bucket";

/// The `FindingSourceType` of access granted by the resource's own policy, from
/// [`crate::ops::types::FINDING_SOURCE_TYPE_VALUES`].
pub const FINDING_SOURCE_TYPE_POLICY: &str = "POLICY";

/// The `Principal` element's wildcard, as a finding reports the principal it stands for: a
/// statement that admits every principal grants to `*` and the finding says so.
pub const EVERY_PRINCIPAL: &str = "*";

/// How many resources of one account this emulator analyzes for one call.
///
/// The stores this reads are caller-driven growth, so the analysis is bounded in the number
/// of resources it walks as well as in the work each one costs ([`Budget`]); the ceiling is
/// generous beside the ten full pages ([`super::MAX_PAGE_SIZE`]) a realistic listing walks.
/// An account past it is REFUSED rather than analyzed in part, so no resource is left out of
/// a listing that reports itself complete.
pub const MAX_MONITORED_RESOURCES: usize = 1_000;

/// Whether an Amazon Web Services service principal is inside the zone of trust.
///
/// The two analyses in this crate answer it differently, and each answer is the recorded
/// behaviour of the operation that asks:
///
/// * [`Self::InsideZoneOfTrust`] — the analysis of a DEPLOYED resource. A role whose trust
///   policy names only a service principal reports no external access at all, so a service
///   acting on the account's own behalf is not an entity outside it.
/// * [`Self::OutsideZoneOfTrust`] — the access-preview analysis, which reports a service
///   principal as the external access it is asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServicePrincipals {
    /// A `Service` principal is one of the account's own: it contributes no finding.
    InsideZoneOfTrust,
    /// A `Service` principal is outside the account: it contributes a finding.
    OutsideZoneOfTrust,
}

/// What an analysis may still spend, charged for every policy read, statement visited,
/// principal considered, string copied into a finding and quantum of bytes a filter
/// criterion compares.
///
/// One budget per call, shared by every stage of it, so what is bounded is the PRODUCT of
/// the factors (resources times statements times principals times criteria) rather than
/// each factor alone.
#[derive(Debug)]
pub struct Budget {
    left: u64,
    ceiling: u64,
}

impl Budget {
    /// A budget of `units`, the ceiling the calling operation sets on one call's analysis.
    pub fn new(units: u64) -> Self {
        Self {
            left: units,
            ceiling: units,
        }
    }

    /// The ceiling this budget was built with, which is what a refusal past it states: the
    /// number a caller's analysis has to fit in, not the remainder it ran out of.
    pub fn ceiling(&self) -> u64 {
        self.ceiling
    }

    /// Charge `units`; `false` once the budget is spent, which stops the analysis
    /// BEFORE it does the work the units were for.
    pub fn spend(&mut self, units: u64) -> bool {
        match self.left.checked_sub(units) {
            Some(left) => {
                self.left = left;
                true
            }
            None => false,
        }
    }
}

/// Why an analyzable request could not be answered: the analysis is past one of this
/// emulator's ceilings. Names the request member a client has to change and what the
/// ceiling is — never the stored policy's content.
#[derive(Debug)]
pub struct Exhausted {
    /// The request member a client narrows to get under the ceiling.
    pub member: &'static str,
    /// What the ceiling is, as the refusal states it.
    pub detail: String,
}

/// One resource an external access analyzer monitors: where its resource-based policy came
/// from, and what the finding it contributes reports about the resource itself.
#[derive(Debug, Clone)]
pub struct MonitoredResource {
    /// The resource's own ARN, as the service that owns it reports it.
    pub arn: String,
    /// Which entry of the model's `ResourceType` enum the resource is.
    pub resource_type: &'static str,
    /// The account that owns it — the zone of trust its findings are analyzed against.
    pub owner_account: String,
    /// Its resource-based policy, exactly as the owning service holds it.
    pub policy: String,
    /// When the resource came into being, when the owning service records it. The finding
    /// cannot have been analyzed before the resource existed, which is what this bounds.
    pub created_at: Option<SystemTime>,
}

impl MonitoredResource {
    /// When the analysis that produced this resource's findings ran.
    ///
    /// The analyzer analyzes a resource once BOTH exist, so the instant is the later of the
    /// analyzer's own creation and the resource's: a stored instant either way, which is
    /// what makes every read of a finding report the same `analyzedAt` instead of the
    /// instant it was read at.
    pub fn analyzed_at(&self, analyzer_created_at: SystemTime) -> SystemTime {
        match self.created_at {
            Some(created_at) if created_at > analyzer_created_at => created_at,
            _ => analyzer_created_at,
        }
    }

    /// The `sources` of this resource's findings: how the access they report is granted.
    ///
    /// "It is populated for Amazon S3 bucket findings" (`FindingSummary$sources`), so only a
    /// bucket's findings carry it, and the access is granted by the bucket's own policy —
    /// which is the only policy of a bucket this analysis reads.
    pub fn sources(&self) -> Option<Vec<FindingSource>> {
        if self.resource_type != RESOURCE_TYPE_S3_BUCKET {
            return None;
        }
        Some(vec![FindingSource {
            r#type: FINDING_SOURCE_TYPE_POLICY.to_string(),
            detail: None,
        }])
    }
}

/// Every resource of `account` whose resource-based policy this emulator holds, in ARN
/// order.
///
/// Call it with a handler's `&*self.engine` and NO store lock held: each provider takes the
/// lock of the service that owns its resources, copies what the analysis needs out of it
/// and releases it, so no two of this emulator's stores are ever held at once.
///
/// Every resource found is returned — what this emulator will not analyze in ONE call is
/// refused by [`external_access_findings`] rather than silently dropped here, because a
/// resource left out of the walk would report no external access while granting some.
///
/// An owning service whose store cannot be reached contributes nothing: a resource whose
/// policy was not read is not analyzed, rather than reported as granting no access.
pub fn monitored_resources<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account: &str,
) -> Vec<MonitoredResource> {
    let mut resources: Vec<MonitoredResource> = Vec::new();
    iam_roles(engine, account, &mut resources);
    s3_buckets(engine, account, &mut resources);
    // ARN order, so the walk — and with it every finding's id and the order the findings
    // are reported in — is independent of how each store happens to be laid out.
    resources.sort_by(|left, right| left.arn.cmp(&right.arn));
    resources
}

/// The account's IAM roles, each with its trust policy.
///
/// A role's trust policy IS its resource-based policy: it names the principals that may
/// assume the role, so it is what decides whether a principal outside the account can reach
/// it. A role that carries none grants no access to analyze.
fn iam_roles<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account: &str,
    into: &mut Vec<MonitoredResource>,
) {
    let Some(iam) = ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(
        engine,
    ) else {
        return;
    };
    let store = iam.lock();
    for role in store.roles.values() {
        let Some(document) = role.assume_role_policy_document.as_deref() else {
            continue;
        };
        let owner_account = resource_owner_account(&role.arn, None);
        if owner_account != account {
            continue;
        }
        into.push(MonitoredResource {
            arn: role.arn.clone(),
            resource_type: RESOURCE_TYPE_IAM_ROLE,
            owner_account,
            policy: document.to_string(),
            created_at: SystemTime::try_from(role.create_date).ok(),
        });
    }
}

/// The account's Amazon S3 general purpose buckets, each with its bucket policy.
///
/// A bucket ARN carries no account segment, so the owner is the account every resource this
/// emulator serves belongs to ([`resource_owner_account`]); a bucket another service
/// provisioned as its own object store is not a bucket of the account's S3, which is what
/// `account_buckets` is the seam for.
fn s3_buckets<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account: &str,
    into: &mut Vec<MonitoredResource>,
) {
    let Some(s3) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return;
    };
    let store = s3.lock();
    for (name, bucket) in store.account_buckets() {
        let Some(document) = bucket.policy.as_deref() else {
            continue;
        };
        let arn = ::service_com_amazonaws_s3_amazons3::state::bucket_arn(name);
        let owner_account = resource_owner_account(&arn, None);
        if owner_account != account {
            continue;
        }
        into.push(MonitoredResource {
            arn,
            resource_type: RESOURCE_TYPE_S3_BUCKET,
            owner_account,
            policy: document.to_string(),
            created_at: bucket.created_at(),
        });
    }
}

/// The request member a refusal about the analysis itself names: the analyzer whose account
/// holds more than this emulator analyzes for one call.
const ANALYZER_ARN_MEMBER: &str = "analyzerArn";

/// What a stored policy is blamed on inside the policy engine's own errors. The analysis
/// reads a document the STORE holds rather than one the request carried, so the member a
/// finding's `error` names is the finding's own `resource`, and the text below names the
/// resource whose policy failed.
const RESOURCE_MEMBER: &str = "resource";

/// Every finding the account's monitored resources produce, each paired with the key the
/// listing is ordered by.
///
/// The key is the finding's position in the analysis — resource, then statement, then
/// principal — so the order is deterministic and a continuation token keeps naming the same
/// place when the same analyzer is listed again. Bounded in the PRODUCT of its factors:
/// every policy read, statement visited, principal considered and string copied into a
/// finding is charged to `budget`, and the finding count to `max_findings`.
pub fn external_access_findings(
    analyzer_arn: &str,
    analyzer_created_at: SystemTime,
    resources: &[MonitoredResource],
    max_findings: usize,
    budget: &mut Budget,
) -> Result<Vec<(String, FindingSummary)>, Exhausted> {
    // The account holds more analyzable resources than one call walks: refused naming the
    // ceiling, never answered with the findings of the first few, which would report no
    // external access for every resource left out of the walk.
    if resources.len() > MAX_MONITORED_RESOURCES {
        return Err(too_many_resources(resources.len()));
    }
    let mut findings: Vec<(String, FindingSummary)> = Vec::new();
    for resource in resources {
        if !budget.spend(1) {
            return Err(analysis_exhausted(budget.ceiling()));
        }
        let analyzed_at = ::aws_smithy_types::DateTime::from(
            resource.analyzed_at(analyzer_created_at),
        );
        let template = FindingSummary {
            id: String::new(),
            principal: None,
            action: None,
            resource: Some(resource.arn.clone()),
            is_public: None,
            resource_type: resource.resource_type.to_string(),
            // `condition` is a required member, so a statement that constrains nothing
            // reports an EMPTY map rather than no member at all.
            condition: HashMap::new(),
            // The analysis is what produced the finding, and nothing has updated it
            // since, so all three instants are the one instant the analysis ran at.
            created_at: analyzed_at,
            analyzed_at,
            updated_at: analyzed_at,
            status: FINDING_STATUS_ACTIVE.to_string(),
            resource_owner_account: resource.owner_account.clone(),
            error: None,
            sources: resource.sources(),
            resource_control_policy_restriction: None,
        };

        let document = readable_policy(&resource.policy);
        match policy::parse(&document, RESOURCE_MEMBER, policy::Flavor::Resource) {
            Err(error) => {
                // The analysis of THIS resource failed. Reported as a finding carrying the
                // modelled `error` member — which is what that member is for — rather than
                // as silence, which would claim the policy grants no external access.
                if findings.len() >= max_findings {
                    return Err(too_many_findings(max_findings));
                }
                let key = format!("{}\u{1f}", resource.arn);
                findings.push((
                    key.clone(),
                    FindingSummary {
                        id: derived_uuid(&[analyzer_arn, &key]),
                        error: Some(analysis_error(&resource.arn, &error)),
                        ..template
                    },
                ));
            }
            Ok(parsed) => {
                for statement in &parsed.statements {
                    // One unit for the statement, and one per principal its element names —
                    // the principals this statement will be walked over.
                    let named = statement
                        .principals
                        .as_ref()
                        .map_or(0, |principals| principals.values.len());
                    if !budget.spend(1 + named as u64) {
                        return Err(analysis_exhausted(budget.ceiling()));
                    }
                    let actions = granted_actions(statement);
                    let condition = granted_conditions(statement);
                    // What one finding of this statement COPIES: its principal, its action
                    // list and its condition map. Charged per finding, so what the
                    // derivation allocates is bounded by the budget too.
                    let per_finding = 1
                        + actions.as_ref().map_or(0, Vec::len) as u64
                        + condition.as_ref().map_or(0, HashMap::len) as u64;
                    for grant in external_grants(
                        statement,
                        &resource.owner_account,
                        // A deployed resource's analysis: a service principal acts on the
                        // account's own behalf, so it is inside the zone of trust.
                        ServicePrincipals::InsideZoneOfTrust,
                    ) {
                        if findings.len() >= max_findings {
                            return Err(too_many_findings(max_findings));
                        }
                        if !budget.spend(per_finding) {
                            return Err(analysis_exhausted(budget.ceiling()));
                        }
                        let (kind, value) = reported_principal(&grant);
                        let key = format!(
                            "{}\u{1f}{:08}\u{1f}{kind}\u{1f}{value}",
                            resource.arn, statement.index
                        );
                        findings.push((
                            key.clone(),
                            FindingSummary {
                                id: derived_uuid(&[analyzer_arn, &key]),
                                principal: Some(HashMap::from([(kind, value)])),
                                action: actions.clone(),
                                condition: condition.clone().unwrap_or_default(),
                                is_public: Some(grant.is_public),
                                ..template.clone()
                            },
                        ));
                    }
                }
            }
        }
    }
    findings.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(findings)
}

/// The analysis of this account's resources takes more work than one call spends.
fn analysis_exhausted(ceiling: u64) -> Exhausted {
    Exhausted {
        member: ANALYZER_ARN_MEMBER,
        detail: format!(
            "analyzing the resource-based policies this account holds takes more than the \
             {ceiling} steps this emulator spends on one call"
        ),
    }
}

/// The account holds more resources with a resource-based policy than one call analyzes.
fn too_many_resources(held: usize) -> Exhausted {
    Exhausted {
        member: ANALYZER_ARN_MEMBER,
        detail: format!(
            "this account holds {held} resources with a resource-based policy, more than the \
             {MAX_MONITORED_RESOURCES} this emulator analyzes for one call"
        ),
    }
}

/// The account grants external access to more principals than this emulator reports
/// findings for.
fn too_many_findings(max_findings: usize) -> Exhausted {
    Exhausted {
        member: ANALYZER_ARN_MEMBER,
        detail: format!(
            "the resources of this account grant external access to more than the \
             {max_findings} principals this emulator reports findings for"
        ),
    }
}

/// The `error` a finding carries when the analysis of one stored policy failed: which
/// resource, and what the shared parser could not read in its policy.
fn analysis_error(resource: &str, error: &policy::EngineError) -> String {
    let detail = match error {
        policy::EngineError::Unparseable { detail, .. } => {
            format!("it is not a JSON document ({detail})")
        }
        policy::EngineError::Invalid { detail, .. } => {
            format!("it is not a resource policy ({detail})")
        }
        policy::EngineError::Unsupported { detail } => detail.clone(),
    };
    format!("the resource-based policy of {resource} could not be analyzed: {detail}")
}

/// The policy `document` carries, in either of the two forms a store hands one back in.
///
/// A document that is JSON as it stands is taken AS IT STANDS: percent-decoding is a URI
/// decoder, so running it over text that needs no decoding would rewrite a literal `+`
/// inside the policy (a condition value, a base64-looking string) to a space and corrupt
/// it. Only a document that does not parse is percent-decoded — the URL-encoded form IAM
/// reports a trust policy in — and read again.
fn readable_policy(document: &str) -> Cow<'_, str> {
    if ::serde_json::from_str::<::serde_json::Value>(document).is_ok() {
        return Cow::Borrowed(document);
    }
    Cow::Owned(::smithy_server_runtime::percent_decode(document))
}

/// The principal a finding reports for one grant: the type the statement keyed it by, and
/// the principal itself.
///
/// An `AWS` principal that is an account's ROOT is reported as the account id: the account
/// is the external entity the grant reaches, and the bare id and the `:root` ARN are the two
/// spellings of it, so a finding names it the one way whichever spelling the policy used.
fn reported_principal(grant: &ExternalGrant) -> (String, String) {
    let value = match grant.kind {
        policy::PrincipalKind::Aws => {
            account_root(&grant.value).unwrap_or(grant.value.as_str())
        }
        _ => grant.value.as_str(),
    };
    (grant.kind.as_str().to_string(), value.to_string())
}

/// The account an `arn:<partition>:iam::<account>:root` names, or `None` for any other
/// value.
fn account_root(value: &str) -> Option<&str> {
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    if fields.len() != 6 || fields[0] != "arn" || fields[2] != "iam" || fields[5] != "root" {
        return None;
    }
    Some(fields[4]).filter(|account| !account.is_empty())
}

// ---------------------------------------------------------------------------
// The one rule for which principals a statement grants external access to
//
// Shared by every analysis in this crate — the findings of a deployed resource and the
// findings of an access preview — so the two cannot answer the question differently.
// ---------------------------------------------------------------------------

/// One external grant a statement makes: which principal it reaches, and whether that
/// principal is the public.
#[derive(Debug)]
pub struct ExternalGrant {
    /// The principal type the `Principal` element keyed it by.
    pub kind: policy::PrincipalKind,
    /// The principal as the element names it.
    pub value: String,
    /// Whether the grant is to the public: every principal, with nothing binding the
    /// caller.
    pub is_public: bool,
}

/// The principals OUTSIDE the zone of trust that one statement grants access to, in the
/// order the statement names them.
///
/// Empty for a statement that grants nothing (`Deny`), names no principal, or names only
/// principals inside the zone of trust — which is what "this policy allows no external
/// access" means.
pub fn external_grants(
    statement: &policy::Statement,
    owner_account: &str,
    services: ServicePrincipals,
) -> Vec<ExternalGrant> {
    if statement.effect != policy::Effect::Allow {
        return Vec::new();
    }
    let Some(principals) = statement.principals.as_ref() else {
        return Vec::new();
    };

    // An element that admits EVERY principal reaches principals outside the account
    // whatever else it lists, and `NotPrincipal` with `Allow` admits every principal the
    // element does not name — so the grant is to `*`, and the principals such an element
    // lists are the ones EXCLUDED from it rather than granted to.
    if principals.any || principals.negated {
        return vec![ExternalGrant {
            kind: policy::PrincipalKind::Aws,
            value: EVERY_PRINCIPAL.to_string(),
            // The one public-access rule in this crate, which `CheckNoPublicAccess`
            // answers with: public exactly when no `Condition` clause binds the caller.
            is_public: crate::ops::check_no_public_access::grants_public_access(statement),
        }];
    }

    principals
        .values
        .iter()
        .filter(|named| outside_zone_of_trust(named, owner_account, services))
        .map(|named| ExternalGrant {
            kind: named.kind,
            value: named.value.clone(),
            // A named principal is one entity, never the public.
            is_public: false,
        })
        .collect()
}

/// Whether one named principal is outside the zone of trust of `owner_account`.
///
/// An `AWS` principal belongs to the account its ARN names (the parser has already
/// normalised a bare account id to that account's `:root` ARN), so it is inside the zone
/// of trust exactly when that account is the owning one; a principal whose account cannot
/// be read — a wildcard account segment, or a value that is not an ARN — is reported
/// rather than trusted. A federated identity and an Amazon S3 canonical user name no
/// account to compare against and are not covered by the owning account's own principals,
/// so each is external; whether a service principal is depends on what the analysis is of
/// ([`ServicePrincipals`]).
fn outside_zone_of_trust(
    named: &policy::NamedPrincipal,
    owner_account: &str,
    services: ServicePrincipals,
) -> bool {
    match named.kind {
        policy::PrincipalKind::Aws => {
            principal_account(&named.value).is_none_or(|account| account != owner_account)
        }
        policy::PrincipalKind::Service => services == ServicePrincipals::OutsideZoneOfTrust,
        policy::PrincipalKind::Federated | policy::PrincipalKind::CanonicalUser => true,
    }
}

/// The account segment of an ARN, or `None` when the value is not an ARN with one.
///
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — the account is the fifth
/// of six colon-separated fields, and the resource part may itself contain `:`.
pub fn principal_account(value: &str) -> Option<&str> {
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    if fields.len() != 6 || fields[0] != "arn" {
        return None;
    }
    Some(fields[4]).filter(|account| !account.is_empty())
}

/// The actions a statement allows the external principal to perform, as the finding
/// reports them.
///
/// `None` for a statement written with `NotAction`, which allows every action EXCEPT the
/// ones it lists: the finding's `action` is "the action in the analyzed policy statement
/// that an external principal has permission to use", and that set is not the listed one —
/// reporting the list would name actions the statement does not allow.
pub fn granted_actions(statement: &policy::Statement) -> Option<Vec<String>> {
    if statement.actions.negated || statement.actions.values.is_empty() {
        return None;
    }
    Some(statement.actions.values.clone())
}

/// The condition keys a statement constrains, keyed as the document spells them.
///
/// `ConditionKeyMap` is one value per key, so a key constrained by several operators (or
/// accepting several values) reports them as one comma-separated value; `None` for a
/// statement that carries no `Condition` at all, since the member reports "the condition
/// in the analyzed policy statement that resulted in a finding".
pub fn granted_conditions(statement: &policy::Statement) -> Option<HashMap<String, String>> {
    if statement.conditions.is_empty() {
        return None;
    }
    let mut accepted: std::collections::BTreeMap<&str, Vec<String>> =
        std::collections::BTreeMap::new();
    for clause in &statement.conditions {
        accepted
            .entry(clause.spelling.as_str())
            .or_default()
            .extend(clause.values.iter().cloned());
    }
    Some(
        accepted
            .into_iter()
            .map(|(key, values)| (key.to_string(), values.join(",")))
            .collect(),
    )
}
