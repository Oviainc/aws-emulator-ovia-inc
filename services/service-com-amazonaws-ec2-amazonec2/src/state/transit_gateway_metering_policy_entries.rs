// SPDX-License-Identifier: CC0-1.0

//! The entries of a transit gateway metering policy: what one is stored as, the state a
//! created entry is in, what a create is refused with, the store's bounds and its one writer.
//!
//! The policies themselves — their id form, their own states and the refusals for an id that
//! names none — live in the "Transit gateway metering policies" section of [`crate::state`],
//! beside the other stores; this module holds only what an ENTRY of one adds to them. The
//! entries are kept in [`ComAmazonawsEc2Amazonec2State::transit_gateway_metering_policy_entries`],
//! keyed by policy and then by rule number, as the modelled
//! [`TransitGatewayMeteringPolicyEntry`] shape itself, so every member a writer sets
//! round-trips through `GetTransitGatewayMeteringPolicyEntries` without a field-by-field copy
//! that can silently drop one.
//!
//! Re-exported from [`crate::state`], so a handler reaches these as `crate::state::<item>`
//! like every other primitive there.

use std::time::SystemTime;

use super::{
    ComAmazonawsEc2Amazonec2State, MAX_PORT, MAX_STORED_RECORDS_PER_STORE,
    STORED_RECORD_OVERHEAD_BYTES, TRANSITION_DELAY, system_time_to_date_time,
};
use crate::ops::types::TransitGatewayMeteringPolicyEntry;
use crate::scaffolded::Scaffolded;

/// An entry that is in force: the `available` member of the modelled
/// `TransitGatewayMeteringPolicyEntryState`
/// ([`crate::ops::types::TRANSIT_GATEWAY_METERING_POLICY_ENTRY_STATE_VALUES`], whose only
/// members are `available` and `deleted`).
///
/// That catalog names no transient state, so a created entry is reported in this, its
/// terminal, state straight away and the clock owes it no move — unlike the policy it belongs
/// to, whose own catalog does carry a `pending` member.
pub const TRANSIT_GATEWAY_METERING_POLICY_ENTRY_STATE_AVAILABLE: &str = "available";

/// The code for an entry the policy already holds at that rule number.
///
/// Named in the exception-suffixed spelling this resource family's recorded codes use
/// ([`super::INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_MALFORMED`]), over the sense
/// [`super::NETWORK_ACL_ENTRY_ALREADY_EXISTS`] carries for EC2's other numbered entries: a
/// rule number identifies one entry of one policy, and no operation modifies an entry in
/// place, so a create never overwrites one.
pub const TRANSIT_GATEWAY_METERING_POLICY_ENTRY_ALREADY_EXISTS: &str =
    "TransitGatewayMeteringPolicyEntryAlreadyExistsException";

/// The smallest rule number a metering policy entry may take.
///
/// "The rule number for the metering policy entry. Rules are processed in order from lowest
/// to highest number" (`CreateTransitGatewayMeteringPolicyEntryRequest$PolicyRuleNumber`), so
/// the number is an ordinal position in that processing order, and EC2's numbered entries
/// count from one ("Positive integer from 1 to 32766",
/// `CreateNetworkAclEntryRequest$RuleNumber`, the same family's documented bound). No upper
/// bound is published for THIS resource, so none is enforced: the model's `Integer` is the
/// ceiling.
pub const MIN_TRANSIT_GATEWAY_METERING_POLICY_RULE_NUMBER: i32 = 1;

/// The separator between the two ends of a port range, as the port-range members of this
/// service spell one (`PortRange$From` and `$To` are the same pair of ports written out as
/// members).
const PORT_RANGE_SEPARATOR: char = '-';

