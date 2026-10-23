Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-control-mappings
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-control-mappings
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListControlMappings|2xx->2xx|$.ControlMappings[*]
Issue-Commit: 9b28682be3d06b0e1ca8f56d8f0bc182f6004def

# controlcatalog ListControlMappings returns no FRAMEWORK, COMMON_CONTROL or RELATED_CONTROL mappings (first control-ARN group)

> _Note: this issue was identified against the code base at commit `9b28682be3d06b0e1ca8f56d8f0bc182f6004def`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListControlMappings

## Description
ListControlMappings answers 200 with an empty ControlMappings list for every control ARN the corpus queries. This issue carries the first nine of the seventeen response clusters (the remaining eight are filed as the 'controlcatalog ListControlMappings mapping catalog empty for RELATED_CONTROL and remaining control ARNs' issue); all nine causes below must be closed. The resolved issue 20261021T234107Z-controlcatalog-listcontrolmappings-yields-no-framework-common-control-or-related covers this surface but is CLOSED and so does not cover these replays.

AWS (applies to every cause): ListControlMappings (Smithy: restJson1, POST /list-control-mappings; output ListControlMappingsResponse) returns a paginated ControlMappings list; each entry carries ControlArn, MappingType (FRAMEWORK | COMMON_CONTROL | RELATED_CONTROL) and a matching Mapping union member — Mapping.Framework {Name, Item}, Mapping.CommonControl {CommonControlArn} or Mapping.RelatedControl {ControlArn, RelationType}. Membership per control ARN is recorded behavior only.
Emulator (applies to every cause): HTTP 200 with none of the recorded entries present ($.ControlMappings[0..n] missing).
Why (applies to every cause): compliance tooling that maps an AWS control onto framework items (NIST, PCI-DSS, ISO, FedRAMP, CIS, CRI...) or onto its common/related controls receives a successful empty answer and reports zero framework coverage for every control.
Fix (applies to every cause): the ListControlMappings handler and the seeded mapping catalog (control -> framework/common-control/related-control relations) it reads.

Cause 1 of 9: control/1038h7d21e6g8i10g6v3xsssi — 30 mappings expected (NIST-CSF-v1.1 PR.AC-4, AWS-WAF-v10 SEC08-BP04, CIS-v7.1 16.3, PCI-DSS-v4.0 8.2.1, FedRAMP-r4 AC-12/AC-3, ISO-IEC-27001 A.9.4.1, ...). Trace: controlcatalog-catalog-list-control-mappings#task#1.
Cause 2 of 9: control/1ks6hwrtiiia8t2djn1iys62x — 42 mappings expected, mixing FRAMEWORK (CIS-v8.0 3.3, CRI-Profile-v2.2 PR.IR-01.03, NIST-SP-800-53-r5 SC-7) and COMMON_CONTROL (common-control/1d01qjgc798y7qvapb22amb3a). Trace: #task#2.
Cause 3 of 9: control/27lgd3oknluio5kzcl324kr0d — 9 mappings expected, including a RELATED_CONTROL entry {ControlArn control/ywt2dac1mdoeffk216hldg2q, RelationType COMPLEMENTARY}. Trace: #task#3 (2 steps).
Cause 4 of 9: control/39o509cq5imkyls30q7qc6ynd — 44 mappings expected (FedRAMP-r5 SC-7/SC-7 (5)/AC-4, CIS-AWS-Benchmark-v1.2 4.2, PCI-DSS-v4.0 1.3.1, ...). Trace: #task#4.
Cause 5 of 9: control/45du52gha9l59sznumimptmcj — 13 mappings expected (CRI-Profile-v2.2 PR.PS-04.01/03, NIST-CSF-v1.1 PR.IP-6, SSAE-18-SOC-2 C1.1, COMMON_CONTROL common-control/cka0tukunopu3vqts04wqfq2z, ...). Trace: #task#5.
Cause 6 of 9: control/55i15zi1ernedla1x1ymgz3gb — 6 mappings expected, including RELATED_CONTROL {control/keijcuz1a7n70grfn62tb2jj, ALTERNATIVE} and COMMON_CONTROL common-control/dchokxd0zjgz3nmih9mal12jw. Trace: #task#6.
Cause 7 of 9: control/5w9ie29h0nydz0kiqjaw8f5wg — 16 mappings expected (CMMC-Level2 CM.L2-3.4.2, NIST-SP-800-171-r2 3.4.2, FedRAMP-r5 CM-6, COMMON_CONTROL common-control/30mf4xxk2johagsi97ynxz8ae and /ez1ccu4xkxrmlekcvjqo3jk59, ...). Trace: #task#7.
Cause 8 of 9: control/6m6ffltyr8ddio6ocdabw8xak — 18 mappings expected (AWS-WAF-v10 REL09-BP01, NIST-SP-800-53-r5 CP-9/CP-10(2), COMMON_CONTROL common-control/5nzyjix1vm9734750ghlfxew7 and /69j1wukdrzz7wubby8863bwed, ...). Trace: #task#8 (2 steps).
Cause 9 of 9: control/7e2vbhsghm8rh3nveq0kft19u — 11 mappings expected (CRI-Profile-v2.2 PR.IR-04.02/PR.IR-03.01, ACSC-ISM-02-Mar-2023 ISM-1580, two RELATED_CONTROL entries, COMMON_CONTROL common-control/dchokxd0zjgz3nmih9mal12jw). Trace: #task#9.

## Evidence
All nine clusters: verdict=mismatch, expected_status 200, actual_status 200, payload missing. Examples:
- #task#1: "$.ControlMappings[0]: missing (expected {'ControlArn': 'arn:aws:controlcatalog:::control/1038h7d21e6g8i10g6v3xsssi', 'Mapping': {'Framework': {'Item': 'PR.AC-4', 'Name': 'NIST-CSF-v1.1'}}, 'MappingType': 'FRAMEWORK'}, got None)" through $.ControlMappings[29].
- #task#3: "$.ControlMappings[3]: missing (expected {... 'Mapping': {'RelatedControl': {'ControlArn': 'arn:aws:controlcatalog:::control/ywt2dac1mdoeffk216hldg2q', 'RelationType': 'COMPLEMENTARY'}}, 'MappingType': 'RELATED_CONTROL'}, got None)".
No cluster carries stub_path or owned_by.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. POST /list-control-mappings with a ControlMappingFilter naming each control ARN above (fixture controlcatalog-catalog-list-control-mappings, steps task#1 through task#9).
3. Each answers 200 with a ControlMappings array that omits every recorded mapping for that control.
