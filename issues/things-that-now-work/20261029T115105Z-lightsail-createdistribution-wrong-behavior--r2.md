Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: abe0d61d6a18bd2197d540196c8b7c6e43f8cd4b

# A distribution created with a certificateName names that certificate but publishes none of its domains, breaking the state module's own "never one without the other" rule for the attachment pair

_Filed: 2026-10-29T11:51:05Z_
_Operation: CreateDistribution | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `abe0d61d6a18bd2197d540196c8b7c6e43f8cd4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDistribution` stores the certificate the request names as the distribution's attached certificate (`certificate_name: input.certificate_name.clone()`, after resolving it through `store.issued_certificate(..)` — the same ISSUED check the attach path applies) but writes `alternative_domain_names: Vec::new()`.

The crate's state module declares the opposite rule for exactly this pair of members: `StoredDistribution::detach_certificate` is documented as "the ONE owner of that rule" and says the two members are "never one without the other", because "the domains it accepts HTTPS traffic for come from the certificate attached to it". A distribution created with `certificateName` is left in precisely the state that doc says cannot exist: `certificateName` set, `alternativeDomainNames` empty.

The crate's two other doors onto the same member write both halves: `AttachCertificateToDistribution` sets `certificate_name` and `alternative_domain_names = certificate.domain_names()`, and `UpdateDistribution` does the same for its own `certificateName` input (its comment: "the same attachment `AttachCertificateToDistribution` makes"). So two routes to the same stored attachment report different distributions: Create(certificateName=c) reports no alternate domain names, while Create() + Attach(c) — or Update(certificateName=c) — reports the certificate's domains. Every other consumer of the record treats the create-named certificate as attached (it is the single `certificateName` the attach handler calls "ONE attached certificate", and the certificate counts as in use for `DeleteCertificate`), so only the published-domains half is missing.

The empty list IS pinned by a test, but with a stated reason the model contradicts: the test says the create does not make the attachment, while the modelled input member says it does ("the SSL/TLS certificate that you want to attach to the distribution").

## Evidence

Model (smithy_query doc, CreateDistributionRequest$certificateName):
> "The name of the SSL/TLS certificate that you want to **attach** to the distribution."

Model (AttachCertificateToDistribution, quoted by the handler at services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/attach_certificate_to_distribution.rs:212-215):
> "After the certificate is attached, your distribution accepts HTTPS traffic for all of the domains that are associated with the certificate."

The crate's own invariant — src/state.rs:12711-12730:
```
/// Release the SSL/TLS certificate this distribution serves HTTPS traffic
/// with, so it names none and publishes none of that certificate's domains —
/// the ONE owner of that rule.
///
/// A distribution that serves no certificate of its own publishes no
/// alternate domain names: the domains it accepts HTTPS traffic for come
/// from the certificate attached to it ("your distribution accepts HTTPS
/// traffic for all of the domains that are associated with the
/// certificate"), so releasing the attachment clears BOTH members, never
/// one without the other. ... (The reverse direction —
/// a certificate the request named at CREATE time, which
/// `CreateDistribution` records without publishing its domains — is that
/// operation's own business, not this release's.)
pub fn detach_certificate(&mut self) {
    self.certificate_name = None;
    self.alternative_domain_names = Vec::new();
}
```

The handler that breaks it — src/ops/create_distribution.rs:393-404:
```
// The certificate the request named is recorded as asked for,
// after the store confirmed it is one a distribution may serve
// HTTPS traffic with.
certificate_name: input.certificate_name.clone(),
// The alternate domain names of a distribution are the domains
// of its attached certificate, and
// `AttachCertificateToDistribution` is what attaches one and
// publishes them — so a distribution this call created reports
// none of its own.
alternative_domain_names: Vec::new(),
```

The siblings that write both halves — src/ops/attach_certificate_to_distribution.rs:216 and :239-240:
```
let alternative_domain_names = certificate.domain_names();
...
distribution.certificate_name = Some(certificate_name.clone());
distribution.alternative_domain_names = alternative_domain_names;
```
and src/ops/update_distribution.rs:447-453:
```
match (input.certificate_name, attached_domain_names) {
    // A certificate of the account, with the domains it covers — the
    // same attachment `AttachCertificateToDistribution` makes.
    (Some(certificate_name), Some(domain_names)) => {
        distribution.certificate_name = Some(certificate_name);
        distribution.alternative_domain_names = domain_names;
    }
```

Test coverage: tests/all/distribution_create_certificate_validation.rs:447-458 pins the empty list, and states the reason the model sentence above contradicts:
```
// The domains a certificate covers are published on the distribution by
// `AttachCertificateToDistribution`, which is the operation that makes
// the attachment, so a create publishes none — pinned in
// `distribution_certificate_attachment.rs`.
let published = created["alternativeDomainNames"] ...
assert_eq!(published, 0, ...)
```
tests/all/distribution_certificate_attachment.rs:641-656 pins the attach path publishing the certificate's domains, and tests/all/distribution_update.rs:424 pins the update path doing the same.

## Steps to Reproduce

1. CreateCertificate with certificateName "cert", domainName "example.com" and subjectAlternativeNames ["www.example.com"]; poll GetCertificates until its status is ISSUED.
2. CreateInstances to make an origin instance, waiting for it to settle to running.
3. CreateDistribution with distributionName "dist", that instance as the origin, a published bundleId, and certificateName "cert" (from us-east-1) — 200.
4. Read the answered `distribution` (and a later GetDistributions for "dist"): `certificateName` is "cert" but `alternativeDomainNames` is empty, so the distribution names an attached certificate while publishing none of the domains that attachment accepts HTTPS traffic for.
5. Compare the other two doors onto the same member: CreateDistribution "dist2" with no certificateName, then AttachCertificateToDistribution("dist2", "cert") — or UpdateDistribution("dist2", certificateName="cert") — reports alternativeDomainNames ["example.com", "www.example.com"] for the same stored attachment.
