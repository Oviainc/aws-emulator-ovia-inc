// SPDX-License-Identifier: CC0-1.0

//! ListEntitlements, end to end over HTTP: `GET /v1/entitlements`.
//!
//! The subject is the rule ListEntitlements follows: it reports the entitlements this
//! account HOLDS — "all entitlements that have been granted to this account", each a
//! "ListedEntitlement ... granted to you from other Amazon Web Services accounts" — so an
//! entitlement is reported to each account its `Subscribers` name, projected field for
//! field from the stored record, in one stable order, a page at a time. Nothing is fixed
//! in the handler: every member asserted below is compared against what the grant stored,
//! and an entitlement no longer stored is no longer listed.
//!
//! The cases therefore vary what that rule generalises over: which accounts a grant names
//! as subscribers (this account alone, other accounts alone, a mix, and where in the list
//! this account sits), the optional members a grant carries, whether the entitlement was
//! stored by CreateFlow or by GrantFlowEntitlements, which flow holds it, how many are
//! held, the page size (across the modelled `MaxResults` range and its default) and the
//! form of the pagination token.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front door uses, on the harness `flow_lifecycle` owns, so what these tests assert is
//! what a MediaConnect client sees: routing, restJson1 deserialization, constraint
//! validation, handler, serialization.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_entitlements::{entitlements, flow_with, grant};
use crate::flow_lifecycle::Emulator;

/// An account id that is NOT the one this emulator serves: an entitlement granted only to
/// it is granted to somebody else, so this account does not hold it.
const OTHER_ACCOUNT: &str = "111122223333";

/// A second such account, so a row can name more than one foreign subscriber.
const THIRD_ACCOUNT: &str = "222233334444";

/// `GET /v1/entitlements` — the ListEntitlements binding
/// (`smithy_query(query="operation",operation="ListEntitlements")`).
async fn list_entitlements(emulator: &Emulator, query: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/entitlements{query}"), None)
        .await
}

/// The `Entitlements` member of a ListEntitlements response.
fn listed(body: &Value) -> Vec<Value> {
    body["entitlements"].as_array().cloned().unwrap_or_default()
}

