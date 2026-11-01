Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 09793af671f727f5831b9f0700c1238f4e9cbb7e

# Host DNS hostname uniqueness is decided byte-exactly, so two hosts whose names differ only in case are both admitted — against the crate's own "a DNS name is case-insensitive" rule

_Filed: 2026-10-29T12:42:01Z_
_Operation: CreateEnvironmentHost | Class: validation_

> _Note: this issue was identified against the code base at commit `09793af671f727f5831b9f0700c1238f4e9cbb7e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`host_name_is_taken` is the one place this crate decides whether a `hostName` is free, and it compares the requested name to every stored host's name as RAW BYTES. Everywhere else this crate compares a DNS name it treats case as insignificant, with that reason written into the code and into a test. So `CreateEnvironmentHost` accepts `ESX-1` for an account that already holds a host named `esx-1` (and `CreateEnvironment` accepts a `hosts` list whose four entries differ only in case), storing two records under two map keys for what is one DNS hostname — the opposite of the rule the helper's own doc quotes from the model.

The same byte comparison sits on the sibling path through the same helper: `CreateEnvironment` asks it per `hosts[i].hostName` (create_environment.rs:543) and refuses only an exact-case collision, while `named.insert(host_name)` (create_environment.rs:546) also dedupes the request's own four names case-sensitively. One fix — comparing the name the way `entitlements_sharing_vcenter` already compares an appliance FQDN — covers both call sites and leaves every other response alone.

This is NOT the already-open applianceFqdn issue (20261027T195946Z-evs-updateenvironmentconnector-wrong-behavior--r2): that one is about `Connector$applianceFqdn` in `UpdateEnvironmentConnector`/`CreateEnvironmentConnector` and names neither `host_name_is_taken` nor either host-create path.

## Evidence

The rule the helper quotes from the model, and the byte comparison under it — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:2793-2809:
```
    /// Whether some host of some environment this service holds already carries
    /// `host_name`: "DNS hostnames for hosts must be unique across Amazon EVS
    /// environments and within VCF" (`HostInfoForCreate$hostName` doc), so the scope is
    /// every environment and not one — and not one Region either, since the uniqueness
    /// the doc states is within VCF rather than within an endpoint.
...
    pub fn host_name_is_taken(&self, host_name: &str) -> bool {
        self.environments.values().any(|environment| {
            environment.state != STATE_DELETED
                && environment
                    .live_hosts()
                    .any(|host| host.host_name == host_name)
        })
    }
```
The model sentence it quotes (`smithy_query(query="shape", shape="HostInfoForCreate")`): `hostName` — "The DNS hostname of the host. DNS hostnames for hosts must be unique across Amazon EVS environments and within VCF." The modelled `HostName` pattern is `^([a-zA-Z0-9\-]*)$`, which admits upper and lower case alike, so the boundary validator passes both spellings and the handler is what has to decide they are one name.

The SAME crate's rule for comparing a DNS name — src/state.rs:1706-1724:
```
    /// The vCenter is the APPLIANCE the connector targets, not the connector record, so
    /// every connector of this environment reaching the same `applianceFqdn` counts —
...
    /// the same vCenter. The comparison is case-insensitive because a DNS name is.
...
                other
                    .appliance_fqdn
                    .eq_ignore_ascii_case(&connector.appliance_fqdn)
```
and the crate's own test asserting that premise — tests/all/connector_deletion.rs:596-598:
```
        // DNS names are case-insensitive, so the uppercase spelling is the same vCenter.
        &appliance_fqdn.to_uppercase(),
```

The two call sites that act on the byte comparison — src/ops/create_environment_host.rs:260 and :328-337:
```
        let host_name_is_taken = state.host_name_is_taken(&input.host.host_name);
...
        // "DNS hostnames for hosts must be unique across Amazon EVS environments and
        // within VCF": the scope is every environment, THIS one included, which is what
        // `host_name_is_taken` walks.
        if host_name_is_taken {
            failures.push(crate::state::field_failure(
                "host.hostName",
                "is already used by a host of an Amazon EVS environment; host DNS hostnames \
                 must be unique across environments",
            ));
        }
```
and src/ops/create_environment.rs:542-551 (`let clause = if state.host_name_is_taken(host_name) { … } else if !named.insert(host_name) { … }`). The host is then stored under the raw string — create_environment_host.rs:419: `environment.hosts.insert(input.host.host_name.clone(), host);` — so `esx-1` and `ESX-1` occupy two entries of the map `ListEnvironmentHosts` reports.

The only text defending the exact comparison is a comment about a DIFFERENT member, which asserts the sibling rule rather than a reason for it — create_environment.rs:1086-1089: "The comparison is the exact string the request gave, as the sibling DNS-hostname rule compares host names ([`…host_name_is_taken`])."

Test coverage: the uniqueness rule is pinned only for identical spellings, so nothing pins the current behaviour — tests/all/created_environment_hosts.rs:599-671 (`a_create_may_not_take_a_host_name_that_is_already_taken`), whose row table carries only exact-match rows (`["held-1", "free-5", …]`, `["twice", "free-13", "twice", "free-14"]`) and no case-mutated row; tests/all/hosts.rs:20 and tests/all/host_deletion.rs:491-522 likewise exercise only exact names.

## Steps to Reproduce

1. CreateEnvironment with an installed VCF version (for example `VCF-5.2.1`) and let the CREATING -> CREATED transition settle.
2. CreateEnvironmentHost with `host.hostName` = "esx-1" — accepted, and the host is stored under that key (src/ops/create_environment_host.rs:419).
3. CreateEnvironmentHost again with `host.hostName` = "ESX-1" (the same DNS hostname, which this crate treats as the same name at src/state.rs:1724 and in tests/all/connector_deletion.rs:597).
4. Observe HTTP 200: `host_name_is_taken` (src/state.rs:2808) compared the bytes and found no collision, so a second host is added for one DNS hostname.
5. ListEnvironmentHosts for the environment reports BOTH records, although `HostInfoForCreate$hostName` states "DNS hostnames for hosts must be unique across Amazon EVS environments and within VCF" — the sentence the helper's own doc quotes at src/state.rs:2794-2795.
6. The same holds for CreateEnvironment: a request whose `hosts` carry "esx-a" and "ESX-A" is served (create_environment.rs:543-551), and both hosts are materialised.
