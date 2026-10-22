// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53recoveryreadiness-route53recoveryreadiness service.
//!
//! Besides the state struct this module holds the few helpers the resource-set and
//! readiness-check handlers share — ARN construction, name validation, the
//! record-to-modelled-shape conversions and the list cursor codec — so the eight
//! operation files agree on one implementation of each instead of carrying eight
//! copies that can drift apart.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ops::types::{ReadinessCheckOutput, Resource, ResourceSetOutput};

/// The `arnNamespace` this service's ARNs carry, from the model's `aws.api#service`
/// trait (`smithy_query(query="service_info")`: `arnNamespace:
/// route53-recovery-readiness`).
pub const ARN_NAMESPACE: &str = "route53-recovery-readiness";

/// Route 53 Application Recovery Controller readiness is a GLOBAL service: its
/// readiness-check and resource-set ARNs carry an empty region segment
/// (`arn:aws:route53-recovery-readiness::<account>:resource-set/<name>`), which is
/// why no handler reads the request's region.
const ARN_REGION: &str = "";

/// The `@pattern` and `@length` every name this service reports carries, from the
/// `__stringMax64PatternAAZAZ09Z` shape the `ResourceSetName` / `ReadinessCheckName`
/// / `ResourceSet` OUTPUT members target
/// (`smithy_query(query="shape", shape="__stringMax64PatternAAZAZ09Z")`).
///
/// The matching INPUT members target plain `__string`, so the boundary validator
/// does not refuse a name the service could never report back; [`validate_name`]
/// is what applies the rule on the way in.
pub const NAME_PATTERN: &str = r"^\A[a-zA-Z0-9_]+\z$";

/// The maximum name length the same shape declares (`length 0..64`).
pub const NAME_MAX_LENGTH: usize = 64;

/// The bounds `MaxResults` declares on both list operations (`range 1..1000`,
/// `smithy_query(query="shape", shape="MaxResults")`).
pub const MAX_RESULTS_MIN: i32 = 1;
pub const MAX_RESULTS_MAX: i32 = 1000;

/// The page size a list uses when the caller sends no `MaxResults`.
///
/// The model publishes no default page size, so this is the emulator's own: a caller
/// who names no page size gets a bounded page and a `NextToken`, rather than having a
/// one-line request amplified into every record the store holds. A caller who wants
/// more asks for it, up to [`MAX_RESULTS_MAX`].
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// The maximum number of records this emulator keeps in EACH of its caller-driven
/// stores (resource sets, readiness checks, cross-account authorizations).
///
/// This is NOT an AWS service quota — Route 53 Application Recovery Controller's
/// published limits are per-account and adjustable, and nothing here claims to
/// reproduce them. It is the emulator's own memory ceiling: every successful create
/// inserts a record that lives until a caller deletes it, so without a ceiling a
/// caller varying the name could grow the process until the host runs out of memory.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The maximum number of `Resource` entries one resource set may carry, and the
/// maximum number of `ReadinessScopes` one of those entries may carry.
///
/// Also the emulator's own ceilings rather than AWS quotas: together with
/// [`MAX_RECORDS_PER_STORE`] and [`MAX_TAGS_PER_RESOURCE`] they make the total a store
/// can hold — and therefore the largest list response — a finite product instead of
/// something a caller chooses.
pub const MAX_RESOURCES_PER_SET: usize = 100;
pub const MAX_READINESS_SCOPES_PER_RESOURCE: usize = 50;

/// The maximum number of tags one record may carry (the emulator's own ceiling).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The maximum number of nested cell ARNs one cell may carry (the emulator's own
/// ceiling, for the same reason as [`MAX_RESOURCES_PER_SET`]: one accepted record must
/// not be arbitrarily large).
pub const MAX_NESTED_CELLS_PER_CELL: usize = 50;

