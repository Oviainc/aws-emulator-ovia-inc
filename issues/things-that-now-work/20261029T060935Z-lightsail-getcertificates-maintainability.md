Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 6cada3bea20bb0dfad4860907220259726a5f31e

# The certificate listing assembles its own certificate view instead of the store's declared single renderer, so the two copies of that assembly can drift

_Filed: 2026-10-29T06:09:34Z_
_Operation: GetCertificates | Class: maintainability_

> _Note: this issue was identified against the code base at commit `6cada3bea20bb0dfad4860907220259726a5f31e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::ComAmazonawsLightsailLightsail20161128State::certificate_view` declares itself "The one way a reporting operation renders a certificate" and pairs `StoredCertificate::view` with `certificate_in_use_resource_count`, because the in-use count has to be computed from the whole store rather than from the certificate record. `GetCertificates` uses it on the named-certificate path, but the LISTING path re-assembles the same pair inline for every record of the page. The two copies compute the same answer today, so this is a drift hazard rather than a behavioural gap: a change to how a reported certificate is assembled (another kind of resource counted as in use, another member derived at render time) has to be made twice, and only one of the two is the one the state module says owns the rule.

The fix is to call the existing `store.certificate_view(name, include_details)` from the page's `view_of` closure (or have the closure delegate to it), leaving every response byte and every existing test unchanged.

## Evidence

The declared single owner — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:14531-14544:
```
    /// The certificate `name` names as the modelled `CertificateSummary`, with
    /// the resources it is attached to counted.
    ///
    /// The one way a reporting operation renders a certificate: the reference
    /// lives on the record of the resource that holds it — a distribution, or a
    /// container service whose public domain names it keys — so assembling the
    /// view needs the whole store, not just the certificate.
    pub fn certificate_view(&self, name: &str, include_details: bool) -> Option<CertificateSummary> {
        let certificate = self.certificates.get(name)?;
        Some(certificate.view(
            self.certificate_in_use_resource_count(name),
            include_details,
        ))
    }
```

The path that calls it — src/ops/get_certificates.rs:223-228:
```
                let reported: Vec<CertificateSummary> =
                    if matches_status(Some(certificate.status)) {
                        store
                            .certificate_view(name, include_details)
                            .into_iter()
                            .collect()
```

The path that re-implements it — src/ops/get_certificates.rs:238-251:
```
            let (certificates, next_page_token) = crate::state::resource_page(
                ...
                crate::state::records_from(&store.certificates, &page_token)
                    .filter(|(_, certificate)| matches_status(Some(certificate.status))),
                crate::state::CERTIFICATE_PAGE_SIZE,
                |name, certificate| {
                    certificate.view(
                        store.certificate_in_use_resource_count(name),
                        include_details,
                    )
                },
            );
```

Test coverage for both copies: tests/all/certificate_request.rs:359 (`requesting_a_certificate_stores_the_record_every_later_read_reports`, which reads the record with and without `includeCertificateDetails` — see the comment at tests/all/certificate_request.rs:566) and tests/all/certificate_request.rs:125-130 (`all_certificate_names`, which walks the LISTING path page by page), plus tests/all/certificate_in_use_resources.rs:88-98, whose helper doc says `inUseResourceCount` is read "through `GetCertificates`, which is the member the delete's gate and the client both read".

## Steps to Reproduce

1. Read src/state.rs:14531-14544: `certificate_view` is documented as "The one way a reporting operation renders a certificate".
2. Read src/ops/get_certificates.rs:223-228 (named branch) — it calls that helper.
3. Read src/ops/get_certificates.rs:238-251 (listing branch) — it calls `certificate.view(store.certificate_in_use_resource_count(name), include_details)` itself, duplicating the helper's body.
4. Change what a rendered certificate carries (for example count another kind of resource as in use) and observe that only one of the two assembly sites is reached from the state module's declared owner, so the listing can drift from the single-record read.
