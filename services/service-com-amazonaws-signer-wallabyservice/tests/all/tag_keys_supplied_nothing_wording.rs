// SPDX-License-Identifier: CC0-1.0

//! What this service answers a request that supplies NOTHING for an input member the
//! model binds a `@required` LIST to on the query string, end to end over HTTP.
//!
//! The rule under test: for a member a model binds a `@required` LIST to the query
//! string, a request that omits the parameter and a request that carries an empty list
//! are the SAME request on the wire — an SDK serializing an empty list emits no
//! parameter at all — so such a request is a request for the operation its method and
//! path address: it is refused with that operation's modelled 400 stating that the
//! member must carry something (`<member> must not be null or empty`), never answered as
//! an operation nobody implemented (501), and it changes nothing. A request that does
//! supply entries is served by the operation, which applies them.
//!
//! The rows span what the rule generalises over: both members this model binds a list to
//! the query string — `UntagResource`'s `tagKeys` on `DELETE /tags/{resourceArn}` and
//! `GetRevocationStatus`'s `certificateHashes` on `GET /revocations`
//! (`smithy_query(query="operation",operation="UntagResource,GetRevocationStatus",\
//! model_path="services/service-com-amazonaws-signer-wallabyservice/smithy-model/\
//! com-amazonaws-signer-wallabyservice.json")`) — how the parameter comes to be
//! unsupplied (no query string at all, another parameter in its place, its name
//! misspelled, its name differing only in case), what the path labels carry (a stored
//! profile's ARN, a well-formed ARN naming no profile, a label that is no ARN at all),
//! and the status of the resource they name. The counterpart rows supply the parameter
//! with one key, several keys, a key the resource does not hold and a key carrying the
//! punctuation `TagKey`'s alphabet admits, and one table marks where the rule stops: a
//! parameter that is PRESENT carries an entry even when that entry is empty, which is a
//! list with an entry and is refused for the entry.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_signer_wallabyservice::state::SIGNING_PLATFORM_CATALOG;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// The wire error shape the framework's boundary refusal renders, which this service
/// leaves where it is when it rewords such a refusal's message.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The sentence this service states for a query-bound list member a request supplied
/// nothing for, with the member's own name in it.
fn supplied_nothing(member: &str) -> String {
    format!("{member} must not be null or empty")
}

/// A published `platformId` whose profiles need no `signingMaterial`, taken from the
/// catalog this service serves rather than written out, so one `PutSigningProfile` call
/// seeds a profile.
fn platform_needing_no_signing_material() -> &'static str {
    SIGNING_PLATFORM_CATALOG
        .iter()
        .find(|platform| !platform.requires_signing_material)
        .expect("this service publishes a platform that needs no signing material")
        .platform_id
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status and the body as JSON.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// `value` percent-encoded for ONE URI segment or query value: `resourceArn` is a
/// non-greedy `@httpLabel` carrying `:` and `/`, and `TagKey`'s alphabet admits `/`, `:`
/// and `=`, so an SDK encodes everything outside the unreserved set. Spelled out here
/// the way this crate's other suites spell it, so a suite takes no dependency of its own.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", SIGNED_FOR_US_WEST_2);
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

/// A profile of `name`, and the ARN this service holds for it.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> String {
    let platform = platform_needing_no_signing_material();
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": platform })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding profile {name}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("PutSigningProfile must report an arn: {:?}", answer.body))
        .to_string()
}

