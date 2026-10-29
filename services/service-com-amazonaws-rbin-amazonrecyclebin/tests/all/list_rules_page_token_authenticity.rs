// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHICH `NextToken` values `ListRules` resumes a walk at.
//!
//! The rule asserted here: a page cursor is honoured only by the endpoint that issued
//! it, and only for a position that endpoint really handed out — a token whose syntax
//! satisfies `NextToken`'s `@pattern` but which this endpoint did not issue (a value a
//! client invented, or a cursor another Region's endpoint issued) is refused with the
//! modelled `ValidationException`, HTTP 400 and `Reason=INVALID_PAGE_TOKEN`, never
//! answered with a page that resumes "after" a position never handed out.
//!
//! The tables span the dimension the rule generalises over — what a token decodes to:
//! a value that is not base64, base64 of a payload that is no cursor at all, base64 of
//! an identifier-shaped value across the identifier alphabet (including the identifier
//! of a rule the Region really holds), a cursor naming a position no walk of this store
//! has reached, and a real cursor of ANOTHER Region's endpoint — crossed with the
//! Region the request is signed for. The positive rows pin what the refusal must not
//! break: a cursor this endpoint issued still pages the Region's rules exactly once,
//! and it stays usable after the rule it names is deleted, since the position it names
//! was handed out whether or not that rule is still stored.
//!
//! Every case drives the service through the real router, exactly as a signed SDK
//! request would, and states its own case so a failure names the input.

use serde_json::{json, Value};

use crate::error_response_bodies::error_code;
use crate::retention_rule_lifecycle::{
    call_signed_for, list_of, retention, router, supported_retention, RESOURCE_TYPES,
};
use service_com_amazonaws_rbin_amazonrecyclebin::state::encode_page_token;
use smithy_server_runtime::MultiServiceRouter;

/// The Regions the tables span: published AWS Region codes from three geographies, each
/// one admitted by `RuleArn`'s Region segment
/// (`^arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}$`, from
/// smithy_query(query="shape", shape="RuleArn")).
pub(crate) const REGIONS: [&str; 3] = ["us-east-1", "eu-central-1", "ap-northeast-1"];

/// `RuleIdentifier` is `^[0-9a-zA-Z]{11}$` — 11 characters of the base62 alphabet.
/// Source: smithy_query(query="shape", shape="RuleIdentifier").
const IDENTIFIER_LEN: usize = 11;

/// Identifier-shaped values no store that has minted a handful of rules can have handed
/// out, each one sorting after every minted identifier (which is rendered left-padded
/// with `'0'`): the top of each half of the letter range, and the two mixed with a
/// digit. Every row satisfies `^[0-9a-zA-Z]{11}$`, so each is a value the model's
/// `RuleIdentifier` admits and only the service's own records can rule out.
const UNMINTED_IDENTIFIERS: [&str; 5] = [
    "ZZZZZZZZZZZ",
    "zzzzzzzzzzz",
    "zzzzzzzzzz9",
    "Z0Z0Z0Z0Z0Z",
    "aaaaaaaaaaa",
];

/// How many rules each Region holds in the tables below: enough that a one-rule page
/// hands back a cursor and that a forged cursor in the middle of the alphabet would
/// visibly drop rules.
const RULES_PER_REGION: usize = 4;

/// `POST /list-rules` signed for `region` — `ListRules`.
async fn list_rules_in(router: &MultiServiceRouter, region: &str, body: Value) -> (u16, Value) {
    call_signed_for(router, region, "POST", "/list-rules", Some(body)).await
}

/// `DELETE /rules/{Identifier}` signed for `region` — `DeleteRule`.
async fn delete_rule_in(router: &MultiServiceRouter, region: &str, identifier: &str) -> u16 {
    let (status, _body) =
        call_signed_for(router, region, "DELETE", &format!("/rules/{identifier}"), None).await;
    status
}

