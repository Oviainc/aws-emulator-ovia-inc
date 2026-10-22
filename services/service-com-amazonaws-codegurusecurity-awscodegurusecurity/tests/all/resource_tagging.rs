// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the resource-tagging surface: `TagResource`, `UntagResource`
//! and `ListTagsForResource`, all three bound to `/tags/{resourceArn}`
//! (`smithy_query(query="digest")`: POST, DELETE and GET of that one URI).
//!
//! RULE 1: the `resourceArn` path label is a CodeGuru Security scan ARN — the form the
//! `ScanNameArn` shape's `@pattern` declares. A label that is not one makes the request
//! MALFORMED on every tagging route: HTTP 400 with the error code
//! `BadRequestException`, never the generic constraint-violation code a member that
//! failed a modelled rule gets. A refusal that is NOT the ARN's syntax keeps the
//! modelled `ValidationException`.
//!
//! RULE 2: a label that IS a scan ARN resolves to the scan whose ARN it is, so the tag
//! map a write leaves is exactly the map the read reports, removal takes exactly the
//! keys it names, and an ARN naming no scan of this account is a 404.
//!
//! Every suite is a TABLE whose rows differ along what the rule generalises over:
//! which clause of the ARN form the label breaks, which route (HTTP method) carries it,
//! which scan the ARN names, and how many tags of which key/value forms the request
//! carries. Each case drives real HTTP through the production front door
//! (`MultiServiceRouter::dispatch`), so what is asserted is the wire behaviour a
//! `codeguru-security` SDK client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::TAGS_PER_SCAN_QUOTA;

use crate::scan_lifecycle::{artifact_for, call, create_scan, expected_scan_arn, harness, Answer, REGION};

/// The canonical account id every emulated ARN carries (`engine::emulator_account_id`).
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// Percent-encode `value` for a NON-GREEDY `@httpLabel`, the way an AWS SDK does:
/// every byte outside RFC 3986's unreserved set, so an ARN's own `:` and `/` travel as
/// ONE path segment instead of being read as path structure.
fn label(value: &str) -> String {
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
async fn tag(router: &MultiServiceRouter, arn: &str, tags: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "tags": tags })),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`.
async fn untag(router: &MultiServiceRouter, arn: &str, tag_keys: &[&str]) -> Answer {
    let query: Vec<String> = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", label(key)))
        .collect();
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{}", label(arn), query.join("&")),
        None,
    )
    .await
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", label(arn)), None).await
}

/// The three routes `/tags/{resourceArn}` carries, one per method.
const TAGGING_ROUTES: [&str; 3] = ["POST", "GET", "DELETE"];

/// One request on `route`, carrying the body or query that route requires. Used by the
/// tables whose rule is about the LABEL, which every route spells the same way.
async fn tagging_call(router: &MultiServiceRouter, route: &str, arn: &str) -> Answer {
    match route {
        "POST" => tag(router, arn, json!({ "team": "appsec" })).await,
        "GET" => list_tags(router, arn).await,
        "DELETE" => untag(router, arn, &["team"]).await,
        other => panic!("unknown tagging route {other}"),
    }
}

/// A JSON object from key/value pairs, for comparing a reported tag map whole.
fn tag_object(pairs: &[(String, String)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// Create a scan named `scan_name` carrying `tags`, and answer with its `scanNameArn`.
async fn scan_with_tags(router: &MultiServiceRouter, scan_name: &str, tags: Value) -> String {
    let artifact = artifact_for(router, scan_name).await;
    let created = create_scan(
        router,
        json!({ "scanName": scan_name, "tags": tags }),
        &artifact,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateScan({scan_name}): expected 200, got {created:?}"
    );
    let arn = created.str_of("scanNameArn");
    assert_eq!(
        arn,
        expected_scan_arn(scan_name),
        "CreateScan({scan_name}): the ARN a tagging call names the scan by, got {created:?}"
    );
    arn
}

// ---------------------------------------------------------------------------
// RULE 1: a malformed resourceArn is a malformed REQUEST
// ---------------------------------------------------------------------------

/// One malformed-label row: which clause of the ARN form it breaks, and the label.
#[derive(Debug)]
struct MalformedCase {
    case: &'static str,
    arn: String,
}

/// The rows, one per clause of
/// `^arn:aws:codeguru-security:[\S]+:[\d]{12}:scans\/[a-zA-Z0-9-_$:.]*$` — the
/// `@pattern` of `com.amazonaws.codegurusecurity#ScanNameArn`
/// (`smithy_query(query="shape",shape="ScanNameArn")`). The issue cited only the first.
fn malformed_cases() -> Vec<MalformedCase> {
    let account = account();
    vec![
        MalformedCase {
            case: "not an ARN at all",
            arn: "not-a-valid-arn".to_string(),
        },
        MalformedCase {
            case: "a label of one space",
            arn: " ".to_string(),
        },
        MalformedCase {
            case: "an ARN that stops before the resource",
            arn: format!("arn:aws:codeguru-security:{REGION}:{account}"),
        },
        MalformedCase {
            case: "another partition",
            arn: format!("arn:aws-cn:codeguru-security:{REGION}:{account}:scans/demo"),
        },
        MalformedCase {
            case: "another service's namespace",
            arn: format!("arn:aws:codegurusecurity:{REGION}:{account}:scans/demo"),
        },
        MalformedCase {
            case: "an empty region segment",
            arn: format!("arn:aws:codeguru-security::{account}:scans/demo"),
        },
        MalformedCase {
            case: "whitespace inside the region segment",
            arn: format!("arn:aws:codeguru-security:us west 2:{account}:scans/demo"),
        },
        MalformedCase {
            case: "account id shorter than 12 digits",
            arn: format!("arn:aws:codeguru-security:{REGION}:12345:scans/demo"),
        },
        MalformedCase {
            case: "account id longer than 12 digits",
            arn: format!("arn:aws:codeguru-security:{REGION}:{account}0:scans/demo"),
        },
        MalformedCase {
            case: "account id that is not digits",
            arn: format!("arn:aws:codeguru-security:{REGION}:00000000000a:scans/demo"),
        },
        MalformedCase {
            case: "another resource type",
            arn: format!("arn:aws:codeguru-security:{REGION}:{account}:runs/demo"),
        },
        MalformedCase {
            case: "no separator before the scan name",
            arn: format!("arn:aws:codeguru-security:{REGION}:{account}:scans-demo"),
        },
        MalformedCase {
            case: "scan name outside the [a-zA-Z0-9-_$:.] alphabet",
            arn: format!("arn:aws:codeguru-security:{REGION}:{account}:scans/demo scan"),
        },
        MalformedCase {
            case: "a second path segment under the scan name",
            arn: format!("arn:aws:codeguru-security:{REGION}:{account}:scans/demo/run"),
        },
    ]
}

