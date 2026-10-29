Issue-Source: trace-sweep-divergence
Issue-Trace: location-tracker-crud-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-tracker-crud-lifecycle
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|DescribeTracker|2xx->2xx|$.CreateTime
Issue-Key: location|DescribeTracker|2xx->2xx|$.Tags
Issue-Key: location|DescribeTracker|2xx->2xx|$.UpdateTime
Issue-Key: location|ListTrackers|2xx->2xx|$.Entries[*]
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# Location tracker read responses serialize timestamps as epoch numbers and carry the wrong member set

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTracker

## Description
Two causes, one shared fault: the tracker read paths serialize the modelled date-time timestamps as bare epoch-second numbers and get the optional member set wrong — one member AWS returns is missing, another AWS omits is emitted.

Cause 1 of 2: DescribeTracker omits Tags and serializes CreateTime/UpdateTime as epoch numbers.
AWS: Smithy: DescribeTracker GET /tracking/v0/trackers/{TrackerName}; output members include Tags (TagMap, @length max 50), CreateTime (required, Timestamp, @timestampFormat date-time) and UpdateTime (required, Timestamp, @timestampFormat date-time), documented as ISO 8601 'YYYY-MM-DDThh:mm:ss.sssZ'. The recorded response for a tracker created without tags carries Tags as an empty JSON object and both timestamps as ISO 8601 date-time strings.
Emulator: Tags is absent from the body, and CreateTime and UpdateTime are JSON numbers (epoch seconds with a fractional part) rather than date-time strings.
Why: an SDK decoding a @timestampFormat date-time member against a bare number either fails or produces a wrong instant, so every consumer of a tracker's create/update time breaks; and a client that reads Tags from DescribeTracker cannot distinguish "no tags" from "field not returned", since AWS answers with an empty map rather than omitting the member.
Fix: the DescribeTracker response serializer and tracker state projection in the location service crate — render Timestamp members with the modelled date-time format and always project the tracker's tag map (empty map when untagged).
Traces: location-tracker-crud-lifecycle#task#2.

Cause 2 of 2: ListTrackers entries repeat the epoch-number timestamps and add a PricingPlanDataSource member the recorded response omits.
AWS: Smithy: ListTrackers POST /tracking/v0/list-trackers; each ListTrackersResponseEntry carries TrackerName, Description, PricingPlan (deprecated, always RequestBasedUsage), CreateTime (required, date-time) and UpdateTime (required, date-time). PricingPlanDataSource is modelled on the entry but deprecated and unused, and the recorded entry does not carry it at all (recorded behavior only).
Emulator: the entry's CreateTime and UpdateTime are epoch-second JSON numbers instead of ISO 8601 date-time strings, and the entry additionally carries PricingPlanDataSource as an empty string, which the recorded listing does not contain.
Why: the listing's timestamps are undecodable in the same way as the describe path, and the extra deprecated member makes a round-trip comparison between a listed entry and the service's own listing fail on a field AWS does not send.
Fix: the ListTrackers entry serializer in the location service crate — share the date-time timestamp rendering with DescribeTracker and stop emitting the deprecated PricingPlanDataSource member.
Traces: location-tracker-crud-lifecycle#task#3.

## Evidence
Cause 1 — location-tracker-crud-lifecycle#task#2 (DescribeTracker, 200 both sides): $.Tags expected {} (empty map), got None; $.CreateTime expected an ISO 8601 date-time string (shape asserted only), got the number 1793317510.879; $.UpdateTime likewise expected a date-time string, got the number 1793317510.879.
Cause 2 — location-tracker-crud-lifecycle#task#3 (ListTrackers, 200 both sides): $.Entries[0] expected {TrackerName, Description, PricingPlan: 'RequestBasedUsage', CreateTime: <date-time>, UpdateTime: <date-time>}, got the same entry with PricingPlanDataSource: '' added and CreateTime/UpdateTime as the numbers 1793317510.879.

## Steps to Reproduce
1. Replay fixture location-tracker-crud-lifecycle against the emulator.
2. Step task#2 — GET /tracking/v0/trackers/{TrackerName} (DescribeTracker) for a tracker created with no tags: observe the missing Tags member and CreateTime/UpdateTime emitted as JSON numbers instead of ISO 8601 date-time strings.
3. Step task#3 — POST /tracking/v0/list-trackers (ListTrackers): observe the entry's epoch-number CreateTime/UpdateTime and the extra PricingPlanDataSource: '' member.
