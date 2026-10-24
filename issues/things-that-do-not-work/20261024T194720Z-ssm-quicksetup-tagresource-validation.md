Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7cd33b8d616838a49131331fab4e1704473d18dc

# TagResource accepts reserved aws:-prefixed tag keys instead of refusing them

_Filed: 2026-10-24T19:47:20Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `7cd33b8d616838a49131331fab4e1704473d18dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` stores whatever `Tags` map the request carries after only counting its entries. A key in the reserved `aws:` namespace is not an ordinary key: AWS refuses an attempt to create or edit a tag whose key begins with `aws:` rather than storing it.

Expected: `PUT /tags/{ResourceArn}` with `Tags={"aws:createdBy":"me"}` is refused with 400 `ValidationException` naming the reserved prefix, and the resource's stored tag set is untouched (no `aws:`-prefixed key becomes visible to a later `ListTagsForResource` / `GetConfigurationManager`).
Actual: `tags_rejection()` checks only `tags.len() > MAX_TAGS`; the handler then merges every supplied pair into `manager.tags`, bumps `LastModifiedAt` and answers 200. `ListTagsForResource` subsequently reports the reserved key as a normal tag, so the emulator lets a caller mint AWS-managed metadata on the configuration manager.

Note this is the one remaining hole in this crate's tagging surface: the sibling gaps on `CreateConfigurationManager` (reserved key in the create's `Tags`) and `UntagResource` (reserved key in `TagKeys`) are already filed; the write path through `TagResource` is not, and it is the path that actually makes the key appear in state.

## Evidence

Spec grounding: `TagResourceInput$Tags` targets `TagsMap`, whose key constraints are only `@length(1,128)` and `@pattern(^[A-Za-z0-9 _=@:.+-/]+$)` (smithy_query shape TagsMap) — the pattern admits `:` and therefore `aws:createdBy`, so the boundary validator cannot refuse it and the rule has to live in the handler. Well-known AWS rule (AWS General Reference, "Tag naming limits and requirements"): the `aws:` prefix is reserved for AWS use and "you can't edit or delete tag keys or values with this prefix". Repo precedent: the already-open ssm-quicksetup issues 20261024T170559Z-ssm-quicksetup-createconfigurationmanager-validation--r2.md and 20261024T170559Z-ssm-quicksetup-untagresource-validation.md apply exactly this rule to the other two tag entry points of this same crate.

Handler evidence: `src/ops/tag_resource.rs` — `tags_rejection()` (lines 143-151) only bounds the map size; lines 226-258 compute the added-key count against `MAX_TAGS` and then `manager.tags.insert(key, value)` for every pair, returning `Ok(TagResourceOutput)`.

Test coverage: none. `tests/all/resource_tags.rs` covers merge semantics (`tag_resource_merges_exactly_the_named_pairs`), idempotency, an unknown ARN 404 and an oversized map (`tag_resource_rejects_an_unservable_tags_map`); a repo-wide grep for an `aws:`-prefixed tag key in this crate's tests returns nothing.

## Steps to Reproduce

1. POST /configurationManager with {"Name":"tagged","ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-SSMHostMgmt","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2"}}]} and keep the returned ManagerArn.
2. PUT /tags/<url-encoded ManagerArn> with body {"Tags":{"aws:createdBy":"someone"}}.
3. Observe 200 with an empty body; the real service refuses the reserved prefix with 400 ValidationException.
4. GET /tags/<url-encoded ManagerArn>: the emulator reports a TagEntry with Key "aws:createdBy", i.e. the reserved key was persisted.