/// The sentence EVERY not-found answer of this service's resource-addressed operations
/// carries.
///
/// This service's not-found message is a CONSTANT, resource-agnostic text — the same
/// string whatever was asked for — rather than a sentence built from the request, as
/// recorded consistently across `DeleteCell`, `GetCell`, `GetCellReadinessSummary`,
/// `GetReadinessCheck`, `GetReadinessCheckStatus` and `GetReadinessCheckResourceStatus`.
/// Keeping it in one place is what keeps every handler's wording identical, and it is
/// why no handler echoes the requested name back (which would also leak which names an
/// account uses into an error body).
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The same constant for the ARN-addressed TAGGING operations
/// (`ListTagsForResources`, `TagResource`, `UntagResource`), which this service answers
/// with its own wire error code [`crate::scaffolded::NOT_FOUND_EXCEPTION_NAME`] and its
/// own fixed sentence.
pub const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The sentence a list operation answers when the `NextToken` it was handed is not one
/// this service minted. Also a constant: the rejected token is NOT echoed back.
pub const INVALID_PAGINATION_TOKEN_MESSAGE: &str = "Invalid pagination token.";

/// The sentence the account-eligibility gate below refuses a create with.
pub const ACCOUNT_NOT_ELIGIBLE_MESSAGE: &str =
    "This operation is currently unavailable for new accounts.";

/// The environment variable an operator uses to tell this emulated account that it MAY
/// create Route 53 Application Recovery Controller readiness resources, since the API
/// models no operation that grants that eligibility
/// (`ROUTE53_RECOVERY_READINESS_ACCOUNT_ELIGIBLE=1`), read once when this service's
/// store is created.
///
/// Unset means NOT eligible, which is what an account that does not already use the
/// readiness feature has: the feature is closed to new accounts, so AWS passes such a
/// create through input validation and then denies it with HTTP 403. See
/// [`ComAmazonawsRoute53recoveryreadinessRoute53recoveryreadinessState::account_readiness_eligible`].
pub const ACCOUNT_ELIGIBLE_ENV: &str = "ROUTE53_RECOVERY_READINESS_ACCOUNT_ELIGIBLE";

