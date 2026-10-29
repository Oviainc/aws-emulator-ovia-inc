// SPDX-License-Identifier: CC0-1.0

//! @op StopAutoManagement
//!
//! Stops Automatic Management for the account AND removes the configurations
//! associated with it: the whole stored record goes, with the `OptInLevel`,
//! `OptInType`, `NotificationArn` and `ExclusionList` it carried (see
//! [`crate::state::ComAmazonawsServicequotasServicequotasv20190624State::clear_auto_management`]).
//! The account is then back to where it was before it ever opted in, which is what
//! `GetAutoManagementConfiguration` reports as `OptInStatus = DISABLED` with no other
//! member set. `StartAutoManagement` starts it again, and has to be given the whole
//! configuration afresh.
//!
//! A stop for an account that is not running Automatic Management — one that never
//! started it, or has already stopped it — is refused with `NoSuchResourceException`.
//!
//! ---
//!
//! # Modeled contract for `StopAutoManagement`
//!
//! For the FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Stops Service Quotas Automatic Management for an Amazon Web Services account and removes
//! all associated configurations. Automatic Management monitors your Service Quotas
//! utilization and notifies you before you run out of your allocated quotas.
//!
//! ## Input / Output
//!
//! Neither the request nor the response carries any members.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permission to perform
//!   this action.
//! * `IllegalArgumentException` (HTTP 400) — Invalid input was provided.
//! * `NoSuchResourceException` (HTTP 404) — The specified resource does not exist.
//! * `ServiceException` (HTTP 500) — Something went wrong.
//! * `TooManyRequestsException` (HTTP 429) — Due to throttling, the request was denied. Slow
//!   down the rate of request calls, or request an increase for this quota.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.servicequotas#StopAutoManagement")]
#[derive(Debug)]
pub enum StopAutoManagementErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type StopAutoManagementError = crate::scaffolded::Scaffolded<StopAutoManagementErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<StopAutoManagementErrorModeled> for StopAutoManagementError {
    fn from(modeled: StopAutoManagementErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StopAutoManagement` operation.
pub struct StopAutoManagement<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StopAutoManagement<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StopAutoManagement<Eng>
{
    type Input = StopAutoManagementInput;
    type Output = StopAutoManagementOutput;
    type Error = StopAutoManagementError;

    const NAME: &'static str = "StopAutoManagement";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::STOP_AUTO_MANAGEMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::STOP_AUTO_MANAGEMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        _input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StopAutoManagementErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        let mut quotas_state = state.lock();
        // Only a RUNNING configuration can be stopped: `None` is an account that never
        // opted in, or one whose stop already removed the record it had.
        let running = quotas_state
            .auto_management()
            .is_some_and(|config| config.opt_in_status == crate::state::OPT_IN_STATUS_ENABLED);
        if !running {
            return Err(
                StopAutoManagementErrorModeled::NoSuchResourceException(NoSuchResourceException {
                    message: Some(
                        "Automatic Management is not started for this account, so there is \
                         nothing to stop"
                            .to_string(),
                    ),
                })
                .into(),
            );
        }

        // Stopping removes all the configurations associated with Automatic Management,
        // so nothing of the stopped opt-in is left on record to be reported or reused.
        quotas_state.clear_auto_management();

        Ok(StopAutoManagementOutput {})
    }
}
