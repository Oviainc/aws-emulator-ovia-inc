// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs side of an app monitor's `DataStorage.CwLog`: the permission a
//! caller needs to turn log delivery on, and the vended log group RUM provisions when it
//! is on.
//!
//! Two things happen when a `CreateAppMonitor` or `UpdateAppMonitor` request turns
//! `CwLogEnabled` on, and both are RUM's side of a MANAGED side effect — neither is the
//! client's call to make:
//!
//! 1. **the caller must hold `logs:PutResourcePolicy`.** RUM copies the monitor's
//!    telemetry into a log group in the caller's own account, which means installing a
//!    resource policy on that log group on the caller's behalf, so a caller that may not
//!    install one may not turn delivery on either. The check is an identity-based
//!    decision in the CloudWatch Logs namespace ([`authorization::LOGS_PUT_RESOURCE_POLICY`])
//!    against the log group's ARN, and it is made BEFORE anything is written: a refused
//!    request leaves `CwLogEnabled` exactly as it was.
//! 2. **the log group is created.** `GetAppMonitor` reports
//!    `DataStorage.CwLog.CwLogGroup` for a monitor with delivery on, and that name has to
//!    name a REAL CloudWatch Logs log group — a client that reads it and then tails it,
//!    sets retention on it or reads its policy must find it there. So RUM creates it, by
//!    calling the emulator's own CloudWatch Logs service over the in-process callback
//!    (`logs:CreateLogGroup`), exactly as the managed service does for a vended-logs
//!    delivery.
//!
//! Two consequences of provisioning that group, both deliberate and both the shape b2bi's
//! `CreateProfile` already has for its own log group:
//!
//! * a router that mounts RUM WITHOUT the CloudWatch Logs service cannot provision it, so
//!   a request that turns delivery on answers `InternalServerException` there rather than
//!   advertising a `CwLogGroup` that resolves nowhere. The assembled emulator is not such
//!   a router — `core/router`'s `build_router` registers every service crate in the
//!   workspace — and this crate's own tests mount the pair explicitly;
//! * the group is created before the record is written, so a call whose future is dropped
//!   at that `.await` can leave the group behind. It is named after the app monitor
//!   ([`super::cw_log_group_name`]), so the next call that turns delivery on for the same
//!   monitor adopts that very group rather than colliding with it.
//!
//! Turning delivery OFF needs neither: no policy is installed, and the group is left
//! alone, because the telemetry already in it belongs to the account (the same reason
//! b2bi's `DeleteProfile` leaves its group behind) and nothing documents RUM deleting a
//! customer's logs.

use super::authorization;

/// A CloudWatch Logs client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here. `region` is the
/// request's own signing region, so the group lands where the monitor's ARN says it does.
fn logs_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_cloudwatchlogs::Client {
    let config = ::aws_sdk_cloudwatchlogs::Config::builder()
        .behavior_version(::aws_sdk_cloudwatchlogs::config::BehaviorVersion::latest())
        .region(::aws_sdk_cloudwatchlogs::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://logs.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_cloudwatchlogs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudwatchlogs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudwatchlogs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudwatchlogs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudwatchlogs::Client::from_conf(config)
}

/// Whether the caller may have RUM turn log delivery on: does it hold
/// `logs:PutResourcePolicy` on the log group RUM would install the delivery policy on?
///
/// Identity-based only: the resource lives in CloudWatch Logs, so there is no RUM resource
/// policy to consult for it.
pub fn caller_may_enable_delivery<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    app_monitor_name: &str,
) -> Result<(), authorization::Denied> {
    let log_group = super::cw_log_group_name(app_monitor_name);
    authorization::authorize_identity(
        engine,
        ctx,
        authorization::LOGS_PUT_RESOURCE_POLICY,
        &super::cw_log_group_arn(region, &log_group),
    )
}

/// Create the vended log group for `app_monitor_name`, so the `CwLogGroup` RUM reports
/// names a group `logs:DescribeLogGroups` finds.
///
/// `Ok(())` once the group exists — whether this call created it or it was already there,
/// since a group under that name is exactly what this step owes the caller. `Err` carries
/// what CloudWatch Logs said, for the modelled server error the operation answers with.
pub async fn provision_log_group(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    app_monitor_name: &str,
) -> Result<(), String> {
    let log_group = super::cw_log_group_name(app_monitor_name);
    match logs_client(ctx, region)
        .create_log_group()
        .log_group_name(&log_group)
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|service| service.is_resource_already_exists_exception()) =>
        {
            Ok(())
        }
        Err(error) => Err(
            ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string(),
        ),
    }
}
