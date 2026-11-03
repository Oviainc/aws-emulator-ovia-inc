// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `TagResource` — the tag-writing half of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization (including the `@httpLabel` ARN in `PUT /tags/{ResourceArn}`),
//! the modelled-constraint walk and the handlers — so what is asserted is what a signed
//! AWS SDK request observes.
//!
//! The rules under test, from `TagResource`'s and `TooManyTagsException`'s own
//! documentation:
//!
//! * the request's tags are MERGED into the tag set the ARN's resource already carries —
//!   a key it does not hold is appended, a key it holds has its value replaced, a key the
//!   request does not mention is left alone — and the result is what every later read of
//!   that one record reports (`GetSink`/`GetLink` with `IncludeTags`);
//! * a resource can hold no more than [`MAX_TAGS_PER_RESOURCE`] user tags; a call that
//!   would leave it holding more is refused with the tagging path's
//!   `BadRequestException` (400) and writes nothing;
//! * an ARN that names no stored sink or link of this account and Region is that same
//!   path's `NotFoundException` (404) — nothing is created for it, and an ARN that is
//!   not one of this service's at all is refused for its syntax instead.
//!
//! Both of this service's taggable resources (a sink and a link) are driven through the
//! same table, because the rule is about the resource the ARN names and not about which
//! kind it is.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_oam_oamservice::state::{
    INVALID_TAGGING_ARN_MESSAGE, MAX_TAGS_PER_RESOURCE, TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The access key id every case signs with.
const AKID: &str = "AKIDOAMTAGS1";

/// The access key id the sink of a sink/link pair is created with: a link joins a
/// source account to a sink in a MONITORING account, so the sink is never created by
/// the credential that then links to it.
const MONITORING_AKID: &str = "AKIDOAMTAGSSINK";

/// The region most cases sign for.
const REGION: &str = "us-east-1";

