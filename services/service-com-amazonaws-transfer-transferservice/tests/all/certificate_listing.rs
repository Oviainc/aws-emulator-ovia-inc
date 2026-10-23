// SPDX-License-Identifier: CC0-1.0

//! `ListCertificates` over the whole class of stored certificate sets.
//!
//! Every call goes over the real wire path through the shared [`crate::harness`] —
//! `POST /` with `X-Amz-Target: TransferService.ListCertificates` — so what is asserted
//! is what an SDK sees.
//!
//! RULE under test: a listing reports EVERY stored certificate exactly once, in a
//! stable `CertificateId` order, projecting each record's stored `Arn`,
//! `CertificateId`, `Usage`, `Type`, `Description` and validity window, with `Status`
//! DERIVED from that window and the clock (`ACTIVE` inside `[ActiveDate,
//! InactiveDate)`, `PENDING_ROTATION` before it opens, `INACTIVE` once it has closed);
//! it answers at most `MaxResults` records per call with a `NextToken` only while more
//! remain, resumes exactly where that token points, refuses a token it did not issue
//! with `InvalidNextTokenException` (400), and changes nothing a later caller sees.
//!
//! The rows differ along what the rule generalises over: how many certificates are
//! stored (none, one, several), every value of `CertificateUsageType` and of
//! `CertificateType`, a `Description` that is absent, at the modelled `len 1..200`
//! lower bound and at its upper bound, each form of validity window (neither bound, a
//! bound at one end only, a window that is open, one not open yet and one that has
//! closed), the exact boundary instants of a window (one tick either side of each
//! bound, and the bound itself) and the page bound requested (both published bounds of
//! `MaxResults`' modelled `range 1..1000`, and every bound from 1 to two past the
//! stored count, for counts 0 to 8).
//!
//! The paging sweep is EXHAUSTIVE over those small sizes rather than sampled from a
//! generator crate, so the ordering-and-bound invariant is checked on every
//! (count, bound) pair in the range instead of a random subset of it.
//!
//! These suites SEED the store directly — through the one shared engine the handlers
//! read, see `state_roundtrip.rs` — because `ImportCertificate` is still a scaffold
//! with its own issue (`transfer-importcertificate-stub`). Nothing here bypasses the
//! handler being tested: the records are written as a certificate import will write
//! them and read back over the wire.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_transfer_transferservice::state::{
    certificate_arn, Certificate, CERTIFICATE_STATUS_ACTIVE, CERTIFICATE_STATUS_INACTIVE,
    CERTIFICATE_STATUS_PENDING_ROTATION, CERTIFICATE_TYPE_CERTIFICATE,
    CERTIFICATE_TYPE_WITH_PRIVATE_KEY,
};

use crate::harness::{
    drain_pages, is_modelled_certificate_id, member_of, unix_seconds, Answer, Emulator, ACCOUNT,
    REGION,
};

/// `CertificateUsageType`'s whole modelled value set
/// (`smithy_query(query="shape",shape="CertificateUsageType")` reports
/// `[SIGNING, ENCRYPTION, TLS]`).
const USAGES: [&str; 3] = ["SIGNING", "ENCRYPTION", "TLS"];

/// One stored certificate to seed, and the `Status` the rule says it must report.
struct CertificateCase {
    name: &'static str,
    usage: &'static str,
    certificate_type: &'static str,
    description: Option<String>,
    /// `ActiveDate` as an offset in seconds from the seeding instant, `None` for a
    /// record that stores none.
    active_offset: Option<i64>,
    /// `InactiveDate`, the same way.
    inactive_offset: Option<i64>,
    /// The `Status` the window above puts the certificate in at the seeding instant.
    status: &'static str,
}

/// A `Description` of exactly 200 characters: the upper bound of `Description`'s
/// modelled `len 1..200`, whose pattern `^[\u0021-\u007E]+$` `d` satisfies.
fn description_at_max_length() -> String {
    "d".repeat(200)
}

