// SPDX-License-Identifier: CC0-1.0

//! @op StartAutoManagement
//!
//! Starts Service Quotas Automatic Management for the account and records the whole
//! configuration the call carries, so `GetAutoManagementConfiguration` reads back
//! `OptInStatus = ENABLED` with the opt-in level, opt-in type, notification ARN and
//! exclusion list the caller asked for.
//!
//! The configuration is ONE record for the whole account, not one per region:
//! `OptInLevel` has the single member `ACCOUNT`, so every regional endpoint reports
//! the same configuration (see [`crate::state::AutoManagementConfig`]).
//!
//! Starting is a FULL write of the configuration — every member of it is on this
//! call — so a start while Automatic Management is already running, or after it was
//! stopped, replaces the record and leaves it `ENABLED` rather than refusing or
//! merging. `UpdateAutoManagement` is the partial writer, and it is the only one.
//!
//! The exclusion list is stored as given — each service's codes in the order the
//! caller named them — and its entries are NOT checked against the quota catalog,
//! because that catalog is a subset of AWS's, so refusing an entry it does not know
//! would refuse a service AWS does know. An excluded quota code the catalog does know
//! gains its `QuotaName` when the configuration is read back.
//!
//! Every modelled input constraint — `OptInLevel`'s and `OptInType`'s value sets, the
//! `NotificationArn` length and pattern, the exclusion list's key and member
//! patterns — is refused by the boundary validator BEFORE this handler runs, so
//! reaching here means the configuration is well formed and there is nothing left
//! for this handler to refuse.
//!
//! Accepting and storing `OptInType = NotifyAndAdjust` is deliberate: the model's
//! `OptInType` enum declares it, so the boundary validator admits it and refusing it
//! here would refuse a value the model allows. The doc below adds "Currently, only
//! NotifyOnly is available", which is an availability note on the real service's
//! rollout rather than a modelled constraint — so a real call naming
//! `NotifyAndAdjust` may be refused where this emulator records it.
//!
//! ---
//!
//! # Modeled contract for `StartAutoManagement`
//!
//! For the FULL text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json")
//! ```
//!
//! Starts Service Quotas Automatic Management for an Amazon Web Services account, including
//! notification preferences and excluded quotas configurations. Automatic Management
//! monitors your Service Quotas utilization and notifies you before you run out of your
//! allocated quotas.
//!
//! ## Input
//!
//! * `ExclusionList` (ExclusionList) — List of Amazon Web Services services excluded from
//!   Automatic Management. You won't be notified of Service Quotas utilization for Amazon Web
//!   Services services added to the Automatic Management exclusion list.
//! * `NotificationArn` (AmazonResourceName, len 1..1011, pattern
//!   ^arn:aws(-[\w]+)*:*:.+:[0-9]{12}:.+$) — The User Notifications Amazon Resource Name
//!   (ARN) for Automatic Management notifications.
//! * `OptInLevel` (OptInLevel, one of: ACCOUNT) **required** — Sets the opt-in level for
//!   Automatic Management. Only Amazon Web Services account level is supported.
//! * `OptInType` (OptInType, one of: NotifyAndAdjust | NotifyOnly) **required** — Sets the
//!   opt-in type for Automatic Management. There are two modes: Notify only and Notify and
//!   Auto-Adjust. Currently, only NotifyOnly is available.
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
#[smithy_macros::operation_error("com.amazonaws.servicequotas#StartAutoManagement")]
#[derive(Debug)]
pub enum StartAutoManagementErrorModeled {
    AccessDeniedException(AccessDeniedException),
    IllegalArgumentException(IllegalArgumentException),
    NoSuchResourceException(NoSuchResourceException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type StartAutoManagementError = crate::scaffolded::Scaffolded<StartAutoManagementErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<StartAutoManagementErrorModeled> for StartAutoManagementError {
    fn from(modeled: StartAutoManagementErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartAutoManagement` operation.
pub struct StartAutoManagement<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartAutoManagement<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartAutoManagement<Eng>
{
    type Input = StartAutoManagementInput;
    type Output = StartAutoManagementOutput;
    type Error = StartAutoManagementError;

    const NAME: &'static str = "StartAutoManagement";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_AUTO_MANAGEMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_AUTO_MANAGEMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The configuration is account-wide, so the caller's region does not pick a
        // state bucket here the way it does for the per-region quota state.
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StartAutoManagementErrorModeled::ServiceException(ServiceException {
                message: Some("the Service Quotas state store is unavailable".to_string()),
            })
        })?;

        state
            .lock()
            .set_auto_management(crate::state::AutoManagementConfig {
                opt_in_level: input.opt_in_level,
                opt_in_type: input.opt_in_type,
                opt_in_status: crate::state::OPT_IN_STATUS_ENABLED.to_string(),
                notification_arn: input.notification_arn,
                exclusion_list: crate::state::stored_exclusion_list(input.exclusion_list),
            });

        Ok(StartAutoManagementOutput {})
    }
}
