// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` is a resume point for the listing that minted it and for no other,
//! driven end to end over the wire through every paged listing this service serves.
//!
//! A cursor names a position in ONE listing's stored order. The paged lists of this
//! service draw their positions from counters that advance independently, and a
//! listing narrowed to one region, instance or filter reports a different set from a
//! listing narrowed to another, so the same position value names unrelated records in
//! each. The rules these cases hold every listing to:
//!
//! * a listing resumes from its OWN cursor: the page after it is the rest of that
//!   listing, in stored order;
//! * a cursor minted by any OTHER listing names no position in this one, so it is
//!   refused with HTTP 400 `ValidationException` and the same sentence a cursor this
//!   service never minted is refused with, reporting nothing and changing nothing;
//! * because the token carries the listing that minted it, two listings that mint a
//!   cursor at the same position still mint different cursors.
//!
//! The listings under test are the ones the two paged operations span: campaigns
//! filtered to one instance, to another, unfiltered, and through a second region's
//! endpoint, plus one instance's integrations, another's, and the same instance's
//! through that second region.

use serde_json::{json, Value};

use super::common::{call, create_campaign, error_code, onboard_instance, router, Harness};

/// The region most listings under test are scoped to.
const REGION_A: &str = "us-east-1";

/// A second region, so a cursor can cross an endpoint boundary: both stores are
/// regional, so the same instance holds a different set of records in each.
const REGION_B: &str = "us-west-2";

/// One instance under test.
const INSTANCE_A: &str = "cursor-scope-one";

/// A second instance, so a cursor can cross a filter or a path label as well.
const INSTANCE_B: &str = "cursor-scope-two";

/// How many records each listing under test holds: enough that a one-record page
/// mints a cursor, and that resuming from a FOREIGN cursor would silently drop
/// records rather than report an empty page.
const RECORDS: usize = 3;

/// The sentence every paged list refuses a cursor it did not issue with, written out
/// here so a case pins the wording rather than whatever the handler happens to
/// return.
const UNISSUED_CURSOR_MESSAGE: &str = "nextToken is not a cursor this service issued";

/// One paged listing: the operation and the result set it is scoped to.
#[derive(Debug)]
enum Listing {
    /// `ListCampaigns` through `region`'s endpoint, filtered to one instance when
    /// `instance_filter` names one.
    Campaigns {
        region: String,
        instance_filter: Option<String>,
    },
    /// `ListConnectInstanceIntegrations` for `instance` through `region`'s endpoint.
    Integrations { region: String, instance: String },
}

impl Listing {
    /// One page of this listing: `max_results` and `next_token` when given, each sent
    /// the way the operation binds it (a JSON member for campaigns, a query parameter
    /// for integrations).
    async fn page(
        &self,
        router: &Harness,
        max_results: Option<usize>,
        next_token: Option<&str>,
    ) -> (u16, Value) {
        match self {
            Self::Campaigns {
                region,
                instance_filter,
            } => {
                let mut request = json!({});
                let object = request.as_object_mut().expect("an object request");
                if let Some(instance) = instance_filter {
                    object.insert(
                        "filters".to_string(),
                        json!({"instanceIdFilter": {"value": instance, "operator": "Eq"}}),
                    );
                }
                if let Some(max_results) = max_results {
                    object.insert("maxResults".to_string(), json!(max_results));
                }
                if let Some(token) = next_token {
                    object.insert("nextToken".to_string(), json!(token));
                }
                call(router, region, "POST", "/v2/campaigns-summary", &request).await
            }
            Self::Integrations { region, instance } => {
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
                call(
                    router,
                    region,
                    "GET",
                    &format!("/v2/connect-instance/{instance}/integrations{query}"),
                    &Value::Null,
                )
                .await
            }
        }
    }

    /// The records a success body of this listing reports, each as the value that
    /// identifies it: a campaign's minted id, an integration's function ARN.
    fn items(&self, body: &Value) -> Vec<String> {
        match self {
            Self::Campaigns { .. } => body
                .get("campaignSummaryList")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|summary| summary["id"].as_str().unwrap_or("<no id>").to_string())
                .collect(),
            Self::Integrations { .. } => body
                .get("integrationSummaryList")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|summary| {
                    summary["lambda"]["functionArn"]
                        .as_str()
                        .unwrap_or("<no functionArn>")
                        .to_string()
                })
                .collect(),
        }
    }

    /// The cursor this listing's first one-record page hands out.
    async fn mint_token(&self, router: &Harness, case: &str) -> String {
        let (status, body) = self.page(router, Some(1), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the first page, got {status} {body}"
        );
        let Some(token) = body.get("nextToken").and_then(Value::as_str) else {
            panic!("case {case:?}: expected the first page to mint a nextToken, got {body}");
        };
        token.to_string()
    }

    /// Every record of this listing, read in one unpaged call — each listing holds
    /// fewer records than one page can carry.
    async fn read_all(&self, router: &Harness, case: &str) -> Vec<String> {
        let (status, body) = self.page(router, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the unpaged read, got {status} {body}"
        );
        self.items(&body)
    }
}

/// `RECORDS` campaigns on `instance` through `region`'s endpoint, in creation order.
async fn create_campaigns(router: &Harness, region: &str, instance: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for n in 0..RECORDS {
        ids.push(
            create_campaign(
                router,
                region,
                instance,
                &format!("cursor-scope-{region}-{instance}-{n:02}"),
                None,
            )
            .await,
        );
    }
    ids
}

