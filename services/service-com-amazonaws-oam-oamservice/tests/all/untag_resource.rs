// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UntagResource`.
//!
//! The rule under test: `UntagResource` removes exactly the tag keys its `TagKeys`
//! names from the ONE tag set persisted on the resource its `ResourceArn` names — a
//! sink or a link, since both support tagging — leaving every other tag that resource
//! carries in place, and a key the resource does not carry removes nothing (so the same
//! call repeated answers the same way and leaves the same state). A key in the namespace
//! Amazon Web Services reserves for its own tags is not one a customer may remove, which
//! the tagging path refuses with `BadRequestException` and its own fixed sentence; an ARN
//! that names no stored resource of the Region the request was signed for is that path's
//! `NotFoundException` and removes nothing at all, and an ARN that is not one of this
//! service's at all is refused for its syntax.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, the
//! restJson1 REST binding this operation's `DELETE /tags/{ResourceArn}` route uses with
//! `TagKeys` bound to the repeated `tagKeys` query parameter, the modelled-constraint
//! walk and the handlers — so what is asserted is what a signed AWS SDK request
//! observes. What was removed is read back through BOTH readers of that one stored map
//! (`ListTagsForResource` and `GetSink`/`GetLink` with `IncludeTags`), so a removal that
//! only edited a second copy of the tags would show.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_oam_oamservice::state::{
    INVALID_TAGGING_ARN_MESSAGE, RESERVED_TAG_KEY_REMOVE_MESSAGE,
    TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::front_end_refusals::refusal_message;
use super::list_tags_for_resource::{list_tags, percent_encode_segment};
use super::sinks_and_links::{
    call, create_link_ok, create_sink_ok, dispatch_parsed, emulator, error_code, ACCOUNT, AKID,
};

/// The region most cases sign for.
const REGION: &str = "us-east-1";

/// The longest and shortest a tag key may be, from `TagKey`'s own
/// `@length(min: 1, max: 128)` (`smithy_query(query="shape",shape="TagKey")`), which
/// `TagKeys`' elements target.
const TAG_KEY_MAX: usize = 128;
const TAG_KEY_MIN: usize = 1;

/// Which of this service's two taggable resources a case is addressing.
///
/// Both are reached by ARN alone on the tagging path (`ResourceArn`), and each has its
/// own reader that reports tags on request: `GetSink`/`GetLink` with `IncludeTags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Sink,
    Link,
}

impl Kind {
    /// The operation that reads this kind back.
    fn get_uri(self) -> &'static str {
        match self {
            Kind::Sink => "/GetSink",
            Kind::Link => "/GetLink",
        }
    }
}

/// A REST-bound request to this service signed for `region` by [`AKID`], with `body` as
/// its payload (empty for a method that carries none).
fn rest_request(
    method: &str,
    path_and_query: &str,
    body: &Value,
    region: &str,
) -> http::Request<bytes::Bytes> {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(serde_json::to_vec(body).expect("serialize the request body"))
    };
    http::Request::builder()
        .method(method)
        .uri(path_and_query)
        .header("host", format!("oam.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={AKID}/20260101/{region}/oam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed test request")
}

/// `DELETE /tags/{ResourceArn}?tagKeys=…` — `UntagResource` as an SDK issues it.
///
/// `ResourceArn` is a non-greedy `@httpLabel` and `TagKeys` is bound to
/// `@httpQuery("tagKeys")` (`smithy_query(query="operation",operation="UntagResource")`),
/// so the ARN travels as ONE percent-encoded path segment and every key as its own
/// query pair, in request order.
async fn untag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    keys: &[String],
    region: &str,
) -> (u16, Value) {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encode_segment(key)))
        .collect();
    let path = format!(
        "/tags/{}?{}",
        percent_encode_segment(resource_arn),
        query.join("&")
    );
    dispatch_parsed(router, rest_request("DELETE", &path, &Value::Null, region)).await
}

/// `PUT /tags/{ResourceArn}` with `Tags` in the body — `TagResource`, used to seed the
/// tag set a case then removes keys from.
///
/// `pub(crate)` so the sibling suites that have to SET tags before judging something
/// else drive this one request builder rather than a second copy of it.
pub(crate) async fn tag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
    region: &str,
) -> (u16, Value) {
    let entries: Map<String, Value> = tags
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    let path = format!("/tags/{}", percent_encode_segment(resource_arn));
    dispatch_parsed(
        router,
        rest_request(
            "PUT",
            &path,
            &json!({ "Tags": Value::Object(entries) }),
            region,
        ),
    )
    .await
}