/// Base64 of `payload`, which is the envelope a `NextToken` takes, so a row can state
/// the bytes a forged token decodes to rather than a base64 blob.
fn as_token(payload: &str) -> String {
    aws_smithy_types::base64::encode(payload)
}

/// Create `total` rules of `resource_type` in `region` and hand back their identifiers
/// in creation order.
pub(crate) async fn create_rules_in(
    router: &MultiServiceRouter,
    region: &str,
    resource_type: &str,
    total: usize,
    case: &str,
) -> Vec<String> {
    let mut created = Vec::new();
    for nth in 0..total {
        let (status, body) = call_signed_for(
            router,
            region,
            "POST",
            "/rules",
            Some(json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(supported_retention(resource_type, nth)),
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateRule #{nth}, got {status} / {body:?}"
        );
        let identifier = body["Identifier"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier: {body:?}"))
            .to_string();
        assert_eq!(
            identifier.len(),
            IDENTIFIER_LEN,
            "case {case:?}: CreateRule minted {identifier:?}, which is not a \
             ^[0-9a-zA-Z]{{11}}$ rule identifier"
        );
        created.push(identifier);
    }
    created
}

/// One page of `resource_type` through `region`'s endpoint: the identifiers it reported
/// and the cursor it handed back. Fails the case unless the page is an HTTP 200.
pub(crate) async fn page_in(
    router: &MultiServiceRouter,
    region: &str,
    resource_type: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let mut request = json!({ "ResourceType": resource_type });
    if let Some(size) = max_results {
        request["MaxResults"] = Value::from(size);
    }
    if let Some(token) = next_token {
        request["NextToken"] = Value::String(token.to_string());
    }
    let (status, body) = list_rules_in(router, region, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListRules, got {status} / {body:?}"
    );
    let reported = list_of(&body, "Rules")
        .iter()
        .map(|summary| summary["Identifier"].as_str().unwrap_or_default().to_string())
        .collect();
    let token = body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (reported, token)
}

/// Assert `ListRules` refuses `token` as a page token: the modelled
/// `ValidationException`, HTTP 400, `Reason` the page-token fault class, a non-empty
/// `Message`, and NO page in the body — a refusal is not a truncated success.
pub(crate) async fn assert_refused(
    router: &MultiServiceRouter,
    region: &str,
    resource_type: &str,
    token: &str,
    case: &str,
) {
    let (status, body) = list_rules_in(
        router,
        region,
        json!({ "ResourceType": resource_type, "NextToken": token }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?} (NextToken={token:?}): expected HTTP 400, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?} (NextToken={token:?}): expected the modelled ValidationException, \
         got {body:?}"
    );
    assert_eq!(
        body.get("Reason").and_then(Value::as_str),
        Some("INVALID_PAGE_TOKEN"),
        "case {case:?} (NextToken={token:?}): expected Reason INVALID_PAGE_TOKEN, got {:?}",
        body.get("Reason")
    );
    assert!(
        body.get("Message")
            .and_then(Value::as_str)
            .is_some_and(|stated| !stated.is_empty()),
        "case {case:?} (NextToken={token:?}): expected a non-empty Message, got {:?}",
        body.get("Message")
    );
    assert!(
        body.get("Rules").is_none(),
        "case {case:?} (NextToken={token:?}): a refused page token must not be answered \
         with a page, got {body:?}"
    );
}

#[tokio::test]
async fn list_rules_refuses_every_page_token_it_did_not_issue() {
    for (row, region) in REGIONS.iter().enumerate() {
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let setup = format!("{RULES_PER_REGION} rules retaining {resource_type} in {region}");
        let router = router();
        let created = create_rules_in(
            &router,
            region,
            resource_type,
            RULES_PER_REGION,
            &setup,
        )
        .await;
        let held = created
            .first()
            .unwrap_or_else(|| panic!("case {setup:?}: the Region must hold a rule"))
            .clone();
        let other = REGIONS[(row + 1) % REGIONS.len()];

        // What a token can decode to, spanning the class: not base64 at all; base64 of
        // a payload that is no cursor; base64 of a bare identifier-shaped value, across
        // the alphabet and including the identifier of a rule this Region really holds
        // (a position the service knows, carried by a token it never handed out); a
        // cursor of this Region naming a position no walk of this store has reached; and
        // a cursor shaped like another Region's.
        let mut rows: Vec<(String, String)> = vec![
            // In `NextToken`'s alphabet, so the constraint gate passes it, but not
            // base64 of anything: five characters cannot be a base64 payload.
            ("not base64".to_string(), "AAAAA".to_string()),
            (
                "base64 of a payload that is no cursor".to_string(),
                as_token("not-a-cursor"),
            ),
            (
                "base64 of a cursor with no identifier".to_string(),
                as_token(&format!("{region}/")),
            ),
            (
                "base64 of a cursor whose identifier is too short".to_string(),
                as_token(&format!("{region}/0000000")),
            ),
            (
                "base64 of a cursor whose identifier is outside the alphabet".to_string(),
                as_token(&format!("{region}/0000000000-")),
            ),
            (
                "base64 of a cursor with no Region".to_string(),
                as_token(&format!("/{held}")),
            ),
            (
                "base64 of the identifier of a rule this Region holds".to_string(),
                as_token(&held),
            ),
        ];
        for unminted in UNMINTED_IDENTIFIERS {
            assert!(
                created.iter().all(|minted| minted.as_str() < unminted),
                "setup {setup:?}: {unminted:?} must sort after every minted identifier, \
                 got {created:?}"
            );
            rows.push((
                format!("base64 of the identifier-shaped value {unminted:?}"),
                as_token(unminted),
            ));
            rows.push((
                format!("a cursor of this Region naming the unminted {unminted:?}"),
                encode_page_token(region, unminted),
            ));
            rows.push((
                format!("a cursor of {other} naming the unminted {unminted:?}"),
                encode_page_token(other, unminted),
            ));
        }
        rows.push((
            format!("a cursor of {other} naming a rule of {region}"),
            encode_page_token(other, &held),
        ));

        for (case, token) in rows {
            let case = format!("{case} against {region}");
            assert_refused(&router, region, resource_type, &token, &case).await;
        }

        // Every refusal above changed nothing: the Region's own walk still reports every
        // rule it holds, in creation order.
        let case = format!("the whole walk of {region} after the refusals");
        let (reported, token) = page_in(&router, region, resource_type, None, None, &case).await;
        assert_eq!(
            reported, created,
            "case {case:?}: ListRules must still report every rule of the Region, \
             got {reported:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: a page holding every rule hands back no cursor, got {token:?}"
        );
    }
}

#[tokio::test]
async fn a_cursor_one_region_issued_is_no_cursor_of_another_regions_walk() {
    let router = router();
    let resource_type = RESOURCE_TYPES[0];
    // Every Region holds its own rules, so a foreign cursor could silently skip the
    // ones sorting at or before it.
    let mut created: Vec<(&str, Vec<String>)> = Vec::new();
    for region in REGIONS {
        let setup = format!("{RULES_PER_REGION} rules in {region}");
        let identifiers = create_rules_in(
            &router,
            region,
            resource_type,
            RULES_PER_REGION,
            &setup,
        )
        .await;
        created.push((region, identifiers));
    }

    for (issuer, identifiers) in &created {
        // A real first page of the issuing Region, and the cursor it handed back.
        let case = format!("the first page of {issuer}");
        let (first, cursor) = page_in(&router, issuer, resource_type, Some(1), None, &case).await;
        let cursor = cursor.unwrap_or_else(|| {
            panic!("case {case:?}: expected a NextToken after a page of one rule")
        });
        assert_eq!(
            first,
            identifiers[..1].to_vec(),
            "case {case:?}: the first page must report the Region's first rule, got {first:?}"
        );

        for (region, held) in &created {
            if region == issuer {
                continue;
            }
            let case = format!("a cursor of {issuer} replayed against {region}");
            assert_refused(&router, region, resource_type, &cursor, &case).await;
            // And that Region's own walk still reports every rule it holds — none of
            // them was skipped by the foreign cursor.
            let (reported, token) = page_in(&router, region, resource_type, None, None, &case).await;
            assert_eq!(
                &reported, held,
                "case {case:?}: the Region must still report every rule it holds, \
                 got {reported:?}"
            );
            assert_eq!(
                token, None,
                "case {case:?}: a page holding every rule hands back no cursor, got {token:?}"
            );
        }

        // The endpoint that issued the cursor still resumes at it.
        let case = format!("the cursor of {issuer} replayed against {issuer}");
        let (resumed, _token) = page_in(
            &router,
            issuer,
            resource_type,
            Some(RULES_PER_REGION),
            Some(&cursor),
            &case,
        )
        .await;
        assert_eq!(
            resumed,
            identifiers[1..].to_vec(),
            "case {case:?}: the issuing Region must resume right after the cursor, \
             got {resumed:?}"
        );
    }
}

#[tokio::test]
async fn a_cursor_this_endpoint_issued_pages_every_rule_exactly_once() {
    // Page sizes from one rule per page to more than the whole collection, crossed with
    // the Region the request is signed for and the resource type the walk filters by.
    for (row, region) in REGIONS.iter().enumerate() {
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let router = router();
        let setup = format!("{RULES_PER_REGION} rules retaining {resource_type} in {region}");
        let created = create_rules_in(
            &router,
            region,
            resource_type,
            RULES_PER_REGION,
            &setup,
        )
        .await;

        for page_size in [None, Some(1), Some(2), Some(RULES_PER_REGION), Some(RULES_PER_REGION + 1)]
        {
            let case = format!(
                "{setup}, MaxResults={}",
                page_size.map_or("unset".to_string(), |size| size.to_string())
            );
            let mut reported: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            for pages in 0..=RULES_PER_REGION + 1 {
                let (ids, next) =
                    page_in(&router, region, resource_type, page_size, token.as_deref(), &case)
                        .await;
                reported.extend(ids);
                match next {
                    Some(next) => token = Some(next),
                    None => break,
                }
                assert!(
                    pages < RULES_PER_REGION,
                    "case {case:?}: paging did not terminate after {pages} pages"
                );
            }
            assert_eq!(
                reported, created,
                "case {case:?}: the walk must report every rule of the Region exactly \
                 once, in creation order, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_cursor_stays_usable_after_the_rule_it_names_is_deleted() {
    // A client that lists a page and then deletes what it listed resumes at the cursor
    // it was handed: the position was handed out, so it is still this endpoint's own
    // cursor whether or not the rule that occupied it is still stored.
    for (row, region) in REGIONS.iter().enumerate() {
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let router = router();
        let setup = format!("{RULES_PER_REGION} rules retaining {resource_type} in {region}");
        let created = create_rules_in(
            &router,
            region,
            resource_type,
            RULES_PER_REGION,
            &setup,
        )
        .await;

        let case = format!("{setup}, first page deleted then resumed");
        let (first, cursor) = page_in(&router, region, resource_type, Some(1), None, &case).await;
        let cursor = cursor.unwrap_or_else(|| {
            panic!("case {case:?}: expected a NextToken after a page of one rule")
        });
        for identifier in &first {
            let status = delete_rule_in(&router, region, identifier).await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected HTTP 204 from DeleteRule {identifier}, got {status}"
            );
        }

        let (resumed, _token) = page_in(
            &router,
            region,
            resource_type,
            Some(RULES_PER_REGION),
            Some(&cursor),
            &case,
        )
        .await;
        assert_eq!(
            resumed,
            created[1..].to_vec(),
            "case {case:?}: the resumed walk must report the rules after the cursor, \
             got {resumed:?}"
        );
    }
}
