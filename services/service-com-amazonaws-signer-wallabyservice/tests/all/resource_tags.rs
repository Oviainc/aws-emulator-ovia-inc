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
//! * a CANCELED profile's tags cannot be CHANGED (the modelled bad request "also
//!   occurs when you call a tagging API on a cancelled signing profile") while it
//!   stays readable, because Signer documents a canceled profile as still
//!   viewable.
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
    ERROR_CODE_BAD_REQUEST, ERROR_CODE_PROFILE_NOT_FOUND, MAX_TAGS_PER_PROFILE,
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
            written.status, 200,
            "case {label:?}: expected TagResource 200, got {} {:?}",
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
            first.status, 200,
            "case {label:?}: expected the first TagResource 200, got {} {:?}",
            first.status, first.body
        );
        if !case.second.is_empty() {
            let second = tag_resource(&router, &arn, &case.second).await;
            assert_eq!(
                second.status, 200,
                "case {label:?}: expected the second TagResource 200, got {} {:?}",
                second.status, second.body
            );
        }
        if !case.remove.is_empty() {
            let removed = untag_resource(&router, &arn, case.remove).await;
            assert_eq!(
                removed.status, 200,
                "case {label:?}: expected UntagResource 200, got {} {:?}",
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
        200,
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

/// A profile's STORED status decides which tagging calls it admits: a CANCELED
/// profile's tags cannot be changed (the modelled bad request names that status),
/// while it stays readable — and an ACTIVE or REVOKED profile's tags are writable,
/// because nothing documents a refusal for those.
#[tokio::test]
async fn a_tag_write_is_admitted_or_refused_by_the_profiles_status() {
    // One row per status a profile can be in when the tagging call arrives, plus
    // the row that reaches `Active` again through a re-put.
    let rows: [(&str, &str, u16); 4] = [
        ("Active", "Active_Tagging", 200),
        ("Revoked", "Revoked_Tagging", 200),
        ("Canceled", "Canceled_Tagging", 400),
        ("Canceled then re-put", "Reput_Tagging", 200),
    ];

    for (status_before, name, expected_status) in rows {
        let router = emulator();
        let arn = seed_profile(&router, name).await;
        let seeded = tags(&[("env", "prod")]);
        assert_eq!(
            tag_resource(&router, &arn, &seeded).await.status,
            200,
            "case {status_before:?}: the profile must be taggable while Active"
        );

        match status_before {
            "Revoked" => {
                let version = get_profile(&router, name).await.str_member("profileVersion");
                let revoked = revoke_profile(&router, name, &version).await;
                assert_eq!(
                    revoked.status, 204,
                    "case {status_before:?}: the revoke must succeed, got {:?}",
                    revoked.body
                );
            }
            "Canceled" | "Canceled then re-put" => {
                assert_eq!(
                    cancel_profile(&router, name).await.status,
                    204,
                    "case {status_before:?}: the cancel must succeed"
                );
            }
            _ => {}
        }
        if status_before == "Canceled then re-put" {
            // A put re-versions the profile and a re-versioned profile is
            // `Active`, so its tags are writable again: the gate follows the
            // STORED status, not the profile's history.
            seed_profile(&router, name).await;
        }

        let added = tags(&[("team", "iot")]);
        let write = tag_resource(&router, &arn, &added).await;
        assert_eq!(
            write.status, expected_status,
            "case {status_before:?}: expected TagResource HTTP {expected_status}, got {} {:?}",
            write.status, write.body
        );
        let removal = untag_resource(&router, &arn, &["env"]).await;
        assert_eq!(
            removal.status, expected_status,
            "case {status_before:?}: expected UntagResource HTTP {expected_status}, got {} {:?}",
            removal.status, removal.body
        );

        // A canceled profile is still VIEWABLE, so the read answers either way —
        // with the map the admitted writes left, or with the untouched one.
        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {status_before:?}: a profile stays readable in every status, got {} {:?}",
            listed.status, listed.body
        );
        let expected = if expected_status == 200 {
            // The write added a key and the removal took the seeded one.
            added
        } else {
            seeded
        };
        assert_eq!(
            listed.tags().as_ref(),
            Some(&expected),
            "case {status_before:?}: expected the tag map {expected:?}, got {:?}",
            listed.tags()
        );
        if expected_status != 200 {
            assert_eq!(
                write.code(),
                Some("ProfileCanceled"),
                "case {status_before:?}: expected the code to name the status that forbids the \
                 write, got {:?}",
                write.body
            );
            assert_eq!(
                write.error_type.as_deref(),
                Some("BadRequestException"),
                "case {status_before:?}: expected an x-amzn-errortype header, got {:?}",
                write.error_type
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
        200,
        "a reserved prefix in a tag VALUE is not refused"
    );
    assert_eq!(
        untag_resource(&router, &arn, &["owner"]).await.status,
        200,
        "clearing the profile before the count cases"
    );

    // Exactly the modelled maximum is admitted...
    let full: BTreeMap<String, String> = (0..MAX_TAGS_PER_PROFILE)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect();
    assert_eq!(
        tag_resource(&router, &arn, &full).await.status,
        200,
        "expected the modelled maximum of {MAX_TAGS_PER_PROFILE} tags to be admitted"
    );
    // ... re-tagging a key the profile already holds never grows the map, so it
    // is admitted at the maximum ...
    let retag = tags(&[("key0", "replaced")]);
    assert_eq!(
        tag_resource(&router, &arn, &retag).await.status,
        200,
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
