// SPDX-License-Identifier: CC0-1.0

//! @op ListAccessPreviewFindings
//!
//! Reports the findings of the access preview the request names, COMPUTED from the
//! proposed configuration `CreateAccessPreview` stored for it: a finding is the external
//! access one proposed policy statement would allow once the configuration is deployed.
//!
//! Nothing is stored for a finding. Every finding is derived from the stored
//! [`crate::state::AccessPreviewRecord`] on each read, so this operation writes nothing
//! and two reads of one preview report the same findings with the same ids — the id is
//! DERIVED from what it identifies ([`crate::state::derived_uuid`]) rather than minted,
//! which is what makes it stable without a stored table of ids.
//!
//! ## What a finding is
//!
//! An access preview answers "what external access would this configuration allow": the
//! analysis walks every policy the proposed configuration carries (the resource's own
//! policy, a KMS key policy, an S3 access-point policy — [`crate::state::proposed_policies`])
//! with the crate's SHARED IAM policy parser,
//! [`crate::ops::check_no_new_access::policy`], and reports one finding per principal
//! OUTSIDE the zone of trust that an `Allow` statement grants access to. For an account
//! analyzer the zone of trust is the account that owns the previewed resource
//! ([`crate::state::resource_owner_account`]), so:
//!
//! * a statement that admits every principal (`"Principal": "*"`, or an `Allow` written
//!   with `NotPrincipal`) contributes ONE finding for the `*` principal, marked
//!   `isPublic` exactly when nothing in its `Condition` binds the caller — the same
//!   public-access rule `CheckNoPublicAccess` answers with
//!   ([`crate::ops::check_no_public_access::grants_public_access`]), so the two cannot
//!   drift apart;
//! * a statement naming an `AWS` principal in ANOTHER account contributes one finding per
//!   such principal, and a `Service`, `Federated` or `CanonicalUser` principal is outside
//!   the account's own principals, so it contributes one too;
//! * a statement naming only principals of the owning account contributes NONE, which is
//!   what "no external access" means — the findings list is then empty, not absent;
//! * a `Deny` statement grants nothing, so it contributes nothing.
//!
//! Each finding carries the statement's own `Action` list and `Condition` keys, because
//! those are what the external principal would be allowed to do and under which
//! constraints, and `changeType` `NEW` with `status` `ACTIVE`: the emulator holds no
//! findings from a completed analysis of a deployed resource, so no preview finding can
//! be an existing one (`existingFindingId`/`existingFindingStatus` are reported only
//! "for existing findings").
//!
//! ## What this analysis does NOT reason about
//!
//! Deliberately, and always in the direction of reporting access rather than hiding it:
//!
//! * `Deny` statements are not evaluated as restrictions on an `Allow`, and `Action` /
//!   `Resource` are not intersected with the previewed resource's own actions — the same
//!   two limits [`crate::ops::check_no_public_access`] documents, for the same reason.
//! * A configuration that shares WITHOUT a policy document — an EBS snapshot's
//!   `userIds`/`groups`, an RDS snapshot's attributes, a KMS grant, an S3 bucket ACL
//!   grant — carries no statement to read an action set out of, so it contributes no
//!   finding; `crate::state::proposed_policies` is the record of which configuration
//!   members carry a policy at all.
//! * Archive rules are not applied to preview findings, so a finding's `status` is never
//!   `ARCHIVED` here (`ApplyArchiveRule` is itself still scaffolded).
//! * A stored policy the shared parser cannot read is reported as ONE finding carrying
//!   the modelled `error` member, never as "no external access": the analysis failed for
//!   that document and says so.
//!
//! ## What bounds the work
//!
//! A preview's findings are not stored, so EVERY call re-derives them from the stored
//! configuration and then matches them against the request's `filter`. The policy parser
//! bounds each factor of that work (document bytes, statements, element values) but not
//! their product, and it is the product that decides how much there is to do — so the
//! whole analysis is bounded: at most [`MAX_DERIVED_FINDINGS`] findings, at most
//! [`MAX_FILTER_CRITERIA`] criteria, and at most [`MAX_ANALYSIS_UNITS`] units of work
//! charged for every statement visited, principal considered, string copied into a
//! finding and [`FILTER_BYTES_PER_UNIT`] bytes compared by a criterion — so the
//! ALLOCATION and the byte-level work of both stages are capped, not just the size of
//! each input. A call past any of those ceilings is refused with the modelled
//! `ValidationException` naming the member to change (`accessPreviewId` or `filter`)
//! rather than run. Both stages run on the blocking pool with the state lock already
//! released, exactly as `CheckNoNewAccess` runs its policy comparison, so a call at the
//! top of its budget cannot starve the async worker or hold up another request; a
//! criterion copies nothing, since it reads a finding's values as borrows.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
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
//! # Modeled contract for `ListAccessPreviewFindings`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json")
//! ```
//!
//! Retrieves a list of access preview findings generated by the specified access preview.
//!
//! ## Input
//!
//! * `accessPreviewId` (AccessPreviewId, pattern
//!   ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$) **required** — The
//!   unique ID for the access preview.
//! * `analyzerArn` (AnalyzerArn, pattern ^[^:]*:[^:]*:[^:]*:[^:]*:[^:]*:analyzer/.{1,255}$)
//!   **required** — The ARN of the analyzer used to generate the access.
//! * `filter` (FilterCriteriaMap) — Criteria to filter the returned findings.
//! * `maxResults` (Integer) — The maximum number of results to return in the response.
//! * `nextToken` (Token) — A token used for pagination of results returned.
//!
//! ## Output
//!
//! * `findings` (AccessPreviewFindingsList) **required** — A list of access preview findings
//!   that match the specified filter criteria.
//! * `nextToken` (Token) — A token used for pagination of results returned.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action.
//! * `ConflictException` (HTTP 409) — A conflict exception error.
//! * `InternalServerException` (HTTP 500) — Internal server error.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found.
//! * `ThrottlingException` (HTTP 429) — Throttling limit exceeded error.
//! * `ValidationException` (HTTP 400) — Validation exception error.

