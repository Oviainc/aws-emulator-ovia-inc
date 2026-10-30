// SPDX-License-Identifier: CC0-1.0

//! @op ListPolicyTemplates
//!
//! `@readonly`, `@paginated(items=policyTemplates, inputToken/outputToken=nextToken,
//! pageSize=maxResults)`: enumerates the store's persisted policy templates in a
//! stable order, truncates at `maxResults` and hands back an opaque `nextToken`
//! that resumes exactly where the page stopped. It writes nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListPolicyTemplates`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Returns a paginated list of all policy templates in the specified policy store.
//!
//! ## Input
//!
//! * `maxResults` (MaxResults, range 1..) — Specifies the total number of results that you
//!   want included in each response. If additional items exist beyond the number you specify,
//!   the NextToken response element is returned with a value (not null). Include the
//!   specified value as the NextToken request parameter in the next call to the operation to
//!   get the next set of results. Note that the service might return fewer results than the
//!   maximum even when there are more results available. You should check NextToken after
//!   every operation to ensure that you receive all of the results. If you do not specify
//!   this parameter, the operation defaults to 10 policy templates per response. You can
//!   specify a maximum of 50 policy templates per response.
//! * `nextToken` (NextToken, len 1..8000, pattern ^[A-Za-z0-9-_=+/\.]*$) — Specifies that you
//!   want to receive the next page of results. Valid only if you received a NextToken
//!   response in the previous request. If you did, it indicates that more output is
//!   available. Set this parameter to the value provided by the previous call's NextToken
//!   response to request the next page of results.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that contains the policy templates you want to
//!   list. To specify a policy store, use its ID or alias name. When using an alias name,
//!   prefix it with policy-store-alias/. For example: ID: PSEXAMPLEabcdefg111111 Alias name:
//!   policy-store-alias/example-policy-store To view aliases, use ListPolicyStoreAliases.
//!
//! ## Output
//!
//! * `nextToken` (NextToken, len 1..8000, pattern ^[A-Za-z0-9-_=+/\.]*$) — If present, this
//!   value indicates that more output is available than is included in the current response.
//!   Use this value in the NextToken request parameter in a subsequent call to the operation
//!   to get the next part of the output. You should repeat this until the NextToken response
//!   element comes back as null. This indicates that this is the last page of results.
//! * `policyTemplates` (PolicyTemplatesList) **required** — The list of the policy templates
//!   in the specified policy store.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.
//! * `ValidationException` (HTTP 400) — a `nextToken` this service did not mint.
//! * `InternalServerException` (HTTP 500) — this service's state store is unreachable.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListPolicyTemplatesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListPolicyTemplatesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#ListPolicyTemplates",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum ListPolicyTemplatesErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_policy_templates::ListPolicyTemplatesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListPolicyTemplatesError = crate::scaffolded::Scaffolded<ListPolicyTemplatesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListPolicyTemplatesErrorModeled> for ListPolicyTemplatesError {
    fn from(modeled: ListPolicyTemplatesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListPolicyTemplates` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListPolicyTemplates<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListPolicyTemplates<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListPolicyTemplates<Eng>
{
    type Input = ListPolicyTemplatesInput;
    type Output = ListPolicyTemplatesOutput;
    type Error = ListPolicyTemplatesError;

    const NAME: &'static str = "ListPolicyTemplates";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_POLICY_TEMPLATES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_POLICY_TEMPLATES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let not_found = || {
            ListPolicyTemplatesErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };

        // The cursor is decoded — and a forged one refused — by the service's one
        // pagination rule, which this operation only lifts into its own error enum.
        let resume_from =
            crate::ops::page_cursor(input.next_token.as_deref()).map_err(|invalid| {
                ListPolicyTemplatesErrorModeled::ValidationException(
                    crate::ops::validation_exception([invalid]),
                )
            })?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListPolicyTemplatesErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
        let store = state.policy_stores.get(&store_id).ok_or_else(not_found)?;

        // `policy_templates` is a `BTreeMap`, so the enumeration order is stable
        // across calls, and the page — its size, its boundary and the cursor that
        // resumes it — is built by the service's one pagination rule. The
        // operation models no filter, so every template is selected.
        let (policy_templates, next_token) = crate::ops::page(
            &store.policy_templates,
            resume_from.as_deref(),
            input.max_results,
            crate::state::DEFAULT_PAGE_SIZE,
            |_| true,
            policy_template_item,
        );

        Ok(ListPolicyTemplatesOutput {
            next_token,
            policy_templates,
        })
    }
}

/// A persisted policy template as the modeled `PolicyTemplateItem`.
///
/// `PolicyTemplateItem` carries the template's description and name but NOT its
/// `statement`, which is what the model says: the Cedar template text is reported
/// only by `GetPolicyTemplate`. Every other member is what that read reports for
/// the same stored template.
fn policy_template_item(template: &crate::state::PolicyTemplate) -> PolicyTemplateItem {
    PolicyTemplateItem {
        policy_store_id: template.policy_store_id.clone(),
        policy_template_id: template.policy_template_id.clone(),
        description: template.description.clone(),
        created_date: template.created_date,
        last_updated_date: template.last_updated_date,
        name: template.name.clone(),
    }
}
