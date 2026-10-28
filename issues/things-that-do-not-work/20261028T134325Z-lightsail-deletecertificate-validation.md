Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# The certificate in-use count ignores container services, so a certificate securing a container service's public domain names is deleted and reported as attached to 0 resources

_Filed: 2026-10-28T13:43:25Z_
_Operation: DeleteCertificate | Class: validation_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteCertificate` refuses a certificate that is still in use by asking `certificate_in_use_resource_count`, and that helper counts DISTRIBUTIONS only. A container service's `publicDomainNames` map is keyed by the NAME of the certificate that secures the domains under it, and the crate enforces that linkage on the way in: `CreateContainerService` and `UpdateContainerService` both refuse a key that does not name an issued certificate of the account, through `unusable_public_domain_certificate`, whose own doc says the point is that "a refused request leaves no container service reporting a domain under a certificate that secures nothing".

Because the count ignores those services, the delete produces exactly the state the create/update path exists to prevent: the certificate is removed while a container service still reports `publicDomainNames` keyed by its name, so the service serves custom domains under a certificate the account no longer holds, and `UpdateContainerService` would now refuse the very configuration the service is reporting. The same under-count is reported to clients as `Certificate$inUseResourceCount`, documented as "The number of Lightsail resources that the certificate is attached to": a certificate used by a container service reports 0.

## Evidence

The count the delete trusts — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:14140-14153:
```
    /// How many resources of this account the certificate `name` is attached to,
    /// which is what `Certificate$inUseResourceCount` reports.
    ...
    pub fn certificate_in_use_resource_count(&self, name: &str) -> i32 {
        let count = self
            .distributions
            .values()
            .filter(|distribution| distribution.certificate_name.as_deref() == Some(name))
            .count();
```
where it is used as the whole in-use gate — src/ops/delete_certificate.rs:201-209:
```
            let in_use = store.certificate_in_use_resource_count(&name);
            if in_use > 0 {
                return Err(invalid(format!(
                    "The certificate {name} is attached to {in_use} resource(s) of this \
                     account. ...
```
The rule the OTHER side of the same linkage enforces — src/state.rs:14095-14104:
```
    /// A container service's `publicDomainNames` is keyed by the NAME of the
    /// SSL/TLS certificate that secures the domains listed under it, so the
    /// certificate that key names answers the same exists-and-issued rule a
    /// distribution's certificate does ... Whichever operation records those names
    /// asks here BEFORE its first write, so a refused request leaves no
    /// container service reporting a domain under a certificate that secures
    /// nothing.
    pub fn unusable_public_domain_certificate(
```
called by src/ops/update_container_service.rs:246-251 and src/ops/create_container_service.rs:283.

And `remove_certificate`'s own claim, which only holds for distributions — src/state.rs:14129-14133:
```
    /// distribution is a resource of its own, and whether any distribution
    /// still serves this certificate is what the caller asks
    /// [`Self::certificate_in_use_resource_count`] BEFORE removing it, so no
    /// distribution is ever left naming a certificate that secures nothing.
```
The modelled member the count feeds — smithy_query(query="doc", shape="Certificate", member="inUseResourceCount"): "The number of Lightsail resources that the certificate is attached to."

Test coverage: tests/all/certificate_delete.rs asserts only the attached-to-nothing (`inUseResourceCount` 0, src/../certificate_delete.rs:488-493) and distribution-attached cases, and tests/all/container_service_public_domain_certificate.rs never calls DeleteCertificate — so no test pins the container-service case either way.

## Steps to Reproduce

1. In us-east-1, CreateCertificate {certificateName: "cs-cert", domainName: "www.example.com"} and let the clock settle it to ISSUED.
2. CreateContainerService {serviceName: "svc", power: "nano", scale: 1, publicDomainNames: {"cs-cert": ["www.example.com"]}} -> HTTP 200 (the certificate key is resolved and accepted).
3. GetCertificates {certificateName: "cs-cert", includeCertificateDetails: true} -> certificateDetail.inUseResourceCount is 0, although a container service serves domains under it.
4. DeleteCertificate {certificateName: "cs-cert"} -> HTTP 200 (the in-use gate counted distributions only).
5. GetContainerServices {serviceName: "svc"} -> publicDomainNames still keyed by "cs-cert", a certificate GetCertificates now answers DoesNotExist for; re-sending the same publicDomainNames through UpdateContainerService is refused, so the service reports a configuration the crate's own write path forbids.
