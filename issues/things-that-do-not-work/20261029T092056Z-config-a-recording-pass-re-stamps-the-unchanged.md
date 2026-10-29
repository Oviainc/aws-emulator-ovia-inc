Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.get-resource-config-history-window-and-forward-order-select-real-items
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.get-resource-config-history-window-and-forward-order-select-real-items|run#13|AssertionError|raise AssertionError(
Issue-Commit: 22a1cf2cca3b33c7fa76f7b475d0b7192834968c

# config: a recording pass re-stamps the UNCHANGED current configuration item, so GetResourceConfigHistory never returns a stable history (configurationItemCaptureTime advances ~every 500 ms)

> _Note: this issue was identified against the code base at commit `22a1cf2cca3b33c7fa76f7b475d0b7192834968c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
Cause 1 of 1: the configuration recorder moves the capture instant of a resource's CURRENT configuration item on every recording pass, even when nothing about the resource changed, so two reads of the same unchanged resource's history never report the same items.

AWS: a configuration item is one captured STATE of a resource. Smithy: `ConfigurationItem$configurationItemCaptureTime` — "The time when the recording of configuration changes was initiated for the resource"; `ConfigurationItem$configurationStateId` — "An identifier that indicates the ordering of the configuration items of a resource"; `GetResourceConfigHistory` — "The list contains details about each state of the resource during the specified time interval" and "By default, the results are listed in reverse chronological order". A state the resource went through is therefore immutable once captured: while a recorded resource's configuration does not change, repeated GetResourceConfigHistory calls return the SAME set of items with the SAME configurationItemCaptureTime and configurationStateId values, and a new item (with a new capture time) appears only when Config observes a change (or, under a periodic recording frequency, at that frequency — never sub-second). Recorded behavior only for the exact propagation delay; what the recording shows is a history that is stable between reads of an unchanged resource.

Emulator: the recorder's pass re-writes the resource's current item unconditionally with `last_recorded_at = now`, and the write path only appends to history when the state actually differs — the crate states this outright: "re-recording the same state only moves the capture instant" (`records_another_state`). Because continuous recording owes a fresh pass roughly every `RECORDING_PASS_DUE_AFTER` (500 ms), the newest item's `configurationItemCaptureTime` (and the `configurationStateId` derived from it) advances a few times a second for a resource nobody touched. The replayed check read an S3 bucket's history repeatedly and the recorded (captureTime, configuration) pairs differed on all 5 comparison passes; the last read reported two items at 09:16:55.553386Z and 09:16:50.830891Z, where only the older (already-superseded) item was stable.

Why: a client cannot use the history at all. Anything that reads a history and then re-reads it to confirm (or that caches by configurationItemCaptureTime / configurationStateId, or diffs successive items, or pages with nextToken — which this handler builds from a capture instant) sees the newest item mutate underneath it: a drift-detection or audit consumer reports endless phantom configuration changes for a resource that never changed, pagination can resume after an instant that has since moved, and any "has this resource changed since T?" comparison is always true. It also makes every downstream window/ordering assertion unverifiable, which is why this check fails before it can even reach its earlierTime/laterTime and Forward/Reverse claims (those are separately tracked by the open issue 20261029T055812Z-config-getresourceconfighistory-an-earliertime-bound-drops-configuration-items.md, which this issue does not duplicate: that one is about which items a window selects, this one is about the capture instant of an item that already exists).

Fix: the recorder's inventory write path — the configuration-item writer in the Config service's state (`record_discovered_resource` / `records_another_state`, which must leave the held item's capture instant alone when the observed state equals the recorded state) and the pass that supplies the observed entries with `last_recorded_at = now` (`record_observed` / `observed_s3_buckets` in the recording module). An observation that records no new state must not re-stamp the existing item; only an observation that records a DIFFERENT state (or a periodic frequency actually falling due) may mint an item with a new capture instant.

Traces: config.get-resource-config-history-window-and-forward-order-select-real-items (mismatch). Relevant code: services/service-com-amazonaws-configservice-starlingdoveservice/src/state.rs lines ~3597-3615 (`record_discovered_resource`) and ~2726-2734 (`records_another_state`); services/service-com-amazonaws-configservice-starlingdoveservice/src/state/recording.rs lines ~56 (`RECORDING_PASS_DUE_AFTER`), ~351-377 (`record_observed`) and ~713-732 (`observed_s3_buckets`, `last_recorded_at: Some(now)`); read path services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/get_resource_config_history.rs lines ~308-347.

## Evidence
Failing check config.get-resource-config-history-window-and-forward-order-select-real-items:

  AssertionError: the configuration history of bucket 'sim2real-behav-cfg-hist-<id>' changed during each of 5 comparison passes; last: [datetime.datetime(2026, 10, 29, 9, 16, 55, 553386, tzinfo=tzlocal()), datetime.datetime(2026, 10, 29, 9, 16, 50, 830891, tzinfo=tzlocal())]

Expected (AWS): with the bucket untouched between reads, GetResourceConfigHistory(resourceType=AWS::S3::Bucket, resourceId=<bucket>) returns the same configurationItems on every call — identical configurationItemCaptureTime and configurationStateId per item — so a read-compare-read loop converges on the first pass.

Actual (emulator): the newest configurationItem's configurationItemCaptureTime advances on every read taken more than one recording pass apart (~500 ms), while the resource's configuration document is byte-identical; the loop never observes two equal histories. No error code or non-2xx status is involved — the responses are HTTP 200 with mutating capture instants.

## Steps to Reproduce
1. Put a delivery bucket with the Config delivery policy, put a delivery channel, put a customer-managed configuration recorder with recordingGroup {allSupported: true} and StartConfigurationRecorder.
2. Create an S3 bucket B and wait until GetResourceConfigHistory(resourceType='AWS::S3::Bucket', resourceId=B) returns at least one configurationItem.
3. Make one real change to B (e.g. PutBucketVersioning Status=Enabled) and wait until the history holds >= 2 items.
4. Touch nothing further. Call GetResourceConfigHistory for B twice, a few seconds apart, paginating each call to exhaustion, and compare the (configurationItemCaptureTime, configuration) pairs.
   Expected: the two reads are identical.
   Actual: the newest item's configurationItemCaptureTime (and configurationStateId) differs between the two reads while its configuration is unchanged; repeating the comparison never yields two equal histories.