/// The `Tags` member of a response, as a map. An absent member is no tags: `Tags` is
/// optional precisely so a response that was asked NOT to report them can leave it out.
fn tags_of(case: &str, body: &Value) -> BTreeMap<String, String> {
    match body.get("Tags") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(Value::Object(entries)) => entries
            .iter()
            .map(|(key, value)| {
                let value = value.as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: tag {key:?} is not a string in {body:?}")
                });
                (key.clone(), value.to_string())
            })
            .collect(),
        other => panic!("case {case:?}: expected a Tags object, got {other:?}"),
    }
}

/// The tags the resource `arn` names really carries, read through BOTH operations that
/// report that one stored map: `ListTagsForResource` and `kind`'s own Get with
/// `IncludeTags`. The two must agree — there is one tag set per resource however it is
/// reached — and that agreed map is what the case asserts on.
async fn stored_tags(
    router: &MultiServiceRouter,
    case: &str,
    kind: Kind,
    arn: &str,
    region: &str,
) -> BTreeMap<String, String> {
    let (status, listed) = list_tags(router, arn, region).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource on {arn} expected 200, got {status} {listed:?}"
    );
    let listed = tags_of(case, &listed);

    let (status, read) = call(
        router,
        kind.get_uri(),
        &json!({ "Identifier": arn, "IncludeTags": true }),
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading {kind:?} {arn} back expected 200, got {status} {read:?}"
    );
    let read = tags_of(case, &read);

    assert_eq!(
        listed, read,
        "case {case:?}: ListTagsForResource and {} must report the same stored tag set, \
         got {listed:?} and {read:?}",
        kind.get_uri()
    );
    listed
}

/// Create a sink and a link attached to it in `region`, answering both ARNs.
///
/// The sink is created by the monitoring account and the link by [`AKID`]'s source
/// account: a link may not attach to a sink its own account owns.
async fn sink_and_link(router: &MultiServiceRouter, name: &str, region: &str) -> (String, String) {
    let sink_arn = create_sink_ok(router, name, region).await;
    let link_arn = create_link_ok(router, &sink_arn, AKID, region).await;
    (sink_arn, link_arn)
}

/// A tag map from `(key, value)` pairs.
fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The keys of `pairs`, as the request carries them.
fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// One row of the removal table: the tag set the resource is seeded with, the keys one
/// `UntagResource` request names, and the tag set the resource must then carry.
struct Row {
    case: String,
    seed: BTreeMap<String, String>,
    remove: Vec<String>,
    remaining: BTreeMap<String, String>,
}

/// `count` distinct tags, keyed `<prefix>-<n>`.
fn generated(prefix: &str, count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|n| (format!("{prefix}-{n:02}"), format!("value-{n:02}")))
        .collect()
}

