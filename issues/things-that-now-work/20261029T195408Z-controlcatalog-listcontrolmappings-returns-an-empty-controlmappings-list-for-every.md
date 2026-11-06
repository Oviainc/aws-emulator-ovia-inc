Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-control-mappings
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-control-mappings
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListControlMappings|2xx->2xx|$.ControlMappings[*]
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# ListControlMappings returns an empty ControlMappings list for every control ARN filter

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListControlMappings

## Description
ListControlMappings answers 200 but never returns a mapping: for every control ARN the corpus filters on, the required ControlMappings list comes back empty. The mapping dataset (framework, common-control and related-control mappings) is not served by the emulator.

AWS: Smithy ListControlMappings (POST /list-control-mappings, paginated inputToken NextToken / outputToken NextToken / pageSize MaxResults up to 1000, items ControlMappings; ControlMappings is a required list) returns, for the requested ControlMappingFilter, one entry per mapping of the control, each carrying ControlArn, MappingType and the matching Mapping union member: MappingType FRAMEWORK with Mapping.Framework {Name, Item}, MappingType COMMON_CONTROL with Mapping.CommonControl {CommonControlArn}, and MappingType RELATED_CONTROL with Mapping.RelatedControl {ControlArn, RelationType}. The recorded responses return pages of 4 to 44 such mappings per control.
Emulator: status matches (200) but every element is missing - $.ControlMappings[0], [1], ... all report "missing (expected <mapping>, got None)" - so the emulator serves an empty list whatever control is filtered on. This reproduces for 17 distinct control ARNs in the fixture (clusters rooted at steps task#1 .. task#20), spanning all three MappingType values; no mapping of any type is ever produced.
Why: a client that asks which compliance-framework items, common controls or related controls a given control satisfies - the normal way to build a compliance cross-reference or to walk from a control to its common controls - gets a 200 with an empty page, so it concludes the control is mapped to nothing instead of surfacing an error, and every framework cross-reference built on the emulator is silently empty.
Fix: the ListControlMappings handler's filter-and-project path and the control-mapping seed state it reads in services/service-com-amazonaws-controlcatalog-controlcatalog - the catalog's mappings must exist in state and be selected by the filter's control ARN, emitting each mapping with the MappingType that matches the populated Mapping union member.

Note: the open issue 20261028T193757Z-controlcatalog-listcontrolmappings-maintainability.md is about how the filter's control ARN is read through a hand-rolled helper; it does not cover the operation returning no mappings at all.

Traces: controlcatalog-catalog-list-control-mappings#task#1 (representative) plus #task#2, #task#3, #task#4, #task#5, #task#6, #task#7, #task#8, #task#9, #task#11, #task#12, #task#13, #task#15, #task#17, #task#18, #task#19, #task#20.

## Evidence
Expected status 200, got 200, body empty on the emulator side. Examples: $.ControlMappings[0]: missing (expected {'ControlArn': 'arn:aws:controlcatalog:::control/1038h7d21e6g8i10g6v3xsssi', 'MappingType': 'FRAMEWORK', 'Mapping': {'Framework': {'Name': 'NIST-CSF-v1.1', 'Item': 'PR.AC-4'}}}, got None); $.ControlMappings[5]: missing (expected {... 'MappingType': 'COMMON_CONTROL', 'Mapping': {'CommonControl': {'CommonControlArn': 'arn:aws:controlcatalog:::common-control/1d01qjgc798y7qvapb22amb3a'}}}, got None); $.ControlMappings[3]: missing (expected {... 'MappingType': 'RELATED_CONTROL', 'Mapping': {'RelatedControl': {'ControlArn': '...', 'RelationType': 'COMPLEMENTARY'}}}, got None). 17 clusters with divergence keys service=controlcatalog|ListControlMappings|status=200->200|keys=$.ControlMappings[0..n]; largest expected page is 44 mappings, emulator returns 0 in all of them.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Replay fixture controlcatalog-catalog-list-control-mappings step task#1: POST /list-control-mappings with a ControlMappingFilter naming a catalog control ARN -> emulator returns 200 with an empty ControlMappings list; AWS returns that control's FRAMEWORK, COMMON_CONTROL and RELATED_CONTROL mappings.
3. Repeat for steps task#2 .. task#20 (other control ARNs) - every one returns an empty list.

## Resolution

The mapping dataset IS served: `ListControlMappings` reports, for every control the
catalog publishes, that control's whole recorded mapping set — the common control it
implements, each published compliance-framework item it satisfies and each control it
relates to — selected by the filter's control ARN in either accepted ARN form, by common
control ARN and by mapping type. The empty page the fixture sees comes from the filter
naming a control ARN the catalog does not publish, which narrows to nothing.

The part of this report's `Fix` sentence that states a rule — "emitting each mapping with
the MappingType that matches the populated Mapping union member" — is now implemented as
that rule rather than as a value stored beside the union. `MappingType` is a function of
the mapping's own details (`state::MappingDetails::mapping_type`), so the `MappingType` a
mapping is reported under, the key the catalog stores it under and the value of the
`MappingTypes` filter that selects it are all one fact and cannot disagree; previously the
kind was a second `String` carried next to the union. Pinned over the whole catalog by
`tests/all/mapping_type_agreement.rs`, which also reads every mapping's target back
through the operation that owns it (`ListCommonControls`, `GetControl`) and checks a
relation is reported from both sides.

The recorded cause is DECLINED, for the reason the earlier
`20261023T204557Z-controlcatalog-listcontrolmappings-returns-no-framework-common-control-or`
issue recorded: each replayed step filters on one of AWS's own opaque published catalog
identifiers (`control/1038h7d21e6g8i10g6v3xsssi`, …) and expects the 4–44 framework,
common-control and related-control rows AWS attaches to it. Those identifiers and rows are
AWS's published catalog CONTENT; no rule derives them from the request or from the model,
and they exist only in the recording, so seeding them would be fitting the recorded
instance. The catalog publishes its own controls under identifiers minted deterministically
from each row's slug, and a well-formed control ARN it does not publish selects no mapping
(200 with an empty list), which `tests/all/control_mappings.rs` pins. The replay of this
fixture therefore still diverges on `$.ControlMappings[*]`.
