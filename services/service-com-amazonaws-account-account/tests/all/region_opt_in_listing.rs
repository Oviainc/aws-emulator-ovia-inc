// SPDX-License-Identifier: CC0-1.0

//! `ListRegions` end to end over HTTP: the Regions of an account and their opt-in
//! statuses.
//!
//! The rule under test: `ListRegions` reports, for the resolved account (the
//! `AccountId` the request names, else the caller's own), every Region of the
//! service's Region catalog paired with the `RegionOptStatus` that account carries
//! for it — the status recorded against that Region for the account, else the
//! Region's catalogued default — keeping only the Regions whose status is one of
//! `RegionOptStatusContains` when the request names that member, and cutting the
//! kept Regions into pages of at most `MaxResults`, with `NextToken` present in a
//! response exactly when a further kept Region follows the last one on the page.
//!
//! Every case goes through the router, so it exercises routing, deserialization,
//! the boundary constraint walk, the handler, the persisted store and
//! serialization: the same path a client takes. The tables below differ along what
//! the rule generalises over — the filter (each `RegionOptStatus` value, several
//! values at once, none, and a status no Region carries), the page size (each
//! published bound of `MaxResults` and sizes in between), the account (the
//! caller's own and a named one), and the recorded per-account statuses.
//!
//! `EnableRegion` and `DisableRegion` are the operations that record a status
//! against a Region; both are still scaffolded (each with its own stub issue), so
//! the cases that need a recorded status write it into the same shared store those
//! handlers will write, through the engine the router was built on.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::{
    default_account_id, ComAmazonawsAccountAccountState,
};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The values `RegionOptStatus` admits, from
/// `smithy_query(query="shape", shape="RegionOptStatus")` — spelled out here, not
/// imported, so a status the enum does not admit cannot make this test pass.
const REGION_OPT_STATUSES: [&str; 5] = [
    "ENABLED",
    "ENABLING",
    "DISABLING",
    "DISABLED",
    "ENABLED_BY_DEFAULT",
];

/// `MaxResults`' modelled `@range(min: 1, max: 50)`, from
/// `smithy_query(query="operation", operation="ListRegions")`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 50;

/// `RegionName`'s modelled `@length(min: 1, max: 50)`, from
/// `smithy_query(query="shape", shape="RegionName")`.
const REGION_NAME_MIN: usize = 1;
const REGION_NAME_MAX: usize = 50;

/// Regions Amazon Web Services turns on for every account, which therefore report
/// `ENABLED_BY_DEFAULT` and cannot be opted out of. SOURCE: the AWS General
/// Reference Region table, minus the opt-in Regions the Account Management guide
/// lists under "Specify which AWS Regions your account can use". A sample of the
/// class, including Regions this issue never cited (`eu-north-1`, `ap-northeast-3`).
const SAMPLE_DEFAULT_ON_REGIONS: [&str; 5] = [
    "us-east-1",
    "eu-west-1",
    "eu-north-1",
    "ap-northeast-3",
    "sa-east-1",
];

/// Opt-in Regions: an account that has not enabled one reports `DISABLED` for it.
/// SOURCE: the opt-in Region list in the Account Management guide ("Specify which
/// AWS Regions your account can use"). Again a sample, including Regions this
/// issue never cited (`mx-central-1`, `eu-central-2`).
const SAMPLE_OPT_IN_REGIONS: [&str; 5] = [
    "af-south-1",
    "ap-east-1",
    "eu-central-2",
    "il-central-1",
    "mx-central-1",
];

