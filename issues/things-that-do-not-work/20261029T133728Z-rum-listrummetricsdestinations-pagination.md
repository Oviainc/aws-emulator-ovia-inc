Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: e9dbd7f28045b442d217f817f50d4dbc748f9e32

# A nextToken carries the app monitor NAME but no Region, so a token issued at one regional endpoint is honoured at another's same-named monitor and silently skips destinations

_Filed: 2026-10-29T13:37:28Z_
_Operation: ListRumMetricsDestinations | Class: pagination_

> _Note: this issue was identified against the code base at commit `e9dbd7f28045b442d217f817f50d4dbc748f9e32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRumMetricsDestinations` scopes its continuation token to the app monitor NAME alone. But this crate makes the REGION part of a monitor's identity: a name is only unique within one region, and two regions can each hold an independent monitor called `foo`. So a token minted by the listing of `foo` in us-east-1 decodes successfully when it is presented to the eu-west-1 endpoint, where it names a DIFFERENT monitor's destination map; the range query then resumes strictly after a key that region's listing never returned, and every destination ordered before that key is dropped from the listing with a 200 and no token-related refusal.

The handler's own doc states the rule the code fails to implement ("a token is only honoured by the listing that issued it: destinations are per-monitor, and two monitors' maps have keys in common") — two REGIONS' monitors of one name are exactly two different monitors whose maps have keys in common, and the name comparison cannot tell them apart. The same key composition is used by the crate's sibling listings (`ListAppMonitors`' token is the monitor name alone, `BatchGetRumMetricDefinitions`' is name+destination+arn, `GetAppMonitorData`' is name+count), none of which carries the region either.

Reachable because the crate's own region suite seeds the SAME names in every region against ONE emulator store, which is precisely the state in which the cross-region resume lands on a real, different map.

## Evidence

The token's scope check compares the monitor NAME only — `services/service-com-amazonaws-rum-rum/src/ops/list_rum_metrics_destinations.rs:295-302`:
```
fn decode_next_token(
    app_monitor_name: &str,
    token: &str,
) -> Option<crate::state::MetricDestinationKey> {
    let key = crate::state::decode_pagination_token(TOKEN_MARKER, token, 3)?;
    let [issued_for, destination, destination_arn] = <[String; 3]>::try_from(key).ok()?;
    (issued_for == app_monitor_name).then_some((destination, destination_arn))
}
```
and its doc claims the scoping that comparison does not achieve — `list_rum_metrics_destinations.rs:277-281`:
```
/// The app monitor the listing is scoped to is part of the key, so a token is only
/// honoured by the listing that issued it: destinations are per-monitor, and two
/// monitors' maps have keys in common.
```
The other side of the inconsistency, in this crate's state module — `services/service-com-amazonaws-rum-rum/src/state.rs:842-848` (`AppMonitorRecord::region`):
```
/// RUM is a regional service: a monitor exists in exactly one region, its name only
/// has to be unique within that region, and no request signed for another region can
/// see it. So the region is part of the resource's identity and is the first half of
/// the key the record is stored under
```
The handler itself resolves the monitor per-region, so the map the token is applied to is the signed region's — `list_rum_metrics_destinations.rs:194` `let region = crate::state::request_region(ctx.parts());` and `:226` `let Some(monitor) = store.app_monitor(&region, &input.app_monitor_name) else {`, then `:241-243` `monitor.metric_destinations.range((start, ::std::ops::Bound::Unbounded));` with `start = Bound::Excluded(after)` (`:237-240`).

Test coverage: `services/service-com-amazonaws-rum-rum/tests/all/metrics_destinations.rs:662-691` enumerates the tokens that must be refused — "Anything else is refused rather than read as some other resume point" / `"case {case:?}: expected HTTP 400 for a token this service never issued"` — and `:693-696` adds the cross-MONITOR row ("a token issued for one monitor's listing is not a resume point for another's"), but no row presents a token at another REGION's endpoint. `services/service-com-amazonaws-rum-rum/tests/all/region_scoped_app_monitors.rs:10-14` pins the rule being broken: "a RUM app monitor belongs to the region the request that created it was signed for, so its name only has to be unique within that region, and [every operation] resolves it among the monitors of the region THAT request was signed for and reaches no other region's" — and `:410-419` seeds "The SAME names in every region".

## Steps to Reproduce

1. Mount the emulator once (one store) and sign requests for two regions, as `tests/all/region_scoped_app_monitors.rs` does.
2. `CreateAppMonitor` with `Name=foo` signed for us-east-1, and again with `Name=foo` signed for eu-west-1 (both succeed: a name is unique per region).
3. For EACH region's `foo`, `PutRumMetricsDestination` twice — once with `Destination=CloudWatch`, once with `Destination=Evidently` plus a `DestinationArn`/`IamRoleArn` — so both maps hold the keys ("CloudWatch","") and ("Evidently",<arn>).
4. `GET /rummetrics/foo/metricsdestination?maxResults=1` signed for us-east-1; keep the returned `NextToken` (it encodes `ListRumMetricsDestinations/1/foo\u{1f}CloudWatch\u{1f}`).
5. Replay that token against the OTHER endpoint: `GET /rummetrics/foo/metricsdestination?maxResults=1&nextToken=<token>` signed for eu-west-1.
6. Observed: HTTP 200 whose page starts at eu-west-1's Evidently destination — eu-west-1's CloudWatch destination is never reported by any page of that walk. Expected per the handler's own doc and the crate's regional-identity rule: the token was not issued by this listing, so the ValidationException the suite asserts for an unissued token (metrics_destinations.rs:678-690).
