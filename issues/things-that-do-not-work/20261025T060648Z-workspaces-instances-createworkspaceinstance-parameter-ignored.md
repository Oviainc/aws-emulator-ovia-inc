Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# CreateWorkspaceInstance drops ManagedInstance.TagSpecifications instead of tagging the instance with it

_Filed: 2026-10-25T06:06:48Z_
_Operation: CreateWorkspaceInstance | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWorkspaceInstance` builds the instance's tag set from the top-level `Tags` member only. The `ManagedInstance.TagSpecifications` list — a `TagSpecification` whose `ResourceType` enumerates `instance` — is never read: the tags it carries never enter `WorkspaceInstanceRecord.tags`, so `ListTagsForResource` (which reports exactly that map) never reports them, and `reject_reserved_tag_keys` is never applied to them, so an `aws:`-prefixed key smuggled through that member is accepted where both sibling paths refuse it.

The sibling create in the same crate does precisely this work for its own `TagSpecifications` member: it rejects reserved keys across every specification's tags, then merges the tags of each specification that targets the resource being created onto the new record.

## Evidence

The sibling path, `src/ops/create_volume.rs:156-166` (reserved keys inside tag specifications):

```
    // A tag key in the reserved `aws:` namespace names a tag AWS generates, not one a
    // caller may set, whichever resource type the specification names.
    reject_reserved_tag_keys(
        "TagSpecifications",
        input
            .tag_specifications
            .iter()
            .flatten()
            .flat_map(|specification| specification.tags.iter().flatten())
            .filter_map(|tag| tag.key.as_deref()),
    )?;
```

and `src/ops/create_volume.rs:398-417` (the merge onto the record):

```
        // "Metadata tags for the volume": each `TagSpecification` carries a
        // `ResourceTypeEnum` and a `TagList`. The only resource this call creates is a
        // volume, so a specification that names another resource type tags nothing here;
        // one that names none, or names `volume`, tags the new volume.
        let mut tags = std::collections::BTreeMap::new();
        for specification in input.tag_specifications.unwrap_or_default() {
```

The other side, `src/ops/create_workspace_instance.rs:174-182` and `:259-264` — only `input.tags` is ever looked at:

```
        crate::state::reject_reserved_tag_keys(
            "Tags",
            input
                .tags
                .iter()
                .flatten()
                .filter_map(|tag| tag.key.as_deref()),
        )
```

```
        let mut tags = std::collections::BTreeMap::new();
        for tag in input.tags.unwrap_or_default() {
            if let Some(key) = tag.key {
                tags.insert(key, tag.value.unwrap_or_default());
            }
        }
```

The dropped member is modelled and documented: `ManagedInstanceRequest$TagSpecifications` — "Configures resource tagging specifications." (`smithy_query(query="shape",shape="ManagedInstanceRequest")`), `TagSpecification` — "Defines tagging configuration for a resource." with `ResourceType` targeting `ResourceTypeEnum`, whose values are `["instance", "volume", "spot-instances-request", "network-interface"]` (`smithy_query(query="shape",shape="ResourceTypeEnum")`), so `instance` is a resource type a specification can name on this call.

What a reader sees instead — `src/ops/list_tags_for_resource.rs:169-172`:

```
        // Exactly what create and `TagResource` persisted — no echo of the request.
        Ok(ListTagsForResourceOutput {
            tags: Some(crate::state::tag_list(&record.tags)),
        })
```

Test coverage: `tests/all/reserved_tag_keys.rs:3` states the suite covers "the reserved tag-key namespace, over every operation that stores caller tags" and exercises `CreateWorkspaceInstance`/`TagResource` via `Tags` (`:102`, `:111`, `:183`) and `CreateVolume` via `TagSpecifications` (`:274`, `:296`) — `ManagedInstance.TagSpecifications` appears in no test, so nothing pins the current drop.

## Steps to Reproduce

1. Call CreateWorkspaceInstance with {"ManagedInstance": {"InstanceType": "t3.medium", "TagSpecifications": [{"ResourceType": "instance", "Tags": [{"Key": "env", "Value": "dev"}]}]}} — 200 with a WorkspaceInstanceId.
2. Call ListTagsForResource with that id — Tags comes back empty; the env=dev tag the specification asked for was never applied (CreateVolume applies the same member's tags to the volume it creates).
3. Call CreateWorkspaceInstance with {"ManagedInstance": {"InstanceType": "t3.medium", "TagSpecifications": [{"ResourceType": "instance", "Tags": [{"Key": "aws:cloudformation:stack-name", "Value": "x"}]}]}} — accepted, while the same key in the top-level Tags member, or in CreateVolume's TagSpecifications, is refused with ValidationException.
