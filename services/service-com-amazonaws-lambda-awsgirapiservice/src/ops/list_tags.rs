// SPDX-License-Identifier: CC0-1.0

//! @op ListTags
//!
//! Reports the tag set of the resource the `Resource` ARN names, out of the single
//! record the store holds for it.
//!
//! The rule: a Lambda tag set belongs to the resource its ARN names, so `ListTags`
//! answers the tags that resource currently carries and nothing else — a resource that
//! carries none answers an empty tag set, an ARN that qualifies a function with a
//! version or an alias names no tag set at all (`Resource`'s own documentation: "Lambda
//! does not support adding tags to function aliases or versions"), and an ARN naming a
//! resource the account does not have is refused as not found. It is a `@readonly`
//! report, so the only write it performs is through
//! [`crate::state::resolve_function`], which materialises the lifecycle transition the
//! clock already owed.
//!
//! Tags are held per resource on the record the store keeps for it —
//! [`crate::state::FunctionRecord::tags`] for a function, which is where
//! `CreateFunction` persists the `Tags` of its request and where `GetFunction` reads the
//! `Tags` it reports ("You can also view function tags with GetFunction"), so all three
//! answer from one record. A capacity provider's tags are held the same way, on
//! [`crate::state::CapacityProviderRecord::tags`], where `CreateCapacityProvider`
//! persists the `Tags` of its request.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `ListTags::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
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
//! # Modeled contract for `ListTags`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")
//! ```
//!
//! Returns a function, event source mapping, or code signing configuration's tags. You can
//! also view function tags with GetFunction.
//!
//! ## Input
//!
//! * `Resource` (TaggableResource, len 1..10000, pattern
//!   ^arn:(aws[a-zA-Z-]*):lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:(function:[a-zA-Z0-9-_]+(:(\$LA
//!   …) **required** — The resource's Amazon Resource Name (ARN). Note: Lambda does not
//!   support adding tags to function aliases or versions.
//!
//! ## Output
//!
//! * `Tags` (Tags) — The function's tags.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — One of the parameters in the request is
//!   not valid.
//! * `ResourceNotFoundException` (HTTP 404) — The resource specified in the request does not
//!   exist.
//! * `ServiceException` (HTTP 500) — The Lambda service encountered an internal error.
//! * `TooManyRequestsException` (HTTP 429) — The request throughput limit was exceeded. For
//!   more information, see Lambda quotas.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListTagsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-lambda-awsgirapiservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTagsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.lambda#ListTags")]
#[derive(Debug)]
pub enum ListTagsErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
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
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListTagsErrorModeled::ServiceException(ServiceException {
                r#type: None,
                message: Some("Lambda state is not reachable on this engine".to_string()),
            })
            .into());
        };
        let region = crate::state::request_region(ctx);

        // The `Resource` pattern has already been enforced at the boundary, so the value
        // here is one of the resource ARNs it admits; the parse says WHICH.
        let named = crate::state::parse_taggable_resource(&input.resource).ok_or_else(|| {
            ListTagsErrorModeled::InvalidParameterValueException(InvalidParameterValueException {
                r#type: None,
                message: Some(format!(
                    "1 validation error detected: Value '{}' at 'resource' failed to satisfy \
                     constraint: Member must be the Amazon Resource Name of a Lambda resource",
                    input.resource
                )),
            })
        })?;

        let name = match named {
            crate::state::TaggableResourceRef::Function(name) => name,
            // "Lambda does not support adding tags to function aliases or versions", so a
            // qualified function ARN names no tag set to report, however the qualifier is
            // spelled.
            crate::state::TaggableResourceRef::FunctionQualifier { qualifier, .. } => {
                return Err(ListTagsErrorModeled::InvalidParameterValueException(
                    InvalidParameterValueException {
                        r#type: None,
                        message: Some(format!(
                            "Tags are not supported for a function version or alias. The resource \
                             '{}' qualifies the function with '{qualifier}'; specify the \
                             unqualified function ARN instead.",
                            input.resource
                        )),
                    },
                )
                .into())
            }
            // A capacity provider is the other taggable resource this store holds, so
            // its tag set is reported out of its own record exactly as a function's is.
            crate::state::TaggableResourceRef::CapacityProvider(provider) => {
                let store = state.lock();
                let Some(record) = store.capacity_providers.get(&provider) else {
                    return Err(ListTagsErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            r#type: None,
                            message: Some(format!(
                                "The resource you requested does not exist: no capacity \
                                 provider is held for '{}'",
                                input.resource
                            )),
                        },
                    )
                    .into());
                };
                return Ok(ListTagsOutput {
                    tags: Some(
                        record
                            .tags
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect(),
                    ),
                });
            }
            // A taggable Lambda resource of another type. This store holds records of
            // functions and capacity providers only — every operation that would mint a
            // code-signing configuration, an event source mapping or a network
            // connector is still a scaffold — so no resource of that type exists to
            // report tags for, which is what the modelled not-found answers. See
            // [`crate::state::parse_taggable_resource`]: whoever implements one of
            // those creates turns this branch into a lookup against its records.
            crate::state::TaggableResourceRef::Other(resource_type) => {
                return Err(ListTagsErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        r#type: None,
                        message: Some(format!(
                            "The resource you requested does not exist: no {resource_type} is \
                             held for '{}'",
                            input.resource
                        )),
                    },
                )
                .into())
            }
        };

        let mut store = state.lock();
        let (record, _version) = crate::state::resolve_function(
            &mut store,
            &name,
            None,
            &region,
            ::std::time::SystemTime::now(),
        )
        .map_err(|failure| match failure {
            crate::state::ResolveFailure::Malformed(message) => {
                ListTagsErrorModeled::InvalidParameterValueException(
                    InvalidParameterValueException {
                        r#type: None,
                        message: Some(message),
                    },
                )
            }
            crate::state::ResolveFailure::NotFound(message) => {
                ListTagsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    r#type: None,
                    message: Some(message),
                })
            }
        })?;

        // Tags are function-level, so what is reported is the one tag set the record
        // holds — empty when the function carries none, which is a value the operation
        // reports rather than a member it omits: "Returns a function['s] ... tags".
        Ok(ListTagsOutput {
            tags: Some(
                record
                    .tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        })
    }
}
