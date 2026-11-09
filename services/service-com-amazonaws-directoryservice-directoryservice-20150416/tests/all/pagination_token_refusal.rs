// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusal of a page token it will not accept, driven end to
//! end over HTTP.
//!
//! Every case goes through the same front door a client uses — `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body — and
//! builds the state it needs through `CreateMicrosoftAD` / `AddTagsToResource`. Nothing
//! reaches into the state struct. The harness (router, call helpers) is the sibling
//! suites', reached as `crate::directory_lifecycle::<item>`.
//!
//! The rule under test: a `NextToken` this service did not issue for the listing it was
//! sent to is refused HTTP 400 with the error shape that operation's model declares for
//! it, and the body's `Message` is this service's diagnostic for a rejected page-token
//! VALUE — the one that names the input member and quotes neither the token nor the
//! listing — followed once by the request-id suffix carrying the id the same answer
//! reports in its modelled `RequestId`. It is asserted over a TABLE of every paginated
//! listing this service serves CROSSED with the ways a token can fail to be one of its
//! own (not hex at all, hex that decodes to nothing this service minted, empty, past the
//! token-length guard, and a token another listing issued) — the recorded call is one row
//! of it — plus the rows that prove the refusal follows stored state: a token a listing
//! DID issue is served by that listing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{call, ok, service, string, PAST_ONE_SPAN};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The diagnostic this service states for a page token it will not accept: it names the
/// input member whose value was rejected and quotes neither the token nor the listing.
/// Recorded behavior — the message the ds corpus holds for this refusal, which is NOT the
/// `InvalidNextTokenException` shape's documentation prose.
const INVALID_NEXT_TOKEN: &str = "Invalid value passed for the NextToken parameter.";

/// The longest `NextToken` a handler decodes before it refuses one unread, from
/// `crate::state::MAX_NEXT_TOKEN_CHARS`.
const MAX_NEXT_TOKEN_CHARS: usize = 1024;

/// The whole `Message` a refused token is answered with: the diagnostic, then the
/// request-id suffix every refusal of this service carries, with the id this very answer
/// reported.
fn refused_token_message(request_id: &str) -> String {
    format!("{INVALID_NEXT_TOKEN} : RequestId: {request_id}")
}

/// `RequestId`'s modelled `@pattern`
/// (`^([A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12})$`), as
/// an assertion.
fn assert_uuid_shaped(request_id: &str, case: &str) {
    let groups: Vec<&str> = request_id.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    assert!(
        groups.len() == widths.len()
            && groups.iter().zip(widths).all(|(group, width)| {
                group.len() == width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            }),
        "case {case:?}: expected a UUID-shaped RequestId, got {request_id:?}"
    );
}

/// One paginated listing of this service: the operation, the error shape its model
/// declares for a token it will not accept, and a body carrying that token.
struct Listing {
    operation: &'static str,
    /// The modelled error the operation answers a refused token with. Every listing that
    /// declares `InvalidNextTokenException` uses it; the two that do not declare it answer
    /// the `InvalidParameterException` their own error list carries
    /// (`smithy_query(query="operation",operation="ListADAssessments")` declares no
    /// `InvalidNextTokenException`).
    error: &'static str,
    body: Box<dyn Fn(&str) -> Value>,
}

