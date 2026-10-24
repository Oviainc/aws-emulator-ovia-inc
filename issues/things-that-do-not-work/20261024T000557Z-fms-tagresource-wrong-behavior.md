Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# TagResource can only tag policies: the ARN of an existing resource set, applications list or protocols list answers ResourceNotFoundException

_Filed: 2026-10-24T00:05:56Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Firewall Manager's tagging APIs apply to its taggable resources — policies, applications lists, protocols lists and resource sets (each of which has its own ARN and accepts a TagList on its Put* operation). The handler resolves the request's ResourceArn only through `state::policy_id_for_arn`, which scans `store.policies` alone, so a well-formed ARN naming an existing resource set (arn:aws:fms:<region>:<account>:resource-set/<Id>), applications list (applications-list/<ListId>) or protocols list (protocols-list/<ListId>) is treated as naming nothing and refused with 400 ResourceNotFoundException. Expected: the tags are merged into that resource's stored tag set and are visible to a later ListTagsForResource. Actual: the write is refused even though the emulator itself minted that ARN and already holds a tag set for it in `tags_by_arn`.

## Evidence

Model: ListTagsForResource/TagResource/UntagResource take a generic `ResourceArn` (com.amazonaws.fms#ResourceArn) and the model gives PutResourceSet, PutAppsList, PutProtocolsList and PutPolicy each a `TagList` member — so all four resource kinds are taggable. Handler: `src/ops/tag_resource.rs` lines 206-217 resolve the ARN only via `store.policy_id_for_arn(...)` then `store.policies.get(...)`; `src/state.rs` `policy_id_for_arn` (line 475) matches policy records only, and its own doc comment concedes it resolves nothing else. State for the other kinds exists: `put_resource_set.rs` lines 314-316 insert `record.arn -> tags` into `tags_by_arn`, and `ResourceSetRecord.tags` is populated from the request's TagList. Test coverage: `tests/all/tagging_and_managing_admins.rs` only ever tags a policy ARN; its `unknown_arns()` applications-list case names a list that was never created, so no test asserts not-found for an ARN of an EXISTING non-policy resource.

## Steps to Reproduce

1. Call AssociateAdminAccount, then PutResourceSet with {"ResourceSet":{"Name":"rs","ResourceTypeList":["AWS::EC2::VPC"]}} and keep the returned ResourceSetArn.
2. POST TagResource with {"ResourceArn": "<that ResourceSetArn>", "TagList":[{"Key":"stage","Value":"test"}]}.
3. Observed: 400 ResourceNotFoundException. Expected: 200, and a later ListTagsForResource on the same ARN reports stage=test.
4. Repeat with a protocols-list / applications-list ARN for a list that exists in the store: same wrong refusal.
