// SPDX-License-Identifier: CC0-1.0

//! @op AssociateLenses
//! Associate a lens to a workload.
//!
//! The lenses a workload is reviewed against are held on the WORKLOAD —
//! `Workload$Lenses`, the list `DisassociateLenses` removes from, every read of the
//! workload reports and every lens review of it is derived from — so this operation
//! resolves the workload and every lens its `LensAliases` names against the store, and
//! records one review per distinct lens named that the workload does not already hold.
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
//! # Modeled contract for `AssociateLenses`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! Associate a lens to a workload. Up to 10 lenses can be associated with a workload in a
//! single API operation. A maximum of 20 lenses can be associated with a workload.
//! Disclaimer By accessing and/or applying custom lenses created by another Amazon Web
//! Services user or account, you acknowledge that custom lenses created by other users and
//! shared with you are Third Party Content as defined in the Amazon Web Services Customer
//! Agreement.
//!
//! ## Input
//!
//! * `LensAliases` (LensAliases, len 1..) **required** — List of lens aliases to associate or
//!   disassociate with a workload. Up to 10 lenses can be specified. Identify a lens using
//!   its LensSummary$LensAlias.
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
// `AssociateLensesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

#[derive(Debug, Clone, Default)]
pub struct AssociateLensesOutput;

impl AssociateLensesOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "PATCH",
        "/workloads/{WorkloadId}/associateLenses",
        Some(200u16),
    ));
}

pub static ASSOCIATE_LENSES_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &AssociateLensesOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for AssociateLensesOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AssociateLensesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#AssociateLenses")]
#[derive(Debug)]
pub enum AssociateLensesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::associate_lenses::AssociateLensesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AssociateLensesError = crate::scaffolded::Scaffolded<AssociateLensesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AssociateLensesErrorModeled> for AssociateLensesError {
    fn from(modeled: AssociateLensesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AssociateLenses` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AssociateLenses<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AssociateLenses<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AssociateLenses<Eng>
{
    type Input = AssociateLensesInput;
    type Output = AssociateLensesOutput;
    type Error = AssociateLensesError;

    const NAME: &'static str = "AssociateLenses";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_LENSES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = ASSOCIATE_LENSES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(|| {
            AssociateLensesErrorModeled::InternalServerException(InternalServerException {
                message: "the Well-Architected Tool workload store is unavailable".to_string(),
            })
        })?;

        // The instant this request is served at: what the workload's `UpdatedAt` records
        // below, read once so the record carries one reading of the clock.
        let now = std::time::SystemTime::now();

        // "Up to 10 lenses can be specified" (`LensAliases` doc), which the member itself
        // does not state, so its LENGTH is checked first — one comparison, before any
        // per-entry work and before the shared store lock is taken — and an oversized
        // request is refused without being walked.
        if input.lens_aliases.len() > crate::state::MAX_LENS_ALIASES_PER_REQUEST {
            return Err(AssociateLensesErrorModeled::ValidationException(
                crate::state::too_many_lens_aliases_exception(input.lens_aliases.len()),
            )
            .into());
        }

        // The lenses the request named, each kept once and in the order it first named
        // them, AS SENT: a lens is one lens however it is spelled, so a list naming one
        // twice asks for ONE review, and what the per-workload ceiling counts is the
        // distinct lenses the workload would end up reviewed against. Resolved through the
        // one place that identity rule lives (`crate::state::distinct_lenses`), the same
        // rule the create that mints a workload applies to its `Lenses`. The name as sent
        // is what is recorded, because that is the `LensAlias` every read of the workload
        // reports. The walk is over a list the quota above has already bounded, so it costs
        // a constant number of lookups.
        let named = crate::state::distinct_lenses(&input.lens_aliases);

        let mut store = state.lock();
        // "Associate a lens to a workload": the lenses are a member OF the workload, so the
        // workload has to exist to hold any — resolved first, before any lens the request
        // named, so a caller whose workload is gone is not told its lens is. Read immutably
        // here; the write below takes the mutable borrow once every refusal has been
        // decided.
        let workload = store.workloads.get(&input.workload_id).ok_or_else(|| {
            AssociateLensesErrorModeled::ResourceNotFoundException(
                crate::state::workload_not_found(&input.workload_id),
            )
        })?;

        // Every named lens is resolved against the lens catalog BEFORE the first review is
        // recorded, so a request naming one lens that exists and one that does not changes
        // nothing: a half-applied association would leave the caller unable to tell which
        // half landed. It is the same resolution a create that applies lenses runs, so this
        // operation cannot record a lens another operation would then report a review of
        // and find nothing for.
        match store.unapplicable_lenses_refusal(&named, crate::state::LENS_ALIASES_MEMBER, now) {
            Some(crate::state::LensesRefusal::NotFound(exception)) => {
                return Err(
                    AssociateLensesErrorModeled::ResourceNotFoundException(exception).into(),
                );
            }
            Some(crate::state::LensesRefusal::Unapplicable(exception)) => {
                return Err(AssociateLensesErrorModeled::ValidationException(exception).into());
            }
            None => {}
        }

        // A lens the workload ALREADY reviews is not associated again: the review is keyed
        // by the lens, so associating it a second time asks for the state the workload is
        // already in, and the name the first association recorded is left as it stands.
        let fresh: Vec<String> = named
            .into_iter()
            .filter(|lens| !workload.reviews_lens(lens))
            .collect();

        // "A maximum of 20 lenses can be associated with a workload" (this operation's
        // doc): a request that would leave the workload holding more asks for a workload
        // the service does not serve, so it is refused whole rather than applied in part.
        // What it is counted against is the DISTINCT lenses the workload reviews, which is
        // what a read of its lens reviews reports.
        let attached = workload.lens_reviews().len();
        if attached + fresh.len() > crate::state::MAX_LENSES_ASSOCIATED_PER_WORKLOAD {
            return Err(AssociateLensesErrorModeled::ValidationException(
                crate::state::too_many_lenses_associated_exception(&input.workload_id, attached),
            )
            .into());
        }

        // Nothing is written for a request that only re-associates lenses the workload
        // already reviews: that request asks for no change, so the workload — `UpdatedAt`
        // included — is left as it stands.
        if fresh.is_empty() {
            return Ok(AssociateLensesOutput);
        }

        // A lens review is a review against ONE version of the lens, so associating a lens
        // pins the version its content is published as right now — the same pin a create
        // that applies lenses records, resolved from the one place that rule lives and
        // BEFORE the workload is taken for writing.
        let reviews = store.applied_workload_lens_reviews(&fresh, now);

        let workload = store.workloads.get_mut(&input.workload_id).ok_or_else(|| {
            AssociateLensesErrorModeled::ResourceNotFoundException(
                crate::state::workload_not_found(&input.workload_id),
            )
        })?;
        for (alias, record) in reviews {
            // A lens associated for the first time gets the review that association pins;
            // one the workload already reviews was filtered out above, so no pin is
            // overwritten and no review the workload holds is reset.
            workload.lens_review_records.insert(alias, record);
        }
        workload.lenses.extend(fresh);
        // The lenses are a member of the workload (`Workload$Lenses`), so associating one
        // changes the workload, and `Workload$UpdatedAt` — "the date and time recorded" —
        // is what every read of the workload and of its lens reviews reports that change
        // by.
        workload.updated_at = now;

        Ok(AssociateLensesOutput)
    }
}
