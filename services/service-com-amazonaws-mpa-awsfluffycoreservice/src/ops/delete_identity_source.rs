// SPDX-License-Identifier: CC0-1.0

//! @op DeleteIdentitySource
//!
//! Deletes the identity source the request's `IdentitySourceArn` names out of
//! [`crate::state`]: the stored record is put into `DELETING`
//! ([`crate::state::IdentitySource::begin_deletion`]) and the clock removes it once that
//! removal comes due ([`crate::state::IDENTITY_SOURCE_DELETION_DELAY`]), so
//! `ListIdentitySources` reports the source `DELETING` straight after this call and omits
//! it afterwards. `DELETING` is the non-terminal `IdentitySourceStatus` token the model
//! declares for exactly this, and
//! [`crate::state::ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`] is what
//! materialises the removal for every later reader — nothing sleeps, and no read starts
//! a deletion.
//!
//! * `IdentitySourceArn` carries no ARN `@pattern` of its own here (`length(max: 1000)`
//!   only), so the malformed-ARN class is decided FIRST by this handler, against what the
//!   member ADDRESSES ([`crate::state::addresses_identity_source`]): a value that is not
//!   an ARN this service could have minted for an identity source — another service's
//!   namespace, another resource kind of this service, a partition outside the
//!   `aws` family, an account that is not twelve digits, a missing resource id, a resource
//!   id that is not an identity-source id of this service
//!   ([`crate::state::is_identity_source_id`]), or no ARN at all — names no identity
//!   source whatever this store holds, so it is refused with the sentence that NAMES the
//!   member it rejected ([`crate::state::invalid_identity_source_arn`]):
//!   `ValidationException` (400), never reported as a deletion that was carried out. The
//!   screen is the one `get_identity_source.rs` applies to the same member, so this
//!   service answers one value that addresses no identity source one way whichever
//!   operation is asked.
//! * The account boundary is decided next, before the source is resolved: an
//!   identity-source ARN outside the caller's own account is refused with
//!   `AccessDeniedException` (403) ([`crate::state::authorize_resource_arn`]), so an
//!   unauthorized deletion is never reported as a completed one.
//! * An identity-source ARN under which this store holds no identity source is a success
//!   that changes
//!   nothing: the operation is `@idempotent` and its model declares NO
//!   `ResourceNotFoundException` (`smithy_query(query="operation",
//!   operation="DeleteIdentitySource")`: `AccessDenied`, `Conflict`, `InternalServer`,
//!   `Throttling`, `Validation`), so a source that is already gone leaves the caller in
//!   the state it asked for — the same rule, and the same reason,
//!   `delete_inactive_approval_team_version.rs` applies to a pair naming no stored
//!   version. A repeat call on a source already `DELETING` is that same success and
//!   keeps the recorded due instant, so asking twice does not postpone the removal.
//! * An identity source an approver record a stored approval team holds authenticates
//!   through is IN USE, and removing it would leave that team pointing at a source this
//!   store no longer holds: `ConflictException` (409), the error this model declares for
//!   `DeleteIdentitySource` and for none of the other identity-source operations
//!   ([`crate::state::ComAmazonawsMpaAwsfluffycoreserviceState::team_arn_using_identity_source`]
//!   resolves the reference). A team holds TWO approver sets and a reference in either
//!   one is a reference this store holds: the live approvers, and the approvers of the
//!   change the team has been asked to approve, which `GetApprovalTeam` reports in
//!   `PendingUpdate$Approvers` and which activation installs as the live set
//!   ([`crate::state::ApprovalTeam::approver_records`] reads both). The rule is the
//!   STORE's rather than this request's, so the clock pass that carries the scheduled
//!   removal out reads the same resolver again when the removal comes due: a reference
//!   recorded after this call was accepted keeps the source `DELETING` with its removal
//!   still owed, so a later delete of it earns the same 409 and no read ever reports an
//!   approver authenticated by a source this store no longer holds.
//!
//! The management-account gate [`crate::service`] puts in front of the creates does not
//! apply: this request identifies its resource BY ARN, so its authorization decision
//! reads a value out of the request and belongs after the request is read — it is the
//! cross-account check above, the same one `start_active_approval_team_deletion.rs` and
//! the other ARN-addressed operations make.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `DeleteIdentitySource::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
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
//! # Modeled contract for `DeleteIdentitySource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json")
//! ```
//!
//! Deletes an identity source. For more information, see Identity Source in the Multi-party
//! approval User Guide.
//!
//! ## Input
//!
//! * `IdentitySourceArn` (String, len ..1000) **required** — Amazon Resource Name (ARN) for
//!   identity source.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action. Check your permissions, and try again.
//! * `ConflictException` (HTTP 409) — The request cannot be completed because it conflicts
//!   with the current state of a resource.
//! * `InternalServerException` (HTTP 500) — The service encountered an internal error. Try
//!   your request again. If the problem persists, contact Amazon Web Services Support.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints specified
//!   by an Amazon Web Services service.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteIdentitySourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json");

