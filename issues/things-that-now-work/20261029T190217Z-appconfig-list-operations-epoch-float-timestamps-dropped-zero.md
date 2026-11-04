Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-experiment-definition-list-filters
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-experiment-definition-list-filters appconfig-experiment-definitions-list-filtering appconfig-experiment-list-operations appconfig-experiment-run-full-lifecycle appconfig-experiment-run-multiple-runs appconfig-experiment-run-rerun-history appconfig-list-applications-pagination-and-contents
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-expa-a4f176a0"].CreatedAt
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-expa-a4f176a0"].UpdatedAt
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-expb-a4f176a0"].CreatedAt
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-expb-a4f176a0"].UpdatedAt
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-expa-2b064c0e"].CreatedAt
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-expa-2b064c0e"].UpdatedAt
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-expb-2b064c0e"].CreatedAt
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-expb-2b064c0e"].UpdatedAt
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-experiment-52bcaac3"].CreatedAt
Issue-Key: appconfig|ListExperimentDefinitions|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-experiment-52bcaac3"].UpdatedAt
Issue-Key: appconfig|ListExperimentRuns|2xx->2xx|$.Items[*]
Issue-Key: appconfig|ListExperimentRunEvents|2xx->2xx|$.Items[*]
Issue-Key: appconfig|ListApplications|2xx->2xx|$.Items[*]
Issue-Commit: 473105e5105ba5cee28620541f40434a5fe4ef94

# AppConfig list operations: epoch-float timestamps, dropped zero ExposurePercentage and wrong page ordering

