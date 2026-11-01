// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for which endpoint a pagination token of this service is good
//! for.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a pagination token resumes a walk of the records ONE listing of
//! ONE regional endpoint serves, so a token is accepted only by the listing and the
//! Region's endpoint that issued it; a token issued elsewhere names no position in this
//! listing and is refused (400 `ValidationException`) rather than used as a start, which
//! would hand the caller a page that silently omits records of its own Region. Quick
//! Setup is a regional control plane — `ListConfigurations` "returns configurations
//! deployed by Quick Setup in the requesting Amazon Web Services account and Amazon Web
//! Services Region" (`smithy_query(query="doc",shape="ListConfigurations")`) and a
//! manager is reported only through the endpoint whose Region its `ManagerArn` carries —
//! so the records a token walks are a different set in every Region.
//!
//! The listings covered here are every paginated operation this service has: only
//! `ListConfigurationManagers` and `ListConfigurations` carry the `@paginated`
//! `inputToken`/`outputToken` pair (`smithy_query(query="digest")`). The token's OTHER
//! halves — the empty string an exhausted listing reports, and the refusal of a token
//! the other listing issued — are pinned by `tests/all/pagination_tokens.rs` and
//! `tests/all/configuration_managers.rs`.
//!
//! The rows here hold one record per manager, and a manager ARN embeds its Region, so the
//! store key a token carries sorts with its Region in the manager listing. A
//! `ListConfigurations` token carries a configuration's service-generated id instead,
//! which places it anywhere among another Region's keys; that half is pinned by
//! `tests/all/configuration_token_region_scope.rs`, which reuses the harness below.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// The Region-independent halves of the harness every suite of this crate shares: one
// emulator mounted the way the router mounts it, and the modelled error shape and
// message of a refusal. Only the request builder below is Region-aware, because the
// Region a request is signed for is what these cases vary.
use super::reserved_tag_keys::{emulator, error_code, error_message};

/// A configuration type the service publishes (`ListQuickSetupTypes`), so every create
/// below is refused only for a reason under test.
const TYPE: &str = "AWSQuickSetupType-SSMOpsCenter";

/// The organizational unit every definition below deploys to: a definition's parameters
/// must name somewhere to deploy to, and no case here is about which accounts that is.
const TARGET_OU: &str = "ou-1";

