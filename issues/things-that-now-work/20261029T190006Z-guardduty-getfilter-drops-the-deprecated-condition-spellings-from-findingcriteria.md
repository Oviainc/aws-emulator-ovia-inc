Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-filter-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-filter-lifecycle guardduty-filter-lifecycle-with-criteria guardduty-multiple-filters-isolation guardduty-filter-with-tags-and-rank-update
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetFilter|2xx->2xx|$.findingCriteria.criterion.severity.eq
Issue-Key: guardduty|GetFilter|2xx->2xx|$.findingCriteria.criterion.region.eq
Issue-Key: guardduty|GetFilter|2xx->2xx|$.findingCriteria.criterion.severity.gte
Issue-Key: guardduty|GetFilter|2xx->2xx|$.findingCriteria.criterion.type.eq
Issue-Key: guardduty|GetFilter|2xx->2xx|$.rank
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# GetFilter drops the deprecated Condition spellings from findingCriteria

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFilter

## Description
A filter created with the legacy Condition members (eq / neq / gt / gte / lt / lte) reads back with those members gone, so the criteria the client wrote are unrecoverable; one trace additionally shows filter rank not re-sequenced after a sibling filter is removed. 5 causes.

Smithy grounding shared by causes 1-4: FindingCriteria.Criterion is a map of field name to Condition. Condition models both the current spellings (equals/notEquals/greaterThan/greaterThanOrEqual/lessThan/lessThanOrEqual) and the @deprecated legacy ones (Eq 'eq', Neq 'neq', Gt 'gt', Gte 'gte', Lt 'lt', Lte 'lte'). Deprecated is not removed: AWS still accepts a filter written with the legacy members and echoes them back on GetFilter in the same spelling they were submitted in.

Cause 1 of 5: GetFilter loses criterion.severity.eq.
AWS: a filter created with criterion {severity: {eq: [<value>]}} reads back with that same eq list, the values taken from the create request.
Emulator: criterion.severity is present but its 'eq' member is absent.
Why: the filter still matches findings server-side (or not), but a client cannot read back what it configured; read-modify-write through UpdateFilter silently erases the condition, and infrastructure tools that diff desired vs actual state loop forever.
Fix: the GetFilter serializer for Condition — it must emit the legacy members it stored, not only the current spellings.
Traces: guardduty-filter-lifecycle#task#3.

Cause 2 of 5: GetFilter loses criterion.severity.gte and criterion.region.eq together.
AWS: echoes both the integer gte and the region eq list as submitted.
Emulator: both members absent from their Conditions.
Why: as cause 1 — and here two different legacy members on two different fields are lost in one read, so the whole criterion is empty in effect.
Fix: the GetFilter serializer for Condition.
Traces: guardduty-filter-lifecycle-with-criteria#task#6.

Cause 3 of 5: GetFilter loses criterion.severity.gte and criterion.type.eq after a filter created with tags.
AWS: echoes the integer gte and the finding-type eq list.
Emulator: both absent.
Why: as cause 1.
Fix: the GetFilter serializer for Condition.
Traces: guardduty-filter-with-tags-and-rank-update#task#3.

Cause 4 of 5: GetFilter loses criterion.region.eq when several filters coexist on one detector.
AWS: echoes the region eq list for the named filter.
Emulator: absent.
Why: as cause 1; it also makes two filters that differ only in their region condition read back identically, so a client cannot tell them apart.
Fix: the GetFilter serializer for Condition.
Traces: guardduty-multiple-filters-isolation#task#6.

Cause 5 of 5: a filter's rank is not re-sequenced after a lower-ranked sibling is deleted.
AWS: ranks on a detector form a dense sequence; after the filter ahead of it is deleted the surviving filter's rank moves up. Smithy: GetFilterResponse.Rank (jsonName 'rank', target FilterRank, @range 1..100). The recording shows the survivor reading back at the head of the sequence; the concrete number is whatever position the remaining filter now occupies.
Emulator: leaves the stored rank untouched, so the sequence keeps a hole.
Why: rank decides filter evaluation order; a client that deletes a filter and then creates a new one at the vacated rank gets a collision or an unexpected order.
Fix: the DeleteFilter state transition, which must re-sequence the detector's surviving filter ranks (this trace also loses criterion.region.eq per cause 4).
Traces: guardduty-multiple-filters-isolation#task#9.

Related open issue (does NOT cover these; it carries no replay key for them): 20261029T183757Z-guardduty-accepts-input-real-aws-refuses-off-scale.md.

## Evidence
Cause 1: $.findingCriteria.criterion.severity.eq expected a list value, got None.
Cause 2: $.findingCriteria.criterion.severity.gte expected an integer, got None; $.findingCriteria.criterion.region.eq expected a list, got None.
Cause 3: $.findingCriteria.criterion.severity.gte expected an integer, got None; $.findingCriteria.criterion.type.eq expected a list, got None.
Cause 4: $.findingCriteria.criterion.region.eq expected a list, got None.
Cause 5: $.findingCriteria.criterion.region.eq expected a list, got None; $.rank expected the re-sequenced head position, got the stale stored rank (recording: expected 1, got 2).

## Steps to Reproduce
1. CreateFilter with findingCriteria.criterion using legacy Condition members (severity.eq / severity.gte / region.eq / type.eq), then GetFilter: replay guardduty-filter-lifecycle step task#3, guardduty-filter-lifecycle-with-criteria step task#6, guardduty-filter-with-tags-and-rank-update step task#3, guardduty-multiple-filters-isolation step task#6. The criterion members are gone from each 200 body.
2. Replay guardduty-multiple-filters-isolation step task#9: after the preceding filter is deleted, GetFilter on the survivor still reports its old rank.
