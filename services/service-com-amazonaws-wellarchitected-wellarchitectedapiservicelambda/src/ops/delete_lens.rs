// SPDX-License-Identifier: CC0-1.0

//! @op DeleteLens
//!
//! Deletes the content of the custom lens the request's `LensAlias` resolves to and
//! answers an empty 200. `LensStatus` is "the status of the lens to be deleted", so it
//! selects WHICH content of the lens goes by the publication state that content is in —
//! the same publication vocabulary `ListLenses` filters its own `LensStatus` on:
//!
//! * `DRAFT` drops the content no version has been published from, leaving the lens at
//!   its latest published version.
//! * `PUBLISHED` drops every version published from the lens, leaving the content it
//!   carries as a draft again.
//! * `ALL` drops both, so the lens itself is gone.
//!
//! A lens with nothing left — its draft deleted and no version ever published from it —
//! is removed from the store, and with it the shares recorded against it and the tokens
//! that resolved into its content
//! (`crate::state::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::remove_lens`),
//! so a `GetLens` of it is the lookup's 404. Content the request's status selects nothing
//! of is that same 404: a lens whose content is published has no draft to delete, and one
//! no version was published from has no published version to delete. An Amazon Web
//! Services official lens is refused 400 — AWS publishes its own lenses, so a caller
//! cannot delete one. The `@idempotencyToken` is honoured: a RETRY — the same
//! `ClientRequestToken` naming the same lens and status — replays the original request's
//! empty 200 rather than reporting the content it dropped as absent.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteLens`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! Delete an existing lens. Only the owner of a lens can delete it. After the lens is
//! deleted, Amazon Web Services accounts and users that you shared the lens with can
//! continue to use it, but they will no longer be able to apply it to new workloads.
//! Disclaimer By sharing your custom lenses with other Amazon Web Services accounts, you
//! acknowledge that Amazon Web Services will make your custom lenses available to those
//! other accounts. Those other accounts may continue to access and use your shared custom
//! lenses even if you delete the custom lenses from your own Amazon Web Services account or
//! terminate your Amazon Web Services account.
//!
//! ## Input
//!
//! * `ClientRequestToken` (ClientRequestToken, len 1..2048, pattern ^[\x00-\x7F]*$,
//!   @idempotencyToken) **required** — A unique case-sensitive string used to ensure that
//!   this request is idempotent (executes only once). You should not reuse the same token for
//!   other requests. If you retry a request with the same client request token and the same
//!   parameters after the original request has completed successfully, the result of the
//!   original request is returned. This token is listed as required, however, if you do not
//!   specify it, the Amazon Web Services SDKs automatically generate one for you. If you are
//!   not using the Amazon Web Services SDK or the CLI, you must provide this token or the
//!   request will fail.
//! * `LensAlias` (LensAlias, len 1..128) **required** — The alias of the lens. For Amazon Web
//!   Services official lenses, this is either the lens alias, such as serverless, or the lens
//!   ARN, such as arn:aws:wellarchitected:us-east-1::lens/serverless. Note that some
//!   operations (such as ExportLens and CreateLensShare) are not permitted on Amazon Web
//!   Services official lenses. For custom lenses, this is the lens ARN, such as
//!   arn:aws:wellarchitected:us-west-2:123456789012:lens/0123456789abcdef01234567890abcdef.
//!   Each lens is identified by its LensSummary$LensAlias.
//! * `LensStatus` (LensStatusType, one of: ALL | DRAFT | PUBLISHED) **required** — The status
//!   of the lens to be deleted.
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
// `DeleteLensError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

#[derive(Debug, Clone, Default)]
pub struct DeleteLensOutput;

impl DeleteLensOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "DELETE",
        "/lenses/{LensAlias}",
        Some(200u16),
    ));
}

