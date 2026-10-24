// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the resource-tagging surface: `TagResource`,
//! `UntagResource` and `ListTagsForResource`, all three bound to
//! `/tags/{resourceArn}`.
//!
//! RULE: the `resourceArn` path label is a medical-imaging resource ARN — the form
//! `com.amazonaws.medicalimaging#Arn` declares. A label that is not one makes the
//! request MALFORMED on every tagging route: HTTP 400 with the error code
//! `BadRequestException`, never the generic constraint-validation failure. A label
//! that IS one resolves to the resource whose ARN it is, so the tag map a write
//! leaves is exactly the map the read reports, and an ARN naming no held resource is
//! a 404.
//!
//! Every suite is a TABLE whose rows differ along what the rule generalises over:
//! which clause of the ARN form the label breaks, which route (HTTP method) carries
//! it, which resource the ARN names, and how many tags of which key/value forms the
//! request carries. Each case drives real HTTP through the production front door, so
//! what is asserted is the wire behaviour a `medical-imaging` SDK client sees.

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    DATASTORE_DELETION_DELAY, MAX_TAGS_PER_RESOURCE,
};

use crate::datastore_lifecycle::{Answer, Emulator, ACCOUNT, SETTLE_MULTIPLE};

/// A well-formed `[0-9a-z]{32}` resource id that no call ever mints (the ids
/// `mint_id` produces are SplitMix64 output, never all zeros).
fn unheld_id() -> String {
    "0".repeat(32)
}

/// The ARN of a data store this emulator does not hold, in the region and account
/// every emulated ARN carries.
fn unheld_datastore_arn() -> String {
    format!(
        "arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{}",
        unheld_id()
    )
}

/// Percent-encode `value` for a NON-GREEDY `@httpLabel`, the way an AWS SDK does:
/// every byte outside RFC 3986's unreserved set, so an ARN's own `:` and `/` are
/// carried as one path segment instead of being read as path structure.
///
/// `pub(crate)` with the three route helpers below: the sibling suite that drives the
/// same `/tags/{resourceArn}` routes uses these rather than spelling them a second time.
pub(crate) fn label(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::new();
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
            out.push(ch);
        } else {
            out.push('%');
            out.push(HEX[usize::from(byte >> 4)] as char);
            out.push(HEX[usize::from(byte & 0x0f)] as char);
        }
    }
    out
}

/// `POST /tags/{resourceArn}` — `TagResource`.
pub(crate) async fn tag(emulator: &Emulator, arn: &str, tags: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/tags/{}", label(arn)),
            Some(json!({ "tags": tags })),
        )
        .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`.
pub(crate) async fn untag(emulator: &Emulator, arn: &str, tag_keys: &[&str]) -> Answer {
    let query: Vec<String> = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key)))
        .collect();
    emulator
        .call(
            "DELETE",
            &format!("/tags/{}?{}", label(arn), query.join("&")),
            None,
        )
        .await
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`.
pub(crate) async fn list_tags(emulator: &Emulator, arn: &str) -> Answer {
    emulator
        .call("GET", &format!("/tags/{}", label(arn)), None)
        .await
}

/// The three routes `/tags/{resourceArn}` carries, one per method, as the model binds
/// them (`smithy_query(query="digest")`: POST TagResource, GET ListTagsForResource,
/// DELETE UntagResource).
const TAGGING_ROUTES: [&str; 3] = ["POST", "GET", "DELETE"];

/// One request on `route`, carrying the body or query that route requires. Used by
/// the tables whose rule is about the LABEL, which every route spells the same way.
async fn tagging_call(emulator: &Emulator, route: &str, arn: &str) -> Answer {
    match route {
        "POST" => tag(emulator, arn, json!({ "team": "radiology" })).await,
        "GET" => list_tags(emulator, arn).await,
        "DELETE" => untag(emulator, arn, &["team"]).await,
        other => panic!("unknown tagging route {other}"),
    }
}

