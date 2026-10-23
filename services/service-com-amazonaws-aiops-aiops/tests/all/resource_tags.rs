// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for this service's tagging slice: `TagResource`,
//! `UntagResource` and `ListTagsForResource`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization (including the `@httpLabel` `resourceArn` and the
//! `@httpQuery` `tagKeys` list), the modelled-constraint walk and the handlers — so
//! what is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a resource's tags are a persisted key-value set belonging to
//! the resource its identifier names, `TagResource` writes it per key (a key already
//! present takes the new value, every other key is untouched), `UntagResource` removes
//! per key and is idempotent, and `ListTagsForResource` reports exactly that stored set
//! — empty when the resource carries no tags — without changing it. An identifier
//! naming no resource is a modelled 404 from all three.
//!
//! An account holds ONE investigation group per Region, so every row below that needs
//! its own group signs for its own Region ([`region`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_aiops_aiops::state::MAX_TAGS;
use smithy_server_runtime::MultiServiceRouter;

/// The region a case signs for when it only needs one.
const REGION: &str = "us-west-2";

/// Regions the rows that each need their own group sign for, in order.
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "eu-central-1",
    "eu-west-2",
    "ap-southeast-1",
    "ap-northeast-1",
    "sa-east-1",
    "ca-central-1",
];

/// The Region row `n` signs for.
///
/// An account holds one investigation group per Region, so a row that needs its own
/// group needs its own Region. The names are real AWS Regions; past the list they are
/// synthetic ones inside what a SigV4 credential scope admits (`[a-zA-Z0-9-]`).
fn region(n: usize) -> String {
    REGIONS
        .get(n)
        .map_or_else(|| format!("region-{n:03}"), |name| (*name).to_string())
}

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in `region`, the signal the router routes on.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{region}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
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

/// [`call_in`] in [`REGION`]: every operation below but the create resolves its
/// resource through an ARN, which names its own Region.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, uri, body).await
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode everything a URI component may not carry literally — what an SDK
/// does for an `@httpLabel` path segment and for an `@httpQuery` value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Create a group that must succeed in `region`, and answer its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, body: &Value) -> String {
    let (status, created) = call_in(router, region, "POST", "/investigationGroups", Some(body)).await;
    assert_eq!(
        status, 201,
        "create {body}: expected 201, got {status} / {created:?}"
    );
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {body}: response carried no arn, got {created:?}"))
        .to_string()
}

async fn list_tags(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", encode(identifier)), None).await
}

async fn tag(router: &MultiServiceRouter, identifier: &str, tags: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/tags/{}", encode(identifier)),
        Some(&json!({ "tags": tags })),
    )
    .await
}

async fn untag(router: &MultiServiceRouter, identifier: &str, keys: &[&str]) -> (u16, Value) {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode(key)))
        .collect();
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{}", encode(identifier), query.join("&")),
        None,
    )
    .await
}

/// The tag set `ListTagsForResource` reported, as a comparable map. An absent `tags`
/// member is reported as such so a case can tell it from an empty set.
fn reported_tags(body: &Value) -> Option<BTreeMap<String, String>> {
    let map = body.get("tags")?.as_object()?;
    Some(
        map.iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.as_str()
                        .unwrap_or_else(|| panic!("a tag value is a string, got {v:?}"))
                        .to_string(),
                )
            })
            .collect(),
    )
}

/// Read the tag set back, asserting the call itself succeeded.
async fn list_tags_ok(
    router: &MultiServiceRouter,
    identifier: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, body) = list_tags(router, identifier).await;
    assert_eq!(
        status,
        200,
        "case {case:?}: expected 200 from ListTagsForResource, got {status} / {} / {body:?}",
        error_code(&body)
    );
    reported_tags(&body).unwrap_or_else(|| {
        panic!(
            "case {case:?}: ListTagsForResource must always report a tag set, got {body:?}"
        )
    })
}

fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn tags_body(pairs: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_string(), Value::String((*value).to_string()));
    }
    Value::Object(map)
}