/// Which of this service's two taggable resources a case is addressing.
///
/// Both are reached by ARN alone on the tagging path (`ResourceArn`), and each has its
/// own reader that reports tags on request: `GetSink`/`GetLink` with `IncludeTags`
/// (`smithy_query(query="operation",operation="GetSink,GetLink")`).
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

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_oam_oamservice::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `oam` in `region` by `akid`: the signal the router routes on,
/// and the one the handlers read the region and the source account from.
fn request(
    method: &str,
    uri: &str,
    body: &Value,
    region: &str,
    akid: &str,
) -> http::Request<bytes::Bytes> {
    http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("oam.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{region}/oam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("a well-formed test request")
}

/// Status and parsed body of one call, signed by `akid`.
async fn call_with_as(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
    region: &str,
    akid: &str,
) -> (u16, Value) {
    let response = router
        .dispatch(request(method, uri, body, region, akid))
        .await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// One `POST` call in `region`, the shape every non-tagging operation of this service
/// takes.
async fn post(router: &MultiServiceRouter, uri: &str, body: &Value, region: &str) -> (u16, Value) {
    call_with(router, "POST", uri, body, region).await
}

/// Status and parsed body of one call signed by [`AKID`]. An empty body reads as
/// `Value::Null`.
async fn call_with(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
    region: &str,
) -> (u16, Value) {
    call_with_as(router, method, uri, body, region, AKID).await
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.oam#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A member of a response body, as a string.
fn field(case: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {body:?}"))
        .to_string()
}

/// An ARN as it travels in a `@httpLabel`: ONE path segment, so the `/` before the
/// resource id has to be written `%2F`. `:` is percent-encoded as well, which is the
/// spelling an SDK sends; the other spelling a path admits is
/// [`encode_slash_only`], exercised by its own case below.
fn encode(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// The same ARN with only its `/` escaped: `:` is a legal path character, so this
/// spelling has to name the very same resource.
fn encode_slash_only(arn: &str) -> String {
    arn.replace('/', "%2F")
}

/// `PUT /tags/{ResourceArn}` with `tags` in the body — what an SDK `tag_resource` sends.
async fn tag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
    region: &str,
) -> (u16, Value) {
    tag_label(router, &encode(resource_arn), tags, region).await
}

/// The same call with the `@httpLabel` already spelled out, for a case that chooses the
/// encoding itself.
async fn tag_label(
    router: &MultiServiceRouter,
    label: &str,
    tags: &BTreeMap<String, String>,
    region: &str,
) -> (u16, Value) {
    let entries: Map<String, Value> = tags
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    call_with(
        router,
        "PUT",
        &format!("/tags/{label}"),
        &json!({ "Tags": Value::Object(entries) }),
        region,
    )
    .await
}

/// The `Tags` of a read-back response, as a map. An absent member is no tags: `Tags` is
/// optional precisely so a resource carrying none can leave it out.
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

/// The tags `kind`'s resource carries, read back through its own Get operation with
/// `IncludeTags` — the round trip that proves a tag was STORED and not merely accepted.
async fn stored_tags(
    router: &MultiServiceRouter,
    case: &str,
    kind: Kind,
    arn: &str,
    region: &str,
) -> BTreeMap<String, String> {
    let (status, read) = post(
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
    tags_of(case, &read)
}

/// Create a sink and a link attached to it in `region`, answering both ARNs.
///
/// The sink is created by the monitoring account and the link by [`AKID`]'s source
/// account: a link may not attach to a sink its own account owns.
async fn sink_and_link(router: &MultiServiceRouter, name: &str, region: &str) -> (String, String) {
    let (status, sink) = call_with_as(
        router,
        "POST",
        "/CreateSink",
        &json!({ "Name": name }),
        region,
        MONITORING_AKID,
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateSink {name}: expected 200, got {status} {sink:?}"
    );
    let sink_arn = field(name, &sink, "Arn");

    let (status, link) = post(
        router,
        "/CreateLink",
        &json!({
            "LabelTemplate": "$AccountName",
            "ResourceTypes": ["AWS::CloudWatch::Metric"],
            "SinkIdentifier": sink_arn,
        }),
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLink on {name}: expected 200, got {status} {link:?}"
    );
    let link_arn = field(name, &link, "Arn");
    (sink_arn, link_arn)
}

/// A map from `(key, value)` pairs.
fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `count` distinct tags, keyed `<prefix>-<n>`.
fn generated(prefix: &str, count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|n| (format!("{prefix}-{n:02}"), format!("value-{n:02}")))
        .collect()
}

/// One row of the merge table: a sequence of `TagResource` calls and the tag set the
/// resource must hold afterwards.
struct Merge {
    case: &'static str,
    calls: Vec<BTreeMap<String, String>>,
    expected: BTreeMap<String, String>,
}

/// The merge rule, over a table that spans what it generalises over: how many tags one
/// call carries, how many calls there are, whether a later call repeats a key, and the
/// forms a key and a value may take (the bounds come from `TagKey` `@length(1..128)` and
/// `TagValue` `@length(0..256)`, per
/// `smithy_query(query="shape",shape="TagKey,TagValue")`).
///
/// Every row runs against BOTH a sink and a link, since the rule is about the resource
/// the ARN names rather than its kind.
#[tokio::test]
async fn tag_resource_merges_its_tags_into_the_resources_stored_set() {
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    let rows = vec![
        Merge {
            case: "a single tag on an untagged resource",
            calls: vec![map(&[("team", "observability")])],
            expected: map(&[("team", "observability")]),
        },
        Merge {
            case: "several tags in one call",
            calls: vec![map(&[("a", "1"), ("b", "2"), ("c", "3")])],
            expected: map(&[("a", "1"), ("b", "2"), ("c", "3")]),
        },
        Merge {
            case: "a second call appends a new key",
            calls: vec![map(&[("a", "1")]), map(&[("b", "2")])],
            expected: map(&[("a", "1"), ("b", "2")]),
        },
        Merge {
            case: "a repeated key replaces its value",
            calls: vec![map(&[("env", "dev")]), map(&[("env", "prod")])],
            expected: map(&[("env", "prod")]),
        },
        Merge {
            case: "a call that both replaces and appends",
            calls: vec![
                map(&[("env", "dev"), ("keep", "me")]),
                map(&[("env", "prod"), ("owner", "sre")]),
            ],
            expected: map(&[("env", "prod"), ("keep", "me"), ("owner", "sre")]),
        },
        Merge {
            case: "an empty tag map leaves the set alone",
            calls: vec![map(&[("keep", "me")]), BTreeMap::new()],
            expected: map(&[("keep", "me")]),
        },
        Merge {
            case: "an empty value is a value (TagValue @length min 0)",
            calls: vec![map(&[("empty", "")])],
            expected: map(&[("empty", "")]),
        },
        Merge {
            case: "the shortest and longest key and value the model allows",
            calls: vec![map(&[
                ("k", ""),
                (longest_key.as_str(), longest_value.as_str()),
            ])],
            expected: map(&[
                ("k", ""),
                (longest_key.as_str(), longest_value.as_str()),
            ]),
        },
        Merge {
            case: "keys differing only in case are two tags",
            calls: vec![map(&[("Env", "upper"), ("env", "lower")])],
            expected: map(&[("Env", "upper"), ("env", "lower")]),
        },
        Merge {
            case: "punctuation, spaces and non-ASCII in keys and values",
            calls: vec![map(&[
                ("cost:centre/eu", "équipe ops +1"),
                ("awsome:not-reserved_here.1", "a=b,c=d"),
            ])],
            expected: map(&[
                ("cost:centre/eu", "équipe ops +1"),
                ("awsome:not-reserved_here.1", "a=b,c=d"),
            ]),
        },
        Merge {
            case: "the whole per-request maximum of 50 tags at once",
            calls: vec![generated("bulk", MAX_TAGS_PER_RESOURCE)],
            expected: generated("bulk", MAX_TAGS_PER_RESOURCE),
        },
    ];

    for row in rows {
        for kind in [Kind::Sink, Kind::Link] {
            let router = emulator();
            let (sink_arn, link_arn) = sink_and_link(&router, "merge-sink", REGION).await;
            let arn = match kind {
                Kind::Sink => sink_arn,
                Kind::Link => link_arn,
            };
            let case = format!("{} on a {kind:?}", row.case);

            for (nth, call) in row.calls.iter().enumerate() {
                let (status, body) = tag(&router, &arn, call, REGION).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: TagResource call #{nth} expected 200, got {status} {body:?}"
                );
            }

            let stored = stored_tags(&router, &case, kind, &arn, REGION).await;
            assert_eq!(
                stored, row.expected,
                "case {case:?}: expected the stored tags {:?}, got {stored:?}",
                row.expected
            );
        }
    }
}

/// One row of the cap table: the tags the resource already holds, the call made next,
/// and whether the resulting count leaves it inside the cap.
struct Cap {
    case: &'static str,
    seed: BTreeMap<String, String>,
    next: BTreeMap<String, String>,
    accepted: bool,
}

/// The cap is on what the RESOURCE ends up holding, counted over distinct keys: at the
/// published bound a call still succeeds, one tag past it is refused with the tagging
/// path's `BadRequestException` (400) and writes nothing, and a call that only replaces
/// values is never refused however full the resource is.
#[tokio::test]
async fn a_resource_holds_at_most_fifty_tags_and_a_refused_call_writes_nothing() {
    // Rows: how many tags the resource already holds, the call made next, and what the
    // resulting count makes of it. The bound itself (50), one past it, and well past it.
    let below = MAX_TAGS_PER_RESOURCE - 5;
    let cases = vec![
        Cap {
            case: "exactly the maximum lands",
            seed: generated("seed", below),
            next: generated("more", 5),
            accepted: true,
        },
        Cap {
            case: "one tag past the maximum is refused",
            seed: generated("seed", below),
            next: generated("more", 6),
            accepted: false,
        },
        Cap {
            case: "far past the maximum is refused",
            seed: generated("seed", below),
            next: generated("more", MAX_TAGS_PER_RESOURCE),
            accepted: false,
        },
        Cap {
            case: "a repeated key does not count towards the growth",
            seed: generated("seed", below),
            next: {
                let mut tags = generated("more", 5);
                tags.insert("seed-00".to_string(), "replaced".to_string());
                tags
            },
            accepted: true,
        },
        Cap {
            case: "replacing values on a full resource is never refused",
            seed: generated("seed", MAX_TAGS_PER_RESOURCE),
            next: map(&[("seed-00", "replaced"), ("seed-01", "also-replaced")]),
            accepted: true,
        },
        Cap {
            case: "a single new key on a full resource is refused",
            seed: generated("seed", MAX_TAGS_PER_RESOURCE),
            next: map(&[("one-too-many", "x")]),
            accepted: false,
        },
    ];

    for Cap {
        case,
        seed,
        next,
        accepted,
    } in cases
    {
        for kind in [Kind::Sink, Kind::Link] {
            let case = format!("{case} on a {kind:?}");
            // Each row starts from a known tag set, so each one gets its own store.
            let router = emulator();
            let (sink_arn, link_arn) = sink_and_link(&router, "capped-sink", REGION).await;
            let arn = match kind {
                Kind::Sink => sink_arn,
                Kind::Link => link_arn,
            };

            let (status, body) = tag(&router, &arn, &seed, REGION).await;
            assert_eq!(
                status, 200,
                "case {case:?}: seeding {} tags expected 200, got {status} {body:?}",
                seed.len()
            );

            let (status, body) = tag(&router, &arn, &next, REGION).await;
            let mut expected = seed.clone();
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {body:?}"
                );
                for (key, value) in &next {
                    expected.insert(key.clone(), value.clone());
                }
            } else {
                let code = error_code(&body);
                assert_eq!(
                    (status, code.as_str()),
                    (400, "BadRequestException"),
                    "case {case:?}: expected 400 BadRequestException, got {status} {code} {body:?}"
                );
                assert_eq!(
                    body.get("message").and_then(Value::as_str),
                    Some(
                        format!(
                            "The resultant tag set must not have more than \
                             {MAX_TAGS_PER_RESOURCE} user tags."
                        )
                        .as_str()
                    ),
                    "case {case:?}: expected the tag-ceiling message, got {body:?}"
                );
            }

            let stored = stored_tags(&router, &case, kind, &arn, REGION).await;
            assert_eq!(
                stored, expected,
                "case {case:?}: expected {} stored tags, got {} ({stored:?})",
                expected.len(),
                stored.len()
            );
            assert!(
                stored.len() <= MAX_TAGS_PER_RESOURCE,
                "case {case:?}: a resource may never hold more than {MAX_TAGS_PER_RESOURCE} \
                 tags, got {}",
                stored.len()
            );
        }
    }
}

