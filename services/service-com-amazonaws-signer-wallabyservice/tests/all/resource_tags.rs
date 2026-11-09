// SPDX-License-Identifier: CC0-1.0

//! The tags of an AWS Signer signing profile, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! `@httpLabel` ARN, deserialization, the handler, the persisted record and
//! serialization — the same path a client takes. The rules under test are the ones
//! Signer states for the three operations bound to `/tags/{resourceArn}`:
//!
//! * a tagging call resolves its target by the ARN the resource was stored with,
//!   and the tag map one call writes is the one every later read of that resource
//!   reports — `TagResource` MERGES the keys it carries, `UntagResource` removes
//!   only the keys it names, and `ListTagsForResource` and `GetSigningProfile`
//!   report the same map;
//! * a `resourceArn` that is not a signing-profile ARN is refused 400
//!   `BadRequestException`, and a well-formed one naming no stored profile 404
//!   `NotFoundException`, for every one of the three operations;
//! * a tag key is one the modelled `TagKey` shape admits — inside its length
//!   bounds, outside AWS's reserved namespace, and made only of the characters
//!   its pattern's alphabet states — and every operation that takes tag keys
//!   refuses the whole request for any other key, writing and removing nothing;
//! * a CANCELED profile admits no tagging call at all — the modelled bad request
//!   "also occurs when you call a tagging API on a cancelled signing profile", so
//!   all three operations bound to that route answer it, while the profile stays
//!   viewable through the operations that are not tagging APIs.
//!
//! The cases are tables: a row per input class the rules generalise over (tag
//! count up to the modelled maximum, key and value length at both modelled bounds,
//! the punctuation `TagKey`'s alphabet admits, which keys a write names, which ARN
//! field is wrong, and the profile's status).

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_signer_wallabyservice::state::{
    ERROR_CODE_BAD_REQUEST, ERROR_CODE_PROFILE_NOT_FOUND, ERROR_CODE_VALIDATION,
    MAX_TAGS_PER_PROFILE, RESERVED_TAG_KEY_PREFIX, TAG_KEY_MAX_LENGTH, TAG_KEY_MIN_LENGTH,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer,
/// and whose region a profile ARN is built for.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_US_WEST_2`] signs for.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents, spelled out here rather than
/// imported so a wrong account in the handlers fails this suite.
const ACCOUNT_ID: &str = "000000000000";

/// An ACM certificate ARN for `signingMaterial`.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The status an admitted `TagResource` / `UntagResource` answers with.
///
/// Both response shapes declare no members (`smithy_query(query="shape",
/// shape="TagResourceResponse,UntagResourceResponse")` — `{}`), so a successful
/// write has nothing to report and the reply is `204 No Content`. The READ of the
/// same route answers a document, so it keeps the `200` of a reply that carries
/// one.
const NO_CONTENT_STATUS: u16 = 204;

/// The status a tagging READ answers with: it reports a document.
const DOCUMENT_STATUS: u16 = 200;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status, JSON body, and the `x-amzn-errortype`
/// header restJson1 resolves an error SHAPE from.
struct Answer {
    status: u16,
    body: Value,
    error_type: Option<String>,
}

impl Answer {
    /// The modelled `code` member, or `None` when the body carries none.
    fn code(&self) -> Option<&str> {
        self.body["code"].as_str()
    }

    /// The modelled `message` member, or `None` when the body carries none.
    fn message(&self) -> Option<&str> {
        self.body["message"].as_str()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }

    /// The `tags` map this answer reports, as a sorted map so two answers compare
    /// regardless of member order. `None` when the answer reports no map at all —
    /// which is a different claim from reporting an empty one.
    fn tags(&self) -> Option<BTreeMap<String, String>> {
        let map = self.body.get("tags")?.as_object()?;
        Some(
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
        )
    }
}

/// `value` percent-encoded for ONE URI segment or query value.
///
/// `resourceArn` is a non-greedy `@httpLabel` on `/tags/{resourceArn}`
/// (`smithy_query(query="operation", operation="ListTagsForResource")`), and a
/// signing-profile ARN carries both `:` and `/` — so an SDK encodes everything
/// outside the unreserved set, which is what keeps the ARN one segment. The same
/// encoding is what a `tagKeys` query value needs, since `TagKey`'s alphabet
/// admits `/`, `:` and `=`.
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
        .header("host", "signer.us-west-2.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_WEST_2);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
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
    Answer {
        status,
        body,
        error_type,
    }
}

async fn put_profile(router: &MultiServiceRouter, name: &str, request: Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(request),
    )
    .await
}

async fn get_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("/signing-profiles/{name}"), None).await
}

async fn cancel_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "DELETE", &format!("/signing-profiles/{name}"), None).await
}

async fn revoke_profile(router: &MultiServiceRouter, name: &str, profile_version: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(json!({
            "profileVersion": profile_version,
            "reason": "key-compromise",
            "effectiveTime": 1_700_000_000,
        })),
    )
    .await
}

/// `POST /tags/{resourceArn}` — `TagResource`.
async fn tag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> Answer {
    let map: Map<String, Value> = tags
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    call(
        router,
        "POST",
        &format!("/tags/{}", encode(resource_arn)),
        Some(json!({ "tags": Value::Object(map) })),
    )
    .await
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", encode(resource_arn)), None).await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is an
/// `@httpQuery` list, so it travels as one pair per key.
async fn untag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tag_keys: &[&str],
) -> Answer {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode(key)))
        .collect::<Vec<_>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", encode(resource_arn)),
        None,
    )
    .await
}

/// Create an ACTIVE signing profile and answer the ARN it was stored with — the
/// label every tagging call in this suite resolves through.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> String {
    let put = put_profile(
        router,
        name,
        json!({
            "platformId": "AWSLambda-SHA384-ECDSA",
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        }),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "seeding {name}: expected PutSigningProfile 200, got {} {:?}",
        put.status, put.body
    );
    let arn = put.str_member("arn");
    assert_eq!(
        arn,
        format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{name}"),
        "seeding {name}: unexpected profile ARN"
    );
    arn
}

/// A tag map from `(key, value)` pairs.
fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One tag map to write and read back. The rows differ along what the rule
/// generalises over: how many tags, the length of a key and of a value at the
/// modelled bounds, and which characters the key carries.
struct TagCase {
    case: &'static str,
    profile: &'static str,
    tags: BTreeMap<String, String>,
}

fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "one tag, the key at TagKey's @length minimum of 1",
            profile: "Minimal_Key",
            tags: tags(&[("a", "1")]),
        },
        TagCase {
            case: "three tags",
            profile: "Several_Tags",
            tags: tags(&[("env", "prod"), ("team", "iot"), ("tier", "gold")]),
        },
        TagCase {
            case: "a value at TagValue's @length minimum of 0 (present and empty)",
            profile: "Empty_Value",
            tags: tags(&[("env", "")]),
        },
        TagCase {
            // `TagKey` is @length(1, 128) and `TagValue` @length(0, 256)
            // (smithy_query(query="shape", shape="TagMap")).
            case: "a key at TagKey's @length maximum of 128 and a value at TagValue's 256",
            profile: "Bound_Lengths",
            tags: [
                ("k".repeat(128), "v".repeat(256)),
                ("short".to_string(), String::new()),
            ]
            .into_iter()
            .collect(),
        },
        TagCase {
            // Every punctuation character `TagKey`'s pattern
            // `^(?!aws:)[a-zA-Z+-=._:/]+$` admits beside the letters and the
            // digits that pattern's `+-=` range covers.
            case: "keys carrying the punctuation TagKey's alphabet admits",
            profile: "Punctuated_Keys",
            tags: tags(&[
                ("a+b", "plus"),
                ("a-b", "minus"),
                ("a=b", "equals"),
                ("a.b", "dot"),
                ("a_b", "underscore"),
                ("a:b", "colon"),
                ("a/b", "slash"),
                ("key9", "digit"),
            ]),
        },
        TagCase {
            case: "the modelled maximum of 200 tags",
            profile: "Full_Tag_Map",
            tags: (0..MAX_TAGS_PER_PROFILE)
                .map(|index| (format!("key{index}"), format!("value{index}")))
                .collect(),
        },
    ]
}

/// What `TagResource` writes is exactly what `ListTagsForResource` reports, for
/// every tag-map shape the model admits — and `GetSigningProfile` reports the same
/// map, because there is ONE persisted map per profile.
#[tokio::test]
async fn tagging_a_profile_round_trips_through_every_reader() {
    let router = emulator();

    for case in tag_cases() {
        let label = case.case;
        let arn = seed_profile(&router, case.profile).await;

        // A profile created without tags reports a present, EMPTY map: "no tags"
        // and "tags not reported" are different claims.
        let before = list_tags(&router, &arn).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: expected ListTagsForResource 200 before tagging, got {} {:?}",
            before.status, before.body
        );
        assert_eq!(
            before.tags(),
            Some(BTreeMap::new()),
            "case {label:?}: expected an empty tag map before tagging, got {:?}",
            before.body
        );

        let written = tag_resource(&router, &arn, &case.tags).await;
        assert_eq!(
            written.status, NO_CONTENT_STATUS,
            "case {label:?}: expected TagResource {NO_CONTENT_STATUS}, got {} {:?}",
            written.status, written.body
        );

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected ListTagsForResource 200, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.tags().as_ref(),
            Some(&case.tags),
            "case {label:?}: expected the tags that were written, got {:?}",
            listed.tags()
        );

        // The same persisted map, through the other reader of it.
        let profile = get_profile(&router, case.profile).await;
        assert_eq!(
            profile.tags().as_ref(),
            Some(&case.tags),
            "case {label:?}: expected GetSigningProfile to report the same map, got {:?}",
            profile.tags()
        );

        // Listing is a READ: a second caller sees exactly what the first did.
        let again = list_tags(&router, &arn).await;
        assert_eq!(
            again.tags(),
            listed.tags(),
            "case {label:?}: expected the tag map to be stable across reads, got {:?}",
            again.body
        );
    }
}

/// One write-then-write-then-remove walk over a profile's tags.
struct MergeCase {
    case: &'static str,
    profile: &'static str,
    first: BTreeMap<String, String>,
    /// The second `TagResource`, or an empty map for a walk that makes only one
    /// write (`TagMap` is `@length(min: 1)`, so an empty write is not a request a
    /// client can make).
    second: BTreeMap<String, String>,
    remove: &'static [&'static str],
    expected: BTreeMap<String, String>,
}

fn merge_cases() -> Vec<MergeCase> {
    vec![
        MergeCase {
            case: "a second write ADDS its key and leaves the first one",
            profile: "Merge_Adds",
            first: tags(&[("env", "prod")]),
            second: tags(&[("team", "iot")]),
            remove: &[],
            expected: tags(&[("env", "prod"), ("team", "iot")]),
        },
        MergeCase {
            case: "a second write RE-TAGS a key the profile already holds",
            profile: "Merge_Replaces",
            first: tags(&[("env", "prod"), ("team", "iot")]),
            second: tags(&[("env", "staging")]),
            remove: &[],
            expected: tags(&[("env", "staging"), ("team", "iot")]),
        },
        MergeCase {
            case: "a removal takes only the key it names",
            profile: "Untag_One",
            first: tags(&[("env", "prod"), ("team", "iot")]),
            second: tags(&[("tier", "gold")]),
            remove: &["team"],
            expected: tags(&[("env", "prod"), ("tier", "gold")]),
        },
        MergeCase {
            case: "removing a key the profile never held removes nothing",
            profile: "Untag_Absent",
            first: tags(&[("env", "prod")]),
            second: BTreeMap::new(),
            remove: &["never-set"],
            expected: tags(&[("env", "prod")]),
        },
        MergeCase {
            case: "removing every key leaves a present, empty map",
            profile: "Untag_All",
            first: tags(&[("env", "prod"), ("team", "iot")]),
            second: BTreeMap::new(),
            remove: &["env", "team"],
            expected: BTreeMap::new(),
        },
        MergeCase {
            case: "removing keys carrying the punctuation TagKey's alphabet admits",
            profile: "Untag_Punctuated",
            first: tags(&[("a/b", "1"), ("c:d", "2"), ("e=f", "3")]),
            second: BTreeMap::new(),
            remove: &["a/b", "e=f"],
            expected: tags(&[("c:d", "2")]),
        },
        MergeCase {
            case: "a removal of several keys at once",
            profile: "Untag_Several",
            first: tags(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            second: tags(&[("e", "5")]),
            remove: &["a", "c", "e"],
            expected: tags(&[("b", "2"), ("d", "4")]),
        },
    ]
}

/// `TagResource` merges and `UntagResource` removes only what each call names, so
/// the map a profile ends up with is the sum of the writes it saw.
#[tokio::test]
async fn writes_merge_and_removals_take_only_the_keys_they_name() {
    let router = emulator();

    for case in merge_cases() {
        let label = case.case;
        let arn = seed_profile(&router, case.profile).await;

        let first = tag_resource(&router, &arn, &case.first).await;
        assert_eq!(
            first.status, NO_CONTENT_STATUS,
            "case {label:?}: expected the first TagResource {NO_CONTENT_STATUS}, got {} {:?}",
            first.status, first.body
        );
        if !case.second.is_empty() {
            let second = tag_resource(&router, &arn, &case.second).await;
            assert_eq!(
                second.status, NO_CONTENT_STATUS,
                "case {label:?}: expected the second TagResource {NO_CONTENT_STATUS}, got {} {:?}",
                second.status, second.body
            );
        }
        if !case.remove.is_empty() {
            let removed = untag_resource(&router, &arn, case.remove).await;
            assert_eq!(
                removed.status, NO_CONTENT_STATUS,
                "case {label:?}: expected UntagResource {NO_CONTENT_STATUS}, got {} {:?}",
                removed.status, removed.body
            );
        }

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.tags().as_ref(),
            Some(&case.expected),
            "case {label:?}: expected the tag map {:?}, got {:?}",
            case.expected,
            listed.tags()
        );
        let profile = get_profile(&router, case.profile).await;
        assert_eq!(
            profile.tags().as_ref(),
            Some(&case.expected),
            "case {label:?}: expected GetSigningProfile to report the same map, got {:?}",
            profile.tags()
        );
    }
}