use std::collections::HashMap;

// The analysis a preview shares with the findings of a DEPLOYED resource: one budget type,
// one refusal type, and the ONE rule for which principals a statement grants external
// access to, so the two analyses cannot drift apart.
use crate::state::{
    external_grants, granted_actions, granted_conditions, principal_account, Budget, Exhausted,
    ServicePrincipals,
};

#[allow(unused_imports)]
use super::types::*;

/// The crate's shared IAM policy parser: one grammar, one set of element rules and one
/// set of input ceilings for every policy this service reads.
use super::check_no_new_access::policy;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListAccessPreviewFindingsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAccessPreviewFindingsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.accessanalyzer#ListAccessPreviewFindings")]
#[derive(Debug)]
pub enum ListAccessPreviewFindingsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_access_preview_findings::ListAccessPreviewFindingsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAccessPreviewFindingsError = crate::scaffolded::Scaffolded<ListAccessPreviewFindingsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAccessPreviewFindingsErrorModeled> for ListAccessPreviewFindingsError {
    fn from(modeled: ListAccessPreviewFindingsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAccessPreviewFindings` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAccessPreviewFindings<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAccessPreviewFindings<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAccessPreviewFindings<Eng>
{
    type Input = ListAccessPreviewFindingsInput;
    type Output = ListAccessPreviewFindingsOutput;
    type Error = ListAccessPreviewFindingsError;

    const NAME: &'static str = "ListAccessPreviewFindings";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ACCESS_PREVIEW_FINDINGS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ACCESS_PREVIEW_FINDINGS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // A read: the findings are a function of the stored preview and the clock, so
        // nothing of the request metadata is consulted and nothing is written.
        let _ = ctx;

        // The filter is refused before the store is read: a criterion naming no member of
        // a finding cannot be applied to any finding, whatever the store holds.
        let criteria = filter_criteria(input.filter.as_ref())?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListAccessPreviewFindingsErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
        })?;
        // The analyzer, the preview and its status are read under the state lock; the
        // record is COPIED out of it and the lock RELEASED (the block ends here) before
        // any analysis runs, so deriving the findings never holds this service's state.
        let record = {
            let state = state.lock();

            let analyzer = state.analyzer_by_arn(&input.analyzer_arn).ok_or_else(|| {
                // Fixed text, the ARN the caller named in the modelled `resourceId`: every
                // resource-scoped refusal in this service goes through `crate::state`'s
                // constructors.
                ListAccessPreviewFindingsErrorModeled::ResourceNotFoundException(
                    crate::state::analyzer_not_found(&input.analyzer_arn),
                )
            })?;

            // Previews are stored ON the analyzer record, so an id created against a
            // DIFFERENT analyzer is not found here — which is what the request being
            // scoped by `analyzerArn` means.
            let preview = analyzer
                .access_previews
                .get(&input.access_preview_id)
                .ok_or_else(|| {
                    ListAccessPreviewFindingsErrorModeled::ResourceNotFoundException(
                        crate::state::access_preview_not_found(&input.access_preview_id),
                    )
                })?;

            // The findings are the RESULT of the analysis, so there are none to report
            // until the analysis is due; a preview that resolved to `FAILED` produced none
            // at all. Either way the preview exists, so the refusal is the modelled
            // conflict (HTTP 409) rather than a 404 or an empty success.
            let (status, _) = preview.status_at(::std::time::SystemTime::now());
            if status != crate::state::ACCESS_PREVIEW_STATUS_COMPLETED {
                return Err(ListAccessPreviewFindingsErrorModeled::ConflictException(
                    crate::state::access_preview_not_completed(&preview.id),
                )
                .into());
            }

            preview.clone()
        };

        // The whole analysis — deriving the findings from the stored policies AND matching
        // them against the request's criteria — reads documents, allocates findings and
        // compares strings, so it is CPU-bound: it runs on the blocking pool, the way
        // `CheckNoNewAccess` sends its policy comparison there, under ONE budget of
        // [`MAX_ANALYSIS_UNITS`] charged for every statement visited, principal considered,
        // string copied into a finding and byte compared by a criterion. The budget bounds
        // the work and the allocation of both stages together; the pool keeps them off the
        // async worker that serves health checks and every other request. What is left on
        // the worker is taking one page (at most [`crate::state::MAX_PAGE_SIZE`] items)
        // out of the result.
        let analysed = ::tokio::task::spawn_blocking(move || {
            let mut budget = Budget::new(MAX_ANALYSIS_UNITS);
            let findings = preview_findings(&record, &mut budget)?;
            matching_findings(findings, &criteria, &mut budget)
        })
        .await;
        let matching = match analysed {
            Ok(Ok(matching)) => matching,
            // The configuration is analyzable, but reporting its findings would cost more
            // than this emulator spends on one call: refused, naming the request member
            // at fault, rather than allocating further.
            Ok(Err(exhausted)) => {
                ::tracing::info!(
                    operation = Self::NAME,
                    refusal = "budget",
                    "refused an access preview listing past this emulator's analysis ceiling"
                );
                return Err(ListAccessPreviewFindingsErrorModeled::ValidationException(
                    crate::state::field_validation_exception(exhausted.member, exhausted.detail),
                )
                .into());
            }
            // A panic or a cancellation on the pool says nothing about the preview, so it
            // is reported as the server fault it is rather than as an empty result.
            Err(error) => {
                ::tracing::error!(
                    operation = Self::NAME,
                    error = %error,
                    "the access preview analysis did not complete"
                );
                return Err(
                    ListAccessPreviewFindingsErrorModeled::InternalServerException(
                        InternalServerException {
                            message: "the access preview analysis did not complete".to_string(),
                            retry_after_seconds: None,
                        },
                    )
                    .into(),
                );
            }
        };

        let page = crate::state::page(matching, input.max_results, input.next_token.as_deref())
            .map_err(|error| {
                ListAccessPreviewFindingsErrorModeled::ValidationException(
                    crate::state::page_validation_exception(&error),
                )
            })?;

        Ok(ListAccessPreviewFindingsOutput {
            findings: page.items,
            next_token: page.next_token,
        })
    }
}

// ---------------------------------------------------------------------------
// Deriving the findings of a stored access preview
//
// Every value below is computed from the stored proposed configuration: the policies it
// carries, the principals those policies name and the account that owns the previewed
// resource. Nothing is keyed to a particular document, principal or resource type.
// ---------------------------------------------------------------------------

/// The `FindingStatus` a newly previewed external-access finding is in: the status the
/// finding WOULD have after the proposed permissions are deployed, which for access that
/// does not exist yet and matches no archive rule is `ACTIVE`. One of the model's
/// [`FINDING_STATUS_VALUES`].
const FINDING_STATUS_ACTIVE: &str = "ACTIVE";

/// The `FindingChangeType` of a preview finding the emulator reports: `NEW`, "the finding
/// is for newly-introduced access". The emulator holds no findings for deployed
/// resources, so no preview finding can be `UNCHANGED` or `CHANGED` — those compare a
/// preview finding against an existing one. One of [`FINDING_CHANGE_TYPE_VALUES`].
const FINDING_CHANGE_TYPE_NEW: &str = "NEW";

/// `FindingSourceType` values ([`FINDING_SOURCE_TYPE_VALUES`]): the access was granted by
/// the resource's own policy, by an access point's policy, or by an access point owned by
/// another account.
const FINDING_SOURCE_TYPE_POLICY: &str = "POLICY";
const FINDING_SOURCE_TYPE_S3_ACCESS_POINT: &str = "S3_ACCESS_POINT";
const FINDING_SOURCE_TYPE_S3_ACCESS_POINT_ACCOUNT: &str = "S3_ACCESS_POINT_ACCOUNT";

/// The request member a policy the STORE holds is blamed on, for the one error text a
/// finding can carry: the stored document came in as `configurations`.
const CONFIGURATIONS_MEMBER: &str = "configurations";

/// The request member that names the preview a refusal is about.
const ACCESS_PREVIEW_ID_MEMBER: &str = "accessPreviewId";

/// The request member a filter refusal names.
const FILTER_MEMBER: &str = "filter";

/// How many findings this emulator will derive for ONE access preview.
///
/// A preview's findings are not stored, so every call re-derives them: this is the
/// emulator's OWN ceiling on that work, generous beside the ten full pages
/// ([`crate::state::MAX_PAGE_SIZE`]) a realistic listing walks, and far below what a
/// stored configuration could otherwise ask for — the policy parser bounds each factor
/// (documents, statements, element values) but not their PRODUCT, and it is the product
/// that decides how many findings exist.
const MAX_DERIVED_FINDINGS: usize = 1_000;

/// How much work ONE call's analysis may spend: deriving the findings AND matching them
/// against the request's criteria, under one ceiling.
///
/// A unit is one policy visited, one statement visited, one principal considered, one
/// string copied into a finding, or [`FILTER_BYTES_PER_UNIT`] bytes compared by a filter
/// criterion — so this bounds the ALLOCATION and the byte-level work of both stages, in
/// the PRODUCT of their factors rather than in each factor alone, whatever shape the
/// stored configuration and the request's filter have. The counterpart of the policy
/// engine's own budgets ([`policy::MAX_ACCESS_REQUESTS`],
/// [`policy::MAX_COMPARISON_STEPS`]), which bound the comparison `CheckNoNewAccess` runs.
const MAX_ANALYSIS_UNITS: u64 = 500_000;

/// How many bytes of string comparison one filter unit buys. A criterion is checked by
/// comparing a finding's value against a value the request supplied, so what the check
/// costs is their LENGTH, not just the number of pairs — charging per quantum of bytes is
/// what keeps one long value from buying unbounded work.
const FILTER_BYTES_PER_UNIT: usize = 64;

/// How many criteria one `filter` may carry.
///
/// A criterion names a member of a finding, and the model gives a finding sixteen of them
/// (a map-valued one addressable at each of its keys), so a filter past this ceiling is
/// not a finer query but a multiplier on the match: every criterion is checked against
/// every finding. The emulator's own ceiling, in the same spirit as the model's
/// `@length(max: 20)` on each criterion's `ValueList`.
const MAX_FILTER_CRITERIA: usize = 20;

/// The preview grants more external access than this emulator reports findings for.
fn too_many_findings() -> Exhausted {
    Exhausted {
        member: ACCESS_PREVIEW_ID_MEMBER,
        detail: format!(
            "the proposed configuration of this access preview grants external access to more \
             than the {MAX_DERIVED_FINDINGS} principals this emulator reports findings for"
        ),
    }
}

/// Deriving the preview's findings takes more work than one call spends.
fn derivation_exhausted() -> Exhausted {
    Exhausted {
        member: ACCESS_PREVIEW_ID_MEMBER,
        detail: format!(
            "deriving the findings of this access preview takes more than the \
             {MAX_ANALYSIS_UNITS} steps this emulator spends on one call"
        ),
    }
}

/// Matching the request's criteria against those findings takes more work than one call
/// spends — so it is the `filter` the caller has to narrow.
fn filter_exhausted() -> Exhausted {
    Exhausted {
        member: FILTER_MEMBER,
        detail: format!(
            "matching this filter against the findings of this access preview takes more than \
             the {MAX_ANALYSIS_UNITS} steps this emulator spends on one call"
        ),
    }
}

/// What `policy::parse` is told to blame. The stored document's own position inside the
/// configuration is more precise than any request member, so the member a finding's
/// `error` names is composed below from [`crate::state::ProposedPolicy::member`]; this is
/// the `&'static str` the parser's signature requires.
const POLICY_MEMBER: &str = CONFIGURATIONS_MEMBER;

/// Every finding the preview's stored configurations produce, each paired with the key
/// the listing is ordered and paged by.
///
/// The key is the finding's position in the analysis — resource, then the configuration
/// member the policy sits at, then the statement, then the principal — so the order is
/// deterministic, independent of how the configuration map happens to be laid out, and a
/// continuation token keeps naming the same place when the same preview is listed again.
/// Bounded in the PRODUCT of its factors, not just in each of them: every statement
/// visited, principal considered and string copied into a finding is charged to a
/// [`Budget`], and the finding count to [`MAX_DERIVED_FINDINGS`], so the work and the
/// allocation one call can drive are capped whatever the stored configuration looks like.
/// `Err` is the refusal text for a derivation past either ceiling.
fn preview_findings(
    preview: &crate::state::AccessPreviewRecord,
    budget: &mut Budget,
) -> Result<Vec<(String, AccessPreviewFinding)>, Exhausted> {
    // The analysis produced the findings when it completed, which is also the instant the
    // preview starts reporting them: a stored instant, so every read reports the same one.
    let created_at = ::aws_smithy_types::DateTime::from(preview.completes_at);

    // `configurations` is a map, so the walk is ordered by the resource ARN it keys.
    let mut configurations: Vec<(&String, &Configuration)> = preview.configurations.iter().collect();
    configurations.sort_by(|left, right| left.0.cmp(right.0));

    let mut findings: Vec<(String, AccessPreviewFinding)> = Vec::new();
    for (resource, configuration) in configurations {
        let resource_type = crate::state::configuration_resource_type(configuration);
        // For an account analyzer the zone of trust is the account that owns the resource
        // being previewed; `CreateAccessPreview` only accepts an `ACCOUNT` analyzer.
        let owner_account = crate::state::resource_owner_account(resource, None);

        for proposed in crate::state::proposed_policies(configuration) {
            if !budget.spend(1) {
                return Err(derivation_exhausted());
            }
            let sources = policy_sources(&proposed, &owner_account);
            let template = AccessPreviewFinding {
                id: String::new(),
                existing_finding_id: None,
                existing_finding_status: None,
                principal: None,
                action: None,
                condition: None,
                resource: Some(resource.clone()),
                is_public: None,
                resource_type: resource_type.to_string(),
                created_at,
                change_type: FINDING_CHANGE_TYPE_NEW.to_string(),
                status: FINDING_STATUS_ACTIVE.to_string(),
                resource_owner_account: owner_account.clone(),
                error: None,
                sources: sources.clone(),
                resource_control_policy_restriction: None,
            };

            match policy::parse(proposed.document, POLICY_MEMBER, policy::Flavor::Resource) {
                Err(error) => {
                    // The analysis of THIS document failed. Reported as a finding
                    // carrying the modelled `error` member — which is what that member is
                    // for — rather than as silence, which would claim the document grants
                    // no external access.
                    let key = format!("{resource}\u{1f}{}\u{1f}", proposed.member);
                    findings.push((
                        key.clone(),
                        AccessPreviewFinding {
                            id: crate::state::derived_uuid(&[&preview.id, &key]),
                            error: Some(analysis_error(&proposed.member, &error)),
                            ..template
                        },
                    ));
                }
                Ok(parsed) => {
                    for statement in &parsed.statements {
                        // One unit for the statement, and one per principal its element
                        // names — the principals this statement will be walked over.
                        let named = statement
                            .principals
                            .as_ref()
                            .map_or(0, |principals| principals.values.len());
                        if !budget.spend(1 + named as u64) {
                            return Err(derivation_exhausted());
                        }
                        let actions = granted_actions(statement);
                        let condition = granted_conditions(statement);
                        // What one finding of this statement COPIES: its principal, its
                        // action list and its condition map. Charged per finding, so the
                        // strings the derivation allocates are bounded by the budget
                        // rather than by the product of the parser's per-element caps.
                        let per_finding = 1
                            + actions.as_ref().map_or(0, Vec::len) as u64
                            + condition.as_ref().map_or(0, HashMap::len) as u64;
                        for grant in external_grants(
                            statement,
                            &owner_account,
                            // An access preview reports a service principal as the
                            // external access it is asked about; see `ServicePrincipals`.
                            ServicePrincipals::OutsideZoneOfTrust,
                        ) {
                            if findings.len() >= MAX_DERIVED_FINDINGS {
                                return Err(too_many_findings());
                            }
                            if !budget.spend(per_finding) {
                                return Err(derivation_exhausted());
                            }
                            let key = format!(
                                "{resource}\u{1f}{}\u{1f}{:08}\u{1f}{}\u{1f}{}",
                                proposed.member,
                                statement.index,
                                grant.kind.as_str(),
                                grant.value
                            );
                            findings.push((
                                key.clone(),
                                AccessPreviewFinding {
                                    id: crate::state::derived_uuid(&[&preview.id, &key]),
                                    principal: Some(HashMap::from([(
                                        grant.kind.as_str().to_string(),
                                        grant.value.clone(),
                                    )])),
                                    action: actions.clone(),
                                    condition: condition.clone(),
                                    is_public: Some(grant.is_public),
                                    ..template.clone()
                                },
                            ));
                        }
                    }
                }
            }
        }
    }
    findings.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(findings)
}

/// How the access a finding reports is granted, for the resource types the model says
/// `sources` is populated for.
///
/// "It is populated for Amazon S3 bucket findings", so only a bucket's findings carry it:
/// the bucket's own policy is a `POLICY` source, and a policy proposed by one of its
/// access points is an `S3_ACCESS_POINT` source naming that access point —
/// `S3_ACCESS_POINT_ACCOUNT` when the access point is owned by another account, which is
/// what the finding's `accessPointAccount` ("the account of the cross-account access
/// point that generated the finding") reports.
fn policy_sources(
    proposed: &crate::state::ProposedPolicy<'_>,
    owner_account: &str,
) -> Option<Vec<FindingSource>> {
    if !proposed.member.starts_with("s3Bucket.")
        && !proposed.member.starts_with("s3ExpressDirectoryBucket.")
    {
        return None;
    }
    let Some(access_point_arn) = proposed.access_point_arn else {
        return Some(vec![FindingSource {
            r#type: FINDING_SOURCE_TYPE_POLICY.to_string(),
            detail: None,
        }]);
    };
    let access_point_account =
        principal_account(access_point_arn).filter(|account| *account != owner_account);
    Some(vec![FindingSource {
        r#type: if access_point_account.is_some() {
            FINDING_SOURCE_TYPE_S3_ACCESS_POINT_ACCOUNT.to_string()
        } else {
            FINDING_SOURCE_TYPE_S3_ACCESS_POINT.to_string()
        },
        detail: Some(FindingSourceDetail {
            access_point_arn: Some(access_point_arn.to_string()),
            access_point_account: access_point_account.map(str::to_string),
        }),
    }])
}

/// The `error` a finding carries when the analysis of one stored document failed: which
/// document, and what the shared parser could not read in it.
fn analysis_error(member: &str, error: &policy::EngineError) -> String {
    let detail = match error {
        policy::EngineError::Unparseable { detail, .. } => {
            format!("it is not a JSON document ({detail})")
        }
        policy::EngineError::Invalid { detail, .. } => {
            format!("it is not a resource policy ({detail})")
        }
        policy::EngineError::Unsupported { detail } => detail.clone(),
    };
    format!("the policy proposed at {member} could not be analyzed: {detail}")
}

// ---------------------------------------------------------------------------
// `filter`: the criteria a returned finding must satisfy
// ---------------------------------------------------------------------------

/// Which member of a finding one filter key reads.
///
/// A criterion's key names a member of `AccessPreviewFinding`, so the catalog below is
/// the model's own member list (`smithy_query(query="shape", shape="AccessPreviewFinding")`)
/// rather than a list of accepted spellings: a map-valued member is addressed either as a
/// whole or by one of its keys (`principal.AWS`, `condition.aws:PrincipalOrgID`), and the
/// `sources` list by the member of its entries (`sources.type`).
#[derive(Debug)]
enum FindingMember {
    /// A member holding at most one value of its own, read as a borrow of the finding.
    Scalar(for<'a> fn(&'a AccessPreviewFinding) -> Option<&'a str>),
    /// `action`: the actions the statement allows.
    Action,
    /// `principal`, as a whole or at one principal type.
    Principal(Option<String>),
    /// `condition`, as a whole or at one condition key.
    Condition(Option<String>),
    /// `sources.type`: how the access is granted.
    SourceType,
    /// `sources.detail.accessPointArn`.
    SourceAccessPointArn,
    /// `sources.detail.accessPointAccount`.
    SourceAccessPointAccount,
}

/// The finding members a criterion may read, each with the member name the wire spells it
/// by. Written out so a key naming no member is refused rather than silently matching
/// nothing.
const FILTERABLE_MEMBERS: &[&str] = &[
    "id",
    "existingFindingId",
    "existingFindingStatus",
    "principal",
    "action",
    "condition",
    "resource",
    "isPublic",
    "resourceType",
    "changeType",
    "status",
    "resourceOwnerAccount",
    "error",
    "sources.type",
    "sources.detail.accessPointArn",
    "sources.detail.accessPointAccount",
];

impl FindingMember {
    /// The member `key` addresses, or `None` when it addresses none.
    fn parse(key: &str) -> Option<Self> {
        if let Some(entry) = key.strip_prefix("principal.") {
            return Some(Self::Principal(Some(entry.to_string())));
        }
        if let Some(entry) = key.strip_prefix("condition.") {
            return Some(Self::Condition(Some(entry.to_string())));
        }
        match key {
            "id" => Some(Self::Scalar(|finding| Some(finding.id.as_str()))),
            "existingFindingId" => Some(Self::Scalar(|finding| {
                finding.existing_finding_id.as_deref()
            })),
            "existingFindingStatus" => Some(Self::Scalar(|finding| {
                finding.existing_finding_status.as_deref()
            })),
            "resource" => Some(Self::Scalar(|finding| finding.resource.as_deref())),
            // The two spellings a boolean member is filtered by are fixed, so reading it
            // borrows one of them rather than rendering a string per comparison.
            "isPublic" => Some(Self::Scalar(|finding| {
                finding
                    .is_public
                    .map(|public| if public { "true" } else { "false" })
            })),
            "resourceType" => Some(Self::Scalar(|finding| Some(finding.resource_type.as_str()))),
            "changeType" => Some(Self::Scalar(|finding| Some(finding.change_type.as_str()))),
            "status" => Some(Self::Scalar(|finding| Some(finding.status.as_str()))),
            "resourceOwnerAccount" => Some(Self::Scalar(|finding| {
                Some(finding.resource_owner_account.as_str())
            })),
            "error" => Some(Self::Scalar(|finding| finding.error.as_deref())),
            "action" => Some(Self::Action),
            "principal" => Some(Self::Principal(None)),
            "condition" => Some(Self::Condition(None)),
            "sources.type" => Some(Self::SourceType),
            "sources.detail.accessPointArn" => Some(Self::SourceAccessPointArn),
            "sources.detail.accessPointAccount" => Some(Self::SourceAccessPointAccount),
            _ => None,
        }
    }