/// Whether `value` names a range of ports: one port, or a first and a last port in order,
/// each inside the ports a port number can hold ([`MAX_PORT`]).
///
/// "The source port range for traffic matching" and its destination twin
/// (`TransitGatewayMeteringPolicyRule$SourcePortRange`, `$DestinationPortRange`) are plain
/// strings the model puts no pattern on, so the one thing that can be asked of such a value
/// is that it names ports at all — the same question
/// [`super::validate_ip_permission_port_range`] asks of the pair of port MEMBERS a security
/// group rule carries.
pub fn metering_policy_port_range_is_well_formed(value: &str) -> bool {
    let port_is_in_range = |port: &str| {
        port.parse::<i32>()
            .is_ok_and(|port| (0..=MAX_PORT).contains(&port))
    };
    match value.split_once(PORT_RANGE_SEPARATOR) {
        None => port_is_in_range(value),
        Some((first, last)) => match (first.parse::<i32>(), last.parse::<i32>()) {
            (Ok(first), Ok(last)) => {
                (0..=MAX_PORT).contains(&first) && (0..=MAX_PORT).contains(&last) && first <= last
            }
            _ => false,
        },
    }
}

/// Refuse an entry the policy already holds at that rule number: HTTP 400
/// [`TRANSIT_GATEWAY_METERING_POLICY_ENTRY_ALREADY_EXISTS`], naming the rule number the
/// request sent and the policy it named.
pub fn transit_gateway_metering_policy_entry_already_exists<E>(
    transit_gateway_metering_policy_id: &str,
    rule_number: i32,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        TRANSIT_GATEWAY_METERING_POLICY_ENTRY_ALREADY_EXISTS,
        400,
        format!(
            "The transit gateway metering policy {transit_gateway_metering_policy_id} already \
             holds an entry at rule number {rule_number}."
        ),
    )
}

