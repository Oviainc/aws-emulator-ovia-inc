// SPDX-License-Identifier: CC0-1.0

//! @op DescribePersistentAppUI
//!
//! Reports the persistent application user interface `CreatePersistentAppUI` stored
//! under `PersistentAppUIId`, projected onto the wire `PersistentAppUI`
//! ([`crate::state::PersistentAppUIRecord::to_wire`]). An id no UI carries is refused
//! with the modelled `InvalidRequestException` (HTTP 400) rather than answered empty.
//!
//! The only write this read makes is the transition the CLOCK already made due — the
//! move out of the starting status, owed at the same instant whether or not anybody
//! called — so two reads of one UI report the same thing.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `DescribePersistentAppUI::new(engine)` and drives it through the centralized request
//! walk: deserialize -> validate -> `handle` -> serialize.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The
//! store is reached through [`crate::state::service_state`].
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribePersistentAppUI`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` ...`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-emr-elasticmapreduce/smithy-model/com-amazonaws-emr-elasticmapreduce.json")
//! ```
//!
//! Describes a persistent application user interface.
//!
//! ## Input
//!
//! * `PersistentAppUIId` (XmlStringMaxLen256, len 0..256) **required** - The identifier for
//!   the persistent application user interface.
//!
//! ## Output
//!
//! * `PersistentAppUI` (PersistentAppUI) - The persistent application user interface.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) - This exception occurs when there is an internal
//!   failure in the Amazon EMR service.
//! * `InvalidRequestException` (HTTP 400) - This exception occurs when there is something
//!   wrong with user input.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-emr-elasticmapreduce.json");

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.emr#DescribePersistentAppUI")]
#[derive(Debug)]
pub enum DescribePersistentAppUIErrorModeled {
    InternalServerException(InternalServerException),
    InvalidRequestException(InvalidRequestException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_persistent_app_ui::DescribePersistentAppUIError`).
pub type DescribePersistentAppUIError = crate::scaffolded::Scaffolded<DescribePersistentAppUIErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DescribePersistentAppUIErrorModeled> for DescribePersistentAppUIError {
    fn from(modeled: DescribePersistentAppUIErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribePersistentAppUI` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct DescribePersistentAppUI<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribePersistentAppUI<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribePersistentAppUI<Eng>
{
    type Input = DescribePersistentAppUIInput;
    type Output = DescribePersistentAppUIOutput;
    type Error = DescribePersistentAppUIError;

    const NAME: &'static str = "DescribePersistentAppUI";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_PERSISTENT_APP_UI_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_PERSISTENT_APP_UI_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let now = ::std::time::SystemTime::now();

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DescribePersistentAppUIErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(
                        "the EMR persistent application user interface store is unavailable"
                            .to_string(),
                    ),
                },
            )
            .into());
        };
        let mut store = state.lock();

        // An id this account holds no UI for is a client error, not an empty success:
        // the one client error this operation models, with the id named back.
        let Some(record) = store.persistent_app_ui_mut(&input.persistent_app_ui_id, now) else {
            return Err(
                DescribePersistentAppUIErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        error_code: None,
                        message: Some(crate::state::unknown_persistent_app_ui_message(
                            &input.persistent_app_ui_id,
                        )),
                    },
                )
                .into(),
            );
        };

        Ok(DescribePersistentAppUIOutput {
            persistent_app_ui: Some(record.to_wire()),
        })
    }
}