/// A `resourceArn` the tagging operations cannot resolve, and what every one of
/// them answers for it.
struct ArnRefusalCase {
    case: &'static str,
    resource_arn: String,
    status: u16,
    /// The modelled error SHAPE, which is also the `x-amzn-errortype` value.
    shape: &'static str,
    /// The `code` member naming the condition that was refused.
    code: &'static str,
}

/// A label that is not a signing-profile ARN is a bad REQUEST (400), and a
/// well-formed ARN naming no stored profile is a missing RESOURCE (404) — the same
/// two answers from all three operations bound to `/tags/{resourceArn}`.
#[tokio::test]
async fn an_unresolvable_resource_arn_is_refused_the_same_way_by_every_tagging_operation() {
    let router = emulator();
    let arn = seed_profile(&router, "Tagged_Profile").await;
    let version = get_profile(&router, "Tagged_Profile")
        .await
        .str_member("profileVersion");
    assert_eq!(
        tag_resource(&router, &arn, &tags(&[("env", "prod")])).await.status,
        NO_CONTENT_STATUS,
        "seeding the tagged profile"
    );

    let cases = vec![
        ArnRefusalCase {
            case: "a label that is not an ARN at all",
            resource_arn: "not-an-arn".to_string(),
            status: 400,
            shape: "BadRequestException",
            code: ERROR_CODE_BAD_REQUEST,
        },
        ArnRefusalCase {
            case: "an ARN of another service",
            resource_arn: format!("arn:aws:acm:{REGION}:{ACCOUNT_ID}:certificate/abc"),
            status: 400,
            shape: "BadRequestException",
            code: ERROR_CODE_BAD_REQUEST,
        },
        ArnRefusalCase {
            case: "a signer ARN whose resource is not a signing profile",
            resource_arn: format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/abc"),
            status: 400,
            shape: "BadRequestException",
            code: ERROR_CODE_BAD_REQUEST,
        },
        ArnRefusalCase {
            // Tags belong to the profile, not to one of its versions, so the
            // profileVersionArn is not a tagging label.
            case: "the profile VERSION ARN of a profile that does exist",
            resource_arn: format!("{arn}/{version}"),
            status: 400,
            shape: "BadRequestException",
            code: ERROR_CODE_BAD_REQUEST,
        },
        ArnRefusalCase {
            case: "an ARN with no region",
            resource_arn: format!("arn:aws:signer::{ACCOUNT_ID}:/signing-profiles/Tagged_Profile"),
            status: 400,
            shape: "BadRequestException",
            code: ERROR_CODE_BAD_REQUEST,
        },
        ArnRefusalCase {
            case: "a well-formed ARN naming a profile that was never created",
            resource_arn: format!(
                "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/No_Such_Profile"
            ),
            status: 404,
            shape: "NotFoundException",
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
        ArnRefusalCase {
            case: "the profile's name in ANOTHER region",
            resource_arn: format!(
                "arn:aws:signer:eu-west-1:{ACCOUNT_ID}:/signing-profiles/Tagged_Profile"
            ),
            status: 404,
            shape: "NotFoundException",
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
        ArnRefusalCase {
            case: "the profile's name in ANOTHER account",
            resource_arn: format!(
                "arn:aws:signer:{REGION}:123456789012:/signing-profiles/Tagged_Profile"
            ),
            status: 404,
            shape: "NotFoundException",
            code: ERROR_CODE_PROFILE_NOT_FOUND,
        },
    ];

    for row in cases {
        let case = row.case;
        for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
            let answer = match operation {
                "ListTagsForResource" => list_tags(&router, &row.resource_arn).await,
                "TagResource" => {
                    tag_resource(&router, &row.resource_arn, &tags(&[("env", "prod")])).await
                }
                _ => untag_resource(&router, &row.resource_arn, &["env"]).await,
            };
            assert_eq!(
                answer.status, row.status,
                "case {case:?} on {operation}: expected HTTP {}, got {} {:?}",
                row.status, answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                Some(row.code),
                "case {case:?} on {operation}: expected the modelled code {}, got {:?}",
                row.code,
                answer.body
            );
            assert!(
                answer.message().is_some_and(|message| !message.is_empty()),
                "case {case:?} on {operation}: expected a message beside the code, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some(row.shape),
                "case {case:?} on {operation}: expected x-amzn-errortype {}, got {:?}",
                row.shape,
                answer.error_type
            );
        }
    }

    // Every refusal above wrote nothing: the profile still holds the one tag it
    // was seeded with.
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.tags().as_ref(),
        Some(&tags(&[("env", "prod")])),
        "a refused tagging call must not change the stored map, got {:?}",
        listed.tags()
    );
}

