Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 2a76ec95dcbb7025f794fe41b3b77939fee1d4dd

# The member-scoped node lookup (and the Fabric "memberId is required" refusal) is written out in three node handlers instead of once

_Filed: 2026-10-27T09:22:39Z_
_Operation: GetNode | Class: maintainability_

> _Note: this issue was identified against the code base at commit `2a76ec95dcbb7025f794fe41b3b77939fee1d4dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three handlers resolve a node inside a member scope with the same expression, and two of them carry the same Fabric-member-required refusal verbatim. The rule — "on a Hyperledger Fabric network a request must name the owning member, and a named member only SCOPES the node lookup, so a node that member does not own is not found" — therefore exists in three copies that can drift: a change to the scope test or to the refusal has to be made in get_node.rs, delete_node.rs and update_node.rs independently. A single helper in `crate::state` (resolve a node by network/member/node id, plus the one refusal for a Fabric network with no member scope) would leave every response byte-identical: the filter expression is character-for-character the same in all three, and the refusal message is the same format string in both handlers that make it.

## Evidence

Copy 1 — src/ops/get_node.rs:184-196 and 202-213:

```
if input.member_id.is_none()
    && network.framework == crate::state::FRAMEWORK_HYPERLEDGER_FABRIC
{
    return Err(GetNodeErrorModeled::InvalidRequestException(
        InvalidRequestException {
            message: Some(format!(
                "memberId is required on a {} network",
                crate::state::FRAMEWORK_HYPERLEDGER_FABRIC
            )),
...
let found = network.node(&region, &input.node_id).filter(|node| {
    input
        .member_id
        .as_deref()
        .is_none_or(|member_id| node.member_id.as_deref() == Some(member_id))
});
```

Copy 2 — src/ops/delete_node.rs:186-198 and 203-208: the same refusal (`"memberId is required on a {} network"`, `crate::state::FRAMEWORK_HYPERLEDGER_FABRIC`) and the same filter:

```
let found = network.node(&region, &input.node_id).filter(|node| {
    input
        .member_id
        .as_deref()
        .is_none_or(|member_id| node.member_id.as_deref() == Some(member_id))
});
```

Copy 3 — src/ops/update_node.rs:190-195, the same filter again (on the mutable lookup):

```
let found = network.node_mut(&region, &input.node_id).filter(|node| {
    input
        .member_id
        .as_deref()
        .is_none_or(|member_id| node.member_id.as_deref() == Some(member_id))
});
```

For contrast, the crate already single-homes comparable rules in `crate::state`: `not_ready_refusal` is "The one place the three tagging operations get that refusal, so they cannot drift apart" (src/state.rs:1927-1930) and `resolve_tagged_record!` is "The ONE walk of the store" (src/state.rs:1571-1572).

Test coverage that must stay green unchanged: tests/all/not_found_and_malformed_ids.rs:116-278 tables GetNode/DeleteNode/UpdateNode unknown-network and unknown-node 404s; tests/all/node_deletion_lifecycle.rs:117-242 and tests/all/network_and_node_lifecycle.rs:946-1068 cover the DeleteNode/GetNode member scope; tests/all/update_node_log_publishing.rs and tests/all/update_status_window.rs:281-331 cover UpdateNode's scoped lookup.

## Steps to Reproduce

1. Read src/ops/get_node.rs:184-213, src/ops/delete_node.rs:186-214 and src/ops/update_node.rs:190-201 side by side: the member-scope filter is identical in all three and the Fabric "memberId is required on a <framework> network" refusal is identical in the first two.
2. Note that the crate keeps comparable cross-operation rules in one place (src/state.rs:1927-1942 `not_ready_refusal`, src/state.rs:1571-1609 `resolve_tagged_record!`).
3. Change the scope rule in one copy only (e.g. make an unmatched member scope a different answer in get_node.rs) and observe the other two handlers keep the old behaviour — the copies are free to drift.