/// A group created in `region` with `tags`, answering its ARN.
async fn group_with_tags(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    tags: Option<&[(&str, &str)]>,
) -> String {
    let mut body = json!({ "name": name, "roleArn": ROLE_ARN });
    if let Some(pairs) = tags {
        body["tags"] = tags_body(pairs);
    }
    create_ok(router, region, &body).await
}

/// One row of the create-with-tags table: the case name and the `tags` map the create
/// sent, where `None` means the request carried no `tags` member at all.
type CreateCase<'a> = (&'a str, Option<Vec<(&'a str, &'a str)>>);

/// Whatever a create stored is exactly what `ListTagsForResource` reports — over tag
/// sets that differ in COUNT (none, the empty map, one, several, the published
/// per-resource maximum) and in the CHARACTERS a key and value may carry.
///
/// The character rows come from the modelled `TagKey`/`TagValue` pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$` and lengths 1..128 / 1..256:
/// `smithy_query(query="shape",shape="Tags")`.
#[tokio::test]
async fn list_tags_reports_exactly_the_tags_a_create_stored() {
    let router = emulator();

    let max_tags: Vec<(String, String)> = (0..MAX_TAGS)
        .map(|i| (format!("key{i:02}"), format!("value{i:02}")))
        .collect();
    let max_tags: Vec<(&str, &str)> = max_tags
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let cases: Vec<CreateCase> = vec![
        ("no tags member at all", None),
        ("an empty tags map", Some(vec![])),
        ("one tag", Some(vec![("Team", "ops")])),
        (
            "several tags",
            Some(vec![
                ("Team", "ops"),
                ("Environment", "prod"),
                ("CostCenter", "4711"),
            ]),
        ),
        (
            // Every punctuation character the modelled pattern admits, plus the
            // letter, number and separator classes it allows.
            "keys and values across the whole modelled character class",
            Some(vec![
                ("app.name/env:tier=1+2-3_x@y", "a.b/c:d=1+2-3_e@f"),
                ("Équipe Opérations", "производство 2026"),
                ("プロジェクト", "本番"),
            ]),
        ),
        (
            "a 1-character key and value, the modelled minimum length",
            Some(vec![("k", "v")]),
        ),
        (
            "the maximum number of tags one resource may carry",
            Some(max_tags.clone()),
        ),
    ];

    for (index, (case, pairs)) in cases.iter().enumerate() {
        let arn =
            group_with_tags(&router, &region(index), &format!("group-{index}"), pairs.as_deref())
                .await;
        let want = expected(pairs.as_deref().unwrap_or(&[]));
        let got = list_tags_ok(&router, &arn, case).await;
        assert_eq!(
            got, want,
            "case {case:?}: expected the stored tag set {want:?}, got {got:?}"
        );

        // A read may not change what the next caller sees.
        let again = list_tags_ok(&router, &arn, case).await;
        assert_eq!(
            again, want,
            "case {case:?}: a second read must report the same set {want:?}, got {again:?}"
        );
    }
}

