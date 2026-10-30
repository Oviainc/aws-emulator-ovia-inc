Issue-Source: trace-sweep-divergence
Issue-Trace: swf-domain-lifecycle-and-tags
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: swf-domain-lifecycle-and-tags swf-domain-tag-lifecycle
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[key="team"].__type
Issue-Key: swf|ListTagsForResource|2xx->2xx|$.tags[key="owner"].__type
Issue-Related: 20261029T230557Z-swf-workflow-history-visibility-and-tag-responses-omit.md
Issue-Commit: 29f1e00a7002e451c541eb91ee53a740b213bc0d

# swf ListTagsForResource omits the ResourceTag __type discriminator on every returned tag

> _Note: this issue was identified against the code base at commit `29f1e00a7002e451c541eb91ee53a740b213bc0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
This issue bundles TWO distinct wire-fixture divergence clusters of the `swf` service, replayed from simparity-wire-aws-v1. Both are the SAME underlying emulator defect — the ListTagsForResource response serializer does not emit the awsJson type discriminator on the elements of `tags` — but they are two separate, currently UNOWNED replay clusters (distinguished by which tag keys the response carries), so a fixer must confirm BOTH before submitting. Each cause is numbered below and carries its own fixture and step.

Note on overlap: the open issue 20261029T230557Z-swf-workflow-history-visibility-and-tag-responses-omit.md describes the same class of defect as its "Cause 7 of 10", but its recorded replay keys cover only the discriminator on the tag keys env/stage/project/extra/a/b/c. The two clusters below are keyed on `$.tags[key="team"].__type` and `$.tags[key="owner"].__type`, which that issue does not carry, so its replay gate never exercises them and the replay report leaves them unowned. They are filed here so the fix is gated on them too.

=== Cause 1 of 2: ListTagsForResource tag elements are serialized without __type (two-tag response: env + team) ===
AWS: Smithy: ListTagsForResource -> ListTagsForResourceOutput.tags targets com.amazonaws.swf#ResourceTagList, a list of com.amazonaws.swf#ResourceTag {key (required, @length 1..128), value (@length 0..256)}; errors LimitExceededFault / OperationNotPermittedFault / UnknownResourceFault (all httpError 400). The service speaks awsJson, and the recordings show every element of the returned list carries, in addition to key and value, the JSON type discriminator member `"__type": "com.amazonaws.swf.base.model#ResourceTag"`. That discriminator is FIXED text naming the element's shape — it is identical for every tag of every response and does not vary with the tag's key, its value, the resource, or the account; only the key/value pairs beside it come from the request and the stored tag state.
Emulator: each element is serialized as {key, value} only; `__type` is absent from every element. Replayed response was HTTP 200 vs an expected HTTP 200, so the divergence is purely in the body shape.
Why: a client that dispatches on the awsJson `__type` discriminator to resolve the element's shape (shape-driven deserializers, recording proxies, and tooling that compares responses against recorded AWS traffic) sees a list element in a form AWS never emits, and cannot resolve the member's type. The tag key/value data is present, so the failure is silent until the deserializer or comparison runs.
Fix: the ListTagsForResource response serializer in the swf service crate's ListTagsForResource handler (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/list_tags_for_resource.rs) must emit the ResourceTag `__type` discriminator on each element of `tags`. This is a serializer concern only — no state or validation change is needed.
Traces: swf-domain-lifecycle-and-tags#task#5 (also covers swf-domain-tag-lifecycle#task#3).

=== Cause 2 of 2: same omission on a four-tag response (env + team + stage + owner) ===
AWS: same model and same recorded behavior as Cause 1 — every ResourceTag element of ListTagsForResourceOutput.tags carries `"__type": "com.amazonaws.swf.base.model#ResourceTag"`, regardless of how many tags the resource has or which keys they use.
Emulator: all four elements come back as {key, value} with no `__type`, HTTP 200 vs expected HTTP 200.
Why: identical to Cause 1 — the discriminator is missing for every key, so the defect scales with the tag set and is not limited to the key names already recorded in the pre-existing issue.
Fix: same component as Cause 1 — the ListTagsForResource response serializer must emit the discriminator per element, so a single serializer change closes both clusters; the second cluster exists so the replay gate exercises the additional keys.
Traces: swf-domain-tag-lifecycle#task#5 (also covers swf-domain-tag-lifecycle#task#7).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, 409 steps total, 303 passed, 90 mismatched, 4 errored, 12 blocked, score 74.1%, 35 divergence clusters. All 35 clusters belong to service `swf`; 33 carry an `owned_by` pointing at an already-open issue and are skipped. The two clusters below carry owned_by=[] and stub_path="", i.e. nothing already filed carries their replay key.

Cause 1 — fixture swf-domain-lifecycle-and-tags, step task#5, ListTagsForResource, expected HTTP 200 / actual HTTP 200,
divergence_key = service=swf|ListTagsForResource|status=200->200|keys=$.tags[key="env"].__type,$.tags[key="team"].__type:
  $.tags[key="env"].__type: missing (expected 'com.amazonaws.swf.base.model#ResourceTag', got None)
  $.tags[key="team"].__type: missing (expected 'com.amazonaws.swf.base.model#ResourceTag', got None)
Cluster members: swf-domain-lifecycle-and-tags#task#5, swf-domain-tag-lifecycle#task#3.

Cause 2 — fixture swf-domain-tag-lifecycle, step task#5, ListTagsForResource, expected HTTP 200 / actual HTTP 200,
divergence_key = service=swf|ListTagsForResource|status=200->200|keys=$.tags[key="env"].__type,$.tags[key="owner"].__type,$.tags[key="stage"].__type,$.tags[key="team"].__type:
  $.tags[key="owner"].__type: missing (expected 'com.amazonaws.swf.base.model#ResourceTag', got None)
  $.tags[key="team"].__type: missing (expected 'com.amazonaws.swf.base.model#ResourceTag', got None)
  $.tags[key="stage"].__type: missing (expected 'com.amazonaws.swf.base.model#ResourceTag', got None)
  $.tags[key="env"].__type: missing (expected 'com.amazonaws.swf.base.model#ResourceTag', got None)
Cluster members: swf-domain-tag-lifecycle#task#5, swf-domain-tag-lifecycle#task#7.

In both clusters the `key` and `value` members of every element match the recording exactly; the only divergence is the absent `__type` string, whose expected value is the same fixed shape id in every element.

## Steps to Reproduce
Cause 1 (two-tag response):
1. RegisterDomain for a fresh domain, then TagResource / register it with tags env=<v> and team=<v> (any values).
2. Resolve the domain's ARN (DescribeDomain) and call ListTagsForResource(resourceArn=<domain arn>).
3. AWS: HTTP 200 and each element of `tags` is {"__type": "com.amazonaws.swf.base.model#ResourceTag", "key": ..., "value": ...}. Emulator: HTTP 200 and each element is {"key": ..., "value": ...} with no "__type".
Replay: swf-domain-lifecycle-and-tags#task#5.

Cause 2 (four-tag response):
1. RegisterDomain; TagResource(arn, tags env, stage, team, owner).
2. ListTagsForResource(arn).
3. AWS: all four elements carry the "__type": "com.amazonaws.swf.base.model#ResourceTag" discriminator. Emulator: none of the four does.
Replay: swf-domain-tag-lifecycle#task#5.

In both cases assert on the presence of the `__type` member and on its fixed value; the tag keys and values themselves come from the TagResource request and already match.
