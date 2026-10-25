Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# UpdateHypervisor invents a Host-uniqueness constraint and returns a spurious 409 ConflictException

_Filed: 2026-10-24T16:45:21Z_
_Operation: UpdateHypervisor | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: AWS Backup Gateway imposes no uniqueness constraint on a hypervisor's Host. An UpdateHypervisor call that sets a hypervisor's Host to a value another hypervisor record already holds succeeds with HTTP 200 and echoes the HypervisorArn, and a later GetHypervisor on each record reports its own Host (two independent hypervisor configurations may legitimately point at the same host: credential rotation, re-registration, several configurations of one appliance).

Actual: the handler refuses the update with HTTP 409 ConflictException / ErrorCode `HypervisorAlreadyExists` ("A hypervisor has already been imported for this host.") whenever another stored record holds the requested Host, and writes nothing — not even the `Name` or `LogGroupArn` members the same request carried, so a multi-member update is rejected whole because of the host alone.

This is the same invented constraint already established as wrong for ImportHypervisorConfiguration (see the open issue 'ImportHypervisorConfiguration wrongly enforces Host uniqueness and returns a spurious ConflictException', whose recorded AWS behavior shows repeated imports of the identical Name+Host each succeeding with distinct ARNs). The state layer backs both paths with the same `hypervisor_by_host` lookup, so fixing only the import path leaves this divergence on the update path: after the import-side check is removed, two records can legitimately share a host, and UpdateHypervisor would then still refuse to move a third record onto either of them.

## Evidence

Spec: smithy_query(operation="UpdateHypervisor") — input members HypervisorArn (required), Host (len 3..128, pattern ^.+$), Username, Password, Name, LogGroupArn; the model declares no uniqueness trait, no idempotency token, and nothing that makes Host a key. Recorded AWS behavior (open issue 20261023T110543Z-backup-gateway-importhypervisorconfiguration-wrongly-enforces-host-uniqueness-and-returns-a, trace backup-gateway.concurrent-import-hypervisor-duplicate-name-host-all-independent) shows AWS treats Name+Host as non-unique and keeps independent hypervisor records for one host, so no Host collision can be a ConflictException condition.
Handler: src/ops/update_hypervisor.rs lines ~210-225 map `HypervisorUpdateRefusal::HostAlreadyImported` to ConflictException{ErrorCode: HYPERVISOR_ALREADY_EXISTS} and return before any write; src/state.rs `update_hypervisor` lines ~1226-1234 perform the `hypervisor_by_host` collision check (the same helper the import handler uses, src/state.rs ~1192) and return `HostAlreadyImported` with nothing written.
Tests: tests/all/hypervisor_update.rs::an_update_onto_a_host_another_hypervisor_holds_is_refused_as_a_conflict asserts the current (invented) 409 for every pairing of stored records, so that test asserts the divergent behavior and must be updated alongside the handler; no test asserts that two hypervisors may share a host.

## Steps to Reproduce

1. Boot the emulator and point a `backup-gateway` client at it.
2. ImportHypervisorConfiguration with Name="hv-one", Host="10.0.0.11" -> note HypervisorArn A.
3. ImportHypervisorConfiguration with Name="hv-two", Host="10.0.0.12" -> note HypervisorArn B.
4. Call UpdateHypervisor with HypervisorArn=B, Host="10.0.0.11", Name="hv-two-renamed".
5. Observe HTTP 409 ConflictException with ErrorCode "HypervisorAlreadyExists" instead of HTTP 200 echoing B.
6. Call GetHypervisor on B and observe neither the Host nor the Name of the rejected request was applied — AWS would report Host=10.0.0.11 and Name=hv-two-renamed.
