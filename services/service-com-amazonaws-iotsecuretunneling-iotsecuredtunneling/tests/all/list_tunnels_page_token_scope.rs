// SPDX-License-Identifier: CC0-1.0

//! A `ListTunnels` page token resumes only the listing that issued it.
//!
//! The rule under test: a continuation token names a position in ONE listing —
//! the Region whose endpoint serves it together with the `thingName` it filters
//! by — so the service accepts it only on a replay of that same listing and
//! refuses it as invalid input anywhere else, instead of applying a position
//! that listing never held.
//!
//! The table below is the CROSS PRODUCT of a set of listings: every token is
//! minted in one row and replayed in every row, and each replay is expected to
//! be served exactly when the two rows name the same listing. The rows differ
//! along everything the rule generalises over — the Region, a Region label that
//! is a strict prefix of another, the presence of a `thingName` filter, the
//! filter's value, and whether the request carries a credential scope at all
//! (which names the default Region, so two differently-shaped requests can name
//! the SAME listing and must then share their cursors).
//!
//! The tunnels are opened Region by Region, so the cross-listing replays cover
//! all three ways a position of a foreign listing could be applied here: a
//! sequence higher than every tunnel of the called listing (which would restart
//! it), one lower than every tunnel (which would empty it) and one in the middle
//! (which would serve a truncated page).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::DEFAULT_REGION;
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service, over the shared engine.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation the way an SDK does, signed for `region` — or, when
/// `region` is `None`, with no `Authorization` header at all, which is how a
/// request naming no credential scope arrives.
async fn call(
    router: &MultiServiceRouter,
    region: Option<&str>,
    operation: &str,
    input: &Value,
) -> (u16, Value) {
    let body = serde_json::to_vec(input).expect("serialize the request body");
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("IoTSecuredTunneling.{operation}"));
    if let Some(region) = region {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/IoTSecuredTunneling/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        );
    }
    let response = router
        .dispatch(request.body(bytes::Bytes::from(body)).expect("build the request"))
        .await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Whether `token` satisfies `NextToken`'s modelled pattern
/// `^[a-zA-Z0-9_=-]{1,4096}$` (from
/// `smithy_query(query="operation", operation="ListTunnels")`), so a token is
/// known to reach the handler rather than stopping at the boundary validator.
fn is_modelled_next_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 4096
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_=-".contains(c))
}

/// One listing: the request form that addresses it, and the filter it carries.
struct Listing {
    label: &'static str,
    /// The credential scope the call is signed with; `None` carries no scope,
    /// which addresses [`DEFAULT_REGION`].
    scope: Option<&'static str>,
    /// The `thingName` the listing filters by, or `None` for an unfiltered one.
    thing_name: Option<&'static str>,
}

impl Listing {
    /// The listing this request form really names: the Region it resolves to,
    /// and the filter. Two rows with the same identity are ONE listing however
    /// differently their requests are built.
    fn identity(&self) -> (&str, Option<&str>) {
        (self.scope.unwrap_or(DEFAULT_REGION), self.thing_name)
    }

    /// The `ListTunnels` body for this listing, with `extra` members merged in.
    fn request(&self, extra: Value) -> Value {
        let mut input = json!({});
        if let Some(thing_name) = self.thing_name {
            input["thingName"] = Value::String(thing_name.to_string());
        }
        if let Some(members) = extra.as_object() {
            for (name, value) in members {
                input[name.as_str()] = value.clone();
            }
        }
        input
    }
}

/// The listings the matrix runs over.
fn listings() -> Vec<Listing> {
    vec![
        Listing {
            label: "us-east-1, unfiltered",
            scope: Some("us-east-1"),
            thing_name: None,
        },
        // Another Region entirely: its own listing, its own cursors.
        Listing {
            label: "eu-west-1, unfiltered",
            scope: Some("eu-west-1"),
            thing_name: None,
        },
        // A Region label that is a strict PREFIX of the next row's, which is
        // what pins the store's Region-keyed walk as well as the cursor scope.
        Listing {
            label: "us-west-2, unfiltered",
            scope: Some("us-west-2"),
            thing_name: None,
        },
        Listing {
            label: "us-west-20, unfiltered",
            scope: Some("us-west-20"),
            thing_name: None,
        },
        // Same Region as the first row, narrowed by a filter: a different
        // listing, because it reports a different set of tunnels.
        Listing {
            label: "us-east-1, filtered by thing-a",
            scope: Some("us-east-1"),
            thing_name: Some("thing-a"),
        },
        // Same Region, a different filter value.
        Listing {
            label: "us-east-1, filtered by thing-b",
            scope: Some("us-east-1"),
            thing_name: Some("thing-b"),
        },
        // Same filter value, a different Region.
        Listing {
            label: "eu-west-1, filtered by thing-a",
            scope: Some("eu-west-1"),
            thing_name: Some("thing-a"),
        },
        // No credential scope, so these two name the default Region: the SAME
        // listings as the "us-east-1" rows above, reached by another request
        // form, and their cursors must therefore be interchangeable.
        Listing {
            label: "unsigned (default Region), unfiltered",
            scope: None,
            thing_name: None,
        },
        Listing {
            label: "unsigned (default Region), filtered by thing-a",
            scope: None,
            thing_name: Some("thing-a"),
        },
    ]
}

