// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE rule the published CA-certificate catalogue obeys: every
//! entry reports the certificate MATERIAL of the authority it names — that certificate's
//! own `notBefore`/`notAfter` instants as `ValidFrom`/`ValidTill`, and its SHA-1
//! fingerprint as `Thumbprint`.
//!
//! So, for each entry the catalogue publishes:
//!
//! * `ValidFrom` and `ValidTill` are instants at second resolution, ordered, and are the
//!   certificate's own: a signing instant carries a time of day, so an endpoint is never a
//!   calendar day with the clock zeroed, and two authorities signed at different moments
//!   report different windows;
//! * a `Thumbprint` is published for every entry and is a SHA-1 digest — 40 lower-case hex
//!   characters — identifying its own certificate, so no two entries publish the same
//!   digest;
//! * a published authority's validity has not passed: an authority whose certificate has
//!   expired is no longer provided, so it is published nowhere.
//!
//! The entries are read off the wire rather than restated here, so the table spans
//! whatever the catalogue publishes: both key kinds (elliptic-curve and RSA), both RSA
//! moduli, and a validity that runs for forty years beside ones that run for a hundred.
//! (The sibling `static_reference_catalogs` suite asserts the report against the
//! catalogue; this suite asserts the rule the catalogue itself has to satisfy.)

use service_com_amazonaws_docdb_amazonrdsv19::state;

use crate::cluster_lifecycle::{opt_text, text, Docdb};

/// How many characters a SHA-1 digest has, written as hex: 20 bytes, two characters each.
const SHA1_HEX_LENGTH: usize = 40;

/// Seconds in a day, for the "an endpoint is not a calendar day with the clock zeroed"
/// check below.
const SECONDS_PER_DAY: i64 = 86_400;

/// One catalogue entry as the wire reports it, read through the identifier-scoped report
/// so each answer carries exactly one `<Certificate>`.
#[derive(Debug)]
struct ReportedCertificate {
    identifier: String,
    valid_from: String,
    valid_till: String,
    /// `Thumbprint`, which every published authority carries.
    thumbprint: Option<String>,
}

/// Every published authority, as the wire reports it.
async fn published(docdb: &Docdb) -> Vec<ReportedCertificate> {
    let mut reported = Vec::new();
    for certificate in state::CA_CERTIFICATES {
        let case = format!("the published {}", certificate.identifier);
        let body = docdb
            .ok(
                &case,
                "DescribeCertificates",
                &[("CertificateIdentifier", certificate.identifier)],
            )
            .await;
        reported.push(ReportedCertificate {
            identifier: text(&body, "CertificateIdentifier"),
            valid_from: text(&body, "ValidFrom"),
            valid_till: text(&body, "ValidTill"),
            thumbprint: opt_text(&body, "Thumbprint"),
        });
    }
    reported
}

#[tokio::test]
async fn every_published_authority_reports_its_own_validity_instants() {
    let docdb = Docdb::new();
    let entries = published(&docdb).await;

    let now = state::now();
    for entry in &entries {
        let case = format!("validity of {}", entry.identifier);
        let from = state::catalog_timestamp(&entry.valid_from);
        let till = state::catalog_timestamp(&entry.valid_till);
        assert!(
            from.secs() < till.secs(),
            "case {case:?}: expected the validity to start before it ends, got {:?} .. {:?}",
            entry.valid_from,
            entry.valid_till
        );
        // A certificate is signed at a MOMENT, so its notBefore/notAfter carry a time of
        // day: an endpoint sitting exactly on midnight UTC is a calendar date with the
        // clock zeroed, i.e. a window the catalogue invented rather than the certificate's.
        // (A real authority signed exactly at midnight UTC would fail this guard; none of
        // the published ones is, and one added later that is would be the line to revisit.)
        for (member, value, secs) in [
            ("ValidFrom", &entry.valid_from, from.secs()),
            ("ValidTill", &entry.valid_till, till.secs()),
        ] {
            assert_ne!(
                secs.rem_euclid(SECONDS_PER_DAY),
                0,
                "case {case:?}: expected <{member}> to be the certificate's own instant, \
                 got the calendar day {value:?} with the clock zeroed"
            );
        }
        // An authority whose certificate has expired is no longer provided for a cluster's
        // TLS endpoint, so the catalogue publishes none.
        assert!(
            till.secs() > now.secs(),
            "case {case:?}: an authority whose certificate has expired is not published, \
             got ValidTill {:?}",
            entry.valid_till
        );
    }

    // Distinct certificates are signed at distinct moments and run for different terms,
    // so no two entries publish the same window.
    for (index, entry) in entries.iter().enumerate() {
        for other in &entries[index + 1..] {
            let case = format!("{} against {}", entry.identifier, other.identifier);
            assert_ne!(
                (&entry.valid_from, &entry.valid_till),
                (&other.valid_from, &other.valid_till),
                "case {case:?}: two authorities report one validity window, {:?} .. {:?}",
                entry.valid_from,
                entry.valid_till
            );
        }
    }
}

#[tokio::test]
async fn a_published_fingerprint_is_a_sha1_digest_of_its_own_certificate() {
    let docdb = Docdb::new();
    let entries = published(&docdb).await;

    let mut digests: Vec<(&str, &str)> = Vec::new();
    for entry in &entries {
        let case = format!("fingerprint of {}", entry.identifier);
        let thumbprint = entry.thumbprint.as_deref().unwrap_or_else(|| {
            panic!(
                "case {case:?}: every published authority publishes the fingerprint of its \
                 own certificate, got no <Thumbprint> member"
            )
        });
        assert_eq!(
            thumbprint.len(),
            SHA1_HEX_LENGTH,
            "case {case:?}: expected a SHA-1 digest of {SHA1_HEX_LENGTH} hex characters, \
             got {thumbprint:?}"
        );
        assert!(
            thumbprint
                .chars()
                .all(|character| matches!(character, '0'..='9' | 'a'..='f')),
            "case {case:?}: expected a lower-case hex digest, got {thumbprint:?}"
        );
        for (owner, published) in &digests {
            assert_ne!(
                *published, thumbprint,
                "case {case:?}: a fingerprint is a digest of ONE certificate, but {owner} \
                 publishes the same {thumbprint:?}"
            );
        }
        digests.push((entry.identifier.as_str(), thumbprint));
    }
    assert_eq!(
        digests.len(),
        state::CA_CERTIFICATES.len(),
        "expected one published fingerprint per published authority, got {digests:?}"
    );
}
