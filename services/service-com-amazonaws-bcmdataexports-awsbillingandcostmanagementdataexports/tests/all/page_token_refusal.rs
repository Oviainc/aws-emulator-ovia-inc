// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE page-token rule every paginated listing of this service
//! answers: `ListExports`, `ListTables`, `ListTagsForResource` and `ListExecutions`.
//!
//! The rule under test: a `NextToken` a listing did not itself issue is the modelled
//! `ValidationException` (HTTP 400) naming the `NextToken` member with reason
//! `fieldValidationFailed`, while a token the listing DID issue resumes it — and every
//! listing reports that one condition identically, because there is one rule and not one
//! per listing.
//!
//! It is asserted over a TABLE whose rows differ along what the rule generalises over:
//! each of the four listings (the service-owned catalog, the account's exports, one
//! export's tags and one export's executions), each crossed with a set of unissued tokens
//! differing in FORM — not hex at all, too short to carry the integrity tag, hex of a
//! plausible position with no tag, an issued token with its tag corrupted, an issued token
//! truncated — and with every OTHER listing's genuinely issued token, which names a
//! position in a sequence this listing never walked. The per-listing row of the table is
//! then asserted to agree with every other row on the status, the error code, the message
//! and the named member, which is the shared rule, and to carry its OWN requirement
//! sentence for the member, which is what a listing contributes to it.
//!
//! The router, the signing, the error-code helper, the store seeding and the minimal
//! export request all come from the other suites of this same test target.

use serde_json::{json, Map, Value};

use crate::delivery_executions::hold_delivery;
use crate::execution_lookup::{client_and_engine, record_execution};
use crate::export_lifecycle::{error_code, Client};
use crate::table_listing::minimal_export_request;

/// The input member every paginated listing of this service resumes from, and therefore
/// the member a refusal of a token names (smithy_query operations `ListExports`,
/// `ListTables`, `ListTagsForResource`, `ListExecutions`, member `NextToken`).
pub(crate) const PAGE_TOKEN_MEMBER: &str = "NextToken";

/// The status the modelled `ValidationException` is answered with (smithy_query shape
/// `ValidationException`, `@httpError`).
const VALIDATION_STATUS: u16 = 400;

/// The `ValidationExceptionReason` a refusal of one named input member reports, from the
/// model's own published list (smithy_query shape `ValidationExceptionReason`:
/// `cannotParse`, `fieldValidationFailed`, `other`, `unknownOperation`).
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// One row of the table: a paginated listing, and the request members that select what it
/// pages over (none, for the two listings that page over everything they can see).
///
/// `pub(crate)` with [`seeded`], [`request`], [`issued_token`] and [`refusal`] so a suite
/// covering another facet of the SAME shared page-token rule drives the same four listings
/// through the same harness instead of growing a second copy of it.
pub(crate) struct Listing {
    /// The operation name, as `X-Amz-Target` names it.
    pub(crate) operation: &'static str,
    /// The members this listing's request needs beyond the paging ones.
    pub(crate) selector: Vec<(&'static str, String)>,
}

/// A request for `listing` carrying the paging members a case sets.
pub(crate) fn request(listing: &Listing, max_results: Option<i64>, token: Option<&str>) -> Value {
    let mut body = Map::new();
    for (member, value) in &listing.selector {
        body.insert((*member).to_string(), json!(value));
    }
    if let Some(max) = max_results {
        body.insert("MaxResults".to_string(), json!(max));
    }
    if let Some(token) = token {
        body.insert(PAGE_TOKEN_MEMBER.to_string(), json!(token));
    }
    Value::Object(body)
}

/// A client whose store holds enough of everything for each of the four listings to cut a
/// page token: two exports, two tags on the first of them, and two recorded executions of
/// it.
///
/// The deliveries the service owes the exports it persisted are held out of the way, so
/// the executions this suite pages over are the ones it wrote (`delivery_executions.rs`
/// covers the accrual itself).
pub(crate) async fn seeded() -> (Client, Vec<Listing>) {
    let (client, engine) = client_and_engine();

    let mut arns: Vec<String> = Vec::new();
    for name in ["token_rule_one", "token_rule_two"] {
        let mut create = minimal_export_request(name);
        create["ResourceTags"] = json!([
            { "Key": "env", "Value": "dev" },
            { "Key": "owner", "Value": "billing" },
        ]);
        let created = client.ok("CreateExport", &create).await;
        let arn = created["ExportArn"]
            .as_str()
            .expect("CreateExport returns an ExportArn")
            .to_string();
        hold_delivery(&engine, &arn);
        arns.push(arn);
    }
    let tagged = arns[0].clone();
    for (index, execution_id) in ["exec-a", "exec-b"].iter().enumerate() {
        record_execution(&engine, &tagged, execution_id, index);
    }

    let listings = vec![
        Listing {
            operation: "ListExports",
            selector: Vec::new(),
        },
        Listing {
            operation: "ListTables",
            selector: Vec::new(),
        },
        Listing {
            operation: "ListTagsForResource",
            selector: vec![("ResourceArn", tagged.clone())],
        },
        Listing {
            operation: "ListExecutions",
            selector: vec![("ExportArn", tagged)],
        },
    ];
    (client, listings)
}

/// The token `listing` cuts for a page of one item: a token it really did issue.
pub(crate) async fn issued_token(client: &Client, listing: &Listing) -> String {
    let page = client
        .ok(listing.operation, &request(listing, Some(1), None))
        .await;
    page[PAGE_TOKEN_MEMBER]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: a first page of one over a sequence of two must carry {PAGE_TOKEN_MEMBER}, got {page}",
                listing.operation
            )
        })
        .to_string()
}

