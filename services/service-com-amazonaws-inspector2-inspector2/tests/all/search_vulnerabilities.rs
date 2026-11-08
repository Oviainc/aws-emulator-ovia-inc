// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `SearchVulnerabilities`.
//!
//! RULE: a search answers the record the vulnerability database holds for the requested
//! CVE id — at most one, because the modelled `Vulnerabilities` list is `len 0..1` — and an
//! EMPTY list for a well-formed id the database holds no record for; it reports records and
//! changes nothing.
//!
//! The table every case walks is the shipped database itself
//! (`state::VULNERABILITY_CATALOG`), so no case here writes down what a record says: what
//! is asserted is that the wire answer agrees with the published record the store was
//! seeded from, member by member. One case pins a record's data against the NVD figures
//! written down in this file, so the suite is not purely self-referential.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_inspector2_inspector2::state::VULNERABILITY_CATALOG;
use smithy_server_runtime::MultiServiceRouter;

use crate::filters_and_enablement::{router, send_in};

/// The route `SearchVulnerabilities` is bound to.
/// Source: `smithy_query(query="operation", operation="SearchVulnerabilities")`.
const URI: &str = "/vulnerabilities/search";

/// Send one search, optionally resuming at `next_token`.
async fn search(
    router: &MultiServiceRouter,
    ids: &[&str],
    next_token: Option<&str>,
) -> (u16, Option<String>, Value) {
    let mut body = json!({ "filterCriteria": { "vulnerabilityIds": ids } });
    if let Some(token) = next_token {
        body["nextToken"] = json!(token);
    }
    send_in(router, "POST", "us-east-1", URI, Some(&body)).await
}

/// The epoch seconds an RFC 3339 instant from the shipped database renders as on the wire:
/// restJson1 carries a timestamp with no `@timestampFormat` as epoch seconds.
fn epoch_seconds(rfc3339: &str) -> f64 {
    ::aws_smithy_types::DateTime::from_str(
        rfc3339,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| panic!("{rfc3339:?} is not an RFC 3339 instant: {error}"))
    .secs() as f64
}

/// A reported string list, or `None` when the member is absent.
fn list(value: &Value) -> Option<Vec<&str>> {
    value
        .as_array()
        .map(|values| values.iter().filter_map(Value::as_str).collect())
}

/// Every record the database holds is answered for its own id, with the published data the
/// store was seeded from and no page cursor — the whole shipped catalog as the table, so
/// every record is covered rather than one.
#[tokio::test]
async fn every_catalogued_vulnerability_is_answered_with_its_published_record() {
    let router = router();
    assert!(
        VULNERABILITY_CATALOG.len() > 1,
        "the shipped vulnerability database must hold records to search, got {}",
        VULNERABILITY_CATALOG.len()
    );

    for record in VULNERABILITY_CATALOG {
        let case = record.id;
        let (status, error_type, body) = search(&router, &[record.id], None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} / {error_type:?} / {body}"
        );

        let reported = body["vulnerabilities"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no vulnerabilities list in {body}"));
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one record, got {reported:?}"
        );
        let got = &reported[0];

        assert_eq!(
            got["id"].as_str(),
            Some(record.id),
            "case {case:?}: expected the requested id, got {got}"
        );
        assert_eq!(
            got["source"].as_str(),
            Some("NVD"),
            "case {case:?}: expected the NVD source, got {got}"
        );
        assert_eq!(
            got["sourceUrl"].as_str(),
            Some(format!("https://nvd.nist.gov/vuln/detail/{case}").as_str()),
            "case {case:?}: expected the record's NVD detail page, got {got}"
        );
        assert_eq!(
            got["description"].as_str(),
            Some(record.description),
            "case {case:?}: expected the published description, got {got}"
        );
        assert_eq!(
            got["vendorSeverity"].as_str(),
            Some(record.vendor_severity),
            "case {case:?}: expected the published severity, got {got}"
        );

        // A list the record carries nothing for is left ABSENT rather than reported empty.
        for (member, published) in [
            ("cwes", record.cwes),
            ("relatedVulnerabilities", record.related_vulnerabilities),
            ("referenceUrls", record.reference_urls),
            ("detectionPlatforms", record.detection_platforms),
        ] {
            let expected = (!published.is_empty()).then(|| published.to_vec());
            assert_eq!(
                list(&got[member]),
                expected,
                "case {case:?}: unexpected {member}, got {got}"
            );
        }

        // One record carries a CVSS v2 rating, one a v4 one and every record a v3 one: a
        // scoring version the published record was never rated under is absent, which is a
        // different answer from a base score of zero.
        for (member, published) in [
            ("cvss2", record.cvss2),
            ("cvss3", record.cvss3),
            ("cvss4", record.cvss4),
        ] {
            match published {
                Some(score) => {
                    assert_eq!(
                        got[member]["baseScore"].as_f64(),
                        Some(score.base),
                        "case {case:?}: unexpected {member} base score, got {got}"
                    );
                    assert_eq!(
                        got[member]["scoringVector"].as_str(),
                        Some(score.vector),
                        "case {case:?}: unexpected {member} vector, got {got}"
                    );
                }
                None => assert!(
                    got[member].is_null(),
                    "case {case:?}: {member} must be absent for a record never rated under \
                     it, got {got}"
                ),
            }
        }

        assert_eq!(
            got["vendorCreatedAt"].as_f64(),
            Some(epoch_seconds(record.published)),
            "case {case:?}: unexpected vendorCreatedAt, got {got}"
        );
        assert_eq!(
            got["vendorUpdatedAt"].as_f64(),
            Some(epoch_seconds(record.last_modified)),
            "case {case:?}: unexpected vendorUpdatedAt, got {got}"
        );

        match record.kev {
            Some(kev) => {
                assert_eq!(
                    got["cisaData"]["dateAdded"].as_f64(),
                    Some(epoch_seconds(kev.date_added)),
                    "case {case:?}: unexpected cisaData.dateAdded, got {got}"
                );
                assert_eq!(
                    got["cisaData"]["dateDue"].as_f64(),
                    Some(epoch_seconds(kev.date_due)),
                    "case {case:?}: unexpected cisaData.dateDue, got {got}"
                );
                assert!(
                    got["cisaData"]["action"]
                        .as_str()
                        .is_some_and(|action| !action.is_empty()),
                    "case {case:?}: a catalogued entry states a required action, got {got}"
                );
            }
            None => assert!(
                got["cisaData"].is_null(),
                "case {case:?}: cisaData must be absent for a CVE the KEV catalog does not \
                 hold, got {got}"
            ),
        }

        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: one id can match at most one record, so no page follows, got {body}"
        );
    }
}

