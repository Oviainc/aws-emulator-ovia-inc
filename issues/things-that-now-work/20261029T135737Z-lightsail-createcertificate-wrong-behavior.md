Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 02c390dd8e861c3d98b16a550da31d4601fa546f

# CreateCertificate stores subjectAlternativeNames verbatim, skipping the de-dupe reduction the crate declares the ONE owner of — so a certificate publishes its own primary domain as an alternate domain and reports more alternate names than it has validation records

_Filed: 2026-10-29T13:57:37Z_
_Operation: CreateCertificate | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `02c390dd8e861c3d98b16a550da31d4601fa546f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::alternative_domain_names` is declared the single owner of the "a repeat, and a name that already IS the primary domain, add no domain to the certificate" reduction, "so every operation that stores a certificate's alternative domains stores the same shape". `CreateCertificate` is an operation that stores a certificate's alternative domains and it does NOT go through it: it bounds the RAW list, validates each entry as a domain name, and then stores the list byte-for-byte (src/ops/create_certificate.rs:203-219, 267).

Three consequences, all observable:

1. A certificate can be stored whose `subjectAlternativeNames` repeats a name, or repeats its own `domainName` — exactly the shape the crate documents as impossible for the other certificate kind it holds ("every writer of this field goes through that one owner, so a repeat and the primary domain are never among them", src/state.rs:11088-11091).

2. The SAME `Certificate` response then disagrees with itself: `subjectAlternativeNames` is reported raw from the record (src/state.rs:12196) while `domainValidationRecords` is built from the de-duped `domain_names()` (src/state.rs:12156-12163, 12201-12202). A certificate asked for with `domainName "a.example.com"` and `subjectAlternativeNames ["a.example.com","b.example.com","b.example.com"]` reports three alternative names and two validation records. `AttachCertificateToDistribution` publishes the de-duped set too (src/ops/attach_certificate_to_distribution.rs:216,240), so the attached distribution and the certificate disagree about what the certificate covers.

3. The one documented sentence the two certificate paths share — "a maximum of nine alternate domains (in addition to the primary domain name)" — is enforced two different ways in this one crate. The load-balancer path applies the maximum to what the reduction LEAVES (src/state.rs:10983-10991), so eleven entries that de-dupe to nine distinct domains are accepted there; `CreateCertificate` applies it to `subject_alternative_names.len()`, so the equivalent request is refused 400, while nine copies of the primary domain (which add zero alternate domains) are accepted and stored as nine alternate domains.

The defect is the skipped reduction, not the number: whichever list the maximum is counted on, the crate states that reduction has one owner and this handler bypasses it.

## Evidence

The rule and its declared single owner — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:10916-10932:
```
/// The alternative domains a certificate actually covers, given the primary
/// domain it is for and the names a request listed.
///
/// "Lightsail will de-dupe the names for you": a name the list repeats, and a
/// name that already IS the certificate's primary domain, add no domain to the
/// certificate. The ONE owner of that reduction, so every operation that
/// stores a certificate's alternative domains stores the same shape. The walk
/// is bounded by [`MAX_DOMAIN_NAME_LIST_ENTRIES`], which every caller checks
/// before reading the list.
pub fn alternative_domain_names(primary: &str, requested: Vec<String>) -> Vec<String> {
    let mut covered: Vec<String> = Vec::new();
    for name in requested {
        if name != primary && !covered.contains(&name) {
            covered.push(name);
        }
    }
    covered
}
```

The handler that stores a certificate's alternative domains without it — src/ops/create_certificate.rs:203-219:
```
        let subject_alternative_names = input.subject_alternative_names.unwrap_or_default();
        if subject_alternative_names.len() > crate::state::MAX_SUBJECT_ALTERNATIVE_NAMES {
            return Err(invalid(format!(
                "subjectAlternativeNames carries {} entries, more than the {} alternate \
                 domains a certificate may have",
                subject_alternative_names.len(),
                crate::state::MAX_SUBJECT_ALTERNATIVE_NAMES
            )));
        }
        for alternative in &subject_alternative_names {
            crate::state::validate_domain_name(alternative).map_err(|detail| {
                invalid(format!(
                    "subjectAlternativeNames entry '{alternative}' is not an alternate \
                     domain of this certificate: {detail}"
                ))
            })?;
        }
```
and the raw list going straight onto the record — src/ops/create_certificate.rs:266-267:
```
                domain_name,
                subject_alternative_names,
```
(the field is documented as kept raw: src/state.rs:12128-12130, "The alternate domains and subdomains the certificate also covers, kept in the order the request named them.")