/// How `listing` refuses `token`: the three values the shared rule fixes and that a
/// caller reads out of the body — the wire error code, the message and the named member —
/// plus the requirement sentence that member carries, which is the listing's own. The
/// STATUS is asserted here rather than returned, so a comparison across listings is not
/// comparing a constant with itself.
pub(crate) async fn refusal(
    client: &Client,
    listing: &Listing,
    token: &str,
) -> (String, String, String, String) {
    let case = format!("{} with {token:?}", listing.operation);
    let (status, body) = client
        .call(listing.operation, &request(listing, None, Some(token)))
        .await;
    assert_eq!(
        status, VALIDATION_STATUS,
        "case {case:?}: an unissued {PAGE_TOKEN_MEMBER} must be refused with {VALIDATION_STATUS}, got {status} with {body}"
    );
    let named = body["Fields"][0]["Name"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must name a field, got {body}"))
        .to_string();
    let requirement = body["Fields"][0]["Message"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the named field must carry a requirement, got {body}")
        })
        .to_string();
    let message = body["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry a Message, got {body}"))
        .to_string();
    let reason = body["Reason"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry a Reason, got {body}"))
        .to_string();
    assert_eq!(
        (reason.as_str(), named.as_str()),
        (REASON_FIELD_VALIDATION_FAILED, PAGE_TOKEN_MEMBER),
        "case {case:?}: the refusal must name the paging member with the field-validation reason, got {body}"
    );
    assert!(
        requirement.contains(listing.operation),
        "case {case:?}: the named member's requirement must say which listing issues the token, got {requirement:?}"
    );
    (error_code(&body), message, named, requirement)
}

/// Tokens no listing of this service ever issued, differing in form.
fn forged_tokens(issued: &str) -> Vec<String> {
    let mut corrupted = issued.to_string();
    corrupted.replace_range(0..1, if issued.starts_with('a') { "b" } else { "a" });
    vec![
        "not-a-token-this-service-issued".to_string(),
        "00".to_string(),
        hex::encode("exec-a"),
        corrupted,
        issued
            .get(..issued.len().saturating_sub(2))
            .expect("an issued token is hex, so its bytes end on a character boundary")
            .to_string(),
    ]
}

/// Every listing refuses a `NextToken` it did not issue — whatever its form, and including
/// a token another listing really did issue — with the one modelled refusal, and keeps
/// honouring the token it did issue.
#[tokio::test]
async fn every_listing_refuses_an_unissued_page_token_the_same_way() {
    let (client, listings) = seeded().await;

    let mut issued: Vec<String> = Vec::new();
    for listing in &listings {
        issued.push(issued_token(&client, listing).await);
    }

    // The two values the shared rule fixes that a row could still differ on, as the FIRST
    // row reported them; every later row must agree with it. The status and the named
    // member are asserted in place by `refusal`, so they are not compared again here.
    let mut shared: Option<(String, String)> = None;
    for (index, listing) in listings.iter().enumerate() {
        // This listing's own token, every forged form, and every other listing's token:
        // each of them is a token THIS sequence never issued.
        let mut unissued = forged_tokens(&issued[index]);
        unissued.extend(
            issued
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, token)| token.clone()),
        );
        for token in unissued {
            let (code, message, _, _) = refusal(&client, listing, &token).await;
            let row = (code, message);
            match &shared {
                None => shared = Some(row),
                Some(first) => assert_eq!(
                    &row, first,
                    "case {:?} with {token:?}: every listing must report the unissued-token condition identically, got {row:?} against {first:?}",
                    listing.operation
                ),
            }
        }

        // The token this listing DID cut still resumes it: the de-duplicated path answers
        // the valid token as well as the invalid one.
        let resumed = client
            .ok(
                listing.operation,
                &request(listing, Some(1), Some(&issued[index])),
            )
            .await;
        assert!(
            resumed["Message"].is_null(),
            "case {:?}: the token this listing issued must resume it, got {resumed}",
            listing.operation
        );
    }
    assert!(
        shared.is_some(),
        "the table must have refused at least one token per listing"
    );
}

/// Each listing's refusal carries ITS OWN requirement sentence for the named member, so
/// the shared refusal still tells the caller which listing issues a usable token.
#[tokio::test]
async fn each_listing_names_itself_in_the_members_requirement() {
    let (client, listings) = seeded().await;

    let mut sentences: Vec<String> = Vec::new();
    for listing in &listings {
        let (_, _, _, requirement) =
            refusal(&client, listing, "not-a-token-this-service-issued").await;
        assert!(
            !sentences.contains(&requirement),
            "case {:?}: each listing's requirement sentence must be its own, got {requirement:?} twice",
            listing.operation
        );
        sentences.push(requirement);
    }
    assert_eq!(
        sentences.len(),
        listings.len(),
        "every listing must report a requirement for the named member, got {sentences:?}"
    );
}
