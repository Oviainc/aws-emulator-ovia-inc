Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.get-resource-config-history-window-and-forward-order-select-real-items
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.get-resource-config-history-window-and-forward-order-select-real-items|run#26|AssertionError|assert set(newer) <= set(after_pivot), (
Issue-Commit: be7f7e560904e5f7d0ba877d90da2eae21b18549

# config GetResourceConfigHistory: an earlierTime bound drops configuration items captured after it, so the newer half of the history window comes back empty

> _Note: this issue was identified against the code base at commit `be7f7e560904e5f7d0ba877d90da2eae21b18549`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
One cause.

Cause 1 of 1: `GetResourceConfigHistory` with `earlierTime` set returns none of the configuration items captured at or after that instant, although the same resource's unfiltered history (and its `chronologicalOrder=Forward`/`Reverse` reads) report them. Failing check: config.get-resource-config-history-window-and-forward-order-select-real-items.

AWS: Smithy `com.amazonaws.configservice#GetResourceConfigHistory`, input member `earlierTime` (target `EarlierTime`, timestamp, optional) is documented as "The chronologically earliest time in the time range for which the history requested. If not specified, the action returns paginated results that contain configuration items that start when the first configuration item was recorded." `laterTime` is "The chronologically latest time in the time range ... If not specified, current time is taken." The operation's own documentation states "The list contains details about each state of the resource during the specified time interval." So `earlierTime` is the OPENING bound of the interval: every configuration item whose `configurationItemCaptureTime` falls in [earlierTime, laterTime] must be returned, and only items captured before `earlierTime` may be excluded. The only modeled refusal about the bounds is `InvalidTimeRangeException` (HTTP 400, "The specified time range is not valid. The earlier time is not chronologically before the later time"), which does not apply here: the bound used is strictly inside the observed capture range and strictly before the implicit `laterTime` of now. No error is modeled for, and AWS does not answer with, a silently truncated window.

Emulator: for a resource whose recorded history contains capture times both before and after a chosen instant, a read with `earlierTime` set to that instant (and no `laterTime`) returns 200 with a `configurationItems` list that contains NO item captured after the bound. The replay's bound fell strictly inside the widest gap between two adjacent observed capture times, so exactly one recorded item lay after it; that item was absent from the windowed response while the unfiltered read of the same resource had just reported it. The companion assertion that every returned item is at or after the bound held, i.e. the response did not merely mis-round the boundary or invert the bound — it dropped the entire portion of the history the window was supposed to select. The emulator reports no error and no `nextToken`, so a caller cannot tell the window was truncated.

Hypothesis (neither the model nor the recording states the mechanism): the time-window predicate in the handler is `at <= later && earlier.is_none_or(|earlier| at >= earlier)` over `earlier = input.earlier_time.map(crate::state::window_from_nanos)`, so either the `earlierTime` bound is being materialized at a different scale/instant than the stored `configurationItemCaptureTime` it is compared against, or the item's capture instant is being moved by a concurrent re-record of the same unchanged state (state.rs documents "re-recording the same state only moves the capture instant", which would retroactively relocate an already-reported item out of the window — in AWS a configuration item's `configurationItemCaptureTime` is immutable once recorded).

Why: a client relying on AWS's behavior uses `earlierTime`/`laterTime` to page a resource's configuration history in bounded slices (drift analysis, change audits, "what did this bucket look like after time T"). Against this emulator the newer slice of any such scan is silently empty, so the client concludes the resource had no state changes in that interval and reports a clean audit for a window that in fact contains recorded items. Because the two complementary windows (`earlierTime=T` and `laterTime=T`) no longer partition the history, a slice-by-slice walk also loses items that neither slice reports, and the loss is indistinguishable from "nothing happened" since the status is 200 and no `nextToken` is returned.

Fix: the `GetResourceConfigHistory` handler's time-window filter and the window-bound helpers it uses — make the `earlierTime` bound and the stored `configurationItemCaptureTime` comparable at the same scale and resolution, and keep a recorded item's capture instant immutable so a re-record of an unchanged state cannot move an already-reported item out of a window. Component: the operation's handler plus the recording state that owns a configuration item's capture instant (file/line citations under Traces).