/// `TagResource` then `UntagResource` act PER KEY on the stored set: added keys appear,
/// a key already present takes the new value, keys the request does not name are left
/// alone, and removal names only what it removes.
#[tokio::test]
async fn tagging_and_untagging_are_per_key_over_the_stored_set() {
    let router = emulator();

    struct Case<'a> {
        name: &'a str,
        at_create: &'a [(&'a str, &'a str)],
        add: &'a [(&'a str, &'a str)],
        after_add: &'a [(&'a str, &'a str)],
        remove: &'a [&'a str],
        after_remove: &'a [(&'a str, &'a str)],
    }

    let cases = [
        Case {
            name: "tagging an untagged resource, then removing the one tag",
            at_create: &[],
            add: &[("Team", "ops")],
            after_add: &[("Team", "ops")],
            remove: &["Team"],
            after_remove: &[],
        },
        Case {
            name: "adding beside an existing tag, then removing a subset",
            at_create: &[("Team", "ops")],
            add: &[("Environment", "prod"), ("CostCenter", "4711")],
            after_add: &[
                ("Team", "ops"),
                ("Environment", "prod"),
                ("CostCenter", "4711"),
            ],
            remove: &["Environment"],
            after_remove: &[("Team", "ops"), ("CostCenter", "4711")],
        },
        Case {
            name: "a key already present takes the new value",
            at_create: &[("Team", "ops"), ("Environment", "staging")],
            add: &[("Environment", "prod")],
            after_add: &[("Team", "ops"), ("Environment", "prod")],
            remove: &["Team"],
            after_remove: &[("Environment", "prod")],
        },
        Case {
            name: "removing a key the resource never carried changes nothing",
            at_create: &[("Team", "ops")],
            add: &[("Environment", "prod")],
            after_add: &[("Team", "ops"), ("Environment", "prod")],
            remove: &["NeverSet"],
            after_remove: &[("Team", "ops"), ("Environment", "prod")],
        },
        Case {
            name: "removing every key, named and unnamed together",
            at_create: &[("Team", "ops")],
            add: &[("Environment", "prod")],
            after_add: &[("Team", "ops"), ("Environment", "prod")],
            remove: &["Team", "Environment", "NeverSet"],
            after_remove: &[],
        },
        Case {
            name: "an empty tags map adds nothing",
            at_create: &[("Team", "ops")],
            add: &[],
            after_add: &[("Team", "ops")],
            remove: &["Team"],
            after_remove: &[],
        },
        Case {
            name: "keys across the modelled character class survive a round trip",
            at_create: &[("app.name/env:tier=1+2-3_x@y", "one")],
            add: &[("Équipe Opérations", "deux"), ("プロジェクト", "三")],
            after_add: &[
                ("app.name/env:tier=1+2-3_x@y", "one"),
                ("Équipe Opérations", "deux"),
                ("プロジェクト", "三"),
            ],
            remove: &["Équipe Opérations"],
            after_remove: &[
                ("app.name/env:tier=1+2-3_x@y", "one"),
                ("プロジェクト", "三"),
            ],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let name = case.name;
        let arn = group_with_tags(
            &router,
            &region(index),
            &format!("group-{index}"),
            Some(case.at_create),
        )
        .await;

        let (status, body) = tag(&router, &arn, &tags_body(case.add)).await;
        assert_eq!(
            status,
            200,
            "case {name:?}: expected 200 from TagResource, got {status} / {} / {body:?}",
            error_code(&body)
        );

        let got = list_tags_ok(&router, &arn, name).await;
        let want = expected(case.after_add);
        assert_eq!(
            got, want,
            "case {name:?}: after tagging, expected {want:?}, got {got:?}"
        );

        let (status, body) = untag(&router, &arn, case.remove).await;
        assert_eq!(
            status,
            200,
            "case {name:?}: expected 200 from UntagResource, got {status} / {} / {body:?}",
            error_code(&body)
        );

        let got = list_tags_ok(&router, &arn, name).await;
        let want = expected(case.after_remove);
        assert_eq!(
            got, want,
            "case {name:?}: after untagging, expected {want:?}, got {got:?}"
        );

        // Removal is idempotent: repeating it is a success that removes nothing more.
        let (status, body) = untag(&router, &arn, case.remove).await;
        assert_eq!(
            status,
            200,
            "case {name:?}: a repeated UntagResource must succeed, got {status} / {} / {body:?}",
            error_code(&body)
        );
        let got = list_tags_ok(&router, &arn, name).await;
        assert_eq!(
            got, want,
            "case {name:?}: a repeated UntagResource must leave {want:?}, got {got:?}"
        );
    }
}