/// Every paginated listing of this service, with a body that satisfies each published
/// constraint of its input so the request reaches the token check, parameterised by the
/// token under test. `directory_id` is a Managed Microsoft AD directory in service, which
/// every one of these listings admits.
fn listings(directory_id: &str) -> Vec<Listing> {
    let id = directory_id.to_string();
    let owner = id.clone();
    let controllers = id.clone();
    let ldaps = id.clone();
    let routes = id.clone();
    let certificates = id.clone();
    let extensions = id.clone();
    let tags = id.clone();
    let client_auth = id.clone();
    let updates = id.clone();
    vec![
        Listing {
            operation: "DescribeDirectories",
            error: "InvalidNextTokenException",
            body: Box::new(|token| json!({ "NextToken": token })),
        },
        Listing {
            operation: "DescribeSnapshots",
            error: "InvalidNextTokenException",
            body: Box::new(|token| json!({ "NextToken": token })),
        },
        Listing {
            operation: "DescribeTrusts",
            error: "InvalidNextTokenException",
            body: Box::new(|token| json!({ "NextToken": token })),
        },
        Listing {
            operation: "ListLogSubscriptions",
            error: "InvalidNextTokenException",
            body: Box::new(|token| json!({ "NextToken": token })),
        },
        Listing {
            operation: "DescribeSharedDirectories",
            error: "InvalidNextTokenException",
            body: Box::new(move |token| {
                json!({ "OwnerDirectoryId": &owner, "NextToken": token })
            }),
        },
        Listing {
            operation: "DescribeDomainControllers",
            error: "InvalidNextTokenException",
            body: Box::new(move |token| {
                json!({ "DirectoryId": &controllers, "NextToken": token })
            }),
        },
        Listing {
            operation: "DescribeLDAPSSettings",
            error: "InvalidNextTokenException",
            body: Box::new(move |token| json!({ "DirectoryId": &ldaps, "NextToken": token })),
        },
        Listing {
            operation: "ListIpRoutes",
            error: "InvalidNextTokenException",
            body: Box::new(move |token| json!({ "DirectoryId": &routes, "NextToken": token })),
        },
        Listing {
            operation: "ListCertificates",
            error: "InvalidNextTokenException",
            body: Box::new(move |token| {
                json!({ "DirectoryId": &certificates, "NextToken": token })
            }),
        },
        Listing {
            operation: "ListSchemaExtensions",
            error: "InvalidNextTokenException",
            body: Box::new(move |token| {
                json!({ "DirectoryId": &extensions, "NextToken": token })
            }),
        },
        Listing {
            operation: "ListTagsForResource",
            error: "InvalidNextTokenException",
            body: Box::new(move |token| json!({ "ResourceId": &tags, "NextToken": token })),
        },
        Listing {
            operation: "DescribeClientAuthenticationSettings",
            error: "InvalidParameterException",
            body: Box::new(move |token| {
                json!({ "DirectoryId": &client_auth, "Type": "SmartCard", "NextToken": token })
            }),
        },
        Listing {
            operation: "DescribeUpdateDirectory",
            error: "InvalidNextTokenException",
            body: Box::new(move |token| {
                json!({ "DirectoryId": &updates, "UpdateType": "OS", "NextToken": token })
            }),
        },
        Listing {
            operation: "ListADAssessments",
            error: "InvalidParameterException",
            body: Box::new(|token| json!({ "NextToken": token })),
        },
    ]
}

/// The ways a token can fail to be one this service issued for the listing it was sent
/// to, independent of which listing that is.
fn unusable_tokens() -> Vec<(String, String)> {
    vec![
        ("a token that is not hex".to_string(), "not-a-token".to_string()),
        (
            "hex that decodes to nothing this service minted".to_string(),
            "6162636465".to_string(),
        ),
        ("an empty token".to_string(), String::new()),
        (
            "a token past the token-length guard".to_string(),
            "61".repeat(MAX_NEXT_TOKEN_CHARS),
        ),
    ]
}

/// Assert one refused token: the status, the modelled error shape, and the whole `Message`
/// against the id the answer itself reported. Returns that request id, so a caller can
/// assert it is fresh per call.
async fn refused_token(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_error: &str,
    case: &str,
) -> String {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(expected_error),
        "case {case:?}: expected {expected_error} from {operation}, got {parsed}"
    );
    let reported = string(&parsed, "RequestId", case);
    assert_uuid_shaped(&reported, case);
    assert_eq!(
        string(&parsed, "Message", case),
        refused_token_message(&reported),
        "case {case:?}: expected this service's rejected-page-token diagnostic with one \
         request-id suffix, got {parsed}"
    );
    reported
}

