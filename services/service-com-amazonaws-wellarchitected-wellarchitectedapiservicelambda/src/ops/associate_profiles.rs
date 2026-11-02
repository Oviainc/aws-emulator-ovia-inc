// SPDX-License-Identifier: CC0-1.0

//! @op AssociateProfiles
//! Associate a profile with a workload.
//!
//! The association is held on the WORKLOAD — `Workload$Profiles`, the same list
//! `DisassociateProfiles` removes from and every read of the workload and of its lens
//! reviews reports — so this operation resolves the workload and every profile its
//! `ProfileArns` names against the store, and records one association per named profile,
//! pinned at the version that profile is at when it is associated.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `AssociateProfiles`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! Associate a profile with a workload.
//!
//! ## Input
//!
//! * `ProfileArns` (ProfileArns, len 1..) **required** — The list of profile ARNs to
//!   associate with the workload.
//! * `WorkloadId` (WorkloadId, len 32..32, pattern ^[0-9a-f]{32}$) **required** — The ID
//!   assigned to the workload. This ID is unique within an Amazon Web Services Region.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — User does not have sufficient access to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — The resource has already been processed, was deleted,
//!   or is too large.
//! * `InternalServerException` (HTTP 500) — There is a problem with the Well-Architected Tool
//!   API service.
//! * `ResourceNotFoundException` (HTTP 404) — The requested resource was not found.
//! * `ThrottlingException` (HTTP 429) — Request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The user input is not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `AssociateProfilesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

#[derive(Debug, Clone, Default)]
pub struct AssociateProfilesOutput;

impl AssociateProfilesOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "PATCH",
        "/workloads/{WorkloadId}/associateProfiles",
        Some(200u16),
    ));
}