#[derive(Debug, Clone, Default)]
pub struct DeleteIdentitySourceOutput;

impl DeleteIdentitySourceOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "DELETE",
        "/identity-sources/{IdentitySourceArn}",
        Some(200u16),
    ));
}

pub static DELETE_IDENTITY_SOURCE_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeleteIdentitySourceOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteIdentitySourceOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteIdentitySourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.mpa#DeleteIdentitySource")]
#[derive(Debug)]
pub enum DeleteIdentitySourceErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_identity_source::DeleteIdentitySourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteIdentitySourceError = crate::scaffolded::Scaffolded<DeleteIdentitySourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteIdentitySourceErrorModeled> for DeleteIdentitySourceError {
    fn from(modeled: DeleteIdentitySourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteIdentitySource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteIdentitySource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteIdentitySource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteIdentitySource<Eng>
{
    type Input = DeleteIdentitySourceInput;
    type Output = DeleteIdentitySourceOutput;
    type Error = DeleteIdentitySourceError;

    const NAME: &'static str = "DeleteIdentitySource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_IDENTITY_SOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_IDENTITY_SOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        // `IdentitySourceArn` carries no ARN `@pattern` here, so the malformed-ARN class
        // is refused by this handler, with the sentence that names the member it rejected
        // (`crate::state::invalid_identity_source_arn`, 400), rather than by the boundary.
        // A value that is not an ARN this service could have minted for an identity source
        // — including one whose resource id is not an identity-source id of this service —
        // addresses no identity source, so it is refused as that class rather than reported
        // as a delete that was carried out: the same screen `get_identity_source.rs`
        // applies to the same member.
        if !crate::state::addresses_identity_source(&input.identity_source_arn) {
            return Err(DeleteIdentitySourceErrorModeled::ValidationException(
                crate::state::invalid_identity_source_arn(),
            )
            .into());
        }
        // The account boundary is evaluated BEFORE the resource is resolved: a
        // destructive call on an ARN outside the caller's account is refused with the
        // modelled `AccessDeniedException` (403) rather than reported as "already
        // absent".
        crate::state::authorize_resource_arn(Self::NAME, &input.identity_source_arn)
            .map_err(DeleteIdentitySourceErrorModeled::AccessDeniedException)?;
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteIdentitySourceError::Modeled(
                DeleteIdentitySourceErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the identity-source store is unavailable".to_string(),
                    },
                ),
            )
        })?;

        let mut state = store.lock();
        let now = ::std::time::SystemTime::now();
        state.materialize_due(now);

        // An identity-source ARN under which nothing is stored leaves the caller in the
        // state it asked for: the operation is `@idempotent` and models no not-found
        // error, so an identity source that is already gone is a success that writes
        // nothing. Only a value that ADDRESSES an identity source reaches here.
        if state.identity_source(&input.identity_source_arn).is_none() {
            return Ok(DeleteIdentitySourceOutput);
        }

        // What the decision needs, read out of the store before anything is written: an
        // identity source a team's approvers authenticate through cannot be removed
        // without leaving that team pointing at a source this store no longer holds.
        if let Some(team_arn) = state.team_arn_using_identity_source(&input.identity_source_arn) {
            let message = format!(
                "Identity source '{}' is used by approval team '{team_arn}', so it cannot be \
                 deleted. Delete the approval teams that use it, and try again.",
                input.identity_source_arn
            );
            return Err(
                DeleteIdentitySourceErrorModeled::ConflictException(ConflictException { message })
                    .into(),
            );
        }

        // The deletion is recorded on the stored record, not performed here: the source
        // is reported `DELETING` until its removal comes due, which `materialize_due`
        // owes every later reader. A source already `DELETING` keeps the due instant the
        // first call recorded.
        let Some(source) = state.identity_source_mut(&input.identity_source_arn) else {
            return Err(
                DeleteIdentitySourceErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the identity source could not be read back for deletion"
                            .to_string(),
                    },
                )
                .into(),
            );
        };
        // The instant the removal comes due is computed with checked arithmetic BEFORE the
        // record is touched, so a `now` the service cannot add that span to is the modelled
        // server error this operation declares and leaves the source exactly as it was —
        // never a panic, and never a source in `DELETING` whose removal is owed at no
        // instant at all.
        if !source.begin_deletion(now) {
            return Err(
                DeleteIdentitySourceErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the identity source's removal instant could not be computed"
                            .to_string(),
                    },
                )
                .into(),
            );
        }

        Ok(DeleteIdentitySourceOutput)
    }
}
