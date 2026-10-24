Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# ListTagsForResource resolves only policy ARNs, so tags written by PutResourceSet (and the list Put* operations) are unreadable

_Filed: 2026-10-24T00:05:56Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler resolves the request's ResourceArn only through `state::policy_id_for_arn`, which looks in `store.policies`. An ARN naming an existing resource set, applications list or protocols list is therefore answered with 400 ResourceNotFoundException instead of that resource's TagList — even though PutResourceSet stored the caller's TagList both on the record and in the ARN-keyed `tags_by_arn` index. Expected: for every taggable Firewall Manager resource the operation reports the tags the resource carries (an empty TagList when it carries none). Actual: only policies are resolvable; every other resource kind is reported as not found, so tags supplied at create time are write-only.

## Evidence

Model: `ListTagsForResourceRequest$ResourceArn` is a generic `ResourceArn` and the output `TagList` has length 0..200; the Put* operations for resource sets, applications lists and protocols lists each model a `TagList` input, so those resources carry tags that this operation must report. Handler: `src/ops/list_tags_for_resource.rs` lines 165-174 (`store.policy_id_for_arn(...).and_then(|id| store.policies.get(&id))`, else ResourceNotFoundException); `src/state.rs::policy_id_for_arn` matches policy records only. Contradicting stored state: `src/ops/put_resource_set.rs` lines 260-263 and 314-316 persist the TagList under the resource set's ARN. Test coverage: `tests/all/tagging_and_managing_admins.rs` exercises policy ARNs only.

## Steps to Reproduce

1. Call AssociateAdminAccount, then PutResourceSet with {"ResourceSet":{"Name":"rs","ResourceTypeList":["AWS::EC2::VPC"]},"TagList":[{"Key":"owner","Value":"sec"}]} and keep the returned ResourceSetArn.
2. POST ListTagsForResource with {"ResourceArn": "<that ResourceSetArn>"}.
3. Observed: 400 ResourceNotFoundException. Expected: 200 with TagList = [{Key: owner, Value: sec}].
