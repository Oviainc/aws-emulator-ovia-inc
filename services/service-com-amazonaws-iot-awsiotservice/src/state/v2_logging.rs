// SPDX-License-Identifier: CC0-1.0

//! The account's V2 LOGGING OPTIONS: the one record `SetV2LoggingOptions` writes and
//! `GetV2LoggingOptions` reports.
//!
//! Neither operation names a resource — `SetV2LoggingOptions` is "Sets the logging
//! options for the V2 logging service" and `GetV2LoggingOptions` "Gets the fine
//! grained logging options"
//! (`smithy_query(query="operation",operation="GetV2LoggingOptions,SetV2LoggingOptions")`)
//! — so the account holds ONE such record, not a map of them, and the store keeps it
//! as an `Option`: an account nobody has set logging options for holds NOTHING, which
//! is the state the reader refuses with the modelled `NotConfiguredException` ("The
//! resource is not configured", HTTP 404) rather than inventing options for.
//!
//! Every member of `GetV2LoggingOptionsResponse` has a `SetV2LoggingOptionsRequest`
//! member of the same name and shape, so the request IS the whole options document:
//! the record below holds those members and the reader hands them straight back,
//! which is what keeps the writer and the reader from drifting.
//!
//! The per-event-type overrides are keyed by `eventType` rather than kept as the list
//! the request sent, because that member identifies which event type a configuration
//! is for ("The type of event to log", `@required` on `LogEventConfiguration`): the
//! last configuration a request gives for an event type is the one the account holds,
//! and a `BTreeMap` also gives the reader a stable order to report them in.

use std::collections::BTreeMap;

use crate::ops::types::LogEventConfiguration;

/// How many per-event-type overrides one set of V2 logging options may carry.
///
/// `LogEventConfigurations` carries no modelled length and the list is one a CALLER
/// supplies, so this is the emulator's own ceiling on it — the same reason
/// [`super::MAX_CUSTOM_FIELDS`] exists — and `SetV2LoggingOptions` past it refuses
/// with a modelled error rather than recording options of unbounded size.
pub const MAX_LOG_EVENT_CONFIGURATIONS: usize = 100;

/// The message a reader of the account's V2 logging options refuses an account that
/// has never set them with.
///
/// It names the call that configures them and states that none are set, which is how
/// AWS IoT words this refusal: the text is fixed — nothing about the account, the
/// request or the region varies in it.
pub fn missing_v2_logging_options_message() -> String {
    "SetV2LoggingOptions was not previously called. No logging options have been set.".to_string()
}

/// The per-event-type part of one override: everything `LogEventConfiguration` carries
/// besides the `eventType` that keys it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoggedEventConfiguration {
    /// "The logging level for the specified event type. Determines the verbosity of
    /// log messages generated for this event type."
    pub log_level: Option<String>,
    /// "CloudWatch Log Group for event-based logging", which overrides the default log
    /// group for that event type.
    pub log_destination: Option<String>,
}

/// The account's V2 logging options, as this service persists them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct V2LoggingOptions {
    /// "The IAM role ARN IoT uses to write to your CloudWatch logs."
    pub role_arn: Option<String>,
    /// "The default log level" — one of the values `LogLevel` declares; the boundary
    /// validator refuses anything else before a handler sees it.
    pub default_log_level: Option<String>,
    /// "Disables all logs." Modelled with a default of `false`, so a request that
    /// omits it asks for logging NOT to be disabled, and the member is never absent.
    pub disable_all_logs: bool,
    /// "The list of event configurations that override account-level logging", keyed
    /// by the `eventType` each one configures.
    pub event_configurations: BTreeMap<String, LoggedEventConfiguration>,
}

impl V2LoggingOptions {
    /// The options a `SetV2LoggingOptions` request configures.
    ///
    /// A set replaces the account's options in full: every member of the response has
    /// a request member of the same name, so the request carries the whole document
    /// and a member it omits is one the configured options do not have.
    pub fn configured(
        role_arn: Option<String>,
        default_log_level: Option<String>,
        disable_all_logs: bool,
        event_configurations: Option<Vec<LogEventConfiguration>>,
    ) -> Self {
        Self {
            role_arn,
            default_log_level,
            disable_all_logs,
            event_configurations: event_configurations
                .unwrap_or_default()
                .into_iter()
                .map(|configuration| {
                    (
                        configuration.event_type,
                        LoggedEventConfiguration {
                            log_level: configuration.log_level,
                            log_destination: configuration.log_destination,
                        },
                    )
                })
                .collect(),
        }
    }

    /// The stored overrides as the modelled `LogEventConfigurations` list, in
    /// `eventType` order so two reads of one record report the same list.
    pub fn event_configurations(&self) -> Vec<LogEventConfiguration> {
        self.event_configurations
            .iter()
            .map(|(event_type, configuration)| LogEventConfiguration {
                event_type: event_type.clone(),
                log_level: configuration.log_level.clone(),
                log_destination: configuration.log_destination.clone(),
            })
            .collect()
    }
}
