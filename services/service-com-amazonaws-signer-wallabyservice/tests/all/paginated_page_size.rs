// SPDX-License-Identifier: CC0-1.0

//! How much one page of a paginated read carries, end to end over HTTP, for
//! EVERY paginated read of this service.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! `@httpQuery` bindings, deserialization, the handler, the persisted resources
//! and serialization — the same path a client takes. The one rule under test,
//! which `ListSigningProfiles`, `ListSigningPlatforms` and `ListSigningJobs` all
//! follow and all now compute in one place:
//!
//! * a page carries at most the `maxResults` the request named, and at most the
//!   operation's own page limit — the `max` of the range its `maxResults` member
//!   is modelled with — when the request named none;
//! * a page that stops short of the listing hands out a `nextToken`, and the
//!   final page of a listing hands out none, so walking a listing at any page
//!   size reports the same resources as walking it at any other.
//!
//! The rows differ along what the rule generalises over: which paginated read is
//! called, whether the listing holds more resources than the page limit or
//! fewer, and the page size asked for (absent, each published bound of
//! `MaxResults`, sizes in between, and one past each bound).

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::SIGNING_PLATFORM_CATALOG;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite calls through.
const REGION: &str = "us-west-2";

/// A platform this service publishes that signs through the S3 code-signing
/// workflow `StartSigningJob` serves.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// An ACM certificate ARN for `signingMaterial`, so a profile can be created on
/// a platform that requires the certificate its signatures are issued under.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// `MaxResults`'s modelled bounds — `@range(min: 1, max: 25)` on every paginated
/// read of this service (`smithy_query(query="operation",
/// operation="ListSigningProfiles,ListSigningPlatforms,ListSigningJobs")`).
const MAX_RESULTS_FLOOR: i32 = 1;
const MAX_RESULTS_CEILING: i32 = 25;

/// The modelled error shape a page size outside that range is refused with
/// (HTTP 400).
const ERROR_SHAPE_VALIDATION: &str = "ValidationException";

/// How many profiles this suite seeds: more than the page limit, so the page a
/// request naming no `maxResults` gets is cut at the limit rather than holding
/// the whole listing.
const SEEDED_PROFILES: usize = MAX_RESULTS_CEILING as usize + 1;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status and JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }

    /// The `nextToken` this answer carries, or `None` on a final page.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/signer/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One paginated read: where it is called, what it reports and how many
/// resources the seeded emulator holds for it.
struct Listing {
    case: &'static str,
    /// The resource collection the read is bound to.
    path: &'static str,
    /// The output member the page comes back in.
    member: &'static str,
    /// The member each reported resource is identified by.
    id_member: &'static str,
    /// How many resources the whole listing holds.
    total: usize,
}

impl Listing {
    /// The ids `answer` reported for this listing, in the order it reported them.
    fn ids(&self, answer: &Answer) -> Vec<String> {
        answer.body[self.member]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a {} list, got {:?}",
                    self.case, self.member, answer.body
                )
            })
            .iter()
            .map(|reported| {
                reported[self.id_member]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }
}