/// `resourceArn` resolves the investigation group through its ARN and through nothing
/// else: the bare forms the modelled identifier pattern also admits — the group's name
/// and its bare id — name no resource, so all three tagging operations report a
/// modelled 404 for them while the ARN addresses the stored set.
#[tokio::test]
async fn only_the_arn_addresses_the_tag_set() {
    let router = emulator();
    let arn = group_with_tags(&router, REGION, "shared-tags", Some(&[("Team", "ops")])).await;
    let id = arn
        .rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("a group ARN carries an id segment, got {arn:?}"))
        .to_string();

    // The ARN is the address: a write through it is visible to a read through it.
    let (status, body) = tag(&router, &arn, &tags_body(&[("WrittenViaArn", "yes")])).await;
    assert_eq!(
        status,
        200,
        "case \"tag via the ARN\": expected 200, got {status} / {} / {body:?}",
        error_code(&body)
    );
    let got = list_tags_ok(&router, &arn, "written via the ARN").await;
    assert_eq!(
        got,
        expected(&[("Team", "ops"), ("WrittenViaArn", "yes")]),
        "case \"written via the ARN\": expected the stored set, got {got:?}"
    );

    // The bare forms address nothing — on every one of the three operations.
    for (via, identifier) in [("the name", "shared-tags".to_string()), ("the bare id", id)] {
        for (operation, (status, body)) in [
            ("ListTagsForResource", list_tags(&router, &identifier).await),
            (
                "TagResource",
                tag(&router, &identifier, &tags_body(&[("WrittenViaLabel", "no")])).await,
            ),
            ("UntagResource", untag(&router, &identifier, &["Team"]).await),
        ] {
            assert_eq!(
                status, 404,
                "case {via:?} on {operation}: expected 404, got {status} / {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {via:?} on {operation}: expected ResourceNotFoundException, got {body:?}"
            );
        }
    }

    // None of those refused calls touched the stored set.
    let after = list_tags_ok(&router, &arn, "after the refused calls").await;
    assert_eq!(
        after,
        expected(&[("Team", "ops"), ("WrittenViaArn", "yes")]),
        "a refused call must leave the stored set alone, got {after:?}"
    );
}

/// The per-resource tag maximum applies to the set the resource ENDS UP with, and a
/// refused `TagResource` stores nothing. Rows: at the published maximum, one past it,
/// and a request that only replaces values at the maximum (which does not grow the set).
///
/// The bound is the `tags` member's own documentation ("You can associate as many as 50
/// tags with an investigation group"), surfaced as `state::MAX_TAGS`; the refusal is the
/// tagging frontend's HTTP 400 `BadRequestException` rather than this service's own
/// `ValidationException`, which is what a 51-tag call is recorded answering.
#[tokio::test]
async fn the_per_resource_tag_maximum_bounds_the_resulting_set() {
    let router = emulator();

    let pairs: Vec<(String, String)> = (0..MAX_TAGS)
        .map(|i| (format!("key{i:02}"), format!("value{i:02}")))
        .collect();
    let at_max: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // One tag short of the maximum, so the next distinct key lands exactly on it.
    let arn = group_with_tags(&router, REGION, "filling-up", Some(&at_max[..MAX_TAGS - 1])).await;

    let last = at_max[MAX_TAGS - 1];
    let (status, body) = tag(&router, &arn, &tags_body(&[last])).await;
    assert_eq!(
        status,
        200,
        "case \"the key that lands on the maximum\": expected 200, got {status} / {} / {body:?}",
        error_code(&body)
    );
    let got = list_tags_ok(&router, &arn, "at the maximum").await;
    assert_eq!(
        got,
        expected(&at_max),
        "case \"at the maximum\": expected all {MAX_TAGS} tags, got {} of them: {got:?}",
        got.len()
    );

    // One distinct key past the maximum is refused, and stores nothing.
    let (status, body) = tag(&router, &arn, &tags_body(&[("OneTooMany", "no")])).await;
    assert_eq!(
        status, 400,
        "case \"one key past the maximum\": expected 400, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "BadRequestException",
        "case \"one key past the maximum\": expected BadRequestException, got {body:?}"
    );
    let after = list_tags_ok(&router, &arn, "one key past the maximum").await;
    assert_eq!(
        after,
        expected(&at_max),
        "case \"one key past the maximum\": a refused TagResource must store nothing, got {after:?}"
    );

    // At the maximum, replacing the VALUE of keys already present is still accepted:
    // the resulting set is the same size.
    let replacements: Vec<(&str, &str)> = at_max
        .iter()
        .take(3)
        .map(|(key, _)| (*key, "replaced"))
        .collect();
    let (status, body) = tag(&router, &arn, &tags_body(&replacements)).await;
    assert_eq!(
        status,
        200,
        "case \"replacing values at the maximum\": expected 200, got {status} / {} / {body:?}",
        error_code(&body)
    );
    let got = list_tags_ok(&router, &arn, "replacing values at the maximum").await;
    assert_eq!(
        got.len(),
        MAX_TAGS,
        "case \"replacing values at the maximum\": the set must still hold {MAX_TAGS} tags, \
         got {got:?}"
    );
    for (key, _) in &replacements {
        assert!(
            got.get(*key).map(String::as_str) == Some("replaced"),
            "case \"replacing values at the maximum\": expected {key:?} = \"replaced\", got {got:?}"
        );
    }
}

