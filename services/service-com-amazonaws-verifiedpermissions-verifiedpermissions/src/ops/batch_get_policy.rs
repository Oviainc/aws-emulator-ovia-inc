// SPDX-License-Identifier: CC0-1.0

//! @op BatchGetPolicy
//!
//! `@readonly`: reports the persisted policies named by `requests`, one answer per
//! request item, and writes nothing.
//!
//! A batch read reports a per-item miss in `errors` rather than failing the whole
//! call: a reference that names nothing contributes a `BatchGetPolicyErrorItem`
//! carrying the code for WHICH reference missed, while every other item is still
//! reported in `results`. Both lists keep the request's order.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `BatchGetPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Retrieves information about a group (batch) of policies. The BatchGetPolicy operation
//! doesn't have its own IAM permission. To authorize this operation for Amazon Web Services
//! principals, include the permission verifiedpermissions:GetPolicy in their IAM policies.
//!
//! ## Input
//!
//! * `requests` (BatchGetPolicyInputList, len 1..100) **required** — An array of up to 100
//!   policies you want information about.
//!
//! ## Output
//!
//! * `errors` (BatchGetPolicyErrorList) **required** — Information about the policies from
//!   the request that resulted in an error. These results are returned in the order they were
//!   requested.
//! * `results` (BatchGetPolicyOutputList) **required** — Information about the policies
//!   listed in the request that were successfully returned. These results are returned in the
//!   order they were requested.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `BatchGetPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `BatchGetPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#BatchGetPolicy",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum BatchGetPolicyErrorModeled {
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::batch_get_policy::BatchGetPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type BatchGetPolicyError = crate::scaffolded::Scaffolded<BatchGetPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<BatchGetPolicyErrorModeled> for BatchGetPolicyError {
    fn from(modeled: BatchGetPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `BatchGetPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct BatchGetPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> BatchGetPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for BatchGetPolicy<Eng>
{
    type Input = BatchGetPolicyInput;
    type Output = BatchGetPolicyOutput;
    type Error = BatchGetPolicyError;

    const NAME: &'static str = "BatchGetPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_GET_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_GET_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            BatchGetPolicyErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();

        // `results` and `errors` are each built by walking `requests` once, in
        // order, so "returned in the order they were requested" holds for both
        // lists however hits and misses are interleaved. A duplicated request
        // item is answered twice, because each item is answered on its own.
        let mut results: Vec<BatchGetPolicyOutputItem> = Vec::with_capacity(input.requests.len());
        let mut errors: Vec<BatchGetPolicyErrorItem> = Vec::new();

        for item in &input.requests {
            // The three `BatchGetPolicyErrorCode` values name the three distinct
            // references one item carries: the alias in a
            // `policy-store-alias/<name>` store reference, the store that
            // reference resolves to, and the policy inside it. Each miss is
            // reported as the code for the reference that missed.
            let store_id = match state.lookup_store_id(&item.policy_store_id) {
                Ok(store_id) => store_id,
                Err(crate::state::StoreLookupMiss::AliasNotFound) => {
                    let alias = item
                        .policy_store_id
                        .strip_prefix(crate::state::ALIAS_PREFIX)
                        .unwrap_or(&item.policy_store_id);
                    errors.push(error_item(
                        "POLICY_STORE_ALIAS_NOT_FOUND",
                        item,
                        format!("No policy store alias exists with the input name: {alias}"),
                    ));
                    continue;
                }
                Err(crate::state::StoreLookupMiss::StoreNotFound) => {
                    errors.push(error_item(
                        "POLICY_STORE_NOT_FOUND",
                        item,
                        format!(
                            "No policy store exists with the input ID: {}",
                            item.policy_store_id
                        ),
                    ));
                    continue;
                }
            };
            let policy = state.policy_stores.get(&store_id).and_then(|store| {
                store
                    .resolve_policy_id(&item.policy_id)
                    .and_then(|policy_id| store.policies.get(&policy_id))
            });
            let Some(policy) = policy else {
                errors.push(error_item(
                    "POLICY_NOT_FOUND",
                    item,
                    format!("No policy exists with the input ID: {}", item.policy_id),
                ));
                continue;
            };

            // The union arm follows what the stored policy HOLDS: its own Cedar
            // text, or the template it was instantiated from.
            let definition = super::get_policy::policy_definition_detail(policy);

            results.push(BatchGetPolicyOutputItem {
                // The RESOLVED store id and policy id, not the references the
                // caller wrote: an alias or a `name/<policyName>` reference
                // reports the resource it names.
                policy_store_id: policy.policy_store_id.clone(),
                policy_id: policy.policy_id.clone(),
                policy_type: policy.policy_type.clone(),
                definition,
                created_date: policy.created_date,
                last_updated_date: policy.last_updated_date,
                name: policy.name.clone(),
            });
        }

        Ok(BatchGetPolicyOutput { results, errors })
    }
}

/// One per-item miss. `code` is a [`BATCH_GET_POLICY_ERROR_CODE_VALUES`] value, and
/// the echoed references are the request item's own, so a caller can pair the error
/// with the item it asked about.
fn error_item(
    code: &str,
    item: &BatchGetPolicyInputItem,
    message: String,
) -> BatchGetPolicyErrorItem {
    debug_assert!(
        BATCH_GET_POLICY_ERROR_CODE_VALUES.contains(&code),
        "{code} is not a modeled BatchGetPolicyErrorCode"
    );
    BatchGetPolicyErrorItem {
        code: code.to_string(),
        policy_store_id: item.policy_store_id.clone(),
        policy_id: item.policy_id.clone(),
        message,
    }
}
