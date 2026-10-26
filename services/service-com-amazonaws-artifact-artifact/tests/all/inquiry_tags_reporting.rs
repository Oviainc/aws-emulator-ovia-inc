// SPDX-License-Identifier: CC0-1.0

//! The ONE optional-`tags` projection every compliance-inquiry operation answers with,
//! driven end to end over HTTP through all three operations that report it.
//!
//! RULE: an inquiry operation that reports the inquiry's tags beside its own payload
//! reports exactly the tag set the stored record holds, and reports NO `tags` member at
//! all — rather than an empty map — when that record holds none. One rule, in one place
//! ([`ComplianceInquiry::reported_tags`]), so the create, the metadata read and the export
//! cannot disagree about what an untagged inquiry reports. `ListTagsForResource` is NOT
//! this projection: there the member IS the listing, so it is present and empty for an
//! untagged resource — which is what makes the two answers distinguishable here.
//!
//! The three reporters are `CreateComplianceInquiry` (the former copy 1), the metadata
//! read `GetComplianceInquiryMetadata` (copy 2) and `ExportComplianceInquiry` (copy 3);
//! every case drives all three over one stored record, so a projection that differs
//! between them fails.
//!
//! Rows differ along what the rule generalises over — the SIZE and the SHAPE of the tag
//! set: none at all, one entry, several, a value at `TagValue`'s published floor of 0
//! characters, a key and a value at their published ceilings, and a tag set at `TagsMap`'s
//! published entry ceiling. The emptiness the rule turns on is reached from both
//! directions: by creating an untagged inquiry, and by removing every tag of a tagged one.

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router bootstrap, the credential scope and the region it signs for are the crate's
// test harness': one emulator over one shared engine.
use super::harness::{emulator, signed_for, REGION};

/// The endpoint host every request of this suite addresses the emulator on.
const ENDPOINT: &str = "artifact.eu-west-1.amazonaws.com";

/// `TagsMap`'s published entry ceiling — `@length(max: 50)`
/// (`smithy_query(query="shape",shape="TagsMap")`).
const MAX_TAG_ENTRIES: usize = 50;

/// `TagKey`'s published ceiling — `@length(min: 1, max: 128)` (same query).
const MAX_TAG_KEY: usize = 128;

/// `TagValue`'s published ceiling — `@length(min: 0, max: 256)` (same query); its floor of
/// 0 is why the empty value is a row below.
const MAX_TAG_VALUE: usize = 256;

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The `tags` member of this answer as a map, or `None` when the answer carries no
    /// such member at all. The two are what the rule distinguishes, so an absent member
    /// is never defaulted to the empty map here.
    fn reported_tags(&self) -> Option<BTreeMap<String, String>> {
        let member = self.body.get("tags")?;
        if member.is_null() {
            return None;
        }
        let object = member
            .as_object()
            .unwrap_or_else(|| panic!("a tags member must be a map, got {member:?}"));
        Some(
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect(),
        )
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", ENDPOINT)
        .header("content-type", "application/json")
        .header("authorization", signed_for(REGION))
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// `resourceArn` is an `@httpLabel` and a non-greedy label is ONE path segment, so the
/// `/` inside an ARN's resource field has to be written `%2F`.
fn label(resource_arn: &str) -> String {
    resource_arn.replace('/', "%2F")
}

/// A `tags` object for a request body.
fn tags_object(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// The tag set a case submitted, in the comparable form [`Answer::reported_tags`] answers
/// in.
fn expected(tags: &[(String, String)]) -> BTreeMap<String, String> {
    tags.iter().cloned().collect()
}

/// `POST /v1/compliance-inquiry/create` carrying `tags` — the first of the three
/// reporters, and what persists the record the other two read.
async fn create(router: &MultiServiceRouter, name: &str, tags: &[(String, String)]) -> Answer {
    let mut body = json!({
        "name": name,
        "inquiryContent": { "query": "Which controls does this report cover?" },
        "supportMode": "AI_ONLY",
    });
    if !tags.is_empty() {
        body["tags"] = tags_object(tags);
    }
    call(router, "POST", "/v1/compliance-inquiry/create", Some(body)).await
}

/// `GET /v1/compliance-inquiry/getMetadata` — the second reporter.
async fn get_metadata(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"),
        None,
    )
    .await
}

/// `POST /v1/compliance-inquiry/export` — the third reporter.
async fn export(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    call(
        router,
        "POST",
        "/v1/compliance-inquiry/export",
        Some(json!({ "complianceInquiryId": inquiry_id })),
    )
    .await
}

/// `GET /tags/{resourceArn}` — the tag LISTING, whose member is present and empty for an
/// untagged resource. Not the projection under test: the control that tells the two
/// answers apart.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", label(resource_arn)), None).await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`; `tagKeys` is a `@httpQuery`
/// list, so each key is one repetition of the parameter.
async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[String]) -> Answer {
    let query: Vec<String> = keys
        .iter()
        .map(|key| {
            format!(
                "tagKeys={}",
                ::percent_encoding::utf8_percent_encode(key, ::percent_encoding::NON_ALPHANUMERIC)
            )
        })
        .collect();
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{}", label(resource_arn), query.join("&")),
        None,
    )
    .await
}

