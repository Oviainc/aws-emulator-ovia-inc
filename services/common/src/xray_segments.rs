// SPDX-License-Identifier: CC0-1.0

//! The ONE construction of the in-process AWS X-Ray client an emulated service REPORTS its
//! own work through, and the one submission of the segment documents it reports.
//!
//! A service that records work of its own for a traced request — the API Gateway stage that
//! served it, the Lambda invocation it reached — is a PRODUCER of segments: it writes a
//! segment document and submits it with `PutTraceSegments`, which is the operation X-Ray
//! publishes for exactly that ("Uploads segment documents to Amazon Web Services X-Ray").
//! The hop is made the way any emulated service reaches another: the AWS SDK driven over the
//! emulator's own in-process transport, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so no socket is opened, no name
//! is resolved and the segments land in the very store `BatchGetTraces` and
//! `GetTraceSummaries` answer from.
//!
//! Reporting a trace is NEVER part of answering the request: a submission that could not be
//! made is traced and the call it belongs to is answered exactly as it would have been, for
//! the same reason a management-event publication is ([`super::api_call_events`]).

/// The X-Ray client a service reports its own segments through, addressed to `region`.
///
/// Configured exactly as the other in-process clients of this crate are
/// ([`super::kinesis_client`]), and for the same reasons: the transport is in-process, so
/// there is nothing to retry, time out or watch for a stalled stream, and the credentials the
/// callback hands over are the emulator's own and are used directly.
#[must_use]
pub fn xray_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_xray::Client {
    let config = ::aws_sdk_xray::Config::builder()
        .behavior_version(::aws_sdk_xray::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_xray::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://xray.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_xray::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_xray::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_xray::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_xray::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_xray::Client::from_conf(config)
}

/// `at` as the number of seconds since the epoch a SEGMENT DOCUMENT carries an instant as:
/// "the time the segment was created, in floating point seconds in epoch time"
/// (`PutTraceSegments`).
///
/// The one rendering, for every producer: a trace whose documents came from two services must
/// carry one service's instants the same way as the other's, or a reader comparing a child's
/// times with its parent's compares two different renderings.
#[must_use]
pub fn epoch_seconds(at: std::time::SystemTime) -> f64 {
    at.duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// Submit `documents` to X-Ray as the segments this service recorded for a traced request.
///
/// A submission nothing answered, or one X-Ray refused, is TRACED and nothing else: the
/// request that produced the segments is answered either way, because what a service reports
/// about its own work is not part of what it answers. An empty list submits nothing.
pub async fn report_segments(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    documents: Vec<String>,
) {
    if documents.is_empty() {
        return;
    }
    let client = xray_client(ctx, region);
    match client
        .put_trace_segments()
        .set_trace_segment_documents(Some(documents))
        .send()
        .await
    {
        Ok(answered) => {
            for unprocessed in answered.unprocessed_trace_segments() {
                ::tracing::warn!(
                    segment_id = unprocessed.id().unwrap_or_default(),
                    message = unprocessed.message().unwrap_or_default(),
                    "a segment this service recorded was not accepted by X-Ray"
                );
            }
        }
        Err(error) => ::tracing::warn!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the segments this service recorded were not submitted"
        ),
    }
}
