Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# ListTagsForResource resolves only workspace ARNs, so a rule groups namespace's tags are unreadable (404)

_Filed: 2026-10-24T18:42:09Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents that AMP tags live on three resource types: "Currently, the only resources that can be tagged are scrapers, workspaces, and rule groups namespaces" (com.amazonaws.amp#ListTagsForResource documentation). The emulator resolves the `resourceArn` label through `ComAmazonawsAmpAmazonprometheusserviceState::workspace_id_of_arn`, which reads `arn_index` — a map that is only ever written by `CreateWorkspace` (`src/ops/create_workspace.rs:316`) and therefore holds WORKSPACE ARNs exclusively.

Expected: `GET /tags/{resourceArn}` for the ARN of a live rule groups namespace (`arn:aws:aps:<region>:<account>:rulegroupsnamespace/<workspaceId>/<name>`, the exact ARN `CreateRuleGroupsNamespace`/`DescribeRuleGroupsNamespace` just returned) answers 200 with the tags that namespace holds — the same map `DescribeRuleGroupsNamespace` reports in `ruleGroupsNamespace.tags`.

Actual: that ARN resolves to nothing, so the handler returns `crate::state::arn_not_found()` — a 404 not-found refusal — even though the namespace exists and the store holds its tags. Tags attached at `CreateRuleGroupsNamespace` are thus visible through DescribeRuleGroupsNamespace/ListRuleGroupsNamespaces but invisible to the tagging API, which is the API every tag-based tool (Resource Groups, cost allocation, `aws aps list-tags-for-resource`) uses.

## Evidence

Spec: `ListTagsForResource` documentation names rule groups namespaces as taggable; `RuleGroupsNamespaceDescription$tags` and `CreateRuleGroupsNamespaceRequest$tags` exist in the model and the emulator persists them (`src/ops/create_rule_groups_namespace.rs:335`, `src/state.rs` `RuleGroupsNamespace.tags`). Handler: `src/ops/list_tags_for_resource.rs:166-172` resolves only via `store.workspace_id_of_arn(...)`; `grep arn_index` shows the index is populated solely in `create_workspace.rs:316`. Tests: `tests/all/rejection_codes.rs:404-410` only asserts that a rule groups namespace ARN *no store ever minted* 404s — no test asserts the tags of an EXISTING namespace are readable, so this is uncovered.

## Steps to Reproduce

1. POST /workspaces (CreateWorkspace), wait for the workspace to reach ACTIVE.
2. POST /workspaces/{workspaceId}/rulegroupsnamespaces with {"name":"rules1","data":"Z3JvdXBzOiBbXQ==","tags":{"team":"obs"}} — 202.
3. GET /workspaces/{workspaceId}/rulegroupsnamespaces/rules1 and read ruleGroupsNamespace.arn (and observe tags = {"team":"obs"}).
4. GET /tags/{that ARN, URL-encoded} — expected 200 with {"tags":{"team":"obs"}}, actual 404 not-found.
