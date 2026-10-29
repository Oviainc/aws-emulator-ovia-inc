// SPDX-License-Identifier: CC0-1.0

//! Every `TagResource` and `UntagResource` call leaves one attributable record.
//!
//! The rule under test, over a TABLE rather than one recorded request: a call that changes
//! — or is refused the chance to change — a resource's tag set leaves EXACTLY ONE
//! structured record on the audit target, naming the operation, the caller (the access key
//! id and the credential-scope region the request presented), the resource the call
//! addressed and, once it resolved, the stored record's own id, WHICH tag keys the request
//! named, how many, and the outcome. A state-changing call that changed nothing is still
//! attributable, so the refusal paths report their outcome too; and no record carries a tag
//! VALUE, which is caller content the record has no reason to hold.
//!
//! The rows differ along what the rule generalises over: which of the two writes made the
//! call, how many keys it named, whether a key is new to the resource or already carried,
//! the form of the key (the modelled pattern's whole character set, whitespace included),
//! whether a removed key was carried at all, which refusal the call met (a reserved key,
//! an ARN naming no resource of this account, a write past the per-resource tag ceiling),
//! and which region the request was signed for.
//!
//! The responses themselves — status, body and the persisted tag set — are
//! `resource_tag_writes.rs`'s subject; this suite re-reads the tag set at the end only to
//! pin that recording a write does not change what it wrote.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::audit_capture::audit_capture;

/// The access key id every call in this suite signs with: no other suite uses it, so this
/// suite's records can be told apart from a sibling's in the process-wide capture, which
/// is what lets each row demand exactly one record.
const AUDIT_AKID: &str = "AKIDAUDITTAGWRITES";

/// The region the inquiries are created in, and the one every call that addresses them is
/// signed for: an inquiry's ARN names the region of the request that created it.
const REGION: &str = "eu-west-1";

/// A second region, which one refusal row signs for — so the record's `region` has to come
/// from the request's credential scope rather than from a constant.
const OTHER_REGION: &str = "ap-southeast-2";

/// How many tags one resource holds at most, from `TagsMap`'s modelled `@length(max: 50)`
/// (`smithy_query(query="shape",shape="TagsMap")`). Spelled out here rather than imported
/// from the crate, so a wrong constant in `src/state.rs` cannot make this suite pass.
const TAG_CEILING: usize = 50;

/// The tag-key namespace AWS keeps for its own tags: a key under it is refused by both
/// writes, before the ARN is resolved.
const RESERVED_PREFIX: &str = "aws:";

/// Every character `TagKey`'s pattern `^[a-zA-Z0-9\s_.:/=+\-@]*$` admits beside the
/// alphanumerics, gathered into one key: `_ . : / = + - @` and a space (`\s`) — a key whose
/// whitespace a one-line record has to survive.
const PATTERN_CHARS_KEY: &str = "a_b.c:d/e=f+g-h@i j";

/// The marker every tag VALUE this suite writes carries. No record may contain it: a
/// record names the keys a call wrote, never the values.
const VALUE_MARKER: &str = "value-no-record-may-carry";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 credential scope for `region` naming this service, presenting [`AUDIT_AKID`].
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={AUDIT_AKID}/20260101/{region}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

/// Dispatch one request through the router, signed for `region`.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", signed_for(region))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// `resourceArn` is an `@httpLabel` on `/tags/{resourceArn}`, and a non-greedy label is ONE
/// path segment: the `/` inside an ARN's resource field has to be written `%2F`.
fn label(resource_arn: &str) -> String {
    resource_arn.replace('/', "%2F")
}

