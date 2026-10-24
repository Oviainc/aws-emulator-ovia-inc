Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# ConfigureLogs accepts any egressAccessLogs.logGroupName, including empty and names CloudWatch Logs cannot hold

_Filed: 2026-10-24T05:11:29Z_
_Operation: ConfigureLogs | Class: validation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ConfigureLogs` subscribes a packaging group's egress access logs to a CloudWatch Logs log group named by `EgressAccessLogs$LogGroupName` ("Customize the log group name."). AWS validates that string before persisting the subscription — the name has to be a log group CloudWatch Logs can actually hold (1-512 characters, limited to `.`, `-`, `_`, `/`, `#` and alphanumerics) — and refuses anything else with `UnprocessableEntityException` (HTTP 422, "The parameters sent in the request are not valid."), leaving the group's subscription as it was.

Expected: `PUT /packaging_groups/{Id}/configure_logs` with `{"egressAccessLogs": {"logGroupName": ""}}`, with a 600-character name, or with a name containing characters a log group name may not carry (spaces, `$`, `:`, `*`) is refused with UnprocessableEntityException / 422 naming `egressAccessLogs.logGroupName`, and the following `DescribePackagingGroup` reports the previous subscription.

Actual: the handler resolves the group (404 for an unknown `Id`) and then writes the request's block straight through — `stored.egress_access_logs = input.egress_access_logs;` — with no bound on the string at all. The call answers HTTP 200, the unusable name is echoed back, and it is reported by every later describe/list as a live log subscription. The same unvalidated member is also accepted on `CreatePackagingGroup` (whose validation gap is already filed separately).

## Evidence

Spec: `smithy_query(query="shape", shape="EgressAccessLogs")` — one member `LogGroupName` (`jsonName: logGroupName`) documented as "Customize the log group name."; `ConfigureLogs` models `UnprocessableEntityException` (errorKind client, httpError 422) as its invalid-parameter channel. The name constraints are CloudWatch Logs' own documented log-group-name rules (1-512 chars, `[\.\-_/#A-Za-z0-9]+`), which MediaPackage must honour because it creates/writes the group it is given.
Handler: `src/ops/configure_logs.rs` `handle()` line ~197 — `stored.egress_access_logs = input.egress_access_logs;` with no length or charset check anywhere in the body; `src/state.rs` stores the struct as supplied.
In-repo corroboration: the sibling MediaPackage (v1) crate bounds exactly this member before writing it — `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/configure_logs.rs` lines ~176-183 call `crate::state::bound_chars("egressAccessLogs.logGroupName", value, MAX_STRING_CHARS)` and refuse with that service's 422 envelope. The VOD crate has no equivalent.
Tests: `tests/all/configure_logs.rs::rows()` exercises only well-formed names (`/aws/MediaPackage/vod-custom`, `my-vod-egress-logs`, an absent member, a 200-character suffix) — no test asserts acceptance or rejection of an empty, over-long or illegal-character name, so this path is uncovered rather than pinned.

## Steps to Reproduce

1. POST /packaging_groups with {"id": "vod-group"}: observe HTTP 200.
2. PUT /packaging_groups/vod-group/configure_logs with {"egressAccessLogs": {"logGroupName": ""}}: observe HTTP 200 and an echoed empty name, where AWS refuses with UnprocessableEntityException (422).
3. Repeat with {"logGroupName": "<600 'g' characters>"} and {"logGroupName": "not a valid group name!$"}: observe HTTP 200 in both cases.
4. GET /packaging_groups/vod-group: observe the group reports the unusable log group name as its live egress access log subscription.
