// SPDX-License-Identifier: CC0-1.0

//! How `ListSigningProfiles` paginates, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! `@httpQuery` bindings, deserialization, the handler, the persisted profiles and
//! serialization — the same path a client takes. The rule under test:
//!
//! * a pagination token is an opaque value the service ISSUES for one listing —
//!   the region whose endpoint answered and the filters that call carried — so a
//!   page resumes only a call that enumerates that same listing, and a `nextToken`
//!   the service never handed out for it is refused 400 `ValidationException`
//!   (`code` naming the invalid-token condition) instead of being served as a page
//!   out of some other ordering;
//! * walking a listing with the tokens it hands out therefore reports every
//!   profile of that listing exactly once, in one stable order, and the walk
//!   changes nothing a later caller sees.
//!
//! The cases are tables. The refusal rows differ along what the rule generalises
//! over: the FORM of the rejected value (a name-shaped string, a real profile
//! name, a hex payload the service never minted, a truncated token, a corrupted
//! token) and the LISTING the token is replayed against (another platform filter,
//! another status set, another region). The acceptance rows differ along the
//! filter set, the page size, and how the caller spelled a filter set that
//! enumerates the same profiles.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite lists through unless a row names another one.
const REGION: &str = "us-west-2";

/// A second region, so a token can be replayed against another region's endpoint.
const OTHER_REGION: &str = "eu-west-1";

/// The error `code` a `nextToken` this service never handed out carries, spelled
/// out here rather than imported so a wrong code in the handler fails this suite.
const ERROR_CODE_INVALID_NEXT_TOKEN: &str = "InvalidNextToken";

/// The modelled error shape a refused token is answered with (HTTP 400).
const ERROR_SHAPE_VALIDATION: &str = "ValidationException";

/// A platform from this service's catalog whose profiles need no
/// `signingMaterial`, so a seeded profile is one `PutSigningProfile` call.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// A second such platform, so a listing can be filtered to a subset.
const OTHER_PLATFORM_ID: &str = "AWSIoTDeviceManagement-SHA256-ECDSA";