/// The removal rule, over a table spanning what it generalises over: how many tags the
/// resource holds (0, 1, 3, 4, 50), how many keys one request names (1, 2, 3, 49),
/// whether those keys are present, absent or a mix, their order and multiplicity, and
/// their character class (lower, mixed case, dotted, spaced, punctuated, non-ASCII) and
/// length (the `TagKey` `@length(1..128)` bounds).
///
/// Every row runs against BOTH a sink and a link, since the rule is about the resource
/// the ARN names and not about which kind it is, and every row is then repeated to pin
/// the idempotence the rule implies.
#[tokio::test]
async fn untag_resource_removes_exactly_the_keys_the_request_names() {
    let longest_key = "k".repeat(TAG_KEY_MAX);
    let shortest_key = "k".repeat(TAG_KEY_MIN);
    let bulk = generated("bulk", 50);
    let bulk_removed: Vec<String> = bulk.keys().skip(1).cloned().collect();
    let bulk_remaining: BTreeMap<String, String> = bulk
        .iter()
        .take(1)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    let rows = vec![
        Row {
            case: "the only key the resource carries".to_string(),
            seed: map(&[("only", "one")]),
            remove: keys(&["only"]),
            remaining: BTreeMap::new(),
        },
        Row {
            case: "one key of several, the rest surviving".to_string(),
            seed: map(&[
                ("team", "payments"),
                ("tier", "prod"),
                ("cost.center", "42"),
                ("zz", ""),
            ]),
            remove: keys(&["tier"]),
            remaining: map(&[("team", "payments"), ("cost.center", "42"), ("zz", "")]),
        },
        Row {
            case: "a key the resource does not carry removes nothing".to_string(),
            seed: map(&[("env", "dev")]),
            remove: keys(&["nope"]),
            remaining: map(&[("env", "dev")]),
        },
        Row {
            case: "present and absent keys in one request".to_string(),
            seed: map(&[("a", "1"), ("b", "2"), ("c", "3")]),
            remove: keys(&["b", "missing", "c"]),
            remaining: map(&[("a", "1")]),
        },
        Row {
            case: "every key at once, in an order unrelated to any stored one".to_string(),
            seed: map(&[("one", "1"), ("two", "2"), ("three", "3")]),
            remove: keys(&["two", "one", "three"]),
            remaining: BTreeMap::new(),
        },
        Row {
            case: "the same key named twice in one request".to_string(),
            seed: map(&[("dup", "x"), ("keep", "y")]),
            remove: keys(&["dup", "dup"]),
            remaining: map(&[("keep", "y")]),
        },
        Row {
            case: "tag keys are matched exactly, so case matters".to_string(),
            seed: map(&[("env", "lower"), ("Env", "mixed"), ("ENV", "upper")]),
            remove: keys(&["env"]),
            remaining: map(&[("Env", "mixed"), ("ENV", "upper")]),
        },
        Row {
            case: "keys whose characters have to be percent-encoded on the wire".to_string(),
            seed: map(&[
                ("with space", "a b"),
                ("cost:centre/eu", "équipe ops +1"),
                ("dot.ted", "v1"),
                ("ünïcode", "v3"),
            ]),
            remove: keys(&["with space", "ünïcode", "cost:centre/eu"]),
            remaining: map(&[("dot.ted", "v1")]),
        },
        Row {
            case: "a resource carrying no tags at all".to_string(),
            seed: BTreeMap::new(),
            remove: keys(&["env"]),
            remaining: BTreeMap::new(),
        },
        Row {
            case: "a key whose stored value is the empty string".to_string(),
            seed: map(&[("empty", ""), ("keep", "me")]),
            remove: keys(&["empty"]),
            remaining: map(&[("keep", "me")]),
        },
        Row {
            // Both published bounds of `TagKey` `@length(1..128)`, removed together.
            case: "the shortest and the longest key the model allows".to_string(),
            seed: {
                let mut seed = map(&[("keep", "me")]);
                seed.insert(shortest_key.clone(), "short".to_string());
                seed.insert(longest_key.clone(), "long".to_string());
                seed
            },
            remove: vec![shortest_key.clone(), longest_key.clone()],
            remaining: map(&[("keep", "me")]),
        },
        Row {
            case: "49 keys of a 50-tag resource, one left behind".to_string(),
            seed: bulk.clone(),
            remove: bulk_removed,
            remaining: bulk_remaining,
        },
    ];

    for row in rows {
        for kind in [Kind::Sink, Kind::Link] {
            let router = emulator();
            let (sink_arn, link_arn) = sink_and_link(&router, "untag-sink", REGION).await;
            let arn = match kind {
                Kind::Sink => sink_arn,
                Kind::Link => link_arn,
            };
            let case = format!("{} on a {kind:?}", row.case);

            if !row.seed.is_empty() {
                let (status, body) = tag(&router, &arn, &row.seed, REGION).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: seeding {} tags expected 200, got {status} {body:?}",
                    row.seed.len()
                );
            }

            let (status, body) = untag(&router, &arn, &row.remove, REGION).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UntagResource expected 200, got {status} {body:?}"
            );
            assert!(
                matches!(&body, Value::Null | Value::Object(_)),
                "case {case:?}: UntagResource models no output members, got {body:?}"
            );

            let stored = stored_tags(&router, &case, kind, &arn, REGION).await;
            assert_eq!(
                stored, row.remaining,
                "case {case:?}: expected the stored tags {:?}, got {stored:?}",
                row.remaining
            );

            // Repeating the very same call names keys the resource no longer carries,
            // which the rule makes a no-op: the same answer and the same state.
            let (status, body) = untag(&router, &arn, &row.remove, REGION).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the repeated UntagResource expected 200, got {status} {body:?}"
            );
            let repeated = stored_tags(&router, &case, kind, &arn, REGION).await;
            assert_eq!(
                repeated, row.remaining,
                "case {case:?}: the repeated call must leave the same tags {:?}, got {repeated:?}",
                row.remaining
            );
        }
    }
}

