Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 83b2b15993ea2e3b2b9e4b142ed7a7a8695c4dfd

# A tagging ResourceArn naming another ACCOUNT resolves to this account's resource, although the crate refuses the same cross-account ARN for CDN secrets

_Filed: 2026-10-28T21:31:47Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `83b2b15993ea2e3b2b9e4b142ed7a7a8695c4dfd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::parse_tag_arn` splits a tagging `ResourceArn` into its six ARN fields and then DISCARDS the account field (`Some(_account)`). `resolved_group` goes on to compare only the Region. So `arn:aws:mediapackagev2:<region>:999999999999:channelGroup/<g>` — an ARN this emulator never minted, because `::engine::emulator_arn` stamps every ARN with the single account `000000000000` — resolves to THIS account's channel group: `ListTagsForResource` answers 200 with that group's tags, and `TagResource` / `UntagResource` (which share the same helper) answer 204 after writing that group's tag map.

The crate decides the very same question the opposite way one screen further down: `cdn_auth_rejection` parses a caller-supplied secret ARN and refuses one whose account is not `::engine::emulator_account_id()` with the model's `SECRET_FROM_DIFFERENT_ACCOUNT`. And `parse_tag_arn`'s own doc explains why the REGION field is read rather than skipped — because it is part of the resource's identity — while the account field, which is part of an ARN's identity for exactly the same reason, is dropped. A resource ARN that names another account names no resource this emulator holds, which is the case the crate already has an answer for (`RESOURCE_NOT_FOUND_CODE` / `TAGGED_RESOURCE_NOT_FOUND_MESSAGE`, "The resource arn you provided does not name a resource that exists").

## Evidence

The account is parsed and thrown away — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:1530: `let (Some("arn"), Some(partition), Some(service), Some(region), Some(_account), Some(resource)) = (`, and the only field checks that follow are state.rs:1540 `if partition.is_empty() || service != SIGNING_NAME {`.

The doc on the same function's TagTarget explains why the Region is read — state.rs:1508-1510: "The Region is read rather than skipped because it is part of the resource's identity (see [`ChannelGroupKey`]): the same path in two Regions names two different resources, so a request can only reach the one its own Region holds." — and `resolved_group` enforces only that, state.rs:632-634: "fn resolved_group(&self, region: &str, target: &TagTarget) -> Option<&ChannelGroup> { (target.region == region).then_some(())?;".

The SIBLING rule in the same crate, which does judge the account — state.rs:3285-3287: "each entry of `CdnIdentifierSecretArns` has to be the ARN of a Secrets Manager secret in the SAME account and Region as the endpoint the policy is attached to (the account every resource here belongs to, [`::engine::emulator_account_id`]…)" implemented at state.rs:3323-3328: `if arn.account != account { return Some(validation_failed(&path, &arn_scope_detail("the account", account), VALIDATION_SECRET_FROM_DIFFERENT_ACCOUNT)); }`.

Every ARN this service hands out carries one account — state.rs:1425-1427 `::engine::emulator_arn(SIGNING_NAME, region, &format!("channelGroup/{group}"))` over vendor/server/src/lib.rs:153-163: "Build a canonical AWS ARN for an emulated resource: `arn:aws:{service}:{region}:000000000000:{resource}`. The account segment is always [`EMULATOR_ACCOUNT_ID`]".

Handler that reaches it: src/ops/list_tags_for_resource.rs:154-169 (`parse_tag_arn` -> 400 BadRequestException, then `store.tags_of(&region, &target)` -> 404), src/ops/tag_resource.rs:191-239 and src/ops/untag_resource.rs:174-200 share the same helper.

Test coverage: the Region half of the rule IS pinned — tests/all/regional_partitioning.rs:518 `a_resource_arn_reaches_only_the_region_that_holds_it`; the malformed-ARN table pins partition/service/resource-path rows only and has NO foreign-account row — tests/all/rejection_rule_and_negative_paths.rs:607-630; the account half is pinned only for CDN secrets — tests/all/caller_supplied_arns.rs:201-206 (label "a secret in another account", refused `SECRET_FROM_DIFFERENT_ACCOUNT`). No test pins the current cross-account tagging behaviour, so no stated reason justifies it.

## Steps to Reproduce

1. CreateChannelGroup `g` (signed for us-west-2) and note the ARN it returns: `arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/g`.
2. GET /tags/arn%3Aaws%3Amediapackagev2%3Aus-west-2%3A999999999999%3AchannelGroup%2Fg (the same path, another account id). Observe 200 with group `g`'s tag map; expected the crate's own answer for an ARN that names no resource it holds — 404 `NotFoundException` with `TAGGED_RESOURCE_NOT_FOUND_MESSAGE` (the answer the same request gets when the ARN names another Region).
3. POST /tags/<same foreign-account ARN> with {"tags":{"k":"v"}}. Observe 204, then GET /tags/<the real ARN> and see `k=v` written onto this account's group.
4. DELETE /tags/<same foreign-account ARN>?tagKeys=k. Observe 204 and the tag removed from this account's group.