/// The tags `tags` names, written onto the resource `arn` names.
async fn seed_tags(router: &MultiServiceRouter, arn: &str, tags: &BTreeMap<String, String>) {
    let map: Map<String, Value> = tags
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    let answer = call(
        router,
        "POST",
        &format!("/tags/{}", encode(arn)),
        Some(json!({ "tags": Value::Object(map) })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding tags on {arn}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
}

/// The tags the resource `arn` names holds now, read back through
/// `ListTagsForResource`.
async fn tags_of(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let answer = call(router, "GET", &format!("/tags/{}", encode(arn)), None).await;
    assert_eq!(
        answer.status, 200,
        "reading the tags of {arn}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["tags"]
        .as_object()
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /revocations` carrying every required parameter except `certificateHashes`:
/// the values name nothing stored, which that operation reports rather than refuses, so
/// a row about the member that supplied nothing is not answered for another reason.
fn revocations_without_certificate_hashes() -> String {
    let platform = platform_needing_no_signing_material();
    let parameters = [
        ("signatureTimestamp", "2023-11-14T22:13:20Z".to_string()),
        ("platformId", platform.to_string()),
        (
            "profileVersionArn",
            format!(
                "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/Nothing_Supplied/abcde12345"
            ),
        ),
        (
            "jobArn",
            format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/nothing-supplied-job"),
        ),
    ];
    let query = parameters
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("/revocations?{query}")
}

/// One request that supplies nothing for a query-bound list member: what to send, and
/// the member the refusal has to name.
struct Unsupplied {
    case: &'static str,
    method: &'static str,
    uri: String,
    member: &'static str,
}

/// A request that supplies nothing for a required query-bound LIST member is refused
/// with the modelled 400 stating that the member must carry something — on both members
/// this model binds, whatever else the request carries — and the refused call leaves the
/// resource it addressed exactly as it was.
#[tokio::test]
async fn a_query_bound_list_member_supplied_with_nothing_is_refused_by_name() {
    let router = emulator();

    let profile = "Tag_Keys_Nothing";
    let arn = seed_profile(&router, profile).await;
    let seeded: BTreeMap<String, String> = [
        ("team".to_string(), "signing".to_string()),
        ("env".to_string(), "prod".to_string()),
    ]
    .into_iter()
    .collect();
    seed_tags(&router, &arn, &seeded).await;

    // A second profile, canceled: the status its own handler gates tag writes on is
    // never reached, because the member that supplied nothing is refused at the
    // boundary.
    let canceled = "Tag_Keys_Canceled";
    let canceled_arn = seed_profile(&router, canceled).await;
    let cancel = call(
        &router,
        "DELETE",
        &format!("/signing-profiles/{canceled}"),
        None,
    )
    .await;
    assert_eq!(
        cancel.status, 204,
        "canceling {canceled}: expected 204, got {} {:?}",
        cancel.status, cancel.body
    );

    let rows: Vec<Unsupplied> = vec![
        Unsupplied {
            case: "the tag removal with no query string at all",
            method: "DELETE",
            uri: format!("/tags/{}", encode(&arn)),
            member: "tagKeys",
        },
        Unsupplied {
            case: "the tag removal carrying another operation's parameter instead",
            method: "DELETE",
            uri: format!("/tags/{}?nextToken=whatever", encode(&arn)),
            member: "tagKeys",
        },
        Unsupplied {
            case: "the tag removal with the parameter name misspelled",
            method: "DELETE",
            uri: format!("/tags/{}?tag_keys={}", encode(&arn), encode("team")),
            member: "tagKeys",
        },
        Unsupplied {
            case: "the tag removal with the parameter name differing only in case",
            method: "DELETE",
            uri: format!("/tags/{}?tagkeys={}", encode(&arn), encode("team")),
            member: "tagKeys",
        },
        Unsupplied {
            case: "the tag removal naming a well-formed ARN no profile is stored under",
            method: "DELETE",
            uri: format!(
                "/tags/{}",
                encode(&format!(
                    "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/No_Such_Profile"
                ))
            ),
            member: "tagKeys",
        },
        Unsupplied {
            case: "the tag removal naming a label that is no ARN at all",
            method: "DELETE",
            uri: "/tags/not-an-arn".to_string(),
            member: "tagKeys",
        },
        Unsupplied {
            case: "the tag removal naming a canceled profile",
            method: "DELETE",
            uri: format!("/tags/{}", encode(&canceled_arn)),
            member: "tagKeys",
        },
        Unsupplied {
            case: "the revocation status with every other required parameter supplied",
            method: "GET",
            uri: revocations_without_certificate_hashes(),
            member: "certificateHashes",
        },
    ];

    for Unsupplied {
        case,
        method,
        uri,
        member,
    } in &rows
    {
        let answer = call(&router, method, uri, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({member} supplied with nothing): expected HTTP 400 — a 501 would \
             say nobody implemented the operation — got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.shape().contains(VALIDATION_EXCEPTION),
            "case {case:?} ({member} supplied with nothing): expected a {VALIDATION_EXCEPTION}, \
             got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            supplied_nothing(member),
            "case {case:?}: expected {:?}, got {:?}",
            supplied_nothing(member),
            answer.message()
        );
    }

    let remaining = tags_of(&router, &arn).await;
    assert_eq!(
        remaining, seeded,
        "case \"a refused removal\": expected every seeded tag left in place, got {remaining:?}"
    );
}

/// Where the rule STOPS: a parameter that is present carries an ENTRY even when the
/// entry is the empty string — a list with one entry, not a list with none — so it is
/// refused for the entry it carried, with the framework's own constraint wording, and
/// not as a member that supplied nothing.
#[tokio::test]
async fn a_parameter_present_with_an_empty_entry_is_a_list_with_an_entry() {
    let router = emulator();
    let arn = seed_profile(&router, "Tag_Keys_Empty_Entry").await;
    let seeded: BTreeMap<String, String> = [("team".to_string(), "signing".to_string())]
        .into_iter()
        .collect();
    seed_tags(&router, &arn, &seeded).await;

    // The two wire forms of a parameter carrying no characters: the bare name, and the
    // name with an empty value.
    let rows: Vec<(&str, String)> = vec![
        ("the bare parameter name", format!("/tags/{}?tagKeys", encode(&arn))),
        (
            "the parameter name with an empty value",
            format!("/tags/{}?tagKeys=", encode(&arn)),
        ),
    ];

    for (case, uri) in &rows {
        let answer = call(&router, "DELETE", uri, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.message().contains("failed to satisfy constraint"),
            "case {case:?}: expected the framework's constraint wording, got {:?}",
            answer.message()
        );
        assert!(
            answer.message().contains("tagKeys"),
            "case {case:?}: expected the refusal to name the member, got {:?}",
            answer.message()
        );
        assert!(
            !answer.message().contains("must not be null or empty"),
            "case {case:?}: expected the supplied-nothing wording NOT used for an entry that \
             is present, got {:?}",
            answer.message()
        );
    }

    let remaining = tags_of(&router, &arn).await;
    assert_eq!(
        remaining, seeded,
        "case \"a refused removal\": expected every seeded tag left in place, got {remaining:?}"
    );
}

/// The same route with `tagKeys` SUPPLIED is served by the operation: exactly the keys
/// the request names are removed, a key the resource does not hold removes nothing and
/// still succeeds, and every removal is observable through a later read.
#[tokio::test]
async fn tag_keys_that_carry_entries_are_applied() {
    let router = emulator();
    let arn = seed_profile(&router, "Tag_Keys_Supplied").await;

    // A key carrying the punctuation `TagKey`'s pattern admits, beside three plain ones.
    let punctuated = "path/to:value=1";
    let seeded: BTreeMap<String, String> = [
        ("alpha".to_string(), "one".to_string()),
        ("beta".to_string(), "two".to_string()),
        ("gamma".to_string(), "three".to_string()),
        (punctuated.to_string(), "four".to_string()),
    ]
    .into_iter()
    .collect();
    seed_tags(&router, &arn, &seeded).await;

    // The rows are applied in order, each one's expectation read back from the service:
    // one key, two keys at once, a key the resource does not hold, and the punctuated
    // key.
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "one key named",
            vec!["alpha"],
            vec!["beta", "gamma", punctuated],
        ),
        (
            "two keys named at once",
            vec!["beta", "gamma"],
            vec![punctuated],
        ),
        (
            "a key the resource does not hold",
            vec!["never-written"],
            vec![punctuated],
        ),
        ("the punctuated key", vec![punctuated], vec![]),
    ];

    for (case, named, expected) in &rows {
        let query = named
            .iter()
            .map(|key| format!("tagKeys={}", encode(key)))
            .collect::<Vec<_>>()
            .join("&");
        let answer = call(
            &router,
            "DELETE",
            &format!("/tags/{}?{query}", encode(&arn)),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );
        let remaining = tags_of(&router, &arn).await;
        let left: Vec<String> = remaining.keys().cloned().collect();
        let want: Vec<String> = expected.iter().map(|key| key.to_string()).collect();
        assert_eq!(
            left, want,
            "case {case:?}: expected the keys {want:?} left, got {left:?}"
        );
    }
}