/// An ARN that names no stored resource of this account and Region is refused, and the
/// refused call removes nothing: the resources that ARE stored keep every tag they
/// carried.
///
/// The rows span the ways an ARN can fail to name a stored resource — an id nothing was
/// stored under, a field (partition, account, Region) that does not match the stored
/// record, an `oam` resource kind this service does not tag, a resource since deleted —
/// plus the two forms the modelled `Arn` pattern `^arn:(\w|-)+:oam:.+:.+:.+`
/// (`smithy_query(query="shape",shape="Arn")`) refuses outright, which the tagging path
/// answers for its SYNTAX before the handler runs.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_refused_and_removes_nothing() {
    let router = emulator();
    let (sink_arn, link_arn) = sink_and_link(&router, "resolved-sink", REGION).await;
    let sink_id = sink_arn
        .rsplit('/')
        .next()
        .expect("the sink ARN carries an id segment")
        .to_string();
    let link_id = link_arn
        .rsplit('/')
        .next()
        .expect("the link ARN carries an id segment")
        .to_string();

    let seeded = map(&[("env", "dev"), ("team", "observability")]);
    for arn in [&sink_arn, &link_arn] {
        let (status, body) = tag(&router, arn, &seeded, REGION).await;
        assert_eq!(status, 200, "seeding {arn}: got {status} {body:?}");
    }

    let unknown = "11111111-2222-4333-8444-555555555555";
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a sink id nothing was stored under",
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:sink/{unknown}"),
            404,
            "NotFoundException",
        ),
        (
            "a link id nothing was stored under",
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:link/{unknown}"),
            404,
            "NotFoundException",
        ),
        (
            "the stored sink id under another Region",
            format!("arn:aws:oam:eu-west-1:{ACCOUNT}:sink/{sink_id}"),
            404,
            "NotFoundException",
        ),
        (
            "the stored link id under another Region",
            format!("arn:aws:oam:eu-west-1:{ACCOUNT}:link/{link_id}"),
            404,
            "NotFoundException",
        ),
        (
            "the stored sink id under another account",
            format!("arn:aws:oam:{REGION}:000000000001:sink/{sink_id}"),
            404,
            "NotFoundException",
        ),
        (
            "the stored link id in another partition",
            format!("arn:aws-cn:oam:{REGION}:{ACCOUNT}:link/{link_id}"),
            404,
            "NotFoundException",
        ),
        (
            "an oam resource kind that is neither a sink nor a link",
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:policy/{sink_id}"),
            404,
            "NotFoundException",
        ),
        (
            "a sink ARN with an empty id segment",
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:sink/"),
            404,
            "NotFoundException",
        ),
        (
            // Refused for its SYNTAX, before the handler: the ARN is not one of this
            // service's at all, which the tagging path answers with its own code and
            // its own fixed sentence (see `crate::service`).
            "the sink's bare id, which is not an ARN at all",
            sink_id.clone(),
            400,
            "BadRequestException",
        ),
        (
            "an ARN of another service's namespace",
            format!("arn:aws:logs:{REGION}:{ACCOUNT}:sink/{sink_id}"),
            400,
            "BadRequestException",
        ),
    ];

    for (case, resource_arn, expected_status, expected_code) in rows {
        let (status, body) = untag(&router, &resource_arn, &keys(&["env"]), REGION).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?} ({resource_arn}): expected {expected_status} {expected_code}, \
             got {status} {code} {body:?}"
        );
        let expected_message = if expected_status == 404 {
            TAGGED_RESOURCE_NOT_FOUND_MESSAGE
        } else {
            INVALID_TAGGING_ARN_MESSAGE
        };
        assert_eq!(
            refusal_message(&body),
            expected_message,
            "case {case:?}: expected the tagging path's message {expected_message:?}, \
             got {body:?}"
        );
    }

    // Nothing the refused calls named left either stored resource.
    for (case, kind, arn) in [
        ("after the refusals", Kind::Sink, &sink_arn),
        ("after the refusals", Kind::Link, &link_arn),
    ] {
        let stored = stored_tags(&router, case, kind, arn, REGION).await;
        assert_eq!(
            stored, seeded,
            "case {case:?}: the {kind:?} must still carry every seeded tag, got {stored:?}"
        );
    }

    // A deleted resource stops being untaggable, by the same rule.
    let (status, body) = call(
        &router,
        "/DeleteLink",
        &json!({ "Identifier": link_arn }),
        REGION,
    )
    .await;
    assert_eq!(status, 200, "DeleteLink: got {status} {body:?}");
    let (status, body) = untag(&router, &link_arn, &keys(&["env"]), REGION).await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (404, "NotFoundException"),
        "a deleted link: expected 404 NotFoundException, got {status} {code} {body:?}"
    );
}

