Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# Hoist the duplicated ResourceArns bound and ARN-shape checks into one helper

_Filed: 2026-10-28T02:38:29Z_
_Operation: GroupResources | Class: maintainability_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GroupResources` and `UngroupResources` each carry their own private `MAX_RESOURCE_ARNS` constant and their own verbatim copy of the two request checks it guards (list length, then "every entry must be a resource ARN"), including identical refusal sentences. The crate's convention everywhere else is that a rule shared by several operations lives once in `crate::state` (`check_tags`, `check_tag_keys`, `check_resource_query`, `check_configuration`, `page_size`), precisely so the bound and its sentence cannot drift between callers. These two copies can drift. The fix is to move the pair of checks (and the constant) into one `crate::state` helper that both handlers call, returning the same message strings, so every response and every existing test stays byte-identical.

## Evidence

Copy 1 — services/service-com-amazonaws-resourcegroups-ardi/src/ops/group_resources.rs:182-210, 317-320:
    "// `ResourceArns` is modelled `length 1..10` and the framework refuses a longer
     // list before this handler runs. Checking it again here is what BOUNDS the work
     // below: every scan over the request is at most this many entries.
     if input.resource_arns.len() > MAX_RESOURCE_ARNS { ... \"ResourceArns carries {} entries, which is more than the {} this operation accepts\" ... }
     if let Some(not_an_arn) = input.resource_arns.iter().find(|arn| !crate::state::is_resource_arn(arn)) { ... \"ResourceArns contains {not_an_arn}, which is not a resource ARN\" ... }"
    "const MAX_RESOURCE_ARNS: usize = 10;"

Copy 2 — services/service-com-amazonaws-resourcegroups-ardi/src/ops/ungroup_resources.rs:183-211, 290-293: the same two branches with the same two message strings and the same
    "const MAX_RESOURCE_ARNS: usize = 10;"

The crate states the one-place convention for its other shared rules:
  services/service-com-amazonaws-resourcegroups-ardi/src/state.rs:1740-1744
    "The ONE place this service decides a page size, so every listing ... refuses a value outside `1..=MAX_PAGE_SIZE` with the same sentence, and the bound and that sentence cannot drift apart between them."
  services/service-com-amazonaws-resourcegroups-ardi/src/state.rs:474-476
    "The ONE place this service decides whether a `GroupConfigurationList` is one it accepts, so a create and a later put judge the same document the same way."

Existing tests that pin both copies' responses (and must stay unchanged):
  services/service-com-amazonaws-resourcegroups-ardi/tests/all/resource_group_lifecycle.rs:633-665 — "ResourceArns past the modelled length", "a ResourceArns entry that is not an ARN", "... whose account is not twelve digits", "... whose region is not a region" for GroupResources.
  services/service-com-amazonaws-resourcegroups-ardi/tests/all/ungroup_resources.rs:555-580 — "a ResourceArns entry that is not an ARN", "a member beside a ResourceArns entry that is not an ARN", the 11-entry list, and the empty list for UngroupResources.

## Steps to Reproduce

1. Open services/service-com-amazonaws-resourcegroups-ardi/src/ops/group_resources.rs and read lines 182-210 plus the trailing `const MAX_RESOURCE_ARNS: usize = 10;` at line 320.
2. Open services/service-com-amazonaws-resourcegroups-ardi/src/ops/ungroup_resources.rs and read lines 183-211 plus the trailing `const MAX_RESOURCE_ARNS: usize = 10;` at line 293.
3. Observe the two branches, the constant and both refusal sentences are duplicated verbatim, whereas every other rule shared by two operations of this crate (tags, resource queries, configurations, page size) lives once in src/state.rs.
4. Verify the duplication is behaviour-visible on both paths via tests/all/resource_group_lifecycle.rs:633-665 and tests/all/ungroup_resources.rs:555-580, which must keep passing unchanged after the rule is hoisted into one helper.
