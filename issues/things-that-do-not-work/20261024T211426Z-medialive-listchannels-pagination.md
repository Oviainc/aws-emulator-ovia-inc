Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# MaxResults is silently clamped to 50, so a page smaller than the caller asked for is returned with a nextToken

_Filed: 2026-10-24T21:14:26Z_
_Operation: ListChannels | Class: pagination_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

MediaLive's list operations accept a `MaxResults` up to 1000 and return up to that many items in one page. The emulator clamps every `MaxResults` to a private ceiling of 50 and also uses 50 as the page size when `MaxResults` is absent, so a caller that asks for 100 channels when the account holds 100 gets 50 items plus a `nextToken` where AWS returns all 100 and no token. A client that passes a large `MaxResults` precisely so it does not have to paginate silently sees half the account's resources.

Expected: a `MaxResults` within the service's accepted range is honoured verbatim (up to 1000). Actual: anything above 50 is truncated to 50 and a continuation token is handed out.

The clamp is shared by every listing of this service, so the same truncation applies to `ListMultiplexes`, `ListInputSecurityGroups`, `ListSignalMaps`, `ListSdiSources`, `ListNetworks`, `ListClusters`, `ListChannelPlacementGroups`, `ListInputDevices`, `ListInputDeviceTransfers`, `ListMultiplexPrograms`, `ListEventBridgeRuleTemplates`/`Groups`, `ListOfferings`, `ListClusterAlerts`, `ListMultiplexAlerts` and `DescribeSchedule`.

## Evidence

`src/state.rs:815`: `pub const MAX_LIST_PAGE_SIZE: usize = 50;`. `ops/list_channels.rs` lines 155-166 do `Some(max_results) => (max_results as usize).min(crate::state::MAX_LIST_PAGE_SIZE)` and `None => crate::state::MAX_LIST_PAGE_SIZE`; the identical block appears in every other listing handler (e.g. `ops/list_signal_maps.rs` 157-166, `ops/list_offerings.rs` 175-183, `ops/describe_schedule.rs` 164-172, `ops/list_cluster_alerts.rs` 168-176). The store ceiling is larger than the page cap (`MAX_CHANNELS = 100`, `MAX_INPUTS = 100`, state.rs:630/751), so the truncation is observable. `smithy_query(query="shape", shape="MaxResults")` shows the model declares no range, so nothing at the request boundary justifies the 50; MediaLive's published valid range for ListChannels MaxResults is 1-1000. No test asserts the honoured page size.

## Steps to Reproduce

1. CreateChannel 60 times
2. GET /prod/channels?maxResults=100
3. Observe 50 channels and a nextToken in the response (AWS returns all 60 with no nextToken)
4. GET /prod/channels with no maxResults and observe the page capped at 50 as well
