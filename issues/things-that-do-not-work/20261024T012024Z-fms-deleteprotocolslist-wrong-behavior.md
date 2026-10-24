Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# DeleteProtocolsList deletes Firewall Manager MANAGED (default) protocols lists instead of refusing

_Filed: 2026-10-24T01:20:24Z_
_Operation: DeleteProtocolsList | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Firewall Manager distinguishes two protocols-list namespaces: the customer lists a caller creates with PutProtocolsList, and the Firewall Manager MANAGED (default) lists that AWS owns — which is exactly why GetProtocolsList/ListProtocolsLists carry a `DefaultList`/`DefaultLists` member to select between them. A managed list is AWS-owned state a customer cannot destroy; a DeleteProtocolsList naming one must be refused (the service answers the "nothing to do / operation wasn't possible" refusal, InvalidOperationException, or — matching the namespace choice the emulator's own PutProtocolsList already makes — ResourceNotFoundException), and the list must survive.

This handler removes whatever `ListId` names, with no `default_list` filter at all: it happily deletes a managed list, drops its tag index entry, and a following GetProtocolsList(DefaultList=true) for that id then answers ResourceNotFoundException. The emulator is therefore internally inconsistent: GetProtocolsList filters on `record.default_list == input.default_list`, PutProtocolsList refuses an update whose id names a managed list (`.filter(|record| !record.default_list)`), and only the delete path ignores the distinction — so the one operation that destroys state is the one that does not respect the namespace.

## Evidence

Model: GetProtocolsList input has `DefaultList` (boolean, default false) and ListProtocolsLists has `DefaultLists`, so the model itself splits managed from customer lists; ProtocolsListData/AppsListData are the shapes both namespaces share. Handler: `src/ops/delete_protocols_list.rs` lines 180-189 — `store.protocols_lists.remove(&input.list_id)` with no `default_list` check, followed by `store.tags_by_arn.remove(...)`. Contrast `src/ops/get_protocols_list.rs` lines 174-177 (`.filter(|record| record.default_list == input.default_list)`) and `src/ops/put_protocols_list.rs` lines 231-234 (`.filter(|record| !record.default_list)`), which both enforce the namespace. Test coverage: `tests/all/protocols_list_lifecycle.rs` seeds a managed list (`MANAGED_LIST_ID`, `seed_protocols_list(..., true)`) and asserts only that PutProtocolsList and GetProtocolsList(DefaultList=false) do not resolve it (lines 389-479); no test calls DeleteProtocolsList with that id, so the deletion of a managed list is unasserted.

## Steps to Reproduce

1. Call AssociateAdminAccount so the administrator gate is open.
2. Arrange a Firewall Manager managed protocols list in the store (the emulator has no API that creates one; the test suite seeds it with `seed_protocols_list(store, id, name, protocols, /*default_list=*/true)`).
3. Confirm it is visible as a managed list: GetProtocolsList {"ListId": <id>, "DefaultList": true} answers 200.
4. POST DeleteProtocolsList {"ListId": <id>}. Observed: 200 and the managed list is gone. Expected: a 400 refusal (InvalidOperationException, or ResourceNotFoundException consistent with PutProtocolsList's namespace choice) and the managed list still readable.
5. Re-run GetProtocolsList {"ListId": <id>, "DefaultList": true}: observed 400 ResourceNotFoundException, i.e. AWS-owned state was destroyed through a customer API.