/// One emulator: a router carrying only this service, over one shared engine —
/// returned beside the engine, so a case can seed the same store the handlers read.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Record `status` against `region_name` for `account_id`, the way `EnableRegion`
/// and `DisableRegion` will once they are implemented: one write to the shared
/// store the router's handlers read.
fn record_region_status(
    engine: &engine::InMemoryEngine,
    account_id: &str,
    region_name: &str,
    status: &str,
) {
    let state = engine.state::<ComAmazonawsAccountAccountState>();
    let mut store = state.lock();
    let account = store
        .account_mut(account_id, std::time::SystemTime::now())
        .expect("the store admits the account this test writes");
    account
        .region_opt_status
        .insert(region_name.to_string(), status.to_string());
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `Regions` list as `(RegionName, RegionOptStatus)` pairs, in the order
    /// the response carries them.
    fn regions(&self) -> Vec<(String, String)> {
        self.body["Regions"]
            .as_array()
            .unwrap_or_else(|| panic!("a ListRegions success carries Regions, got {:?}", self.body))
            .iter()
            .map(|region| {
                (
                    region["RegionName"]
                        .as_str()
                        .unwrap_or_else(|| panic!("every Region carries a RegionName: {region:?}"))
                        .to_string(),
                    region["RegionOptStatus"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("every Region carries a RegionOptStatus: {region:?}")
                        })
                        .to_string(),
                )
            })
            .collect()
    }

    /// The `NextToken` the response carries, if any.
    fn next_token(&self) -> Option<String> {
        self.body["NextToken"].as_str().map(str::to_string)
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
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
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn list_regions(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/listRegions", request).await
}

/// Every Region the caller's own account reports, in one page (`MaxResults` absent
/// asks for no page cut, and the catalog is inside the modelled maximum).
async fn whole_list(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let answer = list_regions(router, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case \"the whole list\": expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.regions()
}

/// The unfiltered list is the account's whole Region catalog: every entry carries
/// a `RegionName` inside its modelled length and a `RegionOptStatus` the enum
/// admits, the names are unique and ascending, and both classes of Region are
/// present with the status their class reports.
#[tokio::test]
async fn the_unfiltered_list_reports_every_region_with_a_modelled_status() {
    let (router, _engine) = emulator();
    let regions = whole_list(&router).await;

    assert!(
        regions.len() >= SAMPLE_DEFAULT_ON_REGIONS.len() + SAMPLE_OPT_IN_REGIONS.len(),
        "case \"the whole list\": expected at least the sampled Regions, got {:?}",
        regions
    );

    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut previous: Option<&str> = None;
    for (name, status) in &regions {
        assert!(
            (REGION_NAME_MIN..=REGION_NAME_MAX).contains(&name.len()),
            "case {name:?}: a RegionName must be {REGION_NAME_MIN}..{REGION_NAME_MAX} \
             characters, got {} ",
            name.len()
        );
        assert!(
            REGION_OPT_STATUSES.contains(&status.as_str()),
            "case {name:?}: expected one of {REGION_OPT_STATUSES:?}, got {status:?}"
        );
        assert!(
            seen.insert(name.as_str()),
            "case {name:?}: a Region must be reported once, got it twice in {regions:?}"
        );
        assert!(
            previous.is_none_or(|earlier| earlier < name.as_str()),
            "case {name:?}: the list must ascend by RegionName, got {previous:?} before it"
        );
        previous = Some(name.as_str());
    }

    for region in SAMPLE_DEFAULT_ON_REGIONS {
        let status = status_of(&regions, region);
        assert_eq!(
            status, "ENABLED_BY_DEFAULT",
            "case {region:?}: a Region AWS turns on by default reports \
             ENABLED_BY_DEFAULT, got {status:?}"
        );
    }
    for region in SAMPLE_OPT_IN_REGIONS {
        let status = status_of(&regions, region);
        assert_eq!(
            status, "DISABLED",
            "case {region:?}: an opt-in Region nobody enabled reports DISABLED, \
             got {status:?}"
        );
    }
}

/// The status reported for `region_name`, or a panic naming the whole list when it
/// is absent.
fn status_of(regions: &[(String, String)], region_name: &str) -> String {
    regions
        .iter()
        .find(|(name, _)| name == region_name)
        .map(|(_, status)| status.clone())
        .unwrap_or_else(|| panic!("case {region_name:?}: expected it in the list, got {regions:?}"))
}

/// A read reports the store and never changes it: two identical calls answer the
/// same list, and a `ListRegions` against an account the store does not hold
/// creates nothing.
#[tokio::test]
async fn listing_regions_changes_nothing_a_later_caller_sees() {
    let (router, _engine) = emulator();
    let first = whole_list(&router).await;
    let second = whole_list(&router).await;
    assert_eq!(
        first, second,
        "case \"a repeated read\": a report must not change what the next caller sees"
    );

    // An account the emulator holds no record of: refused, and still unknown after
    // the refusal — the refused read created no record.
    let unknown = "210987654321";
    for attempt in ["first", "second"] {
        let answer = list_regions(&router, json!({"AccountId": unknown})).await;
        assert_eq!(
            answer.status, 403,
            "case \"an unknown account, {attempt} attempt\": expected 403, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "AccessDeniedException",
            "case \"an unknown account, {attempt} attempt\": expected \
             AccessDeniedException, got {:?}",
            answer.body
        );
    }
}

/// A named account the store knows reports the same catalog as the caller's own,
/// and the per-account statuses are kept apart: a Region enabled for one account
/// stays `DISABLED` for the other.
#[tokio::test]
async fn each_account_reports_its_own_opt_in_statuses() {
    let (router, engine) = emulator();
    let member = "123456789012";
    let own = default_account_id().to_string();

    record_region_status(&engine, member, "af-south-1", "ENABLED");

    let member_regions = list_regions(&router, json!({"AccountId": member})).await;
    assert_eq!(
        member_regions.status, 200,
        "case \"a member account\": expected 200, got {} with {:?}",
        member_regions.status, member_regions.body
    );
    let member_regions = member_regions.regions();
    let own_regions = list_regions(&router, json!({"AccountId": own})).await.regions();

    assert_eq!(
        status_of(&member_regions, "af-south-1"),
        "ENABLED",
        "case \"a Region enabled for the member account\": expected the recorded status"
    );
    assert_eq!(
        status_of(&own_regions, "af-south-1"),
        "DISABLED",
        "case \"the same Region for another account\": expected DISABLED, the catalog \
         default, got {:?}",
        status_of(&own_regions, "af-south-1")
    );
    assert_eq!(
        member_regions
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
        own_regions
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
        "case \"two accounts\": the Region catalog is the same for both"
    );
}

/// A recorded status is what the Region reports — for every `RegionOptStatus` an
/// opt-in call can leave behind — while a Region AWS turns on by default keeps
/// `ENABLED_BY_DEFAULT`, which no opt-in call can change.
#[tokio::test]
async fn a_recorded_status_is_the_status_reported() {
    // (case, the Region, the recorded status, the status the Region must report)
    let cases: [(&str, &str, &str, &str); 7] = [
        (
            "an opt-in Region enabled",
            "af-south-1",
            "ENABLED",
            "ENABLED",
        ),
        (
            "an opt-in Region still enabling",
            "ap-east-1",
            "ENABLING",
            "ENABLING",
        ),
        (
            "an opt-in Region being disabled",
            "eu-central-2",
            "DISABLING",
            "DISABLING",
        ),
        (
            "an opt-in Region disabled again",
            "il-central-1",
            "DISABLED",
            "DISABLED",
        ),
        (
            "another opt-in Region enabled",
            "mx-central-1",
            "ENABLED",
            "ENABLED",
        ),
        (
            "a default-on Region nothing can disable",
            "us-east-1",
            "DISABLED",
            "ENABLED_BY_DEFAULT",
        ),
        (
            "another default-on Region",
            "ap-northeast-3",
            "ENABLING",
            "ENABLED_BY_DEFAULT",
        ),
    ];

    let (router, engine) = emulator();
    let account = default_account_id().to_string();
    for (case, region, recorded, expected) in cases {
        record_region_status(&engine, &account, region, recorded);
        let regions = whole_list(&router).await;
        let actual = status_of(&regions, region);
        assert_eq!(
            actual, expected,
            "case {case:?}: {region} with {recorded:?} recorded must report \
             {expected:?}, got {actual:?}"
        );
    }
}

/// `RegionOptStatusContains` keeps exactly the Regions whose reported status is one
/// of the listed ones — for one status, for several at once, and for a status no
/// Region carries.
#[tokio::test]
async fn the_filter_keeps_exactly_the_listed_statuses() {
    let (router, engine) = emulator();
    let account = default_account_id().to_string();
    // Every `RegionOptStatus` is represented, so no row's expectation is empty by
    // accident: the two transient ones only exist because an opt-in call recorded them.
    record_region_status(&engine, &account, "af-south-1", "ENABLED");
    record_region_status(&engine, &account, "ap-east-1", "ENABLING");
    record_region_status(&engine, &account, "eu-central-2", "DISABLING");

    let everything = whole_list(&router).await;

    // (case, the statuses the request lists)
    let mut cases: Vec<(String, Vec<&str>)> = REGION_OPT_STATUSES
        .iter()
        .map(|status| (format!("only {status}"), vec![*status]))
        .collect();
    cases.push((
        "the two transient statuses".to_string(),
        vec!["ENABLING", "DISABLING"],
    ));
    cases.push((
        "every enabled form".to_string(),
        vec!["ENABLED", "ENABLED_BY_DEFAULT"],
    ));
    cases.push((
        "all five statuses".to_string(),
        REGION_OPT_STATUSES.to_vec(),
    ));
    cases.push((
        "a status listed twice".to_string(),
        vec!["DISABLED", "DISABLED"],
    ));
    cases.push(("no status at all".to_string(), Vec::new()));

    for (case, statuses) in cases {
        let answer = list_regions(
            &router,
            json!({"RegionOptStatusContains": statuses.clone()}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let expected: Vec<(String, String)> = everything
            .iter()
            .filter(|(_, status)| statuses.contains(&status.as_str()))
            .cloned()
            .collect();
        assert_eq!(
            answer.regions(),
            expected,
            "case {case:?}: expected the Regions whose status is one of \
             {statuses:?}, got {:?}",
            answer.regions()
        );
        assert_eq!(
            answer.next_token(),
            None,
            "case {case:?}: a filtered list that fits one page carries no NextToken, \
             got {:?}",
            answer.next_token()
        );
    }
}

/// `MaxResults` cuts the kept Regions into pages: each page holds at most that
/// many, `NextToken` is present exactly while more follow, and the pages
/// concatenated are the whole list in the same order — at each published bound of
/// `MaxResults` and at sizes in between, with and without a filter.
#[tokio::test]
async fn paging_reassembles_the_whole_filtered_list() {
    let (router, engine) = emulator();
    let account = default_account_id().to_string();
    record_region_status(&engine, &account, "af-south-1", "ENABLED");
    record_region_status(&engine, &account, "ap-east-1", "ENABLING");

    let everything = whole_list(&router).await;

    // (case, MaxResults, the filter the request carries)
    let cases: Vec<(String, i64, Option<Vec<&str>>)> = vec![
        ("the smallest page".to_string(), MAX_RESULTS_MIN, None),
        ("two at a time".to_string(), 2, None),
        ("three at a time".to_string(), 3, None),
        ("seven at a time".to_string(), 7, None),
        ("the largest page".to_string(), MAX_RESULTS_MAX, None),
        (
            "one at a time, filtered to the default-on Regions".to_string(),
            MAX_RESULTS_MIN,
            Some(vec!["ENABLED_BY_DEFAULT"]),
        ),
        (
            "four at a time, filtered to the disabled Regions".to_string(),
            4,
            Some(vec!["DISABLED"]),
        ),
        (
            "two at a time, filtered to a status one Region carries".to_string(),
            2,
            Some(vec!["ENABLED"]),
        ),
    ];

    for (case, page_size, filter) in cases {
        let expected: Vec<(String, String)> = match &filter {
            None => everything.clone(),
            Some(statuses) => everything
                .iter()
                .filter(|(_, status)| statuses.contains(&status.as_str()))
                .cloned()
                .collect(),
        };

        let mut collected: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(statuses) = &filter {
                request["RegionOptStatusContains"] = json!(statuses);
            }
            if let Some(value) = &token {
                request["NextToken"] = json!(value);
            }
            let answer = list_regions(&router, request).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: page {pages} expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page = answer.regions();
            assert!(
                page.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case {case:?}: page {pages} must hold at most {page_size} Regions, \
                 got {:?}",
                page
            );
            collected.extend(page.clone());
            token = answer.next_token();
            assert!(
                collected.len() <= expected.len(),
                "case {case:?}: paging reported {} Regions, more than the {} kept ones",
                collected.len(),
                expected.len()
            );
            let remaining = expected.len() - collected.len();
            assert_eq!(
                token.is_some(),
                remaining > 0,
                "case {case:?}: page {pages} carries a NextToken iff Regions remain \
                 ({remaining} left), got {:?}",
                token
            );
            pages += 1;
            if token.is_none() {
                break;
            }
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging must terminate, got {pages} pages for \
                 {} Regions",
                expected.len()
            );
        }

        assert_eq!(
            collected, expected,
            "case {case:?}: the pages concatenated must be the whole kept list"
        );
    }
}

/// A `MaxResults` outside its modelled range is refused, and each published bound
/// is accepted — the boundary of the class, not a value inside it.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let (router, _engine) = emulator();

    // (case, MaxResults, whether the request is admitted)
    let cases: [(&str, i64, bool); 6] = [
        ("one below the minimum", MAX_RESULTS_MIN - 1, false),
        ("the minimum", MAX_RESULTS_MIN, true),
        ("inside the range", 7, true),
        ("the maximum", MAX_RESULTS_MAX, true),
        ("one past the maximum", MAX_RESULTS_MAX + 1, false),
        ("far past the maximum", 1_000, false),
    ];

    for (case, max_results, admitted) in cases {
        let answer = list_regions(&router, json!({"MaxResults": max_results})).await;
        if admitted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: MaxResults={max_results} is inside the modelled range, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                !answer.regions().is_empty(),
                "case {case:?}: an admitted page is not empty, got {:?}",
                answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: MaxResults={max_results} is outside the modelled range, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
        }
    }
}

