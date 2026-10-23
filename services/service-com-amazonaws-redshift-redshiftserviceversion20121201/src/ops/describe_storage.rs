// SPDX-License-Identifier: CC0-1.0

//! @op DescribeStorage
//!
//! Reports this account's storage summary: how much storage the snapshots this
//! service holds occupy, and how much the clusters it holds have provisioned.
//! Both numbers are SUMS over the store (see
//! [`crate::state::ComAmazonawsRedshiftRedshiftserviceversion20121201State::total_backup_size_in_mega_bytes`]
//! and its provisioned counterpart), so creating or deleting a cluster or a
//! snapshot moves them with no bookkeeping of their own, and an account with
//! nothing provisioned sums to `0.0` on both.
//!
//! A pure report: it writes nothing. The input is Smithy's `Unit` shape and the
//! model declares NO errors for this operation, so there is nothing a caller can
//! get wrong and no request this operation refuses.
//!
//! HAND-AUTHORED, not scaffolded: the op-stub generator declined this operation
//! because its input is the Smithy `Unit` shape (`src/ops/SKIPPED_STUBS.md`), so
//! until now it was modeled-but-unwired and the framework's unroutable-operation
//! path answered a well-formed call with an HTTP 400 `InvalidAction` — which is
//! not one of this operation's modelled outcomes. `build.rs` derives the
//! service's `operations(...)` allow-list from the `//! @op <Name>` header of
//! every `src/ops/*.rs`, so this file is what wires it. Its input and output
//! shapes live in `types/describe_storage_types.rs`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`]; never construct an
//! engine in a handler. `ctx` carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `DescribeStorage`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="DescribeStorage",model_path="services/service-com-amazonaws-redshift-redshiftserviceversion20121201/smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json")`):
//!
//! Returns account level backups storage size and provisional storage.
//!
//! ## Input
//!
//! (no members — the input is `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `TotalBackupSizeInMegaBytes` (Double) — The total amount of storage currently
//!   used for snapshots.
//! * `TotalProvisionedStorageInMegaBytes` (Double) — The total amount of storage
//!   currently provisioned.
//!
//! ## Errors
//!
//! (none — the model declares no errors for this operation)

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeStorageError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json");

// The operation's MODELED errors. The model declares NONE for `DescribeStorage`,
// so `DependentServiceUnavailableFault` is widened in: the service declares the
// shape, and it is the only modelled way to say "the control-plane store could
// not be reached". It is the sole failure this operation can answer with, and it
// is unreachable while the service is mounted on the shared engine — no REQUEST
// can provoke it, which is what the empty error list means.
#[smithy_macros::operation_error(
    "com.amazonaws.redshift#DescribeStorage",
    extra_errors = ["DependentServiceUnavailableFault"]
)]
#[derive(Debug)]
pub enum DescribeStorageErrorModeled {
    DependentServiceUnavailableFault(DependentServiceUnavailableFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_storage::DescribeStorageError`).
pub type DescribeStorageError = crate::scaffolded::Scaffolded<DescribeStorageErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<DescribeStorageErrorModeled> for DescribeStorageError {
    fn from(modeled: DescribeStorageErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeStorage` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so
/// this crate names no concrete engine type.
pub struct DescribeStorage<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeStorage<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is
    /// a construction-time dependency, so engine access is guaranteed by the time
    /// any request runs. Called by the generated `EmulatedService::new(engine)`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeStorage<Eng>
{
    type Input = DescribeStorageInput;
    type Output = DescribeStorageOutput;
    type Error = DescribeStorageError;

    const NAME: &'static str = "DescribeStorage";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_STORAGE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_STORAGE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state as st;

        // The input has no members and `ctx` carries request metadata only; the
        // store is reached through `self.engine`.
        let _ = (&input, ctx);

        let Some(state) = st::service_state(&*self.engine) else {
            return Err(
                DescribeStorageErrorModeled::DependentServiceUnavailableFault(
                    DependentServiceUnavailableFault {
                        message: Some(st::STORE_UNAVAILABLE.to_string()),
                    },
                )
                .into(),
            );
        };

        // A pure report: neither sum depends on a status the clock moves, so this
        // read settles nothing and writes nothing.
        let store = state.lock();

        Ok(DescribeStorageOutput {
            total_backup_size_in_mega_bytes: Some(store.total_backup_size_in_mega_bytes()),
            total_provisioned_storage_in_mega_bytes: Some(
                store.total_provisioned_storage_in_mega_bytes(),
            ),
        })
    }
}