/// The ARNs of the `Entitlements` member, in the order the response carries them.
fn listed_arns(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|entitlement| {
            entitlement["entitlementArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The listed entitlement with this ARN, or `None` when the page does not carry it.
fn listed_by_arn(body: &Value, arn: &str) -> Option<Value> {
    listed(body)
        .into_iter()
        .find(|entitlement| entitlement["entitlementArn"].as_str() == Some(arn))
}

/// The ARNs of an `entitlements` member of a grant or flow projection, in order.
fn minted_arns(body: &Value) -> Vec<String> {
    entitlements(body)
        .iter()
        .map(|entitlement| {
            entitlement["entitlementArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The entitlement ARNs a flow carries, read back through DescribeFlow — the stored
/// record, in the order the flow holds it.
async fn stored_arns(emulator: &Emulator, flow_arn: &str) -> Vec<String> {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "describing {flow_arn} expected 200, got {status} {code:?}: {described}"
    );
    minted_arns(&described["flow"])
}

/// One granted entitlement, and whether this account holds it.
struct EntitlementCase {
    case: &'static str,
    name: &'static str,
    /// `GrantEntitlementRequest$Subscribers` — "the Amazon Web Services account IDs that
    /// you want to share your content with".
    subscribers: Vec<String>,
    /// `GrantEntitlementRequest$DataTransferSubscriberFeePercent`, or `None` when the
    /// grant names none.
    fee_percent: Option<i64>,
    /// Whether ListEntitlements must report this entitlement to this account.
    held: bool,
}

/// The grant request body for one case.
fn request_of(case: &EntitlementCase) -> Value {
    let mut request = json!({
        "name": case.name,
        "subscribers": case.subscribers,
    });
    if let Some(fee_percent) = case.fee_percent {
        request["dataTransferSubscriberFeePercent"] = json!(fee_percent);
    }
    request
}

/// The grants asserted below. The rows differ along what the listing rule generalises
/// over: whether this account is a subscriber at all, whether it is the only one, where in
/// the subscriber list it sits, and what the entitlement's optional fee percentage is.
fn entitlement_cases() -> Vec<EntitlementCase> {
    let this_account = engine::emulator_account_id().to_string();
    vec![
        EntitlementCase {
            case: "granted to this account alone",
            name: "held-only-subscriber",
            subscribers: vec![this_account.clone()],
            // "Percentage from 0-100 of the data transfer cost to be billed to the
            // subscriber" (`smithy_query(query="shape",shape="ListedEntitlement",
            // member="DataTransferSubscriberFeePercent")`): the lower published bound.
            fee_percent: Some(0),
            held: true,
        },
        EntitlementCase {
            case: "granted to this account first, among others",
            name: "held-first-subscriber",
            subscribers: vec![
                this_account.clone(),
                OTHER_ACCOUNT.to_string(),
                THIRD_ACCOUNT.to_string(),
            ],
            // The upper published bound of the same percentage.
            fee_percent: Some(100),
            held: true,
        },
        EntitlementCase {
            case: "granted to this account last, after other accounts",
            name: "held-last-subscriber",
            subscribers: vec![OTHER_ACCOUNT.to_string(), this_account.clone()],
            fee_percent: Some(42),
            held: true,
        },
        EntitlementCase {
            case: "granted to this account with no fee percentage named",
            name: "held-no-fee",
            subscribers: vec![this_account],
            fee_percent: None,
            held: true,
        },
        EntitlementCase {
            case: "granted to one other account only",
            name: "shared-elsewhere",
            subscribers: vec![OTHER_ACCOUNT.to_string()],
            fee_percent: Some(7),
            held: false,
        },
        EntitlementCase {
            case: "granted to several other accounts only",
            name: "shared-elsewhere-twice",
            subscribers: vec![OTHER_ACCOUNT.to_string(), THIRD_ACCOUNT.to_string()],
            // The model puts no range on the member (it targets `Integer`), so a value
            // past the documented 0-100 is stored and reported as given rather than
            // refused or clamped.
            fee_percent: Some(101),
            held: false,
        },
    ]
}

/// ListEntitlements reports exactly the entitlements this account holds, whichever flow
/// holds them and whichever call stored them, each projected from the stored record.
#[tokio::test]
async fn list_reports_every_entitlement_granted_to_this_account() {
    let emulator = Emulator::new();
    let cases = entitlement_cases();
    // Half the rows are stored by CreateFlow, on one flow; the rest by
    // GrantFlowEntitlements, on another — the listing spans the store, not one flow or one
    // call.
    let (created, granted) = cases.split_at(3);

    let created_requests: Vec<Value> = created.iter().map(request_of).collect();
    let creating_flow = flow_with(&emulator, "entitlement-listing-created", &created_requests).await;
    let granting_flow = flow_with(&emulator, "entitlement-listing-granted", &[]).await;
    let (status, code, grant_body) = grant(
        &emulator,
        &granting_flow,
        json!({ "entitlements": granted.iter().map(request_of).collect::<Vec<Value>>() }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "granting expected 201, got {status} {code:?}: {grant_body}"
    );

    let mut arns = stored_arns(&emulator, &creating_flow).await;
    arns.extend(minted_arns(&grant_body));
    assert_eq!(
        arns.len(),
        cases.len(),
        "every case must have a minted ARN, got {arns:?}"
    );

    let (status, code, body) = list_entitlements(&emulator, "?maxResults=1000").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "listing expected 200, got {status} {code:?}: {body}"
    );
    let expected_held = cases.iter().filter(|case| case.held).count();
    assert_eq!(
        listed(&body).len(),
        expected_held,
        "only the entitlements granted to this account are listed, each once, got {body}"
    );

    for (case, arn) in cases.iter().zip(&arns) {
        let name_of_case = case.case;
        assert!(
            mc::is_mintable_entitlement_arn(arn),
            "case {name_of_case:?}: the minted EntitlementArn must be one this service \
             could mint, got {arn:?}"
        );
        let found = listed_by_arn(&body, arn);
        if !case.held {
            assert!(
                found.is_none(),
                "case {name_of_case:?}: an entitlement granted to other accounts only is \
                 not held by this account, got {found:?}"
            );
            continue;
        }
        let entitlement = found.unwrap_or_else(|| {
            panic!("case {name_of_case:?}: the held entitlement must be listed, got {body}")
        });
        assert_eq!(
            entitlement["entitlementName"].as_str(),
            Some(case.name),
            "case {name_of_case:?}: the listed name is the stored one, got {entitlement}"
        );
        assert_eq!(
            entitlement["dataTransferSubscriberFeePercent"].as_i64(),
            case.fee_percent,
            "case {name_of_case:?}: the listed fee percentage is the stored one (absent \
             when the grant named none), got {entitlement}"
        );
        // A `ListedEntitlement` carries exactly three members
        // (`smithy_query(query="shape",shape="ListedEntitlement")`), so the summary is not
        // the whole `Entitlement`: the subscriber list and the grant's other members stay
        // on the flow's own projection.
        assert_eq!(
            entitlement.as_object().map(|members| members.len()),
            Some(if case.fee_percent.is_some() { 3 } else { 2 }),
            "case {name_of_case:?}: a summary carries only the modelled members, got \
             {entitlement}"
        );
    }

    // A read does not change what a later caller sees: listing again reports the same
    // entitlements, in the same order.
    let (_, _, again) = list_entitlements(&emulator, "?maxResults=1000").await;
    assert_eq!(
        again, body,
        "listing twice reports the same entitlements, got {again}"
    );

    // Each held entitlement is still on the flow that granted it, unchanged by the
    // listing: a reporting operation writes nothing.
    assert_eq!(
        stored_arns(&emulator, &creating_flow).await,
        arns[..3].to_vec(),
        "the granting flow still holds its entitlements after the listing"
    );
    assert_eq!(
        stored_arns(&emulator, &granting_flow).await,
        arns[3..].to_vec(),
        "the second flow still holds its entitlements after the listing"
    );
}

/// ListEntitlements pages the entitlements this account holds: whatever `MaxResults` is,
/// the pages partition every held entitlement exactly once, in one stable order, and only
/// the last page omits `NextToken`. With no `MaxResults` the page holds the documented
/// default of twenty.
#[tokio::test]
async fn list_pages_every_held_entitlement_exactly_once() {
    let emulator = Emulator::new();
    let this_account = engine::emulator_account_id();
    // More than the documented default page size, so the default is observable, and under
    // the ceiling on one flow's entitlements.
    let total = 22_usize;
    let flow_arn = flow_with(&emulator, "entitlement-paging", &[]).await;
    let (status, code, grant_body) = grant(
        &emulator,
        &flow_arn,
        json!({
            "entitlements": (0..total)
                .map(|index| json!({
                    "name": format!("paged-{index:02}"),
                    "subscribers": [this_account],
                }))
                .collect::<Vec<Value>>()
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "granting {total} entitlements expected 201, got {status} {code:?}: {grant_body}"
    );
    // One entitlement granted elsewhere: it must not appear on any page.
    let elsewhere = grant(
        &emulator,
        &flow_arn,
        json!({ "entitlements": [{ "name": "not-ours", "subscribers": [OTHER_ACCOUNT] }] }),
    )
    .await;
    assert_eq!(elsewhere.0, 201, "granting elsewhere expected 201");

    let (_, _, all) = list_entitlements(&emulator, "?maxResults=1000").await;
    let expected = listed_arns(&all);
    assert_eq!(
        expected,
        minted_arns(&grant_body),
        "every entitlement granted to this account is listed, in the order its ARNs were \
         minted, got {all}"
    );

    // "If MaxResults is not included in the request, the service defaults to pagination
    // with a maximum of 20 results per page"
    // (`smithy_query(query="doc",shape="ListEntitlementsRequest",member="MaxResults")`).
    let (status, _, default_page) = list_entitlements(&emulator, "").await;
    assert_eq!(status, 200, "the default page expected 200, got {status}");
    assert_eq!(
        listed_arns(&default_page),
        expected[..20].to_vec(),
        "the default page holds the first twenty held entitlements, got {default_page}"
    );
    assert_eq!(
        default_page["nextToken"].as_str(),
        Some(expected[20].as_str()),
        "the default page carries the ARN the next page resumes at, got {default_page}"
    );

    // The page size spans the modelled `MaxResults` range (1..1000): a page smaller than
    // the set, ones that divide it, one that equals it, and the maximum.
    for page_size in [1_usize, 2, 11, 22, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let (status, code, body) = list_entitlements(&emulator, &query).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "page size {page_size}: expected 200, got {status} {code:?}: {body}"
            );
            let page = listed_arns(&body);
            token = body["nextToken"].as_str().map(ToString::to_string);
            if token.is_some() {
                assert_eq!(
                    page.len(),
                    page_size,
                    "page size {page_size}: a page that carries a token is full, got {body}"
                );
            }
            seen.extend(page);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "page size {page_size}: the pages must partition every held entitlement once, \
             in order"
        );
    }

    // The token is the ARN the listing RESUMES AT, so a page resumed from it starts there.
    let (_, _, first_page) = list_entitlements(&emulator, "?maxResults=3").await;
    let token = first_page["nextToken"]
        .as_str()
        .expect("a page smaller than the set carries a token")
        .to_string();
    let (status, code, resumed) =
        list_entitlements(&emulator, &format!("?maxResults=1000&nextToken={token}")).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "resuming at an entitlement's ARN expected 200, got {status} {code:?}: {resumed}"
    );
    assert_eq!(
        listed_arns(&resumed),
        expected[3..].to_vec(),
        "the listing resumes at the key the token names, got {resumed}"
    );
}

/// An account that holds no entitlement is listed empty — not a 404 and not a page of
/// invented entitlements. A store with no flow at all, and a store whose entitlements are
/// all granted to other accounts, are both empty listings.
#[tokio::test]
async fn an_account_that_holds_no_entitlement_is_listed_empty() {
    let emulator = Emulator::new();
    for case in ["an empty store", "entitlements granted elsewhere only"] {
        let (status, code, body) = list_entitlements(&emulator, "").await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: listing expected 200, got {status} {code:?}: {body}"
        );
        assert_eq!(
            listed(&body),
            Vec::<Value>::new(),
            "case {case:?}: an account holding no entitlement lists none, got {body}"
        );
        assert_eq!(
            body["nextToken"].as_str(),
            None,
            "case {case:?}: the last page carries no token, got {body}"
        );

        // Set the second case up: a flow whose entitlements name only other accounts.
        flow_with(
            &emulator,
            "entitlement-listing-empty",
            &[
                json!({ "name": "theirs", "subscribers": [OTHER_ACCOUNT] }),
                json!({ "name": "theirs-too", "subscribers": [OTHER_ACCOUNT, THIRD_ACCOUNT] }),
            ],
        )
        .await;
    }
}

/// What the listing reports follows the store: an entitlement this account holds is listed
/// until it is revoked, and the revoke takes it — and only it — off the listing.
#[tokio::test]
async fn a_revoked_entitlement_leaves_the_listing() {
    let emulator = Emulator::new();
    let this_account = engine::emulator_account_id();
    let flow_arn = flow_with(
        &emulator,
        "entitlement-listing-revoke",
        &[
            json!({ "name": "stays-one", "subscribers": [this_account] }),
            json!({ "name": "goes", "subscribers": [this_account] }),
            json!({ "name": "stays-two", "subscribers": [this_account] }),
        ],
    )
    .await;
    let arns = stored_arns(&emulator, &flow_arn).await;
    let (_, _, before) = list_entitlements(&emulator, "?maxResults=1000").await;
    assert_eq!(
        listed_arns(&before),
        arns,
        "every entitlement the flow granted this account is listed, got {before}"
    );

    let revoked = &arns[1];
    let (status, code, body) = emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/entitlements/{revoked}"),
            None,
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "revoking {revoked} expected 202, got {status} {code:?}: {body}"
    );

    let (status, code, after) = list_entitlements(&emulator, "?maxResults=1000").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "listing after the revoke expected 200, got {status} {code:?}: {after}"
    );
    assert_eq!(
        listed_arns(&after),
        vec![arns[0].clone(), arns[2].clone()],
        "a revoked entitlement leaves the listing and the others stay, got {after}"
    );
}

/// One refused ListEntitlements request.
struct RefusalCase {
    case: &'static str,
    query: String,
    status: u16,
    code: &'static str,
}

/// ListEntitlements refuses a page size outside the modelled range and a pagination token
/// it could not have issued — with the modelled `BadRequestException` (400), never with a
/// 501 and never with a success.
#[tokio::test]
async fn refusals_name_the_modelled_error() {
    let emulator = Emulator::new();

    let cases = vec![
        // The modelled `MaxResults` range is 1..1000, so each bound is usable (the paging
        // suite above lists with 1 and with 1000) and each value past it is refused.
        RefusalCase {
            case: "MaxResults below the modelled range",
            query: "?maxResults=0".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "MaxResults above the modelled range",
            query: "?maxResults=1001".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        // A `NextToken` is the entitlement ARN the listing resumes at, so a token the
        // service could not have minted is refused. The rows are the ways a token can fail
        // that form: not an ARN at all, another service's ARN, an account id that is not 12
        // digits, the wrong resource type, and an empty entitlement name.
        RefusalCase {
            case: "a nextToken that is not an ARN",
            query: "?nextToken=not-a-token".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken from another service",
            query: "?nextToken=arn:aws:medialive:us-east-1:000000000000:entitlement:1-deadbeef:e"
                .to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken whose account id is not 12 digits",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:12345:entitlement:1-deadbeef:e"
                .to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken naming another resource type",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:flow:1-deadbeef:e"
                .to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken with an empty entitlement name",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:entitlement:1-deadbeef:"
                .to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken whose name segments are all empty",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:entitlement:1-deadbeef::"
                .to_string(),
            status: 400,
            code: "BadRequestException",
        },
    ];

    for RefusalCase {
        case,
        query,
        status: expected_status,
        code: expected_code,
    } in cases
    {
        let (status, code, body) = list_entitlements(&emulator, &query).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {body}"
        );
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: the refusal carries a message, got {body}"
        );
    }

    // A token the service COULD have minted is accepted even when no such entitlement is
    // held: paging must survive a revoke between pages.
    let unknown = "arn:aws:mediaconnect:us-east-1:000000000000:entitlement:1-deadbeef:missing";
    let (status, code, body) =
        list_entitlements(&emulator, &format!("?nextToken={unknown}")).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "a well-formed token for an unknown entitlement pages from it, got {status} {code:?}: \
         {body}"
    );
    assert_eq!(
        listed(&body),
        Vec::<Value>::new(),
        "nothing is held past that key, got {body}"
    );
}
