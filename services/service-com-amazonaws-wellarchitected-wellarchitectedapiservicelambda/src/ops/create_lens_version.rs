// SPDX-License-Identifier: CC0-1.0

//! @op CreateLensVersion
//!
//! Publishes a new version of a custom lens from the content `ImportLens` recorded as
//! its draft, so the version becomes readable by the `LensVersion` the request named.
//!
//! What this handler enforces, and why:
//!
//! * The lens is resolved by `LensAlias` through the store's own lookup, so an alias
//!   naming no stored custom lens — or one whose first import has not landed yet, which
//!   has no content to publish — is the lookup's 404.
//! * An Amazon Web Services official lens is refused 400: AWS publishes its own lenses,
//!   so a caller cannot publish a version of one.
//! * A `LensVersion` the lens already published is a 409: a published version is
//!   immutable, so a second publish under the same name collides with it.
//! * "A lens can have up to 100 versions" is the modelled
//!   `ServiceQuotaExceededException` (402).
//! * `ClientRequestToken` is `@idempotencyToken`: the same token with the same
//!   parameters reports the version the original request published; with different
//!   parameters it is a 409.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateLensVersion`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! Create a new lens version. A lens can have up to 100 versions. Use this operation to
//! publish a new lens version after you have imported a lens. The LensAlias is used to
//! identify the lens to be published. The owner of a lens can share the lens with other
//! Amazon Web Services accounts and users in the same Amazon Web Services Region. Only the
//! owner of a lens can delete it.
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
//! * `IsMajorVersion` (IsMajorVersion) — Set to true if this new major lens version.
//! * `LensAlias` (LensAlias, len 1..128) **required** — The alias of the lens. For Amazon Web
//!   Services official lenses, this is either the lens alias, such as serverless, or the lens
//!   ARN, such as arn:aws:wellarchitected:us-east-1::lens/serverless. Note that some
//!   operations (such as ExportLens and CreateLensShare) are not permitted on Amazon Web
//!   Services official lenses. For custom lenses, this is the lens ARN, such as
//!   arn:aws:wellarchitected:us-west-2:123456789012:lens/0123456789abcdef01234567890abcdef.
//!   Each lens is identified by its LensSummary$LensAlias.
//! * `LensVersion` (LensVersion, len 1..32) **required** — The version of the lens being
//!   created.
//!
//! ## Output
//!
//! * `LensArn` (LensArn) — The ARN for the lens.
//! * `LensVersion` (LensVersion, len 1..32) — The version of the lens.
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
//! * `ServiceQuotaExceededException` (HTTP 402) — The user has reached their resource quota.
//! * `ThrottlingException` (HTTP 429) — Request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The user input is not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateLensVersionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateLensVersionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#CreateLensVersion")]
#[derive(Debug)]
pub enum CreateLensVersionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_lens_version::CreateLensVersionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateLensVersionError = crate::scaffolded::Scaffolded<CreateLensVersionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateLensVersionErrorModeled> for CreateLensVersionError {
    fn from(modeled: CreateLensVersionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateLensVersion` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateLensVersion<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateLensVersion<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateLensVersion<Eng>
{
    type Input = CreateLensVersionInput;
    type Output = CreateLensVersionOutput;
    type Error = CreateLensVersionError;

    const NAME: &'static str = "CreateLensVersion";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_LENS_VERSION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_LENS_VERSION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(|| {
            CreateLensVersionErrorModeled::InternalServerException(InternalServerException {
                message: "the Well-Architected Tool lens store is unavailable".to_string(),
            })
        })?;

        // "Note that some operations … are not permitted on Amazon Web Services official
        // lenses" (`LensAlias` doc): an official lens is AWS's own published content, so
        // a caller cannot publish a version of it. The lens NAMED is what is refused, so
        // this is a validation failure on `LensAlias` rather than a lookup miss — the
        // same refusal `ImportLens` and `ExportLens` word for an official lens.
        if crate::state::is_aws_official_lens(&input.lens_alias) {
            return Err(field_validation(
                "LensAlias",
                "Member must be the ARN of a custom lens; a version of an Amazon Web \
                 Services official lens cannot be published",
            ));
        }

        // "Set to true if this new major lens version" (`IsMajorVersion` doc): the member
        // is a plain optional boolean with no modelled default, so a request that omits it
        // publishes a version that is not a major one — the very version a request spelling
        // the member `false` publishes. Resolved HERE, ahead of the fingerprint, so the
        // fingerprint covers the value this publish would RECORD rather than the spelling
        // it was asked by.
        let is_major_version = input.is_major_version.unwrap_or(false);

        // The parameters an `@idempotencyToken` retry is compared on, as this operation
        // RESOLVES them: the lens counts as the lens its `LensAlias` names rather than as
        // the spelling it was named by (`crate::state::lens_alias`, the identity
        // `lens_by_alias_mut` resolves the lens through below), so a retry that spells the
        // same lens the other way is the SAME request and replays. `IsMajorVersion` is
        // taken the same way, as the value the publish would record rather than as the
        // spelling it was named by, so a retry that spells the default explicitly is the
        // SAME request too — the convention the sibling creates hold their defaulted
        // members to.
        let fingerprint = crate::state::parameter_fingerprint(&serde_json::json!({
            "IsMajorVersion": is_major_version,
            "LensAlias": crate::state::lens_alias(&input.lens_alias),
            "LensVersion": &input.lens_version,
        }));
        let now = std::time::SystemTime::now();
        let mut store = state.lock();

        // `@idempotencyToken` replay: the same token with the same parameters reports the
        // version the original request published instead of publishing a second one, and
        // with different parameters cannot be that same request.
        if let Some(token) = store
            .create_lens_version_tokens
            .get(&input.client_request_token)
        {
            let (lens_id, lens_version) = (token.lens_id.clone(), token.lens_version.clone());
            if token.fingerprint != fingerprint {
                return Err(CreateLensVersionErrorModeled::ConflictException(
                    crate::state::idempotency_token_conflict(
                        crate::state::LENS_RESOURCE_TYPE,
                        lens_id,
                        &input.client_request_token,
                    ),
                )
                .into());
            }
            if let Some(record) = store
                .lenses
                .get(&lens_id)
                .filter(|record| record.versions.contains_key(&lens_version))
            {
                return Ok(CreateLensVersionOutput {
                    lens_arn: Some(record.lens_arn.clone()),
                    lens_version: Some(lens_version),
                });
            }
        }

        let record = store.lens_by_alias_mut(&input.lens_alias).ok_or_else(|| {
            CreateLensVersionErrorModeled::ResourceNotFoundException(
                crate::state::lens_not_found(&input.lens_alias),
            )
        })?;
        // This request writes to the lens, so an import the clock already owed becomes
        // the lens's draft content first: what is published is the draft as of now.
        record.settle(now);

        // "Use this operation to publish a new lens version after you have imported a
        // lens": the draft content is what a version is published FROM, so a lens whose
        // first import has not landed yet has nothing to publish and is not readable —
        // the same 404 `ExportLens` answers for it.
        let content = record.settled_content(now).cloned().ok_or_else(|| {
            CreateLensVersionErrorModeled::ResourceNotFoundException(
                crate::state::lens_not_found(&input.lens_alias),
            )
        })?;

        // A published version is immutable, so a second publish under a name the lens
        // already carries collides with it rather than replacing its content.
        if record.versions.contains_key(&input.lens_version) {
            return Err(CreateLensVersionErrorModeled::ConflictException(
                crate::state::lens_version_conflict(&input.lens_alias, &input.lens_version),
            )
            .into());
        }

        // "A lens can have up to 100 versions" (this operation's doc), and it models
        // `ServiceQuotaExceededException` for exactly a reached quota.
        if record.versions.len() >= crate::state::MAX_LENS_VERSIONS_PER_LENS {
            let lens_id = record.lens_id.clone();
            return Err(CreateLensVersionErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "The lens with ID {lens_id} already holds the maximum of {} versions",
                        crate::state::MAX_LENS_VERSIONS_PER_LENS
                    ),
                    resource_id: Some(lens_id),
                    resource_type: Some(crate::state::LENS_RESOURCE_TYPE.to_string()),
                    // The model publishes no numeric Service Quotas code for this quota,
                    // so it is named by what it meters; the service code is this
                    // service's own signing name.
                    quota_code: "VersionsPerLens".to_string(),
                    service_code: crate::state::ARN_NAMESPACE.to_string(),
                },
            )
            .into());
        }

        record.versions.insert(
            input.lens_version.clone(),
            crate::state::PublishedLensVersion {
                content,
                // Resolved above, where the fingerprint reads it too.
                is_major_version,
                published_at: now,
            },
        );
        let (lens_id, lens_arn) = (record.lens_id.clone(), record.lens_arn.clone());

        // Written through the ledger's bounded `remember`, which holds the remembered
        // tokens at `MAX_REMEMBERED_REQUEST_TOKENS` by forgetting the oldest: the token is
        // caller-chosen, so the ledger cannot be allowed to grow with the stream of them.
        store.create_lens_version_tokens.remember(
            input.client_request_token,
            crate::state::CreateLensVersionToken {
                lens_id,
                lens_version: input.lens_version.clone(),
                fingerprint,
            },
        );

        Ok(CreateLensVersionOutput {
            lens_arn: Some(lens_arn),
            lens_version: Some(input.lens_version),
        })
    }
}

/// The service's own modelled `ValidationException` (HTTP 400) naming the member that
/// failed and the constraint it violated — built through the service's shared envelope so
/// every operation words a refusal identically.
fn field_validation(member: &str, constraint: &str) -> CreateLensVersionError {
    CreateLensVersionErrorModeled::ValidationException(crate::state::member_constraint_exception(
        member, constraint,
    ))
    .into()
}
