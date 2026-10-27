Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 53db56bf3158c7205a85c81c618ec0b3c4193a64

# publicDomainNames are stored without resolving the certificate they are keyed by, on the strength of a comment the crate now contradicts ("CreateCertificate's business, which this emulator does not hold yet")

_Filed: 2026-10-27T05:05:22Z_
_Operation: CreateContainerService | Class: validation_

> _Note: this issue was identified against the code base at commit `53db56bf3158c7205a85c81c618ec0b3c4193a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateContainerService` and `UpdateContainerService` accept `publicDomainNames` and store it verbatim, bounding only how many domain names it carries. The comment above both call sites gives the reason: whether a public domain name is backed by a validated certificate is "`CreateCertificate`'s business, which this emulator does not hold yet".

That justification no longer holds. This crate DOES hold `CreateCertificate`: src/ops/create_certificate.rs stores a `StoredCertificate` that starts `PENDING_VALIDATION` and is owed `ISSUED` by the clock, and src/state.rs carries `attachable_certificate` — "the ONE owner of that rule" — which every other handler that records a certificate reference asks FIRST, so that "a request naming a certificate this account does not hold, or one still being validated, is refused before anything is written and no [record] is left naming a certificate that secures nothing". `CreateDistribution` and `AttachCertificateToDistribution` both call it; the container-service pair calls nothing, so a container service can be created — and updated — naming a certificate the account has never requested, or one still `PENDING_VALIDATION`, and `GetContainerServices` reports those domains as served.

This is the same shape as two findings already accepted against this crate (DeleteBucket's force-delete condition, GetContainerLog's superseded deployment): a refusal skipped "on the strength of a comment the crate now contradicts".

## Evidence

The stale justification, written twice. services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_container_service.rs:230-235:
```
        // Whether a public domain name is backed by a validated certificate is
        // `CreateCertificate`'s business, which this emulator does not hold yet,
        // so the names are kept as asked for and only their COUNT is bounded.
        let public_domain_names =
            crate::state::public_domain_names_from_request(input.public_domain_names.as_ref())
                .map_err(invalid)?;
```
src/ops/update_container_service.rs:209-218:
```
        // Whether a public domain name is backed by a validated certificate is
        // `CreateCertificate`'s business, which this emulator does not hold yet,
        // so the names are kept as asked for and only their COUNT is bounded.
        let public_domain_names = input
            .public_domain_names
...
                crate::state::public_domain_names_from_request(Some(requested)).map_err(invalid)
```
The helper bounds the COUNT and nothing else — src/state.rs:9628-9650 (`public_domain_names_from_request`): "The public domain names `requested` carries, or a refusal when it carries more than [`MAX_PUBLIC_DOMAIN_NAMES`] of them."

The crate DOES hold CreateCertificate now — src/ops/create_certificate.rs:251-259:
```
            // A certificate is not usable yet: Lightsail validates the domain
            // first — "After the certificate is valid, use the
            // AttachCertificateToDistribution action" — so the record is stored
            // PENDING_VALIDATION and the clock owes it ISSUED, which is why a
            // read straight after this call still reports the pending status and
            // an attach made straight after it is refused.
            let due =
                crate::state::DueNamedState::after(now, crate::state::CERTIFICATE_STATUS_ISSUED);
            store.insert_certificate(crate::state::StoredCertificate {
```
...and the crate's one owner of the "exists and is issued" rule — src/state.rs:13268-13279:
```
    /// The certificate `name` names, when it is one a distribution may serve
    /// HTTPS traffic with — the ONE owner of that rule.
...
    /// Every operation that records a certificate
    /// on a distribution asks here FIRST, so a request naming a certificate
    /// this account does not hold, or one still being validated, is refused
    /// before anything is written and no distribution record is left naming a
    /// certificate that secures nothing.
```
which the sibling create DOES ask — src/ops/create_distribution.rs:364-368:
```
            if let Some(certificate_name) = &input.certificate_name {
                store
                    .attachable_certificate(certificate_name)
                    .map_err(|refusal| certificate_refusal(certificate_name, refusal))?;
            }
```

Model text (smithy_query doc, CreateContainerServiceRequest$publicDomainNames): "You must create and validate an SSL/TLS certificate before you can use public domain names with your container service. Use the CreateCertificate action to create a certificate for the public domain names you want to use with your container service."

Test coverage that currently exercises the unresolved reference (no stated reason given for it): tests/all/container_service_lifecycle.rs:293 `public_domain_names: &[("site-cert", &["example.com", "www.example.com"])]` and :338-341 ("api-cert", "app-cert"), and tests/all/container_service_update.rs:286-293 — all name certificates no test ever creates; a fix has to request the certificate first (as tests/all/certificate_request.rs does) and let the clock issue it.

## Steps to Reproduce

1. Read src/ops/create_container_service.rs:230-232 and src/ops/update_container_service.rs:209-211: the certificate behind a public domain name is unchecked because "`CreateCertificate`'s business ... this emulator does not hold yet".
2. Observe that src/ops/create_certificate.rs is a fully implemented handler (it stores a certificate `PENDING_VALIDATION` and the clock owes it `ISSUED`), and that src/state.rs:13279 `attachable_certificate` is "the ONE owner" of the exists-and-is-ISSUED rule.
3. POST /ls/api/2016-11-28/container-services with {"serviceName":"svc","power":"nano","scale":1,"publicDomainNames":{"never-created-cert":["example.com"]}} — HTTP 201, and GetContainerServices reports the domain under a certificate the account does not hold.
4. Request a certificate with CreateCertificate and, without waiting for the clock to issue it, PUT the container service's `publicDomainNames` keyed by that still-`PENDING_VALIDATION` certificate — accepted, where CreateDistribution refuses the same reference through `attachable_certificate`.
