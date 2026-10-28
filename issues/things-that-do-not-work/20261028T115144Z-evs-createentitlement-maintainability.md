Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# The distinct-vmIds rule is implemented twice, verbatim, in CreateEntitlement and DeleteEntitlement

_Filed: 2026-10-28T11:51:44Z_
_Operation: CreateEntitlement | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both entitlement handlers implement the same rule — "a `vmIds` list naming the same VM twice names one entitlement" — with the same hand-rolled de-duplicating loop, copied line for line. The two copies can drift: a change to how a repeated `vmId` is folded (ordering, case, trimming) applied to one handler would silently leave the other behaving differently, even though the rule is one rule about one identity (`crate::state::entitlement_key`, the (connector, VM) pair).

The fix is to state the rule once (e.g. a `distinct_vm_ids(&[String]) -> Vec<&str>` helper beside `entitlement_key` in `crate::state`, which already owns the identity the rule derives from) and call it from both handlers. Every response and every existing test stays byte-identical: the helper returns the same first-seen order the loops already produce.

## Evidence

Copy 1 — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_entitlement.rs:245-254:
```
        // One entitlement per DISTINCT requested virtual machine: the (connector, VM)
        // pair identifies an entitlement, so a `vmIds` list naming the same VM twice
        // names one entitlement. `vmIds` holds at most 100 entries (modelled len
        // 1..100), which bounds this scan.
        let mut wanted: Vec<&str> = Vec::with_capacity(input.vm_ids.len());
        for vm_id in &input.vm_ids {
            if !wanted.contains(&vm_id.as_str()) {
                wanted.push(vm_id.as_str());
            }
        }
```
Copy 2 — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/delete_entitlement.rs:247-256 (identical code, same comment):
```
        // One entitlement per DISTINCT requested virtual machine: the (connector, VM)
        // pair identifies an entitlement, so a `vmIds` list naming the same VM twice
        // names one entitlement and that entitlement is reported once. `vmIds` holds at
        // most 100 entries (modelled len 1..100), which bounds this scan.
        let mut wanted: Vec<&str> = Vec::with_capacity(input.vm_ids.len());
        for vm_id in &input.vm_ids {
            if !wanted.contains(&vm_id.as_str()) {
                wanted.push(vm_id.as_str());
            }
        }
```
The single identity the rule derives from already lives in one place — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:870-881 (`entitlement_key`: "That pair is what identifies an entitlement — `CreateEntitlement`, `ListVmEntitlements` and `DeleteEntitlement` all address entitlements by `connectorId` plus `vmIds`").

Existing tests that pin the behaviour of both copies: tests/all/entitlements.rs:146-166 (row "the same VM named twice") and tests/all/delete_entitlement.rs:217-233 (row "the same VM named twice").

## Steps to Reproduce

1. Read src/ops/create_entitlement.rs:245-254 and src/ops/delete_entitlement.rs:247-256 side by side: the de-duplication loop is the same five lines in both.
2. Change the rule in one file only (for example, fold vmIds case-insensitively) and observe that the sibling operation keeps the old behaviour, so CreateEntitlement and DeleteEntitlement would disagree about which VMs one request names.
3. Confirm the tests that cover it — tests/all/entitlements.rs row "the same VM named twice" and tests/all/delete_entitlement.rs row "the same VM named twice" — exercise the two copies independently.
