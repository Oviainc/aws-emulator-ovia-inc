// SPDX-License-Identifier: CC0-1.0

//! The generic tagging surface of this service, end to end over HTTP.
//!
//! The RULE pinned here: a resource's tags are ARN-keyed state the three generic tagging
//! routes share. `TagResource` MERGES the pairs it carries into what the resource already
//! has (a repeated key is re-valued, an untouched one kept), `UntagResource` removes
//! exactly the keys it names and nothing else, and `ListTagsForResource` reports back the
//! tags those two and the CREATE left — an empty list for a resource carrying none. A
//! tagging call keyed by a resource of this service that does not exist is refused with
//! the modelled `ResourceNotFoundException` and writes nothing, and one that would take a
//! resource past the modelled maximum number of tags per resource with the modelled
//! `TooManyTagsException`.
//!
//! The tables span what the rule generalises over: how many pairs one call carries (one,
//! several, the modelled maximum, one past it), the form of a key and a value (short, an
//! EMPTY value, at the modelled length bounds), which create minted the resource
//! (`CreateMeeting` and `CreateMeetingWithAttendees`), how many keys an untag names (one,
//! several, one the resource never carried), and the order the calls arrive in.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::state::MAX_TAGS_PER_RESOURCE;
use service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `chime` signing name
/// this service publishes, as the sibling suites in this binary state it.
const SIGNED_FOR_CHIME: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTCHIME/20260101/us-east-1/chime/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The emulator under test: ONE shared engine with this service mounted on it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://meetings-chime.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "meetings-chime.us-east-1.amazonaws.com")
            .header("authorization", SIGNED_FOR_CHIME)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, body)
    }

    /// Create a meeting and return its `(MeetingId, MeetingArn)`.
    async fn meeting(
        &self,
        token: &str,
        external_id: &str,
        tags: &[(String, String)],
    ) -> (String, String) {
        let mut request = json!({
            "ClientRequestToken": token,
            "MediaRegion": "us-east-1",
            "ExternalMeetingId": external_id,
        });
        if !tags.is_empty() {
            request["Tags"] = wire_tags(tags);
        }
        let (status, body) = self.call("POST", "/meetings", Some(&request)).await;
        assert_eq!(
            status, 200,
            "CreateMeeting({external_id}) must answer 200, got {status} {body:?}"
        );
        ids_of(&body["Meeting"])
    }

    /// Create a meeting through `CreateMeetingWithAttendees` and return the same pair, so
    /// the rule is pinned over BOTH creates that may carry tags.
    async fn meeting_with_attendees(
        &self,
        token: &str,
        external_id: &str,
        tags: &[(String, String)],
    ) -> (String, String) {
        let mut request = json!({
            "ClientRequestToken": token,
            "MediaRegion": "us-east-1",
            "ExternalMeetingId": external_id,
            "Attendees": [{ "ExternalUserId": "user-1" }],
        });
        if !tags.is_empty() {
            request["Tags"] = wire_tags(tags);
        }
        let (status, body) = self
            .call("POST", "/meetings?operation=create-attendees", Some(&request))
            .await;
        assert_eq!(
            status, 200,
            "CreateMeetingWithAttendees({external_id}) must answer 200, got {status} {body:?}"
        );
        ids_of(&body["Meeting"])
    }

    async fn tag(&self, arn: &str, pairs: &[(String, String)]) -> (u16, Value) {
        self.call(
            "POST",
            "/tags?operation=tag-resource",
            Some(&json!({ "ResourceARN": arn, "Tags": wire_tags(pairs) })),
        )
        .await
    }

    async fn untag(&self, arn: &str, tag_keys: &[String]) -> (u16, Value) {
        self.call(
            "POST",
            "/tags?operation=untag-resource",
            Some(&json!({ "ResourceARN": arn, "TagKeys": tag_keys })),
        )
        .await
    }

    async fn list_raw(&self, arn: &str) -> (u16, Value) {
        self.call("GET", &format!("/tags?arn={}", encode(arn)), None)
            .await
    }

    async fn list(&self, arn: &str) -> (u16, BTreeMap<String, String>) {
        let (status, body) = self.list_raw(arn).await;
        (status, tag_map(&body))
    }
}

