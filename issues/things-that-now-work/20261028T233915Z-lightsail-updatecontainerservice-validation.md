Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9f84f44c6767c5c874b7cc6e16fb6469b8a0f678

# A container service's publicDomainNames are stored without ever being checked to be domain names — the check every other door onto a caller-given domain in this crate applies

_Filed: 2026-10-28T23:39:15Z_
_Operation: UpdateContainerService | Class: validation_

> _Note: this issue was identified against the code base at commit `9f84f44c6767c5c874b7cc6e16fb6469b8a0f678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateContainerService` (and `CreateContainerService`, which shares the same helper) accepts the `publicDomainNames` map and passes the caller's domain lists through `crate::state::public_domain_names_from_request`, whose ONLY check is how MANY names the map carries. The names themselves are stored verbatim, so a string that is not a domain name at all (`"not a domain!!"`, `"localhost"`, `"*.example.com"`, `""`) is accepted and afterwards reported by `GetContainerServices` as one of the service's public domain names.

The crate already owns this rule: `crate::state::validate_domain_name` is the one checker of "is this string a domain name", and every OTHER door in this crate where a caller names a domain goes through it — `CreateDomain`, `CreateCertificate` (the primary domain and every `subjectAlternativeNames` entry) and the load-balancer certificate path, whose helper even calls itself "The ONE owner of the rule" and notes that going through it is also what refuses the unsupported wildcard forms, "because `*` is not a character a DNS label carries". A container service's public domain names are exactly the same kind of value — they are the domains the certificate they are keyed by secures — yet they are the one such list that skips the check.

The model cannot refuse these either: `ContainerServicePublicDomainsList`'s member is a plain `string` with no pattern and no length, so the centralized boundary walk lets anything through and the handler's own check is the only one there is. The handler's comment asserts the request-side half of the rule is fully decided where it is not: it checks only a count.

This is NOT the already-fixed `CreateContainerService` finding about resolving the certificate a group of names is keyed by (`store.unusable_public_domain_certificate`), which is about the map's KEY; this is about the VALUES under that key never being validated.

## Evidence

The skipping path — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/update_container_service.rs:210-216:
```
        let public_domain_names = input
            .public_domain_names
            .as_ref()
            .map(|requested| {
                crate::state::public_domain_names_from_request(Some(requested)).map_err(invalid)
            })
            .transpose()?;
```
and the same helper on the create path — src/ops/create_container_service.rs:231-237:
```
        // How many public domain names the request may carry is decided by the
        // request alone; WHICH certificate each group of them may be keyed by is
        // decided by the certificates this account holds, so that half of the
        // rule is asked of the store below.
        let public_domain_names =
            crate::state::public_domain_names_from_request(input.public_domain_names.as_ref())
                .map_err(invalid)?;
```

The helper itself — src/state.rs:9934-9961 — checks a COUNT and nothing else, while its own doc claims to be "the half of the rule the REQUEST decides on its own":
```
/// The public domain names `requested` carries, or a refusal when it carries more
/// than [`MAX_PUBLIC_DOMAIN_NAMES`] of them.
///
/// This is the half of the rule the REQUEST decides on its own. ...
pub fn public_domain_names_from_request(
...
    let total: usize = names.values().map(Vec::len).sum();
    if total > MAX_PUBLIC_DOMAIN_NAMES {
        return Err(format!(
            "publicDomainNames carries {total} domain names, more than the \
             {MAX_PUBLIC_DOMAIN_NAMES} a container service accepts"
        ));
    }
    Ok(names)
}
```

The sibling doors that DO apply the crate's checker:
* src/ops/create_certificate.rs:202 — `crate::state::validate_domain_name(&domain_name).map_err(invalid)?;`
* src/ops/create_certificate.rs:212-219 — `for alternative in &subject_alternative_names { crate::state::validate_domain_name(alternative).map_err(|detail| { invalid(format!("subjectAlternativeNames entry '{alternative}' is not an alternate domain of this certificate: {detail}")) })?; }`
* src/ops/create_domain.rs:176 — `crate::state::validate_domain_name(&name).map_err(invalid)?;`
* src/state.rs:10821-10849 — "The ONE owner of the rule a request describing a certificate of a load balancer has to satisfy, whichever operation carries those members: the primary domain and every alternative name have to be domain names (which is also what refuses the wildcards the documentation says are not supported, because `*` is not a character a DNS label carries)", followed by `validate_domain_name(domain_name).map_err(...)` and the same call for every alternative name.

The model leaves the handler as the only possible checker — `smithy_query(query="shape", shape="ContainerServicePublicDomainsList")`:
```
{"shape_id": "com.amazonaws.lightsail#ContainerServicePublicDomainsList", "type": "list", "member": "string"}
```
(no `pattern`, no `length`), while `CreateContainerServiceRequest$publicDomainNames` is documented as "The public domain names to use with the container service, such as example.com and www.example.com."

Test coverage: tests/all/container_service_public_domain_certificate.rs exercises this map only with well-formed names (`"held.example.com"`, `"www.held.example.com"`, `"one.example.com"`, …) and its refusal table (lines 202-259) varies only the certificate KEY — no test pins a malformed domain name as accepted, so adding the check leaves every existing test passing.

## Steps to Reproduce

1. Create and let the clock issue a certificate: CreateCertificate with certificateName="held-cert", domainName="held.example.com" (in us-east-1).
2. CreateContainerService with serviceName="svc", power/scale valid, and publicDomainNames={"held-cert": ["held.example.com"]} — succeeds.
3. UpdateContainerService with serviceName="svc" and publicDomainNames={"held-cert": ["not a domain!!", "*.example.com", "localhost"]}.
4. Observe HTTP 200: the three strings are accepted and stored, even though none is a domain name and the wildcard form is one the certificate documentation says is unsupported.
5. GetContainerServices and observe the service reporting "not a domain!!" among its publicDomainNames.
6. Contrast: CreateCertificate with domainName="not a domain!!" (or subjectAlternativeNames=["*.example.com"]) is refused with InvalidInputException by crate::state::validate_domain_name, the checker the container-service path never calls.
