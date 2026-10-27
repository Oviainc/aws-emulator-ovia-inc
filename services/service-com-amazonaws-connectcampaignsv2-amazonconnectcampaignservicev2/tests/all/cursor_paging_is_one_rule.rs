// SPDX-License-Identifier: CC0-1.0

//! The ONE cursor-window rule both paged lists of this service page by, driven end
//! to end over the wire through an operation on either side of it.
//!
//! `ListCampaigns` and `ListConnectInstanceIntegrations` page over different stores
//! through the same window computation beside the token codec, so these cases hold
//! both operations to the same rule rather than to one copy of it:
//!
//! * the page starts after the cursor and runs for `maxResults` items clamped to
//!   what is left, so walking either list at any page size reads every record
//!   exactly once in stored order, in pages of the same sizes;
//! * a `nextToken` is handed out only while records remain beyond the page, so the
//!   last page of either list carries none;
//! * a call that names no `maxResults` reports as much as one page can carry, so a
//!   stored set smaller than that bound comes back whole and unpaged from either;
//! * a token the service did not mint resolves to no position at all, so either list
//!   refuses it with HTTP 400 `ValidationException` and the same sentence, and
//!   reports nothing.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::DEFAULT_PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, create_campaign, error_code, router, Harness};
use super::connect_instance_integrations::{list_integrations, onboard, put_integration, summaries};

/// The region every case here signs for: both stores are regional and one region is
/// enough to hold a list of each kind.
const REGION: &str = "us-east-1";

/// The sentence either list refuses a cursor it did not issue with, written out
/// here so a case pins the wording rather than whatever the handler happens to
/// return.
const UNISSUED_CURSOR_MESSAGE: &str = "nextToken is not a cursor this service issued";

/// How many records each store holds under test. Smaller than [`DEFAULT_PAGE_SIZE`],
/// so an unpaged call reads the whole set, and large enough that the page sizes
/// below divide it evenly, unevenly, and not at all.
const RECORDS: usize = 5;

/// One page of a list: the ids (or identifying ARNs) it reported and the cursor it
/// handed out, if any.
#[derive(Debug)]
struct Page {
    items: Vec<String>,
    next_token: Option<String>,
}