The sibling path, which applies the SAME documented maximum to the de-duped list — src/state.rs:10983-10991:
```
    let covered = alternative_domain_names(domain_name, requested);
    if covered.len() > MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES {
        return Err(format!(
            "certificateAlternativeNames names {} alternative domains, more than the \
             {MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES} a certificate may have in \
             addition to its primary domain",
            covered.len()
        ));
    }
```
and the invariant it buys, which the account certificate breaks — src/state.rs:11088-11091:
```
    /// The alternative domains and subdomains the certificate also covers, as
    /// [`alternative_domain_names`] reduced them — every writer of this field
    /// goes through that one owner, so a repeat and the primary domain are
    /// never among them.
    pub alternative_names: Vec<String>,
```

The one response disagreeing with itself — src/state.rs:12196 reports the stored list raw:
```
            subject_alternative_names: Some(self.subject_alternative_names.clone()),
```
while src/state.rs:12201-12202 builds the validation records from the de-duped set:
```
            domain_validation_records: Some(
                self.domain_names()
```
with `domain_names()` re-implementing the reduction inline — src/state.rs:12156-12163:
```
    pub fn domain_names(&self) -> Vec<String> {
        let mut names = vec![self.domain_name.clone()];
        for alternative in &self.subject_alternative_names {
            if !names.iter().any(|name| name == alternative) {
                names.push(alternative.clone());
            }
        }
        names
    }
```

The model text both paths quote — smithy_query(query="doc", shape="CreateCertificateRequest", member="subjectAlternativeNames"): "You can specify a maximum of nine alternate domains (in addition to the primary domain name)." and smithy_query(query="doc", shape="CreateLoadBalancerTlsCertificateRequest", member="certificateAlternativeNames"): "Lightsail will de-dupe the names for you. You can have a maximum of 9 alternative names (in addition to the 1 primary domain)."

Test coverage, both sides. The load-balancer side pins the de-duped behaviour: tests/all/load_balancer_tls_certificate_create.rs:571-577 is the ACCEPTED row
```
    AlternativesRow {
        case: "eleven entries that de-dupe to the nine the maximum admits",
        load_balancer: "dedupe-lb-4",
        certificate: "dedupe-cert-4",
        sent: ELEVEN_WITH_NINE_DISTINCT,
        accepted: true,
    },
```
and tests/all/load_balancer_tls_certificate_read.rs:199-202 pins "primary domain among them, which Lightsail de-dupes away". The account-certificate suite has NO such case: tests/all/certificate_request.rs:762-768 refuses only "one alternate domain more than the documented nine" (ten DISTINCT names) and lines 807-825 accept nine distinct names, so neither a repeat nor the primary domain among the alternates is exercised anywhere — the current behaviour is not pinned by any test.

## Steps to Reproduce

1. Call CreateCertificate in us-east-1 with certificateName "dup-cert", domainName "a.example.com" and subjectAlternativeNames ["a.example.com","b.example.com","b.example.com"] — answered 200.
2. Read the `certificate.certificateDetail` of that answer (or GetCertificates with includeCertificateDetails true): `subjectAlternativeNames` is ["a.example.com","b.example.com","b.example.com"] — it repeats a name and lists the certificate's own primary domain as an alternate domain, the shape src/state.rs:11088-11091 says a certificate of this crate never holds.
3. In the SAME response, `domainValidationRecords` carries only two entries (a.example.com, b.example.com), because it is built from the de-duped `domain_names()` (src/state.rs:12201-12202): the one certificate reports three alternative names and two validated domains.
4. Wait for the certificate to be ISSUED, CreateDistribution, then AttachCertificateToDistribution with it: the distribution's `alternativeDomainNames` is the de-duped set (src/ops/attach_certificate_to_distribution.rs:216,240), so the distribution and the certificate disagree about what the certificate covers.
5. The limit asymmetry: CreateCertificate with ten subjectAlternativeNames entries naming only nine DISTINCT domains is refused 400 (src/ops/create_certificate.rs:204-211 counts the raw list), while CreateLoadBalancerTlsCertificate with eleven certificateAlternativeNames entries naming nine distinct domains is accepted (src/state.rs:10983-10991 counts the de-duped list; pinned accepted at tests/all/load_balancer_tls_certificate_create.rs:571-577) — one documented sentence, two rules in one crate.
6. Mirror case: CreateCertificate with domainName "c.example.com" and subjectAlternativeNames set to nine copies of "c.example.com" is accepted and stored as nine alternate domains, although the reduction says those nine add no domain at all.
