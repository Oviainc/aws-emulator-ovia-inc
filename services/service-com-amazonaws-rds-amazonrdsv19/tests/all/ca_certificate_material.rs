// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE rule the published CA-certificate catalog obeys: every
//! entry reports the certificate MATERIAL of the authority it names — that certificate's
//! own `notBefore`/`notAfter` instants as `ValidFrom`/`ValidTill`, and its SHA-1
//! fingerprint as `Thumbprint` — and reports the same material on every path that
//! projects the entry.
//!
//! So, for each entry the catalog publishes:
//!
//! * `ValidFrom` and `ValidTill` are instants at second resolution, ordered, and are the
//!   certificate's own: a signing instant carries a time of day, so an endpoint is never a
//!   calendar day with the clock zeroed, and two authorities signed at different moments
//!   report different windows;
//! * a `Thumbprint` is published for every entry and is a SHA-1 digest — 40 lower-case
//!   hex characters — identifying its own certificate, so no two entries publish the same
//!   digest;
//! * the whole-catalog read, the identifier-scoped read and `ModifyCertificates` project
//!   one row, so each reports the same window and the same fingerprint for it;
//! * a published authority's validity has not passed: an authority whose certificate has
//!   expired is no longer provided, so it is published nowhere and a read for it is the
//!   modelled 404 (the sibling `query_catalog_reads` suite pins that refusal).
//!
//! The entries are read off the wire rather than restated here, so the table spans
//! whatever the catalog publishes: both key kinds (elliptic-curve and RSA), both RSA
//! moduli, and a validity that runs for forty years beside ones that run for a hundred.
//! (The sibling `static_reference_catalogs` suite pins the published VALUES literally;
//! this suite pins the rule they have to satisfy.)
//!
//! Every call goes over the wire through the harness in [`crate::db_instances`].

use service_com_amazonaws_rds_amazonrdsv19::state;

use crate::db_instances::{call, emulator, expect_ok, optional_text, subtrees, text};

/// A call that carries no parameters beyond `Action` and `Version`.
const NO_PARAMS: &[(&str, &str)] = &[];

/// Seconds in a day, for the "an endpoint is not a calendar day with the clock zeroed"
/// check below.
const SECONDS_PER_DAY: i64 = 86_400;

/// How many characters a SHA-1 digest has, written as hex: 20 bytes, two characters each.
const SHA1_HEX_LENGTH: usize = 40;

/// One catalog entry as the wire reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedCertificate {
    identifier: String,
    valid_from: String,
    valid_till: String,
    /// `Thumbprint`, which every published authority carries.
    thumbprint: Option<String>,
}

/// Read every `<Certificate>` of one answer.
fn reported(body: &str) -> Vec<ReportedCertificate> {
    subtrees(body, "Certificate")
        .iter()
        .map(|certificate| ReportedCertificate {
            identifier: text(certificate, "CertificateIdentifier"),
            valid_from: text(certificate, "ValidFrom"),
            valid_till: text(certificate, "ValidTill"),
            thumbprint: optional_text(certificate, "Thumbprint"),
        })
        .collect()
}

/// The instant `value` names, parsed by the one catalogue-instant parser this service
/// holds (`state::catalog_timestamp`), which panics on anything that is not an ISO-8601
/// instant.
fn instant(value: &str) -> ::aws_smithy_types::DateTime {
    state::catalog_timestamp(value)
}

#[tokio::test]
async fn every_published_authority_reports_its_own_validity_instants() {
    let router = emulator();
    let body = expect_ok(
        "the published catalog",
        call(&router, "DescribeCertificates", NO_PARAMS).await,
    );
    let entries = reported(&body);
    assert_eq!(
        entries.len(),
        state::CA_CERTIFICATES.len(),
        "expected every published authority to be reported, got {entries:?} in {body}"
    );

    for entry in &entries {
        let case = format!("validity of {}", entry.identifier);
        let from = instant(&entry.valid_from);
        let till = instant(&entry.valid_till);
        assert!(
            from.secs() < till.secs(),
            "case {case:?}: expected the validity to start before it ends, got {:?} .. {:?}",
            entry.valid_from,
            entry.valid_till
        );
        assert_eq!(
            (from.subsec_nanos(), till.subsec_nanos()),
            (0, 0),
            "case {case:?}: a certificate's validity endpoints are instants at second \
             resolution, got {:?} .. {:?}",
            entry.valid_from,
            entry.valid_till
        );
        // A certificate is signed at a MOMENT, so its notBefore/notAfter carry a time of
        // day: an endpoint sitting exactly on midnight UTC is a calendar date with the
        // clock zeroed, i.e. a window the catalog invented rather than the certificate's.
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

    // An authority whose certificate has expired is no longer provided for a database's
    // TLS endpoint, so the catalog publishes none: every window reported here is still
    // open now.
    let now = ::std::time::SystemTime::now()
        .duration_since(::std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs() as i64);
    for entry in &entries {
        let case = format!("validity of {}", entry.identifier);
        let till = instant(&entry.valid_till);
        assert!(
            till.secs() > now,
            "case {case:?}: an authority whose certificate has expired is not published, \
             got ValidTill {:?}",
            entry.valid_till
        );
    }
}

#[tokio::test]
async fn a_published_fingerprint_is_a_sha1_digest_of_its_own_certificate() {
    let router = emulator();
    let body = expect_ok(
        "the published catalog",
        call(&router, "DescribeCertificates", NO_PARAMS).await,
    );
    let entries = reported(&body);

    let mut digests: Vec<(&str, &str)> = Vec::new();
    for entry in &entries {
        let case = format!("fingerprint of {}", entry.identifier);
        let thumbprint = entry.thumbprint.as_deref().unwrap_or_else(|| {
            panic!(
                "case {case:?}: every published authority publishes the fingerprint of its \
                 own certificate, got no <Thumbprint> member in {body}"
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
        "expected one published fingerprint per published authority, got {digests:?} in \
         {body}"
    );
}

#[tokio::test]
async fn one_row_is_projected_onto_every_path_that_reports_it() {
    let router = emulator();
    let catalog = reported(&expect_ok(
        "the published catalog",
        call(&router, "DescribeCertificates", NO_PARAMS).await,
    ));

    for entry in &catalog {
        // A fresh emulator per row, so each override is set on an account holding none.
        let router = emulator();
        // The identifier "isn't case-sensitive", so a non-canonical spelling reads the
        // same row; the pin path projects it too.
        for (path, spelling) in [
            ("DescribeCertificates", entry.identifier.clone()),
            (
                "DescribeCertificates",
                entry.identifier.to_ascii_uppercase(),
            ),
            ("ModifyCertificates", entry.identifier.clone()),
        ] {
            let case = format!("{} as {spelling} through {path}", entry.identifier);
            let body = expect_ok(
                &case,
                call(
                    &router,
                    path,
                    &[("CertificateIdentifier", spelling.as_str())],
                )
                .await,
            );
            let projected = reported(&body);
            assert_eq!(
                projected.as_slice(),
                std::slice::from_ref(entry),
                "case {case:?}: expected the catalog row {entry:?}, got {projected:?} in \
                 {body}"
            );
        }
    }
}