/// `MaxResults`'s modelled maximum (`@range(min: 1, max: 25)`).
const MAX_RESULTS_CEILING: i32 = 25;

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
    /// The modelled `code` member, or the empty string when the body carries none.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }

    /// The profile names this answer reported, in the order it reported them.
    fn listed_names(&self) -> Vec<String> {
        self.body["profiles"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "ListSigningProfiles must report a profiles list: {:?}",
                    self.body
                )
            })
            .iter()
            .map(|profile| {
                profile["profileName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The `nextToken` this answer carries, or `None` on a final page.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }
}

async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/signer/aws4_request, \
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

/// Create an ACTIVE profile in `region` on `platform_id`, answering its version.
async fn put_profile(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    platform_id: &str,
) -> String {
    let answer = call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": platform_id })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding {name} in {region}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.str_member("profileVersion")
}

async fn cancel_profile(router: &MultiServiceRouter, region: &str, name: &str) {
    let answer = call(
        router,
        region,
        "DELETE",
        &format!("/signing-profiles/{name}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "canceling {name}: expected 204, got {} {:?}",
        answer.status, answer.body
    );
}

async fn revoke_profile(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    profile_version: &str,
) {
    let answer = call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(json!({
            "profileVersion": profile_version,
            "reason": "rotate-signing-key",
            "effectiveTime": 1_700_000_000,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "revoking {name}: expected 204, got {} {:?}",
        answer.status, answer.body
    );
}

/// `ListSigningProfiles` with the raw query string `query`, through `region`.
async fn list(router: &MultiServiceRouter, region: &str, query: &str) -> Answer {
    call(router, region, "GET", &format!("/signing-profiles{query}"), None).await
}

/// One seeded profile: its name, the platform it is on, and the status it is
/// moved to after creation.
struct Seed {
    name: &'static str,
    platform_id: &'static str,
    status: &'static str,
}

/// `SigningProfileStatus`'s values, as the model spells them.
const STATUS_ACTIVE: &str = "Active";
const STATUS_CANCELED: &str = "Canceled";
const STATUS_REVOKED: &str = "Revoked";

/// The profiles every case in this suite lists over: enough to truncate a page at
/// every page size under test, spread across two platforms and all three
/// statuses, and deliberately NOT seeded in name order so the order under test is
/// the service's and not the insertion sequence.
const SEEDS: &[Seed] = &[
    Seed {
        name: "Profile_C",
        platform_id: PLATFORM_ID,
        status: STATUS_ACTIVE,
    },
    Seed {
        name: "Profile_A",
        platform_id: PLATFORM_ID,
        status: STATUS_ACTIVE,
    },
    Seed {
        name: "Profile_E",
        platform_id: OTHER_PLATFORM_ID,
        status: STATUS_ACTIVE,
    },
    Seed {
        name: "Profile_B",
        platform_id: OTHER_PLATFORM_ID,
        status: STATUS_ACTIVE,
    },
    Seed {
        name: "Profile_D",
        platform_id: PLATFORM_ID,
        status: STATUS_CANCELED,
    },
    Seed {
        name: "Profile_F",
        platform_id: OTHER_PLATFORM_ID,
        status: STATUS_REVOKED,
    },
];

/// Seed [`SEEDS`] into `region`.
async fn seed(router: &MultiServiceRouter, region: &str) {
    for profile in SEEDS {
        let version = put_profile(router, region, profile.name, profile.platform_id).await;
        match profile.status {
            STATUS_ACTIVE => {}
            STATUS_CANCELED => cancel_profile(router, region, profile.name).await,
            STATUS_REVOKED => revoke_profile(router, region, profile.name, &version).await,
            other => panic!("the suite seeds no profile in status {other}"),
        }
    }
}

/// The names [`SEEDS`] expects a listing of `platform_id` (none for every
/// platform) and `statuses` to report, in the order the service reports them:
/// name order within one region.
fn expected_names(platform_id: Option<&str>, statuses: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = SEEDS
        .iter()
        .filter(|profile| platform_id.is_none_or(|wanted| wanted == profile.platform_id))
        .filter(|profile| statuses.contains(&profile.status))
        .map(|profile| profile.name.to_string())
        .collect();
    names.sort();
    names
}

/// Walk a listing with the tokens it hands out, asserting every page is a 200 of
/// at most `page_size` profiles and that the token is opaque, and answering every
/// name reported, in order.
async fn walk(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    filters: &str,
    page_size: i32,
) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..=SEEDS.len() {
        let query = match token.as_deref() {
            Some(token) => format!("?maxResults={page_size}{filters}&nextToken={token}"),
            None => format!("?maxResults={page_size}{filters}"),
        };
        let page = list(router, region, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: page {query} expected 200, got {} {:?}",
            page.status, page.body
        );
        let reported = page.listed_names();
        assert!(
            reported.len() <= page_size as usize,
            "case {case:?}: page {query} expected at most {page_size} profiles, got {reported:?}"
        );
        names.extend(reported);
        match page.next_token() {
            Some(next) => {
                assert_opaque(case, &next);
                token = Some(next);
            }
            None => return names,
        }
    }
    panic!("case {case:?}: the walk did not end within {} pages", SEEDS.len() + 1);
}

/// Assert `token` is an opaque server-issued value and not the resume key itself:
/// it carries only characters an `@httpQuery` value travels in unescaped, and it
/// is neither a profile name this suite seeded nor a value carrying one.
fn assert_opaque(case: &str, token: &str) {
    assert!(
        token.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "case {case:?}: expected an opaque token of hex digits, got {token:?}"
    );
    for profile in SEEDS {
        assert!(
            !token.contains(profile.name),
            "case {case:?}: expected a token that does not carry the profile name \
             {:?} in the clear, got {token:?}",
            profile.name
        );
    }
}

/// A token the service never handed out for the listing it is replayed against.
struct RefusalCase {
    case: &'static str,
    /// The rejected `nextToken` value, built from the real first-page token of
    /// the default listing (`issued`) where the row needs one.
    token: String,
    /// The filters the token is replayed under, appended to the query.
    filters: &'static str,
    /// The region whose endpoint the token is replayed against.
    region: &'static str,
}

fn refusal_cases(issued: &str) -> Vec<RefusalCase> {
    // A token's payload is hex, so flipping one digit of a real token yields a
    // value of the right form that decodes to something the service never issued.
    let mut corrupted: Vec<char> = issued.chars().collect();
    let last = corrupted.len() - 1;
    corrupted[last] = if corrupted[last] == '0' { '1' } else { '0' };
    let corrupted: String = corrupted.into_iter().collect();

    vec![
        RefusalCase {
            case: "a profile-name-shaped string no page ever returned",
            token: "zzzz".to_string(),
            filters: "",
            region: REGION,
        },
        RefusalCase {
            case: "the name of a profile the account holds",
            token: "Profile_B".to_string(),
            filters: "",
            region: REGION,
        },
        RefusalCase {
            case: "the name of the first profile of the listing",
            token: "Profile_A".to_string(),
            filters: "",
            region: REGION,
        },
        RefusalCase {
            case: "a profile name hex-encoded by the client",
            token: ::hex::encode("Profile_B"),
            filters: "",
            region: REGION,
        },
        RefusalCase {
            case: "an issued token truncated in transit",
            token: issued[..issued.len() - 4].to_string(),
            filters: "",
            region: REGION,
        },
        RefusalCase {
            case: "an issued token with one digit corrupted",
            token: corrupted,
            filters: "",
            region: REGION,
        },
        RefusalCase {
            case: "an issued token with a trailing digit appended",
            token: format!("{issued}a"),
            filters: "",
            region: REGION,
        },
        RefusalCase {
            case: "an issued token replayed under a platformId filter",
            token: issued.to_string(),
            filters: "&platformId=AWSLambda-SHA384-ECDSA",
            region: REGION,
        },
        // `platformId` is a request value this service constrains to no
        // alphabet, so these two rows probe the field boundary of the cursor
        // with values shaped like its own rendering and with the separator
        // itself: neither may be read as the listing the token was issued for.
        RefusalCase {
            case: "an issued token replayed under a platformId shaped like a rendered field",
            token: issued.to_string(),
            filters: "&platformId=9:us-west-2",
            region: REGION,
        },
        RefusalCase {
            case: "an issued token replayed under a platformId carrying the field separator",
            token: issued.to_string(),
            filters: "&platformId=x%1Fy",
            region: REGION,
        },
        RefusalCase {
            case: "an issued token replayed with includeCanceled=true",
            token: issued.to_string(),
            filters: "&includeCanceled=true",
            region: REGION,
        },
        RefusalCase {
            case: "an issued token replayed under a statuses filter",
            token: issued.to_string(),
            filters: "&statuses=Canceled",
            region: REGION,
        },
        RefusalCase {
            case: "an issued token replayed against another region's endpoint",
            token: issued.to_string(),
            filters: "",
            region: OTHER_REGION,
        },
    ]
}

/// A `nextToken` this service never issued for the listing it is carried into is
/// refused 400 with the modelled `code`, rather than served as a page.
#[tokio::test]
async fn a_next_token_this_listing_never_handed_out_is_refused() {
    let router = emulator();
    seed(&router, REGION).await;
    // The other region holds the same names, so a cross-region replay is refused
    // because the token was issued elsewhere — not because nothing is there.
    seed(&router, OTHER_REGION).await;

    let first = list(&router, REGION, "?maxResults=2").await;
    assert_eq!(
        first.status, 200,
        "the first page must be served: {:?}",
        first.body
    );
    let issued = first
        .next_token()
        .unwrap_or_else(|| panic!("a truncated page must carry a nextToken: {:?}", first.body));
    assert_opaque("the issued first-page token", &issued);

    for row in refusal_cases(&issued) {
        let case = row.case;
        let query = format!("?maxResults=2{}&nextToken={}", row.filters, row.token);
        let answer = list(&router, row.region, &query).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {query}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.shape(),
            ERROR_SHAPE_VALIDATION,
            "case {case:?}: expected the {ERROR_SHAPE_VALIDATION} shape, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.code(),
            ERROR_CODE_INVALID_NEXT_TOKEN,
            "case {case:?}: expected the modelled code {ERROR_CODE_INVALID_NEXT_TOKEN}, got {:?}",
            answer.body
        );
        assert!(
            !answer.message().is_empty(),
            "case {case:?}: expected a message beside the code, got {:?}",
            answer.body
        );
        assert!(
            answer.body["profiles"].is_null(),
            "case {case:?}: a refusal must report no profiles, got {:?}",
            answer.body
        );
    }
}

/// One listing to walk: the filters a caller sends, the platform and statuses
/// those filters select, and the page size the walk uses.
struct WalkCase {
    case: &'static str,
    filters: &'static str,
    platform_id: Option<&'static str>,
    statuses: &'static [&'static str],
    page_size: i32,
}

/// The rows differ along the filter dimension (none, platform, statuses,
/// `includeCanceled`) and along the page size, including both published bounds of
/// `MaxResults` and a size that divides the result set exactly.
fn walk_cases() -> Vec<WalkCase> {
    vec![
        WalkCase {
            case: "the default listing, one profile per page",
            filters: "",
            platform_id: None,
            statuses: &[STATUS_ACTIVE],
            page_size: 1,
        },
        WalkCase {
            case: "the default listing, two profiles per page",
            filters: "",
            platform_id: None,
            statuses: &[STATUS_ACTIVE],
            page_size: 2,
        },
        WalkCase {
            case: "the default listing, the largest page the model allows",
            filters: "",
            platform_id: None,
            statuses: &[STATUS_ACTIVE],
            page_size: MAX_RESULTS_CEILING,
        },
        WalkCase {
            case: "includeCanceled=false, which is the modelled default",
            filters: "&includeCanceled=false",
            platform_id: None,
            statuses: &[STATUS_ACTIVE],
            page_size: 2,
        },
        WalkCase {
            case: "includeCanceled=true",
            filters: "&includeCanceled=true",
            platform_id: None,
            statuses: &[STATUS_ACTIVE, STATUS_CANCELED],
            page_size: 1,
        },
        WalkCase {
            case: "a platform filter",
            filters: "&platformId=AWSLambda-SHA384-ECDSA",
            platform_id: Some(PLATFORM_ID),
            statuses: &[STATUS_ACTIVE],
            page_size: 1,
        },
        WalkCase {
            case: "the other platform's filter",
            filters: "&platformId=AWSIoTDeviceManagement-SHA256-ECDSA",
            platform_id: Some(OTHER_PLATFORM_ID),
            statuses: &[STATUS_ACTIVE],
            page_size: 2,
        },
        WalkCase {
            case: "a statuses filter naming one status",
            filters: "&statuses=Revoked",
            platform_id: None,
            statuses: &[STATUS_REVOKED],
            page_size: 1,
        },
        WalkCase {
            case: "a statuses filter naming two",
            filters: "&statuses=Canceled&statuses=Revoked",
            platform_id: None,
            statuses: &[STATUS_CANCELED, STATUS_REVOKED],
            page_size: 1,
        },
        WalkCase {
            case: "a platform and a statuses filter together",
            filters: "&platformId=AWSLambda-SHA384-ECDSA&statuses=Active&statuses=Canceled",
            platform_id: Some(PLATFORM_ID),
            statuses: &[STATUS_ACTIVE, STATUS_CANCELED],
            page_size: 2,
        },
    ]
}

/// Walking a listing with the tokens it issues reports every profile of that
/// listing exactly once, in one stable order, whatever the page size — and the
/// walk leaves what a later caller sees untouched.
#[tokio::test]
async fn a_listing_walked_with_its_own_tokens_reports_every_profile_once() {
    let router = emulator();
    seed(&router, REGION).await;

    for row in walk_cases() {
        let expected = expected_names(row.platform_id, row.statuses);
        let walked = walk(&router, REGION, row.case, row.filters, row.page_size).await;
        assert_eq!(
            walked, expected,
            "case {:?}: expected {expected:?} once each in name order, got {walked:?}",
            row.case
        );
        let unique: BTreeSet<&String> = walked.iter().collect();
        assert_eq!(
            unique.len(),
            walked.len(),
            "case {:?}: expected no profile twice, got {walked:?}",
            row.case
        );
        // A read reports, so walking the listing changes nothing: the same walk
        // answers the same thing.
        let again = walk(&router, REGION, row.case, row.filters, row.page_size).await;
        assert_eq!(
            again, walked,
            "case {:?}: expected the walk to be repeatable, got {again:?} after {walked:?}",
            row.case
        );
    }
}

/// Two spellings of the same listing, and whether they enumerate the same
/// profiles in the same order — which is what decides whether a token issued for
/// one resumes the other.
struct SpellingCase {
    case: &'static str,
    issued_under: &'static str,
    replayed_under: &'static str,
    /// True when both spellings enumerate the same listing, so the token resumes.
    same_listing: bool,
}

/// A token is bound to WHAT the listing enumerates, not to the characters of the
/// query it was issued from: two filter sets that select the same profiles in the
/// same order share tokens, and two that do not never do.
fn spelling_cases() -> Vec<SpellingCase> {
    vec![
        SpellingCase {
            case: "statuses=Active against the default, which also admits only Active",
            issued_under: "&statuses=Active",
            replayed_under: "",
            same_listing: true,
        },
        SpellingCase {
            case: "the default against statuses=Active",
            issued_under: "",
            replayed_under: "&statuses=Active",
            same_listing: true,
        },
        SpellingCase {
            case: "the two orderings of the same two statuses",
            issued_under: "&statuses=Canceled&statuses=Active",
            replayed_under: "&statuses=Active&statuses=Canceled",
            same_listing: true,
        },
        SpellingCase {
            case: "includeCanceled=true against the statuses pair it resolves to",
            issued_under: "&includeCanceled=true",
            replayed_under: "&statuses=Active&statuses=Canceled",
            same_listing: true,
        },
        SpellingCase {
            case: "the same statuses listed twice over",
            issued_under: "&statuses=Active&statuses=Active",
            replayed_under: "&statuses=Active",
            same_listing: true,
        },
        SpellingCase {
            case: "one status against a strictly larger set",
            issued_under: "&statuses=Active",
            replayed_under: "&statuses=Active&statuses=Canceled",
            same_listing: false,
        },
        SpellingCase {
            case: "the default against includeCanceled=true",
            issued_under: "",
            replayed_under: "&includeCanceled=true",
            same_listing: false,
        },
        SpellingCase {
            case: "no platform filter against one",
            issued_under: "",
            replayed_under: "&platformId=AWSLambda-SHA384-ECDSA",
            same_listing: false,
        },
        SpellingCase {
            case: "one platform filter against another",
            issued_under: "&platformId=AWSLambda-SHA384-ECDSA",
            replayed_under: "&platformId=AWSIoTDeviceManagement-SHA256-ECDSA",
            same_listing: false,
        },
    ]
}

#[tokio::test]
async fn a_token_resumes_every_spelling_of_the_listing_it_was_issued_for() {
    let router = emulator();
    seed(&router, REGION).await;

    for row in spelling_cases() {
        let case = row.case;
        let first = list(&router, REGION, &format!("?maxResults=1{}", row.issued_under)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first page must be served, got {} {:?}",
            first.status, first.body
        );
        let token = first.next_token().unwrap_or_else(|| {
            panic!("case {case:?}: a truncated page must carry a nextToken: {:?}", first.body)
        });

        let resumed = list(
            &router,
            REGION,
            &format!("?maxResults=1{}&nextToken={token}", row.replayed_under),
        )
        .await;
        if row.same_listing {
            // The token resumes the same enumeration, so the page it serves is
            // the one that spelling's own token would have served.
            let own = list(
                &router,
                REGION,
                &format!("?maxResults=1{}", row.replayed_under),
            )
            .await;
            let own_token = own.next_token().unwrap_or_else(|| {
                panic!("case {case:?}: a truncated page must carry a nextToken: {:?}", own.body)
            });
            let expected = list(
                &router,
                REGION,
                &format!("?maxResults=1{}&nextToken={own_token}", row.replayed_under),
            )
            .await;
            assert_eq!(
                resumed.status, 200,
                "case {case:?}: expected the token to resume the same listing, got {} {:?}",
                resumed.status, resumed.body
            );
            assert_eq!(
                resumed.listed_names(),
                expected.listed_names(),
                "case {case:?}: expected the page {:?}, got {:?}",
                expected.listed_names(),
                resumed.listed_names()
            );
        } else {
            assert_eq!(
                resumed.status, 400,
                "case {case:?}: expected HTTP 400 for a token from another listing, got {} {:?}",
                resumed.status, resumed.body
            );
            assert_eq!(
                resumed.code(),
                ERROR_CODE_INVALID_NEXT_TOKEN,
                "case {case:?}: expected the modelled code {ERROR_CODE_INVALID_NEXT_TOKEN}, got {:?}",
                resumed.body
            );
        }
    }
}

/// A final page carries no token, so a client stops; and a listing that holds
/// nothing is a final page too.
#[tokio::test]
async fn a_final_page_carries_no_token() {
    let router = emulator();
    seed(&router, REGION).await;

    let cases: [(&str, &str, usize); 3] = [
        (
            "every active profile in one page",
            "?maxResults=25",
            expected_names(None, &[STATUS_ACTIVE]).len(),
        ),
        (
            "a listing holding exactly one profile",
            "?maxResults=25&statuses=Revoked",
            1,
        ),
        (
            "a listing holding nothing at all",
            "?maxResults=25&platformId=Notation-OCI-SHA384-ECDSA",
            0,
        ),
    ];

    for (case, query, expected_len) in cases {
        let answer = list(&router, REGION, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.listed_names().len(),
            expected_len,
            "case {case:?}: expected {expected_len} profiles, got {:?}",
            answer.listed_names()
        );
        assert_eq!(
            answer.next_token(),
            None,
            "case {case:?}: expected no nextToken on a final page, got {:?}",
            answer.next_token()
        );
    }
}
