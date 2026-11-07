// SPDX-License-Identifier: CC0-1.0

//! The Directory Service certificate LISTING, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body, so
//! nothing here reaches into the state struct and a response that is not computed from
//! persisted state cannot pass.
//!
//! The rule under test is that a paginated listing of a sub-resource reports exactly the
//! records the named parent holds, computed from the stored records, in creation order,
//! issuing a `NextToken` only while records remain — here: `ListCertificates` reports
//! every certificate `RegisterCertificate` registered with the directory the request
//! names and nothing else, each entry carrying the `CertificateInfo` members of the very
//! registration `DescribeCertificate` reports. The suites assert the invariants: a
//! directory nothing was registered with lists nothing rather than being refused, an id
//! naming no stored directory is refused, a page walk visits every registration exactly
//! once in registration order at every page size the model admits, a repeated listing
//! reports the same page, and a `NextToken` this listing did not issue for this
//! directory is refused.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificates::{
    certificate_pem, connector_in_service, describe, directory_in_service, register,
    CERTIFICATE_GUARD, STATE_REGISTERED, STATE_REGISTERING, TYPE_CLIENT_CERT_AUTH,
    TYPE_CLIENT_LDAPS,
};
use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, refused, service, string, PAST_ONE_SPAN,
};

/// The members the modelled `CertificateInfo` structure declares
/// (`smithy_query(query="shape",shape="CertificateInfo")`): a listed entry carries these
/// and no others, which is what makes the listing a narrower view of the same stored
/// registration `DescribeCertificate` reports in full.
const CERTIFICATE_INFO_MEMBERS: [&str; 5] = [
    "CertificateId",
    "CommonName",
    "State",
    "ExpiryDateTime",
    "Type",
];

/// The published bounds of the modelled `Limit` member, `PageLimit` with
/// `range 1..50` (`smithy_query(query="shape",shape="PageLimit")`).
const LIMIT_MIN: i64 = 1;
const LIMIT_MAX: i64 = 50;

/// One `ListCertificates` page: the answer's `CertificatesInfo` entries.
fn entries(answer: &Value, case: &str) -> Vec<Value> {
    answer["CertificatesInfo"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a CertificatesInfo list, got {answer}")
        })
        .clone()
}

/// `ListCertificates` for `directory_id`, with an optional page size and cursor.
async fn list_page(
    router: &MultiServiceRouter,
    directory_id: &str,
    limit: Option<i64>,
    next_token: Option<&str>,
    case: &str,
) -> Value {
    let mut body = json!({ "DirectoryId": directory_id });
    if let Some(limit) = limit {
        body["Limit"] = json!(limit);
    }
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    ok(router, "ListCertificates", body, case).await
}

/// Every registration of `directory_id`, walked through however many pages the listing
/// hands back at `limit`, in the order the pages reported them.
async fn walk(
    router: &MultiServiceRouter,
    directory_id: &str,
    limit: Option<i64>,
    case: &str,
) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let answer = list_page(router, directory_id, limit, next_token.as_deref(), case).await;
        let page = entries(&answer, case);
        if let Some(limit) = limit {
            assert!(
                page.len() as i64 <= limit,
                "case {case:?}: expected at most {limit} entries per page, got {}",
                page.len()
            );
        }
        walked.extend(page);
        pages += 1;
        assert!(
            pages <= CERTIFICATE_GUARD + 2,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        match answer["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    walked
}

/// The `CertificateId` of each entry, in the order the listing reported them.
fn reported_ids(page: &[Value], case: &str) -> Vec<String> {
    page.iter()
        .map(|entry| {
            entry["CertificateId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected every entry to name its id, got {entry}")
                })
                .to_string()
        })
        .collect()
}

/// One row of the listing table: a certificate to register with the directory.
struct ListedCase {
    case: &'static str,
    common_name: Option<&'static str>,
    not_after: (i32, u8, u8),
    requested_type: Option<&'static str>,
    settings: Option<Value>,
}