/// RULE 1: a `resourceArn` label that is not a scan ARN is refused with HTTP 400 and
/// the error code `BadRequestException` — the ARN parser's own answer, which runs
/// before the modelled member-constraint validator — on EVERY tagging route, and never
/// with the constraint-violation code and prose a member that failed a rule gets.
#[tokio::test]
async fn a_malformed_resource_arn_is_a_bad_request_on_every_tagging_route() {
    let (_engine, router) = harness();
    let rows = malformed_cases();

    for route in TAGGING_ROUTES {
        for row in &rows {
            let answer = tagging_call(&router, route, &row.arn).await;
            assert_eq!(
                answer.status, 400,
                "{route} {}: expected HTTP 400, got {answer:?}",
                row.case
            );
            assert_eq!(
                answer.error_type, "BadRequestException",
                "{route} {}: expected error code BadRequestException, got {answer:?}",
                row.case
            );
            let message = answer.str_of("message");
            assert!(
                !message.is_empty(),
                "{route} {}: expected a message saying the resource ARN is bad, got {answer:?}",
                row.case
            );
            // The defect this suite pins: the refusal was worded as a member that
            // failed a modelled constraint, which is a different claim and carries a
            // different error code.
            assert!(
                !message.contains("failed to satisfy constraint")
                    && !message.contains("validation error"),
                "{route} {}: expected a malformed-ARN message, got the constraint clause \
                 {message:?}",
                row.case
            );
        }
    }
}

