// SPDX-License-Identifier: CC0-1.0

//! @op ListPolicyStoreAliases
//!
//! `@readonly`, `@paginated(items=policyStoreAliases, inputToken/outputToken=nextToken,
//! pageSize=maxResults)`: enumerates the account's persisted policy store
//! aliases in a stable order, applies `filter`, truncates at `maxResults` and
//! hands back an opaque `nextToken` that resumes exactly where the page stopped.
//! It writes nothing.
//!
//! Every alias the account holds is reported, whatever its `state`: a
//! soft-deleted alias stays in state as `PendingDeletion` (see
//! `delete_policy_store_alias.rs`) and `PolicyStoreAliasItem$state` is
//! **required**, so an alias in that state is listed and says so rather than
//! disappearing from the list.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListPolicyStoreAliases`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Returns a paginated list of all policy store aliases in the calling Amazon Web Services
//! account.
//!
//! ## Input
//!
//! * `filter` (PolicyStoreAliasFilter) — Specifies a filter to narrow the results. You can
//!   filter by policyStoreId to list only the policy store aliases associated with a specific
//!   policy store.
//! * `maxResults` (MaxResults, range 1.., default 5) **required** — Specifies the total
//!   number of results that you want included in each response. If additional items exist
//!   beyond the number you specify, the NextToken response element is returned with a value
//!   (not null). Include the specified value as the NextToken request parameter in the next
//!   call to the operation to get the next set of results. Note that the service might return
//!   fewer results than the maximum even when there are more results available. You should
//!   check NextToken after every operation to ensure that you receive all of the results. If
//!   you do not specify this parameter, the operation defaults to 5 policy store aliases per
//!   response. You can specify a maximum of 50 policy store aliases per …
//! * `nextToken` (NextToken, len 1..8000, pattern ^[A-Za-z0-9-_=+/\.]*$) — Specifies that you
//!   want to receive the next page of results. Valid only if you received a NextToken
//!   response in the previous request. If you did, it indicates that more output is
//!   available. Set this parameter to the value provided by the previous call's NextToken
//!   response to request the next page of results.
//!
//! ## Output
//!
//! * `nextToken` (NextToken, len 1..8000, pattern ^[A-Za-z0-9-_=+/\.]*$) — If present, this
//!   value indicates that more output is available than is included in the current response.
//!   Use this value in the NextToken request parameter in a subsequent call to the operation
//!   to get the next part of the output. You should repeat this until the NextToken response
//!   element comes back as null. This indicates that this is the last page of results.
//! * `policyStoreAliases` (PolicyStoreAliasList) **required** — The list of policy store
//!   aliases in the account.
//!
//! ## Errors
//!
//! * `ValidationException` (HTTP 400) — the `nextToken` is not a cursor this
//!   service issued.
//! * `InternalServerException` (HTTP 500) — the state store is unavailable.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListPolicyStoreAliasesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The page size this operation uses when the caller names no `maxResults`, from
// `ListPolicyStoreAliasesInput$maxResults`'s own documentation: "If you do not
// specify this parameter, the operation defaults to 5 policy store aliases per
// response."
//
// It lives in `crate::state` beside the figures the service's other list
// operations page by, so no two list operations of this service page by
// constants that could drift: it differs from theirs only because this
// operation's model says 5 where theirs say 10, and the largest page any of them
// builds is the one `state::MAX_PAGE_SIZE` the shared pagination rule applies.
use crate::state::ALIAS_DEFAULT_PAGE_SIZE;

// The operation's MODELED errors. `ValidationException` and
// `InternalServerException` are service-level errors this operation's own
// `errors` list does not repeat, so they come in through `extra_errors`.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#ListPolicyStoreAliases",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum ListPolicyStoreAliasesErrorModeled {
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_policy_store_aliases::ListPolicyStoreAliasesError`).
pub type ListPolicyStoreAliasesError = crate::scaffolded::Scaffolded<ListPolicyStoreAliasesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<ListPolicyStoreAliasesErrorModeled> for ListPolicyStoreAliasesError {
    fn from(modeled: ListPolicyStoreAliasesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListPolicyStoreAliases` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListPolicyStoreAliases<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListPolicyStoreAliases<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListPolicyStoreAliases<Eng>
{
    type Input = ListPolicyStoreAliasesInput;
    type Output = ListPolicyStoreAliasesOutput;
    type Error = ListPolicyStoreAliasesError;

    const NAME: &'static str = "ListPolicyStoreAliases";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_POLICY_STORE_ALIASES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_POLICY_STORE_ALIASES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;

        // The cursor is decoded — and one belonging to another listing refused —
        // by the service's one pagination rule, which this operation only lifts
        // into its own error enum. This listing enumerates the ACCOUNT's
        // aliases rather than one store's, so its cursors name no store and
        // nothing has to be resolved before reading one.
        let cursor = crate::ops::PageCursor::read(
            crate::state::page_scope(Self::NAME, None),
            input.next_token.as_deref(),
        )
        .map_err(|invalid| {
            ListPolicyStoreAliasesErrorModeled::ValidationException(
                crate::ops::validation_exception([invalid]),
            )
        })?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListPolicyStoreAliasesErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();

        // `aliases` is a `BTreeMap` keyed by the full alias name, so the
        // enumeration order is stable across calls, and the page — its size, its
        // boundary and the cursor that resumes it — is built by the service's one
        // pagination rule. The model's `range min 1` already refuses a zero or
        // negative `maxResults` at the boundary and the modelled `@default: 5` is
        // materialised for an absent one, so what reaches the rule here is always
        // a value the caller named; the operation documents that "the service
        // might return fewer results than the maximum even when there are more
        // results available", which is what the rule's cap answers with.
        let (policy_store_aliases, next_token) = crate::ops::page(
            &state.aliases,
            &cursor,
            Some(input.max_results),
            ALIAS_DEFAULT_PAGE_SIZE,
            |alias| matches_filter(alias, input.filter.as_ref()),
            alias_item,
        );

        Ok(ListPolicyStoreAliasesOutput {
            next_token,
            policy_store_aliases,
        })
    }
}

/// Does `alias` pass `filter`? An absent filter matches everything, and every
/// member a filter sets must match (the members are ANDed).
///
/// `PolicyStoreAliasFilter$policyStoreId` is documented as "The ID of the policy
/// store to filter by", so it is compared against the store id the alias is
/// associated with. The operation models no `ResourceNotFoundException`, so a
/// filter naming a store that does not exist selects nothing instead of failing.
fn matches_filter(
    alias: &crate::state::PolicyStoreAlias,
    filter: Option<&PolicyStoreAliasFilter>,
) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    if let Some(policy_store_id) = filter.policy_store_id.as_deref() {
        if alias.policy_store_id != policy_store_id {
            return false;
        }
    }
    true
}

/// A persisted alias as the modeled `PolicyStoreAliasItem` — every member read
/// out of state, so a value that was never stored cannot appear on the wire. The
/// item carries exactly what `GetPolicyStoreAlias` reports for the same alias.
fn alias_item(alias: &crate::state::PolicyStoreAlias) -> PolicyStoreAliasItem {
    PolicyStoreAliasItem {
        alias_name: alias.alias_name.clone(),
        policy_store_id: alias.policy_store_id.clone(),
        alias_arn: alias.arn.clone(),
        created_at: alias.created_at,
        state: alias.state.clone(),
    }
}