/// `RECORDS` Lambda integrations on `instance` through `region`'s endpoint, in put
/// order, reported by the ARNs that identify them.
async fn put_integrations(router: &Harness, region: &str, instance: &str) -> Vec<String> {
    onboard_instance(router, region, instance).await;
    let mut arns: Vec<String> = Vec::new();
    for n in 0..RECORDS {
        let function_arn =
            format!("arn:aws:lambda:{region}:000000000000:function:cursor-scope-{n:02}");
        let (status, body) = call(
            router,
            region,
            "PUT",
            &format!("/v2/connect-instance/{instance}/integrations"),
            &json!({"integrationConfig": {"lambda": {"functionArn": function_arn}}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"seed {instance} in {region}\": expected HTTP 200 from the put of \
             {function_arn}, got {status} {body}"
        );
        arns.push(function_arn);
    }
    arns
}

/// Every listing under test, each beside the records it reports in stored order.
///
/// The rows differ along what the rule generalises over: the OPERATION the cursor
/// came from, the REGION the listing is scoped to, the INSTANCE it is narrowed to,
/// and whether it is narrowed at all.
async fn seed(router: &Harness) -> Vec<(Listing, Vec<String>)> {
    let campaigns_a_in_a = create_campaigns(router, REGION_A, INSTANCE_A).await;
    let campaigns_b_in_a = create_campaigns(router, REGION_A, INSTANCE_B).await;
    let campaigns_a_in_b = create_campaigns(router, REGION_B, INSTANCE_A).await;
    // The unfiltered listing of one region reports both of its instances' campaigns,
    // in the order they were created.
    let campaigns_in_a: Vec<String> = campaigns_a_in_a
        .iter()
        .chain(campaigns_b_in_a.iter())
        .cloned()
        .collect();

    let integrations_a_in_a = put_integrations(router, REGION_A, INSTANCE_A).await;
    let integrations_b_in_a = put_integrations(router, REGION_A, INSTANCE_B).await;
    let integrations_a_in_b = put_integrations(router, REGION_B, INSTANCE_A).await;

    vec![
        (
            Listing::Campaigns {
                region: REGION_A.to_string(),
                instance_filter: Some(INSTANCE_A.to_string()),
            },
            campaigns_a_in_a,
        ),
        (
            Listing::Campaigns {
                region: REGION_A.to_string(),
                instance_filter: Some(INSTANCE_B.to_string()),
            },
            campaigns_b_in_a,
        ),
        (
            Listing::Campaigns {
                region: REGION_A.to_string(),
                instance_filter: None,
            },
            campaigns_in_a,
        ),
        (
            Listing::Campaigns {
                region: REGION_B.to_string(),
                instance_filter: Some(INSTANCE_A.to_string()),
            },
            campaigns_a_in_b,
        ),
        (
            Listing::Integrations {
                region: REGION_A.to_string(),
                instance: INSTANCE_A.to_string(),
            },
            integrations_a_in_a,
        ),
        (
            Listing::Integrations {
                region: REGION_A.to_string(),
                instance: INSTANCE_B.to_string(),
            },
            integrations_b_in_a,
        ),
        (
            Listing::Integrations {
                region: REGION_B.to_string(),
                instance: INSTANCE_A.to_string(),
            },
            integrations_a_in_b,
        ),
    ]
}

#[tokio::test]
async fn a_cursor_resumes_the_listing_that_minted_it_and_no_other() {
    let router = router();
    let listings = seed(&router).await;

    // One cursor per listing, each minted by that listing's own first page.
    let mut tokens: Vec<String> = Vec::new();
    for (listing, _) in &listings {
        let case = format!("minting a cursor from {listing:?}");
        tokens.push(listing.mint_token(&router, &case).await);
    }

    for (resuming, (listing, records)) in listings.iter().enumerate() {
        for (minted_by, token) in tokens.iter().enumerate() {
            let source = &listings[minted_by].0;
            let case = format!("{listing:?} resuming from a cursor minted by {source:?}");
            let (status, body) = listing.page(&router, None, Some(token)).await;

            if minted_by == resuming {
                // Its own cursor is a position in its own order: the page after it is
                // the rest of this listing, in stored order.
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200 resuming from its own cursor, \
                     got {status} {body}"
                );
                let resumed = listing.items(&body);
                let expected: Vec<String> = records[1..].to_vec();
                assert_eq!(
                    resumed, expected,
                    "case {case:?}: expected the records after the first, {expected:?}, \
                     got {resumed:?}"
                );
            } else {
                // Any other listing's cursor names no position here at all.
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 for a cursor another listing \
                     minted, got {status} {body}"
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
                let reported = listing.items(&body);
                assert!(
                    reported.is_empty(),
                    "case {case:?}: a refused page reports nothing, got {reported:?}"
                );
            }

            // Either way the listing still reads back whole: no record was dropped
            // and nothing was consumed.
            let after = listing.read_all(&router, &case).await;
            assert_eq!(
                &after, records,
                "case {case:?}: expected the listing to read back whole, {records:?}, \
                 got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn two_listings_at_the_same_position_mint_different_cursors() {
    let router = router();
    let listings = seed(&router).await;

    // Every listing's first page ends at its own first record, so under a cursor that
    // carried a bare position two of these would be the same string.
    let mut minted: Vec<(String, String)> = Vec::new();
    for (listing, _) in &listings {
        let case = format!("minting a cursor from {listing:?}");
        let token = listing.mint_token(&router, &case).await;
        if let Some((earlier, earlier_token)) = minted
            .iter()
            .find(|(_, earlier_token)| *earlier_token == token)
        {
            panic!(
                "case {case:?}: expected a cursor of its own, got {token:?}, the one \
                 {earlier:?} minted ({earlier_token:?})"
            );
        }
        minted.push((format!("{listing:?}"), token));
    }
}