/// RULE 1, the other side: a refusal that is NOT the ARN's syntax keeps the modelled
/// `ValidationException` — a member outside a declared bound, and even a `resourceArn`
/// that IS a well-formed ARN but longer than the shape's `@length` allows (a size
/// limit, not a syntax failure). When one request is both, the ARN decides, because the
/// label is parsed first.
#[tokio::test]
async fn a_refusal_that_is_not_the_arns_syntax_stays_a_validation_exception() {
    let (_engine, router) = harness();
    let arn = scan_with_tags(&router, "tag-validation", json!({})).await;

    // `UntagResource.tagKeys` targets `TagKeyList`, len 0..200
    // (`smithy_query(query="shape",shape="TagKeyList")`); 201 keys is one past the
    // published upper bound, and nothing about it is an ARN.
    let too_many: Vec<String> = (0..201).map(|n| format!("key{n}")).collect();
    let too_many: Vec<&str> = too_many.iter().map(String::as_str).collect();
    let refused = untag(&router, &arn, &too_many).await;
    assert_eq!(
        refused.status, 400,
        "201 tagKeys, one past the bound: expected HTTP 400, got {refused:?}"
    );
    assert_eq!(
        refused.error_type, "ValidationException",
        "201 tagKeys, one past the bound: expected error code ValidationException, got {refused:?}"
    );

    // `ScanNameArn` is len 1..300: this ARN satisfies the pattern (so it parses) and
    // is 301 characters, one past the published upper bound.
    let prefix = format!("arn:aws:codeguru-security:{REGION}:{}:scans/", account());
    let over_long = format!("{prefix}{}", "a".repeat(301 - prefix.len()));
    assert_eq!(
        over_long.len(),
        301,
        "the over-long ARN row must be exactly one character past the 300-character bound"
    );
    let long = list_tags(&router, &over_long).await;
    assert_eq!(
        long.status, 400,
        "a 301-character resourceArn: expected HTTP 400, got {long:?}"
    );
    assert_eq!(
        long.error_type, "ValidationException",
        "a 301-character resourceArn parses as an ARN and fails only the @length bound, so it \
         keeps the modelled ValidationException, got {long:?}"
    );

    // The same over-bound request with a malformed label: the ARN is parsed first, so
    // it decides the code even though the request also breaks a member's bound.
    let both = untag(&router, "not-a-valid-arn", &too_many).await;
    assert_eq!(
        both.status, 400,
        "201 tagKeys on a malformed ARN: expected HTTP 400, got {both:?}"
    );
    assert_eq!(
        both.error_type, "BadRequestException",
        "201 tagKeys on a malformed ARN: expected error code BadRequestException, got {both:?}"
    );

    // None of the refusals touched the scan.
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.get("tags"),
        &json!({}),
        "after three refusals: expected the scan to carry no tags, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: a well-formed ARN naming no scan of this account
// ---------------------------------------------------------------------------

/// RULE 2: a well-formed scan ARN that names no scan this emulator holds is a 404
/// `ResourceNotFoundException` on every tagging route — including an ARN that differs
/// from a held scan's own only in region or account.
#[tokio::test]
async fn a_scan_arn_naming_nothing_held_is_not_found_on_every_tagging_route() {
    let (_engine, router) = harness();
    let arn = scan_with_tags(&router, "tag-absent", json!({ "team": "appsec" })).await;
    let account = account();

    let rows: Vec<(&str, String)> = vec![
        (
            "a scan name nothing created",
            format!("arn:aws:codeguru-security:{REGION}:{account}:scans/never-created"),
        ),
        (
            "the empty scan name the pattern allows",
            format!("arn:aws:codeguru-security:{REGION}:{account}:scans/"),
        ),
        (
            "the held scan's ARN in another region",
            arn.replacen(REGION, "eu-central-1", 1),
        ),
        (
            "the held scan's ARN in another account",
            arn.replacen(account, "123456789012", 1),
        ),
        (
            "a scan name that only prefixes the held one",
            format!("arn:aws:codeguru-security:{REGION}:{account}:scans/tag-abs"),
        ),
    ];

    for route in TAGGING_ROUTES {
        for (case, absent_arn) in &rows {
            let answer = tagging_call(&router, route, absent_arn).await;
            assert_eq!(
                answer.status, 404,
                "{route} {case}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "ResourceNotFoundException",
                "{route} {case}: expected error code ResourceNotFoundException, got {answer:?}"
            );
        }
    }

    // The held scan is untouched by every refusal above.
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.status, 200,
        "{arn}: the held scan still lists its tags, got {listed:?}"
    );
    assert_eq!(
        listed.get("tags"),
        &json!({ "team": "appsec" }),
        "{arn}: expected the tags CreateScan carried, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: the round trip — write, read back, remove
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
            tags: vec![("team".to_string(), "appsec".to_string())],
            remove: vec!["team".to_string()],
        },
        RoundTripCase {
            case: "three tags, one removed",
            tags: vec![
                ("team".to_string(), "appsec".to_string()),
                ("env".to_string(), "staging".to_string()),
                ("owner".to_string(), "code-scanning".to_string()),
            ],
            remove: vec!["env".to_string()],
        },
        RoundTripCase {
            case: "three tags, all removed at once",
            tags: vec![
                ("team".to_string(), "appsec".to_string()),
                ("env".to_string(), "staging".to_string()),
                ("owner".to_string(), "code-scanning".to_string()),
            ],
            remove: vec![
                "team".to_string(),
                "env".to_string(),
                "owner".to_string(),
            ],
        },
        RoundTripCase {
            case: "one key removed out of two, the other left",
            tags: vec![
                ("team".to_string(), "appsec".to_string()),
                ("env".to_string(), "prod".to_string()),
            ],
            remove: vec!["team".to_string()],
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
            case: "key carrying the punctuation a tag key allows",
            tags: vec![("ns:sub/part.v-1".to_string(), "punctuated".to_string())],
            remove: vec!["ns:sub/part.v-1".to_string()],
        },
        RoundTripCase {
            case: "value carrying spaces and non-ASCII text",
            tags: vec![("room".to_string(), "code review 室 2".to_string())],
            remove: vec!["room".to_string()],
        },
        RoundTripCase {
            case: "removing a key the scan never carried",
            tags: vec![("team".to_string(), "appsec".to_string())],
            remove: vec!["nobody-set-this".to_string()],
        },
    ]
}

