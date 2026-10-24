Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# TagResource stores tag keys in the reserved aws: namespace instead of refusing them

_Filed: 2026-10-24T01:58:46Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler merges every supplied Tag whose Key is present onto the stored tag map, with the only rejection being the 50-tag ceiling. A key in the reserved `aws:` prefix (which the modelled TagKey pattern admits, since `:` is in its character class) is therefore accepted and afterwards reported by ListTagsForResource.

Expected: AWS reserves the `aws:` tag-key prefix for system tags; a user-supplied key beginning with `aws:` is refused with the modelled ValidationException (HTTP 400) naming the Tags member, and the resource's tag set is left unchanged. Actual: HTTP 200 and the reserved key is persisted.

## Evidence

Spec: TagResource models Tags (TagList len 0..50, required) over Tag{Key: TagKey len 1..128 pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$} — the pattern permits `aws:`, so only handler logic can enforce the reserved namespace — and declares ValidationException (400). The `aws:` prefix being reserved for AWS-generated tags is established behaviour of every AWS tagging API. Handler: src/ops/tag_resource.rs lines ~157-191 merges keys unconditionally and checks only MAX_TAGS_PER_INSTANCE. No test exercises a reserved key (tests/all/workspace_instance_lifecycle.rs `tag_resource_merges_onto_the_stored_tags` and `the_stored_tag_set_cannot_grow_past_what_the_model_can_report` use plain keys).

## Steps to Reproduce

1. Create an instance: CreateWorkspaceInstance {"ManagedInstance":{"InstanceType":"t3.medium"}}
2. Call TagResource with {"WorkspaceInstanceId":"<id>","Tags":[{"Key":"aws:cloudformation:stack-name","Value":"mine"}]} — emulator answers 200 instead of ValidationException
3. Call ListTagsForResource with that id — the reserved aws: key is reported as a user tag
