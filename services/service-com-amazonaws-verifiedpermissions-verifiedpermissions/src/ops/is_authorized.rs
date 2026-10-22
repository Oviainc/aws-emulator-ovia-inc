// SPDX-License-Identifier: CC0-1.0

//! @op IsAuthorized
//!
//! `@readonly`: evaluates the policy store's persisted Cedar policies against the
//! request's principal/action/resource triple with the real Cedar authorizer
//! ([`crate::state::cedar::authorize`]). The decision, the determining policies
//! and the evaluation errors are all whatever Cedar computed — a store with no
//! matching policy default-denies, a `Forbid` beats a `Permit`, and nothing here
//! is canned. It writes nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `IsAuthorized`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Makes an authorization decision about a service request described in the parameters. The
//! information in the parameters can also define additional context that Verified
//! Permissions can include in the evaluation. The request is evaluated against all matching
//! policies in the specified policy store. The result of the decision is either Allow or
//! Deny, along with a list of the policies that resulted in the decision.
//!
//! ## Input
//!
//! * `action` (ActionIdentifier) — Specifies the requested action to be authorized. For
//!   example, is the principal authorized to perform this action on the resource?
//! * `context` (ContextDefinition) — Specifies additional context that can be used to make
//!   more granular authorization decisions.
//! * `entities` (EntitiesDefinition) — (Optional) Specifies the list of resources and
//!   principals and their associated attributes that Verified Permissions can examine when
//!   evaluating the policies. These additional entities and their attributes can be
//!   referenced and checked by conditional elements in the policies in the specified policy
//!   store. You can include only principal and resource entities in this parameter; you can't
//!   include actions. You must specify actions in the schema.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store. Policies in this policy store will be used to make
//!   an authorization decision for the input. To specify a policy store, use its ID or alias
//!   name. When using an alias name, prefix it with policy-store-alias/. For example: ID:
//!   PSEXAMPLEabcdefg111111 Alias name: policy-store-alias/example-policy-store To view
//!   aliases, use ListPolicyStoreAliases.
//! * `principal` (EntityIdentifier) — Specifies the principal for which the authorization
//!   decision is to be made.
//! * `resource` (EntityIdentifier) — Specifies the resource for which the authorization
//!   decision is to be made.
//!
//! ## Output
//!
//! * `decision` (Decision, one of: ALLOW | DENY) **required** — An authorization decision
//!   that indicates if the authorization request should be allowed or denied.
//! * `determiningPolicies` (DeterminingPolicyList) **required** — The list of determining
//!   policies used to make the authorization decision. For example, if there are two matching
//!   policies, where one is a forbid and the other is a permit, then the forbid policy will
//!   be the determining policy. In the case of multiple matching permit policies then there
//!   would be multiple determining policies. In the case that no policies match, and hence
//!   the response is DENY, there would be no determining policies.
//! * `errors` (EvaluationErrorList) **required** — Errors that occurred while making an
//!   authorization decision, for example, a policy references an Entity or entity Attribute
//!   that does not exist in the slice.
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
// `IsAuthorizedError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `IsAuthorizedError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#IsAuthorized",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum IsAuthorizedErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::is_authorized::IsAuthorizedError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type IsAuthorizedError = crate::scaffolded::Scaffolded<IsAuthorizedErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<IsAuthorizedErrorModeled> for IsAuthorizedError {
    fn from(modeled: IsAuthorizedErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `IsAuthorized` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct IsAuthorized<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> IsAuthorized<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for IsAuthorized<Eng>
{
    type Input = IsAuthorizedInput;
    type Output = IsAuthorizedOutput;
    type Error = IsAuthorizedError;

    const NAME: &'static str = "IsAuthorized";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::IS_AUTHORIZED_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::IS_AUTHORIZED_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let not_found = || {
            IsAuthorizedErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };

        // Cedar evaluates a request against a complete triple, so each of the
        // three must be present. Refused as a bad request rather than answered
        // with a deny, which would read as "evaluated and denied" — and the
        // refusal names the absent member in `fieldList`.
        let required = |path: &str| invalid(crate::ops::InvalidInput::new(path, REQUIRED_FOR_EVALUATION));
        let principal = crate::state::cedar::request_entity(input.principal.as_ref())
            .ok_or_else(|| required(crate::ops::paths::PRINCIPAL))?;
        let resource = crate::state::cedar::request_entity(input.resource.as_ref())
            .ok_or_else(|| required(crate::ops::paths::RESOURCE))?;
        let action = crate::state::cedar::request_action(input.action.as_ref())
            .ok_or_else(|| required(crate::ops::paths::ACTION))?;

        // The context travels with the member it came from, so a refusal the
        // evaluation raises about it names the union arm the client sent.
        let context = crate::state::cedar::RequestContext {
            json: crate::state::cedar::context_to_cedar_json(input.context.as_ref())
                .map_err(invalid)?,
            path: crate::state::cedar::context_path(input.context.as_ref()),
        };
        let entities = crate::state::cedar::entities_to_cedar_json(input.entities.as_ref())
            .map_err(invalid)?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            IsAuthorizedErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
        let store = state.policy_stores.get(&store_id).ok_or_else(not_found)?;

        let verdict = crate::state::cedar::authorize(
            store,
            &principal,
            &action,
            &resource,
            context,
            entities,
        )
        .map_err(|refusal| match refusal {
            // A request member the caller can fix is the modelled
            // `ValidationException`; service-side state it cannot — a stored
            // schema that will not parse, a store holding more Cedar than one
            // request evaluates — is a server fault, not a bad request.
            crate::state::cedar::Refusal::InvalidInput(field) => invalid(field),
            crate::state::cedar::Refusal::ServiceFault(message) => {
                IsAuthorizedErrorModeled::InternalServerException(InternalServerException {
                    message,
                })
                .into()
            }
            // An authorization request carries no policy content, so the store's
            // schema has no member of this REQUEST to refuse: reasons reaching
            // here are about policies the store already holds, which is
            // service-side state and not something the caller sent.
            crate::state::cedar::Refusal::SchemaValidation(reasons) => {
                IsAuthorizedErrorModeled::InternalServerException(InternalServerException {
                    message: reasons.join("; "),
                })
                .into()
            }
        })?;

        Ok(IsAuthorizedOutput {
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
        })
    }
}

/// Why an absent member of the authorization triple is refused. The three
/// members are modelled optional, so the boundary validator cannot require them
/// and this is the place that does.
const REQUIRED_FOR_EVALUATION: &str = "it is required to evaluate an authorization request";

/// One offending request member as this operation's modelled
/// `ValidationException`.
fn invalid(invalid: crate::ops::InvalidInput) -> IsAuthorizedError {
    IsAuthorizedErrorModeled::ValidationException(crate::ops::validation_exception([invalid])).into()
}
