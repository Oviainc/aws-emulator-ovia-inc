Issue-Source: trace-sweep-divergence
Issue-Trace: connectcases-case-rule-and-layout-lifecycle
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcases-case-rule-and-layout-lifecycle connectcases-layout-lifecycle connectcases-case-field-values-round-trip-and-update connectcases-template-and-case-lifecycle connectcases-domain-lifecycle-and-tags connectcases-field-and-options-lifecycle connectcases-field-lifecycle-and-batch-get connectcases-template-lifecycle
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|GetLayout|2xx->2xx|$.createdTime
Issue-Key: connectcases|GetLayout|2xx->2xx|$.lastModifiedTime
Issue-Key: connectcases|GetLayout|2xx->2xx|$.tags
Issue-Key: connectcases|GetCase|2xx->2xx|$.tags
Issue-Key: connectcases|GetDomain|2xx->2xx|$.createdTime
Issue-Key: connectcases|GetDomain|2xx->2xx|$.lastModifiedTime
Issue-Key: connectcases|GetDomain|2xx->2xx|$.tags
Issue-Key: connectcases|BatchGetField|2xx->2xx|$.fields[*].createdTime
Issue-Key: connectcases|BatchGetField|2xx->2xx|$.fields[*].lastModifiedTime
Issue-Key: connectcases|BatchGetField|2xx->2xx|$.fields[*].tags
Issue-Key: connectcases|BatchGetField|2xx->2xx|$.fields[*].description
Issue-Key: connectcases|GetTemplate|2xx->2xx|$.createdTime
Issue-Key: connectcases|GetTemplate|2xx->2xx|$.defaultValues
Issue-Key: connectcases|GetTemplate|2xx->2xx|$.lastModifiedTime
Issue-Key: connectcases|GetTemplate|2xx->2xx|$.layoutConfiguration
Issue-Key: connectcases|GetTemplate|2xx->2xx|$.rules
Issue-Key: connectcases|GetTemplate|2xx->2xx|$.tagPropagationConfigurations
Issue-Key: connectcases|GetTemplate|2xx->2xx|$.tags
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-7d842d0e"].arn
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-7d842d0e"].id
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-7d842d0e"].tagPropagationConfigurations
Issue-Commit: b00bffca1001eed9eb3256fe81ec389ea90933f7

# connectcases responses serialize timestamps as epoch numbers and drop empty members

