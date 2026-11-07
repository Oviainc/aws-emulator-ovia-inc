// SPDX-License-Identifier: CC0-1.0

//! The ON-DEMAND capacity ceiling a table carries: the maximum request units it serves.
//!
//! "Sets the maximum number of read and write units for the specified on-demand table. If
//! you use this parameter, you must specify `MaxReadRequestUnits`,
//! `MaxWriteRequestUnits`, or both" (the `OnDemandThroughput` doc), and each member is
//! set and cleared through the SAME member: "To specify a maximum `OnDemandThroughput` on
//! your table, set the value of `MaxReadRequestUnits` as greater than or equal to 1. To
//! remove the maximum `OnDemandThroughput` that is currently set on your table, set the
//! value of `MaxReadRequestUnits` to -1" (`OnDemandThroughput$MaxReadRequestUnits`, which
//! `MaxWriteRequestUnits` repeats).
//!
//! So the pair is TWO INDEPENDENT settings, each of which a request may set, clear or
//! leave alone — which is what this module holds, and what [`OnDemandCeilings::apply`]
//! moves. A ceiling that is not set has no maximum, which the description reports as the
//! same -1 that clears it.

use crate::ops::types::OnDemandThroughput;

use super::RequestError;

/// The value that CLEARS a ceiling, and the value a description reports for a ceiling
/// that is not set: "To remove the maximum `OnDemandThroughput` that is currently set on
/// your table, set the value of `MaxReadRequestUnits` to -1"
/// (`OnDemandThroughput$MaxReadRequestUnits`). The same number means "no maximum" in
/// both directions, so a client can read a description back into a request unchanged.
pub const NO_ON_DEMAND_CEILING: i64 = -1;

/// The smallest ceiling that can be SET: "set the value of `MaxReadRequestUnits` as
/// greater than or equal to 1" (that member's doc).
pub const MIN_ON_DEMAND_CEILING: i64 = 1;

/// The on-demand ceilings in force for one table: the maximum read and write request
/// units it serves, each absent when the table has no maximum for that dimension.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OnDemandCeilings {
    /// "Maximum number of read request units for the specified table"
    /// (`OnDemandThroughput$MaxReadRequestUnits`).
    pub max_read_request_units: Option<i64>,
    /// "Maximum number of write request units for the specified table"
    /// (`OnDemandThroughput$MaxWriteRequestUnits`).
    pub max_write_request_units: Option<i64>,
}

impl OnDemandCeilings {
    /// The pair a request's `OnDemandThroughput` asks for, applied over the pair in force:
    /// a member the request names at `-1` clears that ceiling, a member it names at 1 or
    /// more sets it, and a member it leaves out leaves that ceiling exactly as it was.
    pub fn apply(self, requested: Option<&OnDemandThroughput>) -> Self {
        let Some(requested) = requested else {
            return self;
        };
        Self {
            max_read_request_units: moved(
                self.max_read_request_units,
                requested.max_read_request_units,
            ),
            max_write_request_units: moved(
                self.max_write_request_units,
                requested.max_write_request_units,
            ),
        }
    }

    /// The pair as `TableDescription$OnDemandThroughput` reports it: the ceiling in force
    /// for each dimension, and [`NO_ON_DEMAND_CEILING`] for a dimension with no maximum —
    /// the value that same member takes to clear it.
    pub fn describe(self) -> OnDemandThroughput {
        OnDemandThroughput {
            max_read_request_units: Some(
                self.max_read_request_units.unwrap_or(NO_ON_DEMAND_CEILING),
            ),
            max_write_request_units: Some(
                self.max_write_request_units.unwrap_or(NO_ON_DEMAND_CEILING),
            ),
        }
    }
}

/// One ceiling after a request that may or may not have named it.
fn moved(in_force: Option<i64>, requested: Option<i64>) -> Option<i64> {
    match requested {
        None => in_force,
        Some(NO_ON_DEMAND_CEILING) => None,
        Some(units) => Some(units),
    }
}

/// Hold every `OnDemandThroughput` member a request names to the values the model's doc
/// gives it: 1 or more SETS the ceiling, and -1 REMOVES it. Anything else — zero, or a
/// negative number that is not the removal value — asks for a maximum the capacity model
/// has no meaning for, so the request is refused whatever the table's state.
pub fn check_on_demand_ceiling_values(
    requested: Option<&OnDemandThroughput>,
) -> Result<(), RequestError> {
    let Some(requested) = requested else {
        return Ok(());
    };
    for (member, value) in [
        ("MaxReadRequestUnits", requested.max_read_request_units),
        ("MaxWriteRequestUnits", requested.max_write_request_units),
    ] {
        if let Some(value) = value {
            if value != NO_ON_DEMAND_CEILING && value < MIN_ON_DEMAND_CEILING {
                return Err(RequestError::Validation(format!(
                    "One or more parameter values were invalid: {member} for OnDemandThroughput \
                     must be at least {MIN_ON_DEMAND_CEILING} to set a maximum, or \
                     {NO_ON_DEMAND_CEILING} to remove it, got {value}"
                )));
            }
        }
    }
    Ok(())
}