/// A request signed for `ssm-quicksetup` in `region` — the SigV4 credential scope the
/// router routes on and the handlers read the Region from.
fn request(
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{region}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, region, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// One paginated listing of this service: where to POST it, the member carrying its
/// rows, and the member identifying one row.
pub(crate) struct Listing {
    pub(crate) uri: &'static str,
    pub(crate) rows: &'static str,
    pub(crate) id: &'static str,
}

/// The manager listing.
const MANAGER_LISTING: Listing = Listing {
    uri: "/listConfigurationManagers",
    rows: "ConfigurationManagersList",
    id: "ManagerArn",
};

/// The configuration listing.
pub(crate) const CONFIGURATION_LISTING: Listing = Listing {
    uri: "/listConfigurations",
    rows: "ConfigurationsList",
    id: "Id",
};

/// Every paginated listing this service serves.
const LISTINGS: [Listing; 2] = [MANAGER_LISTING, CONFIGURATION_LISTING];

/// Create `count` managers through the endpoint of `region`, each with one definition
/// deploying into that one Region, so that Region holds `count` managers and `count`
/// configurations.
///
/// No case here turns on which parameters a definition names, so the ones this type's
/// documented table requires beyond the targeting above come from the fixture every
/// suite of this target shares.
async fn fill(router: &MultiServiceRouter, case: &str, region: &str, count: usize) {
    for index in 0..count {
        let body = json!({
            "Name": format!("manager-{region}-{index}"),
            "ConfigurationDefinitions": [{
                "Type": TYPE,
                "Parameters": super::type_parameter_fixtures::with_required_parameters(
                    TYPE,
                    json!({
                        "TargetOrganizationalUnits": TARGET_OU,
                        "TargetRegions": region,
                    }),
                ),
            }],
        });
        let (status, answer) =
            call(router, "POST", "/configurationManager", region, Some(&body)).await;
        assert_eq!(
            status, 200,
            "case {case}: CreateConfigurationManager in {region} must succeed, got {status} / \
             {answer:?}"
        );
    }
}

/// One page of `listing` through the endpoint of `region`.
pub(crate) async fn page(
    router: &MultiServiceRouter,
    listing: &Listing,
    region: &str,
    max_items: Option<i64>,
    token: Option<&str>,
) -> (u16, Value) {
    let mut body = Map::new();
    if let Some(max_items) = max_items {
        body.insert("MaxItems".to_string(), json!(max_items));
    }
    if let Some(token) = token {
        body.insert("StartingToken".to_string(), json!(token));
    }
    call(
        router,
        "POST",
        listing.uri,
        region,
        Some(&Value::Object(body)),
    )
    .await
}

/// The ids one page reports.
pub(crate) fn ids_of(case: &str, listing: &Listing, answer: &Value) -> Vec<String> {
    answer
        .get(listing.rows)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a {}, got {answer:?}", listing.rows))
        .iter()
        .map(|row| {
            row.get(listing.id)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: expected a {}, got {row:?}", listing.id))
                .to_string()
        })
        .collect()
}

/// Every id `region` holds, read by paging `listing` with `max_items` from the start,
/// asserting every page succeeds and that paging terminates.
pub(crate) async fn all_ids(
    router: &MultiServiceRouter,
    case: &str,
    listing: &Listing,
    region: &str,
    max_items: Option<i64>,
    bound: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let (status, answer) = page(router, listing, region, max_items, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case}: {} in {region} must succeed, got {status} / {answer:?}",
            listing.uri
        );
        seen.extend(ids_of(case, listing, &answer));
        pages += 1;
        assert!(
            pages <= bound + 1,
            "case {case}: paging {} in {region} did not terminate after {pages} pages",
            listing.uri
        );
        let next = answer
            .get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected a NextToken, got {answer:?}"));
        if next.is_empty() {
            return seen;
        }
        token = Some(next.to_string());
    }
}

/// One row of the table: the endpoint that issues the token, the endpoint that replays
/// it, how many managers each holds, and the page bound the issuing request asks for.
struct Case {
    issuer: &'static str,
    requester: &'static str,
    issuer_count: usize,
    requester_count: usize,
    max_items: Option<i64>,
}

