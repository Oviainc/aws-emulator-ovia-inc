Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a85c524bdc735756e5abfd799d7dcf6de3bb06dc

# TagResource stores tag keys with the reserved `aws:` prefix instead of refusing them

_Filed: 2026-10-24T07:51:11Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `a85c524bdc735756e5abfd799d7dcf6de3bb06dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `aws:` tag-key prefix is reserved for AWS: a caller may neither create nor modify such a tag, and a TagResource call carrying one is refused with a 400 validation-class error with none of the batch applied.

- Expected: TagResource(resourceARN=<workspace arn>, tags={"aws:owner": "x"}) is refused 400 (ValidationException-class) and ListTagsForResource shows no `aws:`-prefixed key afterwards; a batch mixing a legal key with a reserved one applies nothing.
- Actual: the handler inserts every supplied key verbatim after only the cumulative 50-tag cap check, answers 200, and ListTagsForResource reports `aws:owner` as a caller-set tag that UntagResource can then delete — state AWS would never have allowed to exist. The same unchecked path applies to the `tags` maps accepted by CreateWorkspace/CreateEntity/CreateComponentType/CreateScene/CreateSyncJob, which store reserved keys just as silently.

## Evidence

Well-established AWS tagging rule (the `aws:` prefix is reserved for AWS use; you cannot edit or delete keys with this prefix, and tagging APIs reject caller-supplied ones). Handler: `services/.../src/ops/tag_resource.rs:177-201` — the only checks are membership (for the cap arithmetic) and `MAX_TAGS_PER_RESOURCE`, then `for (key, value) in input.tags { tags.insert(key, value); }`; no key inspection anywhere. `src/state.rs` `MAX_TAGS_PER_RESOURCE` is the sole tag rule in the state module. Tests: `tests/all/resource_tags.rs` covers the tag roundtrip and the quota refusal; no test asserts a reserved-prefix refusal. Distinct class from the existing open TagResource issue (20261023T123634Z), which concerns the quota refusal's error CODE.

## Steps to Reproduce

1. CreateWorkspace("ws-tags") and read its ARN with GetWorkspace.
2. TagResource(resourceARN=<arn>, tags={"aws:owner": "platform"}). Expected: 400 validation-class refusal, nothing persisted. Actual: 200.
3. ListTagsForResource(resourceARN=<arn>). Expected: no `aws:`-prefixed key. Actual: `aws:owner` is reported.
4. TagResource(resourceARN=<arn>, tags={"team": "iot", "aws:created-by": "me"}). Expected: 400 and neither key applied. Actual: 200 and both keys applied.
