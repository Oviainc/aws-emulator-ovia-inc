Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.batch-put-timestamp-out-of-range-rejected iotsitewise.batch-put-conflicting-timestamp-quality-rejected
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.batch-put-timestamp-out-of-range-rejected|run#6|AssertionError|assert len(resp["errorEntries"]) == 1, f"expected exactly 1 errorEntries for the future-timestamp entry, got {resp['errorEntries']}"
Issue-Key: iotsitewise.batch-put-conflicting-timestamp-quality-rejected|run#16|AssertionError|assert unchanged["value"]["doubleValue"] == 20.0, f"value changed by the differing-quality put: {unchanged}"
Issue-Commit: 00a15b53a34d04728621f3d09bcc33a2157eb48f

# IoT SiteWise BatchPutAssetPropertyValue ingests values AWS refuses or drops: no TimestampOutOfRangeException errorEntry for future timestamps, and a differing-quality re-put overwrites the stored TQV (2 causes)

> _Note: this issue was identified against the code base at commit `00a15b53a34d04728621f3d09bcc33a2157eb48f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchPutAssetPropertyValue

## Description
Two independent root causes in the iotsitewise data plane (BatchPutAssetPropertyValue write path). Both must be closed before this issue is resolved.

=== Cause 1 of 2: a far-future timestamp is accepted instead of producing a per-entry TimestampOutOfRangeException ===
AWS: Smithy: BatchPutAssetPropertyValueResponse.errorEntries is a required list of BatchPutAssetPropertyError{errorCode (required), errorMessage (required), timestamps (required)}, and the enum BatchPutAssetPropertyValueErrorCode includes the value TimestampOutOfRangeException alongside ResourceNotFoundException/InvalidRequestException/ConflictingOperationException/LimitExceededException. Recorded behavior: an entry whose propertyValues timestamp is more than ten minutes in the future is refused PER ENTRY — the 200 response carries exactly one errorEntry whose entryId is the submitted entryId and whose errors[0].errorCode is "TimestampOutOfRangeException" — and nothing is stored at that timestamp (a history read over a window around it comes back empty). A value at a current timestamp in the same shape still ingests with errorEntries == [].
Emulator: the future-timestamped entry is accepted with errorEntries == [] (observed: [] where exactly one errorEntry was required), so the out-of-range sample is admitted.
Why: a client that checks errorEntries to detect and re-stamp bad samples sees a clean write, so clock-skewed or malformed device data silently enters the property's time series and later aggregate/history reads return points real AWS would never have stored.
Fix: timestamp-range validation in the BatchPutAssetPropertyValue handler and its per-entry error reporting (errorEntries construction with errorCode TimestampOutOfRangeException), not a top-level exception.
Traces: iotsitewise.batch-put-timestamp-out-of-range-rejected.

=== Cause 2 of 2: a re-put at an existing timestamp with a DIFFERENT quality overwrites the stored TQV ===
AWS: recorded behavior only (the model states only that errorEntries is returned). Recording shows: re-submitting a value at a timestamp that already holds a TQV with the SAME quality overwrites the stored value (errorEntries == []); re-submitting at that same timestamp with a DIFFERENT quality is also accepted with errorEntries == [] — no error entry, no exception — but has NO effect: GetAssetPropertyValue keeps returning the previously stored value and quality, stably across repeated independent reads. A genuinely new timestamp ingests normally.
Emulator: the differing-quality entry is applied. After storing (timestamp ts, 20.0, GOOD) and then putting (ts, 30.0, BAD), GetAssetPropertyValue returns the BAD/30.0 sample at ts, where AWS still returns GOOD/20.0.
Why: the latest-value read diverges from AWS for an input AWS deliberately drops. A client (or a downstream alarm/aggregation) sees a value and quality flag that AWS would never surface, so quality-based filtering and last-known-good logic make the opposite decision on identical input.
Fix: the quality-aware write path of the asset-property time-series state used by BatchPutAssetPropertyValue — a put at an occupied timestamp must only overwrite when the quality matches, and otherwise be accepted and discarded.
Traces: iotsitewise.batch-put-conflicting-timestamp-quality-rejected.

## Evidence
Cause 1 — AssertionError: expected exactly 1 errorEntries for the future-timestamp entry, got [] (entry timestamp = now + 20 minutes; AWS allows at most 10 minutes ahead and returns errors[0].errorCode == "TimestampOutOfRangeException").
Cause 2 — AssertionError: value changed by the differing-quality put: {'value': {'doubleValue': 30.0}, 'timestamp': {'timeInSeconds': <ts>}, 'quality': 'BAD'}; AWS keeps {'doubleValue': 20.0, 'quality': 'GOOD'} at <ts> and reports errorEntries == [] for the dropped put.

## Steps to Reproduce
1. Boot the emulator; create an asset model with a DOUBLE measurement property, wait ACTIVE, CreateAsset from it, wait ACTIVE, and resolve the asset's propertyId.
2. Cause 1: BatchPutAssetPropertyValue with one entry (entryId='future') whose timestamp.timeInSeconds = now + 1200 -> response errorEntries is [] instead of one entry with errors[0].errorCode == 'TimestampOutOfRangeException'; GetAssetPropertyValueHistory around that timestamp shows the value was stored.
3. Cause 2: put (ts = now-60, 10.0, GOOD); put (ts, 20.0, GOOD) and confirm 20.0/GOOD; then put (ts, 30.0, BAD) -> GetAssetPropertyValue returns 30.0/BAD, where AWS keeps returning 20.0/GOOD with errorEntries == [] on that third put.
