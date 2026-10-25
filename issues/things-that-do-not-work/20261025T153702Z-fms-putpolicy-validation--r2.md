Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: efbe6fec395222c954ec73ab018b3346b3354542

# PutPolicy (and the sibling Put* writers) store a TagList that names one key twice, which TagResource refuses and which merge_tags can never repair

_Filed: 2026-10-25T15:37:02Z_
_Operation: PutPolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `efbe6fec395222c954ec73ab018b3346b3354542`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

TagResource refuses a `TagList` that carries the same key twice with the modelled `InvalidInputException`, because "One key cannot carry two values, so a request listing a key twice has no single outcome". The resource-creating writers apply no such check: PutPolicy stores `input.tag_list` verbatim on both the create and the update path, and PutAppsList, PutProtocolsList and PutResourceSet do the same. The resource then holds two Tag entries with one key — exactly the state `merge_tags`' own documentation says cannot exist — and ListTagsForResource reports both. Worse, the duplicate is unrepairable through the API: `merge_tags` updates only the FIRST entry whose key matches, so a later TagResource for that key leaves the second, stale value in place and ListTagsForResource keeps reporting it. The validation the crate already owns (`first_duplicate_key`) is simply not applied on the four Put paths.

## Evidence

The check the crate already has, and the rule it states — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/tag_resource.rs:202-213:
```
        // One key cannot carry two values, so a request listing a key twice has
        // no single outcome: it is refused rather than silently resolved.
        if let Some(duplicate) = first_duplicate_key(&input.tag_list) {
            return Err(TagResourceErrorModeled::InvalidInputException(
                InvalidInputException {
                    message: Some(format!(
                        "TagList contains the tag key {duplicate} more than once"
                    )),
```
(the helper itself is tag_resource.rs:131-145).

The invariant the state module asserts — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:1643-1654:
```
/// Tags are a key:value MAP (`com.amazonaws.fms#Tag` is one key and one value),
/// so a resource can never hold two values for one key; and "add one or more
/// tags" leaves the keys the request did not mention alone.
pub fn merge_tags(existing: &mut Vec<Tag>, added: &[Tag]) {
    for tag in added {
        match existing.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
```
(`find` stops at the first match, so a second entry with the same key is never updated).

The writers that break it — src/ops/put_policy.rs:584-585 (create) and :556-559 (update):
```
                    // A new resource carries only the tags the create sent.
                    tags: input.tag_list.clone().unwrap_or_default(),
```
and the three siblings on the same shape of line: src/ops/put_apps_list.rs:307-308, src/ops/put_protocols_list.rs:313-314, src/ops/put_resource_set.rs:305-306. No Put handler calls `first_duplicate_key`: a grep for it matches only tag_resource.rs.

Test coverage: tests/all/tagging_refusals_across_resource_kinds.rs:94-110 pins the duplicate-key refusal for TagResource over all four resource kinds ("the same key twice" / "a repeated key among other keys" -> (400, "InvalidInputException")), and tests/all/tagging_and_managing_admins.rs:498-523 repeats it for a policy ARN; no test sends a repeated key through PutPolicy/PutAppsList/PutProtocolsList/PutResourceSet, so nothing pins the current acceptance.

## Steps to Reproduce

1. POST / with X-Amz-Target AWSFMS_20180101.AssociateAdminAccount and body {"AdminAccount": "<the emulator account id>"}
2. POST / with X-Amz-Target AWSFMS_20180101.PutPolicy and body {"Policy": {"PolicyName": "dup-tags", "SecurityServicePolicyData": {"Type": "WAFV2"}, "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer", "ExcludeResourceTags": false, "RemediationEnabled": false}, "TagList": [{"Key": "k", "Value": "1"}, {"Key": "k", "Value": "2"}]} — answers 200 and stores both tags (TagResource answers 400 InvalidInputException for the same TagList)
3. POST / with X-Amz-Target AWSFMS_20180101.ListTagsForResource and body {"ResourceArn": "<PolicyArn>"} — TagList reports the key "k" twice, with values "1" and "2"
4. POST / with X-Amz-Target AWSFMS_20180101.TagResource and body {"ResourceArn": "<PolicyArn>", "TagList": [{"Key": "k", "Value": "3"}]} — 200, but ListTagsForResource now reports k=3 AND k=2: the duplicate cannot be repaired
5. Repeat step 2 through PutAppsList, PutProtocolsList and PutResourceSet with the same TagList for the same outcome
