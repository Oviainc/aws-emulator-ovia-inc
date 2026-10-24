Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 743fc0a4792edb6d196f368afdefaae152d33ff0

# Host placement references and esxVersion are accepted without being resolved against EC2 or the version catalog

_Filed: 2026-10-24T02:21:27Z_
_Operation: CreateEnvironmentHost | Class: validation_

> _Note: this issue was identified against the code base at commit `743fc0a4792edb6d196f368afdefaae152d33ff0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEnvironmentHost` launches an EC2 bare metal instance for the host, so the EC2 resources the request names must exist: `placementGroupId` is "The unique ID of the placement group where the host is placed" (`^pg-[a-f0-9]{8}([a-f0-9]{9})?$`), `dedicatedHostId` is "The unique ID of the Amazon EC2 Dedicated Host" (`^h-[a-f0-9]{8}([a-f0-9]{9})?$`), and `keyName` is "the SSH key that is used to access the host". The operation's `esxVersion` is likewise not free text: "If you don't specify an ESX version … Amazon EVS automatically uses the default ESX version for your environment's VCF version. To find the available ESX versions for a particular VCF version, use the GetVersions action."

Expected: a request naming a placement group, Dedicated Host or key pair the account does not hold is refused with the operation's modelled ValidationException naming that member, and so is an `esxVersion` that is not one of the versions available for the environment's VCF version — the same way CreateEnvironment already refuses a `vpcId`/`serviceAccessSubnetId` the account's EC2 store holds no record of.

Actual: the handler's precondition list only covers the environment state, the dedicatedHostId/placementGroupId conflict, host-name uniqueness and the vmkManagement address pool. It never consults the EC2 store at all (no `ec2_resources` use in this file), so `placementGroupId: "pg-00000000"`, `dedicatedHostId: "h-00000000"` and `keyName: "does-not-exist"` are all accepted with HTTP 200 and stored, and the host is reported as CREATING→CREATED and echoed back by ListEnvironmentHosts with those phantom references. `esxVersion` is accepted for any string up to 128 characters, stored, and reported by nothing — so no version is validated and no default is resolved.

## Evidence

Spec: CreateEnvironmentHost doc ("This action can only be used after the Amazon EVS environment is deployed… To find the available ESX versions for a particular VCF version, use the GetVersions action"), `HostInfoForCreate` member docs for keyName/placementGroupId/dedicatedHostId, and the operation's modelled ValidationException (HTTP 400). Unknown-placement-group / unknown-key-pair refusals are well-established EC2 launch behaviour (InvalidPlacementGroup.Unknown, InvalidKeyPair.NotFound). Handler: src/ops/create_environment_host.rs lines 241-308 — the four checks listed above and nothing else; grep for `ec2_resources` under src/ops shows only create_environment.rs and associate_eip_to_vlan.rs, so this handler reads no EC2 state, while src/state/ec2_resources.rs already demonstrates the pattern and the EC2 emulator holds `placement_groups` (services/service-com-amazonaws-ec2-amazonec2/src/state.rs line 1706). src/state.rs lines 605-611 concedes `esx_version` is "Kept as supplied … and reported by nothing". Tests: tests/all/hosts.rs and tests/all/connectors_and_hosts.rs assert the deployed-state rule, the placement conflict, host-name uniqueness and the address pool; no test asserts a refusal for a placement group / Dedicated Host / key pair that does not exist, or for an unsupported esxVersion.

## Steps to Reproduce

1. Create an environment (registering its VPC and subnet in the EC2 store) and wait for environmentState=CREATED.
2. Call CreateEnvironmentHost with host={hostName:"esx-1", keyName:"no-such-key", instanceType:"i4i.metal", placementGroupId:"pg-00000000"} and esxVersion="9.9.9".
3. Observe HTTP 200 with the host in CREATING instead of a ValidationException naming the placement group, the key pair or the ESX version.
4. Call ListEnvironmentHosts — the host is reported (and reaches CREATED) carrying the placementGroupId that names no placement group in the account.
