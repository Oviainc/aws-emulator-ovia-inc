Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-control-mappings
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-control-mappings
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListControlMappings|2xx->2xx|$.ControlMappings[*]
Issue-Commit: 7e25aa936d91b34bd56006c63e64343163fb8337

# ListControlMappings returns an empty ControlMappings list for every filter

> _Note: this issue was identified against the code base at commit `7e25aa936d91b34bd56006c63e64343163fb8337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListControlMappings

## Description
Cause 1 of 1: ControlCatalog's ListControlMappings answers 200 but never serves any control mapping, for any value of Filter.ControlArns. The emulator carries no control-mapping dataset at all, so the required output member comes back empty (or absent) on every request. This is a single defect with 17 distinct replay clusters behind it (one per control ARN exercised by the corpus, fixture controlcatalog-catalog-list-control-mappings steps task#1 through task#20); all 17 clusters report the identical shape of failure, so one fix closes them all — but the fixer must re-replay ALL of the steps named under Traces, not just the representative, because each cluster asserts a different control's mapping set.

AWS: Smithy (com.amazonaws.controlcatalog) — ListControlMappings, POST /list-control-mappings, output ListControlMappingsResponse.ControlMappings is @required (list of ControlMapping) with pagination {inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults, items: ControlMappings}. Each ControlMapping requires ControlArn (pattern ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$), MappingType (enum FRAMEWORK | COMMON_CONTROL | RELATED_CONTROL) and Mapping (union of Framework{Name,Item} | CommonControl{CommonControlArn} | RelatedControl{ControlArn,RelationType}). Input Filter is ControlMappingFilter with ControlArns / CommonControlArns / MappingTypes, each @length min 1 max 1. The catalog's actual mapping content is recorded behavior only: the fixtures show that filtering by a single control ARN returns that control's full mapping set, mixing all three MappingType values — e.g. FRAMEWORK entries naming compliance frameworks and items (NIST-CSF-v1.1 / PR.AC-4, PCI-DSS-v4.0 / 8.2.1, ISO-IEC-27001:2022-Annex-A / 8.22, CRI-Profile-v2.2 / PR.IR-01.03, FedRAMP-r4 / AC-12, CIS-v8.0 / 3.3, CMMC-Level2 / CM.L2-3.4.2, SSAE-18-SOC-2-Oct-2023 / CC6.1, ACSC-ISM-02-Mar-2023 / ISM-1579, AWS-WAF-v10 / SEC08-BP04), COMMON_CONTROL entries carrying a CommonControlArn, and RELATED_CONTROL entries carrying a peer control ARN plus RelationType (ALTERNATIVE or COMPLEMENTARY). Cardinality is per control and substantial: the recordings range from 4 mappings for one control up to 44 for another. Every returned ControlArn echoes the ARN that was filtered on.

Emulator: status matches (200), but the body carries no mappings. Replay reports every expected element as missing: "$.ControlMappings[0]: missing (expected {...}, got None)" through "$.ControlMappings[43]: missing", with no 'extra' elements reported — i.e. the emulator returns zero ControlMappings for all 20 filtered control ARNs the corpus exercises. Nothing is served for FRAMEWORK, COMMON_CONTROL or RELATED_CONTROL mapping types.

Why: ListControlMappings is how a client discovers which compliance-framework requirements a control satisfies, which common control it implements, and which controls are alternatives or complements to it. A client relying on AWS's behavior concludes that every control in the catalog is unmapped: compliance-coverage reports come out empty, framework-to-control traceability matrices are blank, 'which control covers PCI-DSS 8.2.1' resolves to nothing, and related/alternative-control substitution logic silently never fires. Because ControlMappings is @required and a legitimately empty list is indistinguishable from this defect, the failure is silent — no error surfaces and the client cannot tell the catalog was not served. Paginating clients also never observe NextToken, so they cannot detect truncation either.

Fix: the controlcatalog ListControlMappings handler and the control-mapping dataset backing it in the service's state/seed data. The catalog of control mappings must be populated and the handler must resolve Filter.ControlArns / CommonControlArns / MappingTypes against it, emit one ControlMapping per relationship with the filtered ControlArn echoed, the correct MappingType, and the matching Mapping union arm (Framework{Name,Item} / CommonControl{CommonControlArn} / RelatedControl{ControlArn,RelationType}), and honour MaxResults/NextToken pagination. Note that this is a distinct defect from the already-open controlcatalog dataset issues — 20261101T194508Z-controlcatalog-serves-a-fabricated-placeholder-control-dataset-getcontrol.md (controls), 20261030T021409Z-controlcatalog-listcommoncontrols-returns-an-empty-commoncontrols-list-for-every--r2.md (common controls) and 20261106T092706Z-controlcatalog-taxonomy-list-operations-serve-none-of-the.md (domains/objectives) — none of which carry a replay key for ListControlMappings; the mapping relation is its own dataset and its own handler.

Traces: fixture controlcatalog-catalog-list-control-mappings, steps task#1 (representative), task#2, task#3 (also covers task#16), task#4, task#5, task#6, task#7, task#8 (also covers task#10), task#9, task#11 (also covers task#14), task#12, task#13, task#15, task#17, task#18, task#19, task#20.

## Evidence
Expected (recorded AWS, fixture controlcatalog-catalog-list-control-mappings#task#1, HTTP 200): ControlMappings is a 30-element list, every element echoing the filtered ControlArn arn:aws:controlcatalog:::control/<id> and carrying MappingType plus the matching Mapping arm, e.g. {"ControlArn": "arn:aws:controlcatalog:::control/<id>", "MappingType": "FRAMEWORK", "Mapping": {"Framework": {"Name": "NIST-CSF-v1.1", "Item": "PR.AC-4"}}}, {"MappingType": "COMMON_CONTROL", "Mapping": {"CommonControl": {"CommonControlArn": "arn:aws:controlcatalog:::common-control/<id>"}}}, {"MappingType": "RELATED_CONTROL", "Mapping": {"RelatedControl": {"ControlArn": "arn:aws:controlcatalog:::control/<id>", "RelationType": "COMPLEMENTARY"}}}.

Actual (emulator, HTTP 200): zero mappings. Replay diff for task#1: "$.ControlMappings[0]: missing (expected {...}, got None)" repeated for indices 0..29; no 'extra' entries. The same all-missing/no-extra pattern holds for every other filtered control ARN, with cluster sizes from 4 expected mappings (task#19) to 44 (task#4).

Status and error code do not diverge (200 == 200) and there is no error message involved; the divergence is purely the empty required ControlMappings body. 17 divergence clusters, covering fixture steps task#1..task#20, all with verdict=mismatch, stub_path empty (the operation is implemented, not stubbed) and owned_by empty (no open issue carries this replay key).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. POST /list-control-mappings against the controlcatalog endpoint with a body of {"Filter": {"ControlArns": ["arn:aws:controlcatalog:::control/1038h7d21e6g8i10g6v3xsssi"]}} — this is fixture controlcatalog-catalog-list-control-mappings step task#1.
3. Observe HTTP 200 with an empty ControlMappings list, instead of that control's mapping set (30 entries in the recording, spanning MappingType FRAMEWORK, COMMON_CONTROL and RELATED_CONTROL).
4. Repeat for the other filtered control ARNs exercised by steps task#2..task#20 of the same fixture; every one returns no mappings.
5. Also try {"Filter": {"MappingTypes": ["RELATED_CONTROL"]}} and {"Filter": {"CommonControlArns": ["arn:aws:controlcatalog:::common-control/1d01qjgc798y7qvapb22amb3a"]}} — the same empty list comes back, confirming no mapping dataset exists behind any filter arm.
