// SPDX-License-Identifier: CC0-1.0

//! The TERM resource `GetReportMetadata` names, over the whole published catalog.
//!
//! One rule is pinned here, over a SET of reports rather than any one of them:
//! **`ReportDetail$termArn` identifies the TERM resource governing a report, so it is a
//! function of that term alone.** A report is governed by ONE term, whichever of its
//! versions a caller reads, and a term is the document it is — so the ARN is the same at
//! every version of a report and across repeated reads, two reports serving byte-identical
//! term documents name the same term, two serving different ones never do, and the single
//! term a report reports is the single term one acceptance of it authorises every version
//! under.
//!
//! The rows differ along everything that rule generalises over: how many versions a report
//! publishes (one, two, three and seven in the published catalog), its `acceptanceType`,
//! which version the read names (each published one, and none at all), and the region the
//! request is signed for.
//!
//! Every assertion goes through the router, so each case exercises the same path a client
//! takes: routing, deserialization, the handler, the persisted state and serialization.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    emulator, follow_announced_url, get_report, get_signed, get_term_for_report, signed_for,
    Answer, CATALOG, REGION,
};

/// How many characters a term resource id carries after its `term-` prefix: the same
/// 16-character shape this service's other resource ids have (`ReportId` is
/// `^report-[a-zA-Z0-9]{16}$`, from `smithy_query(query="shape",shape="ReportId")`).
/// Spelled out here rather than imported, so a wrong id shape in `src/state.rs` fails.
const TERM_ID_BODY_LEN: usize = 16;

/// `GetReportMetadata` for `query`, signed for `region`.
async fn metadata(router: &MultiServiceRouter, region: &str, query: &str) -> Answer {
    get_signed(
        router,
        &format!("/v1/report/getMetadata{query}"),
        &signed_for(region),
    )
    .await
}

/// The `termArn` a metadata read reports, with the member asserted present.
fn term_arn_of(case: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200 from GetReportMetadata, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["reportDetails"]["termArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: the metadata read must report a termArn, got {:?}",
                answer.body
            )
        })
        .to_string()
}

/// The term resource id a term ARN names: everything after its service-owned
/// `arn:aws:artifact:<region>::term/` prefix. Asserts the ARN is service-owned (the signed
/// region, an empty account id) and that the id has this service's resource-id shape, so
/// nothing version-bearing or otherwise report-shaped can pass as a term id.
fn term_id_of(case: &str, region: &str, arn: &str) -> String {
    let prefix = format!("arn:aws:artifact:{region}::term/");
    let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case}: expected a service-owned term ARN under {prefix:?}, got {arn:?}")
    });
    let body = id.strip_prefix("term-").unwrap_or_else(|| {
        panic!("case {case}: a term ARN must name a term resource, got {arn:?}")
    });
    assert!(
        body.len() == TERM_ID_BODY_LEN && body.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case}: a term id is `term-` and {TERM_ID_BODY_LEN} alphanumeric characters, \
         got {arn:?}"
    );
    id.to_string()
}

/// What one acceptance of a report's term handed back.
struct Accepted {
    /// The `termToken` the acceptance minted — the receipt a download presents.
    receipt: String,
    /// The `documentPresignedUrl` it announced the term document at.
    document_url: String,
}

/// Accept the term governing one version of a report, the way a client does.
async fn accept(
    router: &MultiServiceRouter,
    case: &str,
    report_id: &str,
    version: i64,
) -> Accepted {
    let term = get_term_for_report(
        router,
        &format!("?reportId={report_id}&reportVersion={version}"),
    )
    .await;
    assert_eq!(
        term.status, 200,
        "case {case}: the acceptance must be served, got {} with {:?}",
        term.status, term.body
    );
    let member = |name: &str| {
        term.body[name]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: an acceptance must report {name}, got {:?}",
                    term.body
                )
            })
            .to_string()
    };
    Accepted {
        receipt: member("termToken"),
        document_url: member("documentPresignedUrl"),
    }
}

/// The TERM document one version of a report serves, fetched the way a client reaches it:
/// accept the term, then follow the URL the acceptance announces.
async fn term_document(
    router: &MultiServiceRouter,
    case: &str,
    report_id: &str,
    version: i64,
) -> Vec<u8> {
    let accepted = accept(router, case, report_id, version).await;
    let served = follow_announced_url(router, case, &accepted.document_url).await;
    assert_eq!(
        served.status, 200,
        "case {case}: the announced term document URL must resolve, got {} with {:?}",
        served.status,
        String::from_utf8_lossy(&served.bytes)
    );
    served.bytes.to_vec()
}

