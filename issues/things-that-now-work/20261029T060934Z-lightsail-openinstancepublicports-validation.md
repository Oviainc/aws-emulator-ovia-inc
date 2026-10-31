Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6cada3bea20bb0dfad4860907220259726a5f31e

# A firewall rule naming addresses of a family the instance does not have is stored, although the crate's own rule is that "a rule cannot admit an address family the instance does not have"

_Filed: 2026-10-29T06:09:34Z_
_Operation: OpenInstancePublicPorts | Class: validation_

> _Note: this issue was identified against the code base at commit `6cada3bea20bb0dfad4860907220259726a5f31e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`port_rule_from_request` is handed the instance's `ip_address_type` and the two call-site comments say the rule is "resolved against the instance's own address types" — but the address type is consulted on exactly ONE path: the substitution for a request that names no address at all (`anywhere_cidrs`). Addresses the request DOES name are checked for notation only, so a request may open a port on an IPv4-only instance with `ipv6Cidrs: ["::/0"]` (and on an IPv6-only instance with `cidrs: ["0.0.0.0/0"]`), and the rule is stored and reported.

The resulting state is one the crate says cannot exist: `anywhere_cidrs`' own doc states the rule ("because a rule cannot admit an address family the instance does not have"), `default_port_rules` applies it when it builds a new instance's firewall ("The IPv6 side of each rule is present only when the instance has an IPv6 address at all, so the reported rules match the addresses the instance has"), and the crate already owns the predicates (`ip_address_type_has_ipv4` / `ip_address_type_has_ipv6`, which `SetIpAddressType` calls). After the call, `GetInstancePortStates` and `GetInstance` report `accessFrom` "Anywhere (::/0)" (via `StoredPortRule::access_from`, which falls to the IPv6 arm when `cidrs` is empty) for an instance that holds no IPv6 address at all — the reported rules no longer "match the addresses the instance has".

The same gap is on `PutInstancePublicPorts`, which resolves every `portInfos` entry through the same helper (src/ops/put_instance_public_ports.rs:206-217).

Note the one place the crate deliberately leaves a stale family in a rule: `SetIpAddressType` says "The instance's firewall is NOT rewritten: its rules are what the caller asked for, kept verbatim" (src/ops/set_ip_address_type.rs:277-281). That is about rules ALREADY stored when the address type changes; it says nothing about accepting a new rule for a family the instance does not have, which is the case this issue is about.

## Evidence

The rule, stated by the crate — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:4769-4773:
```
/// The IPv4 and IPv6 "every address" blocks an instance with `ip_address_type`
/// can be reached on: each present only when that address type gives the
/// instance an address of the family, because a rule cannot admit an address
/// family the instance does not have.
fn anywhere_cidrs(ip_address_type: &str) -> (Vec<String>, Vec<String>) {
```
and applied on the default-rule path — src/state.rs:4742-4748:
```
/// The inbound rules Lightsail opens on a newly created instance's firewall:
...
/// The IPv6 side of each rule is present only when the instance has an IPv6
/// address at all, so the reported rules match the addresses the instance has.
pub fn default_port_rules(blueprint_id: &str, ip_address_type: &str) -> Vec<StoredPortRule> {
```

The call site that claims the rule is enforced — src/ops/open_instance_public_ports.rs:185-193:
```
                // The rule is resolved against the instance's own address types
                // and refused BEFORE the firewall is touched, so a request this
                // service cannot accept changes nothing.
                let rule = crate::state::port_rule_from_request(
                    "portInfo",
                    &input.port_info,
                    &instance.ip_address_type,
                )
                .map_err(invalid)?;
```

The helper that does not enforce it. Its own doc enumerates the checks and the family is not among them — src/state.rs:4795-4800:
```
/// What the model cannot express is checked here: the protocol must be one of
/// the modelled `NetworkProtocol` values, a port RANGE must run forwards, each
/// address must be in the notation its own member takes, and an alias must be
/// one Lightsail publishes. A rule that names no address at all is taken as open
/// to anywhere — Lightsail's own default rules are, and a rule admitting nothing
/// would open a port nothing can reach.
```
and the code consults `ip_address_type` only when every list is empty — src/state.rs:4837-4854:
```
    let ipv6_cidrs = checked_cidrs(
        &format!("{member}.ipv6Cidrs"),
        port_info.ipv6_cidrs.as_deref().unwrap_or_default(),
        is_ipv6_cidr,
        "an IPv6 address or an IPv6 range in CIDR notation",
    )?;
...
    let (cidrs, ipv6_cidrs) =
        if cidrs.is_empty() && ipv6_cidrs.is_empty() && cidr_list_aliases.is_empty() {
            anywhere_cidrs(ip_address_type)
        } else {
            (cidrs, ipv6_cidrs)
        };
```
Neither does the store write — `open_port` only de-duplicates by port range and protocol and bounds the firewall (src/state.rs:5225-5245).

What the stored rule then reports — src/state.rs:4724-4732:
```
    fn access_from(&self) -> &'static str {
        if self.cidrs.iter().any(|cidr| cidr == ANY_IPV4_CIDR) {
            ACCESS_FROM_ANYWHERE_IPV4
        } else if self.ipv6_cidrs.iter().any(|cidr| cidr == ANY_IPV6_CIDR) {
            ACCESS_FROM_ANYWHERE_IPV6
```

The predicates the crate already has for the check — src/ops/set_ip_address_type.rs:297-298:
```
    let wants_ipv4 = crate::state::ip_address_type_has_ipv4(ip_address_type);
    let wants_ipv6 = crate::state::ip_address_type_has_ipv6(ip_address_type);
```

Test coverage: tests/all/instance_public_ports.rs:330-446 is the case table for an accepted rule — every case that names an explicit `ipv6Cidrs` (lines 390-396, 431-436) runs against a `dualstack` instance, and the only IPv6-only instance case (lines 346-348, "one tcp port, no address named, ipv6-only instance") names no address, so it pins the substitution path. tests/all/instance_public_ports.rs:803-820 pins `ipv6Cidrs` NOTATION refusals ("ipv6Cidrs carrying an IPv4 block"). No case supplies an address of a family the instance does not have, so the current behaviour is not pinned.

## Steps to Reproduce

1. CreateInstances with instanceNames ["ipv4-only"], a Linux blueprint, a bundle, availabilityZone "us-east-1a" and ipAddressType "ipv4"; wait for it to settle.
2. GetInstance with instanceName "ipv4-only": it reports no ipv6Addresses, and its two default rules carry no ipv6Cidrs (default_port_rules applied the family rule).
3. OpenInstancePublicPorts with instanceName "ipv4-only" and portInfo {"fromPort": 443, "toPort": 443, "protocol": "tcp", "ipv6Cidrs": ["::/0"]} — answered 200.
4. GetInstancePortStates with instanceName "ipv4-only": the firewall now holds a rule whose ipv6Cidrs is ["::/0"] and whose accessFrom is "Anywhere (::/0)", on an instance that has no IPv6 address — the state src/state.rs:4769-4772 says cannot exist ("a rule cannot admit an address family the instance does not have").
5. The mirror case: create an instance with ipAddressType "ipv6" and OpenInstancePublicPorts with portInfo {"fromPort": 80, "toPort": 80, "protocol": "tcp", "cidrs": ["0.0.0.0/0"]} — also 200, and the rule is reported as "Anywhere (0.0.0.0/0)" on an instance holding no IPv4 address.
6. PutInstancePublicPorts with the same portInfos entry reaches the identical helper (src/ops/put_instance_public_ports.rs:206-217) and stores the same rule.