/// The window forms and member values a stored certificate can carry, one row each.
fn certificate_cases() -> Vec<CertificateCase> {
    vec![
        CertificateCase {
            name: "signing certificate with no window",
            usage: USAGES[0],
            certificate_type: CERTIFICATE_TYPE_CERTIFICATE,
            description: Some("as2-signing".to_string()),
            active_offset: None,
            inactive_offset: None,
            status: CERTIFICATE_STATUS_ACTIVE,
        },
        CertificateCase {
            name: "encryption key pair inside its window",
            usage: USAGES[1],
            certificate_type: CERTIFICATE_TYPE_WITH_PRIVATE_KEY,
            description: None,
            active_offset: Some(-3_600),
            inactive_offset: Some(3_600),
            status: CERTIFICATE_STATUS_ACTIVE,
        },
        CertificateCase {
            name: "tls certificate whose window has not opened",
            usage: USAGES[2],
            certificate_type: CERTIFICATE_TYPE_CERTIFICATE,
            // `Description`'s modelled lower bound, `len 1`.
            description: Some("x".to_string()),
            active_offset: Some(3_600),
            inactive_offset: Some(7_200),
            status: CERTIFICATE_STATUS_PENDING_ROTATION,
        },
        CertificateCase {
            name: "signing key pair whose window has closed",
            usage: USAGES[0],
            certificate_type: CERTIFICATE_TYPE_WITH_PRIVATE_KEY,
            description: Some(description_at_max_length()),
            active_offset: Some(-7_200),
            inactive_offset: Some(-3_600),
            status: CERTIFICATE_STATUS_INACTIVE,
        },
        CertificateCase {
            name: "encryption certificate active since the past, no end",
            usage: USAGES[1],
            certificate_type: CERTIFICATE_TYPE_CERTIFICATE,
            description: None,
            active_offset: Some(-60),
            inactive_offset: None,
            status: CERTIFICATE_STATUS_ACTIVE,
        },
        CertificateCase {
            name: "tls key pair with an end in the future only",
            usage: USAGES[2],
            certificate_type: CERTIFICATE_TYPE_WITH_PRIVATE_KEY,
            description: Some("partner-tls".to_string()),
            active_offset: None,
            inactive_offset: Some(86_400),
            status: CERTIFICATE_STATUS_ACTIVE,
        },
        CertificateCase {
            name: "signing certificate with an end in the past only",
            usage: USAGES[0],
            certificate_type: CERTIFICATE_TYPE_CERTIFICATE,
            description: None,
            active_offset: None,
            inactive_offset: Some(-1),
            status: CERTIFICATE_STATUS_INACTIVE,
        },
        CertificateCase {
            name: "encryption key pair with a start in the future only",
            usage: USAGES[1],
            certificate_type: CERTIFICATE_TYPE_WITH_PRIVATE_KEY,
            description: None,
            active_offset: Some(86_400),
            inactive_offset: None,
            status: CERTIFICATE_STATUS_PENDING_ROTATION,
        },
    ]
}

/// The seeding clock, truncated to a whole second, so every timestamp a row derives
/// from it lands on the wire as an exact integer number of seconds.
fn whole_second_now() -> SystemTime {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is past the Unix epoch")
        .as_secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// `base` shifted by `seconds`, which may be negative.
fn shift(base: SystemTime, seconds: i64) -> SystemTime {
    let magnitude = Duration::from_secs(seconds.unsigned_abs());
    if seconds < 0 {
        base - magnitude
    } else {
        base + magnitude
    }
}

/// Seed one certificate per case, exactly as a certificate import will: a minted
/// `CertificateId`, the ARN built by the crate's own [`certificate_arn`], and the
/// record's members as the row gives them. Returns the minted ids, paired with their
/// case, in `CertificateId` order.
fn seed_certificates<'a>(
    emulator: &Emulator,
    base: SystemTime,
    cases: &'a [CertificateCase],
) -> Vec<(String, &'a CertificateCase)> {
    let store = emulator.store();
    let mut guard = store.lock();
    let mut seeded = Vec::with_capacity(cases.len());
    for case in cases {
        let certificate_id = guard.mint_id("cert");
        let certificate = Certificate {
            certificate_id: certificate_id.clone(),
            arn: certificate_arn(REGION, &certificate_id),
            usage: case.usage.to_string(),
            certificate_type: case.certificate_type.to_string(),
            active_date: case.active_offset.map(|offset| shift(base, offset)),
            inactive_date: case.inactive_offset.map(|offset| shift(base, offset)),
            description: case.description.clone(),
        };
        guard.certificates.insert(certificate_id.clone(), certificate);
        seeded.push((certificate_id, case));
    }
    seeded.sort_by(|left, right| left.0.cmp(&right.0));
    seeded
}

