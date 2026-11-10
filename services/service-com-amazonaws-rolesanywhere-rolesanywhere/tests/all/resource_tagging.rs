// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ARN-keyed tag path: `TagResource` and the
//! `ListTagsForResource` read it is proved through.
//!
//! Every request goes through the real `MultiServiceRouter` — the same dispatch the
//! emulator binary uses — so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run exactly as they do in production.
//!
//! The rule under test: `TagResource` merges the submitted tags into the tag set of the
//! resource its `resourceArn` names, keyed by tag key — a key the resource already
//! carries takes the submitted value, a key it does not carry is added, and a key the
//! request does not name is left alone — and `ListTagsForResource` answers with exactly
//! that stored set. So the tables below span the dimensions the rule generalises over:
//! every taggable resource type of this service, the tags the resource was created with,
//! the number and overlap of the keys a call carries, and the published `TagKey` /
//! `TagValue` bounds and character class.
//!
//! The harness below is local to this suite: each sibling suite's client is private to
//! its own module, and the targets are compiled into one test binary only as separate
//! modules.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_rolesanywhere_rolesanywhere::state::MAX_TAGS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle for a trust anchor's certificate source.
/// `SourceData$x509CertificateData` is `@length(1, 204800)` with no `@pattern`.
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW11bGF0ZWQtdGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

/// A placeholder CRL in PEM form; `ImportCrlRequest$crlData` is an opaque `Blob`.
const CRL_PEM: &str = "-----BEGIN X509 CRL-----\nZW11bGF0ZWQtdGVzdC1jcmw=\n-----END X509 CRL-----\n";

/// The Region the tests sign for.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The published `TagKey` `@length` maximum
/// (`smithy_query(query="shape", shape="TagKey")`: `@length(1, 128)`).
const MAX_TAG_KEY_LENGTH: usize = 128;

/// The published `TagValue` `@length` bounds
/// (`smithy_query(query="shape", shape="TagValue")`: `@length(0, 256)`).
const MIN_TAG_VALUE_LENGTH: usize = 0;
const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The published `TagList` `@length` maximum, which bounds the tags ONE call may carry
/// (`smithy_query(query="shape", shape="TagList")`: `@length(0, 200)`). The tags one
/// RESOURCE may hold is the smaller per-resource quota, `MAX_TAGS_PER_RESOURCE`.
const MAX_TAG_LIST_LENGTH: usize = 200;

/// A client over one emulator: one engine, one mounted service, so every request in a
/// test observes the state the previous one wrote.
struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body (`Value::Null` for an empty body)
/// and the wire error code when the answer is a failure.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn tag_resource(&self, resource_arn: &str, tags: &[(String, String)]) -> Answer {
        self.send(
            "POST",
            "/TagResource",
            Some(json!({"resourceArn": resource_arn, "tags": tag_json(tags)})),
        )
        .await
    }

    async fn list_tags(&self, resource_arn: &str) -> Answer {
        self.send(
            "GET",
            &format!("/ListTagsForResource?resourceArn={resource_arn}"),
            None,
        )
        .await
    }

    async fn delete_trust_anchor(&self, id: &str) -> Answer {
        self.send("DELETE", &format!("/trustanchor/{id}"), None).await
    }
}

/// The wire form of a tag list: `TagList` of `Tag { key, value }`.
fn tag_json(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// Owned pairs from literal rows, so a table row reads as `&[("env", "test")]`.
fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The tags a `ListTagsForResource` answer reports, keyed by tag key. Keyed rather than
/// ordered because `ListTagsForResourceResponse$tags` is a list the model attaches no
/// ordering to; a duplicated key would collapse here, so the entry count is asserted
/// against the list length too.
fn listed_tags(case: &str, answer: &Answer) -> BTreeMap<String, String> {
    let tags = answer
        .body
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags list, got {answer:?}"));
    let keyed: BTreeMap<String, String> = tags
        .iter()
        .map(|tag| {
            let key = tag
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag without a key in {answer:?}"))
                .to_string();
            let value = tag
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag without a value in {answer:?}"))
                .to_string();
            (key, value)
        })
        .collect();
    assert_eq!(
        keyed.len(),
        tags.len(),
        "case {case:?}: every key must appear at most once, got {answer:?}"
    );
    keyed
}

/// Which taggable resource a case tags. Every one of this service's stored resources
/// carries its own tag list, so the merge rule has to hold for each of them.
#[derive(Debug, Clone, Copy)]
enum Kind {
    TrustAnchor,
    Profile,
    Crl,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::TrustAnchor => "trust-anchor",
            Kind::Profile => "profile",
            Kind::Crl => "crl",
        }
    }
}