/// The `(MeetingId, MeetingArn)` of a created meeting.
fn ids_of(meeting: &Value) -> (String, String) {
    let id = meeting["MeetingId"]
        .as_str()
        .unwrap_or_else(|| panic!("no MeetingId in {meeting:?}"))
        .to_string();
    let arn = meeting["MeetingArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no MeetingArn in {meeting:?}"))
        .to_string();
    (id, arn)
}

/// Percent-encode a value for a query value, the way an AWS SDK encodes one.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn wire_tags(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

fn tag_map(body: &Value) -> BTreeMap<String, String> {
    body["Tags"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|tag| {
                    Some((
                        tag["Key"].as_str()?.to_string(),
                        tag["Value"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn as_map(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// One row of the round-trip table: the pairs each successive tagging call applies, the
/// keys untagged afterwards, and the tags the resource is left with.
struct TagCase {
    case: &'static str,
    calls: Vec<Vec<(String, String)>>,
    removed: Vec<String>,
    left: Vec<(String, String)>,
}

/// RULE: tagging merges, untagging removes only the keys it names, and the listing
/// reports exactly the tags the writes left.
#[tokio::test]
async fn tags_round_trip_through_the_shared_tagging_routes() {
    let emulator = Emulator::new();

    // The pairs below stay inside the model's own bounds: `TagKey` is len 1..128 and
    // `TagValue` len 0..256, both over the pattern this model declares
    // (`smithy_query(query="shape",shape="Tag")`), so an EMPTY value is a value a row may
    // carry.
    let long_key = "k".repeat(128);
    let long_value = "v".repeat(256);
    let at_bounds = owned(&[
        (long_key.as_str(), long_value.as_str()),
        ("a.b:c/d_e+f", ""),
    ]);

    let rows: Vec<TagCase> = vec![
        TagCase {
            case: "one pair, kept",
            calls: vec![owned(&[("env", "dev")])],
            removed: Vec::new(),
            left: owned(&[("env", "dev")]),
        },
        TagCase {
            case: "several pairs in one call, one of them removed again",
            calls: vec![owned(&[("env", "dev"), ("team", "core"), ("tier", "gold")])],
            removed: keys(&["team"]),
            left: owned(&[("env", "dev"), ("tier", "gold")]),
        },
        TagCase {
            case: "two calls merging, the repeated key re-valued",
            calls: vec![
                owned(&[("env", "dev"), ("team", "core")]),
                owned(&[("env", "prod"), ("tier", "gold")]),
            ],
            removed: Vec::new(),
            left: owned(&[("env", "prod"), ("team", "core"), ("tier", "gold")]),
        },
        TagCase {
            case: "a key the resource never carried is removed as a no-op",
            calls: vec![owned(&[("env", "dev")])],
            removed: keys(&["absent", "neither"]),
            left: owned(&[("env", "dev")]),
        },
        TagCase {
            case: "every key removed, leaving no tags at all",
            calls: vec![owned(&[("env", "dev"), ("team", "core")])],
            removed: keys(&["env", "team"]),
            left: Vec::new(),
        },
        TagCase {
            case: "keys and values at the modelled length bounds, and an empty value",
            calls: vec![at_bounds.clone()],
            removed: Vec::new(),
            left: at_bounds,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let TagCase {
            case,
            calls,
            removed,
            left,
        } = row;
        // Both creates that may carry tags, so the rule is pinned over the way the
        // resource was minted as well as over the tags.
        let (_, plain) = emulator
            .meeting(&format!("tok-tag-{index}"), &format!("ext-tag-{index}"), &[])
            .await;
        let (_, with_attendees) = emulator
            .meeting_with_attendees(
                &format!("tok-tag-{index}-a"),
                &format!("ext-tag-{index}-a"),
                &[],
            )
            .await;

        for arn in [plain.as_str(), with_attendees.as_str()] {
            let (status, tags) = emulator.list(arn).await;
            assert_eq!(
                (status, tags.len()),
                (200, 0),
                "case {case:?} ({arn}): an untagged resource reports no tags, got {tags:?}"
            );

            for pairs in &calls {
                let (status, body) = emulator.tag(arn, pairs).await;
                assert_eq!(
                    status, 204,
                    "case {case:?} ({arn}): expected TagResource to answer 204, got \
                     {status} {body:?}"
                );
            }
            if !removed.is_empty() {
                let (status, body) = emulator.untag(arn, &removed).await;
                assert_eq!(
                    status, 204,
                    "case {case:?} ({arn}): expected UntagResource to answer 204, got \
                     {status} {body:?}"
                );
            }

            let (status, tags) = emulator.list(arn).await;
            assert_eq!(
                (status, &tags),
                (200, &as_map(&left)),
                "case {case:?} ({arn}): expected exactly the tags the writes left"
            );

            // A read does not change what the next reader sees.
            let (status, again) = emulator.list(arn).await;
            assert_eq!(
                (status, &again),
                (200, &tags),
                "case {case:?} ({arn}): a second listing differed from the first"
            );
        }
    }
}

/// One row of the create-time table: the tags the create carries, what the later calls do
/// to them, and the tags the resource is left with.
struct CreateTagCase {
    case: &'static str,
    created_with: Vec<(String, String)>,
    then_tagged: Vec<(String, String)>,
    untagged: Vec<String>,
    left: Vec<(String, String)>,
}

/// RULE: tags given at CREATE time are the same state the tagging routes read and write,
/// so a created resource lists them, the tagging routes change them, and deleting the
/// resource takes them with it.
#[tokio::test]
async fn tags_supplied_at_create_time_are_listed_and_can_be_changed() {
    let emulator = Emulator::new();

    let rows: Vec<CreateTagCase> = vec![
        CreateTagCase {
            case: "one create-time tag, left alone",
            created_with: owned(&[("env", "dev")]),
            then_tagged: Vec::new(),
            untagged: Vec::new(),
            left: owned(&[("env", "dev")]),
        },
        CreateTagCase {
            case: "three create-time tags, one re-valued and one dropped",
            created_with: owned(&[("env", "dev"), ("team", "core"), ("tier", "gold")]),
            then_tagged: owned(&[("env", "prod")]),
            untagged: keys(&["tier"]),
            left: owned(&[("env", "prod"), ("team", "core")]),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let CreateTagCase {
            case,
            created_with,
            then_tagged,
            untagged,
            left,
        } = row;
        let (meeting_id, arn) = emulator
            .meeting(
                &format!("tok-create-tag-{index}"),
                &format!("ext-create-tag-{index}"),
                &created_with,
            )
            .await;
        let (_, with_attendees) = emulator
            .meeting_with_attendees(
                &format!("tok-create-tag-{index}-a"),
                &format!("ext-create-tag-{index}-a"),
                &created_with,
            )
            .await;

        for tagged in [arn.as_str(), with_attendees.as_str()] {
            let (status, listed) = emulator.list(tagged).await;
            assert_eq!(
                (status, &listed),
                (200, &as_map(&created_with)),
                "case {case:?} ({tagged}): expected the create-time tags to be listed"
            );

            if !then_tagged.is_empty() {
                emulator.tag(tagged, &then_tagged).await;
            }
            if !untagged.is_empty() {
                emulator.untag(tagged, &untagged).await;
            }

            let (status, listed) = emulator.list(tagged).await;
            assert_eq!(
                (status, &listed),
                (200, &as_map(&left)),
                "case {case:?} ({tagged}): expected the create-time tags as the later calls \
                 left them"
            );
        }

        // The resource's tags go with the resource: once the meeting is deleted, its ARN
        // is not a resource of this service any more and the listing refuses it.
        let (status, body) = emulator
            .call("DELETE", &format!("/meetings/{meeting_id}"), None)
            .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected DeleteMeeting to answer 204, got {status} {body:?}"
        );
        let (status, body) = emulator.list_raw(&arn).await;
        assert_eq!(
            (status, body["Code"].as_str()),
            (404, Some("NotFound")),
            "case {case:?}: expected a deleted resource's tags to be refused, got {body:?}"
        );
    }
}

/// RULE: a tagging call that names a resource of this service that does not exist is
/// refused with the modelled 404 and writes nothing, and a call past the modelled maximum
/// number of tags per resource is refused with the modelled too-many-tags error, leaving
/// the resource's tags as they were.
#[tokio::test]
async fn a_tagging_call_this_service_cannot_serve_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let (meeting_id, arn) = emulator.meeting("tok-refuse", "ext-refuse", &[]).await;

    // Rows: a meeting ARN this store holds no meeting for, the live meeting's id in
    // another Region (which names no meeting here, because a meeting's ARN carries the
    // Region it was created in), and an ARN outside this family's ARN namespace.
    let absent: Vec<(&str, String)> = vec![
        ("a meeting that never existed", format!("{arn}x")),
        (
            "the live meeting's id in another Region",
            arn.replace("us-east-1", "eu-west-1"),
        ),
        (
            "another vendor's resource ARN",
            format!("arn:aws:sqs:us-east-1:000000000000:queue/{meeting_id}"),
        ),
    ];

    for (case, absent_arn) in absent {
        let refusals = [
            (
                "TagResource",
                emulator.tag(&absent_arn, &owned(&[("k", "v")])).await,
            ),
            (
                "UntagResource",
                emulator.untag(&absent_arn, &keys(&["k"])).await,
            ),
            ("ListTagsForResource", emulator.list_raw(&absent_arn).await),
        ];
        for (operation, (status, payload)) in refusals {
            assert_eq!(
                (status, payload["Code"].as_str()),
                (404, Some("NotFound")),
                "case {case:?} ({operation}): expected 404 NotFound for {absent_arn}, got \
                 {status} {payload:?}"
            );
            assert_eq!(
                payload["ResourceName"].as_str(),
                Some(absent_arn.as_str()),
                "case {case:?} ({operation}): expected the refusal to name the resource it \
                 could not find, got {payload:?}"
            );
        }
    }

    // The tag count is bounded by the model's own `TagList` maximum: a call that would
    // take the resource past it is refused and writes nothing.
    let at_maximum: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("k{index:02}"), "v".to_string()))
        .collect();
    let (status, body) = emulator.tag(&arn, &at_maximum).await;
    assert_eq!(
        status, 204,
        "case the modelled maximum of tags: expected it accepted, got {status} {body:?}"
    );

    let (status, body) = emulator.tag(&arn, &owned(&[("one-too-many", "v")])).await;
    assert_eq!(
        (status, body["Code"].as_str()),
        (400, Some("TooManyTags")),
        "case one tag past the maximum: expected the modelled too-many-tags refusal, got \
         {status} {body:?}"
    );
    let (status, listed) = emulator.list(&arn).await;
    assert_eq!(
        (status, listed.len()),
        (200, at_maximum.len()),
        "case one tag past the maximum: expected the refused call to write nothing"
    );

    // Re-valuing a key the resource already carries adds none, so it is accepted even at
    // the maximum.
    let (status, body) = emulator.tag(&arn, &owned(&[("k00", "re-valued")])).await;
    assert_eq!(
        status, 204,
        "case re-valuing a key at the maximum: expected it accepted, got {status} {body:?}"
    );
    let (status, listed) = emulator.list(&arn).await;
    assert_eq!(
        (status, listed.get("k00").map(String::as_str)),
        (200, Some("re-valued")),
        "case re-valuing a key at the maximum: expected the new value, got {listed:?}"
    );
}
