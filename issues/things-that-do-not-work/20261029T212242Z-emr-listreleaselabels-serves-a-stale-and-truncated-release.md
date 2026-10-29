Issue-Source: trace-sweep-divergence
Issue-Trace: emr-catalog-list-release-labels
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-catalog-list-release-labels
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr|ListReleaseLabels|2xx->2xx|$.ReleaseLabels[*]
Issue-Commit: 14bf84ae7f57c35d094a91bae03d265103651cef

# EMR ListReleaseLabels serves a stale and truncated release-label catalog

> _Note: this issue was identified against the code base at commit `14bf84ae7f57c35d094a91bae03d265103651cef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListReleaseLabels

## Description
This issue bundles 5 net-new root causes, all on EMR's `ListReleaseLabels` (service token `emr`), that share ONE fault: the release-label catalog the handler enumerates is a frozen, partial snapshot rather than EMR's full label set. Every call returns HTTP 200 (matching AWS) but the `ReleaseLabels` list is wrong in three consistent ways across release families — entries are missing from the tail, whole families are absent, and the patch-level labels AWS lists are replaced by the older patch of the same minor. Hypothesis: the catalog is a hard-coded table captured at one point in time and filtered per request, so it was never widened with newer releases nor corrected for patch revisions.

AWS (shared by all 5 causes): Smithy: `ListReleaseLabels(Filters: ReleaseLabelFilter, NextToken: String, MaxResults: MaxResultsNumber @range 1..100)` -> `{ReleaseLabels: StringList, NextToken: String}`, errors `InternalServerException` (500) and `InvalidRequestException` (client, 400); the operation is `@paginated(inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults)`. The model does not enumerate the labels, so recorded behavior only for the content: AWS returns the COMPLETE set of release labels matching the request's `Filters` (an application or a release-prefix filter), in descending release order, paginated by MaxResults/NextToken. The requirement is the completeness and ordering of the set for a given filter — the exact label strings in any one recording are catalog data that moves forward over time, so a fixer should treat the recording's set as the floor the catalog must cover, not as a frozen literal.

Emulator (shared by all 5 causes): the served list is a strict, stale subset of the recorded set. The specific shape of the loss differs per filter, which is why each filter is a separate cause a fixer must re-replay — one widened family does not fix another.

Why (shared by all 5 causes): `ListReleaseLabels` is the discovery call a client makes to choose a `ReleaseLabel` for `RunJobFlow`. With entries missing, a client that validates a user-chosen label against this list rejects labels that are in fact valid; a client that picks the newest returned label pins its cluster to an obsolete release; and a client that pages the list to build a selector renders an incomplete menu. Combined with the fact that `RunJobFlow` does not validate `ReleaseLabel` at all (filed separately), there is no path by which a caller can learn the real supported set.

Fix (shared by all 5 causes): the release-label catalog data in the EMR crate that the `ListReleaseLabels` handler reads (its static catalog/state table) plus the filter/ordering logic over it — the table must carry every label per family, at the patch level AWS publishes, and the handler must order and paginate the filtered result as AWS does. `DescribeReleaseLabel` reads the same catalog and is filed separately; widening the table is likely common to both, but these five steps must each be re-replayed.

--- Cause 1 of 5: the emr-4.x family is absent from the catalog entirely ---
AWS: for the filter exercised by this step, AWS returns 15 emr-4.x labels (the 4.9.x, 4.8.x and 4.6.0-down-to-4.2.0 series), in descending order.
Emulator: returns NONE of them — every expected index comes back `None`, so the whole 4.x family is missing from the table.
Fix / Why: as shared above; add the 4.x family to the catalog.
Traces: emr-catalog-list-release-labels#task#1.

--- Cause 2 of 5: the emr-5.x family is truncated after ~14 entries and two patch labels are the wrong patch ---
AWS: returns the full emr-5.x series down to the oldest 5.x label (86+ entries in the recording), including the patch revisions `emr-5.32.1` and `emr-5.31.1`.
Emulator: returns the wrong patch at two positions (`emr-5.32.0` where AWS lists `emr-5.32.1`; `emr-5.30.2` where AWS lists `emr-5.31.1`) and then returns `None` for every index from 14 onward — the list stops short of the family's tail.
Fix / Why: as shared above; the table must carry the full 5.x tail and the published patch revisions.
Traces: emr-catalog-list-release-labels#task#2.

--- Cause 3 of 5: the emr-6.x family has the wrong patch labels and omits the 6.0.0 beta labels ---
AWS: returns the emr-6.x series including the patch revisions `emr-6.9.1` and `emr-6.8.1`, and the two pre-GA labels `emr-6.0.0-beta` and `emr-6.0.0-beta2` at the tail.
Emulator: serves `emr-6.9.0` and `emr-6.7.0` at those positions (older patch / wrong minor) and returns `None` for both beta labels, so the pre-GA labels are absent.
Fix / Why: as shared above; carry the 6.x patch revisions and the beta labels.
Traces: emr-catalog-list-release-labels#task#3.

--- Cause 4 of 5: the emr-7.x family is frozen at 7.5.0, so every newer 7.x release is missing ---
AWS: returns the emr-7.x series from the newest label downward (the recording's head is `emr-7.14.0`, then 7.13.0, 7.12.0, 7.11.0, 7.10.0, 7.9.0, 7.8.0, 7.7.0, 7.6.0, ...).
Emulator: its 7.x list begins at `emr-7.5.0` and runs down to `emr-7.0.0`, then `None` — i.e. the table was captured when 7.5.0 was newest and the nine newer minors never reach a client. A client picking the newest returned label gets 7.5.0.
Fix / Why: as shared above; the 7.x family must extend to the current head.
Traces: emr-catalog-list-release-labels#task#4.

--- Cause 5 of 5: the emr-spark-* release family is not in the catalog at all ---
AWS: for the filter exercised by this step AWS returns the `emr-spark-*` labels (`emr-spark-8.0.0`, `emr-spark-8.1.0` in the recording) — a release family whose labels do not follow the `emr-<major>.<minor>.<patch>` form.
Emulator: returns `None` for both — it has no `emr-spark-*` entries, so the handler's catalog models only the numeric `emr-N.N.N` naming scheme and the family is unreachable.
Why (specific): a client filtering for this family gets an empty result and concludes the family does not exist, so it cannot select an `emr-spark-*` release at all.
Fix / Why: as shared above; the catalog's label model must admit non-numeric release families such as `emr-spark-*`.
Traces: emr-catalog-list-release-labels#task#5.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator (service token `emr`). All five clusters returned HTTP 200 as expected; only the `ReleaseLabels` list content diverged.

Cause 1 — emr-catalog-list-release-labels#task#1, ListReleaseLabels: `$.ReleaseLabels[0]: missing (expected 'emr-4.9.6', got None)` through `$.ReleaseLabels[14]: missing (expected 'emr-4.2.0', got None)` — all 15 expected 4.x entries absent. divergence_key `service=emr|ListReleaseLabels|status=200->200|keys=$.ReleaseLabels[0]...[14]`.
Cause 2 — emr-catalog-list-release-labels#task#2: `$.ReleaseLabels[7]: missing (expected 'emr-5.32.1', got 'emr-5.32.0')`, `$.ReleaseLabels[9]: missing (expected 'emr-5.31.1', got 'emr-5.30.2')`, then `$.ReleaseLabels[14]` (expected 'emr-5.29.0') through `$.ReleaseLabels[85]` all `got None` — 74 diverging indices.
Cause 3 — emr-catalog-list-release-labels#task#3: `$.ReleaseLabels[8]: missing (expected 'emr-6.9.1', got 'emr-6.9.0')`, `$.ReleaseLabels[10]: missing (expected 'emr-6.8.1', got 'emr-6.7.0')`, `$.ReleaseLabels[24]: missing (expected 'emr-6.0.0-beta', got None)`, `$.ReleaseLabels[25]: missing (expected 'emr-6.0.0-beta2', got None)`.
Cause 4 — emr-catalog-list-release-labels#task#4: `$.ReleaseLabels[0]: missing (expected 'emr-7.14.0', got 'emr-7.5.0')`, `[1]` expected 'emr-7.13.0' got 'emr-7.4.0', `[2]` expected 'emr-7.12.0' got 'emr-7.3.0', `[3]` expected 'emr-7.11.0' got 'emr-7.2.0', `[4]` expected 'emr-7.10.0' got 'emr-7.1.0', `[5]` expected 'emr-7.9.0' got 'emr-7.0.0', and `[6]`/`[7]`/`[8]` (expected 'emr-7.8.0'/'emr-7.7.0'/'emr-7.6.0') all `got None`.
Cause 5 — emr-catalog-list-release-labels#task#5: `$.ReleaseLabels[0]: missing (expected 'emr-spark-8.0.0', got None)`, `$.ReleaseLabels[1]: missing (expected 'emr-spark-8.1.0', got None)`.

Smithy grounding: `ListReleaseLabels` input {Filters: ReleaseLabelFilter, NextToken: String, MaxResults: MaxResultsNumber @range 1..100}, output {ReleaseLabels: StringList, NextToken: String}, paginated(inputToken=NextToken, outputToken=NextToken, pageSize=MaxResults). The model does not enumerate labels, so the catalog content is recorded behavior only.

## Steps to Reproduce
Boot the emulator and replay fixture `emr-catalog-list-release-labels` from wire trace set simparity-wire-aws-v1 (service token `emr`):
1. Step task#1 — ListReleaseLabels filtered to the emr-4.x family; expect 15 labels (emr-4.9.6 .. emr-4.2.0), observe an empty result for that family.
2. Step task#2 — ListReleaseLabels filtered to the emr-5.x family; expect the full 5.x series including emr-5.32.1 and emr-5.31.1, observe older patches at those positions and the list truncated from index 14.
3. Step task#3 — ListReleaseLabels filtered to the emr-6.x family; expect emr-6.9.1, emr-6.8.1 and the emr-6.0.0-beta / emr-6.0.0-beta2 labels.
4. Step task#4 — ListReleaseLabels filtered to the emr-7.x family; expect the series to start at the current head (emr-7.14.0 in the recording), observe it starting at emr-7.5.0.
5. Step task#5 — ListReleaseLabels filtered to the emr-spark-* family; expect emr-spark-8.0.0 and emr-spark-8.1.0, observe an empty result.
In each case compare the full ReleaseLabels list (paging through NextToken) against the recorded response.