/// One profile status a tagging call can arrive in, and what the whole tagging
/// route answers for it.
struct TaggingStatusCase {
    case: &'static str,
    profile: &'static str,
    /// The status the store holds for the profile when the three calls arrive —
    /// asserted through `GetSigningProfile`, so a row that failed to reach that
    /// status is not read as a statement about it.
    stored_status: &'static str,
    /// The HTTP status EVERY operation bound to `/tags/{resourceArn}` answers in
    /// that status.
    ///
    /// `None` when the status ADMITS the route, because the three operations then
    /// answer the envelope each one's own output has: the read a document, the two
    /// writers no content. A refusal is one status for all three.
    refusal_status: Option<u16>,
}

/// One row per status a profile can be in when a tagging call arrives, plus a
/// second `Active` profile in another region — the statuses are
/// `SigningProfileStatus`'s values (`smithy_query(query="shape",
/// shape="SigningProfileStatus")`: `Active | Canceled | Revoked`).
fn tagging_status_cases() -> [TaggingStatusCase; 4] {
    [
        TaggingStatusCase {
            case: "Active",
            profile: "Active_Tagging",
            stored_status: "Active",
            refusal_status: None,
        },
        TaggingStatusCase {
            case: "Revoked",
            profile: "Revoked_Tagging",
            stored_status: "Revoked",
            refusal_status: None,
        },
        TaggingStatusCase {
            case: "Canceled",
            profile: "Canceled_Tagging",
            stored_status: "Canceled",
            refusal_status: Some(400),
        },
        TaggingStatusCase {
            case: "Active after a refused create",
            profile: "Recreated_Tagging",
            stored_status: "Active",
            refusal_status: None,
        },
    ]
}

/// A profile's STORED status decides whether the tagging route admits a call at
/// all: a CANCELED profile refuses EVERY operation bound to
/// `/tags/{resourceArn}` — the read as much as the two writes — with 400
/// `BadRequestException` naming that status, because the shape documents that it
/// "also occurs when you call a tagging API on a cancelled signing profile";
/// an ACTIVE or REVOKED profile admits all three, because nothing documents a
/// refusal for those. The profile itself stays viewable through the operations
/// that are not tagging APIs, which is where a refused call is shown to have
/// changed nothing.
#[tokio::test]
async fn the_profiles_status_decides_which_tagging_calls_it_admits() {
    for row in tagging_status_cases() {
        let case = row.case;
        let name = row.profile;
        let refusal_status = row.refusal_status;
        let router = emulator();
        let arn = seed_profile(&router, name).await;
        let seeded = tags(&[("env", "prod")]);
        assert_eq!(
            tag_resource(&router, &arn, &seeded).await.status,
            NO_CONTENT_STATUS,
            "case {case:?}: the profile must be taggable while Active"
        );

        match case {
            "Revoked" => {
                let version = get_profile(&router, name).await.str_member("profileVersion");
                let revoked = revoke_profile(&router, name, &version).await;
                assert_eq!(
                    revoked.status, 204,
                    "case {case:?}: the revoke must succeed, got {:?}",
                    revoked.body
                );
            }
            "Canceled" => {
                assert_eq!(
                    cancel_profile(&router, name).await.status,
                    204,
                    "case {case:?}: the cancel must succeed"
                );
            }
            _ => {}
        }
        if case == "Active after a refused create" {
            // The name is taken, so the second create is refused and the stored
            // profile — and with it the tagging route's gate — is untouched: the
            // gate follows the STORED status.
            let refused = put_profile(
                &router,
                name,
                json!({
                    "platformId": "AWSLambda-SHA384-ECDSA",
                    "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                }),
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected the repeated create to be refused, got {} {:?}",
                refused.status, refused.body
            );
        }

        // What the store holds now, so the rest of the row is a statement about
        // that status.
        let before = get_profile(&router, name).await;
        assert_eq!(
            before.str_member("status"),
            row.stored_status,
            "case {case:?}: expected the stored status {}, got {:?}",
            row.stored_status,
            before.body
        );

        // Every operation bound to the tagging route, asked of the profile in
        // that status: all three answer alike, because the documented refusal
        // names the CALL and not the change it would make.
        let added = tags(&[("team", "iot")]);
        for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
            let answer = match operation {
                "ListTagsForResource" => list_tags(&router, &arn).await,
                "TagResource" => tag_resource(&router, &arn, &added).await,
                _ => untag_resource(&router, &arn, &["env"]).await,
            };
            // Admitted, each operation answers the envelope its own output has;
            // refused, all three answer the one refusal status.
            let expected_status = refusal_status.unwrap_or(match operation {
                "ListTagsForResource" => DOCUMENT_STATUS,
                _ => NO_CONTENT_STATUS,
            });
            assert_eq!(
                answer.status, expected_status,
                "case {case:?} on {operation}: expected HTTP {expected_status}, got {} {:?}",
                answer.status, answer.body
            );
            if refusal_status.is_none() {
                continue;
            }
            assert_eq!(
                answer.code(),
                Some("ProfileCanceled"),
                "case {case:?} on {operation}: expected the code to name the status that forbids \
                 the call, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("BadRequestException"),
                "case {case:?} on {operation}: expected an x-amzn-errortype header, got {:?}",
                answer.error_type
            );
            assert!(
                answer
                    .message()
                    .is_some_and(|message| message.contains(name)
                        && message.contains(row.stored_status)),
                "case {case:?} on {operation}: expected a message naming the profile and the \
                 status that forbids the call, got {:?}",
                answer.message()
            );
        }

        // `GetSigningProfile` is not a tagging API, so a canceled profile is
        // still viewable through it — and the map it reports is the one the three
        // calls left: what the admitted write and removal did, or the untouched
        // map a refused route changed nothing of.
        let expected = if refusal_status.is_none() {
            // The write added a key and the removal took the seeded one.
            added
        } else {
            seeded
        };
        let profile = get_profile(&router, name).await;
        assert_eq!(
            profile.status, DOCUMENT_STATUS,
            "case {case:?}: a profile stays viewable in every status, got {} {:?}",
            profile.status, profile.body
        );
        assert_eq!(
            profile.tags().as_ref(),
            Some(&expected),
            "case {case:?}: expected the tag map {expected:?}, got {:?}",
            profile.tags()
        );

        // Where the route is admitted, its READ reports that same map — so the
        // row covers the body `ListTagsForResource` answers, not only the status.
        if refusal_status.is_none() {
            let listed = list_tags(&router, &arn).await;
            assert_eq!(
                listed.tags().as_ref(),
                Some(&expected),
                "case {case:?}: expected ListTagsForResource to report the tag map \
                 {expected:?}, got {:?}",
                listed.tags()
            );
        }
    }
}

/// A write that would leave the profile outside a modelled bound is refused and
/// changes nothing: no key may sit in AWS's reserved `aws:` namespace, and a
/// profile holds at most `TagMap`'s maximum of 200 tags.
#[tokio::test]
async fn a_write_outside_the_modelled_tag_bounds_is_refused_and_stores_nothing() {
    let router = emulator();
    let arn = seed_profile(&router, "Bounded_Tags").await;

    // The reserved prefix, on its own and beside a key that is fine: one bad key
    // refuses the whole write.
    for keys in [
        vec![("aws:cloudformation:stack-name", "stack")],
        vec![("env", "prod"), ("aws:createdBy", "someone")],
    ] {
        let refused = tag_resource(&router, &arn, &tags(&keys)).await;
        assert_eq!(
            refused.status, 400,
            "case {keys:?}: expected a reserved tag key to be refused 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            Some(ERROR_CODE_BAD_REQUEST),
            "case {keys:?}: expected the modelled code, got {:?}",
            refused.body
        );
        assert_eq!(
            list_tags(&router, &arn).await.tags(),
            Some(BTreeMap::new()),
            "case {keys:?}: a refused write must store nothing"
        );
    }

    // A key whose VALUE starts with the reserved prefix is fine — the prefix is
    // reserved for keys.
    let allowed = tags(&[("owner", "aws:someone")]);
    assert_eq!(
        tag_resource(&router, &arn, &allowed).await.status,
        NO_CONTENT_STATUS,
        "a reserved prefix in a tag VALUE is not refused"
    );
    assert_eq!(
        untag_resource(&router, &arn, &["owner"]).await.status,
        NO_CONTENT_STATUS,
        "clearing the profile before the count cases"
    );

    // Exactly the modelled maximum is admitted...
    let full: BTreeMap<String, String> = (0..MAX_TAGS_PER_PROFILE)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect();
    assert_eq!(
        tag_resource(&router, &arn, &full).await.status,
        NO_CONTENT_STATUS,
        "expected the modelled maximum of {MAX_TAGS_PER_PROFILE} tags to be admitted"
    );
    // ... re-tagging a key the profile already holds never grows the map, so it
    // is admitted at the maximum ...
    let retag = tags(&[("key0", "replaced")]);
    assert_eq!(
        tag_resource(&router, &arn, &retag).await.status,
        NO_CONTENT_STATUS,
        "expected a re-tag at the maximum to be admitted"
    );
    // ... and one key BEYOND it is refused, leaving the map as it was.
    let over = tags(&[("one-too-many", "x")]);
    let refused = tag_resource(&router, &arn, &over).await;
    assert_eq!(
        refused.status, 400,
        "expected the {}th tag to be refused, got {} {:?}",
        MAX_TAGS_PER_PROFILE + 1,
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.code(),
        Some(ERROR_CODE_BAD_REQUEST),
        "expected the modelled code on the over-limit write, got {:?}",
        refused.body
    );

    let listed = list_tags(&router, &arn).await;
    let stored = listed
        .tags()
        .expect("ListTagsForResource reports a tag map");
    assert_eq!(
        stored.len(),
        MAX_TAGS_PER_PROFILE,
        "expected the refused write to leave {MAX_TAGS_PER_PROFILE} tags, got {}",
        stored.len()
    );
    assert_eq!(
        stored.get("key0").map(String::as_str),
        Some("replaced"),
        "expected the admitted re-tag to have replaced the value, got {:?}",
        stored.get("key0")
    );
    assert!(
        !stored.contains_key("one-too-many"),
        "expected the refused key to be absent, got {stored:?}"
    );
}

/// One request naming tag keys. The rows differ along what the rule generalises
/// over: how many keys the request carries, where the reserved one sits among
/// them, and how deep the reserved key's own namespace goes.
struct ReservedKeyCase {
    case: &'static str,
    /// The keys the request names; at least one of them is reserved.
    keys: &'static [&'static str],
    /// The key the refusal is expected to name — the first reserved one.
    reserved: &'static str,
}

