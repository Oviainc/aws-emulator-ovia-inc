Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# UntagResource refuses every rule groups namespace ARN, so namespace tags can never be removed

_Filed: 2026-10-24T18:42:09Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` removes tags from any taggable AMP resource — rule groups namespaces among them, per the `TagResource`/`ListTagsForResource` documentation that enumerates the taggable types. The emulator resolves `resourceArn` only against the workspace ARN index.

Expected: `DELETE /tags/{rule-groups-namespace-ARN}?tagKeys=team` removes `team` from that namespace's stored tag map and answers 200 (and, being `@idempotent`, answers 200 again for a key the namespace no longer carries).

Actual: the namespace ARN resolves to nothing, so the call is refused 404 and the tag map is left untouched. A tag attached at `CreateRuleGroupsNamespace` can never be removed through any operation.

## Evidence

Spec: `TagResource`/`ListTagsForResource` docs name rule groups namespaces as taggable, and `UntagResource` is the modelled removal path. Handler: `src/ops/untag_resource.rs:159-166` resolves via `store.workspace_id_of_arn` and otherwise returns `crate::state::arn_not_found()`; `arn_index` only holds workspace ARNs (populated in `src/ops/create_workspace.rs:316`). Tests: `tests/all/rejection_codes.rs:404-410` only covers a namespace ARN that was never minted; no test untags a live namespace.

## Steps to Reproduce

1. Create a workspace (wait for ACTIVE) and a rule groups namespace with {"tags":{"team":"obs"}}.
2. Read the namespace ARN via DescribeRuleGroupsNamespace.
3. DELETE /tags/{that ARN, URL-encoded}?tagKeys=team — expected 200 with the tag removed, actual 404 not-found.
4. GET /workspaces/{workspaceId}/rulegroupsnamespaces/{name} — the tag is still there.
