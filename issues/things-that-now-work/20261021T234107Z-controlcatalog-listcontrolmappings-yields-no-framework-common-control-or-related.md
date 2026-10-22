Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-control-mappings
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-control-mappings
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListControlMappings|2xx->2xx|$.ControlMappings[*]
Issue-Commit: 837306e76dca4544e8b76426022c8d4a2dc48510

# ListControlMappings yields no framework, common-control or related-control mappings (first ARN group)

> _Note: this issue was identified against the code base at commit `837306e76dca4544e8b76426022c8d4a2dc48510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListControlMappings

## Description
ListControlMappings answers 200 with an empty ControlMappings list for every control ARN queried. This issue covers the first group of 9 diverging control ARNs; a second issue (titled "Second batch of control ARNs also map to nothing in ListControlMappings") covers the remaining 8. 9 causes, all on service controlcatalog, one per queried control ARN - each cause is a separate replay step because each exercises a different control's mapping set.

AWS (applies to all 9 causes): Smithy: ListControlMappings, POST /list-control-mappings, input ControlMappingFilter with ControlArns/CommonControlArns/MappingTypes each @length 1..1, MaxResults 1..1000; output ListControlMappingsResponse.ControlMappings required, @paginated on ControlMappings. Each ControlMapping carries ControlArn, MappingType (FRAMEWORK | COMMON_CONTROL | RELATED_CONTROL) and the matching Mapping union member (Framework {Name, Item}, CommonControl {CommonControlArn}, RelatedControl {ControlArn, RelationType}). AWS returns every mapping the catalog records for the filtered control.
Emulator (applies to all 9 causes): HTTP 200 with the expected ControlMapping entries absent - replay reports each $.ControlMappings[i] as missing (got None), i.e. an empty list where AWS returned 4-44 mappings.
Why: these mappings are the only wire source for "which compliance framework item does this control satisfy", "which common control does it implement" and "which controls are alternatives/complements". An empty list reads as "this control maps to nothing", so compliance cross-walk tooling, framework coverage reports and related-control navigation produce empty output rather than an error.
Fix: the ListControlMappings handler and the control-mapping data it filters (services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/ and src/state.rs).

Cause 1 of 9: fixture controlcatalog-catalog-list-control-mappings step task#1 - control/1038h7d21e6g8i10g6v3xsssi, 30 mappings expected (NIST-CSF-v1.1 PR.AC-4, PCI-DSS-v4.0 8.2.1, ...), none returned.
Cause 2 of 9: step task#2 - control/1ks6hwrtiiia8t2djn1iys62x, 42 mappings expected (CIS-v8.0 3.3, COMMON_CONTROL .../1d01qjgc798y7qvapb22amb3a, ...), none returned.
Cause 3 of 9: step task#3 (cluster also covers task#16) - control/27lgd3oknluio5kzcl324kr0d, 9+ mappings expected including RELATED_CONTROL {ControlArn .../ywt2dac1mdoeffk216hldg2q, RelationType COMPLEMENTARY}, none returned.
Cause 4 of 9: step task#4 - control/39o509cq5imkyls30q7qc6ynd, 44 mappings expected (FedRAMP-r5 SC-7 (5), CIS-AWS-Benchmark-v1.2 4.2, ...), none returned.
Cause 5 of 9: step task#5 - control/45du52gha9l59sznumimptmcj, 13 mappings expected, none returned.
Cause 6 of 9: step task#6 - control/55i15zi1ernedla1x1ymgz3gb, 6 mappings expected including RELATED_CONTROL with RelationType ALTERNATIVE, none returned.
Cause 7 of 9: step task#7 - control/5w9ie29h0nydz0kiqjaw8f5wg, 16 mappings expected, none returned.
Cause 8 of 9: step task#8 (cluster also covers task#10) - control/6m6ffltyr8ddio6ocdabw8xak, 18 mappings expected, none returned.
Cause 9 of 9: step task#9 - control/7e2vbhsghm8rh3nveq0kft19u, 11 mappings expected, none returned.

## Evidence
All nine clusters: expected_status 200, actual_status 200, with every expected $.ControlMappings[i] reported 'missing (expected {...}, got None)'.
Example (Cause 1, step task#1): expected ControlMappings[0] = {ControlArn: arn:aws:controlcatalog:::control/1038h7d21e6g8i10g6v3xsssi, MappingType: 'FRAMEWORK', Mapping: {Framework: {Name: 'NIST-CSF-v1.1', Item: 'PR.AC-4'}}}; got None - and the same for indices 1..29.
Example (Cause 3, step task#3): expected a RELATED_CONTROL mapping {RelatedControl: {ControlArn: arn:aws:controlcatalog:::control/ywt2dac1mdoeffk216hldg2q, RelationType: 'COMPLEMENTARY'}} and a COMMON_CONTROL mapping {CommonControl: {CommonControlArn: arn:aws:controlcatalog:::common-control/be13otgkbmy60qc52qmdvq7ko}}; got None for both.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Replay fixture controlcatalog-catalog-list-control-mappings and compare its POST /list-control-mappings responses at steps task#1, task#2, task#3, task#4, task#5, task#6, task#7, task#8 and task#9.
3. Each step filters on one control ARN and returns 200 with an empty ControlMappings list where the recording returns that control's FRAMEWORK, COMMON_CONTROL and RELATED_CONTROL mappings.

## Resolution

The rule — ListControlMappings reports, for the controls a filter admits, every
control-mapping record the catalog holds for them, and an empty list when the filter
admits no published control — holds and is now pinned end-to-end by
`services/service-com-amazonaws-controlcatalog-controlcatalog/tests/all/control_mappings.rs`,
which compares the wire response against the records in `src/state.rs` for every
published control, every modelled `MappingType`, every published common control and
several ARN forms the catalog does not publish.

Under that rule the catalog's own mapping records were too thin, and that part was
fixed: every published control now records the common control it implements, items in
several published compliance frameworks, and its related controls in both directions
(`COMPLEMENTARY` and `ALTERNATIVE`), instead of a single mapping per control.

All nine recorded causes are DECLINED. Each asks for the mapping set AWS publishes for
one of nine opaque AWS catalog identifiers (`control/1038h7d21e6g8i10g6v3xsssi`, …).
Those identifiers, and the 4–44 framework/common-control/related-control rows attached
to each, are AWS's published catalog *content*: no rule derives them, and they exist
only in the recording, so transcribing them would be fitting the recorded instance
rather than implementing the rule. The replay of this fixture therefore still diverges
on `$.ControlMappings[*]`.