/// Whether `setting` grants this account the readiness feature: `1`, `true`, `yes` or
/// `on` in any case grants it, and anything else — including an empty value or a typo —
/// grants nothing rather than being guessed at.
pub fn parse_account_eligible(setting: &str) -> bool {
    matches!(
        setting.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// The readiness this emulator reports for a check, a cell or a resource it has never
/// audited: `UNKNOWN`, one of the four values the `Readiness` enum declares
/// (`smithy_query(query="shape", shape="Readiness")`: READY, NOT_READY, UNKNOWN,
/// NOT_AUTHORIZED).
///
/// Nothing in this emulator inspects the real AWS resources a resource set names, so no
/// readiness rule has ever been evaluated against them. `UNKNOWN` is the value that
/// says exactly that, and it is what AWS itself reports for a readiness check whose
/// first audit has not completed; reporting READY or NOT_READY would be a verdict on
/// work nothing ran.
pub const READINESS_UNKNOWN: &str = "UNKNOWN";

/// The resource types this API supports, as the `ResourceSetType` member's own
/// documentation enumerates them (`smithy_query(query="doc",
/// shape="CreateResourceSetRequest", member="ResourceSetType")`).
///
/// This is the catalog a readiness rule can apply to, and therefore the set of values
/// `ListRules`' `ResourceType` filter accepts.
pub const SUPPORTED_RESOURCE_TYPES: &[&str] = &[
    "AWS::ApiGateway::Stage",
    "AWS::ApiGatewayV2::Stage",
    "AWS::AutoScaling::AutoScalingGroup",
    "AWS::CloudWatch::Alarm",
    "AWS::DynamoDB::Table",
    "AWS::EC2::CustomerGateway",
    "AWS::EC2::VPC",
    "AWS::EC2::VPNConnection",
    "AWS::EC2::VPNGateway",
    "AWS::EC2::Volume",
    "AWS::ElasticLoadBalancing::LoadBalancer",
    "AWS::ElasticLoadBalancingV2::LoadBalancer",
    "AWS::Lambda::Function",
    "AWS::MSK::Cluster",
    "AWS::RDS::DBCluster",
    "AWS::Route53::HealthCheck",
    "AWS::Route53RecoveryReadiness::DNSTargetResource",
    "AWS::SNS::Subscription",
    "AWS::SNS::Topic",
    "AWS::SQS::Queue",
];

/// Refuse a resource-type filter this API does not support, in the wording AWS's
/// boundary validator uses.
///
/// A filter value outside [`SUPPORTED_RESOURCE_TYPES`] names no resource type this
/// service has rules for, so it is refused rather than silently ignored — a filter that
/// is honoured for supported values and dropped for unsupported ones would hand back
/// the whole catalog for a typo.
pub fn validate_resource_type(path: &str, value: &str) -> Result<(), String> {
    if SUPPORTED_RESOURCE_TYPES.contains(&value) {
        return Ok(());
    }
    Err(validation_message(
        path,
        value,
        &format!(
            "Member must be one of the supported resource types: {}",
            SUPPORTED_RESOURCE_TYPES.join(", ")
        ),
    ))
}

/// Longest prefix of a rejected value a validation message echoes back, so a
/// multi-megabyte member cannot be amplified into a multi-megabyte error body.
const ECHO_LIMIT: usize = 64;

/// The message every handler answers with when the engine it was handed is not the
/// one this service's store lives in. One wording in one place, rather than eight
/// copies that can drift.
pub const STORE_UNAVAILABLE: &str =
    "the Route 53 Application Recovery Controller readiness store is unavailable";

/// One stored resource set: everything `CreateResourceSet` was given plus the ARN
/// the service minted for it, so `GetResourceSet` and `ListResourceSets` report the
/// identical values `CreateResourceSet` returned instead of recomputing them.
///
/// `resources` holds the modelled [`Resource`] shape itself rather than a parallel
/// record type: the shape is `Clone` + `Debug`, every member of it (including the
/// whole `DnsTargetResource` subtree) is preserved exactly as submitted, and one
/// representation cannot drift from the other.
#[derive(Debug, Clone)]
pub struct ResourceSetRecord {
    pub arn: String,
    pub name: String,
    pub resource_set_type: String,
    pub resources: Vec<Resource>,
    pub tags: BTreeMap<String, String>,
}

/// One stored cell: the ARN the service minted, the name it is keyed by, the ARNs of
/// the cells nested inside it and its tags.
///
/// A cell's `ParentReadinessScopes` is NOT stored: it is the set of records that name
/// THIS cell as nested, which is derived from the other records rather than submitted
/// (see
/// [`ComAmazonawsRoute53recoveryreadinessRoute53recoveryreadinessState::parent_readiness_scopes`]),
/// so one representation cannot drift from the other.
#[derive(Debug, Clone)]
pub struct CellRecord {
    pub arn: String,
    pub name: String,
    pub nested_cell_arns: Vec<String>,
    pub tags: BTreeMap<String, String>,
}

/// One stored recovery group: the ARN the service minted, the name it is keyed by, the
/// ARNs of the cells it contains and its tags.
#[derive(Debug, Clone)]
pub struct RecoveryGroupRecord {
    pub arn: String,
    pub name: String,
    pub cell_arns: Vec<String>,
    pub tags: BTreeMap<String, String>,
}

/// One stored readiness check: the ARN the service minted, the name it is keyed by,
/// the resource set it watches and its tags.
#[derive(Debug, Clone)]
pub struct ReadinessCheckRecord {
    pub arn: String,
    pub name: String,
    pub resource_set_name: String,
    pub tags: BTreeMap<String, String>,
}

/// Which record an ARN belongs to, so an ARN-addressed lookup resolves back to the
/// owning map. Readiness and architecture resources this slice does not store yet
/// (cells, recovery groups) get their own variants when they are implemented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceKey {
    ResourceSet(String),
    ReadinessCheck(String),
    Cell(String),
    RecoveryGroup(String),
}

impl ResourceSetRecord {
    /// The persisted tags as the modelled `Tags` map, or `None` when the resource
    /// carries none: an untagged resource reports no `tags` member rather than an
    /// empty object.
    pub fn modeled_tags(&self) -> Option<HashMap<String, String>> {
        modeled_tags(&self.tags)
    }

    /// This record as a `ListResourceSets` entry.
    pub fn to_resource_set_output(&self) -> ResourceSetOutput {
        ResourceSetOutput {
            resource_set_arn: self.arn.clone(),
            resource_set_name: self.name.clone(),
            resource_set_type: self.resource_set_type.clone(),
            resources: self.resources.clone(),
            tags: self.modeled_tags(),
        }
    }
}

impl ReadinessCheckRecord {
    /// The persisted tags as the modelled `Tags` map, or `None` when the resource
    /// carries none.
    pub fn modeled_tags(&self) -> Option<HashMap<String, String>> {
        modeled_tags(&self.tags)
    }

    /// This record as a `ListReadinessChecks` entry.
    pub fn to_readiness_check_output(&self) -> ReadinessCheckOutput {
        ReadinessCheckOutput {
            readiness_check_arn: self.arn.clone(),
            readiness_check_name: Some(self.name.clone()),
            resource_set: self.resource_set_name.clone(),
            tags: self.modeled_tags(),
        }
    }
}

impl CellRecord {
    /// The persisted tags as the modelled `Tags` map, or `None` when the cell carries
    /// none.
    pub fn modeled_tags(&self) -> Option<HashMap<String, String>> {
        modeled_tags(&self.tags)
    }
}

impl RecoveryGroupRecord {
    /// The persisted tags as the modelled `Tags` map, or `None` when the recovery group
    /// carries none.
    pub fn modeled_tags(&self) -> Option<HashMap<String, String>> {
        modeled_tags(&self.tags)
    }
}

fn modeled_tags(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    if tags.is_empty() {
        return None;
    }
    Some(tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// Per-service state for com-amazonaws-route53recoveryreadiness-route53recoveryreadiness. A handler reaches it through [`service_state`].
///
/// `BTreeMap` everywhere, so list order — and therefore pagination — is the same on
/// every call and a cursor can be a key rather than an index.
///
/// GROWTH IS BOUNDED. Each store holds at most [`MAX_RECORDS_PER_STORE`] records, each
/// record at most [`MAX_RESOURCES_PER_SET`] resources ([`MAX_READINESS_SCOPES_PER_RESOURCE`]
/// scopes each) and [`MAX_TAGS_PER_RESOURCE`] tags, and each list response at most
/// [`MAX_RESULTS_MAX`] records — [`DEFAULT_MAX_RESULTS`] when the caller names no page
/// size — so both the retained state and the largest response are a finite product of
/// those ceilings rather than something a caller chooses. The size of the individual
/// strings inside a record is bounded by the server's own request-size limit.
#[derive(Debug)]
pub struct ComAmazonawsRoute53recoveryreadinessRoute53recoveryreadinessState {
    /// Every stored resource set, keyed by `ResourceSetName` (this API's identity
    /// for the resource).
    pub resource_sets: BTreeMap<String, ResourceSetRecord>,
    /// Every stored readiness check, keyed by `ReadinessCheckName`.
    pub readiness_checks: BTreeMap<String, ReadinessCheckRecord>,
    /// Every stored cell, keyed by `CellName`.
    pub cells: BTreeMap<String, CellRecord>,
    /// Every stored recovery group, keyed by `RecoveryGroupName`.
    pub recovery_groups: BTreeMap<String, RecoveryGroupRecord>,
    /// ARN -> owning record, so an ARN-addressed call (the tag operations, which
    /// take a `ResourceArn` rather than a name) resolves one without scanning
    /// every map.
    pub arn_index: BTreeMap<String, ResourceKey>,
    /// Every cross-account readiness authorization this account has in place.
    ///
    /// A SET, because the authorization value is the resource's whole identity (the
    /// API carries no other member for it, and `ListCrossAccountAuthorizations`
    /// reports a bare list of those values): the same value cannot be authorized
    /// twice, and `BTreeSet` keeps list order — and therefore pagination — stable.
    pub cross_account_authorizations: BTreeSet<String>,
    /// Whether THIS account may create readiness resources at all.
    ///
    /// The Route 53 Application Recovery Controller readiness feature is closed to
    /// accounts that do not already use it: a syntactically valid `CreateCell` /
    /// `CreateReadinessCheck` / `CreateRecoveryGroup` / `CreateResourceSet` passes
    /// input validation and is then denied account-wide with HTTP 403. The API models
    /// no operation that grants the eligibility, so it is a field rather than a
    /// constant: `false` (what a new account has, and the default) makes every create
    /// answer that denial, and [`ACCOUNT_ELIGIBLE_ENV`] is how an operator says this
    /// emulated account is one of the accounts that already had the feature.
    pub account_readiness_eligible: bool,
}

impl Default for ComAmazonawsRoute53recoveryreadinessRoute53recoveryreadinessState {
    /// An account with no resources, eligible to create readiness resources only when
    /// [`ACCOUNT_ELIGIBLE_ENV`] says so.
    fn default() -> Self {
        Self {
            resource_sets: BTreeMap::new(),
            readiness_checks: BTreeMap::new(),
            cells: BTreeMap::new(),
            recovery_groups: BTreeMap::new(),
            arn_index: BTreeMap::new(),
            cross_account_authorizations: BTreeSet::new(),
            account_readiness_eligible: ::std::env::var(ACCOUNT_ELIGIBLE_ENV)
                .is_ok_and(|setting| parse_account_eligible(&setting)),
        }
    }
}

impl ComAmazonawsRoute53recoveryreadinessRoute53recoveryreadinessState {
    /// The ACCOUNT-ELIGIBILITY GATE every create of a readiness resource passes
    /// through, AFTER its input has been validated and BEFORE anything is written.
    ///
    /// `Err` carries [`ACCOUNT_NOT_ELIGIBLE_MESSAGE`]; each create answers it as its
    /// own modelled `AccessDeniedException` (HTTP 403), the error its model declares
    /// for a denial. One gate in one place, so the four creates cannot disagree about
    /// when a create is allowed, and no create can write first and deny after.
    pub fn check_create_allowed(&self) -> Result<(), &'static str> {
        if self.account_readiness_eligible {
            return Ok(());
        }
        Err(ACCOUNT_NOT_ELIGIBLE_MESSAGE)
    }

    /// The tags of the record `key` names, or `None` when no record holds that key.
    pub fn tags_of(&self, key: &ResourceKey) -> Option<&BTreeMap<String, String>> {
        match key {
            ResourceKey::ResourceSet(name) => self.resource_sets.get(name).map(|r| &r.tags),
            ResourceKey::ReadinessCheck(name) => self.readiness_checks.get(name).map(|r| &r.tags),
            ResourceKey::Cell(name) => self.cells.get(name).map(|r| &r.tags),
            ResourceKey::RecoveryGroup(name) => self.recovery_groups.get(name).map(|r| &r.tags),
        }
    }

    /// The tags of the record `key` names, for writing.
    pub fn tags_mut(&mut self, key: &ResourceKey) -> Option<&mut BTreeMap<String, String>> {
        match key {
            ResourceKey::ResourceSet(name) => {
                self.resource_sets.get_mut(name).map(|r| &mut r.tags)
            }
            ResourceKey::ReadinessCheck(name) => {
                self.readiness_checks.get_mut(name).map(|r| &mut r.tags)
            }
            ResourceKey::Cell(name) => self.cells.get_mut(name).map(|r| &mut r.tags),
            ResourceKey::RecoveryGroup(name) => {
                self.recovery_groups.get_mut(name).map(|r| &mut r.tags)
            }
        }
    }

    /// The readiness scopes that CONTAIN the cell whose ARN is `cell_arn`: the ARN of
    /// every stored cell that names it as a nested cell, and of every stored recovery
    /// group that names it as one of its cells — the two kinds of scope the member's
    /// own documentation allows ("a cell Amazon Resource Name (ARN) or a recovery group
    /// ARN").
    ///
    /// Derived on every read rather than stored, so a cell reports the containment the
    /// store actually holds — a parent created after the child included.
    pub fn parent_readiness_scopes(&self, cell_arn: &str) -> Vec<String> {
        let nesting_cells = self
            .cells
            .values()
            .filter(|candidate| {
                candidate.arn != cell_arn
                    && candidate
                        .nested_cell_arns
                        .iter()
                        .any(|nested| nested == cell_arn)
            })
            .map(|parent| parent.arn.clone());
        let owning_groups = self
            .recovery_groups
            .values()
            .filter(|group| group.cell_arns.iter().any(|cell| cell == cell_arn))
            .map(|group| group.arn.clone());
        nesting_cells.chain(owning_groups).collect()
    }

    /// Every readiness scope the cell named `cell_name` covers: its own ARN plus the
    /// ARN of every cell nested inside it, transitively.
    ///
    /// A resource of a resource set declares the cell ARNs that contain it in its
    /// `ReadinessScopes`, and a cell contains whatever its nested cells contain, so
    /// this is the set a resource's scope is matched against. Cycles terminate: an ARN
    /// is expanded at most once.
    pub fn cell_scope_arns(&self, cell_name: &str) -> BTreeSet<String> {
        let mut scopes = BTreeSet::new();
        let Some(root) = self.cells.get(cell_name) else {
            return scopes;
        };
        let mut pending = vec![root.arn.clone()];
        while let Some(arn) = pending.pop() {
            if !scopes.insert(arn.clone()) {
                continue;
            }
            let nested = match self.arn_index.get(&arn) {
                Some(ResourceKey::Cell(name)) => self.cells.get(name),
                _ => None,
            };
            if let Some(cell) = nested {
                pending.extend(cell.nested_cell_arns.iter().cloned());
            }
        }
        scopes
    }

    /// The names of the readiness checks that watch a resource scoped to the cell named
    /// `cell_name` — the checks a cell's readiness summary aggregates.
    ///
    /// A check watches a resource set; that set's resources carry the cell ARNs that
    /// contain them. So a check belongs to a cell exactly when one of the resources it
    /// watches is scoped to that cell or to a cell nested inside it.
    pub fn readiness_check_names_in_cell(&self, cell_name: &str) -> BTreeSet<String> {
        let scopes = self.cell_scope_arns(cell_name);
        self.readiness_checks
            .values()
            .filter(|check| {
                self.resource_sets
                    .get(&check.resource_set_name)
                    .is_some_and(|set| {
                        set.resources.iter().any(|resource| {
                            resource.readiness_scopes.iter().flatten().any(|scope| scopes.contains(scope))
                        })
                    })
            })
            .map(|check| check.name.clone())
            .collect()
    }
}

/// The ARN of the resource set named `name`:
/// `arn:aws:route53-recovery-readiness::<account>:resource-set/<name>`.
pub fn resource_set_arn(name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, ARN_REGION, &format!("resource-set/{name}"))
}

/// The ARN of the cell named `name`:
/// `arn:aws:route53-recovery-readiness::<account>:cell/<name>`.
pub fn cell_arn(name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, ARN_REGION, &format!("cell/{name}"))
}