pub static ASSOCIATE_PROFILES_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &AssociateProfilesOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for AssociateProfilesOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AssociateProfilesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#AssociateProfiles")]
#[derive(Debug)]
pub enum AssociateProfilesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::associate_profiles::AssociateProfilesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AssociateProfilesError = crate::scaffolded::Scaffolded<AssociateProfilesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AssociateProfilesErrorModeled> for AssociateProfilesError {
    fn from(modeled: AssociateProfilesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AssociateProfiles` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AssociateProfiles<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AssociateProfiles<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AssociateProfiles<Eng>
{
    type Input = AssociateProfilesInput;
    type Output = AssociateProfilesOutput;
    type Error = AssociateProfilesError;

    const NAME: &'static str = "AssociateProfiles";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_PROFILES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = ASSOCIATE_PROFILES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(|| {
            AssociateProfilesErrorModeled::InternalServerException(InternalServerException {
                message: "the Well-Architected Tool workload store is unavailable".to_string(),
            })
        })?;

        // The instant this request is served at: what the workload's `UpdatedAt` records
        // below, read once so the record carries one reading of the clock.
        let now = std::time::SystemTime::now();

        // The list is caller-chosen and `ProfileArns` publishes no modelled maximum, so its
        // LENGTH is checked first — one comparison, before any per-entry work and before the
        // shared store lock is taken — and an oversized request is refused without being
        // walked.
        if input.profile_arns.len() > crate::state::MAX_PROFILE_ARNS_PER_REQUEST {
            return Err(AssociateProfilesErrorModeled::ValidationException(
                crate::state::too_many_profile_arns_exception(input.profile_arns.len()),
            )
            .into());
        }

        // The ARNs the request named, each kept once and in the order it first named them:
        // the ARN identifies the profile on its own, so a list naming one twice asks for
        // ONE association, and what the modelled `WorkloadProfiles` bound counts is the
        // distinct profiles the workload would end up reviewed against. A SET decides
        // distinctness, so the walk costs one lookup per entry over a list the clamp above
        // has already bounded, and it stops at the first distinct ARN past
        // `MAX_PROFILES_PER_WORKLOAD` — everything a workload could hold is already
        // collected by then, and the request is refused below whatever follows.
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let mut named: Vec<&str> =
            Vec::with_capacity(crate::state::MAX_PROFILES_PER_WORKLOAD + 1);
        let mut names_more_than_one_workload_holds = false;
        for arn in &input.profile_arns {
            if !seen.insert(arn.as_str()) {
                continue;
            }
            if named.len() == crate::state::MAX_PROFILES_PER_WORKLOAD {
                names_more_than_one_workload_holds = true;
                break;
            }
            named.push(arn.as_str());
        }

        let mut store = state.lock();
        // "Associate a profile with a workload": the profiles are a member OF the workload,
        // so the workload has to exist to hold any — resolved first, before any ARN the
        // request named, so a caller whose workload is gone is not told its profile is.
        // Read immutably here; the write below takes the mutable borrow once every refusal
        // has been decided.
        let workload = store.workloads.get(&input.workload_id).ok_or_else(|| {
            AssociateProfilesErrorModeled::ResourceNotFoundException(
                crate::state::workload_not_found(&input.workload_id),
            )
        })?;

        // `WorkloadProfiles` is `@length(max: 1)`, so a workload cannot report more
        // associations than that: a request that would leave it holding more asks for a
        // result the API cannot carry, and is refused before a single profile is resolved
        // rather than applied in part. A workload already reviewed against a profile
        // collides with the DIFFERENT profile this request names — the caller has to
        // disassociate the one it holds first — while a request naming several profiles is
        // refusable on its own terms, so the two are answered apart.
        if names_more_than_one_workload_holds {
            return Err(over_full(&input.workload_id, workload));
        }

        // Every named profile is resolved against the stored profiles BEFORE the first
        // association is recorded, so a request naming one profile that exists and one that
        // does not changes nothing: a half-applied association would leave the caller unable
        // to tell which half landed. The walk is over the distinct ARNs collected above, so
        // it costs at most `MAX_PROFILES_PER_WORKLOAD` lookups however long the request was.
        // The version each association is PINNED at is the one the profile is at now —
        // `WorkloadProfile` carries `ProfileVersion` beside the ARN, and
        // `UpgradeProfileVersion` is what later moves a workload to a newer one — so it is
        // read here rather than resolved again on every read of the workload.
        let mut pinned: Vec<(&str, String)> =
            Vec::with_capacity(crate::state::MAX_PROFILES_PER_WORKLOAD);
        for arn in &named {
            // A profile the workload ALREADY holds is not associated again, and its pinned
            // version is left exactly as it stood: the association is keyed by the ARN, so
            // associating it a second time asks for the state it is already in.
            if workload.associates_profile(arn) {
                continue;
            }
            let profile = store.profiles.get(*arn).ok_or_else(|| {
                AssociateProfilesErrorModeled::ResourceNotFoundException(
                    crate::state::profile_not_found(arn),
                )
            })?;
            pinned.push((arn, profile.profile_version.clone()));
        }

        // The same bound again, now against what the workload ALREADY holds: a request
        // naming one profile a workload with its slot filled by another is over full too.
        if workload.profiles.len() + pinned.len() > crate::state::MAX_PROFILES_PER_WORKLOAD {
            return Err(over_full(&input.workload_id, workload));
        }

        // Nothing is written for a request that only re-associates what the workload
        // already holds: that request asks for no change, so the workload — `UpdatedAt`
        // included — is left as it stands.
        if pinned.is_empty() {
            return Ok(AssociateProfilesOutput);
        }

        let workload = store.workloads.get_mut(&input.workload_id).ok_or_else(|| {
            AssociateProfilesErrorModeled::ResourceNotFoundException(
                crate::state::workload_not_found(&input.workload_id),
            )
        })?;
        for (arn, version) in &pinned {
            workload.associate_profile(arn, version);
        }
        // The profiles are a member of the workload (`Workload$Profiles`), so associating
        // one changes the workload, and `Workload$UpdatedAt` — "the date and time recorded"
        // — is what every read of the workload reports that change by.
        workload.updated_at = now;

        Ok(AssociateProfilesOutput)
    }
}

/// The refusal for a request that would leave `workload` reviewed against more profiles
/// than `crate::state::MAX_PROFILES_PER_WORKLOAD`.
///
/// A workload whose slot is already filled is a CONFLICT with the association it holds —
/// which the message names, so the caller knows what to disassociate — while a workload
/// holding none was simply asked for more than one profile, which is a bad request. One
/// place decides which, so the two checks above cannot word it differently.
fn over_full(
    workload_id: &str,
    workload: &crate::state::WorkloadRecord,
) -> AssociateProfilesError {
    match workload.profiles.first() {
        Some(existing) => AssociateProfilesErrorModeled::ConflictException(
            crate::state::workload_profile_conflict(workload_id, &existing.profile_arn),
        ),
        None => AssociateProfilesErrorModeled::ValidationException(
            crate::state::too_many_profiles_exception(),
        ),
    }
    .into()
}