/// Seed `count` certificates that differ only in their minted id, for the paging sweep.
/// Returns their ids in `CertificateId` order.
fn seed_plain_certificates(emulator: &Emulator, count: usize) -> Vec<String> {
    let store = emulator.store();
    let mut guard = store.lock();
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        let certificate_id = guard.mint_id("cert");
        let certificate = Certificate {
            certificate_id: certificate_id.clone(),
            arn: certificate_arn(REGION, &certificate_id),
            usage: USAGES[0].to_string(),
            certificate_type: CERTIFICATE_TYPE_CERTIFICATE.to_string(),
            active_date: None,
            inactive_date: None,
            description: None,
        };
        guard.certificates.insert(certificate_id.clone(), certificate);
        ids.push(certificate_id);
    }
    ids.sort();
    ids
}

/// One `ListCertificates` answer and its `Certificates` member.
async fn list_once(emulator: &Emulator, request: Value) -> (Answer, Vec<Value>) {
    let answer = emulator.call("ListCertificates", request).await;
    let items = answer
        .get("Certificates")
        .as_array()
        .cloned()
        .unwrap_or_default();
    (answer, items)
}

/// RULE (empty set): a store holding no certificate answers HTTP 200 with an EMPTY
/// `Certificates` list — the member is modelled required — and no `NextToken`.
#[tokio::test]
async fn an_empty_store_lists_no_certificates_and_hands_out_no_token() {
    let emulator = Emulator::new();

    // Rows: no page bound, and both published bounds of `MaxResults`' range 1..1000.
    for limit in [None, Some(1), Some(1000)] {
        let mut request = json!({});
        if let Some(limit) = limit {
            request["MaxResults"] = json!(limit);
        }
        let case = format!("MaxResults={limit:?}");
        let (answer, items) = list_once(&emulator, request).await;

        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.get("Certificates").is_array(),
            "case {case:?}: Certificates is required, got {}",
            answer.body
        );
        assert!(
            items.is_empty(),
            "case {case:?}: expected no certificates, got {items:?}"
        );
        assert!(
            answer.get("NextToken").is_null(),
            "case {case:?}: expected no token for an empty listing, got {}",
            answer.body
        );
    }
}

/// RULE: the listing projects each stored record — its `Arn`, `CertificateId`, `Usage`,
/// `Type`, `Description` and validity window — and derives `Status` from that window,
/// in `CertificateId` order, once per certificate.
#[tokio::test]
async fn the_listing_projects_every_stored_certificate() {
    let emulator = Emulator::new();
    let base = whole_second_now();
    let cases = certificate_cases();
    let seeded = seed_certificates(&emulator, base, &cases);

    let (answer, items) = list_once(&emulator, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "expected 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        items.len(),
        seeded.len(),
        "expected one entry per stored certificate, got {items:?}"
    );
    assert!(
        answer.get("NextToken").is_null(),
        "a listing that fits one page must hand back no token, got {}",
        answer.body
    );

    for (entry, (certificate_id, case)) in items.iter().zip(&seeded) {
        let name = case.name;
        assert_eq!(
            member_of(entry, "CertificateId"),
            &json!(certificate_id),
            "case {name:?}: expected the certificates in CertificateId order, got {entry}"
        );
        assert!(
            is_modelled_certificate_id(certificate_id),
            "case {name:?}: expected a CertificateId matching ^cert-([0-9a-f]{{17}})$, \
             got {certificate_id:?}"
        );
        // The ARN is spelled out here rather than taken from the crate's own helper,
        // so the listing is checked against the canonical ARN form and not against
        // whatever the handler happens to build.
        assert_eq!(
            member_of(entry, "Arn"),
            &json!(format!(
                "arn:aws:transfer:{REGION}:{ACCOUNT}:certificate/{certificate_id}"
            )),
            "case {name:?}: unexpected Arn, got {entry}"
        );
        assert_eq!(
            member_of(entry, "Usage"),
            &json!(case.usage),
            "case {name:?}: expected Usage {:?}, got {entry}",
            case.usage
        );
        assert_eq!(
            member_of(entry, "Type"),
            &json!(case.certificate_type),
            "case {name:?}: expected Type {:?}, got {entry}",
            case.certificate_type
        );
        match &case.description {
            Some(description) => assert_eq!(
                member_of(entry, "Description"),
                &json!(description),
                "case {name:?}: expected the stored Description, got {entry}"
            ),
            None => assert!(
                member_of(entry, "Description").is_null(),
                "case {name:?}: a record with no Description must report none, got {entry}"
            ),
        }
        for (member, offset) in [
            ("ActiveDate", case.active_offset),
            ("InactiveDate", case.inactive_offset),
        ] {
            match offset {
                Some(offset) => assert_eq!(
                    member_of(entry, member).as_f64(),
                    Some(unix_seconds(shift(base, offset))),
                    "case {name:?}: expected {member} at {offset}s from the seeding \
                     instant, got {entry}"
                ),
                None => assert!(
                    member_of(entry, member).is_null(),
                    "case {name:?}: a record with no {member} must report none, got {entry}"
                ),
            }
        }
        assert_eq!(
            member_of(entry, "Status"),
            &json!(case.status),
            "case {name:?}: expected Status {:?} for this validity window, got {entry}",
            case.status
        );
    }
}

