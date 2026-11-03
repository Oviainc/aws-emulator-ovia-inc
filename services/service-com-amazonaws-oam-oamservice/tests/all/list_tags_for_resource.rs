// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListTagsForResource`.
//!
//! The rule under test: `ListTagsForResource` reports exactly the tag map PERSISTED on
//! the resource its `ResourceArn` names — a sink or a link, since "both sinks and
//! links support tagging" — reading it off that resource's one stored record, and an
//! ARN that names no stored resource of the Region the request was signed for is the
//! modelled `ResourceNotFoundException` rather than an empty tag list.
//!
//! Every case drives the real wire path (the production `MultiServiceRouter`, the
//! restJson1 REST binding this operation's `GET /tags/{ResourceArn}` route uses, the
//! modelled-constraint walk and the handlers), so what is asserted is what a signed
//! AWS SDK request observes.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::sinks_and_links::{
    call, call_as, dispatch_parsed, emulator, error_code, field, ACCOUNT, AKID, MONITORING_AKID,
};

/// A second access key id, so a case can attach a second link to one sink: a link
/// belongs to the source account its request speaks for, and a source account may
/// hold only one link per sink.
const OTHER_AKID: &str = "AKIDOAMTEST2";

/// The message every not-found refusal this service answers carries, as
/// `state::RESOURCE_NOT_FOUND_MESSAGE` states it: a fixed string that names nothing
/// from the request.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found.";

/// `value` as ONE URI path segment: every byte outside RFC 3986's unreserved set
/// (`A-Z`, `a-z`, `0-9`, `-`, `.`, `_`, `~`) percent-encoded.
///
/// That is what an AWS SDK sends for a non-greedy `@httpLabel`, and it is exactly
/// what the framework's own path decoder (`percent_decode_path` in
/// `vendor/smithy-schema-server/src/http_binding.rs`) reads back, so the handler sees
/// the ARN decoded. Encoding `/` is what makes the ARN ONE segment rather than two.
///
/// `pub(crate)` so the sibling `UntagResource` suite encodes a wire value — a path
/// label or an `@httpQuery` entry — through this one encoder rather than a second copy
/// of it.
pub(crate) fn percent_encode_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            // Uppercase hex, two digits, as RFC 3986 percent-encoding writes it.
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// The `GET /tags/{ResourceArn}` request an AWS SDK sends, signed for `region` by
/// `akid`.
///
/// `ResourceArn` is a NON-greedy `@httpLabel`
/// (`smithy_query(query="operation",operation="ListTagsForResource")` reports
/// `GET /tags/{ResourceArn}`), so the ARN travels as ONE path segment with the
/// characters an ARN carries that a path segment cannot — `:` and `/` — percent-encoded.
fn tags_request(resource_arn: &str, akid: &str, region: &str) -> http::Request<bytes::Bytes> {
    let label = percent_encode_segment(resource_arn);
    http::Request::builder()
        .method("GET")
        .uri(format!("/tags/{label}"))
        .header("host", format!("oam.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{region}/oam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed test request")
}

/// Status and parsed body of one `ListTagsForResource` call signed for `region`.
///
/// `pub(crate)` so the sibling `UntagResource` suite reads a resource's tags back
/// through the very operation that reports them.
pub(crate) async fn list_tags(
    router: &MultiServiceRouter,
    resource_arn: &str,
    region: &str,
) -> (u16, Value) {
    list_tags_as(router, resource_arn, AKID, region).await
}

/// Status and parsed body of one `ListTagsForResource` call signed by `akid`.
async fn list_tags_as(
    router: &MultiServiceRouter,
    resource_arn: &str,
    akid: &str,
    region: &str,
) -> (u16, Value) {
    dispatch_parsed(router, tags_request(resource_arn, akid, region)).await
}

/// A tag map as the wire carries it.
fn tags_json(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// A tag map from `(key, value)` pairs.
fn tag_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Create a sink carrying `tags`, owned by the monitoring account
/// ([`MONITORING_AKID`]) so the source accounts below can link to it, which must
/// succeed, and answer its ARN.
async fn create_tagged_sink(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    tags: &BTreeMap<String, String>,
    region: &str,
) -> String {
    let mut body = json!({ "Name": name });
    if !tags.is_empty() {
        body["Tags"] = tags_json(tags);
    }
    let (status, created) = call_as(router, "/CreateSink", &body, MONITORING_AKID, region).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from CreateSink, got {status} {created:?}"
    );
    field(case, &created, "Arn")
}

/// Create a link to `sink_arn` carrying `tags`, which must succeed, and answer its
/// ARN.
async fn create_tagged_link(
    router: &MultiServiceRouter,
    case: &str,
    sink_arn: &str,
    tags: &BTreeMap<String, String>,
    akid: &str,
    region: &str,
) -> String {
    let mut body = json!({
        "LabelTemplate": "$AccountName",
        "ResourceTypes": ["AWS::CloudWatch::Metric"],
        "SinkIdentifier": sink_arn,
    });
    if !tags.is_empty() {
        body["Tags"] = tags_json(tags);
    }
    let (status, created) = call_as(router, "/CreateLink", &body, akid, region).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from CreateLink, got {status} {created:?}"
    );
    field(case, &created, "Arn")
}

