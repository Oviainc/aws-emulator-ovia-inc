// SPDX-License-Identifier: CC0-1.0

//! @op UpdateAutoManagement
//!
//! Changes the Automatic Management configuration members this call names, and
//! leaves every member it omits as it was — the PARTIAL writer of the account's
//! configuration, where `StartAutoManagement` writes the whole of it.
//!
//! Every member of the request is optional, so an omitted `OptInType`,
//! `NotificationArn` or `ExclusionList` means "leave this one alone"; an
//! `ExclusionList` that IS named replaces the stored one outright, because the
//! member is the list, not an addition to it.
//!
//! There is nothing to update until Automatic Management is running: a call for an
//! account that never started it, or that has stopped it, is refused with
//! `NoSuchResourceException`.
//!
//! An `ExclusionList` is stored as given — each service's codes in the order the
//! caller named them — the way `StartAutoManagement` stores it.
//!
//! ---
//!
//! # Modeled contract for `UpdateAutoManagement`
//!
//! For the FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Updates your Service Quotas Automatic Management configuration, including notification
//! preferences and excluded quotas. Automatic Management monitors your Service Quotas
//! utilization and notifies you before you run out of your allocated quotas.
//!
//! ## Input
//!
//! * `ExclusionList` (ExclusionList) — List of Amazon Web Services services you want to
//!   exclude from Automatic Management. You won't be notified of Service Quotas utilization
//!   for Amazon Web Services services added to the Automatic Management exclusion list.
//! * `NotificationArn` (AmazonResourceName, len 1..1011, pattern
//!   ^arn:aws(-[\w]+)*:*:.+:[0-9]{12}:.+$) — The User Notifications Amazon Resource Name
//!   (ARN) for Automatic Management notifications you want to update.
//! * `OptInType` (OptInType, one of: NotifyAndAdjust | NotifyOnly) — Information on the
//!   opt-in type for your Automatic Management configuration. There are two modes: Notify
//!   only and Notify and Auto-Adjust. Currently, only NotifyOnly is available.
//!
//! ## Output
//!
//! The response carries no members.
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
#[smithy_macros::operation_error("com.amazonaws.servicequotas#UpdateAutoManagement")]
#[derive(Debug)]
pub enum UpdateAutoManagementErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type UpdateAutoManagementError =
    crate::scaffolded::Scaffolded<UpdateAutoManagementErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<UpdateAutoManagementErrorModeled> for UpdateAutoManagementError {
    fn from(modeled: UpdateAutoManagementErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateAutoManagement` operation.
pub struct UpdateAutoManagement<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateAutoManagement<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateAutoManagement<Eng>
{
    type Input = UpdateAutoManagementInput;
    type Output = UpdateAutoManagementOutput;
    type Error = UpdateAutoManagementError;

    const NAME: &'static str = "UpdateAutoManagement";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_AUTO_MANAGEMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_AUTO_MANAGEMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateAutoManagementErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        let mut quotas_state = state.lock();
        // Only a RUNNING configuration can be updated: `None` is an account that never
        // opted in, and a `DISABLED` record is one that opted out again.
        let running = quotas_state
            .auto_management()
            .filter(|config| config.opt_in_status == crate::state::OPT_IN_STATUS_ENABLED)
            .cloned();
        let Some(mut config) = running else {
            return Err(
                UpdateAutoManagementErrorModeled::NoSuchResourceException(
                    NoSuchResourceException {
                        message: Some(
                            "Automatic Management is not started for this account, so there is \
                             no configuration to update"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };

        if let Some(opt_in_type) = input.opt_in_type {
            config.opt_in_type = opt_in_type;
        }
        if let Some(notification_arn) = input.notification_arn {
            config.notification_arn = Some(notification_arn);
        }
        if input.exclusion_list.is_some() {
            config.exclusion_list = crate::state::stored_exclusion_list(input.exclusion_list);
        }
        quotas_state.set_auto_management(config);

        Ok(UpdateAutoManagementOutput {})
    }
}
