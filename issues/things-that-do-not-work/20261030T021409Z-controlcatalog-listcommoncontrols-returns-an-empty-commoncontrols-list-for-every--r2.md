Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-common-controls
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-common-controls
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[*]
Issue-Commit: d0aae8ae3b786349fe7939f0729ff56210cff281

# ListCommonControls returns an empty CommonControls list for every objective filter

> _Note: this issue was identified against the code base at commit `d0aae8ae3b786349fe7939f0729ff56210cff281`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCommonControls

## Description
ControlCatalog's common-control enumeration answers 200 with no entries: the emulator has no common-control dataset, so the required CommonControls list comes back empty for every request, whether unfiltered or narrowed by CommonControlFilter.Objectives. One defect, but the replay clusters it into NINE separate divergence clusters (nine distinct request/filter shapes), and ALL NINE must be closed and re-replayed before this issue is done — they are numbered as causes below.

AWS (shared by all nine causes): Smithy ListCommonControls (POST /common-controls, paginated inputToken NextToken / outputToken NextToken / pageSize MaxResults, items CommonControls; MaxResults @range min 1 max 100; NextToken @length max 1024; optional CommonControlFilter with a single-element Objectives ARN list — the model documents "You can use this parameter to specify one objective ARN at a time"). The output member CommonControls is @required, and each CommonControlSummary has @required Arn (pattern ^arn:(aws(?:[-a-z]*)?):controlcatalog:::common-control/[0-9a-z]+$, @length 41..2048), Name, Description, Domain {Arn, Name}, Objective {Arn, Name}, CreateTime and LastUpdateTime. AWS returns the catalog's common controls: unfiltered, the whole catalogue across every domain; filtered by an objective ARN, exactly the common controls owned by that objective, each carrying the Domain and Objective it belongs to. Errors modeled: AccessDeniedException 403, ValidationException 400, ThrottlingException 429, InternalServerException 500 — none of which applies here, the recordings are 200 successes.

Emulator (shared by all nine causes): status matches (200), but every expected CommonControlSummary is reported missing with index-addressed paths ($.CommonControls[0], [1], [2], ... : missing (expected the summary, got None)) and no 'extra' entries at all, i.e. the serialized CommonControls list is empty rather than merely mis-ordered or mis-filtered.

Why: a client that enumerates common controls to build the catalog taxonomy, or that walks domain -> objective -> common control (including resolving the CommonControlArn values that ListControlMappings returns for MappingType COMMON_CONTROL), gets a well-formed empty success instead of the catalogue. It cannot distinguish "this objective owns no common controls" from "the catalogue is not served", so it renders an empty catalog and never reports a failure. Pagination is also untestable: with no items, NextToken is never exercised.

Fix: the ListCommonControls handler in services/service-com-amazonaws-controlcatalog-controlcatalog — the common-control seed state it reads, plus the filter-and-project path that must select by CommonControlFilter.Objectives (one objective ARN) and emit each summary's required Arn/Name/Description/Domain/Objective/CreateTime/LastUpdateTime, and then page the result with MaxResults/NextToken. This is the common-control sibling of the already-open issues 20261029T195408Z-controlcatalog-listdomains-and-listobjectives-return-empty-catalog-taxonomy-lists.md (domains/objectives empty) and 20261029T195408Z-controlcatalog-listcontrolmappings-returns-an-empty-controlmappings-list-for-every.md (mappings empty); neither of those covers ListCommonControls, so this operation is filed separately.

Cause 1 of 9: fixture controlcatalog-catalog-list-common-controls, step task#1 — expected the 'Secure development lifecycle and change management' / 'Change management' common controls (common-control/1nxqoydhsjdyboc4frsrrmev 'Change request and approval', common-control/8xy67kb7kojr38ozqpmrm60d7 'Change deployment and automation', and two more); CommonControls[0..3] all missing. 8 steps in this cluster.
Cause 2 of 9: fixture controlcatalog-catalog-list-common-controls, step task#2 — expected the 'Code reviews and testing' objective's controls (common-control/blplx8cx7l3qy5jd6eryax5e7 'Development testing', common-control/euni3089ej2htlwl6tyjpoy79 'Security testing', ...); CommonControls[0..2] missing. 18 steps.
Cause 3 of 9: fixture controlcatalog-catalog-list-common-controls, step task#4 — expected the 'Asset maintenance' objective's controls (common-control/ez1ccu4xkxrmlekcvjqo3jk59 'Asset performance management', common-control/ez4hcdf2fpirhuyojurkval9w 'Asset maintenance scheduling'); CommonControls[0..1] missing. 9 steps.
Cause 4 of 9: fixture controlcatalog-catalog-list-common-controls, step task#5 — expected the 'Crisis and emergency management' objective's controls (common-control/7s3t061xlrf2pg0ku4uc9ies2 'Emergency planning', ... ); CommonControls[0..4] missing. 11 steps.
Cause 5 of 9: fixture controlcatalog-catalog-list-common-controls, step task#13 — expected the 'Secure configuration management' objective's controls (common-control/7m2i6nvt88hcor0f11ywp3rws 'Secure configuration standards', ...); CommonControls[0..5] missing.
Cause 6 of 9: fixture controlcatalog-catalog-list-common-controls, step task#14 — expected the 'Network architecture and secure configuration' objective's controls (common-control/7n06xcbdggxt86y266cwpngh8 'Network topology design and review', ...); CommonControls[0..7] missing. 2 steps.
Cause 7 of 9: fixture controlcatalog-catalog-list-common-controls, step task#35 — expected the 'Asset inventory management' objective's controls (common-control/d4s7ik8fgv8082v3x31hifzcc, common-control/1ukpmkewk4i92tjmhsvewi4y7, ...); CommonControls[0..6] missing. 3 steps.
Cause 8 of 9: fixture controlcatalog-catalog-list-common-controls, step task#42 — expected the 'Physical security management' objective's controls (common-control/eurwhj15bbp9b4g80686ivehk, ...); CommonControls[0..8] missing.
Cause 9 of 9: fixture controlcatalog-catalog-list-common-controls, step task#49 — expected the 'Authentication and access control' objective's controls (common-control/9706hq18za0bapf4n1bdyhxrs, ...); CommonControls[0..10] missing.

