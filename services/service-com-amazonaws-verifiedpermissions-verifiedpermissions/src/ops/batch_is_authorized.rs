// SPDX-License-Identifier: CC0-1.0

//! @op BatchIsAuthorized
//!
//! `@readonly`: evaluates the policy store's persisted Cedar policies once per
//! request in `requests`, with the real Cedar authorizer, and answers one result
//! per request IN THE ORDER THEY WERE REQUESTED. The store's Cedar text is parsed
//! ONCE for the whole batch ([`crate::state::cedar::load_policies`]) and each
//! request is evaluated against that one policy set
//! ([`crate::state::cedar::evaluate`]), so a batch costs one parse, not one per
//! request. Every decision, determining policy and evaluation error is whatever
//! Cedar computed; nothing here is canned, and it writes nothing.
//!
//! Two rules the model cannot express are enforced here, both from this
//! operation's own documentation (quoted on the constants / helpers below): a
//! batch carries at most [`crate::state::cedar::MAX_BATCH_REQUESTS`] requests, and
//! either the principal or the resource must be identical across all of them.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `BatchIsAuthorized`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Makes a series of decisions about multiple authorization requests for one principal or
//! resource. Each request contains the equivalent content of an IsAuthorized request:
//! principal, action, resource, and context. Either the principal or the resource parameter
//! must be identical across all requests. For example, Verified Permissions won't evaluate
//! a pair of requests where bob views photo1 and alice views photo2. Authorization of bob
//! to view photo1 and photo2, or bob and alice to view photo1, are valid batches. The
//! request is evaluated against all policies in the specified policy store that match the
//! entities that you declare. The result of the decisions is a series of Allow or Deny
//! responses, along with the IDs of the policies that produced each decision. The entities
//! of a BatchIsAuthorized API request can contain up to 100 principals and up to 100
//! resources. The requests of a BatchIsAuthorized API request can contain up to 30
//! requests. The BatchIsAuthorized operation doesn't have its own IAM permission. To
//! authorize this operation for Amazon Web Services principals, include the permission
//! verifiedpermissions:IsAuthorized in their IAM policies.
//!
//! ## Input
//!
//! * `entities` (EntitiesDefinition) — (Optional) Specifies the list of resources and
//!   principals and their associated attributes that Verified Permissions can examine when
//!   evaluating the policies. These additional entities and their attributes can be
//!   referenced and checked by conditional elements in the policies in the specified policy
//!   store. You can include only principal and resource entities in this parameter; you can't
//!   include actions. You must specify actions in the schema.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store. Policies in this policy store will be used to make
//!   the authorization decisions for the input. To specify a policy store, use its ID or
//!   alias name. When using an alias name, prefix it with policy-store-alias/. For example:
//!   ID: PSEXAMPLEabcdefg111111 Alias name: policy-store-alias/example-policy-store To view
//!   aliases, use ListPolicyStoreAliases.
//! * `requests` (BatchIsAuthorizedInputList, len 1..) **required** — An array of up to 30
//!   requests that you want Verified Permissions to evaluate.
//!
//! ## Output
//!
//! * `results` (BatchIsAuthorizedOutputList) **required** — A series of Allow or Deny
//!   decisions for each request, and the policies that produced them. These results are
//!   returned in the order they were requested.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `BatchIsAuthorizedError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `BatchIsAuthorizedError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#BatchIsAuthorized",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum BatchIsAuthorizedErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::batch_is_authorized::BatchIsAuthorizedError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type BatchIsAuthorizedError = crate::scaffolded::Scaffolded<BatchIsAuthorizedErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<BatchIsAuthorizedErrorModeled> for BatchIsAuthorizedError {
    fn from(modeled: BatchIsAuthorizedErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `BatchIsAuthorized` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct BatchIsAuthorized<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> BatchIsAuthorized<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for BatchIsAuthorized<Eng>
{
    type Input = BatchIsAuthorizedInput;
    type Output = BatchIsAuthorizedOutput;
    type Error = BatchIsAuthorizedError;

    const NAME: &'static str = "BatchIsAuthorized";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_IS_AUTHORIZED_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_IS_AUTHORIZED_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let not_found = || {
            BatchIsAuthorizedErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };

        // The model bounds `requests` below (`@length min 1`) but not above, so the
        // published batch size is refused here rather than at the boundary. It is
        // checked ahead of the store lookup: the list's own size is a property of
        // the request, not of any store, so it costs a length check rather than a
        // state read.
        if input.requests.len() > crate::state::cedar::MAX_BATCH_REQUESTS {
            return Err(invalid(crate::ops::InvalidInput::new(
                crate::ops::paths::REQUESTS,
                format!(
                    "it carries {} authorization requests, more than the {} a batch may carry",
                    input.requests.len(),
                    crate::state::cedar::MAX_BATCH_REQUESTS
                ),
            )));
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            BatchIsAuthorizedErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        // The store is resolved BEFORE any of the request's payloads is turned
        // into a Cedar value, so a reference to a store that does not exist
        // answers `ResourceNotFoundException` rather than a refusal about the
        // payload.
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
        let store = state.policy_stores.get(&store_id).ok_or_else(not_found)?;

        // Each request is evaluated as a complete Cedar triple, so every member of
        // every request must be present; an absent one is refused by naming the
        // request's index in the error's `fieldList`, rather than answered with a
        // deny that would read as "evaluated and denied".
        let required = |index: usize, member: &str| {
            invalid(
                crate::ops::InvalidInput::new(
                    member,
                    "it is required to evaluate an authorization request",
                )
                .within_batch_request(index),
            )
        };
        let mut triples: Vec<Triple> = Vec::with_capacity(input.requests.len());
        let mut contexts: Vec<serde_json::Value> = Vec::with_capacity(input.requests.len());
        for (index, item) in input.requests.iter().enumerate() {
            triples.push(Triple {
                principal: crate::state::cedar::request_entity(item.principal.as_ref())
                    .ok_or_else(|| required(index, crate::ops::paths::PRINCIPAL))?,
                action: crate::state::cedar::request_action(item.action.as_ref())
                    .ok_or_else(|| required(index, crate::ops::paths::ACTION))?,
                resource: crate::state::cedar::request_entity(item.resource.as_ref())
                    .ok_or_else(|| required(index, crate::ops::paths::RESOURCE))?,
            });
            contexts.push(
                crate::state::cedar::context_to_cedar_json(item.context.as_ref())
                    // The conversion names the member within ONE request's
                    // `context`; the batch says which request that was.
                    .map_err(|e| invalid(e.within_batch_request(index)))?,
            );
        }

        if !shares_principal_or_resource(&triples) {
            return Err(invalid(crate::ops::InvalidInput::new(
                crate::ops::paths::REQUESTS,
                "either the principal or the resource must be identical across all of \
                 the requests in a batch",
            )));
        }

        // `entities` is a property of the BATCH, not of one request: one entity
        // slice is built once and every request is evaluated against it, typed by
        // the store's schema when it holds one. A refusal here names `/entities`
        // itself, with no request index, because that is where the member lives.
        let entities = crate::state::cedar::entities_to_cedar_json(input.entities.as_ref())
            .map_err(invalid)?;

        // Every policy of the store, parsed ONCE for the whole batch: a static
        // policy's own Cedar text and a template-linked one's link of its
        // template, plus the store's schema when it holds one, which is what
        // types the requests' entities and context.
        let prepared = crate::state::cedar::load_policies(store).map_err(refused)?;

        let entities =
            crate::state::cedar::build_entities(entities, prepared.schema()).map_err(invalid)?;

        let mut results = Vec::with_capacity(input.requests.len());
        for (index, ((item, triple), context)) in input
            .requests
            .iter()
            .zip(&triples)
            .zip(contexts)
            .enumerate()
        {
            let verdict = crate::state::cedar::evaluate(
                &prepared,
                &triple.principal,
                &triple.action,
                &triple.resource,
                crate::state::cedar::RequestContext {
                    json: context,
                    path: crate::state::cedar::context_path(item.context.as_ref()),
                },
                &entities,
            )
            // The evaluation names the member within ONE request; the batch says
            // which request that was.
            .map_err(|e| invalid(e.within_batch_request(index)))?;

            results.push(BatchIsAuthorizedOutputItem {
                // "The authorization request that initiated the decision": the
                // request is echoed back so a caller can line a result up with it.
                request: item.clone(),
                decision: verdict.decision,
                determining_policies: verdict
                    .determining_policies
                    .into_iter()
                    .map(|policy_id| DeterminingPolicyItem { policy_id })
                    .collect(),
                // Required and always present: an empty list is what a clean
                // evaluation reports.
                errors: verdict
                    .errors
                    .into_iter()
                    .map(|error_description| EvaluationErrorItem { error_description })
                    .collect(),
            });
        }

        // "These results are returned in the order they were requested."
        Ok(BatchIsAuthorizedOutput { results })
    }
}

