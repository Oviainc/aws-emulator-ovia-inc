Issue-Source: trace-sweep-divergence
Issue-Trace: ram-permission-tagging
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-permission-tagging
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|TagResource|2xx->4xx|$.tagsCreated
Issue-Key: ram|TagResource|2xx->4xx|status
Issue-Key: ram|GetPermission|2xx->2xx|$.permission.tags[*]
Issue-Key: ram|UntagResource|2xx->4xx|$.deletedTags
Issue-Key: ram|UntagResource|2xx->4xx|status
Issue-Commit: d36bb525c2331ae923943e9e5ae02c5704a8c1d0

# RAM tagging operations reject managed-permission ARNs, so permission tags can never be set or removed

> _Note: this issue was identified against the code base at commit `d36bb525c2331ae923943e9e5ae02c5704a8c1d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
RAM's TagResource/UntagResource accept either a resource-share ARN or a managed-permission ARN; the emulator only honours the share form, so the whole permission-tagging lifecycle fails and the permission's tag set never changes. 4 causes.

Cause 1 of 4 — TagResource with a permission ARN is refused (fixture ram-permission-tagging, step task#3).
AWS: TagResource (Smithy: TagResourceRequest members resourceShareArn:String, resourceArn:String, tags:TagList required; resourceArn doc: 'Specifies the Amazon Resource Name (ARN) of the managed permission that you want to add tags to. You must specify either resourceArn, or resourceShareArn, but not both') adds the tags to the named customer-managed permission and answers HTTP 200 with tagsCreated echoing the key/value pairs it created.
Emulator: HTTP 400 with no tagsCreated member — the request form that targets a permission is not served.
Why: tagging a managed permission is impossible, so tag-based cost allocation, ABAC conditions on permission tags and any tag round-trip against a permission fail at the first call.
Fix: the TagResource handler's target resolution (accept resourceArn naming a managed permission, apply the tags to the stored permission) and its tagsCreated serializer.

Cause 2 of 4 — the added tag is absent from the permission read back (fixture ram-permission-tagging, step task#4).
AWS: GetPermission's ResourceSharePermissionDetail carries tags: TagList ('The tag key and value pairs attached to the resource share'), so a tag just added via TagResource appears in the detail's tags list alongside the tags supplied at CreatePermission (the recording expects a second entry {key: stage, value: beta} next to the create-time tag).
Emulator: GetPermission answers 200 but $.permission.tags[1] is missing — only the create-time tag is present.
Why: a client cannot observe permission tags at all, so it cannot verify or reconcile them. (Consequence of cause 1's rejected write, but it is the read path's projection that must show the tag once the write lands, so it needs confirming separately.)
Fix: ram permission state (tag map on the permission) plus GetPermission's tags projection.

Cause 3 of 4 — UntagResource with a permission ARN is refused (fixture ram-permission-tagging, step task#5).
AWS: UntagResource (Smithy: UntagResourceRequest members resourceShareArn:String, resourceArn:String, tagKeys:TagKeyList required) removes the named keys from the managed permission and answers 200 with deletedTags listing the removed tags — each entry carrying the removed key (the recording shows {key: owner} with no value member).
Emulator: HTTP 400 with no deletedTags member.
Why: tags set on a permission can never be removed, so a permission's tag set is write-once at creation and ABAC/cost-allocation tags cannot be corrected.
Fix: the UntagResource handler's target resolution for resourceArn (managed permission) and its deletedTags serializer.

Cause 4 of 4 — the permission's tag set is unchanged after the untag (fixture ram-permission-tagging, step task#6).
AWS: after removing the create-time key and having added another, GetPermission reports exactly the remaining tag (the recording expects the single entry {key: stage, value: beta}).
Emulator: GetPermission answers 200 with the original create-time tag still present ({key: owner, value: team-a}) and the added one absent — the tag list is frozen at creation.
Why: readers see a tag the caller deleted and miss the one it added; anything authorizing on permission tags decides on stale data.
Fix: as causes 1-3 — the tag mutations must be applied to the stored permission and projected by GetPermission.

Traces: ram-permission-tagging#task#3; ram-permission-tagging#task#4; ram-permission-tagging#task#5; ram-permission-tagging#task#6.

## Evidence
Cause 1 — ram-permission-tagging#task#3, TagResource: status expected 200, got 400; $.tagsCreated missing (expected [{key: stage, value: beta}], got None).
Cause 2 — ram-permission-tagging#task#4, GetPermission, expected 200 / actual 200: $.permission.tags[1] missing (expected {key: stage, value: beta}, got None).
Cause 3 — ram-permission-tagging#task#5, UntagResource: status expected 200, got 400; $.deletedTags missing (expected [{key: owner}], got None).
Cause 4 — ram-permission-tagging#task#6, GetPermission, expected 200 / actual 200: $.permission.tags[0] expected {key: stage, value: beta}, got {key: owner, value: team-a} (reported as both missing and extra).

## Steps to Reproduce
1. CreatePermission with tags [{key: owner, value: team-a}] -> 200.
2. POST /tagresource with resourceArn = the permission ARN and tags [{key: stage, value: beta}] -> emulator answers 400 instead of 200 with tagsCreated (ram-permission-tagging#task#3).
3. GetPermission on the permission ARN -> tags list holds only the create-time tag; the added tag is absent (ram-permission-tagging#task#4).
4. POST /untagresource with resourceArn = the permission ARN and tagKeys [owner] -> emulator answers 400 instead of 200 with deletedTags [{key: owner}] (ram-permission-tagging#task#5).
5. GetPermission again -> tags still report {key: owner, value: team-a} instead of the single remaining {key: stage, value: beta} (ram-permission-tagging#task#6).