/// The reserved keys below are spellings of the ONE prefix `TagKey`'s
/// `@pattern` excludes with its negative lookahead,
/// `^(?!aws:)[a-zA-Z+-=._:/]+$` (`smithy_query(query="shape",
/// shape="TagKeyList")`), which is `RESERVED_TAG_KEY_PREFIX`; the rest of each
/// key uses only characters that pattern's alphabet admits.
fn reserved_key_cases() -> Vec<ReservedKeyCase> {
    vec![
        ReservedKeyCase {
            case: "the bare reserved prefix, alone",
            keys: &["aws:"],
            reserved: "aws:",
        },
        ReservedKeyCase {
            case: "a deep reserved key, alone",
            keys: &["aws:cloudformation:stack-name"],
            reserved: "aws:cloudformation:stack-name",
        },
        ReservedKeyCase {
            case: "a legal key first, the reserved one second",
            keys: &["env", "aws:createdBy"],
            reserved: "aws:createdBy",
        },
        ReservedKeyCase {
            case: "the reserved key first, legal keys after it",
            keys: &["aws:createdBy", "env", "team"],
            reserved: "aws:createdBy",
        },
        ReservedKeyCase {
            case: "two reserved keys among legal ones",
            keys: &["env", "aws:createdBy", "team", "aws:cloudformation:logical-id"],
            reserved: "aws:createdBy",
        },
    ]
}

/// Keys that only LOOK reserved: the namespace is the exact prefix `aws:` at the
/// START of the key, and the pattern that excludes it is case-sensitive, so each
/// of these is a key `TagKey` admits.
const KEYS_OUTSIDE_THE_RESERVED_NAMESPACE: &[&str] =
    &["AWS:createdBy", "awsome", "env:aws:region", "paws:claw"];