pub static DELETE_LENS_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeleteLensOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteLensOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteLensError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#DeleteLens")]
#[derive(Debug)]
pub enum DeleteLensErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_lens::DeleteLensError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteLensError = crate::scaffolded::Scaffolded<DeleteLensErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteLensErrorModeled> for DeleteLensError {
    fn from(modeled: DeleteLensErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteLens` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteLens<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteLens<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteLens<Eng>
{
    type Input = DeleteLensInput;
    type Output = DeleteLensOutput;
    type Error = DeleteLensError;

    const NAME: &'static str = "DeleteLens";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_LENS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_LENS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(|| {
            DeleteLensErrorModeled::InternalServerException(InternalServerException {
                message: "the Well-Architected Tool lens store is unavailable".to_string(),
            })
        })?;

        // "Only the owner of a lens can delete it" (this operation's doc) and "note that
        // some operations … are not permitted on Amazon Web Services official lenses"
        // (`LensAlias` doc): AWS publishes its own lenses rather than an account owning
        // one, so a caller cannot delete one. The lens NAMED is what is refused, so this
        // is a validation failure on `LensAlias` rather than a lookup miss — the same
        // refusal `ImportLens`, `ExportLens` and `CreateLensVersion` word for an official
        // lens.
        if crate::state::is_aws_official_lens(&input.lens_alias) {
            return Err(field_validation(
                LENS_ALIAS_MEMBER,
                "Member must be the ARN of a custom lens; an Amazon Web Services official \
                 lens cannot be deleted",
            ));
        }

        // "The status of the lens to be deleted": the publication state of the content
        // this request removes. `LensStatusType` publishes exactly these three members
        // and the boundary refuses any other value before this handler runs, so the last
        // arm is unreachable over the wire — and a value this handler cannot place must
        // delete NOTHING rather than fall through to a default.
        let deleted = match input.lens_status.as_str() {
            crate::state::LENS_STATUS_TYPE_ALL => Deleted::All,
            crate::state::LENS_STATUS_TYPE_DRAFT => Deleted::Draft,
            crate::state::LENS_STATUS_TYPE_PUBLISHED => Deleted::Published,
            _ => {
                return Err(field_validation(
                    LENS_STATUS_MEMBER,
                    &format!(
                        "Member must satisfy enum value set: [{}]",
                        LENS_STATUS_TYPE_VALUES.join(", ")
                    ),
                ));
            }
        };

        // `ClientRequestToken` is `@idempotencyToken`, and the rule it documents holds for
        // a delete exactly as it does for `DeleteReviewTemplate`: a retry under the same
        // token and the same parameters, after the original request completed
        // successfully, replays that request's result. The parameters besides the token
        // are the lens and the status, so the fingerprint is over both — a token reused to
        // delete another part of the lens is not the same request. The lens is taken as
        // the lens its `LensAlias` NAMES rather than as the spelling it was named by,
        // because that is the identity the alias is resolved through below
        // (`crate::state::lens_alias`): a retry that spells the same lens the other way is
        // the SAME request.
        let fingerprint = crate::state::parameter_fingerprint(&serde_json::json!({
            "LensAlias": crate::state::lens_alias(&input.lens_alias),
            "LensStatus": &input.lens_status,
        }));

        let now = std::time::SystemTime::now();
        let mut store = state.lock();

        // Read before the alias is resolved: the original request may have removed the
        // lens, so it no longer resolves and only the recorded delete can answer the
        // retry.
        match crate::state::delete_replay(
            &store.delete_lens_tokens,
            &input.client_request_token,
            crate::state::LENS_RESOURCE_TYPE,
            &fingerprint,
        ) {
            crate::state::DeleteReplay::Replay => return Ok(DeleteLensOutput),
            crate::state::DeleteReplay::Conflict(exception) => {
                return Err(DeleteLensErrorModeled::ConflictException(exception).into())
            }
            crate::state::DeleteReplay::Fresh => {}
        }

        // The lens is resolved by `LensAlias` through the store's own lookup, so an alias
        // naming no stored custom lens is that lookup's 404. This emulator serves ONE
        // account and `ImportLens` records that account as every lens's owner, so every
        // caller that resolves a lens here is its owner and there is no
        // `AccessDeniedException` to raise for the "only the owner can delete it" rule.
        let record = store.lens_by_alias(&input.lens_alias).ok_or_else(|| {
            DeleteLensErrorModeled::ResourceNotFoundException(crate::state::lens_not_found(
                &input.lens_alias,
            ))
        })?;
        let lens_id = record.lens_id.clone();
        // The lens AS THE SERVICE NAMES IT — "for custom lenses, this is the lens ARN"
        // (`LensAlias` doc), which is what a conflict over this request's token reports as
        // the resource the token was already spent on, whatever spelling of the alias this
        // request used.
        let lens_arn = record.lens_arn.clone();
        // The lens's own publication state, as every read of it reports
        // (`LensSummary$LensStatus`): its content is a DRAFT exactly while no version has
        // been published from that content, and it has published versions exactly while
        // its version map is not empty. A lens can be in both at once — a draft imported
        // over a lens that has already published a version — which is why the two
        // questions are separate.
        let carries_draft = record.published_version(now).is_none();
        let carries_published_versions = !record.versions.is_empty();

        match deleted {
            Deleted::All => {
                store.remove_lens(&lens_id);
            }
            Deleted::Draft => {
                if !carries_draft {
                    return Err(DeleteLensErrorModeled::ResourceNotFoundException(
                        crate::state::lens_draft_not_found(&input.lens_alias),
                    )
                    .into());
                }
                // `discard_draft` leaves the lens at its latest published version and
                // answers whether there was one to fall back to: a lens no version was
                // ever published from has nothing left once its draft is gone, so the
                // record itself goes. The record is reached by the lens id it carries,
                // which is what the lens map is keyed by.
                let kept = store
                    .lenses
                    .get_mut(&lens_id)
                    .is_some_and(|record| record.discard_draft());
                if !kept {
                    store.remove_lens(&lens_id);
                }
            }
            Deleted::Published => {
                if !carries_published_versions {
                    return Err(DeleteLensErrorModeled::ResourceNotFoundException(
                        crate::state::lens_without_published_version(&input.lens_alias),
                    )
                    .into());
                }
                store.remove_lens_published_versions(&lens_id);
            }
        }

        // Recorded only now the delete has COMPLETED SUCCESSFULLY: a refused request has
        // no result to replay, so the token it carried is still free. Written through the
        // ledger's bounded `remember`, which holds the remembered tokens at
        // `MAX_REMEMBERED_REQUEST_TOKENS` by forgetting the oldest: the token is
        // caller-chosen, so the ledger cannot be allowed to grow with the stream of them.
        store.delete_lens_tokens.remember(
            input.client_request_token,
            crate::state::CompletedDeleteToken {
                resource_id: lens_arn,
                fingerprint,
            },
        );

        Ok(DeleteLensOutput)
    }
}

/// Which content of the lens one `DeleteLens` request removes — the `LensStatusType` it
/// named, as this handler acts on it.
#[derive(Debug, Clone, Copy)]
enum Deleted {
    /// `ALL`: every version of the lens and its draft, so the lens itself.
    All,
    /// `DRAFT`: only the content no version has been published from.
    Draft,
    /// `PUBLISHED`: only the versions published from the lens.
    Published,
}

/// The modelled member a refusal about the lens being deleted names
/// (`DeleteLensInput$LensAlias`).
const LENS_ALIAS_MEMBER: &str = "LensAlias";

/// The modelled member a refusal about the publication state being deleted names
/// (`DeleteLensInput$LensStatus`).
const LENS_STATUS_MEMBER: &str = "LensStatus";

/// The service's own modelled `ValidationException` (HTTP 400) naming the member that
/// failed and the constraint it violated — built through the service's shared envelope so
/// every operation words a refusal identically.
fn field_validation(member: &str, constraint: &str) -> DeleteLensError {
    DeleteLensErrorModeled::ValidationException(crate::state::member_constraint_exception(
        member, constraint,
    ))
    .into()
}
