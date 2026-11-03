// SPDX-License-Identifier: CC0-1.0

//! The SPELLING of a document path both of this service's document planes publish under.
//!
//! One rule is pinned here, over a SET of published documents and a SET of spellings
//! rather than any one of them: **a percent-encoded spelling of a path segment names the
//! same document as the character it escapes, so every equivalent spelling of an announced
//! document URL serves that document — and a spelling that is NOT equivalent (an escaped
//! `/`, which is a character of a segment and not a separator, or an escape naming another
//! character) serves none.** An escaped octet and the octet it escapes are the same data
//! (RFC 3986 §2.1), and a client, SDK or proxy may re-spell a segment on the way, so a URL
//! this emulator announced has to resolve however it is written.
//!
//! The rows span both planes — the report and term documents `GetReport` and
//! `GetTermForReport` announce, and the exported document `ExportComplianceInquiry`
//! announces — and, for each, every segment of the published path (the fixed prefix
//! segments, the ids, the version, the fixed `export` segment and the object), escaped
//! wholly, escaped in its first character only, and all segments escaped at once. A rule
//! that held on only one plane, or on only the variable segments, fails here.
//!
//! Every case goes through the router, so each exercises the same path a client takes:
//! claiming, routing, the handler, the persisted state and serialization.

use bytes::Bytes;
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    a_report_requiring_acceptance, call, emulator, follow_announced_url, get, ENDPOINT,
};

/// One document this suite follows the announced URL of: the case it is named by, and the
/// URL the control plane announced it at.
struct Announced {
    case: String,
    url: String,
}

/// The announced URL split into the part before the path, the path, and the query string
/// that carries the receipt authorising the fetch.
struct Parts {
    path: String,
    query: String,
}

/// Split an announced URL into the path whose spelling this suite varies and the query it
/// keeps unchanged — the receipt is not what is under test here.
fn parts_of(case: &str, url: &str) -> Parts {
    let rest = url.strip_prefix(&format!("https://{ENDPOINT}")).unwrap_or_else(|| {
        panic!("case {case:?}: an announced URL names the caller's endpoint, got {url:?}")
    });
    let (path, query) = rest.split_once('?').unwrap_or_else(|| {
        panic!("case {case:?}: an announced URL carries its receipt, got {url:?}")
    });
    Parts {
        path: path.to_string(),
        query: query.to_string(),
    }
}

/// The URL for `path`, under the receipt the announcement carried.
fn url_for(parts: &Parts, path: &str) -> String {
    format!("https://{ENDPOINT}{path}?{}", parts.query)
}

/// Every byte of `segment` written as a percent-escape, in upper-case hex: the most
/// re-spelled form of a segment there is, and still the same segment (RFC 3986 §2.1).
fn escape_whole(segment: &str) -> String {
    segment
        .bytes()
        .map(|byte| format!("%{byte:02X}"))
        .collect()
}

/// `segment` with only its FIRST character escaped, in lower-case hex: hex digits are
/// case-insensitive, so this names the same segment too.
fn escape_first(segment: &str) -> String {
    let mut characters = segment.chars();
    match characters.next() {
        Some(first) => {
            let escaped: String = first
                .to_string()
                .bytes()
                .map(|byte| format!("%{byte:02x}"))
                .collect();
            format!("{escaped}{}", characters.as_str())
        }
        None => segment.to_string(),
    }
}

/// The path with `segment` at `index` replaced.
fn with_segment(segments: &[String], index: usize, replacement: String) -> String {
    let mut rebuilt = segments.to_vec();
    rebuilt[index] = replacement;
    rebuilt.join("/")
}

/// Every equivalent re-spelling of one announced path: for each non-empty segment, that
/// segment wholly escaped and that segment escaped in its first character only, plus the
/// path with every segment wholly escaped at once.
fn equivalent_spellings(path: &str) -> Vec<(String, String)> {
    let segments: Vec<String> = path.split('/').map(str::to_string).collect();
    let mut rows = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            continue;
        }
        rows.push((
            format!("segment {index} ({segment:?}) wholly escaped"),
            with_segment(&segments, index, escape_whole(segment)),
        ));
        rows.push((
            format!("segment {index} ({segment:?}) escaped in its first character"),
            with_segment(&segments, index, escape_first(segment)),
        ));
    }
    let all: Vec<String> = segments
        .iter()
        .map(|segment| escape_whole(segment))
        .collect();
    rows.push(("every segment wholly escaped".to_string(), all.join("/")));
    rows
}

/// Spellings that are NOT equivalent to the announced path, so the plane must serve no
/// document for them: an escaped separator (which is a character INSIDE a segment and
/// never a delimiter), an escape naming another character, and a malformed escape, which
/// is no escape at all.
fn inequivalent_spellings(path: &str) -> Vec<(String, String)> {
    let (parent, object) = path
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("a document path names an object, got {path:?}"));
    vec![
        (
            "the separator before the object escaped".to_string(),
            format!("{parent}%2F{object}"),
        ),
        (
            "another character escaped into the object".to_string(),
            format!("{parent}/{object}%41"),
        ),
        (
            "a malformed escape in the object".to_string(),
            format!("{parent}/{object}%zz"),
        ),
        (
            "an escaped extra segment after the object".to_string(),
            format!("{path}/%65xtra"),
        ),
    ]
}