Traces: controlcatalog-catalog-list-common-controls#task#1, #task#2, #task#4, #task#5, #task#13, #task#14, #task#35, #task#42, #task#49 (the nine cluster representatives; 54 recorded steps in total diverge on this fixture).

## Evidence
Replay of simparity-wire-aws-v1, operation ListCommonControls (POST /common-controls), verdict mismatch on nine clusters, all with expected_status 200 / actual_status 200 and divergence_key service=controlcatalog|ListCommonControls|status=200->200|keys=$.CommonControls[...].

Cause 1 (task#1): $.CommonControls[0]: missing (expected {'Arn': 'arn:aws:controlcatalog:::common-control/1nxqoydhsjdyboc4frsrrmev', 'Name': 'Change request and approval', 'Description': 'Define, document, implement, and maintain a formal process for submitting and approving change requests...', 'Domain': {'Arn': 'arn:aws:controlcatalog:::domain/eyrstdrz8kmo28x5j8vgal6au', 'Name': 'Secure development lifecycle and change management'}, 'Objective': {'Arn': 'arn:aws:controlcatalog:::objective/14qj3s9axkdma059lhktc14dx', 'Name': 'Change management'}, 'CreateTime': ..., 'LastUpdateTime': ...}, got None); likewise $.CommonControls[1] (common-control/8xy67kb7kojr38ozqpmrm60d7 'Change deployment and automation'), [2] and [3].
Cause 2 (task#2): $.CommonControls[0]: missing (expected common-control/blplx8cx7l3qy5jd6eryax5e7 'Development testing', Objective 'Code reviews and testing', got None); also [1] (common-control/euni3089ej2htlwl6tyjpoy79 'Security testing') and [2].
Cause 3 (task#4): $.CommonControls[0]: missing (common-control/ez1ccu4xkxrmlekcvjqo3jk59 'Asset performance management', Domain 'Asset management', Objective 'Asset maintenance'); [1] (common-control/ez4hcdf2fpirhuyojurkval9w 'Asset maintenance scheduling').
Cause 4 (task#5): $.CommonControls[0..4] missing (common-control/7s3t061xlrf2pg0ku4uc9ies2 'Emergency planning', common-control/v4vb6zir7567vnkcna9ioze 'Emergency communications systems', common-control/djyqf4shcydocdojrlplpu7jj 'Emergency training', ...).
Cause 5 (task#13): $.CommonControls[0..5] missing (common-control/7m2i6nvt88hcor0f11ywp3rws 'Secure configuration standards', common-control/4aih1y0rudjzgspgm9gt0u0h6 'Configuration verification and change detection', ...).
Cause 6 (task#14): $.CommonControls[0..7] missing (common-control/7n06xcbdggxt86y266cwpngh8 'Network topology design and review', common-control/9b2ix5ogwa1b85c81k028juod 'Network access control lists and reviews', ...).
Cause 7 (task#35): $.CommonControls[0..6] missing (common-control/d4s7ik8fgv8082v3x31hifzcc 'Asset inventory reconciliation and audit', common-control/1ukpmkewk4i92tjmhsvewi4y7 'Inventory of authorized assets and automated discovery', ...).
Cause 8 (task#42): $.CommonControls[0..8] missing (common-control/eurwhj15bbp9b4g80686ivehk 'Physical security access control management', common-control/3hdplf9gfvuzqgwqzaewwdjov 'Physical fencing and barriers', ...).
Cause 9 (task#49): $.CommonControls[0..10] missing (common-control/9706hq18za0bapf4n1bdyhxrs 'Secrets complexity requirements policies', common-control/8vtkbmh266wkjbvuxcy20tfa2 'Risk-based authentication', ...).

In every cluster the report contains ONLY 'missing' entries and no 'extra' entries, which is the signature of an empty required CommonControls list (contrast ListControls, where the emulator emits placeholder entries and the report shows both 'missing' and 'extra'). Expected vs actual, stated generally: AWS -> 200 with CommonControls populated by the summaries owned by the requested objective (or the whole catalogue when unfiltered), each summary carrying all seven required members; emulator -> 200 with CommonControls empty.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay fixture controlcatalog-catalog-list-common-controls step task#1 (representative): POST /common-controls -> the emulator answers 200 with an empty CommonControls list; AWS answers 200 listing the 'Change management' objective's common controls.
3. Repeat for the other eight cluster representatives of the same fixture: task#2, task#4, task#5, task#13, task#14, task#35, task#42, task#49 — each sends a different CommonControlFilter.Objectives selection and each gets 200 with an empty CommonControls list instead of that objective's summaries.
4. Confirm the fix by re-replaying all nine steps (and the 54 steps of the fixture overall): CommonControls must be non-empty, must contain exactly the common controls owned by the filtered objective, and each summary must carry Arn, Name, Description, Domain{Arn,Name}, Objective{Arn,Name}, CreateTime and LastUpdateTime.
