// SPDX-License-Identifier: CC0-1.0

//! Cover for WHICH strings name a resume point in a paged answer of this service: only
//! the ones the service itself issues.
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//!
//! * A pagination token is OPAQUE, so a paged answer resumes from a `NextToken` only
//!   when that token is character for character the one this service issues for that
//!   list at that boundary (`state::encode_page_token`); any other spelling of the same
//!   boundary — a string the service could never have minted — is refused as a bad
//!   request (400 `ValidationException`) naming the operation it was asked of, never
//!   honoured as a boundary and so never served as a page with everything up to that
//!   boundary silently dropped.
//!
//! The rows span, at the shared rule every paged answer reaches it through, EVERY paged
//! answer this service serves crossed with boundaries from both ends of the range the
//! modelled `Token` pattern's `(\d+)` accepts, each in the one spelling the service
//! issues and in five spellings it does not (zero padding of three widths, and two
//! signed forms); and, over the wire, two paged answers whose lists really hold the
//! boundary under test, each resumed from its own issued token and then asked with that
//! same boundary zero-padded — the non-canonical form the modelled pattern admits, so
//! it is the handler's rule that has to refuse it.
//!
//! The OTHER tolerant case — a token this service DID issue whose boundary is past
//! everything the list now holds — is a boundary the service really minted, and stays
//! an empty page ([`crate::order_listing`], [`crate::page_boundary_rule`]). Which
//! LIST a token belongs to is [`crate::page_token_refusal_shape`]; what error shape
//! the refusal travels on is stated there too.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    encode_page_token, page_boundary, ASSET_INSTANCE_PAGE, ASSET_PAGE, BLOCKING_INSTANCE_PAGE,
    CAPACITY_TASK_PAGE, CATALOG_ITEM_PAGE, CONFIGURED_INSTANCE_TYPE_PAGE,
    ORDERABLE_INSTANCE_TYPE_PAGE, ORDER_PAGE, OUTPOST_PAGE, QUOTE_PAGE, SITE_PAGE,
    SUPPORTED_INSTANCE_TYPE_PAGE,
};

use crate::site_and_outpost_lifecycle::{ids, query_value, text, Outposts, HOME_REGION};

/// The member the modelled `ValidationException` carries its report in.
///
/// Source: `smithy_query(query="shape", shape="ValidationException")` against this
/// service's model — `{"members": {"Message": {"target": "ErrorMessage"}}}`.
const MODELLED_MESSAGE: &str = "Message";

/// Every paged answer this service serves: the stamp its own page tokens carry, read
/// off the service's own constant rather than re-typed, and the operation name its
/// refusal of a token it did not issue has to state.
const EVERY_PAGED_ANSWER: &[(&str, &str)] = &[
    (SITE_PAGE, "ListSites"),
    (OUTPOST_PAGE, "ListOutposts"),
    (QUOTE_PAGE, "ListQuotes"),
    (ORDER_PAGE, "ListOrders"),
    (CATALOG_ITEM_PAGE, "ListCatalogItems"),
    (CAPACITY_TASK_PAGE, "ListCapacityTasks"),
    (ORDERABLE_INSTANCE_TYPE_PAGE, "ListOrderableInstanceTypes"),
    (ASSET_PAGE, "ListAssets"),
    (ASSET_INSTANCE_PAGE, "ListAssetInstances"),
    (CONFIGURED_INSTANCE_TYPE_PAGE, "GetOutpostInstanceTypes"),
    (
        SUPPORTED_INSTANCE_TYPE_PAGE,
        "GetOutpostSupportedInstanceTypes",
    ),
    (
        BLOCKING_INSTANCE_PAGE,
        "ListBlockingInstancesForCapacityTask",
    ),
];

/// The boundaries each spelling is tried at: both ends of what the modelled `Token`
/// pattern's `(\d+)` accepts for a `u64` boundary, and values in between.
const BOUNDARIES: &[u64] = &[0, 1, 7, 4_096, u64::MAX];

/// The report this service states for a `NextToken` it did not issue for `operation`.
///
/// The standard AWS validation report grammar — the violation count, then one
/// `Value at '<member>' failed to satisfy constraint: <requirement>` clause — naming
/// the member the paged input binds the token in and the requirement the token failed.
fn expected_report(operation: &str) -> String {
    format!(
        "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: \
         Member must be a pagination token this service issued for {operation}"
    )
}

/// Spellings of `boundary`'s numeric half that this service never writes, yet which
/// the modelled `Token` pattern's `(\d+)` ADMITS — so they travel past the boundary
/// validator and reach the handler — and out of which Rust's integer parser would read
/// that very boundary: zero padding at three widths, the third wider than the longest
/// `u64` so that it is non-canonical for every boundary, `u64::MAX` included.
fn padded_spellings(boundary: u64) -> Vec<String> {
    vec![
        format!("0{boundary}"),
        format!("00{boundary}"),
        format!("{boundary:0>21}"),
    ]
}

/// Spellings of `boundary`'s numeric half carrying an explicit sign: a form Rust's
/// integer parser accepts and this service never writes. The modelled `Token` pattern
/// `^(\d+)##(\S+)$` does not admit a sign, so the boundary validator refuses these
/// before any handler runs — which is why they are stated at the shared rule rather
/// than over the wire.
fn signed_spellings(boundary: u64) -> Vec<String> {
    vec![format!("+{boundary}"), format!("+0{boundary}")]
}