/// RULE 2: `TagResource` persists the tags it is given ON the scan the ARN names,
/// `ListTagsForResource` reports exactly that map, and `UntagResource` removes exactly
/// the keys it names — removing one the scan does not carry is a success that changes
/// nothing, and repeating a removal changes nothing either (`@idempotent`).
#[tokio::test]
async fn tags_written_through_tag_resource_read_back_and_can_be_removed() {
    let (_engine, router) = harness();

    for (index, row) in round_trip_cases().into_iter().enumerate() {
        let arn = scan_with_tags(&router, &format!("tag-round-{index}"), json!({})).await;

        let before = list_tags(&router, &arn).await;
        assert_eq!(
            before.status, 200,
            "{}: expected ListTagsForResource to answer 200, got {before:?}",
            row.case
        );
        assert_eq!(
            before.get("tags"),
            &json!({}),
            "{}: a scan created with no tags reports an empty map, got {before:?}",
            row.case
        );

        let written = tag(&router, &arn, tag_object(&row.tags)).await;
        assert_eq!(
            written.status, 204,
            "{}: TagResource's modelled success code is 204, got {written:?}",
            row.case
        );

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.get("tags"),
            &tag_object(&row.tags),
            "{}: expected the tags that were written, got {listed:?}",
            row.case
        );

        // A read must not change what the next reader sees.
        let again = list_tags(&router, &arn).await;
        assert_eq!(
            again.get("tags"),
            listed.get("tags"),
            "{}: a second read reports the same map, got {again:?}",
            row.case
        );

        let remove: Vec<&str> = row.remove.iter().map(String::as_str).collect();
        let removed = untag(&router, &arn, &remove).await;
        assert_eq!(
            removed.status, 204,
            "{}: UntagResource's modelled success code is 204, got {removed:?}",
            row.case
        );

        let expected: Vec<(String, String)> = row
            .tags
            .iter()
            .filter(|(key, _)| !row.remove.contains(key))
            .cloned()
            .collect();
        let after = list_tags(&router, &arn).await;
        assert_eq!(
            after.get("tags"),
            &tag_object(&expected),
            "{}: expected the written tags minus the removed keys, got {after:?}",
            row.case
        );

        let repeat = untag(&router, &arn, &remove).await;
        assert_eq!(
            repeat.status, 204,
            "{}: expected the repeated UntagResource to answer 204, got {repeat:?}",
            row.case
        );
        let settled = list_tags(&router, &arn).await;
        assert_eq!(
            settled.get("tags"),
            &tag_object(&expected),
            "{}: expected the repeated removal to change nothing, got {settled:?}",
            row.case
        );
    }
}

