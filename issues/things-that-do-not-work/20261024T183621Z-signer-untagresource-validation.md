Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# UntagResource accepts tag keys in the reserved aws: namespace instead of refusing them

_Filed: 2026-10-24T18:36:21Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` (DELETE /tags/{resourceArn}?tagKeys=...) takes its keys from `TagKeyList`, whose member targets `TagKey` — modelled `@length(min: 1, max: 128)` and `@pattern("^(?!aws:)[a-zA-Z+-=._:/]+$")`. The negative lookahead means a key inside AWS's reserved `aws:` namespace is not a value the member admits, and AWS refuses such a request with the operation's modelled 400 `BadRequestException` ("The request contains invalid parameters for the ARN or tags").

Expected: `UntagResource` with `tagKeys=["aws:cloudformation:stack-name"]` (alone, or beside a legal key) is refused HTTP 400 `BadRequestException` and removes nothing.

Actual: the handler performs no key check at all. It resolves the profile from the ARN, checks the profile status, then loops `for key in &input.tag_keys { profile.tags.remove(key) }` and answers 200 — the reserved key is silently treated as "a key the profile does not hold", so the caller gets a success where AWS gives a 400.

This is specifically a handler-level gap because the boundary validator cannot compile the `(?!aws:)` lookahead — the sibling `TagResource` handler says so in a comment and judges its request's keys by hand via `crate::state::reserved_tag_key(...)` before writing. `UntagResource` never calls that helper, so the same modelled rule is enforced on one half of the tagging pair and dropped on the other.

## Evidence

Spec: `smithy_query(query="shape", shape="TagKeyList")` → list of `TagKey` with `pattern: "^(?!aws:)[a-zA-Z+-=._:/]+$"`, length 1..128; `UntagResourceRequest$tagKeys` is `@required @httpQuery("tagKeys")` targeting that list; `UntagResource` models `BadRequestException` (HTTP 400). Handler: services/service-com-amazonaws-signer-wallabyservice/src/ops/untag_resource.rs lines ~185-214 — ARN resolution, `accepts_tag_writes()` status gate, then an unconditional `profile.tags.remove(key)` loop; no call to `crate::state::reserved_tag_key` / `reserved_tag_key_message`, which exist in src/state.rs (lines ~597-610) and ARE used by src/ops/tag_resource.rs lines ~197-206. Test coverage: tests/all/resource_tags.rs `a_write_outside_the_modelled_tag_bounds_is_refused_and_stores_nothing` (lines ~781-818) asserts the reserved-prefix refusal only for `tag_resource`; every `untag_resource` call in the suite passes ordinary keys, so nothing asserts the UntagResource leg.

## Steps to Reproduce

1. PUT /signing-profiles/Reserved_Keys with {"platformId": "AWSLambda-SHA384-ECDSA"} (SigV4-signed) and note the returned profile ARN.
2. POST /tags/<arn> with {"tags": {"env": "prod"}} — 200, and GET /tags/<arn> reports the tag.
3. DELETE /tags/<arn>?tagKeys=aws%3Acloudformation%3Astack-name — observe HTTP 200 with an empty body instead of HTTP 400 BadRequestException naming the reserved key.
4. DELETE /tags/<arn>?tagKeys=env&tagKeys=aws%3AcreatedBy — observe HTTP 200 and that `env` was REMOVED, i.e. a request carrying a reserved key still took effect; AWS refuses the whole call and leaves every tag in place (the same all-or-nothing rule TagResource already implements here).
5. For contrast: POST /tags/<arn> with {"tags": {"aws:createdBy": "someone"}} is correctly refused 400 BadRequestException, showing the rule is enforced on only one side of the tagging pair.
