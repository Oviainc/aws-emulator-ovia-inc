// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for HOW every paginated listing of this service words its refusal of a
//! `NextToken` it never issued: `ListGateways`, `ListHypervisors` and
//! `ListVirtualMachines`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter` —
//! the harness of `gateway_lifecycle` — so what is asserted is the wire behaviour.
//!
//! The rule under test: an unresolvable pagination token is a condition class of its OWN,
//! not a member-constraint violation, so a listing refuses it with HTTP 400
//! `ValidationException` (`smithy_query(query="operation",operation="ListGateways")`:
//! `ValidationException` with `@httpError 400`) whose `Message` is the one fixed sentence
//! the service states for that class — the same sentence for every unissued token, with no
//! part taken from the request, so neither the token, nor the member it arrived in, nor the
//! expression it satisfied appears in it. The token's modelled constraints (`NextToken`
//! `@length(1, 1000)`, `@pattern ^.+$`) all HOLD in every case below: what fails is where
//! the token came from, which is why the member-constraint envelope
//! (`N validation error(s) detected: Value at '<member>' …`, pinned for the conditions it
//! does cover by `refusal_names_the_member_one_way`) is the wrong wording for it.
//!
//! The table spans that class rather than the one forged string the behavioral unit sends:
//! all three listings, each against tokens differing in form (not hex, odd-length hex, hex
//! of text that is no ARN, hex of another service's ARN), in size (the modelled length
//! minimum of one character and its maximum of a thousand), and in PROVENANCE while still
//! being well formed — a boundary of this listing's own resource type that was never
//! minted, every ARN segment of a minted boundary altered, every spelling of a minted id
//! (shared with `page_token_provenance`'s own table), a token another listing of this
//! service issued, and a token another region's run of the same listing issued. The
//! sentence is asserted by its literal text here, independent of the constant the crate
//! words it from.

use serde_json::json;

use crate::gateway_lifecycle::{build_engine, build_router, REGION};
use crate::page_token_provenance::{
    forged_tokens, list, listed_arns, seed, Listing, LISTINGS, OTHER_REGION,
};

/// The whole `ValidationException$Message` an unissued pagination token is refused with:
/// the sentence the behavioral unit `backup-gateway.list-gateways-returns-list-shape`
/// records this service's real counterpart stating for that error class. Spelled out here
/// so the assertion does not read the wording out of the code under test.
const REFUSAL_SENTENCE: &str = "The pagination token is invalid and cannot be processed.";

/// The member every token below arrives in, as a validation path spells it — asserted to be
/// ABSENT from the refusal: this class of refusal names no member.
const TOKEN_MEMBER_PATH: &str = "nextToken";

/// The opening of the member-constraint envelope this service words a violated constraint
/// under (`refusal_names_the_member_one_way`), asserted to be absent too: an unissued token
/// violates no constraint of the member it arrived in.
const CONSTRAINT_ENVELOPE_OPENING: &str = "validation error";

/// A token the listing of `region` really ISSUED: page one of a listing cut to one record,
/// over a store holding more than one, carries the boundary it stopped on.
async fn issued_token(
    router: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    listing: &Listing,
) -> String {
    let page_one = list(router, region, listing, &json!({ "MaxResults": 1 })).await;
    page_one.next_token().unwrap_or_else(|| {
        panic!(
            "case {} in {region}: a truncated page must carry a NextToken, got {:?}",
            listing.operation, page_one.body
        )
    })
}