/// `ListCampaigns` for one page: `max_results` and `next_token` when given.
async fn campaigns_page(
    router: &MultiServiceRouter,
    instance: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
    case: &str,
) -> Page {
    let mut request = json!({"filters": {"instanceIdFilter": {"value": instance, "operator": "Eq"}}});
    let object = request.as_object_mut().expect("an object request");
    if let Some(max_results) = max_results {
        object.insert("maxResults".to_string(), json!(max_results));
    }
    if let Some(token) = next_token {
        object.insert("nextToken".to_string(), json!(token));
    }
    let (status, body) = call(router, REGION, "POST", "/v2/campaigns-summary", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListCampaigns, got {status} {body}"
    );
    Page {
        items: body["campaignSummaryList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["id"].as_str().unwrap_or("<no id>").to_string())
            .collect(),
        next_token: body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// `ListConnectInstanceIntegrations` for one page: `max_results` and `next_token`
/// when given, each as the query parameter the operation binds it to.
async fn integrations_page(
    router: &MultiServiceRouter,
    instance: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
    case: &str,
) -> Page {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let query = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    let (status, body) = list_integrations(router, instance, &query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListConnectInstanceIntegrations, \
         got {status} {body}"
    );
    Page {
        items: summaries(&body)
            .iter()
            .map(|summary| {
                summary["lambda"]["functionArn"]
                    .as_str()
                    .unwrap_or("<no functionArn>")
                    .to_string()
            })
            .collect(),
        next_token: body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Which list a case is driving: the two operations that page through the shared
/// window computation.
#[derive(Debug, Clone, Copy)]
enum List {
    Campaigns,
    Integrations,
}

impl List {
    /// One page of this list.
    async fn page(
        self,
        router: &MultiServiceRouter,
        instance: &str,
        max_results: Option<usize>,
        next_token: Option<&str>,
        case: &str,
    ) -> Page {
        match self {
            Self::Campaigns => {
                campaigns_page(router, instance, max_results, next_token, case).await
            }
            Self::Integrations => {
                integrations_page(router, instance, max_results, next_token, case).await
            }
        }
    }

    /// Walk this list at `max_results`, following every cursor it hands out, and
    /// report the pages in page order.
    async fn walk(
        self,
        router: &MultiServiceRouter,
        instance: &str,
        max_results: Option<usize>,
        case: &str,
    ) -> Vec<Page> {
        let mut pages: Vec<Page> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            assert!(
                pages.len() <= RECORDS,
                "case {case:?}: paging did not terminate after {} pages",
                pages.len()
            );
            let page = self
                .page(router, instance, max_results, token.as_deref(), case)
                .await;
            token = page.next_token.clone();
            pages.push(page);
            if token.is_none() {
                return pages;
            }
        }
    }
}

/// `RECORDS` campaigns on their own instance, in creation order, and `RECORDS`
/// Lambda integrations on their own, in put order — so each list under test holds a
/// set of the same size whose stored order is the order it was written in.
async fn seed(router: &Harness) -> (String, Vec<String>, String, Vec<String>) {
    let campaign_instance = "cursor-rule-campaigns".to_string();
    let mut campaigns: Vec<String> = Vec::new();
    for n in 0..RECORDS {
        campaigns.push(
            create_campaign(
                router,
                REGION,
                &campaign_instance,
                &format!("cursor-rule-{n:02}"),
                None,
            )
            .await,
        );
    }

    let integration_instance = "cursor-rule-integrations".to_string();
    onboard(router, &integration_instance).await;
    let mut integrations: Vec<String> = Vec::new();
    for n in 0..RECORDS {
        let function_arn =
            format!("arn:aws:lambda:us-east-1:000000000000:function:cursor-rule-{n:02}");
        let (status, body) = put_integration(
            router,
            &integration_instance,
            &json!({"lambda": {"functionArn": function_arn}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"seed {function_arn}\": expected HTTP 200 from the put, got {status} {body}"
        );
        integrations.push(function_arn);
    }

    (
        campaign_instance,
        campaigns,
        integration_instance,
        integrations,
    )
}

#[tokio::test]
async fn both_lists_page_the_same_window_at_every_page_size() {
    let router = router();
    let (campaign_instance, campaigns, integration_instance, integrations) = seed(&router).await;

    // Each row is a page size the model admits (`MaxResults`, range 1..50), chosen so
    // the stored set divides evenly (1, 5), unevenly (2, 3) and not at all (RECORDS +
    // 1 and the published maximum, both larger than the set) — plus the call that
    // names no page size at all, which reports as much as one page can carry.
    let page_sizes: Vec<Option<usize>> = vec![
        Some(1),
        Some(2),
        Some(3),
        Some(RECORDS),
        Some(RECORDS + 1),
        Some(DEFAULT_PAGE_SIZE),
        None,
    ];

    for max_results in page_sizes {
        let effective = max_results.unwrap_or(DEFAULT_PAGE_SIZE);
        // The window the rule names, independent of either store: full pages until
        // the records run out.
        let expected_sizes: Vec<usize> = {
            let mut sizes: Vec<usize> = Vec::new();
            let mut remaining = RECORDS;
            loop {
                let taken = remaining.min(effective);
                sizes.push(taken);
                remaining -= taken;
                if remaining == 0 {
                    break;
                }
            }
            sizes
        };

        for (list, instance, stored) in [
            (List::Campaigns, &campaign_instance, &campaigns),
            (List::Integrations, &integration_instance, &integrations),
        ] {
            let case = format!("{list:?} at maxResults={max_results:?}");
            let pages = list.walk(&router, instance, max_results, &case).await;

            let sizes: Vec<usize> = pages.iter().map(|page| page.items.len()).collect();
            assert_eq!(
                sizes, expected_sizes,
                "case {case:?}: expected pages of {expected_sizes:?} over {RECORDS} \
                 stored records, got {sizes:?}"
            );

            let walked: Vec<String> = pages
                .iter()
                .flat_map(|page| page.items.iter().cloned())
                .collect();
            assert_eq!(
                &walked, stored,
                "case {case:?}: expected every stored record exactly once in stored \
                 order, got {walked:?}"
            );

            // A cursor comes with every page but the last: that is how a client knows
            // to stop.
            let cursors: Vec<bool> = pages
                .iter()
                .map(|page| page.next_token.is_some())
                .collect();
            let expected_cursors: Vec<bool> = (0..pages.len())
                .map(|index| index + 1 < pages.len())
                .collect();
            assert_eq!(
                cursors, expected_cursors,
                "case {case:?}: expected a cursor on every page but the last, got {cursors:?}"
            );
        }
    }
}

#[tokio::test]
async fn both_lists_refuse_a_cursor_the_service_did_not_issue_alike() {
    let router = router();
    let (campaign_instance, campaigns, integration_instance, integrations) = seed(&router).await;

    // Each row is a `nextToken` this service's codec did not mint: a word, a decimal
    // position, hex of something that is not a sequence number, hex of a negative
    // one, and odd-length hex that is not a byte string at all.
    let tokens: Vec<(&str, String)> = vec![
        ("a word", "next-page".to_string()),
        ("a decimal position", "2".to_string()),
        ("hex of a non-number", ::hex::encode("second")),
        ("hex of a negative number", ::hex::encode("-2")),
        ("odd-length hex", "abc".to_string()),
    ];

    for (token_case, token) in tokens {
        for (list, instance, stored) in [
            (List::Campaigns, &campaign_instance, &campaigns),
            (List::Integrations, &integration_instance, &integrations),
        ] {
            let case = format!("{list:?} resuming from {token_case}");
            let (status, body) = match list {
                List::Campaigns => {
                    call(
                        &router,
                        REGION,
                        "POST",
                        "/v2/campaigns-summary",
                        &json!({"nextToken": token}),
                    )
                    .await
                }
                List::Integrations => {
                    list_integrations(&router, instance, &format!("?nextToken={token}")).await
                }
            };
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a cursor this service did not \
                 issue, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {}",
                error_code(&body)
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(UNISSUED_CURSOR_MESSAGE),
                "case {case:?}: expected {UNISSUED_CURSOR_MESSAGE:?}, got {body}"
            );

            // The refusal reported nothing and changed nothing: the stored set reads
            // back whole.
            let after = list
                .walk(&router, instance, None, &case)
                .await
                .into_iter()
                .flat_map(|page| page.items)
                .collect::<Vec<String>>();
            assert_eq!(
                &after, stored,
                "case {case:?}: a refused page must change nothing, got {after:?}"
            );
        }
    }
}