/// The ARN of the recovery group named `name`:
/// `arn:aws:route53-recovery-readiness::<account>:recovery-group/<name>`.
pub fn recovery_group_arn(name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        ARN_REGION,
        &format!("recovery-group/{name}"),
    )
}

/// The ARN of the readiness check named `name`:
/// `arn:aws:route53-recovery-readiness::<account>:readiness-check/<name>`.
pub fn readiness_check_arn(name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        ARN_REGION,
        &format!("readiness-check/{name}"),
    )
}

/// True when `value` satisfies [`NAME_PATTERN`].
///
/// The pattern is `^\A[a-zA-Z0-9_]+\z$`: `\A` and `\z` are the same anchors as `^`
/// and `$` on a single-line value, so it admits exactly a non-empty run of ASCII
/// letters, digits and underscores and nothing else. That character class is what this
/// checks directly — a one-line class, not a language, so it needs no regex engine, and
/// the rejected value is reported against the pattern literal itself so the message and
/// the check cannot disagree.
fn matches_name_pattern(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Refuse a name the service could not report back, in the wording AWS's boundary
/// validator uses: the rejected value, the member path and the constraint it broke.
///
/// `path` is the member's own name (`ResourceSetName`, `ReadinessCheckName`,
/// `ResourceSetName` on a create's resource-set reference), so the caller can fix
/// the request from the message alone.
pub fn validate_name(path: &str, value: &str) -> Result<(), String> {
    if value.chars().count() > NAME_MAX_LENGTH {
        return Err(validation_message(
            path,
            value,
            &format!("Member must have length less than or equal to {NAME_MAX_LENGTH}"),
        ));
    }
    if !matches_name_pattern(value) {
        return Err(validation_message(
            path,
            value,
            &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
        ));
    }
    Ok(())
}

/// Refuse a create because the store is already at [`MAX_RECORDS_PER_STORE`]; `store`
/// names the records it holds, e.g. `"resource sets"`.
///
/// Answered as a 400 `ValidationException` rather than as the 429 `ThrottlingException`
/// the model also declares: neither create models a limit error, and a 429 is
/// retryable, so an SDK would spin on a refusal that cannot clear until a caller
/// deletes something.
pub fn store_full_message(store: &str) -> String {
    format!(
        "this emulator holds at most {MAX_RECORDS_PER_STORE} {store}; \
         delete one before creating another"
    )
}

/// Refuse a cross-account authorization value the service could never address back.
///
/// The model constrains `CrossAccountAuthorization` with neither `@length` nor
/// `@pattern` (`smithy_query(query="shape", shape="CrossAccountAuthorization")`), so
/// the boundary validator accepts any string and this is the only rule applied: the
/// value is the resource's identity and `DeleteCrossAccountAuthorization` binds it to
/// an `@httpLabel` (`DELETE /crossaccountauthorizations/{CrossAccountAuthorization}`),
/// which Smithy forbids from being empty — an empty value would name a resource no
/// later call could delete.
pub fn validate_cross_account_authorization(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(validation_message(
            "crossAccountAuthorization",
            value,
            "Member must have length greater than or equal to 1",
        ));
    }
    Ok(())
}

