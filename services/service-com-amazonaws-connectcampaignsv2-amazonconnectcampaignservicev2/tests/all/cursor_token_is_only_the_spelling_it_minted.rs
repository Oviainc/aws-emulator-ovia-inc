// SPDX-License-Identifier: CC0-1.0

//! A paged list resumes only from the exact token its own codec handed out, driven
//! end to end over the wire through every paged listing this service serves.
//!
//! The codec writes ONE token for a position in a listing, so any other string is a
//! cursor this service did not issue — including a string that merely spells the same
//! position differently. The rules these cases hold every listing to:
//!
//! * a re-spelling of a cursor the listing itself minted names no position in it, so
//!   it is refused with HTTP 400 `ValidationException` and the same sentence a cursor
//!   this service never minted is refused with, reporting nothing and changing
//!   nothing — rather than being served as a page that silently drops every record up
//!   to the position the re-spelling names;
//! * the token the listing did hand out still resumes it, so the refusal above turns
//!   away only strings the codec never wrote.
//!
//! The re-spellings are the ones the readers a decode walks through would otherwise
//! let through: `u64`'s `FromStr` takes a leading `+`, leading zeros and nothing else
//! around the digits, and a hex decoder takes upper-case digits a hex encoder never
//! emits.
//!
//! The listings under test are the ones the two paged operations span: campaigns
//! filtered to one instance and unfiltered, plus one instance's integrations through
//! each of two regions' endpoints.

use serde_json::{json, Value};

use super::common::{call, create_campaign, error_code, onboard_instance, router, Harness};

/// The region most listings under test are scoped to.
const REGION_A: &str = "us-east-1";

/// A second region, so a listing reached through another endpoint is covered too:
/// both stores are regional, so the same instance holds a different set in each.
const REGION_B: &str = "us-west-2";

/// The instance every listing under test is narrowed to.
const INSTANCE: &str = "cursor-spelling";

/// How many records each listing holds: enough that a one-record page mints a cursor,
/// and that resuming from a re-spelled cursor would silently drop records rather than
/// report an empty page.
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

    /// The records a body of this listing reports, each as the value that identifies
    /// it: a campaign's minted id, an integration's function ARN.
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
                &format!("cursor-spelling-{region}-{n:02}"),
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
            format!("arn:aws:lambda:{region}:000000000000:function:cursor-spelling-{n:02}");
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
/// came from, the REGION the listing is scoped to, and whether it is narrowed by a
/// filter at all.
async fn seed(router: &Harness) -> Vec<(Listing, Vec<String>)> {
    let campaigns_in_a = create_campaigns(router, REGION_A, INSTANCE).await;
    let integrations_in_a = put_integrations(router, REGION_A, INSTANCE).await;
    let integrations_in_b = put_integrations(router, REGION_B, INSTANCE).await;

    vec![
        (
            Listing::Campaigns {
                region: REGION_A.to_string(),
                instance_filter: Some(INSTANCE.to_string()),
            },
            campaigns_in_a.clone(),
        ),
        (
            // One instance is seeded in this region, so the unfiltered listing reports
            // exactly its campaigns.
            Listing::Campaigns {
                region: REGION_A.to_string(),
                instance_filter: None,
            },
            campaigns_in_a,
        ),
        (
            Listing::Integrations {
                region: REGION_A.to_string(),
                instance: INSTANCE.to_string(),
            },
            integrations_in_a,
        ),
        (
            Listing::Integrations {
                region: REGION_B.to_string(),
                instance: INSTANCE.to_string(),
            },
            integrations_in_b,
        ),
    ]
}

/// One way of re-spelling a cursor a listing minted, as the token string it rewrites
/// that cursor into.
///
/// `rewrite` is handed the minted token both decoded — the scope tag and the position
/// behind it, which is what the codec writes into a token — and verbatim, so a row
/// can rewrite either half of it.
struct Respelling {
    case: &'static str,
    rewrite: fn(&str, &str, &str) -> String,
}

/// Every re-spelling under test: the same position as the minted cursor, written the
/// ways the readers a decode walks through would otherwise accept.
///
/// The rows differ along what the rule generalises over: a sign before the digits, a
/// leading zero, several leading zeros, space around the digits, and the case of the
/// hex the token itself is written in.
fn respellings() -> Vec<Respelling> {
    vec![
        Respelling {
            case: "a leading plus before the position",
            rewrite: |tag, position, _| ::hex::encode(format!("{tag}:+{position}")),
        },
        Respelling {
            case: "one leading zero on the position",
            rewrite: |tag, position, _| ::hex::encode(format!("{tag}:0{position}")),
        },
        Respelling {
            case: "three leading zeros on the position",
            rewrite: |tag, position, _| ::hex::encode(format!("{tag}:000{position}")),
        },
        Respelling {
            case: "a space before the position",
            rewrite: |tag, position, _| ::hex::encode(format!("{tag}: {position}")),
        },
        Respelling {
            case: "the minted bytes in upper-case hex",
            rewrite: |_, _, minted| minted.to_uppercase(),
        },
    ]
}

/// The scope tag and the position a minted token carries, read back out of it the way
/// the decode side does.
fn decoded_halves(minted: &str, case: &str) -> (String, String) {
    let bytes = ::hex::decode(minted)
        .unwrap_or_else(|error| panic!("case {case:?}: expected hex, got {minted:?} ({error})"));
    let decoded = String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("case {case:?}: expected text, got {minted:?} ({error})"));
    let Some((tag, position)) = decoded.split_once(':') else {
        panic!("case {case:?}: expected a tagged position, got {decoded:?}");
    };
    (tag.to_string(), position.to_string())
}

#[tokio::test]
async fn a_listing_refuses_every_respelling_of_the_cursor_it_minted() {
    let router = router();
    let listings = seed(&router).await;

    for (listing, records) in &listings {
        let minted = listing
            .mint_token(&router, &format!("minting a cursor from {listing:?}"))
            .await;
        let (tag, position) = decoded_halves(&minted, &format!("the cursor of {listing:?}"));

        for respelling in respellings() {
            let token = (respelling.rewrite)(&tag, &position, &minted);
            let case = format!("{listing:?} resuming from {}", respelling.case);
            // A row that spelled the minted token itself would pass for the wrong
            // reason.
            assert_ne!(
                token, minted,
                "case {case:?}: expected a token the codec did not write, got the \
                 minted one"
            );

            let (status, body) = listing.page(&router, None, Some(&token)).await;
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
            let reported = listing.items(&body);
            assert!(
                reported.is_empty(),
                "case {case:?}: a refused page reports nothing, got {reported:?}"
            );

            // The refusal changed nothing: the listing still reads back whole, so no
            // record was dropped or consumed.
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
async fn a_listing_still_resumes_from_the_cursor_it_minted() {
    let router = router();
    let listings = seed(&router).await;

    for (listing, records) in &listings {
        let case = format!("{listing:?} resuming from its own cursor");
        let minted = listing.mint_token(&router, &case).await;

        let (status, body) = listing.page(&router, None, Some(&minted)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 resuming from the token it handed out, \
             got {status} {body}"
        );
        let resumed = listing.items(&body);
        let expected: Vec<String> = records[1..].to_vec();
        assert_eq!(
            resumed, expected,
            "case {case:?}: expected the records after the first, {expected:?}, got \
             {resumed:?}"
        );
    }
}
