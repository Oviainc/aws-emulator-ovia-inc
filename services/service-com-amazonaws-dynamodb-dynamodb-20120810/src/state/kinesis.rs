// SPDX-License-Identifier: CC0-1.0

//! The refusal a command against a table's Kinesis streaming destination earns when it
//! asks for the configuration that destination already replicates under.
//!
//! Beside the other destination refusals in [`super::table_settings`]: this one is
//! about the COMMAND rather than about the record's lifecycle status, so it is stated
//! here and re-exported through [`super`] like the rest.

use super::*;

/// How the command that CHANGES a destination's configuration is refused when it asks
/// for the precision the destination ALREADY replicates under: the change is a no-op,
/// so the service states that nothing is left to do rather than reporting a change it
/// did not make.
///
/// The destination is named by the TABLE ID and the Kinesis stream ARN (`kdsArn`) — the
/// identity the stored record carries, read from this service's own state rather than
/// echoed from the request — and the precision is the one the request asked for.
pub fn kinesis_precision_unchanged(
    table_id: &str,
    stream_arn: &str,
    precision: &str,
) -> RequestError {
    RequestError::Validation(format!(
        "Invalid Request: Precision is already set to the desired value of {precision} for \
         tableId: {table_id}, kdsArn: {stream_arn}"
    ))
}