#[tokio::test]
async fn term_arn_names_one_term_for_every_version_of_a_report() {
    let router = emulator();

    for expected in CATALOG {
        // The read naming no version resolves the latest one; every other row names a
        // published version. All of them are reads of the SAME report, so all of them are
        // reads of the same term.
        let mut queries = vec![format!("?reportId={}", expected.id)];
        queries.extend(
            expected
                .versions
                .iter()
                .map(|version| format!("?reportId={}&reportVersion={version}", expected.id)),
        );

        let mut reported = BTreeSet::new();
        for query in &queries {
            let case = format!("{} {query}", expected.id);
            let first = term_arn_of(&case, &metadata(&router, REGION, query).await);
            term_id_of(&case, REGION, &first);
            // A read changes nothing a later caller sees, the term ARN included.
            let again = term_arn_of(&case, &metadata(&router, REGION, query).await);
            assert_eq!(
                again, first,
                "case {case}: two reads must report the same termArn, got {first:?} then \
                 {again:?}"
            );
            reported.insert(first);
        }

        assert_eq!(
            reported.len(),
            1,
            "case {:?}: a report is governed by ONE term, so every version of it must report \
             one termArn; got {reported:?} over {} reads",
            expected.id,
            queries.len()
        );
    }
}

#[tokio::test]
async fn term_arn_identifies_the_term_document_and_nothing_else() {
    let router = emulator();

    // What each report reports as its term, and the term document it actually serves.
    let mut observed: Vec<(&str, String, Vec<u8>)> = Vec::new();
    for expected in CATALOG {
        let case = expected.id.to_string();
        let arn = term_arn_of(
            &case,
            &metadata(&router, REGION, &format!("?reportId={}", expected.id)).await,
        );
        let id = term_id_of(&case, REGION, &arn);

        // Every version of the report serves the one term document the ARN names.
        let mut documents = BTreeSet::new();
        for version in expected.versions {
            let case = format!("{} v{version}", expected.id);
            documents.insert(term_document(&router, &case, expected.id, *version).await);
        }
        assert_eq!(
            documents.len(),
            1,
            "case {case}: every version of a report must serve the one term document its \
             termArn names, got {} distinct documents",
            documents.len()
        );
        let document = documents
            .into_iter()
            .next()
            .expect("a report serves a term document");
        assert!(
            !document.is_empty(),
            "case {case}: the term document served must carry the term text, got {} bytes",
            document.len()
        );
        observed.push((expected.id, id, document));
    }

    // Two reports name the same term resource exactly when the term they serve is the same
    // document: equal terms are one term, different terms are never one.
    for (left_id, left_term, left_document) in &observed {
        for (right_id, right_term, right_document) in &observed {
            let case = format!("{left_id} against {right_id}");
            assert_eq!(
                left_term == right_term,
                left_document == right_document,
                "case {case}: two reports must report one term id exactly when they serve the \
                 same term document; got {left_term:?} and {right_term:?} for documents that \
                 are {} the same",
                if left_document == right_document {
                    "all"
                } else {
                    "not"
                }
            );
        }
    }
}

#[tokio::test]
async fn term_arn_is_scoped_to_the_signed_region_and_names_the_same_term_in_each() {
    let router = emulator();
    // The rule generalises over the region the request was signed for, so the rows are
    // regions in different partitions' shapes, including the harness's own.
    let regions = [REGION, "us-east-1", "ap-southeast-2", "eu-central-1"];

    for expected in CATALOG {
        let mut ids = BTreeSet::new();
        for region in regions {
            for version in expected.versions {
                let case = format!("{} v{version} signed for {region}", expected.id);
                let arn = term_arn_of(
                    &case,
                    &metadata(
                        &router,
                        region,
                        &format!("?reportId={}&reportVersion={version}", expected.id),
                    )
                    .await,
                );
                ids.insert(term_id_of(&case, region, &arn));
            }
        }
        assert_eq!(
            ids.len(),
            1,
            "case {:?}: the region scopes the ARN, but the term resource it names is the same \
             one everywhere; got {ids:?}",
            expected.id
        );
    }
}

#[tokio::test]
async fn one_acceptance_covers_every_version_of_the_single_term_reported() {
    let router = emulator();

    // A report whose acceptanceType requires the explicit acceptance of its term is where
    // the term ARN is actionable: a client keys its acceptance off it. So for every such
    // report, the one term ARN reported must be the one term one receipt authorises — at
    // any version, including a version other than the one accepted through.
    for expected in CATALOG
        .iter()
        .filter(|row| row.requires_explicit_acceptance())
    {
        let accepted_through = expected.versions[0];
        let case = format!("{} accepted through v{accepted_through}", expected.id);
        let receipt = accept(&router, &case, expected.id, accepted_through)
            .await
            .receipt;

        for version in expected.versions {
            let case = format!("{} v{version} under the v{accepted_through} receipt", expected.id);
            let arn = term_arn_of(
                &case,
                &metadata(
                    &router,
                    REGION,
                    &format!("?reportId={}&reportVersion={version}", expected.id),
                )
                .await,
            );
            let accepted_arn = term_arn_of(
                &case,
                &metadata(
                    &router,
                    REGION,
                    &format!("?reportId={}&reportVersion={accepted_through}", expected.id),
                )
                .await,
            );
            assert_eq!(
                arn, accepted_arn,
                "case {case}: the term reported for this version must be the term that was \
                 accepted, got {arn:?} against {accepted_arn:?}"
            );

            let download = get_report(
                &router,
                &format!(
                    "?reportId={}&reportVersion={version}&termToken={receipt}",
                    expected.id
                ),
            )
            .await;
            assert_eq!(
                download.status, 200,
                "case {case}: one acceptance of the one term reported must authorise every \
                 version, got {} with {:?}",
                download.status, download.body
            );
        }
    }
}
