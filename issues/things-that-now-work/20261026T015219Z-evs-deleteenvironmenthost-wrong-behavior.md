Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8

# Deleting a host leaves the environment's modifiedAt at the instant the host was ADDED, contradicting the sibling create and the EIP handlers

_Filed: 2026-10-26T01:52:19Z_
_Operation: DeleteEnvironmentHost | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate implements two contradictory rules about whether changing a child record of an environment modifies the environment itself, and the two comments stating them sit in sibling handlers for the same resource.

`CreateEnvironmentHost` re-stamps `environment.modified_at_ms` and says adding a host "changes the environment". `DeleteEnvironmentHost` deliberately does NOT re-stamp it and says "deleting a child resource of it is no modification of the environment itself". Both operations answer with an `environmentSummary` that carries that `modifiedAt`, so the same parent field moves for one mutation of its host inventory and stays frozen for the inverse mutation.

The EIP handlers take the create side's reading for BOTH directions: `AssociateEipToVlan` re-stamps `environment.modified_at_ms` when an association is added to a child VLAN, and `DisassociateEipFromVlan` re-stamps it when one is removed. So the delete handlers are the outlier, and the observable result is a stale parent timestamp: add four hosts at T1, delete all four at T2, and the environment still reports `modifiedAt` = T1 even though its host inventory last changed at T2.

The stale stamp also feeds a derived value: `StoredEnvironment::health_changed_at_ms` is the max of the environment's own `modifiedAt` and its hosts', which is the instant `refresh_checks` dates a newly failing check's `impairedSince` from.

## Evidence

Side A — adding a host IS a modification of the environment:
  services/.../src/ops/create_environment_host.rs:421-423
    `        // Adding a host changes the environment, so the record is re-stamped and the`
    `        // summary this operation answers with reports that.`
    `        environment.modified_at_ms = created_at_ms;`

Side B — removing one is NOT, in the sibling handler for the same resource:
  services/.../src/ops/delete_environment_host.rs:270-272
    `        // The host was deleted FROM this environment, and the summary reports the`
    `        // environment as it stands: deleting a child resource of it is no modification of`
    `        // the environment itself, so its own \`modifiedAt\` does not move.`
  (the handler stamps only `host.modified_at_ms`, at delete_environment_host.rs:256, and never touches `environment.modified_at_ms`).

Side C — the crate's other pair of handlers re-stamps the environment for BOTH directions of a child change:
  services/.../src/ops/associate_eip_to_vlan.rs:412-414
    `                if wire.is_some() && !already_associated {`
    `                    environment.modified_at_ms = modified_at_ms;`
    `                }`
  services/.../src/ops/disassociate_eip_from_vlan.rs:284-287
    `        let vlan = vlan.to_wire();`
    `        if removed {`
    `            environment.modified_at_ms = modified_at_ms;`
    `        }`
  (`DeleteEnvironmentConnector` copies the delete-side reading at delete_environment_connector.rs:298-300.)

The stale value is consumed by the derived health timestamp:
  services/.../src/state.rs:1468-1475
    `    fn health_changed_at_ms(&self) -> i64 {`
    `        self.hosts.values().map(|host| host.modified_at_ms)`
    `            .chain(::std::iter::once(self.modified_at_ms)).max()`

Test coverage: nothing asserts the environment summary's `modifiedAt` on either side. `tests/all/host_deletion.rs:209-220` checks only `environmentSummary.environmentId` and `environmentSummary.environmentState`; `tests/all/hosts.rs:103-106` checks only `environmentSummary.environmentId`; `tests/all/connector_deletion.rs:366` compares the whole summary against a second read of the SAME record, which agrees either way. So no test pins the current split with an assertion.

## Steps to Reproduce

1. Create an environment and let its CREATING -> CREATED transition settle.
2. Call CreateEnvironmentHost once and record `environmentSummary.modifiedAt` (call it T1) — it moved, per create_environment_host.rs:421-423.
3. Wait past the host's CREATING -> CREATED transition, then call DeleteEnvironmentHost for that host.
4. Observe `environmentSummary.modifiedAt` in the delete response is still T1: the environment's host inventory changed, but the parent stamp did not move.
5. Call GetEnvironment and observe the same stale `modifiedAt`, and compare with AssociateEipToVlan/DisassociateEipFromVlan on the environment's public HCX VLAN, where BOTH adding and removing an association do move `environment.modifiedAt` (associate_eip_to_vlan.rs:412-414, disassociate_eip_from_vlan.rs:284-287).
