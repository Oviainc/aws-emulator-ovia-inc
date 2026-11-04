// SPDX-License-Identifier: CC0-1.0

//! @op ListPolicyStores
//!
//! `@readonly`, `@paginated(items=policyStores, inputToken/outputToken=nextToken,
//! pageSize=maxResults)`: enumerates the persisted policy stores of the calling
//! account in a stable order, truncates at `maxResults` and hands back an opaque
//! `nextToken` that resumes exactly where the page stopped. It writes nothing.
//!
//! The listing is the ACCOUNT's stores, with no scope inside it: a policy store is
//! reached by its id alone, and `CreatePolicyStore` mints its ARN without a region
//! segment — as AWS publishes a policy-store ARN — so unlike the alias table there
//! is no per-Region partition of this enumeration to name.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListPolicyStores`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Returns a paginated list of all policy stores in the calling Amazon Web Services
//! account.
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
//!   this parameter, the operation defaults to 10 policy stores per response. You can specify
//!   a maximum of 50 policy stores per response.
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
//! * `policyStores` (PolicyStoreList) **required** — The list of policy stores in the
//!   account.
//!
//! ## Errors
//!
//! * `ValidationException` (HTTP 400) — a `nextToken` this service did not mint.
//! * `InternalServerException` (HTTP 500) — this service's state store is unreachable.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListPolicyStoresError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListPolicyStoresError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#ListPolicyStores",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum ListPolicyStoresErrorModeled {
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_policy_stores::ListPolicyStoresError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListPolicyStoresError = crate::scaffolded::Scaffolded<ListPolicyStoresErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListPolicyStoresErrorModeled> for ListPolicyStoresError {
    fn from(modeled: ListPolicyStoresErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListPolicyStores` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListPolicyStores<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListPolicyStores<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListPolicyStores<Eng>
{
    type Input = ListPolicyStoresInput;
    type Output = ListPolicyStoresOutput;
    type Error = ListPolicyStoresError;

    const NAME: &'static str = "ListPolicyStores";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_POLICY_STORES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_POLICY_STORES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Nothing about this listing depends on the request beyond its paging
        // members: a policy store belongs to the account, not to one Region, so
        // `ctx`'s metadata selects nothing here.
        let _ = ctx;

        // This operation enumerates the account's stores as a whole, so the
        // listing a cursor belongs to is the operation alone: there is no scope
        // within it to name. The cursor is decoded — and one belonging to another
        // listing refused — by the service's one pagination rule, which this
        // operation only lifts into its own error enum.
        let cursor = crate::ops::PageCursor::read(
            crate::state::page_scope(Self::NAME, None),
            input.next_token.as_deref(),
        )
        .map_err(|invalid| {
            ListPolicyStoresErrorModeled::ValidationException(crate::ops::validation_exception([
                invalid,
            ]))
        })?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListPolicyStoresErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();

        // `policy_stores` is a `BTreeMap` keyed by `policyStoreId`, so the
        // enumeration order is stable across calls and a cursor resumes exactly
        // where the previous page stopped; the page — its size, its boundary and
        // the cursor that resumes it — is built by the service's one pagination
        // rule. The operation models no filter, so every store the account holds
        // is selected.
        //
        // The caller's `max_results` is the only thing a request sizes here, and
        // it never sizes the response: that rule clamps it to
        // `crate::state::MAX_PAGE_SIZE` before a single item is materialised, so
        // the response this builds is bounded whatever the request asked for, and
        // the rest is a bounded walk of the map under the lock.
        let (policy_stores, next_token) = crate::ops::page(
            &state.policy_stores,
            &cursor,
            input.max_results,
            crate::state::DEFAULT_PAGE_SIZE,
            |_| true,
            policy_store_item,
        );

        Ok(ListPolicyStoresOutput {
            next_token,
            policy_stores,
        })
    }
}

/// A persisted policy store as the modeled `PolicyStoreItem`.
///
/// The item carries the store's id, ARN, timestamps and description and NOTHING
/// else, which is what the model says it holds: the validation mode, the
/// deletion-protection state, the Cedar version, the encryption state and the
/// tags are reported by `GetPolicyStore` alone. Every member is read out of
/// state, so a value that was never persisted cannot appear on the wire, and each
/// is what that read reports for the same stored store.
fn policy_store_item(store: &crate::state::PolicyStore) -> PolicyStoreItem {
    PolicyStoreItem {
        policy_store_id: store.policy_store_id.clone(),
        arn: store.arn.clone(),
        created_date: store.created_date,
        last_updated_date: Some(store.last_updated_date),
        description: store.description.clone(),
    }
}