/// An identifier naming no investigation group is a modelled 404 from each of the three
/// tagging operations, whatever form it takes — and the tag set goes with the resource
/// when it is deleted.
#[tokio::test]
async fn an_unknown_resource_is_a_modelled_404() {
    let router = emulator();
    let arn = group_with_tags(&router, REGION, "doomed", Some(&[("Team", "ops")])).await;
    let id = arn
        .rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("a group ARN carries an id segment, got {arn:?}"))
        .to_string();

    let unknown = [
        ("an unknown name", "no-such-group".to_string()),
        ("an unknown id", "AAAAAAAAAAAAAAAA".to_string()),
        (
            "an ARN with an unknown id",
            format!("arn:aws:aiops:{REGION}:000000000000:investigation-group/BBBBBBBBBBBBBBBB"),
        ),
        (
            "this group's id under another region",
            format!("arn:aws:aiops:eu-west-1:000000000000:investigation-group/{id}"),
        ),
        (
            "this group's id under another account",
            format!("arn:aws:aiops:{REGION}:111111111111:investigation-group/{id}"),
        ),
    ];

    for (case, identifier) in &unknown {
        for (operation, (status, body)) in [
            ("ListTagsForResource", list_tags(&router, identifier).await),
            (
                "TagResource",
                tag(&router, identifier, &tags_body(&[("Team", "ops")])).await,
            ),
            ("UntagResource", untag(&router, identifier, &["Team"]).await),
        ] {
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: expected 404, got {status} / {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {body:?}"
            );
        }
    }

    // A refused TagResource on an unknown identifier created nothing to read.
    let (status, body) = call(
        &router,
        "GET",
        &format!("/investigationGroups/{}", encode("no-such-group")),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "a refused tag must not have created a group, got {status} / {body:?}"
    );

    // The tag set belongs to the resource, so deleting the resource takes it away.
    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/investigationGroups/{}", encode(&arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting the group: expected 200, got {status} / {body:?}"
    );
    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 404,
        "case \"after the group is deleted\": expected 404, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"after the group is deleted\": expected ResourceNotFoundException, got {body:?}"
    );
}

/// Tags stored on one resource are that resource's own: tagging or untagging one never
/// reaches another.
#[tokio::test]
async fn tag_sets_are_per_resource() {
    let router = emulator();
    let first = group_with_tags(&router, &region(0), "first", Some(&[("Team", "ops")])).await;
    let second = group_with_tags(
        &router,
        &region(1),
        "second",
        Some(&[("Team", "research")]),
    )
    .await;

    let (status, body) = tag(&router, &first, &tags_body(&[("Environment", "prod")])).await;
    assert_eq!(
        status,
        200,
        "tagging the first group: expected 200, got {status} / {} / {body:?}",
        error_code(&body)
    );
    let (status, body) = untag(&router, &second, &["Team"]).await;
    assert_eq!(
        status,
        200,
        "untagging the second group: expected 200, got {status} / {} / {body:?}",
        error_code(&body)
    );

    let got = list_tags_ok(&router, &first, "the first group").await;
    assert_eq!(
        got,
        expected(&[("Team", "ops"), ("Environment", "prod")]),
        "case \"the first group\": only its own tags may change, got {got:?}"
    );
    let got = list_tags_ok(&router, &second, "the second group").await;
    assert_eq!(
        got,
        expected(&[]),
        "case \"the second group\": only its own tags may change, got {got:?}"
    );
}
