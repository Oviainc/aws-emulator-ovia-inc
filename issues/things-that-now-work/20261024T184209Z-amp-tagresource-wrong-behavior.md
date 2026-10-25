Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# TagResource cannot tag a rule groups namespace: only workspace ARNs resolve, every namespace ARN is refused 404

_Filed: 2026-10-24T18:42:09Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource`'s own documentation states "The only resources that can be tagged are rule groups namespaces, scrapers, and workspaces." The emulator's handler resolves `resourceArn` exclusively through the workspace ARN index, so tagging any non-workspace AMP resource is impossible.

Expected: `POST /tags/{rule-groups-namespace-ARN}` with a `tags` map appends/replaces tags on that namespace ("If you specify a new tag key for the resource, this tag is appended ... If you specify a tag key that is already associated with the resource, the new tag value ... replaces the previous value"), answers 200, and the new map is then reported by `DescribeRuleGroupsNamespace`, `ListRuleGroupsNamespaces` and `ListTagsForResource`.

Actual: the ARN resolves to no entry in `arn_index`, so the call is refused with the shared 404 not-found rejection and no tag is stored. The namespace's stored tag map can therefore only ever be the one supplied at create time — it can never be changed after creation (note `PutRuleGroupsNamespace` deliberately takes no `tags`, so the tagging API is the ONLY way to retag a namespace, and it is unreachable).

## Evidence

Spec: `TagResource` documentation enumerates rule groups namespaces as taggable; `RuleGroupsNamespaceDescription$tags` is modelled and persisted by this crate. Handler: `src/ops/tag_resource.rs:176-183` — `store.workspace_id_of_arn(&input.resource_arn)` then `crate::state::arn_not_found()`; `arn_index` is written only in `src/ops/create_workspace.rs:316`. `src/ops/put_rule_groups_namespace.rs:292-298` notes tags cannot be changed through that operation. Tests: `tests/all/rule_groups_namespace_lifecycle.rs` asserts create-time tags only; no test tags an existing namespace ARN.

## Steps to Reproduce

1. Create a workspace and wait for ACTIVE.
2. POST /workspaces/{workspaceId}/rulegroupsnamespaces with {"name":"rules1","data":"Z3JvdXBzOiBbXQ=="} — 202; read its arn from DescribeRuleGroupsNamespace.
3. POST /tags/{that ARN, URL-encoded} with body {"tags":{"team":"obs"}} — expected 200 and the tag stored on the namespace, actual 404 not-found with nothing written.
4. GET /workspaces/{workspaceId}/rulegroupsnamespaces/rules1 — tags are still absent.
