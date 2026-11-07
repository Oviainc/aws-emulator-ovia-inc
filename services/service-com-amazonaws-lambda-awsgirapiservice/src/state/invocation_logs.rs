// SPDX-License-Identifier: CC0-1.0

//! WHERE a function's invocations are logged: the one reading of `LoggingConfig$LogGroup`
//! and of the default group a function that names none logs to.
//!
//! `LoggingConfig$LogGroup` is "the name of the Amazon CloudWatch log group the function
//! sends logs to", and "by default, Lambda functions send logs to a default log group named
//! `/aws/lambda/<function name>`" — so a function's log destination is that member when it
//! carries one and [`super::default_log_group`] otherwise.
//!
//! Two places answer with it and must not disagree: what a READ of the function reports as
//! its `LoggingConfig` ([`super::FunctionRecord`]), and where a RUN's log is DELIVERED
//! ([`super::execution::log_delivery::log_target`], which also reads what the execution role
//! may do to that group). Both take it from here, so the group a client is told about is the
//! group a client finds the records in.

use super::LoggingConfig;

/// The CloudWatch log group the invocations of the function `name` are logged to: the one its
/// own `logging` names, and [`super::default_log_group`] when it names none.
///
/// An empty `LogGroup` names no group, so it is the default one too.
#[must_use]
pub fn configured_log_group(logging: Option<&LoggingConfig>, name: &str) -> String {
    logging
        .and_then(|logging| logging.log_group.clone())
        .filter(|group| !group.is_empty())
        .unwrap_or_else(|| super::default_log_group(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `LoggingConfig` carrying `log_group`.
    fn logging(log_group: Option<&str>) -> LoggingConfig {
        LoggingConfig {
            application_log_level: None,
            system_log_level: None,
            log_group: log_group.map(str::to_string),
            log_format: None,
        }
    }

    /// Every form the member takes, and the group each names.
    #[test]
    fn the_group_is_the_configured_one_and_otherwise_the_functions_default() {
        // (the case, the function's `LoggingConfig`, the group its invocations are logged to)
        let rows: Vec<(&str, Option<LoggingConfig>, &str)> = vec![
            (
                "a function with no LoggingConfig at all",
                None,
                "/aws/lambda/orders",
            ),
            (
                "a LoggingConfig naming no group",
                Some(logging(None)),
                "/aws/lambda/orders",
            ),
            (
                "a LoggingConfig whose group is the empty string, which names none",
                Some(logging(Some(""))),
                "/aws/lambda/orders",
            ),
            (
                "a LoggingConfig naming another group",
                Some(logging(Some("/custom/orders"))),
                "/custom/orders",
            ),
            (
                "a LoggingConfig naming a group outside the /aws/lambda prefix entirely",
                Some(logging(Some("orders-log-group"))),
                "orders-log-group",
            ),
        ];
        for (case, configured, expected) in rows {
            let actual = configured_log_group(configured.as_ref(), "orders");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the log group {expected:?}, got {actual:?}"
            );
        }
    }
}
