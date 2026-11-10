// SPDX-License-Identifier: CC0-1.0

//! @op ListTags
//!
//! Reports the tags the resource its `ResourceArn` names carries, read off the
//! SAME stored record `TagResource` writes and that resource's own create
//! wrote, through the step the tagging calls share
//! ([`crate::state::ComAmazonawsBackupCryocontrollerusermanagerState::tagged_resource_tags`]).
//!
//! A `ResourceArn` that is not an ARN is a bad parameter value
//! ([`crate::state::rejected_arn`]); one that names no taggable resource of
//! this account is the one not-found refusal
//! ([`crate::state::tagged_resource_not_found`]). A page is `MaxResults` pairs
//! plus a cursor to the next one, keyed by tag key — the order the stored set
//! keeps — so a cursor resolves to the same position on every request.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListTags`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Returns the tags assigned to the resource, such as a target recovery point, backup plan,
//! or backup vault. This operation returns results depending on the resource type used in
//! the value for resourceArn. For example, recovery points of Amazon DynamoDB with Advanced
//! Settings have an ARN (Amazon Resource Name) that begins with arn:aws:backup. Recovery
//! points (backups) of DynamoDB without Advanced Settings enabled have an ARN that begins
//! with arn:aws:dynamodb. When this operation is called and when you include values of
//! resourceArn that have an ARN other than arn:aws:backup, it may return one of the
//! exceptions listed below. To prevent this exception, include only values representing
//! resource types that are fully managed by Backup. These have an ARN that begins
//! arn:aws:backup and they are noted in the Feature availability by resource table.
//!
//! ## Input
//!
//! * `MaxResults` (MaxResults, range 1..1000) — The maximum number of items to be returned.
//! * `NextToken` (string) — The next item following a partial list of returned items. For
//!   example, if a request is made to return MaxResults number of items, NextToken allows you
//!   to return more items in your list starting at the location pointed to by the next token.
//! * `ResourceArn` (ARN) **required** — An Amazon Resource Name (ARN) that uniquely
//!   identifies a resource. The format of the ARN depends on the type of resource. Valid
//!   targets for ListTags are recovery points, backup plans, and backup vaults.
//!
//! ## Output
//!
//! * `NextToken` (string) — The next item following a partial list of returned items. For
//!   example, if a request is made to return MaxResults number of items, NextToken allows you
//!   to return more items in your list starting at the location pointed to by the next token.
//! * `Tags` (Tags) — Information about the tags.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
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
// `ListTagsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTagsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#ListTags")]
#[derive(Debug)]
pub enum ListTagsErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_tags::ListTagsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListTagsError = crate::scaffolded::Scaffolded<ListTagsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListTagsErrorModeled> for ListTagsError {
    fn from(modeled: ListTagsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListTags` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListTags<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListTags<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListTags<Eng>
{
    type Input = ListTagsInput;
    type Output = ListTagsOutput;
    type Error = ListTagsError;

    const NAME: &'static str = "ListTags";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TAGS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TAGS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();
        // The same two refusals the tag write answers, in the same order, and
        // both before anything is read.
        if let Some(refusal) = crate::state::rejected_arn("ResourceArn", &input.resource_arn) {
            return Err(ListTagsErrorModeled::InvalidParameterValueException(refusal).into());
        }
        if let Some(refusal) = crate::state::rejected_max_results(input.max_results) {
            return Err(ListTagsErrorModeled::InvalidParameterValueException(refusal).into());
        }
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListTagsErrorModeled::ServiceUnavailableException(ServiceUnavailableException {
                message: Some("the backup vault store is unavailable".to_string()),
                ..Default::default()
            })
        })?;
        let mut store = state.lock();
        // The only writes this read performs: the transitions the clock had
        // already made due for the records it can address.
        store.settle_vaults(now);
        store.settle_recovery_points(now);

        let tags = store
            .tagged_resource_tags(&input.resource_arn)
            .map_err(ListTagsErrorModeled::ResourceNotFoundException)?;
        let pairs: Vec<(&String, &String)> = tags.iter().collect();

        let start = match input.next_token.as_deref() {
            None => 0,
            Some(token) => {
                let key = crate::state::decode_page_token(token);
                let position =
                    key.and_then(|key| pairs.iter().position(|(tag_key, _)| **tag_key == key));
                let Some(position) = position else {
                    return Err(ListTagsErrorModeled::InvalidParameterValueException(
                        InvalidParameterValueException {
                            message: Some(
                                "NextToken does not name a page of this result set".to_string(),
                            ),
                            ..Default::default()
                        },
                    )
                    .into());
                };
                position
            }
        };
        let page_size = input
            .max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(crate::state::MAX_PAGE_SIZE)
            .clamp(1, crate::state::MAX_PAGE_SIZE);
        let end = start.saturating_add(page_size).min(pairs.len());
        let next_token = pairs
            .get(end)
            .map(|(key, _)| crate::state::encode_page_token(key));

        Ok(ListTagsOutput {
            tags: Some(
                pairs[start..end]
                    .iter()
                    .map(|(key, value)| ((*key).clone(), (*value).clone()))
                    .collect(),
            ),
            next_token,
        })
    }
}