/// A `NextToken` this operation never issued is refused rather than resuming
/// somewhere, while the token it did issue resumes the page after the one it was
/// returned with.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let (router, _engine) = emulator();

    // Tokens no `ListRegions` response could have carried: not a token's encoding
    // at all, an encoding of something that is not a Region, and the empty string.
    let nowhere = ::hex::encode("zz-nowhere-1");
    let cases: [(&str, &str); 5] = [
        ("a word", "next-page"),
        ("a Region code in the clear", "us-east-1"),
        ("an encoding of a Region nobody serves", nowhere.as_str()),
        ("odd-length hex", "abc"),
        ("the empty token", ""),
    ];

    for (case, token) in cases {
        let answer = list_regions(&router, json!({"NextToken": token})).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a foreign NextToken is refused, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }

    // And the token the operation DID issue resumes where the page stopped.
    let first = list_regions(&router, json!({"MaxResults": 1})).await;
    let token = first
        .next_token()
        .expect("a one-Region page of the catalog is truncated");
    let resumed = list_regions(&router, json!({"MaxResults": 1, "NextToken": token})).await;
    assert_eq!(
        resumed.status, 200,
        "case \"the issued token\": expected 200, got {} with {:?}",
        resumed.status, resumed.body
    );
    let everything = whole_list(&router).await;
    assert_eq!(
        resumed.regions(),
        everything[1..2].to_vec(),
        "case \"the issued token\": expected the Region after the first, got {:?}",
        resumed.regions()
    );
}