/// An ARN that names no stored resource of this account and Region is the tagging
/// path's own refusal, and tagging it creates nothing: a later read of the ARN the case
/// mangled still reports only what was really stored.
///
/// The rows span the ways an ARN can fail to name a stored resource — an id nothing was
/// stored under, a field (partition, account, region) that does not match the stored
/// record, a resource kind this service does not tag, a resource since deleted — plus the
/// two forms the modelled `Arn` pattern `^arn:(\w|-)+:oam:.+:.+:.+`
/// (`smithy_query(query="shape",shape="Arn")`) refuses outright, which that path answers
/// before the handler runs.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_refused_and_stores_nothing() {
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

    // A tag the sink really does carry, so a refusal that wrote something would show.
    let (status, body) = tag(&router, &sink_arn, &map(&[("real", "tag")]), REGION).await;
    assert_eq!(status, 200, "seeding the sink: got {status} {body:?}");

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
            "the stored sink id in another partition",
            format!("arn:aws-cn:oam:{REGION}:{ACCOUNT}:sink/{sink_id}"),
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
        let (status, body) = tag(&router, &resource_arn, &map(&[("new", "tag")]), REGION).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?} ({resource_arn}): expected {expected_status} {expected_code}, \
             got {status} {code} {body:?}"
        );
        let message = body
            .get("Message")
            .or_else(|| body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_message = if expected_status == 404 {
            TAGGED_RESOURCE_NOT_FOUND_MESSAGE
        } else {
            INVALID_TAGGING_ARN_MESSAGE
        };
        assert_eq!(
            message, expected_message,
            "case {case:?} ({resource_arn}): expected the tagging path's message, got {body:?}"
        );
    }

    // Nothing the refused calls carried reached either stored resource.
    let stored = stored_tags(&router, "after the refusals", Kind::Sink, &sink_arn, REGION).await;
    assert_eq!(
        stored,
        map(&[("real", "tag")]),
        "after the refusals: the sink must still carry only what was really stored, \
         got {stored:?}"
    );
    let stored = stored_tags(&router, "after the refusals", Kind::Link, &link_arn, REGION).await;
    assert!(
        stored.is_empty(),
        "after the refusals: the link was never tagged, got {stored:?}"
    );

    // A resource that is deleted stops being taggable, by the same rule.
    let (status, body) = post(
        &router,
        "/DeleteLink",
        &json!({ "Identifier": link_arn }),
        REGION,
    )
    .await;
    assert_eq!(status, 200, "DeleteLink: got {status} {body:?}");
    let (status, body) = tag(&router, &link_arn, &map(&[("after", "delete")]), REGION).await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (404, "NotFoundException"),
        "a deleted link: expected 404 NotFoundException, got {status} {code} {body:?}"
    );
}