> _Note: this issue was identified against the code base at commit `b00bffca1001eed9eb3256fe81ec389ea90933f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLayout

## Description
Seven net-new wire divergences on SUCCESSFUL (200) connectcases responses. Two serialization faults recur across every read operation, and a third drops modeled members entirely:
(a) TIMESTAMPS: every connectcases timestamp member (createdTime, lastModifiedTime; targets CreatedTime / LastModifiedTime) carries @timestampFormat "date-time" in this restJson1 model, so AWS serializes them as RFC3339/ISO-8601 STRINGS. The emulator emits a bare JSON number (epoch seconds with a fractional part), which is a different JSON type - AWS SDKs deserializing a date-time member from a number either fail or silently mis-type it.
(b) EMPTY COLLECTIONS: `tags` (Tags, a sparse map) and the empty list/struct members below are PRESENT in the recorded responses with their empty value ({} / [] / a struct of nulls); the emulator omits the key.
(c) MISSING MEMBERS: some modeled/recorded members are never emitted at all.
THIS ISSUE HAS SEVEN CAUSES - close all seven before submitting.

=== Cause 1 of 7 (representative): GetLayout (fixture connectcases-case-rule-and-layout-lifecycle, step task#11) ===
AWS: Smithy: GetLayout = POST /domains/{domainId}/layouts/{layoutId}; GetLayoutResponse.createdTime and .lastModifiedTime target CreatedTime / LastModifiedTime with timestampFormat date-time (type timestamp); .tags targets Tags (sparse map). Recorded response carries both timestamps as date-time strings (the fixture shape-asserts them, not their value) and `tags` present as {}.
Emulator: createdTime and lastModifiedTime are JSON numbers (epoch seconds, e.g. 1794247493.7843597) and `tags` is absent.
Why: a typed SDK client cannot deserialize a date-time member from a number (parse error or a nonsense instant), and code that reads response.tags without a null guard - legitimate, since AWS always sends the map - NPEs/KeyErrors on a layout with no tags.
Fix: the service's response serializer for timestamp members (date-time formatting) and the layout response builder (emit `tags` as an empty map rather than skipping it).
Traces: connectcases-case-rule-and-layout-lifecycle#task#11; cluster also holds #task#13, #task#15, connectcases-layout-lifecycle#task#6 and #task#9.

=== Cause 2 of 7: GetCase omits `tags` (fixture connectcases-case-field-values-round-trip-and-update, step task#8) ===
AWS: Smithy: GetCase = POST /domains/{domainId}/cases/{caseId}; GetCaseResponse members fields (required), templateId (required), nextToken, tags (Tags, sparse map). Recorded 200 carries tags = {} for an untagged case.
Emulator: 200 with the `tags` key absent.
Why: as (b) above - absent vs empty map is a different document, and tag-reading clients break on a case that simply has no tags.
Fix: GetCase's response builder.
Traces: connectcases-case-field-values-round-trip-and-update#task#8; cluster also holds #task#10, connectcases-template-and-case-lifecycle#task#6 and #task#8.

=== Cause 3 of 7: GetDomain - epoch createdTime, no lastModifiedTime, no tags (fixture connectcases-domain-lifecycle-and-tags, step task#2) ===
AWS: Smithy: GetDomain = POST /domains/{domainId}; GetDomainResponse.createdTime is REQUIRED with timestampFormat date-time, .tags is a sparse map. A `lastModifiedTime` member is not in the on-disk model for this shape but IS present in the recorded response (recorded behavior only) - AWS returns it as a date-time string alongside createdTime.
Emulator: createdTime is an epoch number, lastModifiedTime is absent entirely, tags is absent.
Why: the domain's creation instant is the one field provisioning tooling records; a number breaks typed clients, and a missing lastModifiedTime makes change detection impossible.
Fix: GetDomain's response builder (add lastModifiedTime, emit tags as {}) plus the shared timestamp serializer from Cause 1.
Traces: connectcases-domain-lifecycle-and-tags#task#2.

=== Cause 4 of 7: BatchGetField field entries - epoch timestamps, no tags (fixture connectcases-field-and-options-lifecycle, step task#10) ===
AWS: Smithy: BatchGetField = POST /domains/{domainId}/fields-batch; each entry is the field detail shape whose createdTime/lastModifiedTime carry timestampFormat date-time and whose tags is a sparse map (cf. GetFieldResponse). Recorded entries carry date-time strings and tags = {}.
Emulator: fields[i].createdTime / .lastModifiedTime are epoch numbers; fields[i].tags absent.
Why: same as Cause 1, on the batch read path clients use to hydrate a domain's field model.
Fix: the field-detail serializer used by BatchGetField (shares the timestamp and tags defects).
Traces: connectcases-field-and-options-lifecycle#task#10.

=== Cause 5 of 7: BatchGetField on the field-lifecycle fixture - same timestamp/tags defects (fixture connectcases-field-lifecycle-and-batch-get, step task#11) ===
AWS: as Cause 4: fields[i].createdTime/.lastModifiedTime as date-time strings, fields[i].tags present as {}.
Emulator: epoch numbers and no tags key. This step ALSO shows the field's `description` still holding its pre-update value; that stale-state half is recorded as its own cause in the companion issue 'connectcases field state: System field seed and UpdateField mutations missing' - here the requirement is the serialization of the entry.
Why: same as Cause 4.
Fix: same field-detail serializer.
Traces: connectcases-field-lifecycle-and-batch-get#task#11.

=== Cause 6 of 7: GetTemplate drops five members and mis-serializes timestamps (fixture connectcases-template-lifecycle, step task#5) ===
AWS: Smithy: GetTemplate = POST /domains/{domainId}/templates/{templateId}; GetTemplateResponse models createdTime/lastModifiedTime (timestampFormat date-time), layoutConfiguration (LayoutConfiguration), rules (TemplateCaseRuleList, @length max 50), tagPropagationConfigurations (TagPropagationConfigurationList, @length max 1) and tags (sparse map). Recorded 200 for a plain template carries: both timestamps as date-time strings; tags = {}; rules = []; tagPropagationConfigurations = []; layoutConfiguration PRESENT as a struct with its members explicitly null ({defaultLayout: null, defaultLayoutName: null}); and defaultValues = [] (not in the on-disk model - recorded behavior only).
Emulator: timestamps are epoch numbers and ALL of tags, rules, tagPropagationConfigurations, layoutConfiguration and defaultValues are omitted.
Why: a client reading template.rules or template.layoutConfiguration.defaultLayout - both always present on AWS - hits missing-key/None errors, so template inspection and layout resolution fail against the emulator on templates that work on AWS.
Fix: GetTemplate's response builder (emit the empty collections and the explicit-null layoutConfiguration struct) plus the shared timestamp serializer.
Traces: connectcases-template-lifecycle#task#5; cluster also holds #task#8.

=== Cause 7 of 7: ListTemplates summaries omit the template identifier, its ARN and tagPropagationConfigurations (fixture connectcases-template-lifecycle, step task#6) ===
AWS: Smithy: ListTemplates = POST /domains/{domainId}/templates-list, returning TemplateSummary entries whose identifier and ARN members are REQUIRED (model names them templateId / templateArn, @length 1-500) plus tagPropagationConfigurations (@length max 1). The recorded summary carries the identifier and ARN keys (recorded as `id` and `arn`) and tagPropagationConfigurations = []; the ARN has the form arn:aws:cases:<region>:<account>:domain/<domainId>/template/<templateId>.
Emulator: the summary entry for the created template is missing the identifier, missing the ARN and missing tagPropagationConfigurations (the diff reports only missing keys, no extras, so the emulator emits neither naming).
Why: a template cannot be used after being listed - a client that lists templates and then calls GetTemplate/CreateCase with the id has no id to pass, so list-then-use flows are broken outright. Emit the identifier/ARN under the key names the recording shows for this operation.
Fix: ListTemplates' summary projection (populate identifier + ARN from stored state, emit tagPropagationConfigurations as []).
Traces: connectcases-template-lifecycle#task#6.

## Evidence
Replay summary: simparity-wire-aws-v1, emulator_up=true, 182 steps, 117 passed, 56 mismatched, score 64.3%, 22 clusters (all connectcases). All seven clusters below are verdict=mismatch on expected 200 / actual 200, with empty stub_path and empty owned_by. '<shaped-not-asserted>' in the expected column means the fixture asserts the member's SHAPE (a date-time string) rather than its per-run value, so the failure is the JSON type, not the instant.

Cause 1 - connectcases-case-rule-and-layout-lifecycle#task#11, GetLayout:
  $.createdTime: expected '<shaped-not-asserted>' (date-time string), got 1794247493.7843597
  $.lastModifiedTime: expected '<shaped-not-asserted>', got 1794247493.7843597
  $.tags: missing (expected {}, got None)
  affected: #task#13, #task#15, connectcases-layout-lifecycle#task#6, connectcases-layout-lifecycle#task#9

Cause 2 - connectcases-case-field-values-round-trip-and-update#task#8, GetCase:
  $.tags: missing (expected {}, got None)
  affected: #task#10, connectcases-template-and-case-lifecycle#task#6, connectcases-template-and-case-lifecycle#task#8

Cause 3 - connectcases-domain-lifecycle-and-tags#task#2, GetDomain:
  $.createdTime: expected '<shaped-not-asserted>', got 1794247496.243671
  $.lastModifiedTime: missing (expected '<shaped-not-asserted>', got None)
  $.tags: missing (expected {}, got None)

Cause 4 - connectcases-field-and-options-lifecycle#task#10, BatchGetField:
  $.fields[0].createdTime: expected '<shaped-not-asserted>', got 1794247499.5815444
  $.fields[0].lastModifiedTime: expected '<shaped-not-asserted>', got 1794247499.612217
  $.fields[0].tags: missing (expected {}, got None)

Cause 5 - connectcases-field-lifecycle-and-batch-get#task#11, BatchGetField:
  $.fields[0].createdTime: expected '<shaped-not-asserted>', got 1794247497.9042847
  $.fields[0].lastModifiedTime: expected '<shaped-not-asserted>', got 1794247497.9464273
  $.fields[0].tags: missing (expected {}, got None)
  (same step also shows $.fields[0].description stale - filed as a cause of the companion field-state issue)

Cause 6 - connectcases-template-lifecycle#task#5, GetTemplate:
  $.createdTime / $.lastModifiedTime: expected '<shaped-not-asserted>', got 1794247498.752113
  $.defaultValues: missing (expected [])
  $.layoutConfiguration: missing (expected {'defaultLayout': None, 'defaultLayoutName': None})
  $.rules: missing (expected [])
  $.tagPropagationConfigurations: missing (expected [])
  $.tags: missing (expected {})
  affected: connectcases-template-lifecycle#task#8

Cause 7 - connectcases-template-lifecycle#task#6, ListTemplates (entry selected by name):
  $.templates[name=...].arn: missing (expected an arn of the form arn:aws:cases:<region>:<account>:domain/<domainId>/template/<templateId>)
  $.templates[name=...].id: missing (expected the template identifier)
  $.templates[name=...].tagPropagationConfigurations: missing (expected [])
  (the recorded arn/id values are per-run; the requirement is that the identifier and ARN are present and self-consistent with the created template)

Smithy (services/service-com-amazonaws-connectcases-amazonconnectcases/smithy-model/com-amazonaws-connectcases-amazonconnectcases.json), protocol restJson1: GetLayoutResponse.createdTime/lastModifiedTime timestampFormat date-time; GetDomainResponse.createdTime required + date-time; GetTemplateResponse.createdTime/lastModifiedTime date-time, rules @length max 50, tagPropagationConfigurations @length max 1, layoutConfiguration -> LayoutConfiguration; GetFieldResponse.createdTime/lastModifiedTime date-time; tags -> Tags (sparse map) on GetCaseResponse/GetDomainResponse/GetLayoutResponse/GetTemplateResponse/GetFieldResponse; TemplateSummary identifier + ARN members required.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the connectcases emulator and read the seven clusters named under Traces. Manually, inspecting the RAW JSON bodies (not an SDK's parsed view):
1. CreateDomain(name=<unique>); poll GetDomain until Active. GetDomain(domainId) -> createdTime must be an ISO-8601 date-time STRING, lastModifiedTime must be present, tags must be {} (Cause 3).
2. CreateField(domainId, name=<unique>, type='Text') -> fieldId. BatchGetField(domainId, fields=[{id:fieldId}]) -> fields[0].createdTime/.lastModifiedTime must be date-time strings and fields[0].tags must be {} (Causes 4 and 5).
3. CreateLayout(domainId, name=<unique>, content=<basic sections>) -> layoutId. GetLayout(domainId, layoutId) -> both timestamps as date-time strings, tags = {} (Cause 1).
4. CreateTemplate(domainId, name=<unique>, status='Active') -> templateId. GetTemplate(domainId, templateId) -> timestamps as date-time strings and the keys tags {}, rules [], tagPropagationConfigurations [], defaultValues [] and layoutConfiguration {defaultLayout:null, defaultLayoutName:null} all PRESENT (Cause 6).
5. ListTemplates(domainId) -> the entry for that template must carry its identifier, its ARN (arn:aws:cases:<region>:<account>:domain/<domainId>/template/<templateId>) and tagPropagationConfigurations [] (Cause 7).
6. CreateCase(domainId, templateId, fields=[{id:'title', value:{stringValue:'x'}}]) -> caseId; GetCase(domainId, caseId, fields=[{id:'title'}]) -> tags must be {} (Cause 2).
