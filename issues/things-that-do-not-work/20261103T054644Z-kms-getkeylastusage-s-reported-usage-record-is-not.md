Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.get-key-last-usage-monotonic-timestamps
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.get-key-last-usage-monotonic-timestamps|_poll_for_last_usage#8|Timeout|time.sleep(interval_s)
Issue-Commit: ea825c3b5f0a7c3e6fb8963cf0e8152afa9f60b5

# KMS GetKeyLastUsage's reported usage record is not monotonic: a second cryptographic operation erases the record already reported, so KeyLastUsage.Timestamp disappears instead of advancing

> _Note: this issue was identified against the code base at commit `ea825c3b5f0a7c3e6fb8963cf0e8152afa9f60b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetKeyLastUsage

## Description
One root cause. Cause 1 of 1: GetKeyLastUsage drops the last-usage record it has already reported as soon as the key is used again, so the reported KeyLastUsage.Timestamp goes from present back to ABSENT and never advances for a client that polls it.

AWS: GetKeyLastUsage answers 200 with KeyLastUsage populated from KMS's own usage tracking. Smithy: GetKeyLastUsage input KeyId (KeyIdType, required, length 1..2048); output KeyId, KeyLastUsage (KeyLastUsageData), TrackingStartDate, KeyCreationDate; errors DependencyTimeoutException (503), InvalidArnException (400), KMSInternalException (500), NotFoundException (404). Smithy: KeyLastUsageData.Operation/Timestamp/CloudTrailEventId/KmsRequestId are each documented "Absent if the key has not been used since KMS began tracking", and the operation's own documentation states "If the KeyLastUsage response element is present, the key has been used for a successful cryptographic operation since the TrackingStartDate" and "If the KeyLastUsage response element is empty and KeyCreationDate is on or after TrackingStartDate, the key has not been used for a successful cryptographic operation since it was created". So the element's presence is a monotone fact about the key: tracking lags the call that it records, but once a record has been produced for a key, a LATER successful use replaces that record with a newer one — it never takes the key back to the "not used since tracking began" state, and the reported Timestamp only moves forward. Successive Encrypt calls therefore yield a strictly advancing KeyLastUsage.Timestamp (the absolute instants, the CloudTrail event id and the KMS request id are per-run values and are not the requirement; the requirements are: the element becomes present after use, stays present, and its Timestamp strictly advances on a later use).

Emulator: the key record keeps exactly ONE usage (crate::state::Cmk::last_usage, written by Cmk::record_last_usage) and the read projection Cmk::reported_last_usage filters that single record through KeyUsage::tracked_by(now) = now >= at + USAGE_TRACKING_LAG (USAGE_TRACKING_LAG = 3s), falling back to KeyLastUsageData::default() — i.e. the all-members-absent record — whenever the newest usage is not yet "tracked". A second Encrypt therefore OVERWRITES a record that has already been reported, and every GetKeyLastUsage during the following lag window answers 200 with an empty KeyLastUsage: Operation, Timestamp, CloudTrailEventId and KmsRequestId all absent, although the key has by then been used twice. A client polling for a Timestamp newer than the one it already saw is answered, in order, "present" then "absent" then "present with the same value it saw" — the reported record regresses instead of advancing. In this replay the polling loop never obtains an advanced Timestamp and the check is killed by the harness cap ("Check timed out after 60.0s") instead of concluding.

Why: KeyLastUsage is the surface AWS documents for the key-decommissioning question "has this key been used, and when?". A client that reads it after a burst of cryptographic operations is told the key has NOT been used since tracking began — strictly less than it was told before the latest use — so a drift/idle-key report concludes a live key is unused, and any wait/poll loop watching KeyLastUsage.Timestamp advance (exactly the documented usage-monitoring workflow) either stalls forever or observes the timestamp going backwards to nothing.

Fix: the GetKeyLastUsage state projection — crate::state::Cmk::reported_last_usage, KeyUsage::tracked_by and the USAGE_TRACKING_LAG constant in the kms crate's state, read by the GetKeyLastUsage handler. The tracked record that has already been produced must be held and only REPLACED when the tracking produces a newer one, never erased by a use the tracking has not caught up with; and the emulator's (undocumented, emulator-chosen) lag must be short enough that a newly tracked use is reported to a polling client rather than leaving it to poll indefinitely. This must not be fixed by re-fabricating a record from the in-process call log: the fixed issue 20261029T235222Z-kms-read-projections-diverge-key-policy-re-serialized.md (cause 3) established that a read taken immediately after a use carries the EMPTY record, and that behavior is correct and must stay. What is missing is (a) the hold-over of a record already reported and (b) the forward advance of the reported Timestamp on the next tracked use. The response is 200 throughout, so this is not a refusal regression of that fixed issue.

Traces: behavioral check kms.get-key-last-usage-monotonic-timestamps.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service heading `kms`: [mismatch] kms.get-key-last-usage-monotonic-timestamps — "Error: Check timed out after 60.0s".

What the check does (CreateKey SYMMETRIC_DEFAULT/ENCRYPT_DECRYPT, then): Encrypt -> poll GetKeyLastUsage until KeyLastUsage.Timestamp is present (first_ts) -> Encrypt again -> poll GetKeyLastUsage until KeyLastUsage.Timestamp > first_ts, asserting second_ts > first_ts.

Expected (AWS): both polls terminate — the element becomes present after the first Encrypt and, after the second Encrypt, reports a Timestamp strictly greater than the first (HTTP 200 both times, KeyLastUsage present).

Actual (emulator): HTTP 200 both times, but after the second Encrypt every read answers KeyLastUsage with Operation, Timestamp, CloudTrailEventId and KmsRequestId ALL absent — the record reported for the first Encrypt has been overwritten by an untracked newer usage and is gone. The second poll never observes an advanced Timestamp and the check is killed at the 60 s harness cap.

Code grounding in the checkout: services/service-com-amazonaws-kms-trentservice/src/ops/get_key_last_usage.rs:257 projects `key.reported_last_usage(SystemTime::now())`; services/service-com-amazonaws-kms-trentservice/src/state.rs:2372 `Cmk::reported_last_usage` = `self.last_usage.as_ref().filter(|u| u.tracked_by(now)).map(KeyUsage::last_usage_data).unwrap_or_default()`; state.rs:2116 `tracked_by` = `now >= self.at + USAGE_TRACKING_LAG`; state.rs:240 `USAGE_TRACKING_LAG = Duration::from_secs(3)`; state.rs:2354 `record_last_usage` replaces `last_usage` outright. A single stored usage plus an unwrap_or_default() fallback is what makes an already-reported record vanish.

Smithy cross-check (services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json): KeyLastUsageData members are all optional and each documented "Absent if the key has not been used since KMS began tracking"; GetKeyLastUsage's documentation ties the element's PRESENCE to "the key has been used for a successful cryptographic operation since the TrackingStartDate", and its EMPTINESS to the key not having been used since then.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and look at the `kms` section: kms.get-key-last-usage-monotonic-timestamps fails with "Check timed out after 60.0s".
3. By hand, against the emulator's KMS endpoint:
   a. CreateKey(KeyUsage=ENCRYPT_DECRYPT, KeySpec=SYMMETRIC_DEFAULT) -> note KeyId.
   b. Encrypt(KeyId, Plaintext=b"first-usage").
   c. Poll GetKeyLastUsage(KeyId) until KeyLastUsage.Timestamp is present; record it as first_ts (it appears a few seconds after the Encrypt).
   d. Encrypt(KeyId, Plaintext=b"second-usage").
   e. Immediately call GetKeyLastUsage(KeyId) again: the response is 200 but KeyLastUsage carries NO Operation, Timestamp, CloudTrailEventId or KmsRequestId — the record observed at (c) has been erased, although the key has now been used twice. AWS keeps reporting a record here and advances its Timestamp to the second use once tracking produces it; it never returns the key to the "not used since tracking began" state.
   f. Keep polling: the reported Timestamp never becomes strictly greater than first_ts within the check's bound, which is what makes the poll loop in step 2 run to the harness cap.