/// The data one record is answered with, pinned against the figures NVD publishes for it
/// rather than against the store the answer came out of.
///
/// Source: the NVD record for CVE-2014-0160 (the OpenSSL heartbeat read overflow) —
/// CVSS v3.1 base 7.5 `AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:N/A:N`, CVSS v2 base 5.0
/// `AV:N/AC:L/Au:N/C:P/I:N/A:N`, CWE-125, published 2014-04-07.
#[tokio::test]
async fn a_records_data_is_the_published_nvd_data() {
    let router = router();
    let (status, _, body) = search(&router, &["CVE-2014-0160"], None).await;
    assert_eq!(status, 200, "expected 200, got {status} / {body}");

    let got = &body["vulnerabilities"][0];
    assert_eq!(
        got["id"].as_str(),
        Some("CVE-2014-0160"),
        "expected the requested id, got {body}"
    );
    assert_eq!(
        got["cvss3"]["baseScore"].as_f64(),
        Some(7.5),
        "expected the published CVSS v3.1 base score, got {body}"
    );
    assert_eq!(
        got["cvss3"]["scoringVector"].as_str(),
        Some("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:N/A:N"),
        "expected the published CVSS v3.1 vector, got {body}"
    );
    assert_eq!(
        got["cvss2"]["baseScore"].as_f64(),
        Some(5.0),
        "expected the published CVSS v2 base score, got {body}"
    );
    assert_eq!(
        got["cvss2"]["scoringVector"].as_str(),
        Some("AV:N/AC:L/Au:N/C:P/I:N/A:N"),
        "expected the published CVSS v2 vector, got {body}"
    );
    assert_eq!(
        list(&got["cwes"]),
        Some(vec!["CWE-125"]),
        "expected the published CWE classification, got {body}"
    );
    assert_eq!(
        got["vendorCreatedAt"].as_f64(),
        Some(epoch_seconds("2014-04-07T22:55:00Z")),
        "expected the published date, got {body}"
    );
    assert_eq!(
        got["vendorSeverity"].as_str(),
        Some("HIGH"),
        "expected the published severity, got {body}"
    );
}

/// A well-formed id the database holds no record for is answered 200 with an EMPTY list and
/// no page cursor — not refused, and not answered with a record carrying no data.
///
/// The rows differ along what the id form generalises over: the century the year starts
/// with, the length of the sequence number at both modelled bounds (1 and 10 digits), and
/// an id one digit away from a catalogued one.
#[tokio::test]
async fn an_id_the_database_holds_no_record_for_is_answered_with_an_empty_list() {
    let router = router();
    let cases = [
        "CVE-1999-0001",
        "CVE-2000-1",
        "CVE-2015-1234567890",
        "CVE-1999-9999999999",
        "CVE-2021-44229",
    ];

    for case in cases {
        assert!(
            !VULNERABILITY_CATALOG.iter().any(|record| record.id == case),
            "case {case:?}: this row must name an id the shipped database does not hold"
        );
        let (status, error_type, body) = search(&router, &[case], None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} / {error_type:?} / {body}"
        );
        assert_eq!(
            list(&body["vulnerabilities"]),
            Some(Vec::new()),
            "case {case:?}: expected an empty list, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: nothing matched, so no page follows, got {body}"
        );
    }
}