/// Create a trust anchor and answer its ARN. A CRL import names one, and `ImportCrl`
/// refuses an ARN that resolves to nothing, so every CRL row needs a real one.
async fn create_trust_anchor_arn(client: &Client, name: &str) -> String {
    let answer = client
        .send(
            "POST",
            "/trustanchors",
            Some(json!({
                "name": name,
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "trust anchor {name:?}: expected CreateTrustAnchor 201, got {answer:?}"
    );
    answer
        .body
        .pointer("/trustAnchor/trustAnchorArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("trust anchor {name:?}: no trustAnchorArn in {answer:?}"))
        .to_string()
}

/// Create a resource of `kind` carrying `tags`, and answer its ARN.
async fn create_resource(
    client: &Client,
    kind: Kind,
    name: &str,
    tags: &[(String, String)],
) -> String {
    // A CRL is imported AGAINST a trust anchor, and `ImportCrl` resolves that reference
    // before it stores anything, so a CRL row mints the anchor it links to first.
    let crl_trust_anchor = match kind {
        Kind::Crl => {
            create_trust_anchor_arn(client, &format!("{name}-anchor")).await
        }
        Kind::TrustAnchor | Kind::Profile => String::new(),
    };
    let (uri, mut body, pointer) = match kind {
        Kind::TrustAnchor => (
            "/trustanchors",
            json!({
                "name": name,
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
            }),
            "/trustAnchor/trustAnchorArn",
        ),
        Kind::Profile => (
            "/profiles",
            json!({
                "name": name,
                "roleArns": [format!("arn:aws:iam::{ACCOUNT}:role/TaggingDemoRole")],
            }),
            "/profile/profileArn",
        ),
        Kind::Crl => (
            "/crls",
            json!({
                "name": name,
                "crlData": aws_smithy_types::base64::encode(CRL_PEM.as_bytes()),
                "trustAnchorArn": crl_trust_anchor,
            }),
            "/crl/crlArn",
        ),
    };
    if !tags.is_empty() {
        body["tags"] = tag_json(tags);
    }
    let answer = client.send("POST", uri, Some(body)).await;
    assert_eq!(
        answer.status,
        201,
        "case {:?}: expected the {} create to answer 201, got {answer:?}",
        name,
        kind.label()
    );
    answer
        .body
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {name:?}: no ARN at {pointer} in {answer:?}"))
        .to_string()
}

/// One row of the merge table: what the resource was created with, the successive
/// `TagResource` calls applied to it, and the tag set a later read must report.
struct MergeCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    batches: Vec<Vec<(String, String)>>,
    expected: Vec<(String, String)>,
}

fn merge_cases() -> Vec<MergeCase> {
    // The whole published `TagKey` / `TagValue` character class:
    // `@pattern("^[ a-zA-Z0-9_.:/=+@-]*$")`.
    let punctuated_key = " key_.:/=+@-0Z".to_string();
    let punctuated_value = " value_.:/=+@-9z".to_string();
    vec![
        MergeCase {
            name: "tags-an-untagged-resource",
            initial: Vec::new(),
            batches: vec![pairs(&[("env", "test")])],
            expected: pairs(&[("env", "test")]),
        },
        MergeCase {
            name: "adds-disjoint-keys-over-two-calls",
            initial: pairs(&[("a", "1")]),
            batches: vec![pairs(&[("b", "2")]), pairs(&[("c", "3")])],
            expected: pairs(&[("a", "1"), ("b", "2"), ("c", "3")]),
        },
        MergeCase {
            name: "replaces-the-value-of-a-held-key-and-keeps-the-rest",
            initial: pairs(&[("env", "test"), ("owner", "guild")]),
            batches: vec![pairs(&[("env", "prod")])],
            expected: pairs(&[("env", "prod"), ("owner", "guild")]),
        },
        MergeCase {
            name: "an-empty-tag-list-changes-nothing",
            initial: pairs(&[("kept", "value")]),
            batches: vec![Vec::new()],
            expected: pairs(&[("kept", "value")]),
        },
        MergeCase {
            name: "concurrent-style-batches-lose-no-writes",
            initial: Vec::new(),
            batches: vec![
                pairs(&[("one", "1"), ("two", "2")]),
                pairs(&[("three", "3"), ("one", "9")]),
                pairs(&[("four", "4")]),
            ],
            expected: pairs(&[("one", "9"), ("two", "2"), ("three", "3"), ("four", "4")]),
        },
        MergeCase {
            name: "a-key-repeated-inside-one-call-keeps-its-last-value",
            initial: Vec::new(),
            batches: vec![pairs(&[("dup", "first"), ("dup", "second")])],
            expected: pairs(&[("dup", "second")]),
        },
        MergeCase {
            name: "published-key-and-value-bounds",
            initial: Vec::new(),
            batches: vec![vec![
                ("k".repeat(MAX_TAG_KEY_LENGTH), "v".repeat(MAX_TAG_VALUE_LENGTH)),
                ("shortest".to_string(), "v".repeat(MIN_TAG_VALUE_LENGTH)),
            ]],
            expected: vec![
                ("k".repeat(MAX_TAG_KEY_LENGTH), "v".repeat(MAX_TAG_VALUE_LENGTH)),
                ("shortest".to_string(), String::new()),
            ],
        },
        MergeCase {
            name: "the-whole-published-character-class",
            initial: Vec::new(),
            batches: vec![vec![(punctuated_key.clone(), punctuated_value.clone())]],
            expected: vec![(punctuated_key, punctuated_value)],
        },
        MergeCase {
            name: "re-tagging-a-created-tag-overwrites-it-in-place",
            initial: pairs(&[("from", "create")]),
            batches: vec![pairs(&[("from", "tag-resource")])],
            expected: pairs(&[("from", "tag-resource")]),
        },
    ]
}

/// `TagResource` merges by key into whatever the resource holds, and
/// `ListTagsForResource` answers with that stored set — for every taggable resource type
/// of this service.
#[tokio::test]
async fn tag_resource_merges_by_key_and_list_tags_reports_the_stored_set() {
    let client = Client::new();

    for (index, case) in merge_cases().into_iter().enumerate() {
        for kind in [Kind::TrustAnchor, Kind::Profile, Kind::Crl] {
            let case_name = format!("{}/{}", case.name, kind.label());
            // `ResourceName` is `@pattern("^[ a-zA-Z0-9-_]*$")`, so the row index names
            // the resource rather than the case's own punctuation.
            let resource_name = format!("tagging-{}-{index}", kind.label());
            let arn = create_resource(&client, kind, &resource_name, &case.initial).await;

            // The create's own tags are what a read reports before any tagging call.
            let before = client.list_tags(&arn).await;
            assert_eq!(
                before.status, 200,
                "case {case_name:?}: expected ListTagsForResource 200, got {before:?}"
            );
            assert_eq!(
                listed_tags(&case_name, &before),
                case.initial.iter().cloned().collect::<BTreeMap<_, _>>(),
                "case {case_name:?}: a read before any tagging reports the create's tags"
            );

            for (batch_index, batch) in case.batches.iter().enumerate() {
                let tagged = client.tag_resource(&arn, batch).await;
                assert_eq!(
                    tagged.status, 201,
                    "case {case_name:?} batch {batch_index}: expected TagResource 201, \
                     got {tagged:?}"
                );
            }

            let listed = client.list_tags(&arn).await;
            assert_eq!(
                listed.status, 200,
                "case {case_name:?}: expected ListTagsForResource 200, got {listed:?}"
            );
            assert_eq!(
                listed_tags(&case_name, &listed),
                case.expected.iter().cloned().collect::<BTreeMap<_, _>>(),
                "case {case_name:?}: the merged tag set must be what the read reports"
            );

            // A read is reporting only: asking twice reports the same set.
            let again = client.list_tags(&arn).await;
            assert_eq!(
                listed_tags(&case_name, &again),
                listed_tags(&case_name, &listed),
                "case {case_name:?}: a second read must report the same set"
            );
        }
    }
}

/// Tags belong to the resource the ARN names: tagging one resource leaves every other
/// resource's tags exactly as they were, whatever its type.
#[tokio::test]
async fn tagging_one_resource_leaves_the_others_untouched() {
    let client = Client::new();
    // Each resource, with the tag set it must report at every point in the walk below.
    let mut resources: Vec<(&'static str, String, BTreeMap<String, String>)> = Vec::new();
    for (label, kind) in [
        ("anchor-a", Kind::TrustAnchor),
        ("anchor-b", Kind::TrustAnchor),
        ("profile-a", Kind::Profile),
        ("crl-a", Kind::Crl),
    ] {
        let arn = create_resource(&client, kind, label, &pairs(&[("owner", label)])).await;
        resources.push((label, arn, pairs(&[("owner", label)]).into_iter().collect()));
    }

    for index in 0..resources.len() {
        let (target, target_arn, _) = resources[index].clone();
        let tagged = client
            .tag_resource(&target_arn, &pairs(&[("tagged", target)]))
            .await;
        assert_eq!(
            tagged.status, 201,
            "case {target:?}: expected TagResource 201, got {tagged:?}"
        );
        resources[index]
            .2
            .insert("tagged".to_string(), target.to_string());

        for (other, other_arn, expected) in &resources {
            let listed = client.list_tags(other_arn).await;
            assert_eq!(
                listed_tags(other, &listed),
                *expected,
                "case tag-{target:?}/read-{other:?}: tagging one resource must not change \
                 another's tags, got {listed:?}"
            );
        }
    }
}

/// An ARN no stored resource carries is a 404 `ResourceNotFoundException` on both the
/// write and the read — never a canned success and never an empty tag list.
#[tokio::test]
async fn the_tag_path_refuses_an_arn_no_resource_carries() {
    let client = Client::new();
    // A real resource, so the store is not simply empty, plus one that is deleted again:
    // an ARN stops resolving exactly when the resource it names is gone.
    let live = create_resource(&client, Kind::TrustAnchor, "live-anchor", &[]).await;
    let doomed = create_resource(&client, Kind::TrustAnchor, "doomed-anchor", &[]).await;
    let doomed_id = doomed.rsplit('/').next().unwrap_or_default().to_string();
    let deleted = client.delete_trust_anchor(&doomed_id).await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteTrustAnchor 200, got {deleted:?}"
    );

    let unknown = [
        (
            "never-minted-trust-anchor",
            format!(
                "arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:trust-anchor/\
                 00000000-0000-4000-8000-000000000000"
            ),
        ),
        (
            "never-minted-profile",
            format!(
                "arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:profile/\
                 abcdef12-3456-7890-abcd-ef1234567890"
            ),
        ),
        (
            "never-minted-crl",
            format!(
                "arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:crl/\
                 11111111-2222-3333-4444-555555555555"
            ),
        ),
        (
            "another-partition",
            format!(
                "arn:aws-us-gov:rolesanywhere:us-gov-west-1:{ACCOUNT}:trust-anchor/\
                 22222222-3333-4444-5555-666666666666"
            ),
        ),
        ("deleted-trust-anchor", doomed.clone()),
        (
            "another-region",
            live.replace(REGION, "eu-central-1"),
        ),
    ];

    for (case, arn) in &unknown {
        let tagged = client.tag_resource(arn, &pairs(&[("env", "test")])).await;
        assert_eq!(
            tagged.status, 404,
            "case {case:?}: expected TagResource 404 for {arn:?}, got {tagged:?}"
        );
        assert_eq!(
            tagged.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {tagged:?}"
        );
        assert_eq!(
            tagged.body.get("message").and_then(Value::as_str),
            Some(format!("The item doesn't exist. arn: {arn} ; aborting.").as_str()),
            "case {case:?}: expected the ARN-keyed not-found wording, got {tagged:?}"
        );

        let listed = client.list_tags(arn).await;
        assert_eq!(
            (listed.status, listed.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected ListTagsForResource 404 for {arn:?}, got {listed:?}"
        );
    }

    // The live resource is still untagged: a refused call wrote nothing anywhere.
    let listed = client.list_tags(&live).await;
    assert_eq!(
        listed_tags("live-anchor", &listed),
        BTreeMap::new(),
        "a refused TagResource must not tag any other resource, got {listed:?}"
    );
}

/// The tag set one resource can hold is bounded by the documented per-resource tag
/// maximum: adding keys past it is refused and stores nothing, while re-tagging keys the
/// resource already holds stays within the bound and is accepted.
#[tokio::test]
async fn tag_resource_refuses_more_keys_than_a_resource_may_hold() {
    let client = Client::new();
    let arn = create_resource(&client, Kind::TrustAnchor, "bounded-anchor", &[]).await;

    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();
    let at_bound = client.tag_resource(&arn, &full).await;
    assert_eq!(
        at_bound.status, 201,
        "case \"at-the-bound\": {MAX_TAGS_PER_RESOURCE} tags is the documented maximum, \
         got {at_bound:?}"
    );

    // Re-tagging keys the resource already holds does not grow the set, so it is still
    // within the bound however often it is repeated.
    let rewritten: Vec<(String, String)> = full
        .iter()
        .map(|(key, _)| (key.clone(), "rewritten".to_string()))
        .collect();
    let again = client.tag_resource(&arn, &rewritten).await;
    assert_eq!(
        again.status, 201,
        "case \"rewrite-at-the-bound\": replacing held values must stay within the bound, \
         got {again:?}"
    );

    // One more KEY is one past the bound.
    let over = client
        .tag_resource(&arn, &pairs(&[("one-too-many", "x")]))
        .await;
    assert_eq!(
        (over.status, over.error_code.as_str()),
        (400, "ValidationException"),
        "case \"over-the-bound\": a key past the bound must be refused, got {over:?}"
    );
    assert_eq!(
        over.body.get("message").and_then(Value::as_str),
        Some(
            format!(
                "The resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} \
                 user tags."
            )
            .as_str()
        ),
        "case \"over-the-bound\": expected the quota wording, got {over:?}"
    );

    // The refusal wrote nothing: the set is still the rewritten one, with no extra key.
    let listed = client.list_tags(&arn).await;
    assert_eq!(
        listed_tags("over-the-bound", &listed),
        rewritten.iter().cloned().collect::<BTreeMap<_, _>>(),
        "case \"over-the-bound\": the refused call must leave the stored set alone"
    );

    // A single call carrying more than the published `TagList` maximum is refused at the
    // boundary, before the handler runs — and either way, a list naming more keys than
    // one resource may hold is refused.
    let too_long: Vec<(String, String)> = (0..=MAX_TAG_LIST_LENGTH)
        .map(|index| (format!("extra-{index:03}"), "x".to_string()))
        .collect();
    let refused = client.tag_resource(&arn, &too_long).await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "case \"over-the-list-bound\": {} tags in one call must be refused, got {refused:?}",
        too_long.len()
    );
}
