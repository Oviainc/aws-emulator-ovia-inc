Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# A public HCX VLAN subnet is accepted with any netmask between /24 and /28 instead of the documented /28

_Filed: 2026-10-26T23:25:24Z_
_Operation: CreateEnvironment | Class: validation_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InitialVlans$hcx` documents an extra requirement that applies ONLY when the HCX VLAN is provisioned public (`isHcxPublic` true): the block must have a /28 netmask. The handler applies only the general per-VLAN rule (a prefix length between /24 and /28) to every one of the ten blocks, and the one public-HCX-specific rule it does enforce is the network ACL id. So a `CreateEnvironment` request with `initialVlans.isHcxPublic = true`, `hcxNetworkAclId` supplied and `initialVlans.hcx.cidr` of, say, `/26` or `/24` is accepted and the environment is created with a public HCX VLAN whose subnet is larger than the documented /28.

The machinery for the refusal is already in the same function: `vlan_failures` already branches on `initial.is_hcx_public` (for the network ACL rule) and already has each block's `prefix_len` in hand (for the /24../28 rule), so nothing new has to be computed — the public case simply never narrows the allowed prefix length.

## Evidence

Spec (`smithy_query(query="doc", shape="InitialVlans", member="hcx")`): "If you plan to use a public HCX VLAN subnet, the following requirements must be met: Must have a /28 netmask and be allocated from the IPAM public pool. Required for HCX internet access configuration."

Handler — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_environment.rs:1111-1157 is the only place VLAN blocks are judged:
* line 1117-1123: `// "Required when isHcxPublic is set to true" (`hcxNetworkAclId` doc).` / `if initial.is_hcx_public && initial.hcx_network_acl_id.is_none() {` — the ONE rule keyed on `isHcxPublic`;
* line 1139: `if !(MIN_VLAN_PREFIX_LEN..=MAX_VLAN_PREFIX_LEN).contains(&prefix_len) {` with the constants at line 931-935: `/// The largest VLAN subnet prefix length: "Amazon EVS VLAN subnets have a minimum CIDR block size of /28 and a maximum size of /24" (`InitialVlans` doc).` / `const MAX_VLAN_PREFIX_LEN: u8 = 28;` / `const MIN_VLAN_PREFIX_LEN: u8 = 24;` — applied to every member alike, with no narrowing for the public HCX member.
The handler's own rule list confirms the scope it believes it covers (create_environment.rs:35-37): "2. A VLAN subnet CIDR block is between /24 and /28, and no two of an environment's VLAN subnets may overlap. 3. `hcxNetworkAclId` is required when `isHcxPublic` is true." — no rule 'a public HCX VLAN subnet is a /28'. `grep -n 'is_hcx_public' src/` matches only create_environment.rs:1088 (storing `is_public`) and 1118 (the ACL rule).

Test coverage: tests/all/create_environment_validation.rs:228/245/262 pin the generic range ("a /29 VLAN subnet is one past the /28 minimum", "a /24 VLAN subnet is the published maximum and is accepted", "a /28 VLAN subnet is the published minimum and is accepted") and no row combines `isHcxPublic` with a non-/28 hcx block. tests/all/vlan_subnet_propagation.rs:280-288 does, incidentally, assert a SUCCESSFUL create for `Row { case: "ten /26 blocks in another Region, with a public HCX VLAN", prefix_len: 26, public_hcx: true }` — that row states no reason for the combination (its subject is subnet propagation across block sizes) and would need its hcx block changed to /28 when the rule is enforced.

## Steps to Reproduce

1. Register a VPC (e.g. 192.168.0.0/16) and a service access subnet in the account's EC2 store.
2. Call CreateEnvironment with termsAccepted=true, a provided vcfVersion, ten non-overlapping /26 VLAN blocks of that VPC, `initialVlans.isHcxPublic = true` and `initialVlans.hcxNetworkAclId = "acl-0123456789abcdef0"`.
3. Observe HTTP 200: the environment is created and ListEnvironmentVlans reports the hcx VLAN with `isPublic: true` and a /26 `cidr`.
4. Expected per `InitialVlans$hcx` ("If you plan to use a public HCX VLAN subnet … Must have a /28 netmask"): a ValidationException whose fieldList names `initialVlans.hcx`, exactly as the /24../28 range refusal and the `hcxNetworkAclId` refusal already do.