/// An id outside the modelled `VulnId` pattern, and a `vulnerabilityIds` list outside the
/// modelled length, are refused 400 `ValidationException` rather than answered.
///
/// The rows differ along which part of the id form the value breaks (case, year width,
/// century, sequence width, another id scheme, emptiness) and along which modelled bound
/// the list breaks (below the minimum of one id, above the maximum of one).
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let router = router();
    let malformed_ids = [
        ("a lowercase prefix", "cve-2021-44228"),
        ("a two-digit year", "CVE-21-44228"),
        ("a year outside the modelled centuries", "CVE-3021-44228"),
        ("no sequence number", "CVE-2021-"),
        ("a sequence longer than ten digits", "CVE-2021-12345678901"),
        ("another advisory id scheme", "GHSA-jfh8-c2jp-5v3q"),
        ("an empty id", ""),
    ];

    for (case, id) in malformed_ids {
        let (status, error_type, body) = search(&router, &[id], None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} / {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {error_type:?} / {body}"
        );
        let message = body["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {body}"));
        assert!(
            message.contains("CVE-[12][0-9]{3}-[0-9]{1,10}"),
            "case {case:?}: the refusal must quote the expression every id must match, got \
             {message:?}"
        );
    }

    // The list's own modelled bounds: `VulnIdList` is `len 1..1`.
    let list_cases: [(&str, Vec<&str>); 3] = [
        ("no id at all", vec![]),
        (
            "two ids where the model admits one",
            vec!["CVE-2021-44228", "CVE-2014-0160"],
        ),
        (
            "two ids, one of them unknown",
            vec!["CVE-2021-44228", "CVE-2015-1234567890"],
        ),
    ];
    for (case, ids) in list_cases {
        let (status, error_type, body) = search(&router, &ids, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} / {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {error_type:?} / {body}"
        );
    }

    // And the required `filterCriteria` member itself.
    let (status, error_type, body) =
        send_in(&router, "POST", "us-east-1", URI, Some(&json!({}))).await;
    assert_eq!(
        status, 400,
        "an absent filterCriteria: expected 400, got {status} / {body}"
    );
    assert_eq!(
        error_type.as_deref(),
        Some("ValidationException"),
        "an absent filterCriteria: expected ValidationException, got {error_type:?} / {body}"
    );
}

/// A `nextToken` this operation did not hand out is refused, and one it could have hands
/// back the records ordered after the position it names: a cursor at or past the requested
/// id leaves nothing to report.
#[tokio::test]
async fn a_page_cursor_names_a_position_or_is_refused() {
    let router = router();
    let requested = "CVE-2021-44228";

    let refused = [
        ("a token that is not even hex", "not-a-token".to_string()),
        ("hex that decodes to no id", ::hex::encode("nonsense")),
        (
            "hex of a resource ARN rather than an id",
            ::hex::encode("arn:aws:inspector2:us-east-1:000000000000:filter/abc"),
        ),
        (
            "hex of a malformed id",
            ::hex::encode("cve-2021-44228"),
        ),
    ];
    for (case, token) in refused {
        let (status, error_type, body) = search(&router, &[requested], Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} / {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {error_type:?} / {body}"
        );
    }

    // Rows differ along where the cursor sits relative to the requested id: before it, at
    // it, and after it.
    let served: [(&str, &str, usize); 3] = [
        ("a cursor before the requested id", "CVE-2014-0160", 1),
        ("a cursor at the requested id", requested, 0),
        ("a cursor past the requested id", "CVE-2024-3094", 0),
    ];
    for (case, cursor, expected) in served {
        let token = ::hex::encode(cursor);
        let (status, error_type, body) = search(&router, &[requested], Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} / {error_type:?} / {body}"
        );
        let reported = body["vulnerabilities"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no vulnerabilities list in {body}"));
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: expected {expected} record(s), got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: one id can match at most one record, so no page follows, got {body}"
        );
    }
}

/// Searching reports records and changes nothing a later caller sees: the same search
/// repeated, and interleaved with searches for an unknown and for another catalogued id,
/// answers the same thing every time.
#[tokio::test]
async fn searching_does_not_change_what_a_later_caller_sees() {
    let router = router();
    let first = VULNERABILITY_CATALOG[0].id;
    let second = VULNERABILITY_CATALOG[VULNERABILITY_CATALOG.len() - 1].id;

    let (_, _, baseline) = search(&router, &[first], None).await;
    for round in 0..3 {
        for other in [second, "CVE-2000-1"] {
            let (status, _, body) = search(&router, &[other], None).await;
            assert_eq!(
                status, 200,
                "round {round}, {other:?}: expected 200, got {status} / {body}"
            );
        }
        let (status, _, again) = search(&router, &[first], None).await;
        assert_eq!(
            status, 200,
            "round {round}: expected 200, got {status} / {again}"
        );
        assert_eq!(
            again, baseline,
            "round {round}: a read must not change what a later search answers, got {again}"
        );
    }
}