/// One row of the table: a tag set the rule has to project the same way through all three
/// reporters.
struct Case {
    label: &'static str,
    tags: Vec<(String, String)>,
}

/// The tag sets under test. Every key and value is drawn from the character class
/// `TagKey`/`TagValue` publish (`^[a-zA-Z0-9\s_.:/=+\-@]*$`, same query as the ceilings
/// above).
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no tags at all",
            tags: Vec::new(),
        },
        Case {
            label: "one tag",
            tags: vec![("Team".to_string(), "Compliance".to_string())],
        },
        Case {
            label: "several tags",
            tags: vec![
                ("Team".to_string(), "Compliance".to_string()),
                ("Environment".to_string(), "Production".to_string()),
                ("cost.center".to_string(), "eu/west=1".to_string()),
            ],
        },
        Case {
            label: "a value at TagValue's floor of 0 characters",
            tags: vec![
                ("Reviewed".to_string(), String::new()),
                ("Team".to_string(), "Compliance".to_string()),
            ],
        },
        Case {
            label: "a key and a value at their published ceilings",
            tags: vec![
                ("k".repeat(MAX_TAG_KEY), "v".repeat(MAX_TAG_VALUE)),
                ("Team".to_string(), "Compliance".to_string()),
            ],
        },
        Case {
            label: "a tag set at TagsMap's entry ceiling",
            tags: (0..MAX_TAG_ENTRIES)
                .map(|index| (format!("key.{index}"), format!("value.{index}")))
                .collect(),
        },
    ]
}

/// The id and the ARN a create answered with.
fn created_identity(answer: &Answer, case: &str) -> (String, String) {
    let summary = &answer.body["complianceInquirySummary"];
    let id = summary["id"].as_str().unwrap_or_default().to_string();
    let arn = summary["arn"].as_str().unwrap_or_default().to_string();
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "case {case:?}: the create must report an id and an ARN, got {:?}",
        answer.body
    );
    (id, arn)
}

/// Assert the three reporters agree with `want` for the inquiry `inquiry_id`/`arn`, and
/// that the tag LISTING of the same record still answers the map form.
async fn assert_all_three_report(
    router: &MultiServiceRouter,
    case: &str,
    inquiry_id: &str,
    arn: &str,
    created: Option<&Answer>,
    want: Option<BTreeMap<String, String>>,
) {
    if let Some(created) = created {
        assert_eq!(
            created.reported_tags(),
            want,
            "case {case:?}: CreateComplianceInquiry must report the projection, got {:?}",
            created.body
        );
    }

    let read = get_metadata(router, inquiry_id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the metadata read must be served, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.reported_tags(),
        want,
        "case {case:?}: GetComplianceInquiryMetadata must report the projection, got {:?}",
        read.body
    );

    let exported = export(router, inquiry_id).await;
    assert_eq!(
        exported.status, 200,
        "case {case:?}: the export must be served, got {} with {:?}",
        exported.status, exported.body
    );
    assert_eq!(
        exported.reported_tags(),
        want,
        "case {case:?}: ExportComplianceInquiry must report the projection, got {:?}",
        exported.body
    );

    // The LISTING of the same record: present whatever the tag set, so the absent member
    // above is this operation's projection choice rather than a tag set that vanished.
    let listed = list_tags(router, arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: the tag listing must be served, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.reported_tags(),
        Some(want.unwrap_or_default()),
        "case {case:?}: the tag listing reports the map form even when empty, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn every_inquiry_operation_reports_the_one_tags_projection() {
    let router = emulator();

    for case in cases() {
        let created = create(&router, "Annual audit", &case.tags).await;
        assert_eq!(
            created.status, 202,
            "case {:?}: the create must be accepted, got {} with {:?}",
            case.label, created.status, created.body
        );
        let (inquiry_id, arn) = created_identity(&created, case.label);
        let want = (!case.tags.is_empty()).then(|| expected(&case.tags));

        assert_all_three_report(&router, case.label, &inquiry_id, &arn, Some(&created), want).await;
    }
}

#[tokio::test]
async fn removing_every_tag_makes_all_three_operations_omit_the_member() {
    let router = emulator();

    for case in cases() {
        if case.tags.is_empty() {
            // Nothing to remove: the untagged row is covered by the table above.
            continue;
        }
        let label = format!("{} then untagged", case.label);
        let created = create(&router, "Annual audit", &case.tags).await;
        assert_eq!(
            created.status, 202,
            "case {label:?}: the create must be accepted, got {} with {:?}",
            created.status, created.body
        );
        let (inquiry_id, arn) = created_identity(&created, &label);

        let keys: Vec<String> = case.tags.iter().map(|(key, _)| key.clone()).collect();
        let removed = untag(&router, &arn, &keys).await;
        assert_eq!(
            removed.status, 204,
            "case {label:?}: removing every tag must be accepted, got {} with {:?}",
            removed.status, removed.body
        );

        // The record now holds no tags, so the same one projection applies: every
        // reporter omits the member, and the listing answers the empty map.
        assert_all_three_report(&router, &label, &inquiry_id, &arn, None, None).await;
    }
}
