// SPDX-License-Identifier: CC0-1.0

//! The size rule every router listing applies to the `Filters` of a request.
//!
//! ListRouterInputs, ListRouterOutputs and ListRouterNetworkInterfaces all narrow their
//! listing with a `Filters` list whose elements are unions carrying a list of accepted
//! values, and all three test every stored record against every filter and every value one
//! names. The ceilings that bound that work, and the refusal a request past them is answered
//! with, are therefore ONE rule — stated here once, so the three listings cannot drift apart
//! on it. The listings differ only in the resource each lists, which is the phrase its
//! refusal names ([`check_router_filters`]).

/// How many `Filters` entries one router listing request may carry.
///
/// Every router filter list (`RouterInputFilterList`, `RouterOutputFilterList`,
/// `RouterNetworkInterfaceFilterList`) is a plain list and its members are plain lists of
/// values, so the model bounds neither how many criteria a request states nor how many values
/// any one of them names. Every stored record is tested against every filter and every filter
/// value, so these two ceilings are what bound that work: the comparisons one listing performs
/// are at most `MAX_ROUTER_FILTERS` x [`MAX_ROUTER_FILTER_VALUES`] x the ceiling on what that
/// listing's store holds ([`super::MAX_ROUTER_INPUTS`], [`super::MAX_ROUTER_OUTPUTS`],
/// [`super::MAX_ROUTER_NETWORK_INTERFACES`]), whatever a request asks for. A filter beyond
/// them is a request a listing refuses rather than one it spends the store's lock on.
///
/// Both ceilings, and the wording of the refusal they produce, are this emulator's own: AWS
/// publishes no limit on either count for these operations, so a request past them is refused
/// with the client-input error the model does declare (`BadRequestException`, "thrown if the
/// request contains a semantic error") rather than with an error the model does not. They are
/// set far above any listing a client writes by hand, so a request a MediaConnect client would
/// send is answered, not refused.
pub const MAX_ROUTER_FILTERS: usize = 10;

/// How many values one `Filters` entry may name. See [`MAX_ROUTER_FILTERS`].
pub const MAX_ROUTER_FILTER_VALUES: usize = 100;

/// Judge one router listing request's `Filters` against both ceilings, before the store's
/// guard is taken: `Ok(())` when the request is one to answer, otherwise the detail of the
/// refusal it earns.
///
/// The caller carries the detail into its own modelled `BadRequestException`
/// ([`super::validation_failed_message`]), because the error channel is per operation while
/// this rule is not.
///
/// `listing` is the resource the operation lists, as that operation's own documentation names
/// it ("Retrieves a list of router inputs" → `"router inputs"`): it is the only part of the
/// refusal that is per operation. `value_counts` carries one entry per `Filters` element —
/// how many values that element names, which is all a filter's union member holds — so its
/// own length is how many criteria the request states.
pub fn check_router_filters(listing: &str, value_counts: &[usize]) -> Result<(), String> {
    let filter_count = value_counts.len();
    if filter_count > MAX_ROUTER_FILTERS {
        return Err(format!(
            "A list {listing} request may apply at most {MAX_ROUTER_FILTERS} Filters entries; \
             this one has {filter_count}."
        ));
    }
    for values in value_counts.iter().copied() {
        if values > MAX_ROUTER_FILTER_VALUES {
            return Err(format!(
                "A Filters entry may name at most {MAX_ROUTER_FILTER_VALUES} values; this one \
                 names {values}."
            ));
        }
    }
    Ok(())
}
