Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# UntagResource accepts reserved aws:-prefixed tag keys instead of refusing them

_Filed: 2026-10-24T17:05:59Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` treats every supplied `TagKeys` entry alike: a key the resource does not carry is a silent no-op success. That is right for an ordinary key, but a key in the reserved `aws:` namespace is not an ordinary key — AWS refuses an attempt to delete a tag with that prefix rather than reporting success.

Expected: `DELETE /tags/{ResourceArn}?tagKeys=aws:createdBy` is refused with 400 `ValidationException` naming the reserved prefix, and the stored tag set is untouched.

Actual: `tag_keys_rejection()` only bounds the list length (`> MAX_TAGS`); the handler then runs `manager.tags.remove(key)` for each key and answers 200 `UntagResourceOutput` regardless, so a caller asking to strip an AWS-managed tag is told it succeeded. (Combined with the sibling CreateConfigurationManager gap, where `aws:`-prefixed keys can be stored, this path would actually delete an AWS-reserved tag.)

## Evidence

Spec/well-known grounding: `UntagResourceInput$TagKeys` targets `TagKeys` (`list<String>`) with no modelled pattern or length, so the boundary validator admits any key and the rule has to live in the handler. AWS-wide tagging rule (AWS General Reference, "Tag naming limits and requirements"): the `aws:` prefix is reserved and "you can't edit or delete tag keys or values with this prefix". Repo precedent for exactly this operation shape: `service-com-amazonaws-secretsmanager-secretsmanager/src/ops/untag_resource.rs:196-203` refuses a reserved-prefix key on the untag path.

Handler evidence: `services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/untag_resource.rs` — `tag_keys_rejection()` (lines 142-150) checks only `tag_keys.len() > crate::state::MAX_TAGS`; lines 225-236 remove each named key and always return `Ok(UntagResourceOutput)`.

Test coverage: none. `tests/all/resource_tags.rs` asserts that exactly the named keys are removed, that the call is idempotent, that an unknown ARN is a 404 and that an oversized key list is refused — no case uses an `aws:`-prefixed key.

## Steps to Reproduce

1. Create a configuration manager with tags, e.g. POST /configurationManager with ConfigurationDefinitions=[{"Type":"AWSQuickSetupType-SSMHostMgmt","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2"}}] and Tags={"Owner":"team"}.
2. Call DELETE /tags/<ManagerArn>?tagKeys=aws%3AcreatedBy.
3. Observe 200 with an empty body; the real service refuses the reserved key with 400 ValidationException.
4. GET /configurationManager/<ManagerArn> and confirm the remaining tags are unchanged either way — the divergence is the 200 vs. 400 answer on a reserved key.