/// RULE 2: the tag map the tagging operations read and write is the SCAN's own — so the
/// tags `CreateScan` carried are what `ListTagsForResource` reports, and a `TagResource`
/// on a key already held replaces its value rather than adding a second entry.
#[tokio::test]
async fn tagging_reads_and_writes_the_scans_own_tag_map() {
    let (_engine, router) = harness();

    // Rows: how many tags the create carried.
    let creates: [(&str, Value); 3] = [
        ("no tags at create", json!({})),
        ("one tag at create", json!({ "team": "appsec" })),
        (
            "three tags at create",
            json!({ "team": "appsec", "env": "prod", "owner": "code-scanning" }),
        ),
    ];

    for (index, (case, tags)) in creates.into_iter().enumerate() {
        let arn = scan_with_tags(&router, &format!("tag-create-{index}"), tags.clone()).await;

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.get("tags"),
            &tags,
            "{case}: expected the tags CreateScan carried, got {listed:?}"
        );

        // An upsert, not an insert: the same key takes the new value.
        let overwritten = tag(&router, &arn, json!({ "team": "platform" })).await;
        assert_eq!(
            overwritten.status, 204,
            "{case}: expected TagResource to answer 204, got {overwritten:?}"
        );
        let after = list_tags(&router, &arn).await;
        assert_eq!(
            after.get("tags")["team"].as_str(),
            Some("platform"),
            "{case}: expected the re-tagged key to carry the new value, got {after:?}"
        );
        let expected_len = match tags.as_object() {
            Some(map) if map.contains_key("team") => map.len(),
            Some(map) => map.len() + 1,
            None => 1,
        };
        assert_eq!(
            after.get("tags").as_object().map(serde_json::Map::len),
            Some(expected_len),
            "{case}: expected {expected_len} tags after the upsert, got {after:?}"
        );
    }
}

/// RULE 2: the tag map one scan holds is BOUNDED, because every entry is memory a
/// caller chose — at the bound a tag still lands, past it the call is refused with the
/// operation's own `ValidationException` and the map is unchanged.
#[tokio::test]
async fn a_scans_tag_map_is_bounded() {
    let (_engine, router) = harness();
    let arn = scan_with_tags(&router, "tag-bound", json!({})).await;

    let at_bound: Vec<(String, String)> = (0..TAGS_PER_SCAN_QUOTA)
        .map(|n| (format!("key{n}"), format!("value{n}")))
        .collect();
    let filled = tag(&router, &arn, tag_object(&at_bound)).await;
    assert_eq!(
        filled.status, 204,
        "{TAGS_PER_SCAN_QUOTA} tags, the bound itself: expected 204, got {filled:?}"
    );

    // Re-tagging a key already held adds nothing, so it is never refused.
    let rewritten = tag(&router, &arn, json!({ "key0": "rewritten" })).await;
    assert_eq!(
        rewritten.status, 204,
        "re-tagging a held key at the bound: expected 204, got {rewritten:?}"
    );

    let past_bound = tag(&router, &arn, json!({ "one-too-many": "v" })).await;
    assert_eq!(
        past_bound.status, 400,
        "one tag past the bound: expected HTTP 400, got {past_bound:?}"
    );
    assert_eq!(
        past_bound.error_type, "ValidationException",
        "one tag past the bound: expected error code ValidationException, got {past_bound:?}"
    );

    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.get("tags").as_object().map(serde_json::Map::len),
        Some(TAGS_PER_SCAN_QUOTA),
        "after the refusal: expected the map to still hold {TAGS_PER_SCAN_QUOTA} tags, got \
         {listed:?}"
    );
    assert_eq!(
        listed.get("tags")["key0"].as_str(),
        Some("rewritten"),
        "after the refusal: expected the rewritten value to survive, got {listed:?}"
    );

    // Freeing one entry makes room again.
    let freed = untag(&router, &arn, &["key0"]).await;
    assert_eq!(freed.status, 204, "freeing one entry: got {freed:?}");
    let accepted = tag(&router, &arn, json!({ "one-too-many": "v" })).await;
    assert_eq!(
        accepted.status, 204,
        "the same tag once there is room: expected 204, got {accepted:?}"
    );
}
