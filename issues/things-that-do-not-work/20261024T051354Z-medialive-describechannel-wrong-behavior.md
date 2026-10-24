Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# Channel responses omit the service-assigned maintenance window and never report egressEndpoints

_Filed: 2026-10-24T05:13:54Z_
_Operation: DescribeChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every channel-shaped response this service produces is built by the single projection `ComAmazonawsMedialiveMedialiveState::channel_description` / `channel_summary` (src/state.rs:2070 and :2112), so the two members below are missing from DescribeChannel, CreateChannel, ListChannels, StartChannel, StopChannel, DeleteChannel, RestartChannelPipelines and BatchDelete alike.

Cause 1 of 2 — `maintenance` is reported only when the create request happened to carry it.
Expected: MediaLive assigns a PVRE maintenance window to EVERY channel. A caller that creates a channel without a `maintenance` block still reads back a `maintenance` object carrying the day and start time the service picked, and may then move it with UpdateChannel. The model's own wording says the same thing: `MaintenanceStatus$MaintenanceDay` is documented as "The currently selected maintenance day" and `MaintenanceStartTime` as "The currently selected maintenance start time" — a *currently selected* value presupposes the service always has one selected, which is why `MaintenanceCreateSettings` is optional on the request ("Choose one day of the week for maintenance") while the response type is a *status*.
Actual: `ops/create_channel.rs:264` does `channel.maintenance = input.maintenance.map(...)`, so a channel created without the member stores `None` and every later read reports the `maintenance` member as absent. There is no code path anywhere in the crate that assigns a window.

Cause 2 of 2 — `egressEndpoints` is always absent.
Expected: a channel reports the public IP its outputs originate from, one `ChannelEgressEndpoint` per pipeline (`SourceIp`: "Public IP of where a channel's output comes from"), so a STANDARD channel reports two entries and a SINGLE_PIPELINE channel one. This is the egress counterpart of the push-input ingest endpoint the emulator ALREADY synthesises from a pool (`allocate_input_ingest_addresses`, used by CreateInput/UpdateInput), so the mechanism to report it faithfully already exists in this crate.
Actual: `egress_endpoints: None` is hard-coded in both channel projections (src/state.rs:2081 and :2119) with the note "ABSENT, not invented". No channel ever reports an egress endpoint, in any state, for any class.

Effect: a client cannot discover or verify the maintenance window of a channel it just created (an IaC provider that treats `maintenance` as computed sees an empty value and reports perpetual drift, or re-issues UpdateChannel), and a client that needs the channel's source IP — to allow-list it on a downstream origin, which is the whole purpose of the member — gets nothing to allow-list.

## Evidence

Spec: `smithy_query(query="shape", shape="MaintenanceStatus")` — members documented as "The currently selected maintenance day" / "The currently selected maintenance start time"; `MaintenanceCreateSettings` is optional on CreateChannelRequest. `smithy_query(query="shape", shape="ChannelEgressEndpoint")` — `SourceIp`: "Public IP of where a channel's output comes from"; `Channel$EgressEndpoints` and `ChannelSummary$EgressEndpoints` are both modelled. Well-established service behaviour: MediaLive auto-assigns a two-hour PVRE maintenance window to each channel and the customer may only change its day/start time, and describe-channel output carries egressEndpoints for the channel's pipelines.
Handler/state: `services/service-com-amazonaws-medialive-medialive/src/ops/create_channel.rs:264-271` (maintenance stored only from the request, deadline/scheduled-date deliberately left unset); `services/service-com-amazonaws-medialive-medialive/src/state.rs:2081` and `:2119` (`egress_endpoints: None` in `channel_description` and `channel_summary`); no occurrence of an egress or maintenance-window assignment anywhere in the crate (grep for `egress_endpoints` yields only the two `None` literals and the output-macro move at :4156).
Test coverage: NONE asserts either behaviour. `tests/all/channel_lifecycle.rs:303-312` exercises only the case where the request DOES carry `maintenance`, and compares just the two members the request set (lines 340-343); no test creates a channel without `maintenance` and asserts what the member reports, and no test mentions `egressEndpoints` at all (grep over tests/).

## Steps to Reproduce

1. POST /prod/channels with a body that omits `maintenance` (e.g. {"name":"plain"}).
2. Observe the 201 response body: it carries no `maintenance` member — AWS answers with the window it assigned (maintenanceDay + maintenanceStartTime).
3. GET /prod/channels/<id>: still no `maintenance` member, so the assigned window is unobservable for the channel's whole lifetime.
4. Observe the same response body has no `egressEndpoints` member; GET /prod/channels also omits it from the ChannelSummary. AWS reports one entry per pipeline, each with a `sourceIp`.
5. Repeat for a STANDARD (two-pipeline) channel: still no egressEndpoints, where AWS reports two.