/// Refuse a resource list that is longer than this emulator stores, or one whose entry
/// carries more readiness scopes than it stores.
pub fn validate_resources(resources: &[Resource]) -> Result<(), String> {
    if resources.len() > MAX_RESOURCES_PER_SET {
        return Err(count_message(
            "Resources",
            resources.len(),
            MAX_RESOURCES_PER_SET,
        ));
    }
    for (index, resource) in resources.iter().enumerate() {
        let scopes = resource.readiness_scopes.as_ref().map_or(0, Vec::len);
        if scopes > MAX_READINESS_SCOPES_PER_RESOURCE {
            return Err(count_message(
                &format!("Resources.{}.ReadinessScopes", index + 1),
                scopes,
                MAX_READINESS_SCOPES_PER_RESOURCE,
            ));
        }
    }
    Ok(())
}

/// Refuse a tag map with more entries than this emulator stores per record.
pub fn validate_tags(tags: Option<&HashMap<String, String>>) -> Result<(), String> {
    let count = tags.map_or(0, HashMap::len);
    if count > MAX_TAGS_PER_RESOURCE {
        return Err(count_message("Tags", count, MAX_TAGS_PER_RESOURCE));
    }
    Ok(())
}

/// Refuse a nested-cell list longer than this emulator stores. Also what a recovery
/// group's `Cells` list is bounded by: the member is the same, so the ceiling is.
pub fn validate_nested_cells(cells: Option<&Vec<String>>) -> Result<(), String> {
    let count = cells.map_or(0, Vec::len);
    if count > MAX_NESTED_CELLS_PER_CELL {
        return Err(count_message("Cells", count, MAX_NESTED_CELLS_PER_CELL));
    }
    Ok(())
}

