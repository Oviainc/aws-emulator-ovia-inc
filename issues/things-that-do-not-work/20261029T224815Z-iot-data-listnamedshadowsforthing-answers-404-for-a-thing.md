Issue-Source: trace-sweep-divergence
Issue-Trace: iotdata-list-named-shadows-pagination
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotdata-list-named-shadows-pagination iotdata-thing-shadow-ops-not-found-for-unseen-thing
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data|ListNamedShadowsForThing|2xx->4xx|$.results
Issue-Key: iot-data|ListNamedShadowsForThing|2xx->4xx|$.timestamp
Issue-Key: iot-data|ListNamedShadowsForThing|2xx->4xx|status
Issue-Commit: 42112e68a35449969ef0708992459a3b7df02e34

# iot-data ListNamedShadowsForThing answers 404 for a thing with no named shadows instead of 200 with an empty results list

> _Note: this issue was identified against the code base at commit `42112e68a35449969ef0708992459a3b7df02e34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNamedShadowsForThing

## Description
One net-new wire divergence in the iot-data (IoT Data Plane) shadow listing surface.

=== Cause 1 of 1: ListNamedShadowsForThing treats "this thing has no stored shadow" as a not-found resource ===
AWS: Smithy models ListNamedShadowsForThing with output members `results` (NamedShadowList), `nextToken` and `timestamp` (required, default 0), and its success status is 200; ResourceNotFoundException (404) is modelled, but recorded behavior shows it is NOT used for an empty listing. Recorded: listing the named shadows of a thing that currently has no named shadow — whether the thing was never written to at all, or every named shadow it had has been deleted — answers HTTP 200 with `results` present and EMPTY ([]) and a `timestamp`. In other words this is a LIST operation whose empty result is an empty collection, not an absence; the 404 is reserved for genuinely unaddressable requests, not for "no items".
Emulator: the handler short-circuits on a `has_any_shadow(thingName)` guard and answers HTTP 404 ResourceNotFoundException ("no shadow exists ...") with no `results` and no `timestamp` — services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/list_named_shadows_for_thing.rs:177-182, whose module comment states the intended rule as "a thing with no stored shadow at all is a 404". The same 404 is served both before any shadow exists and after the thing's shadows are removed, so the divergence covers the opening and closing steps of a shadow lifecycle as well as a never-seen thing.
Why: a client cannot distinguish "nothing to list" from "wrong request" — code that enumerates a thing's named shadows (a reconcile loop, a paginator, a cleanup pass) either crashes on an unexpected ResourceNotFoundException or treats the thing as nonexistent and skips/re-provisions it, where against AWS it would simply iterate zero items. SDK paginators in particular raise instead of yielding an empty page, so any "list then act" flow breaks at both ends of the lifecycle. The missing `timestamp` also removes a member the model marks required on the 200 response.
Fix: the emptiness guard in the ListNamedShadowsForThing handler must go: the operation should return 200 with `results: []`, no `nextToken`, and the response `timestamp` whenever the thing has no named shadows, reserving ResourceNotFoundException for requests that are not addressable at all (services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/list_named_shadows_for_thing.rs, with the shadow lookup in that crate's src/state.rs). Note the unnamed (classic) shadow must still never appear in `results`.
Traces: iotdata-list-named-shadows-pagination#task#1 (representative), plus the same cluster's iotdata-list-named-shadows-pagination#task#10 (after the thing's named shadows are deleted) and iotdata-thing-shadow-ops-not-found-for-unseen-thing#task#3 (a thing never written to).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; 9 of 79 steps mismatched). One divergence cluster, 3 affected steps:

fixture iotdata-list-named-shadows-pagination step task#1 (ListNamedShadowsForThing):
  status: expected 200, got 404
  $.results: missing (expected [], got None)
  $.timestamp: missing (expected a shaped value, got None)

same divergence at iotdata-list-named-shadows-pagination#task#10 and iotdata-thing-shadow-ops-not-found-for-unseen-thing#task#3.

## Steps to Reproduce
1. iot:CreateThing(thingName=T) (or simply pick a thing name that has never been written to).
2. iot-data:ListNamedShadowsForThing(thingName=T) -> expect HTTP 200 with body {"results": [], "timestamp": <epoch seconds>}; observe HTTP 404 ResourceNotFoundException with no results/timestamp.
3. iot-data:UpdateThingShadow(thingName=T, shadowName="s1", payload={"state":{"reported":{"a":1}}}), then ListNamedShadowsForThing(thingName=T) -> 200 with results ["s1"] (this already passes).
4. iot-data:DeleteThingShadow(thingName=T, shadowName="s1"), then ListNamedShadowsForThing(thingName=T) -> expect HTTP 200 with results [] again; observe HTTP 404.
Replay fixtures iotdata-list-named-shadows-pagination (steps task#1 and task#10) and iotdata-thing-shadow-ops-not-found-for-unseen-thing (step task#3) to confirm.
