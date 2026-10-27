Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# httpProtocolIpv6 is accepted and applied for instances in every Region, although the model restricts the parameter to eu-north-1

_Filed: 2026-10-27T14:59:33Z_
_Operation: UpdateInstanceMetadataOptions | Class: validation_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled `httpProtocolIpv6` member carries a note stating where it is available: "This parameter is available only for instances in the Europe (Stockholm) Amazon Web Services Region (eu-north-1)." The handler's own module doc repeats that sentence, and then the handler stores the value for whatever instance the request names, wherever that instance is: `MetadataOptionsSelection { ..., http_protocol_ipv6: input.http_protocol_ipv6, ... }` with no reference to the instance's Region. Every later `GetInstance` then reports `metadataOptions.httpProtocolIpv6` as the caller set it for, say, a us-east-1 instance. The crate enforces the comparable documented Region restriction elsewhere through one owner (`crate::state::domain_region_refusal`, applied by the domain and distribution operations and by `SetIpAddressType` for a distribution), so the machinery and the precedent for a Region-restricted path already exist here; this member is the one that is not held to its own documented restriction.

The default projection has the same reach: with nobody having set the member, `StoredMetadataOptions::view` reports `enabled` for any dual-stack or ipv6 instance in any Region.

## Evidence

The modelled restriction — smithy_query(query="doc", shape="UpdateInstanceMetadataOptionsRequest", member="httpProtocolIpv6"): "Enables or disables the IPv6 endpoint for the instance metadata service. This setting applies only when the HTTP metadata endpoint is enabled." / "This parameter is available only for instances in the Europe (Stockholm) Amazon Web Services Region (<code>eu-north-1</code>)."

The handler repeats it in its own contract header — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/update_instance_metadata_options.rs:64:
```
//!   Europe (Stockholm) Amazon Web Services Region (eu-north-1).
```
and then stores the member unconditionally, src/ops/update_instance_metadata_options.rs:206-221:
```
        let selection = crate::state::MetadataOptionsSelection {
            http_tokens: input.http_tokens,
            http_endpoint: input.http_endpoint,
            http_put_response_hop_limit: input.http_put_response_hop_limit,
            http_protocol_ipv6: input.http_protocol_ipv6,
        };
...
            let applied_at_ms = instance.apply_metadata_options(selection, now);
```
(`ctx` is discarded at line 169-ish with `let _ = ctx;`-style handling of request metadata, and the instance's own `location.region_name` is never consulted.) The crate's own owner of a documented Region restriction, for comparison: src/ops/create_domain_entry.rs:170-172 `if let Some(message) = crate::state::domain_region_refusal(ctx.parts()) { return Err(invalid(message)); }`.

The projection that reports it regardless of Region — src/state.rs:4956-4962:
```
            http_protocol_ipv6: Some(self.http_protocol_ipv6.clone().unwrap_or_else(|| {
                if ip_address_type_has_ipv6(ip_address_type) {
                    HTTP_PROTOCOL_IPV6_ENABLED.to_string()
                } else {
                    HTTP_PROTOCOL_IPV6_DISABLED.to_string()
                }
            })),
```
Test coverage: tests/all/instance_metadata_options.rs drives the member in the suite's default Region and asserts it is applied (rows at :86-90, :116-124, :428-432, assertion at :277-280); its stated rule (tests/all/instance_metadata_options.rs:7-18) is only that "the call records the metadata settings the request NAMES" — it states no reason for ignoring the Region restriction, so a fix has to move those rows to eu-north-1.

## Steps to Reproduce

1. Call CreateInstances for "i1" in us-east-1 with ipAddressType="dualstack".
2. Call UpdateInstanceMetadataOptions with instanceName="i1" and httpProtocolIpv6="enabled" against the us-east-1 endpoint.
3. Observe HTTP 200 and an operation-log entry, and call GetInstance with instanceName="i1": metadataOptions.httpProtocolIpv6 is "enabled" for an instance outside eu-north-1.
4. Compare with the model sentence quoted in the handler's own header (src/ops/update_instance_metadata_options.rs:64): the parameter is available only for instances in eu-north-1, and no code path in the crate consults the instance's Region for it.