/// A tag key inside AWS's reserved namespace is not a key `TagKey` admits, so
/// every operation that takes tag keys refuses the whole request 400 and changes
/// nothing — `UntagResource` removes none of the keys it names (not even the
/// legal ones beside the reserved key), `TagResource` writes none of them, and
/// `PutSigningProfile` stores no profile.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_by_every_operation_that_takes_tag_keys() {
    let router = emulator();
    let seeded = tags(&[("env", "prod"), ("team", "iot")]);

    for (index, row) in reserved_key_cases().into_iter().enumerate() {
        let case = row.case;
        let profile = format!("Reserved_Keys_{index}");
        let arn = seed_profile(&router, &profile).await;
        assert_eq!(
            tag_resource(&router, &arn, &seeded).await.status,
            NO_CONTENT_STATUS,
            "case {case:?}: seeding the profile's tags must succeed"
        );

        // The removal half: refused, and every seeded key is still there.
        let removal = untag_resource(&router, &arn, row.keys).await;
        assert_eq!(
            removal.status, 400,
            "case {case:?}: expected UntagResource to refuse {:?} with 400, got {} {:?}",
            row.keys, removal.status, removal.body
        );
        assert_eq!(
            removal.code(),
            Some(ERROR_CODE_BAD_REQUEST),
            "case {case:?}: expected the modelled code, got {:?}",
            removal.body
        );
        assert_eq!(
            removal.error_type.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected the modelled error shape, got {:?}",
            removal.error_type
        );
        assert!(
            removal.message().unwrap_or_default().contains(row.reserved),
            "case {case:?}: expected the refusal to name the reserved key {:?}, got {:?}",
            row.reserved,
            removal.message()
        );
        assert_eq!(
            list_tags(&router, &arn).await.tags().as_ref(),
            Some(&seeded),
            "case {case:?}: a refused removal must remove nothing, got {:?}",
            list_tags(&router, &arn).await.tags()
        );

        // The write half, over the same keys: refused, and nothing added.
        let written: BTreeMap<String, String> = row
            .keys
            .iter()
            .map(|key| ((*key).to_string(), "written".to_string()))
            .collect();
        let write = tag_resource(&router, &arn, &written).await;
        assert_eq!(
            write.status, 400,
            "case {case:?}: expected TagResource to refuse {:?} with 400, got {} {:?}",
            row.keys, write.status, write.body
        );
        assert_eq!(
            list_tags(&router, &arn).await.tags().as_ref(),
            Some(&seeded),
            "case {case:?}: a refused write must store nothing, got {:?}",
            list_tags(&router, &arn).await.tags()
        );

        // The create half: the same keys on `PutSigningProfile`, whose modelled
        // 400 is `ValidationException` — and no profile is stored for it.
        let created = format!("Reserved_Create_{index}");
        let create = put_profile(
            &router,
            &created,
            json!({
                "platformId": "AWSLambda-SHA384-ECDSA",
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                "tags": Value::Object(
                    written
                        .iter()
                        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                        .collect::<Map<String, Value>>(),
                ),
            }),
        )
        .await;
        assert_eq!(
            create.status, 400,
            "case {case:?}: expected PutSigningProfile to refuse {:?} with 400, got {} {:?}",
            row.keys, create.status, create.body
        );
        assert_eq!(
            create.code(),
            Some(ERROR_CODE_VALIDATION),
            "case {case:?}: expected the modelled code, got {:?}",
            create.body
        );
        let read_back = get_profile(&router, &created).await;
        assert_eq!(
            read_back.status, 404,
            "case {case:?}: a refused create must store no profile, got {} {:?}",
            read_back.status, read_back.body
        );
    }
}

/// The refusal is the reserved NAMESPACE, not a resemblance to it: a key the
/// prefix rule does not catch round-trips through the create, the write and the
/// removal. The namespace is also judged before the `resourceArn` is resolved,
/// so both halves of the tagging pair answer the same refusal for a reserved key
/// on an ARN that reaches no profile.
#[tokio::test]
async fn a_key_outside_the_reserved_namespace_round_trips_through_every_tag_operation() {
    let router = emulator();

    for key in KEYS_OUTSIDE_THE_RESERVED_NAMESPACE {
        assert!(
            !key.starts_with(RESERVED_TAG_KEY_PREFIX),
            "case {key:?}: this row must be a key outside the reserved namespace"
        );
    }

    // Created WITH the keys: a create carrying them stores them.
    let created = tags(
        &KEYS_OUTSIDE_THE_RESERVED_NAMESPACE
            .iter()
            .map(|key| (*key, "created"))
            .collect::<Vec<_>>(),
    );
    let create = put_profile(
        &router,
        "Lookalike_Keys",
        json!({
            "platformId": "AWSLambda-SHA384-ECDSA",
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            "tags": Value::Object(
                created
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect::<Map<String, Value>>(),
            ),
        }),
    )
    .await;
    assert_eq!(
        create.status, 200,
        "expected keys outside the reserved namespace to be admitted by the create, got {} {:?}",
        create.status, create.body
    );
    let arn = format!(
        "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{}",
        "Lookalike_Keys"
    );
    assert_eq!(
        list_tags(&router, &arn).await.tags().as_ref(),
        Some(&created),
        "expected the created tags to be reported, got {:?}",
        list_tags(&router, &arn).await.tags()
    );

    // Written and removed one at a time: each key is admitted by both halves.
    for key in KEYS_OUTSIDE_THE_RESERVED_NAMESPACE {
        let write = tag_resource(&router, &arn, &tags(&[(key, "written")])).await;
        assert_eq!(
            write.status, NO_CONTENT_STATUS,
            "case {key:?}: expected TagResource {NO_CONTENT_STATUS}, got {} {:?}",
            write.status, write.body
        );
        assert_eq!(
            list_tags(&router, &arn)
                .await
                .tags()
                .and_then(|map| map.get(*key).cloned()),
            Some("written".to_string()),
            "case {key:?}: expected the written value to be reported"
        );
        let removal = untag_resource(&router, &arn, &[key]).await;
        assert_eq!(
            removal.status, NO_CONTENT_STATUS,
            "case {key:?}: expected UntagResource {NO_CONTENT_STATUS}, got {} {:?}",
            removal.status, removal.body
        );
        assert!(
            list_tags(&router, &arn)
                .await
                .tags()
                .is_some_and(|map| !map.contains_key(*key)),
            "case {key:?}: expected the removed key to be gone"
        );
    }

    // A reserved key on an ARN that reaches no profile: the request's own keys
    // are judged first, so the pair answers the bad request rather than the
    // missing resource.
    let missing = format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/No_Such_Profile");
    for (operation, answer) in [
        (
            "UntagResource",
            untag_resource(&router, &missing, &["aws:createdBy"]).await,
        ),
        (
            "TagResource",
            tag_resource(&router, &missing, &tags(&[("aws:createdBy", "someone")])).await,
        ),
    ] {
        assert_eq!(
            answer.status, 400,
            "case {operation:?}: expected the reserved key to be refused 400 ahead of the ARN \
             lookup, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            Some(ERROR_CODE_BAD_REQUEST),
            "case {operation:?}: expected the modelled code, got {:?}",
            answer.body
        );
        assert_ne!(
            answer.code(),
            Some(ERROR_CODE_PROFILE_NOT_FOUND),
            "case {operation:?}: the reserved key is refused before the profile lookup"
        );
    }
}