#[tokio::test]
async fn the_listing_reports_the_registrations_of_the_directory_it_names() {
    let router = service();
    // The rows span both modelled `CertificateType` values, and client authentication is
    // offered for AD Connector alone, so the directories these registrations are made
    // with are of that type.
    let directory_id = connector_in_service(&router, "ldaps.example.com", "listing").await;
    let other_id = connector_in_service(&router, "other.example.net", "listing").await;

    // A directory nothing was registered with lists NOTHING: an empty page, not a
    // refusal, and no token.
    let empty = list_page(&router, &directory_id, None, None, "no registrations yet").await;
    assert!(
        entries(&empty, "no registrations yet").is_empty() && empty["NextToken"].is_null(),
        "case \"no registrations yet\": expected an empty final page, got {empty}"
    );

    // The rows differ along everything a registration varies in: the common name the
    // certificate states (several forms, and one certificate stating none at all), the
    // expiry it states (near and far), the function it performs (both modelled
    // `CertificateType` values and a request naming none), and whether client
    // certificate authentication settings came with it.
    let cases = vec![
        ListedCase {
            case: "a secure-LDAP certificate, type named",
            common_name: Some("ldaps.example.com"),
            not_after: (2030, 1, 2),
            requested_type: Some(TYPE_CLIENT_LDAPS),
            settings: None,
        },
        ListedCase {
            case: "no type named: the documented default",
            common_name: Some("corp.example.net"),
            not_after: (2027, 6, 15),
            requested_type: None,
            settings: None,
        },
        ListedCase {
            case: "client certificate authentication, with an OCSP responder",
            common_name: Some("smartcard.example.org"),
            not_after: (2045, 12, 31),
            requested_type: Some(TYPE_CLIENT_CERT_AUTH),
            settings: Some(json!({ "OCSPUrl": "http://ocsp.example.org/status" })),
        },
        ListedCase {
            case: "a common name of one character",
            common_name: Some("a"),
            not_after: (2026, 2, 28),
            requested_type: Some(TYPE_CLIENT_CERT_AUTH),
            settings: None,
        },
        ListedCase {
            case: "a certificate whose subject states no common name",
            common_name: None,
            not_after: (2031, 11, 9),
            requested_type: None,
            settings: None,
        },
    ];

    let mut registered: Vec<String> = Vec::new();
    for row in &cases {
        let case = row.case;
        let (pem, _) = certificate_pem(row.common_name, row.not_after);
        let mut request = json!({ "DirectoryId": &directory_id, "CertificateData": pem });
        if let Some(wanted) = row.requested_type {
            request["Type"] = json!(wanted);
        }
        if let Some(settings) = &row.settings {
            request["ClientCertAuthSettings"] = settings.clone();
        }
        registered.push(register(&router, request, case).await);
    }

    // The other directory holds its own registration, so a listing that reported the
    // store rather than the named parent would be caught here.
    let (elsewhere_pem, _) = certificate_pem(Some("elsewhere.example.net"), (2033, 4, 5));
    let elsewhere = register(
        &router,
        json!({ "DirectoryId": &other_id, "CertificateData": elsewhere_pem }),
        "another directory's registration",
    )
    .await;

    // Every registration, once, in the order they were made.
    let page = entries(
        &list_page(&router, &directory_id, None, None, "every registration").await,
        "every registration",
    );
    assert_eq!(
        reported_ids(&page, "every registration"),
        registered,
        "case \"every registration\": expected every registration once in registration \
         order, got {page:?}"
    );
    assert!(
        !reported_ids(&page, "every registration").contains(&elsewhere),
        "case \"every registration\": expected the other directory's registration to be \
         absent, got {page:?}"
    );

    // Each entry is the very registration `DescribeCertificate` reports, narrowed to the
    // members `CertificateInfo` declares.
    for (row, (certificate_id, entry)) in cases.iter().zip(registered.iter().zip(page.iter())) {
        let case = row.case;
        let described = describe(&router, &directory_id, certificate_id, case).await;
        for member in CERTIFICATE_INFO_MEMBERS {
            assert_eq!(
                entry.get(member).cloned(),
                described.get(member).cloned(),
                "case {case:?}: expected the listed {member} to be the described one, got \
                 {entry} against {described}"
            );
        }
        let listed_members: BTreeSet<&str> = entry
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected an entry object, got {entry}"))
            .keys()
            .map(String::as_str)
            .collect();
        let declared: BTreeSet<&str> = CERTIFICATE_INFO_MEMBERS.into_iter().collect();
        assert!(
            listed_members.is_subset(&declared),
            "case {case:?}: expected only the CertificateInfo members, got {entry}"
        );
        // The register answers while the registration is still being made, so the
        // transient state is what the listing reports straight after it.
        assert_eq!(
            entry["State"],
            json!(STATE_REGISTERING),
            "case {case:?}: expected the transient state right after the register, got {entry}"
        );
        assert_eq!(
            entry["CommonName"].as_str(),
            row.common_name,
            "case {case:?}: expected the certificate's own common name, got {entry}"
        );
    }

    // A read does not change what the next caller sees.
    let again = entries(
        &list_page(&router, &directory_id, None, None, "a repeated listing").await,
        "a repeated listing",
    );
    assert_eq!(
        again, page,
        "case \"a repeated listing\": expected the same page again, got {again:?}"
    );

    // The transition the register owed has fallen due, so the listing reports the settled
    // state — the one the clock already made due, for every entry.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let settled = entries(
        &list_page(&router, &directory_id, None, None, "once the transition fell due").await,
        "once the transition fell due",
    );
    assert_eq!(
        reported_ids(&settled, "once the transition fell due"),
        registered,
        "case \"once the transition fell due\": expected the same registrations, got {settled:?}"
    );
    for entry in &settled {
        assert_eq!(
            entry["State"],
            json!(STATE_REGISTERED),
            "case \"once the transition fell due\": expected the settled state, got {entry}"
        );
    }
}