/// A key in the namespace Amazon Web Services reserves for its own tags is not one a
/// customer may REMOVE: the refusal carries the tagging path's code and the removal
/// wording (distinct from the wording a request that tries to SET one earns), the prefix
/// is matched case-insensitively, and the resource's tag set is left exactly as it was.
///
/// The rows span the CASE the prefix is spelled in and what follows it, and each is sent
/// alone and beside a key the resource really holds — one reserved key refuses the whole
/// request. The boundary of the rule is a table of keys that only LOOK reserved: those
/// are ordinary customer keys, and they come off.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_removes_nothing() {
    let router = emulator();
    let (sink_arn, link_arn) = sink_and_link(&router, "reserved-sink", REGION).await;
    let seeded = map(&[("env", "prod"), ("team", "observability")]);
    for arn in [&sink_arn, &link_arn] {
        let (status, body) = tag(&router, arn, &seeded, REGION).await;
        assert_eq!(status, 200, "seeding {arn}: got {status} {body:?}");
    }

    let reserved: &[(&str, &str)] = &[
        ("lower case", "aws:sim2real"),
        ("upper case", "AWS:sim2real"),
        ("title case", "Aws:cloudformation:stack-name"),
        ("mixed case", "aWs:managed"),
        ("the prefix alone", "aws:"),
    ];

    for (why, key) in reserved {
        for (kind, arn) in [(Kind::Sink, &sink_arn), (Kind::Link, &link_arn)] {
            let case = format!("UntagResource on a {kind:?} with a key in {why}");
            for named in [keys(&[key]), keys(&["env", key])] {
                let (status, refused) = untag(&router, arn, &named, REGION).await;
                assert_eq!(
                    (status, error_code(&refused).as_str()),
                    (400, "BadRequestException"),
                    "case {case:?} ({named:?}): expected 400 BadRequestException, got {status} \
                     {refused:?}"
                );
                assert_eq!(
                    refusal_message(&refused),
                    RESERVED_TAG_KEY_REMOVE_MESSAGE,
                    "case {case:?} ({named:?}): expected the reserved-namespace removal \
                     message, got {refused:?}"
                );
            }
            let stored = stored_tags(&router, &case, kind, arn, REGION).await;
            assert_eq!(
                stored, seeded,
                "case {case:?}: a refused removal must leave the tag set alone, got {stored:?}"
            );
        }
    }

    let lookalikes = map(&[
        ("aws", "no colon"),
        ("aws-managed", "a dash, not a colon"),
        ("awsome:tag", "the prefix is longer than aws:"),
        ("cost:aws:centre", "the prefix is not at the start"),
    ]);
    let (status, body) = tag(&router, &sink_arn, &lookalikes, REGION).await;
    assert_eq!(
        status, 200,
        "keys that only look reserved must be settable, got {status} {body:?}"
    );
    let named: Vec<String> = lookalikes.keys().cloned().collect();
    let (status, body) = untag(&router, &sink_arn, &named, REGION).await;
    assert_eq!(
        status, 200,
        "keys that only look reserved must be removable, got {status} {body:?}"
    );
    let stored = stored_tags(&router, "the lookalike keys", Kind::Sink, &sink_arn, REGION).await;
    assert_eq!(
        stored, seeded,
        "expected only the seeded tags to be left, got {stored:?}"
    );
}

