Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-common-controls
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-common-controls
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[*]
Issue-Commit: 837306e76dca4544e8b76426022c8d4a2dc48510

# ListCommonControls returns an empty CommonControls list for every common-control query

> _Note: this issue was identified against the code base at commit `837306e76dca4544e8b76426022c8d4a2dc48510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCommonControls

## Description
ListCommonControls answers 200 but carries none of the common controls the Control Catalog publishes, for the unfiltered call and for every objective/domain-scoped call in the corpus. 9 causes, all on service controlcatalog, all the same shape of failure against a different slice of the catalog's common-control population - each needs its own replayed step to confirm the data is served.

AWS (applies to all 9 causes): Smithy: ListCommonControls, POST /common-controls, input CommonControlFilter.Objectives (documented as accepting one objective ARN at a time), MaxResults 1..100, NextToken @length max 1024; output ListCommonControlsResponse.CommonControls required, @paginated on CommonControls. Each CommonControlSummary carries Arn, Name, Description, Domain {Arn, Name}, Objective {Arn, Name}, CreateTime, LastUpdateTime. AWS returns the common controls of the requested objective/domain slice.
Emulator (applies to all 9 causes): HTTP 200 with the expected CommonControlSummary entries absent - replay reports each $.CommonControls[i] as missing (got None), i.e. the list is empty where AWS returned 2-11 entries.
Why: common controls are the ontology layer a client walks to go from a domain/objective to the controls that implement it. An empty list is indistinguishable from "this objective has no common controls", so mapping, reporting and UI flows silently produce nothing instead of failing loudly, and paginated walks terminate on the first page.
Fix: the ListCommonControls handler and the common-control catalog data it enumerates/filters (services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/ and src/state.rs).

Cause 1 of 9: fixture controlcatalog-catalog-list-common-controls step task#1 (cluster of 8 steps) - change-management common controls (e.g. common-control/1nxqoydhsjdyboc4frsrrmev 'Change request and approval') absent.
Cause 2 of 9: same fixture step task#2 (cluster of 18 steps) - code-review/testing common controls (e.g. common-control/blplx8cx7l3qy5jd6eryax5e7 'Development testing') absent.
Cause 3 of 9: same fixture step task#4 (cluster of 9 steps) - asset-maintenance common controls (e.g. common-control/ez1ccu4xkxrmlekcvjqo3jk59) absent.
Cause 4 of 9: same fixture step task#5 (cluster of 11 steps) - crisis/emergency-management common controls (e.g. common-control/7s3t061xlrf2pg0ku4uc9ies2) absent.
Cause 5 of 9: same fixture step task#13 - secure-configuration-management common controls (e.g. common-control/7m2i6nvt88hcor0f11ywp3rws) absent (6 entries expected).
Cause 6 of 9: same fixture step task#14 (cluster of 2 steps) - network-security common controls (e.g. common-control/7n06xcbdggxt86y266cwpngh8) absent (8 entries expected).
Cause 7 of 9: same fixture step task#35 (cluster of 3 steps) - asset-inventory common controls (e.g. common-control/d4s7ik8fgv8082v3x31hifzcc) absent (7 entries expected).
Cause 8 of 9: same fixture step task#42 - physical-security common controls (e.g. common-control/eurwhj15bbp9b4g80686ivehk) absent (9 entries expected).
Cause 9 of 9: same fixture step task#49 - identity-and-access-management common controls (e.g. common-control/9706hq18za0bapf4n1bdyhxrs) absent (11 entries expected).

## Evidence
All nine clusters: expected_status 200, actual_status 200, and every expected $.CommonControls[i] reported 'missing (expected {...}, got None)'.
Example (Cause 1, fixture controlcatalog-catalog-list-common-controls step task#1): expected CommonControls[0] = {Arn: arn:aws:controlcatalog:::common-control/1nxqoydhsjdyboc4frsrrmev, Name: 'Change request and approval', Domain: {Arn: .../domain/eyrstdrz8kmo28x5j8vgal6au, Name: 'Secure development lifecycle and change management'}, Objective: {Arn: .../objective/14qj3s9axkdma059lhktc14dx, Name: 'Change management'}, CreateTime 1710288000.0, LastUpdateTime 1710288000.0}; got None.
Example (Cause 9, step task#49): 11 expected entries ($.CommonControls[0]..$.CommonControls[10]) all missing.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Replay fixture controlcatalog-catalog-list-common-controls and compare its POST /common-controls responses at steps task#1, task#2, task#4, task#5, task#13, task#14, task#35, task#42 and task#49.
3. Each returns 200 with an empty CommonControls list where the recording returns 2-11 CommonControlSummary entries for that objective/domain slice.
