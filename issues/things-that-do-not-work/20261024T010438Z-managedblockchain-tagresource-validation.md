Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# TagResource stores reserved `aws:`-prefixed tag keys instead of refusing them

_Filed: 2026-10-24T01:04:38Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `aws:` tag-key prefix is reserved for AWS use: Managed Blockchain (like every tag-aware AWS service) refuses a user request that creates or overwrites a key beginning with `aws:`, with InvalidRequestException (400), and leaves the resource's tag set unchanged.

Expected: TagResource(ResourceArn=<a network/member/node/proposal/accessor ARN>, Tags={'aws:foo': 'bar'}) → 400 InvalidRequestException, and a subsequent ListTagsForResource shows no `aws:foo` key. Actual: the handler validates only the ARN shape and the resulting tag count, then inserts every key/value pair verbatim, so the call answers 200 and ListTagsForResource reports a reserved-namespace tag that the real service never lets a caller create (and which UntagResource would also not let a caller delete). The same unchecked path is used by the `Tags` member of CreateNetwork, CreateMember (MemberConfiguration.Tags), CreateNode, CreateProposal and CreateAccessor, all of which funnel through `crate::state::map_in` with no key inspection.

## Evidence

Spec/well-known behavior: `com.amazonaws.managedblockchain#TagKey` carries only @length(1,128) — no pattern — so the reserved-prefix rule is service behavior a handler must enforce; the Managed Blockchain tagging documentation referenced from `TagResource` states the `aws:` prefix is reserved for AWS use and cannot be used for user tag keys. TagResource models InvalidRequestException (400).
Handler: services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/tag_resource.rs lines ~166-213 — ARN-kind check, then a MAX_TAGS_PER_RESOURCE count check, then `for (key, value) in input.tags { tags.insert(key, value); }`; no key is examined. Creates: src/ops/create_*.rs all call `crate::state::map_in(input.tags)` (state.rs ~2002) which only converts the map.
Tests: services/.../tests/all/resource_tags.rs covers the 50-tag cap, overwrite semantics and foreign ARNs, but contains no `aws:`-prefix case (grep for 'aws:' finds only ARN literals).

## Steps to Reproduce

1. Create any taggable resource, e.g. acc = client.create_accessor(ClientRequestToken='t1', AccessorType='BILLING_TOKEN', NetworkType='ETHEREUM_MAINNET'), and form its ARN arn:aws:managedblockchain:<region>:<account>:accessors/<AccessorId> (or read it from get_accessor).
2. Call client.tag_resource(ResourceArn=<that ARN>, Tags={'aws:reserved': 'x'}). Observe HTTP 200; expected 400 InvalidRequestException for the reserved `aws:` prefix.
3. Call client.list_tags_for_resource(ResourceArn=<that ARN>). Observe the stored key 'aws:reserved'; expected the tag set to be unchanged.
4. Equivalently, pass Tags={'aws:reserved': 'x'} to create_accessor / create_network and observe the create succeeds with the reserved key persisted.