/// One request's complete Cedar triple.
struct Triple {
    principal: crate::state::ScopeEntity,
    action: crate::state::ScopeEntity,
    resource: crate::state::ScopeEntity,
}

/// One offending request member as this operation's modelled
/// `ValidationException`, whose `fieldList` names it.
fn invalid(invalid: crate::ops::InvalidInput) -> BatchIsAuthorizedError {
    BatchIsAuthorizedErrorModeled::ValidationException(crate::ops::validation_exception([invalid]))
        .into()
}

/// A Cedar refusal as this operation's answer: a bad request member is the
/// modelled `ValidationException`, and service-side state the caller cannot fix
/// — a stored schema that will not parse, a store holding more Cedar than one
/// request evaluates — is a server fault rather than a bad request.
fn refused(refusal: crate::state::cedar::Refusal) -> BatchIsAuthorizedError {
    match refusal {
        crate::state::cedar::Refusal::InvalidInput(field) => invalid(field),
        crate::state::cedar::Refusal::ServiceFault(message) => {
            BatchIsAuthorizedErrorModeled::InternalServerException(InternalServerException {
                message,
            })
            .into()
        }
        // An authorization request carries no policy content, so the store's
        // schema has no member of this REQUEST to refuse: reasons reaching here
        // are about policies the store already holds, which is service-side
        // state and not something the caller sent.
        crate::state::cedar::Refusal::SchemaValidation(reasons) => {
            BatchIsAuthorizedErrorModeled::InternalServerException(InternalServerException {
                message: reasons.join("; "),
            })
            .into()
        }
    }
}

/// Whether the batch satisfies this operation's documented rule: "Either the
/// principal or the resource parameter must be identical across all requests."
/// A single-request batch satisfies it trivially.
fn shares_principal_or_resource(triples: &[Triple]) -> bool {
    let Some(first) = triples.first() else {
        return true;
    };
    triples
        .iter()
        .all(|triple| triple.principal == first.principal)
        || triples
            .iter()
            .all(|triple| triple.resource == first.resource)
}
