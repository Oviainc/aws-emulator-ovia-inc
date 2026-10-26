Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8

# CreateEnvironment accepts a vcfHostnames object whose nine hostnames are not unique

_Filed: 2026-10-26T01:52:19Z_
_Operation: CreateEnvironment | Class: validation_

> _Note: this issue was identified against the code base at commit `6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`VcfHostnames` documents a uniqueness requirement over its own nine members: "Each hostname must be unique". `CreateEnvironment`'s handler never compares the nine hostnames to each other — it stores the whole object verbatim onto the new environment record — so a request whose `vCenter` and `sddcManager` (or any other pair of the nine) carry the same string is accepted with HTTP 200 and the duplicate pair is persisted and reported back by `GetEnvironment`.

The handler enumerates twelve request-level rules it enforces beyond the modelled constraints in its module header, and separately enumerates the rules it deliberately does NOT enforce (`initialVlans.hcxNetworkAclId` resolution, the SELF_DEPLOYED-unsupported parameters, Premium Support). The `vcfHostnames` uniqueness rule appears in neither list: it is simply absent, with no stated reason.

The same crate DOES implement the analogous DNS-hostname uniqueness rule for the other member that carries hostnames — `hosts[].hostName` — in this very handler, with a dedicated state helper, so the capability and the pattern already exist here.

## Evidence

Smithy model (smithy_query(query="shape", shape="VcfHostnames")), shape documentation:
  "The DNS hostnames that Amazon EVS uses to install VMware vCenter Server, NSX, SDDC Manager, and Cloud Builder. Each hostname must be unique, and resolve to a domain name that you've registered in your DNS service of choice. Hostnames cannot be changed."
  The shape has nine @required members (vCenter, nsx, nsxManager1..3, nsxEdge1..2, sddcManager, cloudBuilder), all targeting `HostName`.

Handler: the object is stored without any comparison —
  services/.../src/ops/create_environment.rs:723
    `            vcf_hostnames: input.vcf_hostnames,`
  and nothing else in the file mentions it: `grep -n "vcf_hostnames|vcfHostnames"` inside `create_environment.rs` returns only the doc lines 86/152/229/234 and this store at 723. There is no uniqueness check, and `vlan_failures` / `host_failures` / `terms_accepted_failure` — the only request-level validators the handler calls — do not look at it.

The crate enforces the sibling DNS-hostname rule in the SAME handler, showing the rule class is already implemented here:
  services/.../src/ops/create_environment.rs:455-483
    `        // "DNS hostnames for hosts must be unique across Amazon EVS environments and`
    `        // within VCF" (\`HostInfoForCreate$hostName\` doc): the hosts this create adds take`
    `        // their names, so a name a host of some environment already carries — or a name`
    `        // this one request gives twice — names a host that cannot be added`
    `                let clause = if state.host_name_is_taken(host_name) { ... } else if !named.insert(host_name) {`
    `                    "is used by another host of this request; host DNS hostnames must be \
                     unique across environments"`
  with the helper at services/.../src/state.rs:2337 `pub fn host_name_is_taken(&self, host_name: &str) -> bool`.

Test coverage: no test exercises duplicate `vcfHostnames`. Every fixture supplies nine distinct names, e.g.
  services/.../tests/all/create_environment_validation.rs:150-154
    `    let hostnames = json!({`
    `        "vCenter": "vcenter", "nsx": "nsx", "nsxManager1": "nsxm1", "nsxManager2": "nsxm2",`
    `        "nsxManager3": "nsxm3", "nsxEdge1": "nsxe1", "nsxEdge2": "nsxe2",`
    `        "sddcManager": "sddc", "cloudBuilder": "builder",`
    `    });`
so nothing pins the current acceptance of duplicates.

## Steps to Reproduce

1. Create a VPC and a subnet in it through the EC2 emulator so the network preconditions resolve.
2. Call CreateEnvironment with `vcfVersion` `VCF-5.2.1`, a valid `vpcId`/`serviceAccessSubnetId`, `termsAccepted: true`, ten non-overlapping /28 `initialVlans` blocks inside the VPC, four `hosts`, and `vcfHostnames` in which two members carry the same string (for example `"vCenter": "evs-1"` and `"sddcManager": "evs-1"`, the other seven distinct).
3. Observe HTTP 200 and an `environment` in the response: the duplicate pair was accepted even though `VcfHostnames` documents "Each hostname must be unique".
4. Call GetEnvironment for the returned `environmentId` and observe `vcfHostnames` reporting the duplicated hostname on both members.
5. Compare with the same request where two entries of `hosts` share a `hostName`: that one is refused with ValidationException and a `fieldList` entry naming `hosts[n].hostName` (create_environment.rs:455-483), showing the crate enforces the sibling uniqueness rule but not this one.