/// A `tags` JSON object carrying ONE tag. The key is an expression rather than a
/// literal, which is what `json!`'s object syntax cannot spell.
fn tags_json(key: &str, value: &str) -> Value {
    Value::Object(
        [(key.to_string(), Value::String(value.to_string()))]
            .into_iter()
            .collect::<Map<String, Value>>(),
    )
}

/// One tag key the modelled alphabet does not admit. The rows differ along what
/// the rule generalises over: WHICH character outside the alphabet the key
/// carries and where in the key it sits.
///
/// The alphabet is the character class of `TagKey`'s `@pattern`,
/// `^(?!aws:)[a-zA-Z+-=._:/]+$` (`smithy_query(query="shape", shape="TagKey")`):
/// the ASCII letters, the inclusive range from `+` to `=` — which covers the
/// digits and `+ , - . / : ; < =` — and `_`. Every character below sits outside
/// it, so no key below is one AWS Signer could hold.
struct RejectedKeyCase {
    case: &'static str,
    key: &'static str,
}

fn keys_outside_the_modelled_alphabet() -> Vec<RejectedKeyCase> {
    vec![
        RejectedKeyCase {
            case: "a space inside the key",
            key: "my key",
        },
        RejectedKeyCase {
            case: "a space in front of an otherwise admitted key",
            key: " env",
        },
        RejectedKeyCase {
            case: "an at sign",
            key: "owner@team",
        },
        RejectedKeyCase {
            case: "a hash",
            key: "cost#center",
        },
        RejectedKeyCase {
            case: "braces",
            key: "a{b}",
        },
        RejectedKeyCase {
            case: "square brackets",
            key: "tag[0]",
        },
        RejectedKeyCase {
            case: "an exclamation mark, at the end of the key",
            key: "env!",
        },
        RejectedKeyCase {
            case: "a percent sign",
            key: "100%",
        },
        RejectedKeyCase {
            case: "an ampersand",
            key: "a&b",
        },
        RejectedKeyCase {
            case: "a star — the character immediately below the alphabet's + to = range",
            key: "a*b",
        },
        RejectedKeyCase {
            case: "a caret",
            key: "a^b",
        },
        RejectedKeyCase {
            case: "a question mark",
            key: "why?",
        },
        RejectedKeyCase {
            case: "a backslash",
            key: "a\\b",
        },
        RejectedKeyCase {
            case: "a pipe",
            key: "a|b",
        },
        RejectedKeyCase {
            case: "a tilde",
            key: "a~b",
        },
        RejectedKeyCase {
            case: "a tab",
            key: "a\tb",
        },
        RejectedKeyCase {
            case: "a character outside ASCII",
            key: "naïve",
        },
    ]
}

/// Keys the modelled alphabet DOES admit, beside the ones the suite's other
/// tables already carry: the three characters the pattern's `+` to `=` range
/// covers without the class spelling them out (`,`, `;`, `<`), and one key
/// carrying every punctuation character of that alphabet at once.
const KEYS_INSIDE_THE_MODELLED_ALPHABET: &[&str] =
    &["a,b", "a;b", "a<b", "k+,-./0:;<=Z_", "Env9"];