/// RULE: a listing returns at most `MaxResults` certificates, hands back a `NextToken`
/// only while more remain, and resuming from that token walks every stored certificate
/// exactly once in `CertificateId` order.
///
/// The sweep is exhaustive over 0 to 8 stored certificates and every page bound from 1
/// to two past the stored count, plus no bound and the modelled maximum.
#[tokio::test]
async fn list_certificates_honours_every_page_bound_and_resumes_from_its_token() {
    for count in 0..=8usize {
        let emulator = Emulator::new();
        let ids = seed_plain_certificates(&emulator, count);

        let mut limits: Vec<Option<i64>> = vec![None, Some(1000)];
        for bound in 1..=(count + 2) {
            limits.push(Some(i64::try_from(bound).expect("a small page bound")));
        }

        for limit in limits {
            let case = format!("{count} stored, MaxResults={limit:?}");
            let (page_sizes, listed) = drain_pages(
                &emulator,
                "ListCertificates",
                &json!({}),
                "Certificates",
                "CertificateId",
                limit,
            )
            .await;

            assert_eq!(
                listed, ids,
                "case {case:?}: expected every certificate once in CertificateId order, \
                 got {listed:?}"
            );
            match limit {
                Some(limit) => {
                    let limit = usize::try_from(limit).expect("a positive page bound");
                    for page in &page_sizes {
                        assert!(
                            *page <= limit,
                            "case {case:?}: a page carried {page} certificates, above \
                             the bound {limit}"
                        );
                    }
                    let want_pages = count.div_ceil(limit).max(1);
                    assert_eq!(
                        page_sizes.len(),
                        want_pages,
                        "case {case:?}: expected {want_pages} page(s), got {page_sizes:?}"
                    );
                }
                None => assert_eq!(
                    page_sizes,
                    vec![count],
                    "case {case:?}: expected a single unbounded page, got {page_sizes:?}"
                ),
            }
        }
    }
}