/// One query-string value, percent-encoded through the crate this service already depends
/// on: a tag key may carry `/`, `=`, `+`, `:` and whitespace, each of which means
/// something else in a query string.
fn query_value(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// `POST /tags/{resourceArn}` — `TagResource`.
async fn tag(
    router: &MultiServiceRouter,
    region: &str,
    resource_arn: &str,
    tags: &[(String, String)],
) -> Answer {
    let body = Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    );
    call(
        router,
        region,
        "POST",
        &format!("/tags/{}", label(resource_arn)),
        Some(json!({ "tags": body })),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is a `@httpQuery`
/// list, so each key is one repetition of the parameter.
async fn untag(
    router: &MultiServiceRouter,
    region: &str,
    resource_arn: &str,
    keys: &[String],
) -> Answer {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect();
    call(
        router,
        region,
        "DELETE",
        &format!("/tags/{}?{}", label(resource_arn), query.join("&")),
        None,
    )
    .await
}

/// The tag set `ListTagsForResource` reports for `resource_arn`.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> BTreeMap<String, String> {
    let answer = call(
        router,
        REGION,
        "GET",
        &format!("/tags/{}", label(resource_arn)),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "reading the tag set back must answer HTTP 200, got {} with {:?}",
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

/// Create one compliance inquiry and answer its id and the ARN the service minted for it —
/// the only resource an account owns in this service, so the only one a tag write
/// addresses.
async fn create_inquiry(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = call(
        router,
        REGION,
        "POST",
        "/v1/compliance-inquiry/create",
        Some(json!({
            "name": name,
            "inquiryContent": { "query": "Which controls does this report cover?" },
            "supportMode": "AI_ONLY"
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "creating {name:?} must answer HTTP 202, got {} with {:?}",
        created.status, created.body
    );
    let summary = &created.body["complianceInquirySummary"];
    let id = summary["id"].as_str().unwrap_or_default().to_string();
    let arn = summary["arn"].as_str().unwrap_or_default().to_string();
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "creating {name:?} must report an id and an ARN, got {:?}",
        created.body
    );
    (id, arn)
}

/// A tag value carrying [`VALUE_MARKER`], distinguished by `n`.
fn value(n: usize) -> String {
    format!("{VALUE_MARKER}-{n}")
}

/// `&[(&str, &str)]` as owned tag pairs.
fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Whether `record` carries `pair` as a WHOLE field: the renderer writes one space before
/// every `name=value`, so the pair has to start at such a space and end at the next one or
/// at the end of the line — which is what keeps `tag_count=5` from being satisfied by
/// `tag_count=50`, or `removed=2` by `removed=20`.
fn carries(record: &str, pair: &str) -> bool {
    let field = format!(" {pair}");
    record.match_indices(&field).any(|(at, matched)| {
        record[at + matched.len()..]
            .chars()
            .next()
            .is_none_or(|next| next == ' ')
    })
}

/// The keys `tags` names, sorted and comma-joined — the form a record states them in.
///
/// Recomputed here rather than imported from `crate::state::tag_key_list`, on purpose: the
/// expectation has to be independent of the code under test, or a change to that ordering
/// rule would move the record and the assertion together and be caught nowhere.
fn joined_keys<'a, I: IntoIterator<Item = &'a str>>(keys: I) -> String {
    let mut keys: Vec<&str> = keys.into_iter().collect();
    keys.sort_unstable();
    keys.join(",")
}

/// Which call a row makes.
enum Call {
    /// `TagResource` with these key/value pairs.
    Tag(Vec<(String, String)>),
    /// `UntagResource` naming these keys.
    Untag(Vec<String>),
}

impl Call {
    /// The operation name the record must carry.
    fn operation(&self) -> &'static str {
        match self {
            Call::Tag(_) => "TagResource",
            Call::Untag(_) => "UntagResource",
        }
    }

    /// The keys the request names, as the record states them.
    fn keys(&self) -> String {
        match self {
            Call::Tag(tags) => joined_keys(tags.iter().map(|(key, _)| key.as_str())),
            Call::Untag(keys) => joined_keys(keys.iter().map(String::as_str)),
        }
    }

    /// How many keys the request names.
    fn count(&self) -> usize {
        match self {
            Call::Tag(tags) => tags.len(),
            Call::Untag(keys) => keys.len(),
        }
    }
}

/// Which resource a row addresses.
enum Target {
    /// The inquiry this suite tags and untags.
    Inquiry,
    /// A second inquiry, filled to the per-resource tag ceiling.
    Full,
    /// A well-formed ARN this account holds no resource for.
    Absent,
}

/// One row: a call, and the record it must leave.
struct Case {
    /// What this row exercises — named in every assertion.
    case: &'static str,
    call: Call,
    target: Target,
    /// The region the request is signed for.
    region: &'static str,
    /// The status the call must answer.
    status: u16,
    /// The `outcome` the record must report.
    outcome: &'static str,
    /// Whether the record must name the stored resource's own id: it does exactly when the
    /// call got as far as resolving the ARN to a resource.
    resolved: bool,
    /// How many tags the resource holds after the call, when the record reports it.
    tags_on_resource: Option<usize>,
    /// How many of the named keys the resource actually carried, for a removal.
    removed: Option<usize>,
}

#[tokio::test]
async fn every_tag_write_emits_exactly_one_attributable_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a failure below can never be mistaken for a missing record:
    // the capture must receive an event emitted right here.
    tracing::info!(target: "emulator::audit", akid = AUDIT_AKID, "probe");
    let probe = captured.drain_for(AUDIT_AKID);
    assert_eq!(
        probe.len(),
        1,
        "case \"subscriber self-test\": the capture must receive events, got {probe:?}"
    );

    let router = emulator();
    let (inquiry_id, inquiry_arn) = create_inquiry(&router, "the tagged inquiry").await;
    let (full_id, full_arn) = create_inquiry(&router, "the inquiry at its tag ceiling").await;
    let absent_arn = format!(
        "arn:aws:artifact:{REGION}:{}:compliance-inquiry/compliance-inquiry-0000000000000000",
        engine::emulator_account_id()
    );

    // The keys that fill a resource to the modelled ceiling, written in ONE call.
    let ceiling_tags: Vec<(String, String)> = (0..TAG_CEILING)
        .map(|n| (format!("ceiling{n:02}"), value(n)))
        .collect();

    let cases = vec![
        Case {
            case: "one new key",
            call: Call::Tag(pairs(&[("team", &value(1))])),
            target: Target::Inquiry,
            region: REGION,
            status: 204,
            outcome: "recorded",
            resolved: true,
            tags_on_resource: Some(1),
            removed: None,
        },
        Case {
            case: "three new keys in one call",
            call: Call::Tag(pairs(&[
                ("env", &value(2)),
                ("owner", &value(3)),
                ("cost.center", &value(4)),
            ])),
            target: Target::Inquiry,
            region: REGION,
            status: 204,
            outcome: "recorded",
            resolved: true,
            tags_on_resource: Some(4),
            removed: None,
        },
        Case {
            case: "a key the resource already carries",
            call: Call::Tag(pairs(&[("team", &value(5))])),
            target: Target::Inquiry,
            region: REGION,
            status: 204,
            outcome: "recorded",
            resolved: true,
            tags_on_resource: Some(4),
            removed: None,
        },
        Case {
            case: "a key using the pattern's whole character set, with an empty value",
            call: Call::Tag(pairs(&[(PATTERN_CHARS_KEY, "")])),
            target: Target::Inquiry,
            region: REGION,
            status: 204,
            outcome: "recorded",
            resolved: true,
            tags_on_resource: Some(5),
            removed: None,
        },
        Case {
            case: "removing a key the resource carries",
            call: Call::Untag(vec!["team".to_string()]),
            target: Target::Inquiry,
            region: REGION,
            status: 204,
            outcome: "removed",
            resolved: true,
            tags_on_resource: Some(4),
            removed: Some(1),
        },
        Case {
            case: "removing a key the resource does not carry",
            call: Call::Untag(vec!["never-written".to_string()]),
            target: Target::Inquiry,
            region: REGION,
            status: 204,
            outcome: "removed",
            resolved: true,
            tags_on_resource: Some(4),
            removed: Some(0),
        },
        Case {
            case: "removing three keys, two of them carried",
            call: Call::Untag(vec![
                "env".to_string(),
                "never-written".to_string(),
                "owner".to_string(),
            ]),
            target: Target::Inquiry,
            region: REGION,
            status: 204,
            outcome: "removed",
            resolved: true,
            tags_on_resource: Some(2),
            removed: Some(2),
        },
        Case {
            case: "a tag write naming a key in the reserved namespace",
            call: Call::Tag(pairs(&[
                (
                    // Refused from the request alone, so the record names no resource.
                    "aws:cloudformation:stack-name",
                    &value(6),
                ),
                ("team", &value(7)),
            ])),
            target: Target::Inquiry,
            region: REGION,
            status: 400,
            outcome: "refused",
            resolved: false,
            tags_on_resource: None,
            removed: None,
        },
        Case {
            case: "a removal naming a key in the reserved namespace",
            call: Call::Untag(vec![format!("{RESERVED_PREFIX}created-by")]),
            target: Target::Inquiry,
            region: REGION,
            status: 400,
            outcome: "refused",
            resolved: false,
            tags_on_resource: None,
            removed: None,
        },
        Case {
            case: "a tag write naming an ARN this account holds no resource for",
            call: Call::Tag(pairs(&[("team", &value(8))])),
            target: Target::Absent,
            region: REGION,
            status: 401,
            outcome: "refused",
            resolved: false,
            tags_on_resource: None,
            removed: None,
        },
        Case {
            // Signed for another region, so the record's region cannot come from the
            // resource or from a constant: it is the one the request presented.
            case: "a removal naming an ARN this account holds no resource for, signed for \
                   another region",
            call: Call::Untag(vec!["team".to_string()]),
            target: Target::Absent,
            region: OTHER_REGION,
            status: 401,
            outcome: "refused",
            resolved: false,
            tags_on_resource: None,
            removed: None,
        },
        Case {
            case: "a write filling the resource to the modelled tag ceiling",
            call: Call::Tag(ceiling_tags),
            target: Target::Full,
            region: REGION,
            status: 204,
            outcome: "recorded",
            resolved: true,
            tags_on_resource: Some(TAG_CEILING),
            removed: None,
        },
        Case {
            case: "a write that would take the resource past the ceiling",
            call: Call::Tag(pairs(&[("one.too.many", &value(9))])),
            target: Target::Full,
            region: REGION,
            status: 400,
            outcome: "refused",
            resolved: true,
            tags_on_resource: None,
            removed: None,
        },
    ];

    // What the writes above leave on the inquiry, tracked here so the re-read at the end
    // shows the records did not change what the writes persisted.
    let mut expected_tags: BTreeMap<String, String> = BTreeMap::new();

    for expected in cases {
        let case = expected.case;
        let (arn, resource_id) = match expected.target {
            Target::Inquiry => (inquiry_arn.as_str(), inquiry_id.as_str()),
            Target::Full => (full_arn.as_str(), full_id.as_str()),
            Target::Absent => (absent_arn.as_str(), ""),
        };

        let answer = match &expected.call {
            Call::Tag(tags) => tag(&router, expected.region, arn, tags).await,
            Call::Untag(keys) => untag(&router, expected.region, arn, keys).await,
        };
        assert_eq!(
            answer.status, expected.status,
            "case {case:?}: the call must answer HTTP {}, got {} with {:?}",
            expected.status, answer.status, answer.body
        );

        if matches!(expected.target, Target::Inquiry) && expected.status == 204 {
            match &expected.call {
                Call::Tag(tags) => expected_tags.extend(tags.iter().cloned()),
                Call::Untag(keys) => {
                    for key in keys {
                        expected_tags.remove(key);
                    }
                }
            }
        }

        let records = captured.drain_for(AUDIT_AKID);
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: one tag write must leave exactly one audit record, got {records:?}"
        );
        let record = &records[0];

        // Asserted as the WHOLE `field=value` pair ([`carries`]), so a value can satisfy
        // the check neither from the wrong field nor as the prefix of a longer value. The
        // ARN and the key list travel in their `Debug` form, which is what keeps a key's
        // whitespace from breaking the record onto a second line.
        let mut pairs_expected = vec![
            format!("operation={}", expected.call.operation()),
            format!("akid={AUDIT_AKID}"),
            format!("region={}", expected.region),
            format!("resource_arn={arn:?}"),
            format!("tag_keys={:?}", expected.call.keys()),
            format!("tag_count={}", expected.call.count()),
            format!("outcome={}", expected.outcome),
        ];
        if expected.resolved {
            pairs_expected.push(format!("compliance_inquiry_id={resource_id}"));
        }
        if let Some(total) = expected.tags_on_resource {
            pairs_expected.push(format!("tags_on_resource={total}"));
        }
        if let Some(removed) = expected.removed {
            pairs_expected.push(format!("removed={removed}"));
        }
        for pair in pairs_expected {
            assert!(
                carries(record, &pair),
                "case {case:?}: the audit record must carry {pair:?}, got {record:?}"
            );
        }

        if !expected.resolved {
            assert!(
                !record.contains("compliance_inquiry_id"),
                "case {case:?}: a call that resolved no resource must name none, got {record:?}"
            );
        }

        // A record names the keys a call wrote, never the caller's values.
        assert!(
            !record.contains(VALUE_MARKER),
            "case {case:?}: the audit record must carry no tag value, got {record:?}"
        );

        // One record is one line: a key carrying whitespace must not break it.
        assert!(
            !record.contains('\n') && !record.contains('\r'),
            "case {case:?}: the audit record must be one line, got {record:?}"
        );
    }

    // The records are the only thing this fix added: the tag set the writes persisted is
    // still the one a reader reports.
    let reported = list_tags(&router, &inquiry_arn).await;
    assert_eq!(
        reported, expected_tags,
        "the persisted tag set must be the one the writes left, got {reported:?}"
    );
    let full = list_tags(&router, &full_arn).await;
    assert_eq!(
        full.len(),
        TAG_CEILING,
        "the resource filled to the ceiling must still hold {TAG_CEILING} tags, got {}",
        full.len()
    );
}
