Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-common-controls
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-common-controls
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[*]
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# ListCommonControls returns an empty CommonControls list for every domain and objective filter

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCommonControls

## Description
ListCommonControls answers 200 but with no common-control summaries at all, for every filter the corpus exercises. The common-control dataset the operation projects is not served by the emulator.

AWS: Smithy ListCommonControls (POST /common-controls, paginated inputToken NextToken / outputToken NextToken / pageSize MaxResults, items CommonControls; CommonControls is a required list) returns, for the requested CommonControlFilter (by objective and/or domain), the catalog's common-control summaries - each with Arn, Name, Description, Domain {Arn, Name}, Objective {Arn, Name}, CreateTime and LastUpdateTime. The recorded responses return non-empty pages for each filter.
Emulator: status matches (200) but every element of the page is missing - $.CommonControls[0], [1], [2], ... are all "missing (expected <summary>, got None)", i.e. the required CommonControls list comes back with nothing in it. This reproduces across nine distinct filter shapes in the fixture, covering the change-management, code-review/testing, asset-management, business-continuity, secure-configuration, network-security, asset-inventory, physical-security and identity-and-access domains/objectives (clusters rooted at steps task#1, task#2, task#4, task#5, task#13, task#14, task#35, task#42, task#49, 63 affected steps in total).
Why: a client enumerating common controls - to render the catalog taxonomy, to map a framework requirement onto AWS common controls, or to resolve the CommonControlArn values that ListControlMappings hands back - gets an empty result that is indistinguishable from "this domain has no common controls", so the whole taxonomy appears empty rather than failing loudly.
Fix: the ListCommonControls handler's filter-and-project path and the common-control seed state it reads in services/service-com-amazonaws-controlcatalog-controlcatalog - the catalog's common controls must be present in state and the filter must select them by objective/domain ARN rather than yielding nothing.

Note: the open issue 20261028T193757Z-controlcatalog-listcontrolmappings-maintainability.md touches a neighbouring filter-ARN helper, and the open GetControl issues (20261029T103342Z, 20261029T152442Z, 20261029T183447Z) concern GetControl parameters, ARN-partition validation and authorization. None of them covers ListCommonControls returning an empty list.

Traces: controlcatalog-catalog-list-common-controls#task#1 (representative) plus #task#2, #task#4, #task#5, #task#13, #task#14, #task#35, #task#42, #task#49.

## Evidence
Expected status 200, got 200, but the page body diverges: $.CommonControls[0]: missing (expected {'Arn': 'arn:aws:controlcatalog:::common-control/1nxqoydhsjdyboc4frsrrmev', 'Name': 'Change request and approval', 'Domain': {...'Name': 'Secure development lifecycle and change management'}, 'Objective': {...'Name': 'Change management'}, 'CreateTime': ..., 'LastUpdateTime': ...}, got None); likewise $.CommonControls[1] .. $.CommonControls[10] for the wider filters. Nine clusters (divergence keys service=controlcatalog|ListCommonControls|status=200->200|keys=$.CommonControls[0..n]) covering 63 fixture steps; in every one the emulator's list is empty.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Replay fixture controlcatalog-catalog-list-common-controls step task#1: POST /common-controls with a CommonControlFilter selecting a domain/objective -> emulator returns 200 with an empty CommonControls list; AWS returns the domain's common-control summaries.
3. Repeat for steps task#2, task#4, task#5, task#13, task#14, task#35, task#42, task#49 (other filter shapes) - all come back empty.