/// Refuse a resulting tag set larger than this emulator stores per record, counting the
/// keys `incoming` adds to the `existing` ones.
pub fn validate_added_tags(
    existing: &BTreeMap<String, String>,
    incoming: &HashMap<String, String>,
) -> Result<(), String> {
    let merged = existing.len()
        + incoming
            .keys()
            .filter(|key| !existing.contains_key(*key))
            .count();
    if merged > MAX_TAGS_PER_RESOURCE {
        return Err(count_message("Tags", merged, MAX_TAGS_PER_RESOURCE));
    }
    Ok(())
}

/// One validation clause for a collection that is too large. The COUNT is reported
/// rather than the collection, so a refusal cannot amplify an oversized request into
/// an oversized error body.
fn count_message(path: &str, count: usize, max: usize) -> String {
    format!(
        "1 validation error detected: Value with {count} entries at '{path}' failed to \
         satisfy constraint: Member must have length less than or equal to {max}"
    )
}

/// One validation clause in the envelope AWS uses.
fn validation_message(path: &str, value: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{}' at '{path}' failed to satisfy constraint: {requirement}",
        echoed(value)
    )
}

/// `value`, truncated on a character boundary to [`ECHO_LIMIT`] characters.
fn echoed(value: &str) -> String {
    if value.chars().count() <= ECHO_LIMIT {
        return value.to_string();
    }
    value.chars().take(ECHO_LIMIT).collect::<String>() + "..."
}

