// SPDX-License-Identifier: CC0-1.0

//! @op ListPolicies
//!
//! `@readonly`, `@paginated(items=policies, inputToken/outputToken=nextToken,
//! pageSize=maxResults)`: enumerates the store's persisted policies in a stable
//! order, applies `filter`, truncates at `maxResults` and hands back an opaque
//! `nextToken` that resumes exactly where the page stopped. It writes nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListPolicies`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Returns a paginated list of all policies stored in the specified policy store.
//!
//! ## Input
//!
//! * `filter` (PolicyFilter) — Specifies a filter that limits the response to only policies
//!   that match the specified criteria. For example, you list only the policies that
//!   reference a specified principal.
//! * `maxResults` (MaxResults, range 1..) — Specifies the total number of results that you
//!   want included in each response. If additional items exist beyond the number you specify,
//!   the NextToken response element is returned with a value (not null). Include the
//!   specified value as the NextToken request parameter in the next call to the operation to
//!   get the next set of results. Note that the service might return fewer results than the
//!   maximum even when there are more results available. You should check NextToken after
//!   every operation to ensure that you receive all of the results. If you do not specify
//!   this parameter, the operation defaults to 10 policies per response. You can specify a
//!   maximum of 50 policies per response.
//! * `nextToken` (NextToken, len 1..8000, pattern ^[A-Za-z0-9-_=+/\.]*$) — Specifies that you
//!   want to receive the next page of results. Valid only if you received a NextToken
//!   response in the previous request. If you did, it indicates that more output is
//!   available. Set this parameter to the value provided by the previous call's NextToken
//!   response to request the next page of results.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store you want to list policies from. To specify a policy
//!   store, use its ID or alias name. When using an alias name, prefix it with
//!   policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//!
//! ## Output
//!
//! * `nextToken` (NextToken, len 1..8000, pattern ^[A-Za-z0-9-_=+/\.]*$) — If present, this
//!   value indicates that more output is available than is included in the current response.
//!   Use this value in the NextToken request parameter in a subsequent call to the operation
//!   to get the next part of the output. You should repeat this until the NextToken response
//!   element comes back as null. This indicates that this is the last page of results.
//! * `policies` (PolicyList) **required** — Lists all policies that are available in the
//!   specified policy store.
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
// `ListPoliciesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListPoliciesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#ListPolicies",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum ListPoliciesErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_policies::ListPoliciesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListPoliciesError = crate::scaffolded::Scaffolded<ListPoliciesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListPoliciesErrorModeled> for ListPoliciesError {
    fn from(modeled: ListPoliciesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListPolicies` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListPolicies<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListPolicies<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListPolicies<Eng>
{
    type Input = ListPoliciesInput;
    type Output = ListPoliciesOutput;
    type Error = ListPoliciesError;

    const NAME: &'static str = "ListPolicies";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_POLICIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_POLICIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let not_found = || {
            ListPoliciesErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };

        // A cursor this service did not mint is a bad request, not an empty page:
        // answering an empty page would hide the client's mistake.
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(crate::state::decode_page_token(token).ok_or_else(|| {
                ListPoliciesErrorModeled::ValidationException(crate::ops::validation_exception([
                    crate::ops::InvalidInput::new(
                        crate::ops::paths::NEXT_TOKEN,
                        "it is not a cursor this service issued",
                    ),
                ]))
            })?),
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListPoliciesErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
        let store = state.policy_stores.get(&store_id).ok_or_else(not_found)?;

        // `policies` is a `BTreeMap`, so the enumeration order is stable across
        // calls and a cursor resumes at the first key at or after it.
        let matching = store
            .policies
            .iter()
            .filter(|(policy_id, _)| {
                resume_from
                    .as_ref()
                    .is_none_or(|from| policy_id.as_str() >= from.as_str())
            })
            .filter(|(_, policy)| matches_filter(policy, input.filter.as_ref()));

        // No `maxResults` does NOT mean "the whole store in one response": the
        // operation is `@paginated`, so an absent page size takes this service's
        // own default and the caller pages for the rest. An asked-for size is
        // clamped to the maximum, so one small request can never materialise an
        // arbitrarily large response, and to at least one, so a page is
        // structurally non-empty: a zero page size would hand back an empty page
        // and a cursor that never advances, which is a client loop that never
        // ends. The model's `range min 1` already refuses zero at the boundary;
        // this does not depend on that.
        let page_size = input
            .max_results
            .map_or(crate::state::DEFAULT_PAGE_SIZE, |n| {
                usize::try_from(n)
                    .unwrap_or(crate::state::DEFAULT_PAGE_SIZE)
                    .clamp(1, crate::state::MAX_PAGE_SIZE)
            });
        let mut policies: Vec<PolicyItem> = Vec::new();
        let mut next_token = None;
        for (policy_id, policy) in matching {
            if policies.len() == page_size {
                // The page stopped here, so the cursor names THIS key: the next
                // call resumes with it as its first candidate.
                next_token = Some(crate::state::encode_page_token(policy_id));
                break;
            }
            policies.push(policy_item(policy));
        }

        Ok(ListPoliciesOutput {
            next_token,
            policies,
        })
    }
}

/// Does `policy` pass `filter`? An absent filter matches everything, and every
/// member a filter sets must match (the members are ANDed).
fn matches_filter(policy: &crate::state::Policy, filter: Option<&PolicyFilter>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    if let Some(policy_type) = filter.policy_type.as_deref() {
        if policy.policy_type != policy_type {
            return false;
        }
    }
    if let Some(policy_template_id) = filter.policy_template_id.as_deref() {
        // Only a template-linked policy was instantiated from a template, so
        // this member selects the policies that follow exactly that template.
        if policy.policy_template_id() != Some(policy_template_id) {
            return false;
        }
    }
    if let Some(reference) = filter.principal.as_ref() {
        if !matches_entity_reference(policy.principal.as_ref(), reference) {
            return false;
        }
    }
    if let Some(reference) = filter.resource.as_ref() {
        if !matches_entity_reference(policy.resource.as_ref(), reference) {
            return false;
        }
    }
    true
}

/// `EntityReference::Unspecified(true)` selects policies whose scope names no
/// entity; `Identifier(e)` selects policies whose scope names exactly `e`.
fn matches_entity_reference(
    scope: Option<&crate::state::ScopeEntity>,
    reference: &EntityReference,
) -> bool {
    match reference {
        EntityReference::Unspecified(unspecified) => scope.is_none() == *unspecified,
        EntityReference::Identifier(identifier) => scope.is_some_and(|entity| {
            entity.entity_type == identifier.entity_type
                && entity.entity_id == identifier.entity_id
        }),
    }
}

/// A persisted policy as the modeled `PolicyItem`. `PolicyDefinitionItem` carries
/// the policy's description but not its statement, which is what the model says;
/// for a template-linked policy it carries the template the policy follows and
/// the entities it bound that template's placeholders to.
fn policy_item(policy: &crate::state::Policy) -> PolicyItem {
    PolicyItem {
        policy_store_id: policy.policy_store_id.clone(),
        policy_id: policy.policy_id.clone(),
        policy_type: policy.policy_type.clone(),
        principal: policy
            .principal
            .as_ref()
            .map(super::create_policy::entity_identifier),
        resource: policy
            .resource
            .as_ref()
            .map(super::create_policy::entity_identifier),
        actions: (!policy.actions.is_empty()).then(|| {
            policy
                .actions
                .iter()
                .map(super::create_policy::action_identifier)
                .collect()
        }),
        definition: match policy.template_link.as_ref() {
            Some(link) => {
                PolicyDefinitionItem::templateLinked(TemplateLinkedPolicyDefinitionItem {
                    policy_template_id: link.policy_template_id.clone(),
                    principal: link
                        .principal
                        .as_ref()
                        .map(super::create_policy::entity_identifier),
                    resource: link
                        .resource
                        .as_ref()
                        .map(super::create_policy::entity_identifier),
                })
            }
            None => PolicyDefinitionItem::Static(StaticPolicyDefinitionItem {
                description: policy.description.clone(),
            }),
        },
        created_date: policy.created_date,
        last_updated_date: policy.last_updated_date,
        effect: policy.effect.clone(),
        name: policy.name.clone(),
    }
}