/// Open `count` tunnels in `region` whose destination names `thing_name`.
async fn open_tunnels(
    router: &MultiServiceRouter,
    region: Option<&str>,
    thing_name: &str,
    count: usize,
) {
    for index in 0..count {
        let (status, opened) = call(
            router,
            region,
            "OpenTunnel",
            &json!({
                "description": format!("{thing_name}-{index}"),
                "destinationConfig": {"thingName": thing_name, "services": ["SSH"]},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "OpenTunnel for {thing_name:?} in {region:?} answered {opened:?}"
        );
    }
}

/// The tunnel ids a `ListTunnels` body reports, in the order it reported them.
fn reported(body: &Value) -> Vec<String> {
    body["tunnelSummaries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|summary| summary["tunnelId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every listing's tunnels, so each row of the matrix pages over at least two.
async fn open_the_store(router: &MultiServiceRouter) {
    // One batch per Region, in Region order, so the creation sequences of two
    // Regions interleave rather than nest: a cursor of one Region then lands
    // above, below and inside another Region's range of sequences.
    open_tunnels(router, Some("us-east-1"), "thing-plain", 2).await;
    open_tunnels(router, Some("eu-west-1"), "thing-plain", 2).await;
    open_tunnels(router, Some("us-west-2"), "thing-plain", 2).await;
    open_tunnels(router, Some("us-west-20"), "thing-plain", 2).await;
    open_tunnels(router, Some("us-east-1"), "thing-a", 2).await;
    open_tunnels(router, Some("us-east-1"), "thing-b", 2).await;
    open_tunnels(router, Some("eu-west-1"), "thing-a", 2).await;
}

#[tokio::test]
async fn a_page_token_is_served_only_by_the_listing_that_issued_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);
    open_the_store(&router).await;

    let listings = listings();

    // The whole of each listing, and the token its first page of one mints.
    let mut whole: Vec<Vec<String>> = Vec::with_capacity(listings.len());
    let mut tokens: Vec<String> = Vec::with_capacity(listings.len());
    for listing in &listings {
        let (status, listed) = call(
            &router,
            listing.scope,
            "ListTunnels",
            &listing.request(json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "listing {:?}: ListTunnels answered {listed:?}",
            listing.label
        );
        let ids = reported(&listed);
        assert!(
            ids.len() >= 2,
            "listing {:?}: the matrix needs a listing of at least two tunnels, got {ids:?}",
            listing.label
        );
        assert!(
            listed.get("nextToken").is_none(),
            "listing {:?}: a listing that fits in one page mints no token, got {:?}",
            listing.label,
            listed.get("nextToken")
        );

        let (status, first_page) = call(
            &router,
            listing.scope,
            "ListTunnels",
            &listing.request(json!({"maxResults": 1})),
        )
        .await;
        assert_eq!(
            status, 200,
            "listing {:?}: a first page of one answered {first_page:?}",
            listing.label
        );
        assert_eq!(
            reported(&first_page),
            ids[..1].to_vec(),
            "listing {:?}: the first page must report the newest tunnel",
            listing.label
        );
        let token = first_page
            .get("nextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "listing {:?}: a paged listing must mint a nextToken, got {first_page:?}",
                    listing.label
                )
            })
            .to_string();
        assert!(
            is_modelled_next_token(&token),
            "listing {:?}: minted token {token:?} must match the modelled pattern",
            listing.label
        );
        whole.push(ids);
        tokens.push(token);
    }

    // A token of one listing is served by that listing and refused by every
    // other, whichever direction the pair is taken in.
    for (minted_in, token) in listings.iter().zip(&tokens) {
        for (replayed_in, expected_rest) in listings.iter().zip(&whole) {
            let case = format!(
                "token minted in [{}] replayed in [{}]",
                minted_in.label, replayed_in.label
            );
            let (status, body) = call(
                &router,
                replayed_in.scope,
                "ListTunnels",
                &replayed_in.request(json!({"nextToken": token})),
            )
            .await;

            if minted_in.identity() == replayed_in.identity() {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the listing that issued the token must serve it, \
                     answered {body:?}"
                );
                assert_eq!(
                    reported(&body),
                    expected_rest[1..].to_vec(),
                    "case {case:?}: the token must resume after its own page, got {:?}",
                    reported(&body)
                );
                continue;
            }

            assert_eq!(
                status, 400,
                "case {case:?}: a token of another listing must be refused, answered {body:?}"
            );
            assert_eq!(
                body["__type"].as_str(),
                Some("ValidationException"),
                "case {case:?}: answered {body:?}"
            );
            assert!(
                body["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("nextToken")),
                "case {case:?}: the refusal must name the member to fix, answered {:?}",
                body["message"]
            );
            assert!(
                body.get("tunnelSummaries").is_none(),
                "case {case:?}: a refusal must serve no page, answered {body:?}"
            );
        }
    }

    // The refusals above are about the listing and nothing else: every listing
    // still pages over its own tunnels, at every page size, exactly once.
    for (listing, expected) in listings.iter().zip(&whole) {
        for page_size in 1..=3_i64 {
            let mut collected: Vec<String> = Vec::new();
            let mut next: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut input = listing.request(json!({"maxResults": page_size}));
                if let Some(token) = next.as_ref() {
                    input["nextToken"] = Value::String(token.clone());
                }
                let (status, listed) = call(&router, listing.scope, "ListTunnels", &input).await;
                assert_eq!(
                    status, 200,
                    "listing {:?} at page size {page_size}: answered {listed:?}",
                    listing.label
                );
                collected.extend(reported(&listed));
                pages += 1;
                assert!(
                    pages <= 20,
                    "listing {:?} at page size {page_size}: paging did not terminate after \
                     {pages} pages",
                    listing.label
                );
                match listed.get("nextToken").and_then(Value::as_str) {
                    Some(token) => next = Some(token.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                &collected, expected,
                "listing {:?} at page size {page_size}: paging must cover the listing once, in \
                 order, got {collected:?}",
                listing.label
            );
        }
    }
}