#[tokio::test]
async fn a_page_walk_visits_every_registration_once_at_every_modelled_page_size() {
    let router = service();
    let directory_id = directory_in_service(&router, "pages.example.com", "pagination").await;

    let mut registered: Vec<String> = Vec::new();
    for index in 0..4 {
        let (pem, _) =
            certificate_pem(Some(&format!("page-{index}.example.com")), (2030, 1, 1));
        registered.push(
            register(
                &router,
                json!({ "DirectoryId": &directory_id, "CertificateData": pem }),
                "pagination",
            )
            .await,
        );
    }

    // Page sizes spanning the published minimum, several, exactly the record count, one
    // past it, the published maximum, and a request naming no page size at all.
    let count = registered.len() as i64;
    for limit in [
        Some(LIMIT_MIN),
        Some(2),
        Some(count),
        Some(count + 1),
        Some(LIMIT_MAX),
        None,
    ] {
        let case = match limit {
            Some(limit) => format!("Limit {limit}"),
            None => "no Limit".to_string(),
        };
        let walked = walk(&router, &directory_id, limit, &case).await;
        assert_eq!(
            reported_ids(&walked, &case),
            registered,
            "case {case:?}: expected every registration once in registration order, got \
             {walked:?}"
        );
    }

    // One page size past each published bound is outside the modelled range, so it is
    // refused by the framework's validation rather than served.
    for limit in [LIMIT_MIN - 1, LIMIT_MAX + 1] {
        let case = format!("Limit {limit}, past the modelled range");
        let (status, answer) = call(
            &router,
            "ListCertificates",
            json!({ "DirectoryId": &directory_id, "Limit": limit }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the page size to be refused, got {status} with {answer}"
        );
        assert_eq!(
            answer["__type"],
            json!("ValidationException"),
            "case {case:?}: expected the framework validation envelope, got {answer}"
        );
    }
}

#[tokio::test]
async fn an_absent_directory_is_refused_and_a_foreign_token_is_not_a_page() {
    let router = service();
    let directory_id = directory_in_service(&router, "present.example.com", "refusals").await;
    let other_id = directory_in_service(&router, "sales.example.org", "refusals").await;

    for subject in [&directory_id, &other_id] {
        for index in 0..2 {
            let (pem, _) = certificate_pem(
                Some(&format!("token-{subject}-{index}.example.com")),
                (2031, 9, 9),
            );
            register(
                &router,
                json!({ "DirectoryId": subject, "CertificateData": pem }),
                "refusals",
            )
            .await;
        }
    }

    // A directory this account never created, in several forms the modelled
    // `DirectoryId` pattern `^d-[0-9a-f]{10}$` admits...
    let mut absent: Vec<String> = vec![
        "d-0000000000".to_string(),
        "d-00000000ff".to_string(),
        "d-abcdef0123".to_string(),
        "d-ffffffffff".to_string(),
    ];
    // ...and one that existed until it was deleted, so the refusal follows the store
    // rather than the shape of the id.
    let doomed = directory_in_service(&router, "doomed.example.com", "refusals").await;
    ok(
        &router,
        "DeleteDirectory",
        json!({ "DirectoryId": &doomed }),
        "refusals: delete",
    )
    .await;
    absent.push(doomed);

    for directory in &absent {
        let case = format!("ListCertificates against {directory}");
        refused(
            &router,
            "ListCertificates",
            json!({ "DirectoryId": directory }),
            "DirectoryDoesNotExistException",
            &case,
        )
        .await;
        // The refusal is built from the request: it quotes the id the caller asked for,
        // so the caller can fix the request from the message alone.
        let (_, parsed) = call(
            &router,
            "ListCertificates",
            json!({ "DirectoryId": directory }),
        )
        .await;
        let message = parsed["message"]
            .as_str()
            .or_else(|| parsed["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains(&format!("'{directory}'")),
            "case {case:?}: expected the message to name the requested directory, got {parsed}"
        );
    }

    // A directory still being built holds no registrations yet, which is an empty page:
    // the listing reports the store, and refuses nothing for the stage.
    let building = create_directory(
        &router,
        minimal_create("building.example.com"),
        "a directory still being built",
    )
    .await;
    let page = list_page(
        &router,
        &building,
        None,
        None,
        "a directory still being built",
    )
    .await;
    assert!(
        entries(&page, "a directory still being built").is_empty(),
        "case \"a directory still being built\": expected an empty page, got {page}"
    );

    // A token this service did not issue for THIS listing is not a page of it: a token
    // nobody issued, an empty one, one issued by another paginated operation of the same
    // service, and one this listing issued for another directory.
    let foreign_token = {
        let answer = ok(
            &router,
            "DescribeDirectories",
            json!({ "Limit": 1 }),
            "a token from another listing",
        )
        .await;
        string(&answer, "NextToken", "a token from another listing")
    };
    let other_directorys_token = {
        let answer = list_page(
            &router,
            &other_id,
            Some(1),
            None,
            "a token issued for another directory",
        )
        .await;
        string(&answer, "NextToken", "a token issued for another directory")
    };
    for (case, token) in [
        (
            "a token this service never issued",
            "not-a-token".to_string(),
        ),
        ("an empty token", String::new()),
        ("a token issued by another listing", foreign_token),
        (
            "a token issued for another directory",
            other_directorys_token,
        ),
    ] {
        refused(
            &router,
            "ListCertificates",
            json!({ "DirectoryId": &directory_id, "NextToken": token }),
            "InvalidNextTokenException",
            case,
        )
        .await;
    }
}