/// Every listing of this service refuses a `NextToken` it never issued with the one fixed
/// sentence, whatever the token was — and the refusal echoes nothing of the request.
#[tokio::test]
async fn an_unissued_page_token_is_refused_with_the_pagination_token_sentence() {
    let engine = build_engine();
    let router = build_router(&engine);
    // Two regions, each holding more than one record of every kind, so a listing really
    // issues a token and a token of another region's run of it exists to replay.
    let here = seed(&engine, &router, REGION, 2).await;
    let _there = seed(&engine, &router, OTHER_REGION, 2).await;

    for listing in LISTINGS {
        // The tokens the OTHER listings of this service issued, which are minted boundaries
        // of another resource type: well formed, issued, and not of this listing.
        let mut rows: Vec<(String, String)> = Vec::new();
        for other in LISTINGS {
            if other.operation == listing.operation {
                continue;
            }
            rows.push((
                format!("a token {} issued", other.operation),
                issued_token(&router, REGION, &other).await,
            ));
        }
        rows.push((
            format!("a token the {OTHER_REGION} run of this listing issued"),
            issued_token(&router, OTHER_REGION, &listing).await,
        ));
        rows.extend([
            ("not hex at all".to_string(), "not-a-token".to_string()),
            ("odd-length hex".to_string(), "abc".to_string()),
            (
                "hex of text that is no ARN".to_string(),
                hex::encode("nothing"),
            ),
            (
                "hex of an ARN of another service".to_string(),
                hex::encode("arn:aws:backup:us-west-2:000000000000:vault/default"),
            ),
            // The modelled bounds of `NextToken` themselves: one character is its
            // `@length` minimum and a thousand its maximum, so each is forwarded unchanged
            // and reaches the handler rather than the boundary validator. The single
            // character is a digit, which the sentence carries nowhere, so the refusal can
            // still be checked for echoing it.
            ("the one-character minimum length".to_string(), "7".to_string()),
            (
                "the thousand-character maximum length".to_string(),
                "a".repeat(1000),
            ),
        ]);
        // The boundaries of this listing's own resource type that were never issued: a
        // sequence past every minted one, and every segment and id spelling altered.
        rows.extend(forged_tokens(&listing));

        // Page one of this listing, for the comparison each refusal is judged against.
        let page_one = list(&router, REGION, &listing, &json!({ "MaxResults": 1 })).await;
        let first_page = listed_arns(&page_one, &listing);
        assert_eq!(
            page_one.status, 200,
            "case {}: the unpaged listing must answer 200, got {} {:?}",
            listing.operation, page_one.status, page_one.body
        );

        for (case, token) in &rows {
            let case = format!("{} / {case}", listing.operation);
            let refused = list(
                &router,
                REGION,
                &listing,
                &json!({ "MaxResults": 1, "NextToken": token }),
            )
            .await;
            assert_eq!(
                (refused.status, refused.code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: an unissued NextToken must be refused with 400 \
                 ValidationException, got {} {:?}",
                refused.status,
                refused.body
            );
            let stated = refused.str_at("/Message").unwrap_or_else(|| {
                panic!("case {case:?}: the refusal must carry a Message, got {:?}", refused.body)
            });
            assert_eq!(
                stated, REFUSAL_SENTENCE,
                "case {case:?}: the refusal must state {REFUSAL_SENTENCE:?}, got {stated:?}"
            );
            assert!(
                !stated.contains(token.as_str()),
                "case {case:?}: the opaque token must not be echoed back, got {stated:?}"
            );
            assert!(
                !stated.contains(TOKEN_MEMBER_PATH)
                    && !stated.contains(CONSTRAINT_ENVELOPE_OPENING),
                "case {case:?}: an unissued token is no member-constraint violation, so the \
                 refusal must name neither the member nor the envelope, got {stated:?}"
            );
            let served = listed_arns(&refused, &listing);
            assert!(
                served.is_empty(),
                "case {case:?}: a refused listing must serve no records — least of all page \
                 one ({first_page:?}) — got {served:?}"
            );
            assert_eq!(
                refused.next_token(),
                None,
                "case {case:?}: a refused listing must promise no successor, got {:?}",
                refused.body
            );
        }

        // The rows mean nothing unless the listing really holds records to page over.
        let records = here.of(&listing);
        assert!(
            records.len() >= 2,
            "case {}: the seed must hold at least two records for a token to mean \
             anything, got {records:?}",
            listing.operation
        );

        // And the token this listing DID issue still resumes, so the refusals above are
        // about the token's provenance and not about the listing being broken.
        let issued = issued_token(&router, REGION, &listing).await;
        let resumed = list(
            &router,
            REGION,
            &listing,
            &json!({ "MaxResults": 1, "NextToken": issued }),
        )
        .await;
        let mut expected = records.to_vec();
        expected.sort();
        assert_eq!(
            (resumed.status, listed_arns(&resumed, &listing)),
            (200, vec![expected[1].clone()]),
            "case {}: a token this listing issued must still resume after its boundary, \
             got {} {:?}",
            listing.operation,
            resumed.status,
            resumed.body
        );
    }
}

/// The refusal is worded the same whether the listing holds records or not, and whether the
/// request names a page size or not: the token is resolved before any page is cut, so the
/// state of the store and the page controls cannot change the sentence.
#[tokio::test]
async fn the_sentence_does_not_depend_on_the_store_or_the_page_controls() {
    let engine = build_engine();
    let router = build_router(&engine);
    let token = hex::encode("arn:aws:backup-gateway:us-west-2:000000000000:gateway/bgw-ffffffffffffffff");

    // (case, the body beside the token)
    let bodies = [
        ("no page size", json!({})),
        ("the smallest page", json!({ "MaxResults": 1 })),
        ("a page larger than the store", json!({ "MaxResults": 1000 })),
    ];

    // Before anything is stored, and again once the listing really has records to page.
    for (store_case, seeded) in [("an empty store", 0usize), ("a seeded store", 3)] {
        if seeded > 0 {
            seed(&engine, &router, REGION, seeded).await;
        }
        for (body_case, body) in &bodies {
            let mut request = body.clone();
            request["NextToken"] = json!(token);
            for listing in LISTINGS {
                let case = format!("{} / {store_case} / {body_case}", listing.operation);
                let refused = list(&router, REGION, &listing, &request).await;
                assert_eq!(
                    (
                        refused.status,
                        refused.code.as_deref(),
                        refused.str_at("/Message")
                    ),
                    (400, Some("ValidationException"), Some(REFUSAL_SENTENCE)),
                    "case {case:?}: the refusal must be 400 ValidationException stating \
                     {REFUSAL_SENTENCE:?}, got {} {:?}",
                    refused.status,
                    refused.body
                );
            }
        }
    }
}