#[tokio::test]
async fn every_listing_refuses_a_token_it_did_not_issue_with_this_services_diagnostic() {
    let router = service();

    // Two directories in service, so the listing of them has a second page to hand a
    // token out for, and every per-directory listing has a directory to run against.
    let directory = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("paged.corp.example.com"),
        "the directory under test",
    )
    .await;
    create_microsoft_ad(
        &router,
        minimal_microsoft_ad("second.corp.example.com"),
        "a second directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // A token one listing really did issue, to be replayed against the others: every
    // cursor is tagged with the listing that minted it.
    let case = "a page of the directory listing";
    let first_page = ok(
        &router,
        "DescribeDirectories",
        json!({ "Limit": 1 }),
        case,
    )
    .await;
    let directories_token = string(&first_page, "NextToken", case);

    // And one from a second listing, so the row for `DescribeDirectories` is a token
    // another listing issued too rather than a synthetic one.
    let case = "a page of the tag listing";
    ok(
        &router,
        "AddTagsToResource",
        json!({
            "ResourceId": &directory,
            "Tags": [
                { "Key": "owner", "Value": "platform" },
                { "Key": "tier", "Value": "test" }
            ]
        }),
        case,
    )
    .await;
    let tag_page = ok(
        &router,
        "ListTagsForResource",
        json!({ "ResourceId": &directory, "Limit": 1 }),
        case,
    )
    .await;
    let tags_token = string(&tag_page, "NextToken", case);

    let mut reported_ids: Vec<String> = Vec::new();
    for listing in listings(&directory) {
        let foreign = if listing.operation == "DescribeDirectories" {
            tags_token.clone()
        } else {
            directories_token.clone()
        };
        let rows = unusable_tokens()
            .into_iter()
            .chain([("a token another listing issued".to_string(), foreign)]);

        for (how, token) in rows {
            let case = format!("{} sent {}", listing.operation, how);
            reported_ids.push(
                refused_token(
                    &router,
                    listing.operation,
                    (listing.body)(&token),
                    listing.error,
                    &case,
                )
                .await,
            );
        }
    }

    // The id in each refusal is that call's own: a correlation handle is never shared
    // between two answers.
    let mut distinct = reported_ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        reported_ids.len(),
        "expected one fresh RequestId per refusal, got {reported_ids:?}"
    );
}

#[tokio::test]
async fn a_token_a_listing_issued_is_served_by_that_listing() {
    let router = service();

    let directory = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("served.corp.example.com"),
        "the directory under test",
    )
    .await;
    create_microsoft_ad(
        &router,
        minimal_microsoft_ad("served-second.corp.example.com"),
        "a second directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // The refusal is about whether this service issued the token for this listing, not
    // about the token's shape: the token the first page handed back is served.
    let case = "the second page of the directory listing";
    let first_page = ok(
        &router,
        "DescribeDirectories",
        json!({ "Limit": 1 }),
        case,
    )
    .await;
    let token = string(&first_page, "NextToken", case);
    let second_page = ok(
        &router,
        "DescribeDirectories",
        json!({ "Limit": 1, "NextToken": &token }),
        case,
    )
    .await;
    assert_eq!(
        second_page["DirectoryDescriptions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case {case:?}: expected the page the token resumes, got {second_page}"
    );
    assert!(
        second_page["Message"].is_null(),
        "case {case:?}: expected no refusal diagnostic on a served page, got {second_page}"
    );

    // The same holds for a second listing, so the served branch is not one operation's.
    let case = "the second page of the tag listing";
    ok(
        &router,
        "AddTagsToResource",
        json!({
            "ResourceId": &directory,
            "Tags": [
                { "Key": "owner", "Value": "platform" },
                { "Key": "tier", "Value": "test" }
            ]
        }),
        case,
    )
    .await;
    let tag_page = ok(
        &router,
        "ListTagsForResource",
        json!({ "ResourceId": &directory, "Limit": 1 }),
        case,
    )
    .await;
    let tag_token = string(&tag_page, "NextToken", case);
    let second_tags = ok(
        &router,
        "ListTagsForResource",
        json!({ "ResourceId": &directory, "Limit": 1, "NextToken": &tag_token }),
        case,
    )
    .await;
    assert_eq!(
        second_tags["Tags"].as_array().map(Vec::len).unwrap_or_default(),
        1,
        "case {case:?}: expected the page the token resumes, got {second_tags}"
    );
}
