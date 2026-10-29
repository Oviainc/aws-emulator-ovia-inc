// SPDX-License-Identifier: CC0-1.0

//! The alert vocabulary this service's alert surfaces share.
//!
//! MediaLive reports alerts on three resources — a channel (`ListAlerts`), a
//! cluster (`ListClusterAlerts`) and a multiplex (`ListMultiplexAlerts`) — and
//! the model gives all three the SAME two-state enum and the SAME `StateFilter`
//! documentation: `ChannelAlertState`, `ClusterAlertState` and
//! `MultiplexAlertState` each declare SET | CLEARED ("SET - The alert is
//! actively happening. CLEARED - The alert is no longer happening"), and each
//! request's `StateFilter` documents "Specifies the set of alerts to return
//! based on their state. SET - Return only alerts with SET state. CLEARED -
//! Return only alerts with CLEARED state. ALL - Return all alerts."
//! (`smithy_query(query="shape", shape="ClusterAlertState,MultiplexAlertState")`
//! and the member docs of each list operation).
//!
//! So the state names, the documented filter set, the membership rule that
//! refuses a value outside it, the state-from-cleared-instant derivation and the
//! filter predicate live here ONCE, and each resource's own slice
//! ([`super::clusters`], [`super::multiplexes`]) delegates to them: two alert
//! surfaces of one service cannot then disagree about what `SET` means, which
//! filter values are documented, how an undocumented one is reported, or which
//! alerts a filter admits.

use std::time::SystemTime;

/// The two states an alert is in, spelled exactly as every one of this service's
/// alert-state enums declares them.
pub mod alert_state {
    pub const SET: &str = "SET";
    pub const CLEARED: &str = "CLEARED";
}

/// The `StateFilter` value that asks for every alert whatever its state ("ALL -
/// Return all alerts"), which is NOT one of the two states an alert can be in.
pub const ALERT_STATE_FILTER_ALL: &str = "ALL";

/// Every `StateFilter` value an alert listing documents, in the order its member
/// documents them. The member targets bare `__string`, which carries no enum
/// trait, so the request boundary cannot refuse any other value and this is the
/// documented value set each listing refuses against.
pub const ALERT_STATE_FILTERS: [&str; 3] = [
    alert_state::SET,
    alert_state::CLEARED,
    ALERT_STATE_FILTER_ALL,
];

/// The `StateFilter` one alert listing was asked for, out of the member the
/// request carried — or `Err` carrying the value that names no documented set of
/// alerts at all.
///
/// ONE copy of this membership rule, for the reason this module exists: all
/// three alert listings (`ListAlerts`, `ListClusterAlerts`,
/// `ListMultiplexAlerts`) narrow their log by the same
/// [`ALERT_STATE_FILTERS`], so none of them may drift into its own reading of
/// which values are documented. An ABSENT member narrows nothing and so asks for
/// every alert, which is why it is served rather than refused.
///
/// The caller turns the refused value into the message
/// [`undocumented_alert_state_filter_message`] states and answers it as the
/// modelled client error its own operation declares, exactly as the operations
/// wrapping [`super::list_page_size`] answer that rule's refusal.
pub fn alert_state_filter(filter: Option<&str>) -> Result<Option<&str>, &str> {
    match filter {
        Some(value) if !ALERT_STATE_FILTERS.contains(&value) => Err(value),
        documented => Ok(documented),
    }
}

/// What to tell a caller whose `StateFilter` is outside [`ALERT_STATE_FILTERS`]:
/// the member, the values it accepts — read off that one set, so the sentence
/// cannot name a set the listings do not refuse against — and `offending`, the
/// value the request carried as the listing answering it writes that value into
/// its message.
///
/// ONE copy of this sentence for all three alert listings, beside the one
/// membership rule [`alert_state_filter`] states.
pub fn undocumented_alert_state_filter_message(offending: &str) -> String {
    format!(
        "stateFilter must be one of {}, got {offending}",
        ALERT_STATE_FILTERS.join(", ")
    )
}

/// "The state of the alert", out of the instant it was cleared at: `SET` while
/// it is actively happening, `CLEARED` once it is not. Derived, never stored, so
/// a reported state cannot disagree with the reported `ClearedTimestamp`.
pub fn alert_state_of(cleared_at: Option<SystemTime>) -> &'static str {
    if cleared_at.is_none() {
        alert_state::SET
    } else {
        alert_state::CLEARED
    }
}

/// Whether a listing's `StateFilter` admits an alert cleared at `cleared_at`:
/// `SET` and `CLEARED` admit the alerts in that state, `ALL` admits every one,
/// and a request that named no filter is not filtered at all.
///
/// A value outside [`ALERT_STATE_FILTERS`] names no set of alerts and is refused
/// by [`alert_state_filter`] before it reaches here, so it would admit nothing.
pub fn alert_admitted_by(cleared_at: Option<SystemTime>, filter: Option<&str>) -> bool {
    match filter {
        None | Some(ALERT_STATE_FILTER_ALL) => true,
        Some(state) => alert_state_of(cleared_at) == state,
    }
}
