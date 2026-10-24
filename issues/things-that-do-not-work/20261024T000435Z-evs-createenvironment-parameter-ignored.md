Issue-Severity: HIGH
Issue-Gap-Class: parameter-ignored
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# CreateEnvironment stores `hosts` but never materialises them, so the environment has no hosts

_Filed: 2026-10-24T00:04:35Z_
_Operation: CreateEnvironment | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For any `vcfVersion` other than `SELF_DEPLOYED` the `hosts` member (modelled `len 4..4`) names the four ESX hosts Amazon EVS adds to the environment while it installs VCF ('The ESX hosts to add to the environment', `CreateEnvironmentRequest$hosts`; 'When you specify any other VCF version, Amazon EVS installs and configures VCF for you', CreateEnvironment doc). Expected: after a create that carried four hosts, those hosts are part of the environment — ListEnvironmentHosts reports four `Host` records (hostName/ipAddress/keyName/instanceType/placementGroupId/dedicatedHostId as supplied, each moving CREATING -> CREATED), and a later CreateEnvironmentHost for one of those hostnames is refused as a duplicate name. Actual: the handler copies the list into `StoredEnvironment::requested_hosts` and nothing ever reads it: `environment.hosts` (the map ListEnvironmentHosts/CreateEnvironmentHost use) stays empty, so ListEnvironmentHosts answers an empty list for an environment created with four hosts, and the hostnames the create consumed are still free. The only validation applied to the list is the dedicatedHostId/placementGroupId conflict check; every other value is write-only.

## Evidence

Spec: `CreateEnvironmentRequest$hosts` doc ('The ESX hosts to add to the environment ... Not supported when vcfVersion is SELF_DEPLOYED. In that case, you can add hosts using CreateEnvironmentHost after the environment is created') plus the CreateEnvironment doc note that non-SELF_DEPLOYED versions install VCF. Handler: src/ops/create_environment.rs lines 347 and 410 (`let requested_hosts = input.hosts.unwrap_or_default();` ... `requested_hosts,`) — nothing else in the crate reads that field; src/state.rs `StoredEnvironment::requested_hosts` doc still says 'nothing reports them yet (CreateEnvironmentHost / ListEnvironmentHosts still answer 501)', which is stale now that both are implemented. src/ops/list_environment_hosts.rs:208 iterates `environment.hosts` only. Tests: tests/all/environment_lifecycle.rs rows at lines 667-686 only assert the 200/400 status of host placement validation; no test asserts the created hosts are readable back, and tests/all/hosts.rs only exercises CreateEnvironmentHost.

## Steps to Reproduce

1. CreateEnvironment with vcfVersion=VCF-5.2.1, termsAccepted=true, a valid 10-VLAN plan, and hosts=[4 HostInfoForCreate entries with distinct hostNames].
2. Wait for the environment to reach CREATED (GetEnvironment).
3. Call ListEnvironmentHosts for that environmentId — the emulator returns an empty environmentHosts list instead of the four hosts the create asked for.
4. Call CreateEnvironmentHost with one of the four hostNames — it is accepted as a brand-new host instead of being refused for a duplicate DNS hostname.
