// SPDX-License-Identifier: CC0-1.0

//! @op TagResource
//!
//! Writes the tags the request carries onto the resource its `ResourceArn`
//! names, in [`crate::state`] — the SAME stored record that resource's own
//! create wrote and its own reads report, resolved by the step the tagging
//! calls share
//! ([`crate::state::ComAmazonawsBackupCryocontrollerusermanagerState::tagged_resource_tags_mut`]).
//!
//! Three steps, in the order AWS Backup applies them:
//!
//! * a `ResourceArn` that is not an ARN is a bad parameter value, not an absent
//!   resource ([`crate::state::rejected_arn`]);
//! * an ARN that names no taggable resource of this account is the one
//!   not-found refusal ([`crate::state::tagged_resource_not_found`]), shared
//!   with the tag read;
//! * otherwise the pairs are MERGED into the resource's tag set — a key it
//!   already carries takes the value just sent, a key it does not is added, and
//!   a key the request does not name is left alone — so the set every read of
//!   that resource reports follows.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `TagResource`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Assigns a set of key-value pairs to a resource.
//!
//! ## Input
//!
//! * `ResourceArn` (ARN) **required** — The ARN that uniquely identifies the resource.
//! * `Tags` (Tags) **required** — Key-value pairs that are used to help organize your
//!   resources. You can assign your own metadata to the resources you create. For clarity,
//!   this is the structure to assign tags: [{"Key":"string","Value":"string"}].
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `LimitExceededException` (HTTP 400) — A limit in the request has been exceeded; for
//!   example, a maximum number of items allowed in a request.
//! * `MissingParameterValueException` (HTTP 400) — Indicates that a required parameter is
//!   missing.
//! * `ResourceNotFoundException` (HTTP 400) — A resource that is required for the action
//!   doesn't exist.
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a temporary failure
//!   of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TagResourceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

#[derive(Debug, Clone, Default)]
pub struct TagResourceOutput;

impl TagResourceOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/tags/{ResourceArn}",
        Some(200u16),
    ));
}

pub static TAG_RESOURCE_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &TagResourceOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for TagResourceOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TagResourceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#TagResource")]
#[derive(Debug)]
pub enum TagResourceErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    LimitExceededException(LimitExceededException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::tag_resource::TagResourceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TagResourceError = crate::scaffolded::Scaffolded<TagResourceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TagResourceErrorModeled> for TagResourceError {
    fn from(modeled: TagResourceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `TagResource` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TagResource<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TagResource<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TagResource<Eng>
{
    type Input = TagResourceInput;
    type Output = TagResourceOutput;
    type Error = TagResourceError;

    const NAME: &'static str = "TagResource";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TAG_RESOURCE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = TAG_RESOURCE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        // The model types `ResourceArn` as `ARN` with no pattern, so the
        // framework cannot refuse a malformed one; AWS does, rather than
        // reporting a value that could never name a resource as merely absent.
        if let Some(refusal) = crate::state::rejected_arn("ResourceArn", &input.resource_arn) {
            return Err(TagResourceErrorModeled::InvalidParameterValueException(refusal).into());
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            TagResourceErrorModeled::ServiceUnavailableException(ServiceUnavailableException {
                message: Some("the backup vault store is unavailable".to_string()),
                ..Default::default()
            })
        })?;
        let mut store = state.lock();
        // The transitions the clock already owes the records this call can
        // address, so the resource it resolves is the resource as it stands
        // now: a vault whose creation came due, and a recovery point whose
        // backup did.
        store.settle_vaults(now);
        store.settle_recovery_points(now);

        let tags = store
            .tagged_resource_tags_mut(&input.resource_arn)
            .map_err(TagResourceErrorModeled::ResourceNotFoundException)?;
        // The ceiling is applied to the set the write would LEAVE, so a request
        // that only overwrites keys the resource already carries is never
        // refused for the size of that set.
        let merged: usize = input
            .tags
            .keys()
            .filter(|key| !tags.contains_key(key.as_str()))
            .count();
        if tags.len().saturating_add(merged) > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(TagResourceErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(format!(
                        "this emulator keeps at most {} tags on one resource",
                        crate::state::MAX_TAGS_PER_RESOURCE
                    )),
                    ..Default::default()
                },
            )
            .into());
        }
        for (key, value) in input.tags {
            tags.insert(key, value);
        }
        Ok(TagResourceOutput)
    }
}