> _Note: this issue was identified against the code base at commit `473105e5105ba5cee28620541f40434a5fe4ef94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListExperimentDefinitions

## Description
The AppConfig paginated read operations diverge from AWS in their page CONTENTS: every timestamp inside a list item is serialized as a bare epoch-seconds float instead of an ISO-8601 date-time string, a zero ExposurePercentage is dropped from run-event items, and ListApplications puts a different application on the page than AWS does. 8 causes are bundled; they are the item serializer and the page-ordering of the one list layer.

Shared AWS behavior for the timestamp causes: every timestamp member reached from a list item targets Iso8601DateTime with @timestampFormat("date-time") - ExperimentDefinition.CreatedAt/UpdatedAt (ListExperimentDefinitions.Items), ExperimentRun.StartedAt/UpdatedAt/EndedAt (ListExperimentRuns.Items) and ExperimentRunEvent.OccurredAt, documented as "The date and time the event occurred, in ISO 8601 format" (ListExperimentRunEvents.Items). So each is an RFC-3339/ISO-8601 string ending in 'Z', never a JSON number. Recorded behavior additionally shows the list summaries carrying CreatedAt at millisecond precision and UpdatedAt truncated to whole seconds - the requirement here is the string date-time encoding.

Emulator: every one of those members is emitted as a JSON floating-point epoch-seconds value.

Why: an SDK decoding a paginated page per the model's date-time format either fails on the whole page or yields nonsense instants, so a client cannot sort, filter or display list results by time - and because a list page is the normal discovery path, this breaks clients that never touch the single-resource APIs.

Fix: the shared item serializer used by the appconfig List* handlers (render Iso8601DateTime members as date-time strings for list items exactly as for full resources), the ExperimentRunEvent serializer's ExposurePercentage member, and the ListApplications page-ordering in the appconfig service state.

Cause 1 of 8: ListExperimentDefinitions - fixture appconfig-experiment-definition-list-filters, step task#7. 200/200; each item's CreatedAt and UpdatedAt are epoch floats where ISO-8601 strings are modeled. Cluster covers 4 steps (task#7..task#10) of that fixture, i.e. every filter variant of the list.

Cause 2 of 8: ListExperimentDefinitions with status/profile filtering - fixture appconfig-experiment-definitions-list-filtering, step task#7. 200/200; same epoch-float CreatedAt/UpdatedAt on every item, confirming the defect is in item serialization and not in a filter path.

Cause 3 of 8: ListExperimentDefinitions - fixture appconfig-experiment-list-operations, step task#6. 200/200; the fixture does not assert the instants themselves, only that they are shaped like timestamps, and the numeric encoding still fails - so the divergence is the TYPE on the wire, not a clock difference.

Cause 4 of 8: ListExperimentRuns - fixture appconfig-experiment-run-full-lifecycle, step task#13. 200/200; the item's StartedAt and UpdatedAt are epoch floats. Every other member of the item (ExperimentDefinitionId, Run, Description, Status) matches, which localises the cause to the timestamp members.

Cause 5 of 8: ListExperimentRunEvents - fixture appconfig-experiment-run-full-lifecycle, step task#14. 200/200; each event item's OccurredAt is an epoch float. EventType/Description/TriggeredBy/ExposurePercentage match, so again only the timestamp encoding diverges.

Cause 6 of 8: ListExperimentRuns with run history - fixture appconfig-experiment-run-multiple-runs, step task#11. 200/200; StartedAt, UpdatedAt and EndedAt of the DONE run and of the RUNNING run are all epoch floats, so the defect covers all three ExperimentRun timestamp members in list form.

Cause 7 of 8: ListExperimentRunEvents - fixture appconfig-experiment-run-multiple-runs, step task#12. 200/200; TWO divergences on one item: OccurredAt as an epoch float, AND ExposurePercentage absent where AWS reports 0.0. AWS: ExperimentRunEvent.ExposurePercentage targets NullablePercentage (float, @range 0.0-100.0) and the recorded RUN_STARTED event for a run started with no exposure carries ExposurePercentage 0.0. Emulator: the member is dropped from the event item, so a client reading the run's event history sees "exposure unknown" at RUN_STARTED instead of "0%". (The sibling cluster at appconfig-experiment-run-full-lifecycle#task#14 shows a NON-zero ExposurePercentage of 75.0 being reported correctly, which isolates the defect to the zero value.)

Cause 8 of 8: ListApplications page ordering - fixture appconfig-list-applications-pagination-and-contents, step task#4 (cluster also covers task#5 and task#6, i.e. the whole page walk). 200/200, and the page is well-formed, but the application it contains is the wrong one: the fixture creates three applications in a fixed order and walks the pages with max_results, and AWS's recorded first page holds the application created THIRD while the emulator's holds the one created SECOND. AWS: recorded behavior only - the model (ListApplications, GET /applications, @paginated inputToken NextToken / outputToken NextToken / items Items / pageSize MaxResults) states no ordering, and the recording shows AWS's order is stable but is NOT the creation order the emulator uses. Hypothesis: AWS pages applications in an id-derived order rather than insertion order. Emulator: pages in creation/insertion order. Why: a client paginating applications does not see the same set per page as against AWS, so page-scoped logic ("process the first page", "stop after finding X") touches different applications, and any fixture or client that pins an item to a page position breaks. The generated application ids and the uuid suffixes in the recorded names are per-run values and are not part of the requirement; the requirement is that the page sequence reproduce AWS's ordering rather than insertion order.

## Evidence
Cause 1 (appconfig-experiment-definition-list-filters#task#7, ListExperimentDefinitions, 200/200):
  $.Items[<name>].CreatedAt: expected an ISO-8601 string (recorded '<date>T<time>.<millis>Z'), got 1793300164.8894117
  $.Items[<name>].UpdatedAt: expected an ISO-8601 string (recorded '<date>T<time>Z', second precision), got 1793300164.8894117

Cause 3 (appconfig-experiment-list-operations#task#6, ListExperimentDefinitions, 200/200):
  $.Items[<name>].CreatedAt / .UpdatedAt: expected '<shaped-not-asserted>' (any timestamp-shaped value), got 1793300167.6680837 -> fails on the wire TYPE alone.

Cause 6 (appconfig-experiment-run-multiple-runs#task#11, ListExperimentRuns, 200/200):
  $.Items[0]: expected {'Description': 'second attempt', 'ExperimentDefinitionId': <id>, 'Run': 2, 'StartedAt': '<shaped-not-asserted>', 'Status': 'RUNNING', 'UpdatedAt': '<shaped-not-asserted>'}, got the same item with StartedAt/UpdatedAt as 1793300171.1627107
  $.Items[1]: likewise, with EndedAt as a number for the DONE run

Cause 7 (appconfig-experiment-run-multiple-runs#task#12, ListExperimentRunEvents, 200/200):
  $.Items[0]: expected {'Description': 'Experiment run started', 'EventType': 'RUN_STARTED', 'ExposurePercentage': 0.0, 'OccurredAt': '<shaped-not-asserted>', 'TriggeredBy': 'USER'}, got {'Description': 'Experiment run started', 'EventType': 'RUN_STARTED', 'OccurredAt': 1793300171.1627107, 'TriggeredBy': 'USER'} -> ExposurePercentage absent AND OccurredAt numeric.
  Contrast appconfig-experiment-run-full-lifecycle#task#14, where ExposurePercentage 75.0 and 50.0 ARE present -> only the 0.0 case is dropped.

Cause 8 (appconfig-list-applications-pagination-and-contents#task#4, ListApplications, 200/200):
  $.Items[0]: expected the application created third (recorded name suffix '-c-'), got the application created second (recorded name suffix '-b-'). The ids ('tn20ygr' expected vs 'algcbw8' actual) and uuid name suffixes are per-run; the divergence is WHICH application lands on the page.

Smithy support: ExperimentDefinition.CreatedAt/UpdatedAt, ExperimentRun.StartedAt/UpdatedAt/EndedAt and ExperimentRunEvent.OccurredAt all target Iso8601DateTime @timestampFormat("date-time") (OccurredAt doc: "The date and time the event occurred, in ISO 8601 format"); ExperimentRunEvent.ExposurePercentage targets NullablePercentage (float, @range 0.0-100.0); ListApplications/ListExperimentDefinitions/ListExperimentRuns/ListExperimentRunEvents are @paginated with no modeled ordering.

## Steps to Reproduce
1. Replay fixture appconfig-experiment-definition-list-filters and inspect step task#7 (Cause 1).
2. Create two experiment definitions under one application, then call ListExperimentDefinitions: AWS returns each item's CreatedAt/UpdatedAt as ISO-8601 strings; the emulator returns JSON numbers.
3. StartExperimentRun with no ExposurePercentage, then ListExperimentRuns and ListExperimentRunEvents (appconfig-experiment-run-multiple-runs#task#11 / #task#12): item StartedAt/UpdatedAt/EndedAt/OccurredAt come back as numbers, and the RUN_STARTED event omits ExposurePercentage where AWS reports 0.0.
4. Update the run's exposure to a non-zero value and list the events again (appconfig-experiment-run-full-lifecycle#task#14): ExposurePercentage is present, isolating the zero case.
5. For the ordering cause, replay appconfig-list-applications-pagination-and-contents steps task#4..task#6: create three applications in order and walk ListApplications with max_results; compare which application occupies each page against the recording.
6. Re-replay all 8 bundled fixture#step bindings listed under Traces to confirm the fix.