/// Every document this suite drives both planes over: the term and report documents of a
/// report whose download requires the explicit acceptance of its term (so the path is one
/// a receipt authorises), at its first and its newest published version, and the document
/// exported from an inquiry this suite creates.
async fn announced(router: &MultiServiceRouter) -> Vec<Announced> {
    let mut rows = Vec::new();

    let report = a_report_requiring_acceptance();
    let mut versions = vec![
        report.versions.first().copied().unwrap_or(1),
        report.latest(),
    ];
    versions.dedup();
    for version in versions {
        let term = get(
            router,
            &format!(
                "/v1/report/getTermForReport?reportId={}&reportVersion={version}",
                report.id
            ),
        )
        .await;
        assert_eq!(
            term.status, 200,
            "GetTermForReport on {} v{version} must be served, got {} with {:?}",
            report.id, term.status, term.body
        );
        let token = term.body["termToken"]
            .as_str()
            .unwrap_or_else(|| panic!("GetTermForReport carries a termToken: {:?}", term.body))
            .to_string();
        rows.push(Announced {
            case: format!("the term document of {} v{version}", report.id),
            url: term.body["documentPresignedUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("GetTermForReport announces a URL: {:?}", term.body))
                .to_string(),
        });

        let download = get(
            router,
            &format!(
                "/v1/report/get?reportId={}&reportVersion={version}&termToken={token}",
                report.id
            ),
        )
        .await;
        assert_eq!(
            download.status, 200,
            "GetReport on {} v{version} must be served under its term grant, got {} with {:?}",
            report.id, download.status, download.body
        );
        rows.push(Announced {
            case: format!("the report document of {} v{version}", report.id),
            url: download.body["documentPresignedUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("GetReport announces a URL: {:?}", download.body))
                .to_string(),
        });
    }

    let created = call(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(
            json!({
                "name": "Document path spelling",
                "inquiryContent": { "query": "Does the SOC 2 report cover eu-west-1?" }
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the create must be accepted, got {} with {:?}",
        created.status, created.body
    );
    let inquiry_id = created.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the create carries an inquiry id: {:?}", created.body))
        .to_string();
    let exported = call(
        router,
        "POST",
        "/v1/compliance-inquiry/export",
        Bytes::from(json!({ "complianceInquiryId": inquiry_id }).to_string()),
    )
    .await;
    assert_eq!(
        exported.status, 200,
        "the export must be served, got {} with {:?}",
        exported.status, exported.body
    );
    rows.push(Announced {
        case: format!("the exported document of {inquiry_id}"),
        url: exported.body["documentPresignedUrl"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "ExportComplianceInquiry announces a URL: {:?}",
                    exported.body
                )
            })
            .to_string(),
    });

    rows
}

#[tokio::test]
async fn every_equivalent_spelling_of_an_announced_document_url_serves_the_document() {
    let router = emulator();
    let rows = announced(&router).await;
    assert!(
        rows.len() >= 3,
        "the rows must span both document planes, got {} of them",
        rows.len()
    );

    for row in &rows {
        let case = &row.case;
        let parts = parts_of(case, &row.url);

        // The announced URL itself, which is what every spelling below must answer alike.
        let served = follow_announced_url(&router, case, &row.url).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: the announced URL must serve its document, got {} with {:?}",
            served.status,
            String::from_utf8_lossy(&served.bytes)
        );
        assert!(
            !served.bytes.is_empty(),
            "case {case:?}: the served document must carry content"
        );

        for (how, path) in equivalent_spellings(&parts.path) {
            let case = format!("{case}: {how}");
            let answer = follow_announced_url(&router, &case, &url_for(&parts, &path)).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: an equivalent spelling of {:?} must serve the document, got {} \
                 with {:?}",
                parts.path,
                answer.status,
                String::from_utf8_lossy(&answer.bytes)
            );
            assert_eq!(
                answer.bytes, served.bytes,
                "case {case:?}: expected the document the announced URL serves, got {:?}",
                String::from_utf8_lossy(&answer.bytes)
            );
            assert_eq!(
                answer.content_type, served.content_type,
                "case {case:?}: expected content type {:?}, got {:?}",
                served.content_type, answer.content_type
            );
        }

        for (how, path) in inequivalent_spellings(&parts.path) {
            let case = format!("{case}: {how}");
            let answer = follow_announced_url(&router, &case, &url_for(&parts, &path)).await;
            assert_ne!(
                answer.status, 200,
                "case {case:?}: {path:?} names no document this service publishes, but it was \
                 served {:?}",
                String::from_utf8_lossy(&answer.bytes)
            );
            assert_ne!(
                answer.bytes, served.bytes,
                "case {case:?}: {path:?} must serve no document, got the announced one"
            );
        }

        // And none of that changed what the announced URL serves: a fetch is a read.
        let again = follow_announced_url(&router, case, &row.url).await;
        assert_eq!(
            again.bytes, served.bytes,
            "case {case:?}: the announced URL must still serve its document, got {} with {:?}",
            again.status,
            String::from_utf8_lossy(&again.bytes)
        );
    }
}