/// The tag sets the cases span.
///
/// Rows differ along what the rule generalises over: how MANY tags the resource
/// carries (none, one, several), the characters and the LENGTH of a key and a value —
/// `TagKey` is `@length 1..128` and `TagValue` `@length 0..256`
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`), so a row sits on each of
/// those bounds — and the order the keys were supplied in.
fn tag_cases() -> Vec<(&'static str, BTreeMap<String, String>, &'static str)> {
    vec![
        ("no tags at all", BTreeMap::new(), "us-west-2"),
        ("one tag", tag_map(&[("env", "prod")]), "us-east-1"),
        (
            "several tags, supplied out of key order",
            tag_map(&[("zone", "c"), ("env", "prod"), ("team", "observability")]),
            "eu-central-1",
        ),
        (
            "empty value (TagValue @length min 0)",
            tag_map(&[("owner", "")]),
            "ap-southeast-2",
        ),
        (
            "maximum-length key and value (128 / 256)",
            BTreeMap::from([("k".repeat(128), "v".repeat(256))]),
            "sa-east-1",
        ),
        (
            "punctuation and non-ASCII in key and value",
            tag_map(&[("cost:centre/eu", "coût 100%"), ("équipe", "données")]),
            "eu-west-1",
        ),
    ]
}

#[tokio::test]
async fn list_tags_for_resource_reports_the_tags_stored_on_a_sink_or_a_link() {
    for (case, tags, region) in tag_cases() {
        let router = emulator();
        let expected = tags_json(&tags);

        // Both resource kinds, tagged with the same set, in one store: the operation
        // answers each ARN from that ARN's own stored record.
        let sink_arn = create_tagged_sink(&router, case, "tagged-sink", &tags, region).await;
        let link_arn =
            create_tagged_link(&router, case, &sink_arn, &tags, AKID, region).await;

        for (kind, arn) in [("sink", &sink_arn), ("link", &link_arn)] {
            let (status, listed) = list_tags(&router, arn, region).await;
            assert_eq!(
                status, 200,
                "case {case:?} ({kind}): expected 200, got {status} {listed:?}"
            );
            assert_eq!(
                listed.get("Tags"),
                Some(&expected),
                "case {case:?} ({kind}): expected the stored tags {expected:?}, got {listed:?}"
            );

            // The operation is `@readonly`: reading the tags twice answers the same
            // thing, and leaves what `GetSink`/`GetLink` report untouched.
            let (second_status, second) = list_tags(&router, arn, region).await;
            assert_eq!(
                (second_status, second.get("Tags")),
                (200, Some(&expected)),
                "case {case:?} ({kind}): expected a second read to answer identically, \
                 got {second_status} {second:?}"
            );
        }

        // The same stored record is what the two readers of this slice report, so
        // `GetSink`/`GetLink` with `IncludeTags` agree with it. They omit an EMPTY tag
        // map (`Tags` is optional there and the caller asked for tags a resource has
        // none of), which is why the empty row expects no member from them.
        let include_tags = (!tags.is_empty()).then_some(&expected);
        for (operation, identifier) in [("/GetSink", &sink_arn), ("/GetLink", &link_arn)] {
            let (status, got) = call(
                &router,
                operation,
                &json!({ "Identifier": identifier, "IncludeTags": true }),
                region,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} ({operation}): expected 200, got {status} {got:?}"
            );
            assert_eq!(
                got.get("Tags"),
                include_tags,
                "case {case:?} ({operation}): expected the same stored tags the tag list \
                 reported, got {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn list_tags_for_resource_reports_each_resource_its_own_tags() {
    let region = "us-west-2";
    let router = emulator();

    // Three resources in one store, each with a DIFFERENT tag set, including one
    // carrying none: every ARN must answer from its own record and no other.
    let sink_tags = tag_map(&[("kind", "sink"), ("env", "prod")]);
    let first_link_tags = tag_map(&[("kind", "link"), ("index", "1")]);
    let second_link_tags = BTreeMap::new();

    let sink_arn = create_tagged_sink(&router, "mixed", "mixed-sink", &sink_tags, region).await;
    let first_link_arn =
        create_tagged_link(&router, "mixed", &sink_arn, &first_link_tags, AKID, region).await;
    // A second source account, because a source account may hold only one link per
    // sink.
    let second_link_arn = create_tagged_link(
        &router,
        "mixed",
        &sink_arn,
        &second_link_tags,
        OTHER_AKID,
        region,
    )
    .await;

    let cases = [
        ("sink", &sink_arn, &sink_tags),
        ("first link", &first_link_arn, &first_link_tags),
        ("second link", &second_link_arn, &second_link_tags),
    ];
    for (case, arn, tags) in cases {
        let expected = tags_json(tags);
        // Read from the source account that did NOT create the link too: the tags are
        // a property of the resource, not of who asks for them.
        for akid in [AKID, OTHER_AKID] {
            let (status, listed) = list_tags_as(&router, arn, akid, region).await;
            assert_eq!(
                status, 200,
                "case {case:?} as {akid}: expected 200, got {status} {listed:?}"
            );
            assert_eq!(
                listed.get("Tags"),
                Some(&expected),
                "case {case:?} as {akid}: expected {expected:?}, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn list_tags_for_resource_refuses_an_arn_naming_no_stored_resource() {
    let region = "us-west-2";
    let router = emulator();

    let sink_arn = create_tagged_sink(
        &router,
        "not found",
        "present-sink",
        &tag_map(&[("env", "prod")]),
        region,
    )
    .await;
    let link_arn = create_tagged_link(
        &router,
        "not found",
        &sink_arn,
        &tag_map(&[("env", "prod")]),
        AKID,
        region,
    )
    .await;
    let sink_id = sink_arn
        .rsplit('/')
        .next()
        .expect("the sink ARN carries an id segment")
        .to_string();

    // A sink created in another Region, deleted again, so its ARN is well formed and
    // names a resource this store once held.
    let other_region = "eu-central-1";
    let deleted_arn = create_tagged_sink(
        &router,
        "not found",
        "deleted-sink",
        &BTreeMap::new(),
        other_region,
    )
    .await;
    let (delete_status, deleted) = call(
        &router,
        "/DeleteSink",
        &json!({ "Identifier": deleted_arn }),
        other_region,
    )
    .await;
    assert_eq!(
        delete_status, 200,
        "DeleteSink: expected 200, got {delete_status} {deleted:?}"
    );

    // Rows differ along WHY the ARN resolves to nothing: a never-minted id, a deleted
    // resource, the wrong ARN field (partition, account, region), the wrong resource
    // kind, and the right id under the other kind's prefix.
    let never_minted = "11111111-2222-4333-8444-555555555555";
    let cases: &[(&str, String, &str)] = &[
        (
            "never-created sink",
            format!("arn:aws:oam:{region}:{ACCOUNT}:sink/{never_minted}"),
            region,
        ),
        (
            "never-created link",
            format!("arn:aws:oam:{region}:{ACCOUNT}:link/{never_minted}"),
            region,
        ),
        ("deleted sink", deleted_arn.clone(), other_region),
        (
            "stored sink read from another Region's endpoint",
            sink_arn.clone(),
            other_region,
        ),
        (
            "another account's ARN",
            format!("arn:aws:oam:{region}:000000000001:sink/{sink_id}"),
            region,
        ),
        (
            "another partition's ARN",
            format!("arn:aws-us-gov:oam:{region}:{ACCOUNT}:sink/{sink_id}"),
            region,
        ),
        (
            "the stored sink's id under the link prefix",
            format!("arn:aws:oam:{region}:{ACCOUNT}:link/{sink_id}"),
            region,
        ),
        (
            "an oam ARN of neither taggable kind",
            format!("arn:aws:oam:{region}:{ACCOUNT}:policy/{sink_id}"),
            region,
        ),
    ];

    for (case, arn, signed_region) in cases {
        let (status, refused) = list_tags(&router, arn, signed_region).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused.get("Message").and_then(Value::as_str),
            Some(RESOURCE_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the constant not-found message, got {refused:?}"
        );
        assert_eq!(
            refused.get("Tags"),
            None,
            "case {case:?}: expected no tag list on a refusal, got {refused:?}"
        );
    }

    // The refusals changed nothing: both stored resources still report their tags.
    for (kind, arn) in [("sink", &sink_arn), ("link", &link_arn)] {
        let (status, listed) = list_tags(&router, arn, region).await;
        assert_eq!(
            (status, listed.get("Tags")),
            (200, Some(&tags_json(&tag_map(&[("env", "prod")])))),
            "{kind}: expected the stored tags after the refusals, got {status} {listed:?}"
        );
    }
}

#[tokio::test]
async fn list_tags_for_resource_refuses_a_resource_arn_the_model_does_not_admit() {
    let region = "us-west-2";
    let router = emulator();
    let sink_arn =
        create_tagged_sink(&router, "pattern", "pattern-sink", &BTreeMap::new(), region).await;

    // `ResourceArn` targets `Arn`, `@pattern ^arn:(\w|-)+:oam:.+:.+:.+`
    // (`smithy_query(query="shape",shape="Arn")`). Rows differ along WHICH part of
    // that pattern the value misses, and the refusal is the `ValidationException`
    // these three tagging operations model — not the `InvalidParameterException`
    // every other operation of this service declares.
    let cases: &[(&str, &str)] = &[
        ("not an ARN at all", "demo-sink"),
        ("another service's ARN", "arn:aws:s3:::my-bucket"),
        ("empty partition", "arn::oam:us-west-2:000000000000:sink/x"),
        ("too few ARN fields", "arn:aws:oam:us-west-2:sink"),
    ];

    for (case, resource_arn) in cases {
        let (status, refused) = list_tags(&router, resource_arn, region).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
    }

    // A refused request is not a read: the stored sink still answers.
    let (status, listed) = list_tags(&router, &sink_arn, region).await;
    assert_eq!(
        (status, listed.get("Tags")),
        (200, Some(&Value::Object(Map::new()))),
        "expected the untagged sink's empty tag list, got {status} {listed:?}"
    );
}
