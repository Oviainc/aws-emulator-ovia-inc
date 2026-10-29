Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 889b7a0ea20193cdb3c9577846ef2b90647eb8e8

# A CDN distribution is given the ipv6 address type its own modelled member documents it cannot have

_Filed: 2026-10-29T08:02:20Z_
_Operation: SetIpAddressType | Class: validation_

> _Note: this issue was identified against the code base at commit `889b7a0ea20193cdb3c9577846ef2b90647eb8e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`SetIpAddressType` writes the requested `ipAddressType` onto a distribution record with no check of WHICH values a distribution may carry, so `ipAddressType="ipv6"` is accepted and stored, and every later `GetDistributions` / `GetDistribution*` read reports a distribution whose `ipAddressType` is `ipv6`.

The model documents the value domain per resource, and the distribution's is NARROWER than the shared `IpAddressType` enum: `LightsailDistribution$ipAddressType` is documented "The possible values are `ipv4` for IPv4 only, and `dualstack` for IPv4 and IPv6" — two values, no `ipv6` — while `Instance$ipAddressType` and `LoadBalancer$ipAddressType` each document all three ("`ipv4` for IPv4 only, `ipv6` for IPv6 only, and `dualstack` for IPv4 and IPv6"). The restriction belongs to the distribution alone, and this handler treats all three resource kinds alike on this point: it already applies the per-resource rules it knows about (the instance arm refuses a bundle-changing move without `acceptBundleUpdate`, and the distribution arm is gated on the us-east-1-only rule), but no arm holds the distribution to its documented two values.

The same value reaches a distribution on the create path: `CreateDistribution` takes `input.ip_address_type` verbatim, its comment quoting only the DEFAULT sentence of the very paragraph that states the two possible values. So a distribution can be born `ipv6` as well as be moved there, and nothing in the crate ever refuses it — a client is handed a distribution state the model says does not exist, and an `ipv6` distribution is one that, by the model's own wording, serves no IPv4 traffic at all.

## Evidence

The modelled value domain for a DISTRIBUTION — `smithy_query(query="doc", shape="LightsailDistribution", member="ipAddressType")`:
```
"The IP address type of the distribution." / "The possible values are <code>ipv4</code> for IPv4 only, and <code>dualstack</code> for IPv4 and IPv6."
```
and `smithy_query(query="doc", shape="CreateDistributionRequest", member="ipAddressType")`:
```
"The IP address type for the distribution." / "The possible values are <code>ipv4</code> for IPv4 only, and <code>dualstack</code> for IPv4 and IPv6." / "The default value is <code>dualstack</code>."
```
The same member on the two OTHER resource kinds this operation serves documents three values, so the two-value domain is the distribution's own — `smithy_query(query="doc", shape="Instance", member="ipAddressType")` and `shape="LoadBalancer"`:
```
"The possible values are <code>ipv4</code> for IPv4 only, <code>ipv6</code> for IPv6 only, and <code>dualstack</code> for IPv4 and IPv6."
```

The handler that writes it unchecked — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/set_ip_address_type.rs:421-434:
```
fn set_distribution_ip_address_type(
    store: &mut crate::state::ComAmazonawsLightsailLightsail20161128State,
    name: &str,
    ip_address_type: &str,
    now: i64,
) -> Result<Operation, SetIpAddressTypeError> {
    let due = crate::state::DueNamedState::after(now, crate::state::DISTRIBUTION_STATUS_DEPLOYED);
    let distribution = store
        .distribution_mut(name)
        .ok_or_else(|| not_found(crate::state::RESOURCE_TYPE_DISTRIBUTION, name))?;
    distribution.ip_address_type = ip_address_type.to_string();
```
The only value-domain check this operation makes is on the RESOURCE TYPE — src/ops/set_ip_address_type.rs:197-203:
```
        if !SETTABLE_RESOURCE_TYPES.contains(&resource_type.as_str()) {
            return Err(invalid(format!(
                "resourceType '{resource_type}' is not a resource type this action sets the \
                 IP address type of: the resource values are {}",
```
while the instance arm shows the handler DOES apply per-resource rules where it knows them — src/ops/set_ip_address_type.rs:308-314:
```
    if wants_ipv4 != crate::state::ip_address_type_has_ipv4(&current) && !accept_bundle_update {
        return Err(invalid(format!(
            "acceptBundleUpdate must be true to change the ipAddressType of instance \
             '{name}' from {current} to {ip_address_type}: ..."
```

The create path stores the same value verbatim, quoting only the default sentence — src/ops/create_distribution.rs:299-304:
```
        // "The IP address type for the distribution. [...] The default value is
        // dualstack."
        let ip_address_type = input
            .ip_address_type
            .clone()
            .unwrap_or_else(|| crate::state::DEFAULT_IP_ADDRESS_TYPE.to_string());
```

Test coverage: tests/all/resource_ip_address_type.rs:46 declares `const IP_ADDRESS_TYPES: [&str; 3] = ["dualstack", "ipv4", "ipv6"];` and the distribution case loops over ALL THREE (line 648 `for (position, to) in IP_ADDRESS_TYPES.iter().enumerate()`), asserting at lines 679-684 that the distribution reports the value that was set:
```
        assert_eq!(
            rolling_out["ipAddressType"].as_str(),
            Some(*to),
            "case {case}: the next read reports the address type that was set, got \
             {rolling_out:?}"
```
The stated reason for that generalisation is the GENERIC member doc only — tests/all/resource_ip_address_type.rs:7-13: "the call records the requested `ipAddressType` on the ONE stored resource ... an instance holds an address of exactly the families \"ipv4 for IPv4 only, ipv6 for IPv6 only, and dualstack for IPv4 and IPv6\" gives it" and lines 22-24 "every ordered pair of the modelled `IpAddressType` values". It cites no distribution-specific sentence, and the distribution's own two-value documentation above contradicts it, so the distribution row of that loop is the case to narrow to `ipv4`/`dualstack` plus a refusal for `ipv6`.

## Steps to Reproduce

1. Create an origin: CreateInstances with instanceNames=["cdn-origin"], availabilityZone="us-east-1a", blueprintId="amazon_linux_2023", bundleId="micro_3_0"; wait for running.
2. CreateDistribution in us-east-1 with distributionName="typed-cdn", bundleId="small_1_0", origin={name:"cdn-origin", regionName:"us-east-1", protocolPolicy:"http-only"}, defaultCacheBehavior={behavior:"cache"}, ipAddressType="ipv4".
3. Call SetIpAddressType in us-east-1 with resourceType="Distribution", resourceName="typed-cdn", ipAddressType="ipv6".
4. Observe HTTP 200 and an operation-log entry, instead of the modelled InvalidInputException: the value is accepted for a resource whose modelled member documents only ipv4 and dualstack.
5. Call GetDistributions and observe the distribution reporting ipAddressType="ipv6" — a value LightsailDistribution$ipAddressType says is not one of its possible values.
6. Also reachable at create time: CreateDistribution with ipAddressType="ipv6" is accepted and the distribution reports it from birth.
7. Contrast: the same ipAddressType="ipv6" on resourceType="Instance" or "LoadBalancer" is legitimate, since those members document three values.