/// RULE: a `NextToken` the paging codec cannot have issued is refused with
/// `InvalidNextTokenException` (400) — the listing is never silently restarted — a
/// `MaxResults` outside the modelled `range 1..1000` is refused at the boundary while
/// both published bounds are accepted, and a well-formed token naming a position past
/// the last certificate resumes after it rather than starting over.
#[tokio::test]
async fn list_certificates_refuses_an_unissued_token_and_a_bad_page_bound() {
    let emulator = Emulator::new();
    seed_plain_certificates(&emulator, 3);

    // Rows: not hex at all, odd-length hex, and valid hex whose bytes are not UTF-8.
    for token in ["not-a-token", "zz", "abc", "c0ffee00", "ff"] {
        let case = format!("NextToken={token:?}");
        let (answer, _) = list_once(&emulator, json!({"NextToken": token})).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException, got {}",
            answer.body
        );
    }

    // One PAST each published bound of `MaxResults`' modelled range 1..1000.
    for bad in [0, 1001] {
        let (answer, _) = list_once(&emulator, json!({"MaxResults": bad})).await;
        assert_eq!(
            answer.status, 400,
            "case \"MaxResults={bad}\": expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
    }

    // Both published bounds themselves are accepted.
    for good in [1, 1000] {
        let (answer, _) = list_once(&emulator, json!({"MaxResults": good})).await;
        assert_eq!(
            answer.status, 200,
            "case \"MaxResults={good}\": expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
    }

    // A token the codec could have issued that names a position past every stored id
    // is a POSITION, not a lookup: the listing resumes after it, which is an empty
    // last page and no further token. `7a7a` is the hex of "zz", which sorts after
    // every `cert-` id.
    let (answer, items) = list_once(&emulator, json!({"NextToken": "7a7a"})).await;
    assert_eq!(
        answer.status, 200,
        "case \"NextToken past the end\": expected HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        items.is_empty(),
        "case \"NextToken past the end\": expected an empty page, got {items:?}"
    );
    assert!(
        answer.get("NextToken").is_null(),
        "case \"NextToken past the end\": expected no further token, got {}",
        answer.body
    );
}

/// RULE: the listing REPORTS state — reading it, with any page bound and over any
/// number of pages, leaves every stored record exactly as it was, so a later caller
/// sees the same listing.
#[tokio::test]
async fn listing_certificates_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let base = whole_second_now();
    let cases = certificate_cases();
    let seeded = seed_certificates(&emulator, base, &cases);

    let (_, before) = list_once(&emulator, json!({})).await;
    let stored_before: Vec<String> = emulator.store().lock().certificates.keys().cloned().collect();

    // Read it again, unpaged and paged one record at a time.
    let (_, again) = list_once(&emulator, json!({})).await;
    assert_eq!(
        again, before,
        "expected a repeated listing to answer the same records, got {again:?}"
    );
    let (_, drained) = drain_pages(
        &emulator,
        "ListCertificates",
        &json!({}),
        "Certificates",
        "CertificateId",
        Some(1),
    )
    .await;
    let ids_before: Vec<String> = seeded
        .iter()
        .map(|(certificate_id, _)| certificate_id.clone())
        .collect();
    assert_eq!(
        drained, ids_before,
        "expected the paged walk to answer the same certificates, got {drained:?}"
    );

    let stored_after: Vec<String> = emulator.store().lock().certificates.keys().cloned().collect();
    assert_eq!(
        stored_after, stored_before,
        "expected the stored certificates to be untouched by a read, got {stored_after:?}"
    );
    let (_, after) = list_once(&emulator, json!({})).await;
    assert_eq!(
        after, before,
        "expected the listing after the reads to match the first one, got {after:?}"
    );
}

/// The `Status` the listing reports for the ONE stored certificate.
async fn listed_status(emulator: &Emulator) -> String {
    let (answer, items) = list_once(emulator, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "expected 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        items.len(),
        1,
        "expected the one stored certificate to be listed, got {items:?}"
    );
    member_of(&items[0], "Status")
        .as_str()
        .unwrap_or_else(|| panic!("expected a Status on the listed certificate, got {items:?}"))
        .to_string()
}

/// RULE (the clock, not a caller, moves `Status`): a certificate whose window opens in
/// the future lists as `PENDING_ROTATION` and becomes `ACTIVE` once the clock is inside
/// the window, with no call in between that touches the record.
///
/// One transition over the wire, with a window that opens 500 ms from now and never
/// closes, so the observation before it has 500 ms of margin and the one after it
/// 500 ms, and the whole test is done in about a second. The exact window bounds, and
/// the `INACTIVE` phase, are pinned without sleeping by
/// [`the_status_rule_is_a_half_open_window`]; that every phase reaches the WIRE is
/// pinned by the projection table above, which lists records in all three statuses.
#[tokio::test]
async fn a_certificates_status_follows_the_clock_into_its_window() {
    let emulator = Emulator::new();

    let now = SystemTime::now();
    {
        let store = emulator.store();
        let mut guard = store.lock();
        let certificate_id = guard.mint_id("cert");
        let certificate = Certificate {
            certificate_id: certificate_id.clone(),
            arn: certificate_arn(REGION, &certificate_id),
            usage: USAGES[2].to_string(),
            certificate_type: CERTIFICATE_TYPE_CERTIFICATE.to_string(),
            active_date: Some(now + Duration::from_millis(500)),
            inactive_date: None,
            description: None,
        };
        guard.certificates.insert(certificate_id, certificate);
    }

    let before_the_window = listed_status(&emulator).await;
    assert_eq!(
        before_the_window, CERTIFICATE_STATUS_PENDING_ROTATION,
        "case \"before the window opens\": expected {CERTIFICATE_STATUS_PENDING_ROTATION}, \
         got {before_the_window:?}"
    );

    tokio::time::sleep(Duration::from_millis(1_000)).await;
    let inside_the_window = listed_status(&emulator).await;
    assert_eq!(
        inside_the_window, CERTIFICATE_STATUS_ACTIVE,
        "case \"inside the window\": expected {CERTIFICATE_STATUS_ACTIVE}, got \
         {inside_the_window:?}"
    );
}

