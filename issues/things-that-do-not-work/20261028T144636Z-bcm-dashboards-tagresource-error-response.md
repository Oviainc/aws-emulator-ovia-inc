Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 15aaa5471fc3f7278352228b97d2216e65bea584

# Reserved-tag-key ValidationException echoes every offending key, so message can run far past its modelled 1024-character bound

_Filed: 2026-10-28T14:46:36Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `15aaa5471fc3f7278352228b97d2216e65bea584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The reserved-`aws:`-namespace refusal every tagging path of this service shares (`crate::state::check_tag_keys_not_reserved`) builds its `ValidationException` message by joining EVERY offending tag key into one sentence. The model bounds that member: `ValidationException$message` targets `GenericString`, whose `@length` is `min 0, max 1024`. One `TagResource` request may legally carry 200 tags (`TagResourceRequest$resourceTags` -> `ResourceTagList`, `@length 0..200`) whose keys are up to 128 characters each (`ResourceTagKey`, `@length 1..128`, `@pattern ^[\S\s]*$` — so `aws:`-prefixed keys pass boundary validation and reach the handler). With all 200 keys reserved the handler emits a `message` of roughly 26,000 characters: the fixed sentence plus 200 keys joined with `", "`. The service therefore puts an error body on the wire that violates the bound its own model declares for that member, and a client that allocates or stores the message against the modelled 1024 maximum sees a value it was told could not occur.

The defect is in the shared helper, so the same over-long body comes back from `UntagResource` (`resourceTagKeys`, also `@length 0..200`) and from the two create paths that tag on create; `TagResource` is named here because the crate's own handlers document it as the path that owns this rule (`create_dashboard.rs:203-206`: "the reserved `aws:` namespace is refused here exactly as `TagResource` refuses it"), and one bound in the helper fixes every caller. Nothing in the crate justifies the unbounded echo — the sibling refusal builder reasons explicitly about keeping the echoed text inside the member's modelled maximum (`state.rs:1483-1486`).

## Evidence

Model (smithy_query query="closure" operation="CreateDashboard"): `"ValidationException": {... "members": {"message": {"target": "GenericString", "required": true, "length": {"min": 0, "max": 1024}, "pattern": "^[\\S\\s]*$"}}}`; `"ResourceTagList": {... "length": {"min": 0, "max": 200}, "member": "ResourceTag"}`; `"ResourceTagKey": {... "length": {"min": 1, "max": 128}, "pattern": "^[\\S\\s]*$"}`.

Code that can exceed it — services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/state.rs:1265-1279:
```
    let mut reserved: Vec<&str> = keys
        .into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .collect();
...
    Err(format!(
        "{member}: the '{RESERVED_TAG_KEY_PREFIX}' prefix is reserved for tags Amazon Web \
         Services generates and cannot be set or removed by a caller, so these keys are not \
         allowed: {}",
        reserved.join(", ")
    ))
```
Caller — src/ops/tag_resource.rs:177-191 lifts that string straight into the modelled error: `return Err(TagResourceErrorModeled::ValidationException(ValidationException { message }).into());`

The crate's own counter-example, which shows the bound is otherwise respected on purpose — src/state.rs:1483-1486: "// The value the caller submitted, which a constraint clause echoes. Bounded // by the member's own `@length` maximum (50 for a name or a title, 200 for a // description), which the boundary already enforced."

Test coverage: tests/all/reserved_tag_keys.rs only ever sends ONE reserved key per request (`reserved_keys()` rows, lines 49-70, the longest being 128 characters) and asserts `message.contains(member) && message.contains(offending_key)` (lines 110-120), so the multi-key case that overflows the bound is untested and a bounded message still satisfies every assertion.

## Steps to Reproduce

1. CreateDashboard with name "Bound Probe" and widgets [] — keep the returned arn.
2. Call TagResource with that resourceArn and resourceTags = 200 entries whose keys are `aws:` + 124 'k' characters (each key 128 characters, the modelled maximum) and any values.
3. Expect HTTP 400 ValidationException whose `message` honours the modelled `ValidationException$message` @length maximum of 1024 characters.
4. Observe a `message` of roughly 26,000 characters: the fixed sentence followed by all 200 keys joined with ", ".
5. Repeat with UntagResource and resourceTagKeys = the same 200 keys — the same helper produces the same over-long body.