/// Rule, read off the shared helper every paged answer of this service takes its resume
/// point from: the one spelling the service issues names the boundary, and every other
/// spelling of that same boundary is refused with the report naming the operation.
#[test]
fn only_the_spelling_this_service_issues_names_a_resume_point() {
    for (stamp, operation) in EVERY_PAGED_ANSWER {
        for boundary in BOUNDARIES.iter().copied() {
            // The one rendering this service issues resumes from that boundary.
            let issued = encode_page_token(stamp, boundary);
            let read = page_boundary(stamp, operation, Some(&issued));
            assert_eq!(
                read,
                Ok(Some(boundary)),
                "case {operation} asked with {issued:?}: expected boundary {boundary}, \
                 got {read:?}"
            );

            // No other spelling of it does, however readable its digits are.
            let others = padded_spellings(boundary)
                .into_iter()
                .chain(signed_spellings(boundary));
            for sequence in others {
                let token = format!("{sequence}##{stamp}");
                let read = page_boundary(stamp, operation, Some(&token));
                assert_eq!(
                    read,
                    Err(expected_report(operation)),
                    "case {operation} asked with {token:?} (boundary {boundary} spelled a \
                     way this service never writes): expected the refusal report, got \
                     {read:?}"
                );
            }
        }
    }
}

/// One paged answer over the wire, asked on a list that really holds more items than
/// one page of it reports.
struct Paged {
    operation: &'static str,
    path: &'static str,
    /// The output member the answer reports its items in.
    member: &'static str,
    /// The identifiers the list holds, in the order it reports them.
    held: Vec<String>,
}

/// Two lists this store really holds: three sites, and eight Outposts on the first of
/// them — enough that a page short of the whole list issues a token, which is the
/// boundary the padded spellings are then tried at.
async fn populated_lists(outposts: &Outposts) -> Vec<Paged> {
    let mut sites: Vec<String> = Vec::new();
    for index in 0..3 {
        sites.push(
            outposts
                .site_id_in(HOME_REGION, &format!("canonical-token-{index}"))
                .await,
        );
    }
    let mut held: Vec<String> = Vec::new();
    for index in 0..8 {
        let outpost = outposts
            .create_outpost(&json!({
                "Name": format!("canonical-token-outpost-{index}"),
                "SiteId": sites[0],
            }))
            .await;
        held.push(text(&outpost, "OutpostId"));
    }

    vec![
        Paged {
            operation: "ListSites",
            path: "/sites",
            member: "Sites",
            held: sites,
        },
        Paged {
            operation: "ListOutposts",
            path: "/outposts",
            member: "Outposts",
            held,
        },
    ]
}

/// The same rule driven end-to-end: a paged answer resumes from the token IT issued,
/// and refuses a zero-padded spelling of that same boundary — the form the modelled
/// `Token` pattern `^(\d+)##(\S+)$` admits, so it reaches the handler — rather than
/// serving a page with everything up to the boundary dropped.
#[tokio::test]
async fn a_paged_answer_refuses_a_spelling_of_its_boundary_it_never_issued() {
    let outposts = Outposts::new();

    for answer in populated_lists(&outposts).await {
        let last = answer.held.len() - 1;
        let page = outposts
            .ok("GET", &format!("{}?MaxResults={last}", answer.path), None)
            .await;
        let issued = text(&page, "NextToken");
        let case = answer.operation;
        assert_eq!(
            ids(&page, answer.member),
            answer.held[..last].to_vec(),
            "case {case} asked for {last} of {}: expected the first {last} of the list, \
             got {page}",
            answer.held.len()
        );

        // The boundary that token carries, and the proof it is the one spelling this
        // service writes for it.
        let (sequence, stamp) = issued
            .split_once("##")
            .unwrap_or_else(|| panic!("case {case}: expected a page token, got {issued:?}"));
        let boundary: u64 = sequence
            .parse()
            .unwrap_or_else(|_| panic!("case {case}: expected a numeric boundary in {issued:?}"));
        assert_eq!(
            issued,
            encode_page_token(stamp, boundary),
            "case {case}: the issued token is this service's own rendering of boundary \
             {boundary}, got {issued:?}"
        );

        // Resuming from the issued token reports the rest of the list.
        let resumed = outposts
            .ok(
                "GET",
                &format!("{}?NextToken={}", answer.path, query_value(&issued)),
                None,
            )
            .await;
        assert_eq!(
            ids(&resumed, answer.member),
            answer.held[last..].to_vec(),
            "case {case} resumed from {issued:?}: expected the rest of the list, got \
             {resumed}"
        );

        // The same boundary spelled a way this service never writes is a bad request,
        // not a truncated page.
        let expected = expected_report(case);
        for sequence in padded_spellings(boundary) {
            let token = format!("{sequence}##{stamp}");
            let reply = outposts
                .call(
                    "GET",
                    &format!("{}?NextToken={}", answer.path, query_value(&token)),
                    None,
                )
                .await;
            let case = format!("{case} asked with {token:?}");
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (400, "ValidationException"),
                "case {case}: expected 400 ValidationException, got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
            assert_eq!(
                reply.body.get(MODELLED_MESSAGE).and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case}: expected the refusal to name the operation, got {}",
                reply.body
            );
            assert!(
                reply.body.get(answer.member).is_none(),
                "case {case}: a refused token reports no items at all, got {}",
                reply.body
            );
        }
    }
}