Traces: config.get-resource-config-history-window-and-forward-order-select-real-items. Handler: services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/get_resource_config_history.rs lines 220-245 (bound materialization) and 318-335 (the `items.retain` window predicate). Helpers: services/service-com-amazonaws-configservice-starlingdoveservice/src/state.rs lines 5345-5376 (`epoch_nanos`, `REPORTED_INSTANT_NANOS`, `window_from_nanos`, `window_to_nanos`) and lines 2679-2734 (`configuration_items` / `records_another_state`, where a re-record moves the capture instant). No open issue covers this: the open config issues carry other replay keys, and the resolved 20261025T141829Z-...-config-getresourceconfighistory-every-configuration-item-omits-arn and 20261023T140058Z-config-getresourceconfighistory-stub concerned a missing `arn` member and an unimplemented stub respectively, not the time-window selection.

## Evidence
Check: config.get-resource-config-history-window-and-forward-order-select-real-items (verdict: mismatch).

Setup established by the check before the failing assertion: a customer-managed configuration recorder is running with a delivery channel, an S3 bucket is recorded, and the recorder has captured more than one configuration item for AWS::S3::Bucket/<bucket>. These all PASSED first, so the history and the ordering surface are intact:
  - `chronologicalOrder=Forward` returned a superset of the default (Reverse) capture times;
  - Forward was ascending;
  - reversed(Forward) == explicit Reverse (exact inversion over the same items).

The pivot is derived from the run's own capture times as the midpoint of the widest gap between two adjacent items, so it falls STRICTLY between two observed capture times and the result is independent of boundary inclusivity.

Expected (AWS): GetResourceConfigHistory(resourceType=AWS::S3::Bucket, resourceId=<bucket>, earlierTime=<pivot>) returns every configuration item whose configurationItemCaptureTime is >= pivot — i.e. it must contain the capture times the unfiltered read reported as newer than the pivot.

Actual (emulator): HTTP 200, configurationItems contains no item captured after the pivot. Assertion that failed (check line 563):

    assert set(newer) <= set(after_pivot), (
        f"earlierTime={pivot.isoformat()} dropped items it must return: ..."
    )
    AssertionError: earlierTime=2026-10-29T05:53:02.485563+00:00 dropped items it must return:
        [datetime.datetime(2026, 10, 29, 5, 53, 7, 234760, tzinfo=tzlocal())]

In that message the fixed text is "earlierTime=<bound> dropped items it must return: <list>"; the bound 2026-10-29T05:53:02.485563+00:00 and the dropped capture time 2026-10-29T05:53:07.234760 are per-run values derived from this run's own recording instants and are NOT part of the requirement — the requirement is that the set of capture times greater than the bound is a subset of what the windowed read returns, for any bound strictly inside the observed capture range.

The immediately preceding assertion — every returned item is at or after the bound — PASSED, which rules out a mis-rounded boundary or an inverted bound: the window returned nothing from the portion of the history it was asked for, rather than the wrong side of it. No error code and no nextToken accompanied the truncated list, so the truncation is silent.

## Steps to Reproduce
1. Against the emulator's config endpoint, create an S3 delivery bucket carrying the AWS Config delivery bucket policy, PutConfigurationRecorder for a customer-managed recorder (allSupported=true), PutDeliveryChannel for that bucket, and StartConfigurationRecorder.
2. Create an S3 bucket and then change it (e.g. enable versioning) so the recorder captures at least TWO configuration items for AWS::S3::Bucket/<bucket>. Poll GetResourceConfigHistory until two or more items are present.
3. Read the full history: GetResourceConfigHistory(resourceType="AWS::S3::Bucket", resourceId=<bucket>), walking nextToken to exhaustion. Collect the configurationItemCaptureTime of every item.
4. Pick a pivot instant strictly between two adjacent collected capture times (e.g. the midpoint of the widest gap), so at least one item lies before it and at least one after it.
5. Call GetResourceConfigHistory(resourceType="AWS::S3::Bucket", resourceId=<bucket>, earlierTime=<pivot>), walking nextToken to exhaustion.
6. Observe: the response is 200 but contains none of the capture times from step 3 that are greater than the pivot. AWS returns exactly those items. For contrast, repeat with laterTime=<pivot> and note that the two complementary windows do not partition the history collected in step 3.
