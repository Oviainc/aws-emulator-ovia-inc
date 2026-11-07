// SPDX-License-Identifier: CC0-1.0

//! Building another emulated service's AWS SDK client over the in-process callback.
//!
//! Several things this service does are calls to ANOTHER service of the same emulator: a
//! subscription filter delivers to its destination, an export task resolves its bucket and
//! writes its objects, an association reads a KMS key's policy. Each is made the way the
//! framework documents a handler reaching another emulated service
//! ([`smithy_server_runtime::OperationContext::callback`]) — an AWS SDK client over the
//! callback's HTTP client and credentials — so none of them opens a socket or leaves the
//! host.
//!
//! The client CONFIGURATION is the same every time, so it is stated once here: the transport
//! is in-process, which means there is nothing to retry, time out or watch for a stalled
//! stream, and each of those would need a sleep implementation the callback transport is not
//! given. The same goes for caching the identity: the credentials the callback hands over are
//! used directly, and they only sign that in-process call.
//!
//! The only caller-influenced value that reaches the client's AUTHORITY is the signing Region,
//! and every call site narrows it first ([`service_common::arn::region_label`]), so no request
//! can re-point a client at a host of its own choosing: it reaches this emulator's own
//! services and nothing else.

/// One other service's client over the in-process callback, addressing that service's
/// endpoint in `region`.
///
/// `$sdk` is the SDK crate (`aws_sdk_s3`), `$callback` the [`EmbeddedEmulator`] to call
/// through ([`smithy_server_runtime::OperationContext::callback`], or a clone of it held by a
/// worker), `$endpoint` the service's endpoint prefix and `$region` the already-narrowed
/// signing Region. Any further builder settings one service needs follow as
/// `setting(argument)` — `force_path_style(true)` for Amazon S3, whose bucket must travel in
/// the path because the emulator serves one endpoint for every service.
///
/// [`EmbeddedEmulator`]: smithy_server_runtime::EmbeddedEmulator
macro_rules! callback_client {
    ($sdk:ident, $callback:expr, $endpoint:literal, $region:expr $(, $setting:ident($($argument:expr),*))* $(,)?) => {{
        let config = ::$sdk::Config::builder()
            .behavior_version(::$sdk::config::BehaviorVersion::latest())
            .http_client($callback.http_client())
            .credentials_provider($callback.aws_credentials_provider())
            .region(::$sdk::config::Region::new($region.to_string()))
            .endpoint_url(format!("https://{}.{}.amazonaws.com", $endpoint, $region))
            $(.$setting($($argument),*))*
            .retry_config(::$sdk::config::retry::RetryConfig::disabled())
            .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::$sdk::config::IdentityCache::no_cache())
            .stalled_stream_protection(::$sdk::config::StalledStreamProtectionConfig::disabled())
            .build();
        ::$sdk::Client::from_conf(config)
    }};
}

pub(crate) use callback_client;