/// Seed the emulator and answer the paginated reads under test, each with the
/// number of resources it then enumerates.
///
/// The profile and job listings hold MORE resources than the page limit; the
/// platform catalog is a published fact, so its size is whatever this service
/// publishes — fewer than the limit today, which is the other side of the rule.
async fn seed(router: &MultiServiceRouter) -> Vec<Listing> {
    for index in 0..SEEDED_PROFILES {
        let name = profile_name(index);
        let answer = call(
            router,
            "PUT",
            &format!("/signing-profiles/{name}"),
            Some(json!({
                "platformId": PLATFORM_ID,
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "seeding profile {name}: expected 200, got {} {:?}",
            answer.status, answer.body
        );

        let started = call(
            router,
            "POST",
            "/signing-jobs",
            Some(json!({
                "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
                "destination": { "s3": { "bucketName": "dst", "prefix": "signed/" } },
                "profileName": name,
                "clientRequestToken": format!("tok-{name}"),
            })),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "starting a job for {name}: expected 200, got {} {:?}",
            started.status, started.body
        );
        assert!(
            !started.str_member("jobId").is_empty(),
            "starting a job for {name}: expected a job id, got {:?}",
            started.body
        );
    }

    vec![
        Listing {
            case: "ListSigningProfiles, a listing larger than the page limit",
            path: "/signing-profiles",
            member: "profiles",
            id_member: "profileName",
            total: SEEDED_PROFILES,
        },
        Listing {
            case: "ListSigningJobs, a listing larger than the page limit",
            path: "/signing-jobs",
            member: "jobs",
            id_member: "jobId",
            total: SEEDED_PROFILES,
        },
        Listing {
            case: "ListSigningPlatforms, the published catalog",
            path: "/signing-platforms",
            member: "platforms",
            id_member: "platformId",
            total: SIGNING_PLATFORM_CATALOG.len(),
        },
    ]
}

/// The name of the `index`th seeded profile — zero padded, so the seeding order
/// is also name order and a row reads the same whatever the page size.
fn profile_name(index: usize) -> String {
    format!("Page_Profile_{index:02}")
}

/// One page of `listing`, asking for `max_results` (`None` naming none at all)
/// and resuming `token`.
async fn page(
    router: &MultiServiceRouter,
    listing: &Listing,
    max_results: Option<i32>,
    token: Option<&str>,
) -> Answer {
    let mut query = String::new();
    if let Some(max_results) = max_results {
        query.push_str(&format!("?maxResults={max_results}"));
    }
    if let Some(token) = token {
        query.push(if query.is_empty() { '?' } else { '&' });
        query.push_str(&format!("nextToken={token}"));
    }
    call(router, "GET", &format!("{}{query}", listing.path), None).await
}

/// Walk `listing` with the tokens it hands out at page size `max_results`,
/// answering every id reported in order and asserting each page respects the
/// cap `expected_cap`.
async fn walk(
    router: &MultiServiceRouter,
    listing: &Listing,
    max_results: Option<i32>,
    expected_cap: usize,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..=listing.total {
        let answer = page(router, listing, max_results, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {:?} at maxResults {max_results:?}: expected 200, got {} {:?}",
            listing.case, answer.status, answer.body
        );
        let reported = listing.ids(&answer);
        assert!(
            reported.len() <= expected_cap,
            "case {:?} at maxResults {max_results:?}: expected at most {expected_cap} per page, \
             got {} ({reported:?})",
            listing.case,
            reported.len()
        );
        ids.extend(reported);
        match answer.next_token() {
            Some(next) => token = Some(next),
            None => return ids,
        }
    }
    panic!(
        "case {:?} at maxResults {max_results:?}: walking the listing never reached a final page",
        listing.case
    );
}

/// A request naming no `maxResults` is served the operation's page limit: the
/// first page carries the limit when the listing holds more than that, and the
/// whole listing when it holds fewer — and it hands out a token exactly when
/// something is left.
#[tokio::test]
async fn an_absent_max_results_serves_the_operations_page_limit() {
    let router = emulator();
    let listings = seed(&router).await;

    for listing in &listings {
        let limit = MAX_RESULTS_CEILING as usize;
        let answer = page(&router, listing, None, None).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            listing.case, answer.status, answer.body
        );
        let reported = listing.ids(&answer);
        assert_eq!(
            reported.len(),
            listing.total.min(limit),
            "case {:?}: expected the smaller of the listing ({}) and the page limit ({limit}), \
             got {} ({reported:?})",
            listing.case,
            listing.total,
            reported.len()
        );
        assert_eq!(
            answer.next_token().is_some(),
            listing.total > limit,
            "case {:?}: a token is handed out exactly when the page stops short, got {:?}",
            listing.case,
            answer.body["nextToken"]
        );

        // Naming the page limit explicitly is the same request, so it gets the
        // same page: that is what "absent defaults to the limit" means.
        let explicit = page(&router, listing, Some(MAX_RESULTS_CEILING), None).await;
        assert_eq!(
            listing.ids(&explicit),
            reported,
            "case {:?}: naming maxResults {MAX_RESULTS_CEILING} must report the page an absent \
             maxResults reports, got {:?}",
            listing.case,
            explicit.body[listing.member]
        );
    }
}

/// Every page size the model allows caps every page of every paginated read, and
/// the walk it cuts the listing into reports the same resources as the walk at
/// any other page size.
#[tokio::test]
async fn a_named_max_results_caps_every_page_of_every_paginated_read() {
    let router = emulator();
    let listings = seed(&router).await;

    // Both published bounds, and sizes in between — including one that divides
    // a listing evenly and one that does not.
    let page_sizes = [
        MAX_RESULTS_FLOOR,
        2,
        3,
        MAX_RESULTS_CEILING - 1,
        MAX_RESULTS_CEILING,
    ];

    for listing in &listings {
        let reference: BTreeSet<String> = walk(
            &router,
            listing,
            Some(MAX_RESULTS_FLOOR),
            MAX_RESULTS_FLOOR as usize,
        )
        .await
        .into_iter()
        .collect();
        assert_eq!(
            reference.len(),
            listing.total,
            "case {:?}: walking one at a time must report every resource once, got {reference:?}",
            listing.case
        );

        for page_size in page_sizes {
            let walked = walk(&router, listing, Some(page_size), page_size as usize).await;
            let unique: BTreeSet<String> = walked.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                walked.len(),
                "case {:?} at maxResults {page_size}: expected no resource twice, got {walked:?}",
                listing.case
            );
            assert_eq!(
                unique, reference,
                "case {:?} at maxResults {page_size}: expected the same resources as the \
                 one-at-a-time walk, got {walked:?}",
                listing.case
            );
        }
    }
}

/// A `maxResults` outside the modelled range reaches no page-size computation at
/// all: it is refused 400 with the modelled error shape, by every paginated read.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    let listings = seed(&router).await;

    for listing in &listings {
        for page_size in [MAX_RESULTS_FLOOR - 1, MAX_RESULTS_CEILING + 1] {
            let answer = page(&router, listing, Some(page_size), None).await;
            assert_eq!(
                answer.status, 400,
                "case {:?} at maxResults {page_size}: a size outside the modelled range must be \
                 refused, got {} {:?}",
                listing.case, answer.status, answer.body
            );
            assert!(
                answer.shape().ends_with(ERROR_SHAPE_VALIDATION),
                "case {:?} at maxResults {page_size}: expected the \
                 {ERROR_SHAPE_VALIDATION} shape, got {:?}",
                listing.case,
                answer.body
            );
        }
    }
}
