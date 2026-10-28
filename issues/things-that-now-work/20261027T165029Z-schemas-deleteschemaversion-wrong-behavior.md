Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 03bb735b5164c6b554d75cb5873a72ff1e3f303a

# DeleteSchemaVersion leaves LastModified stale on a rule UpdateSchema already breaks

_Filed: 2026-10-27T16:50:29Z_
_Operation: DeleteSchemaVersion | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `03bb735b5164c6b554d75cb5873a72ff1e3f303a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate holds two incompatible definitions of a schema's `LastModified`.

`DeleteSchemaVersion` deliberately does not advance `schema.last_modified`, and justifies it with a narrow rule it writes down in the handler: `LastModified` "reports when a definition of the schema was last WRITTEN". Under that rule a change that writes no definition must not move the timestamp.

`UpdateSchema` breaks exactly that rule in its sibling path: a request that carries only `Description` (no `Content`, so no version is minted and no definition is written) sets `schema.last_modified = now`. So the crate already treats a metadata-only change as a modification that moves `LastModified` — which means the "definition last written" rule DeleteSchemaVersion relies on is not the rule this crate actually implements, and the model's own wording for the member ("The date and time that schema was modified") is the broader one.

Observable effect: deleting a version changes what the schema is (`ListSchemas`' `VersionCount` drops, and a version-less `DescribeSchema`/`ExportSchema` now resolves to a different version, or to none at all), yet `DescribeSchema`/`ListSchemas` keep reporting the pre-delete `LastModified`. A client that polls `LastModified` to detect that a schema changed sees no change after a delete, while it does see one after a description-only update. The two paths must agree on one definition of the member; today they do not.

## Evidence

Smithy (the only external statement of what the member means) — `smithy_query(query="doc", shape="DescribeSchemaResponse", member="LastModified")`: "<p>The date and time that schema was modified.</p>" (same doc on `SchemaSummary.LastModified` and `UpdateSchemaResponse.LastModified`).

Side 1 — the rule stated in the crate, services/service-com-amazonaws-schemas-schemas/src/ops/delete_schema_version.rs:224-229:
```
        // The schema's `LastModified` is deliberately NOT advanced: it reports when a
        // definition of the schema was last WRITTEN, which taking one away does not
        // change, and what a caller reads to see which versions a schema holds is the
        // version set itself (`ListSchemaVersions`, and `ListSchemas`' `VersionCount`),
        // both read from the stored versions rather than from a timestamp.
        if !schema.remove_version_named(&input.schema_version) {
```

Side 2 — the sibling handler that breaks it, services/service-com-amazonaws-schemas-schemas/src/ops/update_schema.rs:299-308:
```
            // No `Content`: the definition is unchanged, so no version is minted and
            // the only thing a request can still change is the description.
            None => {
                if let Some(description) = input.description {
                    // The same write as above: the description belongs to the schema,
                    // ...
                    schema.description = Some(description);
                    schema.last_modified = now;
                }
```
Here no definition is written (`input.content` is `None`, so `Schema::add_version` — the only other writer of `last_modified`, state.rs:1322 `self.last_modified = now;` — is never called), and `last_modified` is advanced anyway.

Test coverage: nothing pins either side. `tests/all/schema_version_deletion.rs` asserts the version set and the 404s but never reads `LastModified`; grepping the whole suite for `LastModified` (tests/all/iso8601_timestamps.rs, registry_schema_lifecycle.rs:413, aws_managed_catalog_metadata.rs:167-240, code_binding_lifecycle.rs:262, resource_tag_removal.rs:529) shows only timestamp FORMAT assertions and the fixed AWS-managed catalogue instants — no test states what a delete or a description-only update does to the member, so neither behaviour is pinned with a stated reason.

## Steps to Reproduce

1. POST /v1/registries/name/mine -> 201 (CreateRegistry)
2. POST /v1/registries/name/mine/schemas/name/s with {"Type":"OpenApi3","Content":"{\"openapi\":\"3.0.0\"}"} -> 201 (CreateSchema); note the LastModified it reports
3. PUT /v1/registries/name/mine/schemas/name/s with {"Content":"{\"openapi\":\"3.0.0\",\"info\":{}}"} -> 200 (UpdateSchema), minting version 2
4. GET /v1/registries/name/mine/schemas/name/s -> 200; record LastModified (call it T) and SchemaVersion 2
5. DELETE /v1/registries/name/mine/schemas/name/s/version/2 -> 204 (DeleteSchemaVersion)
6. GET /v1/registries/name/mine/schemas/name/s -> 200: SchemaVersion is now 1 and the Content is the first definition again, but LastModified is still T — the schema changed and the timestamp did not
7. GET /v1/registries/name/mine/schemas -> 200: VersionCount dropped to 1 while the summary's LastModified is still T
8. PUT /v1/registries/name/mine/schemas/name/s with {"Description":"anything"} -> 200 (UpdateSchema with no Content): LastModified DOES move, even though no definition was written — the opposite of the rule delete_schema_version.rs:224-228 states