    /// The values this member holds on `finding`, BORROWED from it: empty when the
    /// finding does not carry the member, which is what `exists` reads.
    ///
    /// Borrowed rather than cloned, so checking a criterion against a finding copies no
    /// string at all — the cost of a check is the bytes it compares, which is what the
    /// [`Budget`] is charged for.
    fn values<'a>(&self, finding: &'a AccessPreviewFinding) -> Vec<&'a str> {
        /// One entry of a map-valued member, or every entry when no key is named.
        fn entries<'a>(
            map: Option<&'a HashMap<String, String>>,
            at: Option<&String>,
        ) -> Vec<&'a str> {
            let Some(map) = map else {
                return Vec::new();
            };
            match at {
                Some(key) => map
                    .get(key.as_str())
                    .map(String::as_str)
                    .into_iter()
                    .collect(),
                None => map.values().map(String::as_str).collect(),
            }
        }
        /// One member of every finding source.
        fn of_sources<'a>(
            finding: &'a AccessPreviewFinding,
            member: impl Fn(&'a FindingSource) -> Option<&'a str>,
        ) -> Vec<&'a str> {
            finding
                .sources
                .iter()
                .flatten()
                .filter_map(member)
                .collect()
        }
        match self {
            Self::Scalar(read) => read(finding).into_iter().collect(),
            Self::Action => finding
                .action
                .iter()
                .flatten()
                .map(String::as_str)
                .collect(),
            Self::Principal(at) => entries(finding.principal.as_ref(), at.as_ref()),
            Self::Condition(at) => entries(finding.condition.as_ref(), at.as_ref()),
            Self::SourceType => of_sources(finding, |source| Some(source.r#type.as_str())),
            Self::SourceAccessPointArn => of_sources(finding, |source| {
                source
                    .detail
                    .as_ref()
                    .and_then(|detail| detail.access_point_arn.as_deref())
            }),
            Self::SourceAccessPointAccount => of_sources(finding, |source| {
                source
                    .detail
                    .as_ref()
                    .and_then(|detail| detail.access_point_account.as_deref())
            }),
        }
    }
}