/// Each resource has its own tag set: a tag written through one ARN is visible through
/// that ARN alone — across the two kinds, across two resources of the same kind, and
/// across Regions.
#[tokio::test]
async fn tags_belong_to_the_one_resource_their_arn_names() {
    let router = emulator();
    let (east_sink, east_link) = sink_and_link(&router, "east-sink", REGION).await;
    // "Each account can contain one sink per Region" (`CreateSink`), so a second sink
    // means a second Region.
    let (west_sink, west_link) = sink_and_link(&router, "west-sink", "eu-west-1").await;

    let targets = [
        ("the east sink", Kind::Sink, east_sink.clone(), REGION),
        ("the east link", Kind::Link, east_link.clone(), REGION),
        ("the west sink", Kind::Sink, west_sink.clone(), "eu-west-1"),
        ("the west link", Kind::Link, west_link.clone(), "eu-west-1"),
    ];

    // Every resource gets the SAME key with a value naming it, so a tag that leaked to
    // another record would be read back under the wrong value.
    for (case, _, arn, region) in &targets {
        let (status, body) = tag(&router, arn, &map(&[("owner", case)]), region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource expected 200, got {status} {body:?}"
        );
    }

    for (case, kind, arn, region) in &targets {
        let stored = stored_tags(&router, case, *kind, arn, region).await;
        assert_eq!(
            stored,
            map(&[("owner", case)]),
            "case {case:?}: expected only its own tag, got {stored:?}"
        );
    }

    // The label's two legal spellings name one resource: `:` percent-encoded (what an
    // SDK sends, used by every case above) and `:` left literal, which a path segment
    // also admits.
    for (case, kind, arn, region) in &targets {
        let spelled = format!("{case} addressed with literal colons");
        let (status, body) = tag_label(
            &router,
            &encode_slash_only(arn),
            &map(&[("spelling", "literal-colons")]),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {spelled:?}: TagResource expected 200, got {status} {body:?}"
        );
        let stored = stored_tags(&router, &spelled, *kind, arn, region).await;
        assert_eq!(
            stored,
            map(&[("owner", case), ("spelling", "literal-colons")]),
            "case {spelled:?}: the other spelling must reach the same record, got {stored:?}"
        );
    }
}
