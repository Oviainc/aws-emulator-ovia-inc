Issue-Source: trace-sweep-divergence
Issue-Trace: connectcases-template-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcases-template-lifecycle
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-bdacbacb"].arn
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-bdacbacb"].id
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-bdacbacb"].tagPropagationConfigurations
Issue-Commit: a0fae0073c5a57a2ef84b70eabff7d12afa58f91

# connectcases ListTemplates summaries omit the template id/arn aliases and tagPropagationConfigurations

> _Note: this issue was identified against the code base at commit `a0fae0073c5a57a2ef84b70eabff7d12afa58f91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTemplates

## Description
Cause 1 of 1: ListTemplates returns TemplateSummary entries that are missing three members AWS emits, so a template listed by the emulator cannot be identified or inspected the way it can on AWS.

AWS: Smithy: ListTemplates POST /domains/{domainId}/templates-list -> ListTemplatesResponse.templates (TemplateSummaryList, @required); TemplateSummary members templateId (@required), templateArn (@required), name (@required), status (@required), tagPropagationConfigurations (TagPropagationConfigurationList, @length max 1). The model therefore requires the summary to carry the template's identity and, where the service tracks tag propagation, that list. For the two members `id` and `arn` the model says nothing: recorded behavior only — the recorded 200 body for each element of `templates` additionally carries `id`, holding the same template UUID as `templateArn`'s trailing `template/<uuid>` segment, and `arn`, holding the full template ARN `arn:aws:cases:<region>:<account>:domain/<domainId>/template/<templateId>`. The recording also shows `tagPropagationConfigurations` present as an empty list `[]` for a template created with no tag propagation, i.e. AWS serializes the empty list rather than omitting the member. (The ARN, UUIDs and names in the recording are per-run values that fill that template; only the shape and the member set are the requirement.)

Emulator: for the same 200 response the three members are absent from the summary element: replay reports `$.templates[name="..."].arn: missing (expected the template ARN, got None)`, `$.templates[name="..."].id: missing (expected the template id, got None)`, and `$.templates[name="..."].tagPropagationConfigurations: missing (expected [], got None)`. Status and the remaining members matched (expected 200, actual 200), so this is purely a serialization gap in the summary projection, not a state or routing fault.

Why: a client that reads `id`/`arn` off a ListTemplates element — the shape real AWS responses have, and what SDK-adjacent and console-style code paths key template lookups on — gets null and cannot resolve the listed template to a template id or ARN without a second GetTemplate round trip. A client that iterates `tagPropagationConfigurations` gets null instead of an empty list and must special-case the absent member, which is the same null-vs-empty hazard the open issue "connectcases responses serialize timestamps as epoch numbers and drop empty members" (20261109T181134Z-connectcases-responses-serialize-timestamps-as-epoch-numbers-and.md) describes for other operations; that issue does not cover this step and the replay reports this cluster as unowned, so it is filed separately here.

Fix: the ListTemplates response serializer / summary projection in the connectcases service crate — the code that maps a stored template into a TemplateSummary. It must emit `tagPropagationConfigurations` as an empty list when the template has no tag propagation configured (never omit it), and additionally emit the `id` and `arn` aliases alongside `templateId`/`templateArn`, derived from the same stored template identity and ARN so all four agree.

Traces: connectcases-template-lifecycle#task#6 (ListTemplates, expected 200 / actual 200, keys $.templates[...].arn, $.templates[...].id, $.templates[...].tagPropagationConfigurations).

## Evidence
Fixture connectcases-template-lifecycle, step task#6, operation ListTemplates.
Verdict: mismatch. Expected status 200, actual status 200 (the divergence is in the body).
Replay detail, verbatim:
  $.templates[name="sim2real-probe-cc-template-bdacbacb"].arn: missing (expected 'arn:aws:cases:us-west-2:000000000000:domain/9764d83a-17ed-42d9-b179-65681ed6923a/template/d1ab2270-0236-4b88-9265-29d6ea51ee46', got None)
  $.templates[name="sim2real-probe-cc-template-bdacbacb"].id: missing (expected 'd1ab2270-0236-4b88-9265-29d6ea51ee46', got None)
  $.templates[name="sim2real-probe-cc-template-bdacbacb"].tagPropagationConfigurations: missing (expected [], got None)
The template name, domain id, template UUID and ARN above are per-run values of this recording; the requirement is that each `templates` element carry an `id` equal to the template's id, an `arn` equal to its ARN, and `tagPropagationConfigurations` serialized as [] when empty.
Model basis: Smithy TemplateSummary (com.amazonaws.connectcases#TemplateSummary) declares templateId/templateArn/name/status as @required and tagPropagationConfigurations (@length max 1); `id` and `arn` are recorded behavior only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1); fixture connectcases-template-lifecycle, step task#6.
3. Equivalent by hand: CreateDomain, then CreateTemplate in that domain with no tagPropagationConfigurations, then POST /domains/{domainId}/templates-list (ListTemplates) for the same domain.
4. Inspect the 200 body: each element of `templates` lacks `id`, `arn` and `tagPropagationConfigurations`, whereas the recorded AWS response carries `id` (the template id), `arn` (the template ARN) and `tagPropagationConfigurations: []`.