/// The tag every cursor this service mints carries ahead of the key it points at, so a
/// token the service did not mint is refused rather than honoured as a start key.
const CURSOR_TAG: &str = "v1:";

/// The pagination cursor for a page that starts at `key`.
///
/// Hex of a tagged payload rather than the key itself, so a caller cannot read
/// structure out of the token: a `NextToken` is opaque and only ever travels back
/// unchanged.
pub fn encode_cursor(key: &str) -> String {
    ::hex::encode(format!("{CURSOR_TAG}{key}").as_bytes())
}

/// The inclusive start key a cursor minted by [`encode_cursor`] carries, or `None`
/// when the token was not minted by this service — not hex, not text, or not carrying
/// [`CURSOR_TAG`].
pub fn decode_cursor(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(CURSOR_TAG).map(str::to_string)
}

/// Refuse a `NextToken` this service did not mint, with
/// [`INVALID_PAGINATION_TOKEN_MESSAGE`] — for an operation that walks something other
/// than one of the stores [`page`] pages.
pub fn validate_next_token(token: Option<&str>) -> Result<(), String> {
    match token {
        Some(token) if decode_cursor(token).is_none() => {
            Err(INVALID_PAGINATION_TOKEN_MESSAGE.to_string())
        }
        _ => Ok(()),
    }
}