/// A tag key the modelled `TagKey` alphabet does not admit is refused 400 by
/// every operation that takes tag keys, and the profile's tag map is left exactly
/// as it was — while a key made only of characters that alphabet admits round
/// trips through all three.
#[tokio::test]
async fn a_tag_key_outside_the_modelled_alphabet_is_refused_by_every_tag_operation() {
    let router = emulator();
    let seeded = tags(&[("env", "prod"), ("team", "iot")]);
    let arn = seed_profile(&router, "Alphabet_Keys").await;
    assert_eq!(
        tag_resource(&router, &arn, &seeded).await.status,
        NO_CONTENT_STATUS,
        "seeding the profile's tags must succeed"
    );

    for (index, row) in keys_outside_the_modelled_alphabet()
        .into_iter()
        .enumerate()
    {
        let case = row.case;
        let key = row.key;

        // The write half: refused, and nothing added.
        let write = tag_resource(&router, &arn, &tags(&[(key, "written")])).await;
        assert_eq!(
            write.status, 400,
            "case {case:?}: expected TagResource to refuse key {key:?} with 400, got {} {:?}",
            write.status, write.body
        );
        assert_eq!(
            write.code(),
            Some(ERROR_CODE_BAD_REQUEST),
            "case {case:?}: expected the modelled code for key {key:?}, got {:?}",
            write.body
        );
        assert_eq!(
            write.error_type.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected the modelled error shape for key {key:?}, got {:?}",
            write.error_type
        );
        assert_eq!(
            list_tags(&router, &arn).await.tags().as_ref(),
            Some(&seeded),
            "case {case:?}: a refused write must store nothing, got {:?}",
            list_tags(&router, &arn).await.tags()
        );

        // The removal half, over the same key: refused, and every seeded key is
        // still there.
        let removal = untag_resource(&router, &arn, &[key]).await;
        assert_eq!(
            removal.status, 400,
            "case {case:?}: expected UntagResource to refuse key {key:?} with 400, got {} {:?}",
            removal.status, removal.body
        );
        assert_eq!(
            removal.code(),
            Some(ERROR_CODE_BAD_REQUEST),
            "case {case:?}: expected the modelled code for key {key:?}, got {:?}",
            removal.body
        );
        assert_eq!(
            list_tags(&router, &arn).await.tags().as_ref(),
            Some(&seeded),
            "case {case:?}: a refused removal must remove nothing, got {:?}",
            list_tags(&router, &arn).await.tags()
        );

        // The create half: the same key on `PutSigningProfile`, whose modelled
        // 400 is `ValidationException` — and no profile is stored for it.
        let created = format!("Alphabet_Create_{index}");
        let create = put_profile(
            &router,
            &created,
            json!({
                "platformId": "AWSLambda-SHA384-ECDSA",
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                "tags": tags_json(key, "created"),
            }),
        )
        .await;
        assert_eq!(
            create.status, 400,
            "case {case:?}: expected PutSigningProfile to refuse key {key:?} with 400, got {} {:?}",
            create.status, create.body
        );
        assert_eq!(
            create.code(),
            Some(ERROR_CODE_VALIDATION),
            "case {case:?}: expected the modelled code for key {key:?}, got {:?}",
            create.body
        );
        let read_back = get_profile(&router, &created).await;
        assert_eq!(
            read_back.status, 404,
            "case {case:?}: a refused create must store no profile, got {} {:?}",
            read_back.status, read_back.body
        );
    }

    // The other direction: a key the alphabet admits is written, reported and
    // removed by the same three operations.
    for key in KEYS_INSIDE_THE_MODELLED_ALPHABET {
        let write = tag_resource(&router, &arn, &tags(&[(key, "written")])).await;
        assert_eq!(
            write.status, NO_CONTENT_STATUS,
            "case {key:?}: expected TagResource {NO_CONTENT_STATUS}, got {} {:?}",
            write.status, write.body
        );
        assert_eq!(
            list_tags(&router, &arn)
                .await
                .tags()
                .and_then(|map| map.get(*key).cloned()),
            Some("written".to_string()),
            "case {key:?}: expected the written value to be reported"
        );
        let removal = untag_resource(&router, &arn, &[key]).await;
        assert_eq!(
            removal.status, NO_CONTENT_STATUS,
            "case {key:?}: expected UntagResource {NO_CONTENT_STATUS}, got {} {:?}",
            removal.status, removal.body
        );
        assert!(
            list_tags(&router, &arn)
                .await
                .tags()
                .is_some_and(|map| !map.contains_key(*key)),
            "case {key:?}: expected the removed key to be gone"
        );
    }
}

/// One tag key judged by `TagKey`'s own `@length(min: 1, max: 128)`
/// (`smithy_query(query="shape", shape="TagKey")`): each published bound, and one
/// character past each of them.
struct KeyLengthCase {
    case: &'static str,
    key: String,
    admitted: bool,
}

fn key_length_cases() -> Vec<KeyLengthCase> {
    vec![
        KeyLengthCase {
            case: "the empty key, one character below the modelled minimum",
            key: String::new(),
            admitted: false,
        },
        KeyLengthCase {
            case: "a key at the modelled minimum length",
            key: "k".repeat(TAG_KEY_MIN_LENGTH),
            admitted: true,
        },
        KeyLengthCase {
            case: "a key at the modelled maximum length",
            key: "k".repeat(TAG_KEY_MAX_LENGTH),
            admitted: true,
        },
        KeyLengthCase {
            case: "a key one character past the modelled maximum",
            key: "k".repeat(TAG_KEY_MAX_LENGTH + 1),
            admitted: false,
        },
    ]
}

/// A tag key is admitted at both of `TagKey`'s published length bounds and
/// refused one character outside either of them, by both operations that write a
/// tag MAP — the map's key is the one value the framework's boundary leaves
/// entirely to the handler, while `UntagResource`'s `tagKeys` are list members the
/// boundary already judges against the same bounds.
#[tokio::test]
async fn a_tag_key_is_judged_against_the_modelled_length_bounds() {
    let router = emulator();
    let arn = seed_profile(&router, "Key_Lengths").await;

    for (index, row) in key_length_cases().into_iter().enumerate() {
        let case = row.case;
        let key = row.key.as_str();
        let written = tags(&[(key, "written")]);

        let write = tag_resource(&router, &arn, &written).await;
        let expected_status = if row.admitted {
            NO_CONTENT_STATUS
        } else {
            400
        };
        assert_eq!(
            write.status, expected_status,
            "case {case:?}: expected TagResource {expected_status} for a {}-character key, got {} \
             {:?}",
            row.key.len(),
            write.status,
            write.body
        );

        let created = format!("Key_Length_Create_{index}");
        let create = put_profile(
            &router,
            &created,
            json!({
                "platformId": "AWSLambda-SHA384-ECDSA",
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                "tags": tags_json(key, "created"),
            }),
        )
        .await;

        if row.admitted {
            assert_eq!(
                list_tags(&router, &arn)
                    .await
                    .tags()
                    .and_then(|map| map.get(key).cloned()),
                Some("written".to_string()),
                "case {case:?}: expected the written value to be reported"
            );
            assert_eq!(
                untag_resource(&router, &arn, &[key]).await.status,
                NO_CONTENT_STATUS,
                "case {case:?}: expected UntagResource to remove the admitted key"
            );
            assert_eq!(
                create.status, 200,
                "case {case:?}: expected PutSigningProfile 200, got {} {:?}",
                create.status, create.body
            );
            assert_eq!(
                get_profile(&router, &created)
                    .await
                    .tags()
                    .and_then(|map| map.get(key).cloned()),
                Some("created".to_string()),
                "case {case:?}: expected the created tag to be reported by GetSigningProfile"
            );
        } else {
            assert_eq!(
                write.code(),
                Some(ERROR_CODE_BAD_REQUEST),
                "case {case:?}: expected the modelled code, got {:?}",
                write.body
            );
            assert!(
                list_tags(&router, &arn)
                    .await
                    .tags()
                    .is_some_and(|map| !map.contains_key(key)),
                "case {case:?}: a refused write must store nothing"
            );
            assert_eq!(
                create.status, 400,
                "case {case:?}: expected PutSigningProfile to refuse the key with 400, got {} {:?}",
                create.status, create.body
            );
            assert_eq!(
                create.code(),
                Some(ERROR_CODE_VALIDATION),
                "case {case:?}: expected the modelled code, got {:?}",
                create.body
            );
            assert_eq!(
                get_profile(&router, &created).await.status,
                404,
                "case {case:?}: a refused create must store no profile"
            );
        }
    }
}