/// One criterion of the request's `filter`: the finding member it reads, and the
/// comparisons that member's values must satisfy.
#[derive(Debug)]
struct FilterCriterion {
    member: FindingMember,
    criterion: Criterion,
}

impl FilterCriterion {
    /// Whether `finding` satisfies this criterion, charging the comparison to `budget`.
    ///
    /// The operators of one `Criterion` are a CONJUNCTION — a criterion carrying `eq` and
    /// `contains` demands both — and the values of one operator are a disjunction, which
    /// is how the model documents them ("an \"equals\" operator to match"). A member
    /// holding several values (`action`, a whole `principal` map) satisfies a positive
    /// operator when ANY of its values does, and a negative one only when NONE does.
    ///
    /// `Err` once the budget is spent: the match is refused rather than run to the end.
    fn holds_for(
        &self,
        finding: &AccessPreviewFinding,
        budget: &mut Budget,
    ) -> Result<bool, Exhausted> {
        let held = self.member.values(finding);
        // Reading the member's values is charged for too, so a member holding many of
        // them costs more than one holding none even before a comparison happens.
        if !budget.spend(1 + held.len() as u64) {
            return Err(filter_exhausted());
        }
        if let Some(expected) = &self.criterion.eq {
            if !any_pair(&held, expected, budget, |value, expected| value == expected)? {
                return Ok(false);
            }
        }
        if let Some(rejected) = &self.criterion.neq {
            if any_pair(&held, rejected, budget, |value, rejected| value == rejected)? {
                return Ok(false);
            }
        }
        if let Some(needles) = &self.criterion.contains {
            if !any_pair(&held, needles, budget, |value, needle| value.contains(needle))? {
                return Ok(false);
            }
        }
        if let Some(exists) = self.criterion.exists {
            if exists == held.is_empty() {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Whether `accept` holds for any (held value, criterion value) pair, charged to `budget`
/// by the BYTES each comparison may touch — so one long value buys no more work than its
/// length, and the match as a whole is bounded in the product of its factors.
fn any_pair(
    held: &[&str],
    values: &[String],
    budget: &mut Budget,
    accept: impl Fn(&str, &str) -> bool,
) -> Result<bool, Exhausted> {
    for value in held {
        for candidate in values {
            let bytes = value.len().saturating_add(candidate.len());
            if !budget.spend(1 + (bytes / FILTER_BYTES_PER_UNIT) as u64) {
                return Err(filter_exhausted());
            }
            if accept(value, candidate.as_str()) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The findings every criterion holds for, in the order they were derived.
///
/// Runs beside the derivation on the blocking pool and under the SAME [`Budget`], so one
/// call's findings-times-criteria work is capped rather than each factor alone.
fn matching_findings(
    findings: Vec<(String, AccessPreviewFinding)>,
    criteria: &[FilterCriterion],
    budget: &mut Budget,
) -> Result<Vec<(String, AccessPreviewFinding)>, Exhausted> {
    if criteria.is_empty() {
        return Ok(findings);
    }
    let mut matching = Vec::new();
    for (key, finding) in findings {
        let mut holds = true;
        for criterion in criteria {
            if !criterion.holds_for(&finding, budget)? {
                holds = false;
                break;
            }
        }
        if holds {
            matching.push((key, finding));
        }
    }
    Ok(matching)
}

/// Read the request's `filter` into the criteria a returned finding must satisfy.
///
/// A key naming no member of a finding, and a filter carrying more than
/// [`MAX_FILTER_CRITERIA`] criteria, are refused with the operation's own
/// `ValidationException`: it is a field validation, so the message names the member at
/// fault and the constraint it violated, which is what a client has to fix. The criteria
/// are ordered by key so a refusal does not depend on the map's layout.
fn filter_criteria(
    filter: Option<&HashMap<String, Criterion>>,
) -> Result<Vec<FilterCriterion>, ListAccessPreviewFindingsError> {
    let refuse = |message: String| -> ListAccessPreviewFindingsError {
        ListAccessPreviewFindingsErrorModeled::ValidationException(
            crate::state::field_validation_exception(FILTER_MEMBER, message),
        )
        .into()
    };
    let mut keys: Vec<&String> = filter.into_iter().flatten().map(|(key, _)| key).collect();
    if keys.len() > MAX_FILTER_CRITERIA {
        // Every criterion is checked against every finding, so the count is a multiplier
        // on the match and not just a finer query.
        return Err(refuse(format!(
            "filter carries {} criteria, more than the {MAX_FILTER_CRITERIA} this emulator \
             matches against one access preview's findings",
            keys.len()
        )));
    }
    keys.sort();
    let mut criteria = Vec::with_capacity(keys.len());
    for key in keys {
        let member = FindingMember::parse(key).ok_or_else(|| {
            refuse(format!(
                "filter key {key:?} names no member of an access preview finding; \
                 the filterable members are {}, and a map-valued member may also \
                 be addressed at one of its keys (principal.<principal type>, \
                 condition.<condition key>)",
                FILTERABLE_MEMBERS.join(", ")
            ))
        })?;
        criteria.push(FilterCriterion {
            member,
            criterion: filter
                .and_then(|filter| filter.get(key))
                .cloned()
                .unwrap_or_default(),
        });
    }
    Ok(criteria)
}