/// One page of a list operation: the records on it, and the cursor the next call
/// resumes from (`None` on the last page).
#[derive(Debug)]
pub struct Page<'a, V> {
    pub records: Vec<&'a V>,
    pub next_token: Option<String>,
}

/// The page of `records` the caller asked for.
///
/// `max_results` bounds the page (defaulting to [`DEFAULT_MAX_RESULTS`]) and
/// `next_token` resumes at the key the previous page stopped before. `Err` carries
/// [`INVALID_PAGINATION_TOKEN_MESSAGE`] for a token this service did not mint.
pub fn page<'a, V>(
    records: &'a BTreeMap<String, V>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<'a, V>, String> {
    let start = start_key(next_token)?;
    // `range` from the cursor's key rather than a filtered full scan, so following a
    // cursor to the end of the store costs one walk of it and not one per page. The
    // bound borrows as `&str`, so forming it allocates nothing.
    let from = start.as_deref().unwrap_or("");
    Ok(take_page(
        records
            .range::<str, _>((
                ::std::ops::Bound::Included(from),
                ::std::ops::Bound::Unbounded,
            ))
            .map(|(key, record)| (key.as_str(), record)),
        page_size(max_results),
    ))
}

/// The page of `values` the caller asked for, for a store whose record IS its key — a
/// set of identity-only resources such as the cross-account authorizations.
///
/// Same page size, cursor codec and refusal as [`page`], so the two list surfaces
/// cannot drift apart.
pub fn page_set<'a>(
    values: &'a BTreeSet<String>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<'a, String>, String> {
    let start = start_key(next_token)?;
    let from = start.as_deref().unwrap_or("");
    Ok(take_page(
        values
            .range::<str, _>((
                ::std::ops::Bound::Included(from),
                ::std::ops::Bound::Unbounded,
            ))
            .map(|value| (value.as_str(), value)),
        page_size(max_results),
    ))
}

/// How many records one page carries: what the caller asked for, or
/// [`DEFAULT_MAX_RESULTS`].
fn page_size(max_results: Option<i32>) -> usize {
    match max_results {
        // PURELY DEFENSIVE: `MaxResults` carries the model's `range 1..1000`, so the
        // boundary validator has already refused anything outside it and this clamp
        // cannot fire. It is here so a page size can never be zero (which would make
        // the cursor non-terminating) if the member's constraint ever changes.
        Some(n) => n.clamp(MAX_RESULTS_MIN, MAX_RESULTS_MAX) as usize,
        None => DEFAULT_MAX_RESULTS,
    }
}

/// The inclusive key the page starts at, or `None` for the first page. `Err` carries
/// [`INVALID_PAGINATION_TOKEN_MESSAGE`] for a token this service did not mint — the
/// constant sentence, which does not echo the rejected token back.
fn start_key(next_token: Option<&str>) -> Result<Option<String>, String> {
    match next_token {
        Some(token) => Ok(Some(
            decode_cursor(token).ok_or_else(|| INVALID_PAGINATION_TOKEN_MESSAGE.to_string())?,
        )),
        None => Ok(None),
    }
}

/// One page off an ordered `(key, record)` walk that already starts at the cursor: the
/// first `page_size` records, and the cursor pointing at the record after them.
fn take_page<'a, V>(
    entries: impl Iterator<Item = (&'a str, &'a V)>,
    page_size: usize,
) -> Page<'a, V> {
    let mut page = Page {
        records: Vec::new(),
        next_token: None,
    };
    for (key, record) in entries {
        if page.records.len() == page_size {
            page.next_token = Some(encode_cursor(key));
            break;
        }
        page.records.push(record);
    }
    page
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53recoveryreadinessRoute53recoveryreadinessState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53recoveryreadinessRoute53recoveryreadinessState>)
}
