// SPDX-License-Identifier: CC0-1.0

//! @op GetAutoManagementConfiguration
//!
//! Reports the account's Service Quotas Automatic Management configuration from state:
//! the opt-in level, type and status, the notification ARN and the exclusion list the
//! opt-in stored, with each excluded `QuotaCode` paired with the `QuotaName` the quota
//! catalog holds for it.
//!
//! This is a REPORTING operation: it writes nothing, so two reads with no intervening
//! Start/Update/Stop answer identically. An account that has never opted in has no
//! configuration record, and is reported as `OptInStatus: DISABLED` with no
//! preferences — the status enum's own "stopped" member, rather than an error, because
//! Automatic Management is an account-level setting that is simply off until it is
//! started.
//!
//! A code the catalog does not know is reported with its `QuotaCode` alone: the
//! exclusion is the caller's own, so dropping it would under-report the configuration.
//!
//! ---
//!
//! # Modeled contract for `GetAutoManagementConfiguration`
//!
//! For the FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Retrieves information about your Service Quotas Automatic Management configuration.
//! Automatic Management monitors your Service Quotas utilization and notifies you before
//! you run out of your allocated quotas.
//!
//! ## Input
//!
//! The request carries no members.
//!
//! ## Output
//!
//! * `ExclusionList` (ExclusionQuotaList) — List of Amazon Web Services services excluded
//!   from Automatic Management. You won't be notified of Service Quotas utilization for
//!   Amazon Web Services services added to the Automatic Management exclusion list.
//! * `NotificationArn` (AmazonResourceName, len 1..1011, pattern
//!   ^arn:aws(-[\w]+)*:*:.+:[0-9]{12}:.+$) — The User Notifications Amazon Resource Name
//!   (ARN) for Automatic Management notifications.
//! * `OptInLevel` (OptInLevel, one of: ACCOUNT) — Information on the opt-in level for
//!   Automatic Management. Only Amazon Web Services account level is supported.
//! * `OptInStatus` (OptInStatus, one of: DISABLED | ENABLED) — Status on whether Automatic
//!   Management is started or stopped.
//! * `OptInType` (OptInType, one of: NotifyAndAdjust | NotifyOnly) — Information on the
//!   opt-in type for Automatic Management. There are two modes: Notify only and Notify and
//!   Auto-Adjust. Currently, only NotifyOnly is available.
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
#[smithy_macros::operation_error("com.amazonaws.servicequotas#GetAutoManagementConfiguration")]
#[derive(Debug)]
pub enum GetAutoManagementConfigurationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type GetAutoManagementConfigurationError =
    crate::scaffolded::Scaffolded<GetAutoManagementConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetAutoManagementConfigurationErrorModeled> for GetAutoManagementConfigurationError {
    fn from(modeled: GetAutoManagementConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAutoManagementConfiguration` operation.
pub struct GetAutoManagementConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAutoManagementConfiguration<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAutoManagementConfiguration<Eng>
{
    type Input = GetAutoManagementConfigurationInput;
    type Output = GetAutoManagementConfigurationOutput;
    type Error = GetAutoManagementConfigurationError;

    const NAME: &'static str = "GetAutoManagementConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTO_MANAGEMENT_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_AUTO_MANAGEMENT_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The request carries no members, and the configuration is ACCOUNT scoped, so
        // neither the input nor the endpoint's region narrows what is reported.
        let _ = (&input, ctx);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetAutoManagementConfigurationErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        let quotas_state = state.lock();
        let Some(config) = quotas_state.auto_management() else {
            // Never opted in: nothing is monitored and no preference was ever given,
            // so the only thing to report is that Automatic Management is not running.
            return Ok(GetAutoManagementConfigurationOutput {
                opt_in_status: Some(crate::state::OPT_IN_STATUS_DISABLED.to_string()),
                ..GetAutoManagementConfigurationOutput::default()
            });
        };

        // An absent `ExclusionList` and an empty one say the same thing — nothing is
        // excluded — so the empty map is left off the wire rather than serialized.
        let exclusion_list = quotas_state.auto_management_exclusion_view();
        Ok(GetAutoManagementConfigurationOutput {
            opt_in_level: Some(config.opt_in_level.clone()),
            opt_in_type: Some(config.opt_in_type.clone()),
            opt_in_status: Some(config.opt_in_status.clone()),
            notification_arn: config.notification_arn.clone(),
            exclusion_list: (!exclusion_list.is_empty()).then_some(exclusion_list),
        })
    }
}
