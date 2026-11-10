Issue-Source: trace-sweep-divergence
Issue-Trace: connectcases-template-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcases-template-lifecycle
Issue-Shape-Id: com.amazonaws.connectcases#AmazonConnectCases
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-ce446451"].arn
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-ce446451"].id
Issue-Key: connectcases|ListTemplates|2xx->2xx|$.templates[name="sim2real-probe-cc-template-ce446451"].tagPropagationConfigurations
Issue-Commit: 7ddd296fa3724341be1a6933a8dc37e7146d5c05

# connectcases ListTemplates element lacks id arn and empty tagPropagationConfigurations list

> _Note: this issue was identified against the code base at commit `7ddd296fa3724341be1a6933a8dc37e7146d5c05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTemplates

## Description
Cause 1 of 1: the TemplateSummary elements ListTemplates serializes omit three members that are present in the recorded AWS 200 body, so a template the emulator lists cannot be resolved to an id/ARN or tag-inspected the way it can on AWS.

AWS: Smithy ListTemplates (POST /domains/{domainId}/templates-list, 200) returns ListTemplatesResponse.templates (TemplateSummaryList, @required, @length max 100) plus nextToken. Smithy com.amazonaws.connectcases#TemplateSummary declares templateId (@required, @length 1..500), templateArn (@required, @length 1..500), name (@required, @length 1..100, pattern ^.*[\S]$), status (@required) and tagPropagationConfigurations (TagPropagationConfigurationList, @length max 1) — so every listed summary must carry the template's identity, and where the service tracks tag propagation, that list. For the two members `id` and `arn` the model says nothing: recorded behavior only — the recorded 200 body carries, on each `templates` element, an `id` alias equal to that template's template id (the trailing `template/<templateId>` segment of templateArn) and an `arn` alias equal to that template's full ARN, of the form arn:aws:cases:<region>:<account>:domain/<domainId>/template/<templateId>. The recording also shows tagPropagationConfigurations serialized as an empty list [] for a template created with no tag propagation, i.e. AWS emits the empty list rather than omitting the member. No error is involved; the recorded status is 200 and matches.

Emulator: on the same 200 response all three members are absent from the summary element. Replay reports, with expected and actual status both 200: `$.templates[name="<templateName>"].arn: missing (expected the template ARN, got None)`, `$.templates[name="<templateName>"].id: missing (expected the template id, got None)` and `$.templates[name="<templateName>"].tagPropagationConfigurations: missing (expected [], got None)`. Every other member of the element, and the status, matched — this is a response-projection gap only, not a state, routing or validation fault.

Why: a client that reads `id` or `arn` off a ListTemplates element — the shape real AWS responses have, which console-style and SDK-adjacent code keys template lookups on — gets null and cannot map a listed template to an id or ARN without an extra GetTemplate round trip, so list-then-fetch and pagination loops break. A client that iterates tagPropagationConfigurations gets null instead of an empty list and must special-case the absent member; that is the same null-vs-empty hazard the open issue "connectcases responses serialize timestamps as epoch numbers and drop empty members" (20261109T181134Z-connectcases-responses-serialize-timestamps-as-epoch-numbers-and.md) records for other operations, and this replay does not report the cluster as owned by it. Two open issues describe this same ListTemplates fault — "connectcases ListTemplates summaries omit the template id/arn aliases and tagPropagationConfigurations" (20261109T223410Z-connectcases-listtemplates-summaries-omit-the-template-id-arn.md) and "connectcases ListTemplates summary projection drops id, arn and tagPropagationConfigurations" (20261110T113333Z-connectcases-listtemplates-summary-projection-drops-id-arn-and.md) — but both pin their recorded replay keys to a per-run template name from an earlier recording, and the current replay reports this cluster unowned, so it is re-filed here with the key the current corpus produces rather than left unexercised.

Fix: the ListTemplates response serializer / TemplateSummary projection in the connectcases service crate — the code that maps a stored template into a summary element. It must serialize tagPropagationConfigurations as [] when the template has no tag propagation configured (never omit the member) and additionally emit the `id` and `arn` aliases alongside templateId/templateArn, derived from the same stored template identity and ARN so all four agree. GetTemplate's serializer shares the same stored template and must stay consistent with it.

Traces: connectcases-template-lifecycle#task#6 (ListTemplates, expected 200 / actual 200, keys $.templates[...].arn, $.templates[...].id, $.templates[...].tagPropagationConfigurations).

## Evidence
Fixture connectcases-template-lifecycle, step task#6, operation ListTemplates, service connectcases.
Verdict: mismatch. Expected status 200, actual status 200 — the divergence is entirely in the body.
Replay detail, verbatim:
  $.templates[name="sim2real-probe-cc-template-ce446451"].arn: missing (expected 'arn:aws:cases:us-west-2:000000000000:domain/fbb53616-4183-45ff-9b25-effbe489c2e2/template/4dabb402-b68b-4b5f-98cf-eabb37903e78', got None)
  $.templates[name="sim2real-probe-cc-template-ce446451"].id: missing (expected '4dabb402-b68b-4b5f-98cf-eabb37903e78', got None)
  $.templates[name="sim2real-probe-cc-template-ce446451"].tagPropagationConfigurations: missing (expected [], got None)
Divergence key: service=connectcases|ListTemplates|status=200->200|keys=$.templates[name="..."].arn,$.templates[name="..."].id,$.templates[name="..."].tagPropagationConfigurations (affected_count 1, owned_by empty, stub_path empty).
The template name, domain id, template UUID and the literal ARN above are per-run values of this one recording and are NOT the requirement. The requirement is that each `templates` element carry an `id` equal to that template's id, an `arn` equal to that template's ARN (arn:aws:cases:<region>:<account>:domain/<domainId>/template/<templateId>), and `tagPropagationConfigurations` serialized as [] when the template has none.
Model basis: Smithy com.amazonaws.connectcases#TemplateSummary — templateId/templateArn/name/status @required, tagPropagationConfigurations (TagPropagationConfigurationList, @length max 1); the `id` and `arn` aliases are recorded behavior only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1) and inspect fixture connectcases-template-lifecycle, step task#6.
3. Equivalent by hand: CreateDomain; CreateTemplate in that domain with no tagPropagationConfigurations; then POST /domains/{domainId}/templates-list (ListTemplates) for that domain.
4. Inspect the 200 body: each element of `templates` lacks `id`, `arn` and `tagPropagationConfigurations`, whereas AWS returns `id` (the template id), `arn` (the template ARN) and `tagPropagationConfigurations: []`.
5. Expected: every summary element carries templateId, templateArn, name, status, the `id`/`arn` aliases agreeing with templateId/templateArn, and tagPropagationConfigurations as [] when empty.