/// A `tagKeys` entry outside the modelled `TagKey` `@length(1..128)` is refused before
/// the handler runs, and the refused call removes nothing — while a key AT either bound
/// is served (the table above removes both).
#[tokio::test]
async fn a_tag_key_outside_its_modelled_length_is_refused_and_removes_nothing() {
    let router = emulator();
    let (sink_arn, _link_arn) = sink_and_link(&router, "bounded-sink", REGION).await;
    let seeded = map(&[("env", "dev")]);
    let (status, body) = tag(&router, &sink_arn, &seeded, REGION).await;
    assert_eq!(status, 200, "seeding the sink: got {status} {body:?}");

    let too_long = "k".repeat(TAG_KEY_MAX + 1);
    let rows: Vec<(String, Vec<String>)> = vec![
        (
            format!("a key of {} characters, one past the maximum", too_long.len()),
            vec![too_long],
        ),
        (
            "an empty key, one short of the minimum".to_string(),
            vec![String::new()],
        ),
        (
            "a present key alongside one past the maximum".to_string(),
            vec!["env".to_string(), "k".repeat(TAG_KEY_MAX + 1)],
        ),
    ];

    for (case, remove) in rows {
        let (status, body) = untag(&router, &sink_arn, &remove, REGION).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code} {body:?}"
        );
        let stored = stored_tags(&router, &case, Kind::Sink, &sink_arn, REGION).await;
        assert_eq!(
            stored, seeded,
            "case {case:?}: a refused call must remove nothing, got {stored:?}"
        );
    }

    // `TagKeys` is `@required` and bound to the query string, so a request carrying no
    // `tagKeys` pair at all is not a legal `UntagResource`: it must not be served, and
    // it must leave the tag set alone.
    let path = format!("/tags/{}", percent_encode_segment(&sink_arn));
    let (status, body) =
        dispatch_parsed(&router, rest_request("DELETE", &path, &Value::Null, REGION)).await;
    // A request binding none of a `@required` query member names no legal
    // `UntagResource`, so the wording is the framework's routing answer rather than one
    // of this operation's own refusals: what matters here is that it is not served and
    // that it removes nothing.
    assert_ne!(
        status, 200,
        "case \"no tagKeys at all\": the call must not be served, got {status} {body:?}"
    );
    let stored = stored_tags(&router, "no tagKeys at all", Kind::Sink, &sink_arn, REGION).await;
    assert_eq!(
        stored, seeded,
        "case \"no tagKeys at all\": nothing may be removed, got {stored:?}"
    );
}

/// Each resource has its own tag set, so a removal reaches the ONE resource its ARN
/// names: across the two kinds, across two resources of the same kind, and across
/// Regions.
#[tokio::test]
async fn a_removal_reaches_only_the_resource_its_arn_names() {
    let router = emulator();
    let (east_sink, east_link) = sink_and_link(&router, "east-sink", REGION).await;
    // "Each account can contain one sink per Region" (`CreateSink`), so a second sink
    // means a second Region — and the second Region's link is created by a second
    // source account's credential through the same monitoring-account sink.
    let west_sink = create_sink_ok(&router, "west-sink", "eu-west-1").await;
    let west_link = create_link_ok(&router, &west_sink, AKID, "eu-west-1").await;

    let shared = map(&[("owner", "shared"), ("keep", "me")]);
    let targets = [
        ("the east sink", Kind::Sink, east_sink.clone(), REGION),
        ("the east link", Kind::Link, east_link.clone(), REGION),
        ("the west sink", Kind::Sink, west_sink.clone(), "eu-west-1"),
        ("the west link", Kind::Link, west_link.clone(), "eu-west-1"),
    ];

    // Every resource is seeded with the SAME keys, so a removal that leaked to another
    // record would leave that record short of a tag.
    for (case, _, arn, region) in &targets {
        let (status, body) = tag(&router, arn, &shared, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource expected 200, got {status} {body:?}"
        );
    }

    // One target at a time loses `owner`; every other target must still carry both.
    for (nth, (case, kind, arn, region)) in targets.iter().enumerate() {
        let (status, body) = untag(&router, arn, &keys(&["owner"]), region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource expected 200, got {status} {body:?}"
        );
        let stored = stored_tags(&router, case, *kind, arn, region).await;
        assert_eq!(
            stored,
            map(&[("keep", "me")]),
            "case {case:?}: expected only its remaining tag, got {stored:?}"
        );

        for (other_case, other_kind, other_arn, other_region) in targets.iter().skip(nth + 1) {
            let stored = stored_tags(&router, other_case, *other_kind, other_arn, other_region)
                .await;
            assert_eq!(
                stored, shared,
                "case {case:?}: {other_case} must keep every seeded tag, got {stored:?}"
            );
        }
    }
}
