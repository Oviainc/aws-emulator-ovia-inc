Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# DeleteAppsList deletes Firewall Manager MANAGED (default) applications lists instead of refusing

_Filed: 2026-10-24T01:20:24Z_
_Operation: DeleteAppsList | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

As with protocols lists, Firewall Manager keeps customer applications lists and AWS-MANAGED (default) applications lists in separate namespaces — GetAppsList takes a `DefaultList` member and ListAppsLists a `DefaultLists` member precisely to select between them, and `AppsListRecord.default_list` records the distinction in this emulator's own store. A managed list is AWS-owned and a customer DeleteAppsList naming one must be refused (InvalidOperationException, or ResourceNotFoundException for the namespace it is not in), leaving the list in place.

The handler applies no `default_list` filter: it removes whatever `ListId` names, managed or not, and drops the ARN's tag index entry with it. Expected: a managed list survives a delete attempt; actual: it is destroyed and reports 200.

## Evidence

Model: GetAppsList declares `DefaultList` (boolean, default false) and ListAppsLists declares `DefaultLists`, so the model splits the two namespaces; `src/state.rs` AppsListRecord (lines 250-268) carries `default_list` and its doc states "`GetAppsList`'s `DefaultList` member selects between the two namespaces, so a customer list is not reported as a managed one". Handler: `src/ops/delete_apps_list.rs` lines 179-188 — `store.apps_lists.remove(&input.list_id)` with no default_list check. The sibling protocols-list handlers show the intended discipline (`put_protocols_list.rs` lines 231-234 filter managed lists out of the writable namespace). Test coverage: `tests/all/administrator_gated_operations.rs` seeds apps lists via `seed_apps_list` and asserts the gate/not-found paths only; no test deletes a managed (default_list=true) applications list.

## Steps to Reproduce

1. Call AssociateAdminAccount so the gate is open.
2. Arrange a managed applications list in the store with `default_list = true` (no implemented API creates one — PutAppsList is still a stub — so seed it the way the test suite's `seed_apps_list` helper does).
3. POST DeleteAppsList {"ListId": <that id>}. Observed: 200 and the managed list is removed from `apps_lists` (its tags_by_arn entry too). Expected: a 400 refusal and the managed list still stored.