/// A token issued by one Region's endpoint is refused by every other Region's endpoint,
/// with the 400 `ValidationException` a token this listing did not issue earns — and the
/// requesting Region's own records stay reachable, so nothing is silently skipped.
///
/// The rows differ along what the rule generalises over: which of the two Regions sorts
/// first (a manager ARN embeds its Region, so the foreign token's key sorts before the
/// requester's records in some rows and after them in others), how many records each
/// endpoint holds (including a requester holding none and one holding more than a page),
/// which listing the token came from, and the page bound asked for.
#[tokio::test]
async fn a_page_token_of_one_regions_endpoint_is_refused_by_another_regions_endpoint() {
    let cases = [
        // The issuer sorts AFTER the requester, so its token's key is past every record
        // the requester holds: the defect this pins answered an empty page.
        Case {
            issuer: "us-west-2",
            requester: "us-east-1",
            issuer_count: 2,
            requester_count: 1,
            max_items: Some(1),
        },
        // The issuer sorts BEFORE the requester, so its token's key is before every
        // record the requester holds.
        Case {
            issuer: "ap-south-1",
            requester: "eu-west-1",
            issuer_count: 3,
            requester_count: 2,
            max_items: Some(1),
        },
        // A requester that holds nothing: the answer is still the refusal, not the empty
        // page that would read as "this Region holds none".
        Case {
            issuer: "eu-central-1",
            requester: "us-east-2",
            issuer_count: 2,
            requester_count: 0,
            max_items: Some(1),
        },
        // A requester holding more than one page of its own, and a page bound above one.
        Case {
            issuer: "us-east-1",
            requester: "us-west-1",
            issuer_count: 4,
            requester_count: 3,
            max_items: Some(2),
        },
    ];

    for case in &cases {
        for listing in &LISTINGS {
            let name = format!(
                "{} / token from {} replayed on {} ({} and {} records, MaxItems={:?})",
                listing.uri,
                case.issuer,
                case.requester,
                case.issuer_count,
                case.requester_count,
                case.max_items
            );
            let router = emulator();
            fill(&router, &name, case.issuer, case.issuer_count).await;
            fill(&router, &name, case.requester, case.requester_count).await;

            // What the requesting endpoint holds, read without any token: the records no
            // page may silently drop.
            let own = all_ids(
                &router,
                &name,
                listing,
                case.requester,
                case.max_items,
                case.issuer_count + case.requester_count,
            )
            .await;
            assert_eq!(
                own.len(),
                case.requester_count,
                "case {name}: the requesting Region must report its own {} records, got {own:?}",
                case.requester_count
            );

            // A token the issuing endpoint handed out, which it accepts itself.
            let (status, answer) = page(&router, listing, case.issuer, case.max_items, None).await;
            assert_eq!(
                status, 200,
                "case {name}: the issuing listing must succeed, got {status} / {answer:?}"
            );
            let token = answer
                .get("NextToken")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {name}: expected a NextToken, got {answer:?}"))
                .to_string();
            assert!(
                !token.is_empty(),
                "case {name}: the issuing listing must hand out a resume token, got {answer:?}"
            );

            let (status, answer) = page(
                &router,
                listing,
                case.requester,
                case.max_items,
                Some(&token),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {name}: a token of another Region's endpoint must be refused, got {status} \
                 / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {name}: expected ValidationException, got {answer:?}"
            );
            let message = error_message(&answer);
            assert!(
                message.contains("StartingToken"),
                "case {name}: the refusal must name the member it refused, got {message:?}"
            );

            // The issuing endpoint still accepts its own token...
            let (status, answer) =
                page(&router, listing, case.issuer, case.max_items, Some(&token)).await;
            assert_eq!(
                status, 200,
                "case {name}: the issuing endpoint must accept its own token, got {status} / \
                 {answer:?}"
            );

            // ...and the refused request changed nothing: the requesting Region still
            // reports exactly the records it held.
            let again = all_ids(
                &router,
                &name,
                listing,
                case.requester,
                case.max_items,
                case.issuer_count + case.requester_count,
            )
            .await;
            assert_eq!(
                again, own,
                "case {name}: the requesting Region's records must stay reachable, got {again:?}"
            );
        }
    }
}

/// Paging a listing with the tokens IT issued still visits every record of the
/// requesting Region exactly once while another Region's endpoint holds records whose
/// store keys interleave with them — the token is a position in the requesting Region's
/// own listing, not in the shared store order.
#[tokio::test]
async fn paging_one_regions_listing_visits_its_own_records_exactly_once() {
    // (the Regions holding records, how many each holds) — Regions on both sides of one
    // another in ARN order, so neither Region's records are all first.
    let populations: [&[(&str, usize)]; 3] = [
        &[("us-east-1", 2), ("us-west-2", 3)],
        &[("us-west-2", 3), ("ap-south-1", 1)],
        &[("eu-west-1", 2), ("eu-central-1", 2), ("us-east-1", 1)],
    ];

    for population in populations {
        for max_items in [Some(1), Some(2), None] {
            for listing in &LISTINGS {
                let name = format!("{} / {population:?}, MaxItems={max_items:?}", listing.uri);
                let router = emulator();
                let total: usize = population.iter().map(|(_, count)| *count).sum();
                for (region, count) in population {
                    fill(&router, &name, region, *count).await;
                }

                for (region, count) in population {
                    let seen = all_ids(&router, &name, listing, region, max_items, total).await;
                    assert_eq!(
                        seen.len(),
                        *count,
                        "case {name}: {region} must report its own {count} records exactly once, \
                         got {seen:?}"
                    );
                    assert_eq!(
                        seen.iter().cloned().collect::<BTreeSet<String>>().len(),
                        *count,
                        "case {name}: {region} must serve no record twice, got {seen:?}"
                    );
                }
            }
        }
    }
}