/// RULE (the window's exact bounds): `Status` is `ACTIVE` on the HALF-OPEN window
/// `[ActiveDate, InactiveDate)` — active from the instant the window opens, inactive
/// from the instant it closes — `PENDING_ROTATION` strictly before it opens, and an
/// absent bound leaves that end of the window open.
///
/// The rows are the boundary instants themselves, asserted against the record's own
/// derivation at an instant the test names, so no row depends on wall-clock margins:
/// one tick before each bound, the bound itself, and one tick after.
#[test]
fn the_status_rule_is_a_half_open_window() {
    let tick = Duration::from_nanos(1);
    let opens = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let closes = opens + Duration::from_secs(86_400);

    let bounded = Certificate {
        certificate_id: "cert-00000000000000001".to_string(),
        arn: certificate_arn(REGION, "cert-00000000000000001"),
        usage: USAGES[0].to_string(),
        certificate_type: CERTIFICATE_TYPE_CERTIFICATE.to_string(),
        active_date: Some(opens),
        inactive_date: Some(closes),
        description: None,
    };
    let open_start = Certificate {
        active_date: None,
        ..bounded.clone()
    };
    let open_end = Certificate {
        inactive_date: None,
        ..bounded.clone()
    };
    let unbounded = Certificate {
        active_date: None,
        inactive_date: None,
        ..bounded.clone()
    };

    // (case, the record, the instant, the expected status)
    let rows: [(&str, &Certificate, SystemTime, &str); 11] = [
        (
            "one tick before the window opens",
            &bounded,
            opens - tick,
            CERTIFICATE_STATUS_PENDING_ROTATION,
        ),
        (
            "the instant the window opens",
            &bounded,
            opens,
            CERTIFICATE_STATUS_ACTIVE,
        ),
        (
            "one tick after the window opens",
            &bounded,
            opens + tick,
            CERTIFICATE_STATUS_ACTIVE,
        ),
        (
            "one tick before the window closes",
            &bounded,
            closes - tick,
            CERTIFICATE_STATUS_ACTIVE,
        ),
        (
            "the instant the window closes",
            &bounded,
            closes,
            CERTIFICATE_STATUS_INACTIVE,
        ),
        (
            "one tick after the window closes",
            &bounded,
            closes + tick,
            CERTIFICATE_STATUS_INACTIVE,
        ),
        (
            "no ActiveDate, long before the end",
            &open_start,
            opens - Duration::from_secs(86_400),
            CERTIFICATE_STATUS_ACTIVE,
        ),
        (
            "no ActiveDate, at the end",
            &open_start,
            closes,
            CERTIFICATE_STATUS_INACTIVE,
        ),
        (
            "no InactiveDate, before the start",
            &open_end,
            opens - tick,
            CERTIFICATE_STATUS_PENDING_ROTATION,
        ),
        (
            "no InactiveDate, long after the start",
            &open_end,
            closes + Duration::from_secs(86_400),
            CERTIFICATE_STATUS_ACTIVE,
        ),
        (
            "neither bound",
            &unbounded,
            opens,
            CERTIFICATE_STATUS_ACTIVE,
        ),
    ];

    for (case, certificate, at, expected) in rows {
        let actual = certificate.status(at);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected}, got {actual:?}"
        );
    }
}