/// The caller-chosen bytes ONE stored metering policy entry holds: the text of its rule's
/// matching criteria, of the account the traffic is attributed to, and of the rule number and
/// state as they are reported.
///
/// The ONE definition of that charge, the way
/// [`super::transit_gateway_metering_policy_payload_bytes`] is a policy's, so
/// [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway_metering_policy_entry`] charges
/// exactly this and no path can store an entry uncharged.
pub fn transit_gateway_metering_policy_entry_payload_bytes(
    entry: &TransitGatewayMeteringPolicyEntry,
) -> usize {
    let text = |value: Option<&str>| value.map_or(0, str::len);
    let rule = entry.metering_policy_rule.as_ref().map_or(0, |rule| {
        text(rule.source_transit_gateway_attachment_id.as_deref())
            .saturating_add(text(rule.source_transit_gateway_attachment_type.as_deref()))
            .saturating_add(text(rule.source_cidr_block.as_deref()))
            .saturating_add(text(rule.source_port_range.as_deref()))
            .saturating_add(text(rule.destination_transit_gateway_attachment_id.as_deref()))
            .saturating_add(text(
                rule.destination_transit_gateway_attachment_type.as_deref(),
            ))
            .saturating_add(text(rule.destination_cidr_block.as_deref()))
            .saturating_add(text(rule.destination_port_range.as_deref()))
            .saturating_add(text(rule.protocol.as_deref()))
    });
    text(entry.policy_rule_number.as_deref())
        .saturating_add(text(entry.metered_account.as_deref()))
        .saturating_add(text(entry.state.as_deref()))
        .saturating_add(rule)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether the metering policy `transit_gateway_metering_policy_id` already holds an entry
    /// at `rule_number`.
    ///
    /// One rule number names one entry of one policy — a delete resolves an entry by its
    /// policy and that number alone — so this is what a create asks before it writes.
    pub fn transit_gateway_metering_policy_holds_entry(
        &self,
        transit_gateway_metering_policy_id: &str,
        rule_number: i32,
    ) -> bool {
        self.transit_gateway_metering_policy_entries
            .get(transit_gateway_metering_policy_id)
            .is_some_and(|entries| entries.contains_key(&rule_number))
    }

    /// How many entries the metering policy `transit_gateway_metering_policy_id` holds.
    pub fn transit_gateway_metering_policy_entry_count(
        &self,
        transit_gateway_metering_policy_id: &str,
    ) -> usize {
        self.transit_gateway_metering_policy_entries
            .get(transit_gateway_metering_policy_id)
            .map_or(0, std::collections::BTreeMap::len)
    }

    /// Whether the metering policy `transit_gateway_metering_policy_id` can hold another
    /// entry.
    ///
    /// The same contract as [`Self::has_transit_gateway_metering_policy_capacity`]: a create
    /// asks first and refuses with [`super::resource_limit_exceeded`], and the writer below
    /// asks the same predicate, so the two cannot drift apart. Nothing AWS publishes bounds
    /// the entries of one metering policy, so the bound is this emulator's own
    /// [`MAX_STORED_RECORDS_PER_STORE`] — the ceiling every other store of this crate holds
    /// its records to, because each entry is a record in this process's memory.
    pub fn has_transit_gateway_metering_policy_entry_capacity(
        &self,
        transit_gateway_metering_policy_id: &str,
    ) -> bool {
        self.transit_gateway_metering_policy_entry_count(transit_gateway_metering_policy_id)
            < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store `view` as the entry of the metering policy `transit_gateway_metering_policy_id`
    /// at `rule_number`, and answer it as it was stored — or `None`, with NOTHING written,
    /// for a policy this store does not hold, one that already holds an entry at that rule
    /// number, or one that holds as many entries as it can.
    ///
    /// Everything the request does not decide is decided HERE rather than by the handler, so
    /// a later writer of the kind cannot record a fresh entry differently: the rule number as
    /// it is REPORTED (`TransitGatewayMeteringPolicyEntry$PolicyRuleNumber` is a string, where
    /// the create's own member is an integer), the terminal
    /// [`TRANSIT_GATEWAY_METERING_POLICY_ENTRY_STATE_AVAILABLE`] its catalog admits, "the date
    /// and time when the metering policy entry was last updated" (`$UpdatedAt`) read off the
    /// wall clock, and "the date and time when the metering policy entry update becomes
    /// effective" (`$UpdateEffectiveAt`) one [`TRANSITION_DELAY`] later — the same span, read
    /// off the same clock, that the policy's own `UpdateEffectiveAt` is set from, so an entry
    /// and its policy cannot disagree about when a change starts metering.
    pub fn insert_transit_gateway_metering_policy_entry(
        &mut self,
        transit_gateway_metering_policy_id: &str,
        rule_number: i32,
        mut view: TransitGatewayMeteringPolicyEntry,
    ) -> Option<TransitGatewayMeteringPolicyEntry> {
        if !self
            .transit_gateway_metering_policies
            .contains_key(transit_gateway_metering_policy_id)
            || self.transit_gateway_metering_policy_holds_entry(
                transit_gateway_metering_policy_id,
                rule_number,
            )
            || !self.has_transit_gateway_metering_policy_entry_capacity(
                transit_gateway_metering_policy_id,
            )
        {
            return None;
        }
        let now = SystemTime::now();
        view.policy_rule_number = Some(rule_number.to_string());
        view.state = Some(TRANSIT_GATEWAY_METERING_POLICY_ENTRY_STATE_AVAILABLE.to_string());
        view.updated_at = Some(system_time_to_date_time(now));
        view.update_effective_at = Some(system_time_to_date_time(now + TRANSITION_DELAY));
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                + transit_gateway_metering_policy_entry_payload_bytes(&view),
        );
        self.transit_gateway_metering_policy_entries
            .entry(transit_gateway_metering_policy_id.to_string())
            .or_default()
            .insert(rule_number, view);
        self.transit_gateway_metering_policy_entries
            .get(transit_gateway_metering_policy_id)
            .and_then(|entries| entries.get(&rule_number))
            .cloned()
    }
}