/// A JSON object from key/value pairs, for comparing a reported tag map whole.
///
/// `pub(crate)`, like the route helpers above: the sibling tag-validation suite builds
/// its request bodies and expectations the same way.
pub(crate) fn tag_object(pairs: &[(String, String)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// A malformed resourceArn is a malformed REQUEST
// ---------------------------------------------------------------------------

/// One malformed-label row: which clause of the ARN form it breaks, and the label.
#[derive(Debug)]
struct MalformedCase {
    case: &'static str,
    arn: String,
}

/// The rows, one per clause of
/// `^arn:aws((-us-gov)|(-iso)|(-iso-b)|(-cn))?:medical-imaging:[a-z0-9-]+:[0-9]{12}:datastore/[0-9a-z]{32}(/imageset/[0-9a-z]{32})?$`
/// — the `@pattern` of `com.amazonaws.medicalimaging#Arn`
/// (`smithy_query(query="shape",shape="Arn")`). The issue cited only the first.
fn malformed_cases() -> Vec<MalformedCase> {
    let id = unheld_id();
    vec![
        MalformedCase {
            case: "not an ARN at all",
            arn: "not-a-valid-arn".to_string(),
        },
        MalformedCase {
            case: "the empty label",
            arn: " ".to_string(),
        },
        MalformedCase {
            case: "an ARN that stops before the resource",
            arn: format!("arn:aws:medical-imaging:us-east-1:{ACCOUNT}"),
        },
        MalformedCase {
            case: "unknown partition",
            arn: format!("arn:aws-fake:medical-imaging:us-east-1:{ACCOUNT}:datastore/{id}"),
        },
        MalformedCase {
            case: "another service's namespace",
            arn: format!("arn:aws:medicalimaging:us-east-1:{ACCOUNT}:datastore/{id}"),
        },
        MalformedCase {
            case: "region outside the [a-z0-9-] alphabet",
            arn: format!("arn:aws:medical-imaging:US-EAST-1:{ACCOUNT}:datastore/{id}"),
        },
        MalformedCase {
            case: "account id shorter than 12 digits",
            arn: format!("arn:aws:medical-imaging:us-east-1:12345:datastore/{id}"),
        },
        MalformedCase {
            case: "another resource type",
            arn: format!("arn:aws:medical-imaging:us-east-1:{ACCOUNT}:imageset/{id}"),
        },
        MalformedCase {
            case: "data store id shorter than 32 characters",
            arn: format!("arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/0a1b2c"),
        },
        MalformedCase {
            case: "data store id outside the [0-9a-z] alphabet",
            arn: format!(
                "arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{}",
                "A".repeat(32)
            ),
        },
        MalformedCase {
            case: "image set id shorter than 32 characters",
            arn: format!(
                "arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{id}/imageset/0a1b2c"
            ),
        },
        MalformedCase {
            case: "a segment past the image set",
            arn: format!(
                "arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{id}/imageset/{id}/frame/{id}"
            ),
        },
    ]
}

/// RULE: a `resourceArn` label that is not a medical-imaging resource ARN is refused
/// with HTTP 400 and the error code `BadRequestException` — the shape this service
/// models for "The request is invalid or malformed." — on every tagging route, and
/// NOT with the constraint-validation code a member that failed a rule would get.
#[tokio::test]
async fn a_malformed_resource_arn_is_a_bad_request_on_every_tagging_route() {
    let emulator = Emulator::new();
    let rows = malformed_cases();

    for route in TAGGING_ROUTES {
        for row in &rows {
            let answer = tagging_call(&emulator, route, &row.arn).await;
            assert_eq!(
                answer.status, 400,
                "{route} {}: expected HTTP 400, got {answer:?}",
                row.case
            );
            assert_eq!(
                answer.error_code, "BadRequestException",
                "{route} {}: expected error code BadRequestException, got {answer:?}",
                row.case
            );
            let message = answer.string(&["message"]);
            assert!(
                !message.is_empty(),
                "{route} {}: expected a message saying the resource ARN is invalid, got {answer:?}",
                row.case
            );
            // The defect this suite pins: the refusal was worded as a member that
            // failed a modelled constraint, which is a different claim and a
            // different error code.
            assert!(
                !message.contains("failed to satisfy constraint"),
                "{route} {}: expected a malformed-ARN message, got the constraint clause {message:?}",
                row.case
            );
        }
    }
}

/// RULE (the other side of the same rule): a refusal that is NOT a malformed resource
/// ARN keeps the modelled `ValidationException` — and when one request is both, the
/// ARN decides, because the service parses the label before it validates members.
#[tokio::test]
async fn a_constraint_failure_that_is_not_the_arn_stays_a_validation_exception() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("tag.validation", "token-tag-validation").await;
    let arn = emulator
        .get_datastore(&datastore_id)
        .await
        .string(&["datastoreProperties", "datastoreArn"]);

    // `UntagResource.tagKeys` targets `TagKeyList`, len 0..200
    // (`smithy_query(query="shape",shape="TagKeyList")`); 201 keys is one past the
    // published upper bound, and nothing about it is an ARN.
    let too_many: Vec<String> = (0..201).map(|n| format!("key{n}")).collect();
    let too_many: Vec<&str> = too_many.iter().map(String::as_str).collect();
    let refused = untag(&emulator, &arn, &too_many).await;
    assert_eq!(
        refused.status, 400,
        "201 tagKeys, one past the bound: expected HTTP 400, got {refused:?}"
    );
    assert_eq!(
        refused.error_code, "ValidationException",
        "201 tagKeys, one past the bound: expected error code ValidationException, got {refused:?}"
    );

    // The same request with a malformed label: the ARN is parsed first, so it decides
    // the code even though the request also breaks a member's bound.
    let both = untag(&emulator, "not-a-valid-arn", &too_many).await;
    assert_eq!(
        both.status, 400,
        "201 tagKeys on a malformed ARN: expected HTTP 400, got {both:?}"
    );
    assert_eq!(
        both.error_code, "BadRequestException",
        "201 tagKeys on a malformed ARN: expected error code BadRequestException, got {both:?}"
    );

    // Neither refusal touched the resource.
    let listed = list_tags(&emulator, &arn).await;
    assert_eq!(
        listed.field(&["tags"]),
        &json!({}),
        "after two refusals: expected no tags, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// A well-formed ARN naming no held resource
// ---------------------------------------------------------------------------

/// RULE: a well-formed resource ARN that names no resource this emulator holds is a
/// 404 `ResourceNotFoundException` on every tagging route — including an ARN that
/// differs from a held resource's own only in partition, region or account, and the
/// ARN of a data store whose deletion has fallen due.
#[tokio::test]
async fn a_resource_arn_naming_nothing_held_is_not_found_on_every_tagging_route() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("tag.absent", "token-tag-absent").await;
    let arn = emulator
        .get_datastore(&datastore_id)
        .await
        .string(&["datastoreProperties", "datastoreArn"]);
    assert_eq!(
        arn,
        format!("arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{datastore_id}"),
        "the data store's reported ARN is what a tagging call names it by"
    );

    let doomed_id = emulator.active_datastore("tag.doomed", "token-tag-doomed").await;
    let doomed_arn = emulator
        .get_datastore(&doomed_id)
        .await
        .string(&["datastoreProperties", "datastoreArn"]);
    let deleted = emulator.delete_datastore(&doomed_id).await;
    assert_eq!(
        deleted.status, 200,
        "delete {doomed_id}: expected 200, got {deleted:?}"
    );
    tokio::time::sleep(SETTLE_MULTIPLE * DATASTORE_DELETION_DELAY).await;

    let rows: Vec<(&str, String)> = vec![
        ("a data store id nothing minted", unheld_datastore_arn()),
        (
            "an image set under a held data store (no image set exists yet)",
            format!("{arn}/imageset/{}", unheld_id()),
        ),
        (
            "the held data store's ARN in another partition",
            arn.replacen("arn:aws:", "arn:aws-cn:", 1),
        ),
        (
            "the held data store's ARN in another region",
            arn.replacen("us-east-1", "us-west-2", 1),
        ),
        (
            "the held data store's ARN in another account",
            arn.replacen(ACCOUNT, "123456789012", 1),
        ),
        ("a data store whose deletion fell due", doomed_arn),
    ];

    for route in TAGGING_ROUTES {
        for (case, absent_arn) in &rows {
            let answer = tagging_call(&emulator, route, absent_arn).await;
            assert_eq!(
                answer.status, 404,
                "{route} {case}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "{route} {case}: expected error code ResourceNotFoundException, got {answer:?}"
            );
        }
    }

    // The held data store is untouched by every refusal above.
    let listed = list_tags(&emulator, &arn).await;
    assert_eq!(
        listed.status, 200,
        "the held data store still lists its tags: got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// The round trip: write, read back, remove
// ---------------------------------------------------------------------------

/// One round-trip row: the tags written, then the keys removed.
#[derive(Debug)]
struct RoundTripCase {
    case: &'static str,
    tags: Vec<(String, String)>,
    remove: Vec<String>,
}

/// Rows spanning the forms a tag can take, with the bounds taken from
/// `smithy_query(query="shape",shape="TagMap")`: a key is `TagKey`, len 1..128, and a
/// value is `TagValue`, len 0..256.
fn round_trip_cases() -> Vec<RoundTripCase> {
    vec![
        RoundTripCase {
            case: "one tag, removed again",
            tags: vec![("team".to_string(), "radiology".to_string())],
            remove: vec!["team".to_string()],
        },
        RoundTripCase {
            case: "three tags, one removed",
            tags: vec![
                ("team".to_string(), "radiology".to_string()),
                ("env".to_string(), "staging".to_string()),
                ("owner".to_string(), "imaging-ops".to_string()),
            ],
            remove: vec!["env".to_string()],
        },
        RoundTripCase {
            case: "three tags, all removed at once",
            tags: vec![
                ("team".to_string(), "radiology".to_string()),
                ("env".to_string(), "staging".to_string()),
                ("owner".to_string(), "imaging-ops".to_string()),
            ],
            remove: vec![
                "team".to_string(),
                "env".to_string(),
                "owner".to_string(),
            ],
        },
        RoundTripCase {
            case: "key at the 1-character lower length bound",
            tags: vec![("a".to_string(), "one".to_string())],
            remove: vec!["a".to_string()],
        },
        RoundTripCase {
            case: "key at the 128-character upper length bound",
            tags: vec![("k".repeat(128), "long-key".to_string())],
            remove: vec!["k".repeat(128)],
        },
        RoundTripCase {
            case: "value at the 0-character lower length bound",
            tags: vec![("empty".to_string(), String::new())],
            remove: vec!["empty".to_string()],
        },
        RoundTripCase {
            case: "value at the 256-character upper length bound",
            tags: vec![("long".to_string(), "v".repeat(256))],
            remove: vec!["long".to_string()],
        },
        RoundTripCase {
            case: "key using the punctuation the key alphabet allows",
            tags: vec![("ns:sub/part.v-1".to_string(), "punctuated".to_string())],
            remove: vec!["ns:sub/part.v-1".to_string()],
        },
        RoundTripCase {
            case: "value carrying spaces and non-ASCII text",
            tags: vec![("room".to_string(), "x-ray \u{5ba4} 2".to_string())],
            remove: vec!["room".to_string()],
        },
        RoundTripCase {
            case: "removing a key the resource never carried",
            tags: vec![("team".to_string(), "radiology".to_string())],
            remove: vec!["nobody-set-this".to_string()],
        },
    ]
}

/// RULE: `TagResource` persists the tags it is given ON the resource the ARN names,
/// `ListTagsForResource` reports exactly that map, and `UntagResource` removes exactly
/// the keys it is given — removing one it does not hold is a success that changes
/// nothing.
#[tokio::test]
async fn tags_written_through_tag_resource_read_back_and_can_be_removed() {
    let emulator = Emulator::new();

    for (index, row) in round_trip_cases().into_iter().enumerate() {
        let datastore_id = emulator
            .active_datastore(&format!("tag.round.{index}"), &format!("token-round-{index}"))
            .await;
        let arn = emulator
            .get_datastore(&datastore_id)
            .await
            .string(&["datastoreProperties", "datastoreArn"]);

        let before = list_tags(&emulator, &arn).await;
        assert_eq!(
            before.field(&["tags"]),
            &json!({}),
            "{}: a data store created with no tags reports an empty map, got {before:?}",
            row.case
        );

        let written = tag(&emulator, &arn, tag_object(&row.tags)).await;
        assert_eq!(
            written.status, 200,
            "{}: expected TagResource to answer 200, got {written:?}",
            row.case
        );

        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.status, 200,
            "{}: expected ListTagsForResource to answer 200, got {listed:?}",
            row.case
        );
        assert_eq!(
            listed.field(&["tags"]),
            &tag_object(&row.tags),
            "{}: expected the tags that were written, got {listed:?}",
            row.case
        );

        // A read must not change what the next reader sees.
        let again = list_tags(&emulator, &arn).await;
        assert_eq!(
            again.field(&["tags"]),
            listed.field(&["tags"]),
            "{}: a second read reports the same map, got {again:?}",
            row.case
        );

        let remove: Vec<&str> = row.remove.iter().map(String::as_str).collect();
        let removed = untag(&emulator, &arn, &remove).await;
        assert_eq!(
            removed.status, 200,
            "{}: expected UntagResource to answer 200, got {removed:?}",
            row.case
        );

        let expected: Vec<(String, String)> = row
            .tags
            .iter()
            .filter(|(key, _)| !row.remove.contains(key))
            .cloned()
            .collect();
        let after = list_tags(&emulator, &arn).await;
        assert_eq!(
            after.field(&["tags"]),
            &tag_object(&expected),
            "{}: expected the written tags minus the removed keys, got {after:?}",
            row.case
        );

        // Untagging is idempotent: the same removal again leaves the same map.
        let repeat = untag(&emulator, &arn, &remove).await;
        assert_eq!(
            repeat.status, 200,
            "{}: expected the repeated UntagResource to answer 200, got {repeat:?}",
            row.case
        );
        let settled = list_tags(&emulator, &arn).await;
        assert_eq!(
            settled.field(&["tags"]),
            &tag_object(&expected),
            "{}: expected the repeated removal to change nothing, got {settled:?}",
            row.case
        );
    }
}

/// RULE: the tag map the tagging operations read and write is the resource's OWN — so
/// tags supplied at `CreateDatastore` are what `ListTagsForResource` reports, and a
/// `TagResource` on a key already present replaces its value rather than adding a
/// second entry.
#[tokio::test]
async fn tagging_reads_and_writes_the_resources_own_tag_map() {
    let emulator = Emulator::new();

    // Rows: how many tags the create carried.
    let creates: [(&str, Value); 3] = [
        ("no tags at create", json!({})),
        ("one tag at create", json!({ "team": "radiology" })),
        (
            "three tags at create",
            json!({ "team": "radiology", "env": "prod", "owner": "imaging-ops" }),
        ),
    ];

    for (index, (case, tags)) in creates.into_iter().enumerate() {
        let created = emulator
            .create_datastore(json!({
                "datastoreName": format!("tag.create.{index}"),
                "clientToken": format!("token-create-{index}"),
                "tags": tags.clone(),
            }))
            .await;
        assert_eq!(created.status, 200, "{case}: expected 200, got {created:?}");
        let datastore_id = created.string(&["datastoreId"]);
        let arn = format!("arn:aws:medical-imaging:us-east-1:{ACCOUNT}:datastore/{datastore_id}");

        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.field(&["tags"]),
            &tags,
            "{case}: expected the tags the create carried, got {listed:?}"
        );

        // An upsert, not an insert: the same key takes the new value.
        let overwritten = tag(&emulator, &arn, json!({ "team": "cardiology" })).await;
        assert_eq!(
            overwritten.status, 200,
            "{case}: expected TagResource to answer 200, got {overwritten:?}"
        );
        let after = list_tags(&emulator, &arn).await;
        assert_eq!(
            after.string(&["tags", "team"]),
            "cardiology",
            "{case}: expected the re-tagged key to carry the new value, got {after:?}"
        );
        let expected_len = match tags.as_object() {
            Some(map) if map.contains_key("team") => map.len(),
            Some(map) => map.len() + 1,
            None => 1,
        };
        assert_eq!(
            after.field(&["tags"]).as_object().map(serde_json::Map::len),
            Some(expected_len),
            "{case}: expected {expected_len} tags after the upsert, got {after:?}"
        );
    }
}

/// RULE: the tag map one resource holds is BOUNDED, because every entry is memory a
/// caller chose — at the bound a tag still lands, past it the call is refused with the
/// operation's own `ValidationException` and the map is unchanged.
#[tokio::test]
async fn a_resources_tag_map_is_bounded() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("tag.bound", "token-tag-bound").await;
    let arn = emulator
        .get_datastore(&datastore_id)
        .await
        .string(&["datastoreProperties", "datastoreArn"]);

    let at_bound: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|n| (format!("key{n}"), format!("value{n}")))
        .collect();
    let filled = tag(&emulator, &arn, tag_object(&at_bound)).await;
    assert_eq!(
        filled.status, 200,
        "{MAX_TAGS_PER_RESOURCE} tags, the bound itself: expected 200, got {filled:?}"
    );

    // Re-tagging a key already held adds nothing, so it is never refused.
    let rewritten = tag(&emulator, &arn, json!({ "key0": "rewritten" })).await;
    assert_eq!(
        rewritten.status, 200,
        "re-tagging a held key at the bound: expected 200, got {rewritten:?}"
    );

    let past_bound = tag(&emulator, &arn, json!({ "one-too-many": "v" })).await;
    assert_eq!(
        past_bound.status, 400,
        "one tag past the bound: expected HTTP 400, got {past_bound:?}"
    );
    assert_eq!(
        past_bound.error_code, "ValidationException",
        "one tag past the bound: expected error code ValidationException, got {past_bound:?}"
    );

    let listed = list_tags(&emulator, &arn).await;
    assert_eq!(
        listed
            .field(&["tags"])
            .as_object()
            .map(serde_json::Map::len),
        Some(MAX_TAGS_PER_RESOURCE),
        "after the refusal: expected the map to still hold {MAX_TAGS_PER_RESOURCE} tags, got {listed:?}"
    );
    assert_eq!(
        listed.string(&["tags", "key0"]),
        "rewritten",
        "after the refusal: expected the rewritten value to survive, got {listed:?}"
    );

    // Freeing one entry makes room again.
    let freed = untag(&emulator, &arn, &["key0"]).await;
    assert_eq!(freed.status, 200, "freeing one entry: got {freed:?}");
    let accepted = tag(&emulator, &arn, json!({ "one-too-many": "v" })).await;
    assert_eq!(
        accepted.status, 200,
        "the same tag once there is room: expected 200, got {accepted:?}"
    );
}
