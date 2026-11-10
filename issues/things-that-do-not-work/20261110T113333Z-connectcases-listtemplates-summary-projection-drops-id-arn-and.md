Issue-Source: trace-sweep-divergence
Issue-Trace: connectcases-template-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcases-template-lifecycle
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-747bb4bd"].arn
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-747bb4bd"].id
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-747bb4bd"].tagPropagationConfigurations
Issue-Commit: e6de55ab242ddacc2879e4dbb5ab97154006a405

# connectcases ListTemplates summary projection drops id, arn and tagPropagationConfigurations

> _Note: this issue was identified against the code base at commit `e6de55ab242ddacc2879e4dbb5ab97154006a405`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTemplates

## Description
Cause 1 of 1: ListTemplates returns TemplateSummary elements that omit three members AWS emits, so a template the emulator lists cannot be identified or tag-inspected the way it can on AWS.

AWS: Smithy: ListTemplates, POST /domains/{domainId}/templates-list, output ListTemplatesResponse.templates (TemplateSummaryList, @required, @length max 100). Smithy TemplateSummary (com.amazonaws.connectcases#TemplateSummary) declares templateId (@required, @length 1..500), templateArn (@required, @length 1..500), name (@required), status (@required) and tagPropagationConfigurations (TagPropagationConfigurationList, @length max 1). The model therefore requires every listed summary to carry the template's identity and, where the service tracks tag propagation, that list. For the two members `id` and `arn` the model says nothing: recorded behavior only - the recorded 200 body carries, on each element of `templates`, an `id` alias holding the same template id as the trailing `template/<templateId>` segment of `templateArn`, and an `arn` alias holding the full template ARN of the form arn:aws:cases:<region>:<account>:domain/<domainId>/template/<templateId>. The recording also shows tagPropagationConfigurations present as an empty list [] for a template created with no tag propagation, i.e. AWS serializes the empty list rather than omitting the member. No error is involved: the recorded status is 200.

Emulator: for the same 200 response all three members are absent from the summary element. Replay reports, on a step whose expected and actual status are both 200, `$.templates[name="<template name>"].arn: missing (expected the template ARN, got None)`, `$.templates[name="<template name>"].id: missing (expected the template id, got None)` and `$.templates[name="<template name>"].tagPropagationConfigurations: missing (expected [], got None)`. Every other member of the summary and the status matched, so this is a serialization/projection gap only, not a state, routing or validation fault.

Why: a client that reads `id` or `arn` off a ListTemplates element - the shape real AWS responses have, which SDK-adjacent and console-style code keys template lookups on - gets null and cannot resolve a listed template to an id or ARN without an extra GetTemplate round trip, so pagination-and-fetch loops break. A client that iterates `tagPropagationConfigurations` gets null instead of an empty list and must special-case the absent member; that is the same null-vs-empty hazard the open issue "connectcases responses serialize timestamps as epoch numbers and drop empty members" (20261109T181134Z-connectcases-responses-serialize-timestamps-as-epoch-numbers-and.md) describes for other operations, and the replay does not report this cluster as owned by it. An earlier open issue, "connectcases ListTemplates summaries omit the template id/arn aliases and tagPropagationConfigurations" (20261109T223410Z-connectcases-listtemplates-summaries-omit-the-template-id-arn.md), describes the same fault, but its recorded replay key is pinned to a per-run template name and the current replay reports this cluster unowned, so the finding is re-filed here with the key the current corpus produces rather than left uncovered.

Fix: the ListTemplates response serializer / TemplateSummary projection in the connectcases service crate - the code that maps a stored template into a summary element. It must serialize tagPropagationConfigurations as an empty list when the template has no tag propagation configured (never omit the member), and additionally emit the `id` and `arn` aliases next to templateId/templateArn, derived from the same stored template identity and ARN so all four agree. GetTemplate's own serializer shares the same stored template and should stay consistent with it.

Traces: connectcases-template-lifecycle#task#6 (ListTemplates, expected 200 / actual 200, keys $.templates[...].arn, $.templates[...].id, $.templates[...].tagPropagationConfigurations).

## Evidence
Fixture connectcases-template-lifecycle, step task#6, operation ListTemplates, service connectcases.
Verdict: mismatch. Expected status 200, actual status 200 - the divergence is entirely in the body.
Replay detail, verbatim:
  $.templates[name="sim2real-probe-cc-template-747bb4bd"].arn: missing (expected 'arn:aws:cases:us-west-2:000000000000:domain/c1ee846f-88bf-4d9d-b9a0-90afa215784e/template/fad324fc-94ae-47f7-8d71-b99c06a3f5bc', got None)
  $.templates[name="sim2real-probe-cc-template-747bb4bd"].id: missing (expected 'fad324fc-94ae-47f7-8d71-b99c06a3f5bc', got None)
  $.templates[name="sim2real-probe-cc-template-747bb4bd"].tagPropagationConfigurations: missing (expected [], got None)
Divergence key: service=connectcases|ListTemplates|status=200->200|keys=$.templates[name="..."].arn,$.templates[name="..."].id,$.templates[name="..."].tagPropagationConfigurations (affected_count 1, owned_by empty, stub_path empty).
The template name, domain id, template UUID and the ARN above are per-run values of this recording and are NOT the requirement; the requirement is that each `templates` element carry an `id` equal to that template's id, an `arn` equal to that template's ARN (arn:aws:cases:<region>:<account>:domain/<domainId>/template/<templateId>), and `tagPropagationConfigurations` serialized as [] when the template has none.
Model basis: Smithy com.amazonaws.connectcases#TemplateSummary - templateId/templateArn/name/status @required, tagPropagationConfigurations (TagPropagationConfigurationList, @length max 1); `id` and `arn` are recorded behavior only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1) and look at fixture connectcases-template-lifecycle, step task#6.
3. Equivalent by hand: CreateDomain; CreateTemplate in that domain with no tagPropagationConfigurations; then POST /domains/{domainId}/templates-list (ListTemplates) for the same domain.
4. Inspect the 200 body: each element of `templates` lacks `id`, `arn` and `tagPropagationConfigurations`, whereas AWS returns `id` (the template id), `arn` (the template ARN) and `tagPropagationConfigurations: []`.
5. Expected: every summary element carries templateId, templateArn, name, status, the `id`/`arn` aliases agreeing with templateId/templateArn, and tagPropagationConfigurations as [] when empty.
