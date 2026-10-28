Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f2ead5306d42c2c14ef865f57d355d9f40696e5a

# The MaxResults page-size rule is open-coded in 23 paginated handlers instead of once

_Filed: 2026-10-27T18:24:13Z_
_Operation: ListChannels | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f2ead5306d42c2c14ef865f57d355d9f40696e5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this crate re-implements the identical page-size rule inline: refuse `maxResults < 1` with the operation's `BadRequestException` carrying the message "maxResults must be at least 1, got {max_results}", otherwise clamp to `crate::state::MAX_LIST_PAGE_SIZE`, and use that ceiling when the member is absent. The block appears verbatim in 23 handlers, so the rule, the ceiling application and the refusal sentence can drift independently in any one of them — the same class of duplication the already-fixed `updateeventbridgeruletemplate`/`updatecloudwatchalarmtemplate` maintainability issues removed for the template validation rules, and the shape the crate already prefers elsewhere (`crate::state::bounded_length`, `crate::state::at_least_one`, `crate::state::specification_filter_refusal` are each one shared rule that the handler only maps onto its own modelled error).

The fix is a single `crate::state` helper returning the page size or the refusal message (the same shape `bounded_length`/`at_least_one` already have), with each handler mapping the message onto its own modelled `BadRequestException`. No response and no existing test changes.

## Evidence

The shared ceiling — `services/service-com-amazonaws-medialive-medialive/src/state.rs:1131`: `pub const MAX_LIST_PAGE_SIZE: usize = 1000;`

Copy 1 — `src/ops/list_channels.rs:155-166`:
```
let page_size = match input.max_results {
    Some(max_results) if max_results < 1 => {
        return Err(ListChannelsErrorModeled::BadRequestException(BadRequestException {
            message: Some(format!(
                "maxResults must be at least 1, got {max_results}"
            )),
        })
        .into());
    }
    Some(max_results) => (max_results as usize).min(crate::state::MAX_LIST_PAGE_SIZE),
    None => crate::state::MAX_LIST_PAGE_SIZE,
};
```
Copy 2 — `src/ops/list_offerings.rs:175-183`:
```
let page_size = match input.max_results {
    Some(max_results) if max_results < 1 => {
        return Err(bad_request(format!(
            "maxResults must be at least 1, got {max_results}"
        )));
    }
    Some(max_results) => (max_results as usize).min(crate::state::MAX_LIST_PAGE_SIZE),
    None => crate::state::MAX_LIST_PAGE_SIZE,
};
```
Copy 3 — `src/ops/describe_schedule.rs:164-172` (identical body). The remaining 20 are `list_alerts.rs:188`, `list_channel_placement_groups.rs:182`, `list_cloud_watch_alarm_template_groups.rs:157`, `list_cloud_watch_alarm_templates.rs:164`, `list_cluster_alerts.rs:168`, `list_clusters.rs:170`, `list_event_bridge_rule_template_groups.rs:154`, `list_event_bridge_rule_templates.rs:162`, `list_input_device_transfers.rs:183`, `list_input_devices.rs:150`, `list_input_security_groups.rs:150`, `list_inputs.rs:150`, `list_multiplex_alerts.rs:165`, `list_multiplex_programs.rs:164`, `list_multiplexes.rs:158`, `list_networks.rs:151`, `list_nodes.rs:161`, `list_reservations.rs:177`, `list_sdi_sources.rs:151`, `list_signal_maps.rs:157`.

The crate's own single-rule precedent — `src/ops/purchase_offering.rs:304-318` wraps `crate::state::bounded_length` and `crate::state::at_least_one` so the rule lives once and the handler only maps the message onto its modelled error.

Test coverage that must stay green unchanged: `tests/all/list_page_size.rs` — its header states the rule "a page carries as many items as the request's `MaxResults` asks for while items are left to report — up to the upper bound of the `MaxResults` range the service publishes, which is also the size a request naming no `MaxResults` gets", and "Suite 2 varies the OPERATION, because the rule is the service's and not one listing's" (lines 3-16), importing `MAX_LIST_PAGE_SIZE` at line 23.

## Steps to Reproduce

1. grep -n "maxResults must be at least 1" services/service-com-amazonaws-medialive-medialive/src/ops — 23 handlers carry the identical block
2. Read src/ops/list_channels.rs:155-166 and src/ops/list_offerings.rs:175-183 side by side: same guard, same message, same clamp, same absent-member default
3. Compare with src/ops/purchase_offering.rs:304-318, where the crate already keeps a shared rule in crate::state and lets the handler map only the message
4. cargo test -p service-com-amazonaws-medialive-medialive list_page_size — the suite pins the behaviour the refactor must leave unchanged
