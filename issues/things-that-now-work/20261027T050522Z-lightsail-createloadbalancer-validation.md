Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 53db56bf3158c7205a85c81c618ec0b3c4193a64

# CreateLoadBalancer stores a load-balancer TLS certificate with neither the domain-name check nor the nine-alternative-name quota its sibling CreateLoadBalancerTlsCertificate applies to the same three members

_Filed: 2026-10-27T05:05:22Z_
_Operation: CreateLoadBalancer | Class: validation_

> _Note: this issue was identified against the code base at commit `53db56bf3158c7205a85c81c618ec0b3c4193a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both `CreateLoadBalancer` and `CreateLoadBalancerTlsCertificate` build the SAME stored record (`crate::state::StoredLoadBalancerCertificate`, held on the load balancer's `certificates` list and read back by `DeleteLoadBalancerTlsCertificate`/`GetLoadBalancerTlsCertificates`) out of the same three members: `certificateName`, `certificateDomainName` and `certificateAlternativeNames`. Both reduce the alternative names through the same de-dupe helper `crate::state::alternative_domain_names`.

The sibling then applies two further checks that `CreateLoadBalancer` skips:

1. `crate::state::validate_domain_name` on `certificateDomainName` and on every alternative name — the check whose comment says it is also "what refuses the wildcards the documentation says are not supported". `DomainName`/`DomainNameList` carry NO modelled constraint (smithy_query: `{"DomainName": {"type": "string"}}`), so nothing else refuses them: `CreateLoadBalancer` happily stores `certificateDomainName: "*.example.com"`, or a value that is not a domain name at all.
2. The de-duped list is held to `crate::state::MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES` (9) — a constant whose own doc calls it "An AWS quota". `CreateLoadBalancer` applies ONLY the emulator's pre-de-dupe guard `MAX_DOMAIN_NAME_LIST_ENTRIES` (100), whose doc says it is "Emulator resource guard, NOT an AWS quota". So one `CreateLoadBalancer` call can store a certificate record covering up to 100 distinct alternative domains — a record `CreateLoadBalancerTlsCertificate` would have refused.

The handler's own comment claims the opposite of what the code does: it says the record it stores "carries the same shape as one `CreateLoadBalancerTlsCertificate` stores".

## Evidence

The crate's claim, in CreateLoadBalancer — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_load_balancer.rs:252-260:
```
        // "Lightsail will de-dupe the names for you" — reduced through the one
        // owner of that rule, so the certificate this call stores carries the
        // same shape as one `CreateLoadBalancerTlsCertificate` stores.
        let alternative_names = certificate
            .as_ref()
            .map(|(_, domain_name)| {
                crate::state::alternative_domain_names(domain_name, requested_alternatives)
            })
            .unwrap_or_default();
```
Everything CreateLoadBalancer does to those members is the entry-count guard — src/ops/create_load_balancer.rs:242-251:
```
        // How many entries the list may carry is bounded before any of them is
        // read, because the de-duping below examines every one of them.
        if requested_alternatives.len() > crate::state::MAX_DOMAIN_NAME_LIST_ENTRIES {
            return Err(invalid(format!(
                "certificateAlternativeNames carries {} entries, more than the {} this service \
                 reads for one certificate",
```
...and it then stores the record — src/ops/create_load_balancer.rs:343-349:
```
                certificates: certificate
                    .map(|(certificate_name, domain_name)| {
                        crate::state::StoredLoadBalancerCertificate {
                            name: certificate_name,
                            domain_name,
                            alternative_names,
```

The sibling, on the same three members — src/ops/create_load_balancer_tls_certificate.rs:199-250:
```
        crate::state::validate_domain_name(&domain_name).map_err(|detail| {
            invalid(format!(
                "certificateDomainName '{domain_name}' is not the domain name of a \
                 certificate: {detail}"
            ))
        })?;
...
        for requested in &requested_alternatives {
            crate::state::validate_domain_name(requested).map_err(|detail| {
...
        let alternative_names =
            crate::state::alternative_domain_names(&domain_name, requested_alternatives);
        if alternative_names.len()
            > crate::state::MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES
        {
            return Err(invalid(format!(
                "certificateAlternativeNames names {} alternative domains, more than the {} a \
                 certificate may have in addition to its primary domain",
```

The two bounds, and which one is the AWS rule — src/state.rs:10414-10433:
```
/// An AWS quota too, from the modelled
/// `CreateLoadBalancerTlsCertificate$certificateAlternativeNames`
/// documentation: "You can have a maximum of 9 alternative names (in addition
/// to the 1 primary domain)."
pub const MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES: usize = 9;
...
/// Emulator resource guard, NOT an AWS quota, and the bound that makes the
/// de-duping affordable
pub const MAX_DOMAIN_NAME_LIST_ENTRIES: usize = 100;
```

Model text for the sibling's member (smithy_query doc, CreateLoadBalancerTlsCertificateRequest$certificateAlternativeNames): "Lightsail will de-dupe the names for you. You can have a maximum of 9 alternative names (in addition to the 1 primary domain). We do not support wildcards (*.example.com)."

Test coverage: tests/all/load_balancer_provisioning.rs:656-693 is CreateLoadBalancer's refusal table — it covers only the certificate member PAIRING, the orphan alternative-name list and the blank health-check path; no row pins acceptance of an invalid domain name or of more than nine de-duped alternative names. tests/all/load_balancer_tls_certificate_create.rs:136-139 and load_balancer_tls_certificate_delete.rs:186 exercise the sibling's checks.

## Steps to Reproduce

1. Open src/ops/create_load_balancer_tls_certificate.rs:199-250 and note the two checks on `certificateDomainName`/`certificateAlternativeNames`: `validate_domain_name` for each name, then `MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES` (9) applied to the de-duped list.
2. Open src/ops/create_load_balancer.rs:231-260 and note that neither check is there: only `MAX_DOMAIN_NAME_LIST_ENTRIES` (100, documented as "NOT an AWS quota") is applied, and the names go straight into `alternative_domain_names`.
3. POST /ls/api/2016-11-28/CreateLoadBalancer with {"loadBalancerName":"lb","instancePort":80,"certificateName":"c","certificateDomainName":"*.example.com","certificateAlternativeNames":["a1.example.com", ... 20 distinct names ...]} — it is accepted, and the stored certificate carries a wildcard primary domain plus 20 alternative names.
4. POST the same certificate members to CreateLoadBalancerTlsCertificate against an existing load balancer — they are refused with InvalidInputException naming `certificateDomainName` and the nine-name maximum.
5. Compare: the same stored `StoredLoadBalancerCertificate` shape is reachable through one operation and refused through the other, contradicting the comment at create_load_balancer.rs:252-254.
