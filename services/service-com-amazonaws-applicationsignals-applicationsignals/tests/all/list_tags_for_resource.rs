// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListTagsForResource`, this service's tag READER, over HTTP.
//!
//! The rule under test: `ListTagsForResource` reports the tag set persisted for the STORED
//! resource its `ResourceArn` names — whatever kind of taggable resource of this service that
//! ARN names — as the modelled `Tags` list, which is reported as a key-to-value map (each key
//! once) and is PRESENT and empty for a resource that carries no tag, since holding no tags
//! and holding an empty tag set are the same state. An ARN that names no resource this
//! service holds is the modelled `ResourceNotFoundException` (404), and a `ResourceArn`
//! outside the modelled `AmazonResourceName` `length 1..1024` is refused before any resource
//! is resolved, with a 400 `ValidationException`.
//!
//! The sibling suites cover the removal side (`resource_tags.rs` for an SLO,
//! `instrumentation_configuration_tags.rs` for an instrumentation configuration); what is
//! asserted here is the READER's own contract: the shape of the list it answers over BOTH
//! taggable kinds, and the modelled bounds on the ARN it is addressed by. Their request
//! helpers are reused, so every suite speaks to one harness.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tags::{expected_map, list_tags_raw, reported_tags, tag, tag_list, untag};
use crate::slo_lifecycle::{arn_uri_segment, call, create, router, sli_config, ACCOUNT, REGION};

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`).
const SNAPSHOT: &str = "SNAPSHOT";

/// Which taggable resource of this service a row addresses. The two kinds are written by
/// different operations and carry differently shaped ARNs, which is exactly what the rule
/// generalises over.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// A service level objective, written by `CreateServiceLevelObjective`.
    Slo,
    /// A dynamic instrumentation configuration, written by
    /// `CreateInstrumentationConfiguration`.
    Configuration,
}

/// Create a resource of `kind` carrying `tags` (or naming the `Tags` member not at all when
/// `tags` is `None`) and hand back the ARN the service MINTED for it — the stored record's
/// own, never one the test derived.
async fn create_resource(
    router: &MultiServiceRouter,
    label: &str,
    kind: Kind,
    name: &str,
    tags: Option<&[(String, String)]>,
) -> String {
    match kind {
        Kind::Slo => {
            let mut body = json!({
                "Name": name,
                "SliConfig": sli_config("checkout", "GET /cart", 100.0),
            });
            if let Some(tags) = tags {
                body["Tags"] = tag_list(tags);
            }
            let (status, created) = create(router, body).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the setup create of the SLO {name:?} to succeed, got {status} with {created}"
            );
            created["Slo"]["Arn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        }
        Kind::Configuration => {
            let mut body = json!({
                "InstrumentationType": "PROBE",
                "Service": name,
                "Environment": "eks:cluster-prod/namespace",
                "SignalType": SNAPSHOT,
                "Location": {
                    "CodeLocation": {
                        "Language": "Java",
                        "FilePath": "src/main/java/com/example/CartService.java",
                        "LineNumber": 142,
                    }
                },
                // `CaptureConfiguration`'s required union member, with only its own required
                // member (`smithy_query(query="shape",shape="CodeCaptureConfiguration")`).
                "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } },
            });
            if let Some(tags) = tags {
                body["Tags"] = tag_list(tags);
            }
            let (status, created) = call(
                router,
                "POST",
                "/create-instrumentation-configuration",
                Some(body),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the setup create of the configuration {name:?} to succeed, got {status} with {created}"
            );
            created["ARN"].as_str().unwrap_or_default().to_string()
        }
    }
}

/// `n` distinct tags, so a row can span tag-set SIZES as well as spellings.
fn generated_tags(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|index| tag(&format!("key-{index:03}"), &format!("value-{index:03}")))
        .collect()
}

/// One row of the reader table: a resource kind and the tags its writer persisted.
struct Case {
    label: &'static str,
    kind: Kind,
    /// The name (SLO name, or the configuration's service name) the resource is created
    /// under, so each row addresses a resource of its own.
    name: &'static str,
    /// The `Tags` the create carries, or `None` when it names the member not at all.
    tags: Option<Vec<(String, String)>>,
}

/// The resources and tag sets the reader's rule is asserted over.
///
/// The rows differ along what the rule generalises over: the KIND of taggable resource (hence
/// the shape of the ARN the reader is addressed by and the operation that persisted the
/// tags), how many tags the resource holds (none, one, several, and a count no smaller than
/// the documented per-resource maximum of 50), whether the create named `Tags` at all or
/// named it empty, and the spellings the modelled constraints admit
/// (`smithy_query(query="shape",shape="Tag,TagKey,TagValue")`: `TagKey` has `length 1..128`,
/// `TagValue` `length 0..256`) — both published key-length bounds and the empty value appear
/// below.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "an SLO holding three tags",
            kind: Kind::Slo,
            name: "three-tag-slo",
            tags: Some(vec![
                tag("env", "prod"),
                tag("team", "payments"),
                tag("tier", "gold"),
            ]),
        },
        Case {
            label: "an SLO holding one tag",
            kind: Kind::Slo,
            name: "one-tag-slo",
            tags: Some(vec![tag("env", "prod")]),
        },
        Case {
            label: "an SLO created with the empty tag list the model admits",
            kind: Kind::Slo,
            name: "empty-list-slo",
            tags: Some(Vec::new()),
        },
        Case {
            label: "an SLO created with no Tags member at all",
            kind: Kind::Slo,
            name: "no-tags-slo",
            tags: None,
        },
        Case {
            label: "an SLO holding the key and value sizes the model admits",
            kind: Kind::Slo,
            name: "bounds-slo",
            tags: Some(vec![
                // The longest key `TagKey` `length 1..128` admits, the shortest it admits,
                // and the empty value `TagValue` `length 0..256` admits.
                tag(&"k".repeat(128), "longest-key"),
                tag("x", ""),
                tag("v", &"v".repeat(256)),
            ]),
        },
        Case {
            label: "an SLO holding the 50 tags its documented maximum admits",
            kind: Kind::Slo,
            name: "fifty-tag-slo",
            tags: Some(generated_tags(50)),
        },
        Case {
            label: "a configuration holding two tags",
            kind: Kind::Configuration,
            name: "two-tag-config",
            tags: Some(vec![tag("team", "checkout"), tag("env", "prod")]),
        },
        Case {
            label: "a configuration created with no Tags member at all",
            kind: Kind::Configuration,
            name: "no-tags-config",
            tags: None,
        },
        Case {
            label: "a configuration holding the 200 tags TagList admits",
            kind: Kind::Configuration,
            name: "two-hundred-tag-config",
            tags: Some(generated_tags(200)),
        },
    ]
}

/// The keys a `ListTagsForResource` response reports, in the order it reported them.
fn reported_keys(body: &Value) -> Vec<String> {
    body["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|entry| entry["Key"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The reader reports exactly the tag set its writer persisted for the resource the ARN
/// names, for every taggable kind and every shape of tag set — and reports each key exactly
/// once, since a tag set is a key-to-value map.
#[tokio::test]
async fn the_reader_reports_the_tag_set_its_writer_persisted() {
    let router = router();
    for case in cases() {
        let arn = create_resource(
            &router,
            case.label,
            case.kind,
            case.name,
            case.tags.as_deref(),
        )
        .await;
        assert!(
            !arn.is_empty(),
            "case {:?}: expected the create to answer the ARN it minted",
            case.label
        );

        let (status, body) = list_tags_raw(&router, &arn_uri_segment(&arn)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 reading the tags of {arn}, got {status} with {body}",
            case.label
        );

        let expected = expected_map(case.tags.as_deref().unwrap_or_default());
        assert_eq!(
            reported_tags(&body),
            expected,
            "case {:?}: expected the persisted tags {expected:?}, got {:?}",
            case.label,
            reported_tags(&body)
        );

        let keys = reported_keys(&body);
        let distinct: BTreeSet<&String> = keys.iter().collect();
        assert_eq!(
            keys.len(),
            distinct.len(),
            "case {:?}: expected every key to be reported once, got {keys:?}",
            case.label
        );
    }
}

/// A resource that carries no tag reports the modelled `Tags` member as the EMPTY LIST rather
/// than omitting it or failing: holding no tags and holding an empty tag set are the same
/// state, and the resource exists either way.
///
/// The rows differ along HOW the resource came to carry nothing: a create that named `Tags`
/// not at all, a create that named it empty, and a resource whose last tag a later
/// `UntagResource` detached — over both taggable kinds.
#[tokio::test]
async fn a_resource_with_no_tags_reports_the_empty_list() {
    let router = router();

    let mut untagged: Vec<(&str, String)> = Vec::new();
    for (label, kind, name, tags) in [
        ("an SLO created without Tags", Kind::Slo, "bare-slo", None),
        (
            "an SLO created with an empty Tags list",
            Kind::Slo,
            "empty-slo",
            Some(Vec::new()),
        ),
        (
            "an SLO whose only tag was detached",
            Kind::Slo,
            "detached-slo",
            Some(vec![tag("env", "prod")]),
        ),
        (
            "a configuration created without Tags",
            Kind::Configuration,
            "bare-config",
            None,
        ),
        (
            "a configuration whose only tag was detached",
            Kind::Configuration,
            "detached-config",
            Some(vec![tag("env", "prod")]),
        ),
    ] {
        let arn = create_resource(&router, label, kind, name, tags.as_deref()).await;
        if let Some(tags) = tags.as_ref().filter(|tags| !tags.is_empty()) {
            let keys: Vec<String> = tags.iter().map(|(key, _value)| key.clone()).collect();
            let (status, body) = untag(&router, &arn, &keys).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected HTTP 200 from the setup untag, got {status} with {body}"
            );
        }
        untagged.push((label, arn));
    }

    for (label, arn) in &untagged {
        let (status, body) = list_tags_raw(&router, &arn_uri_segment(arn)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 for a stored resource carrying no tag, got {status} with {body}"
        );
        assert_eq!(
            body["Tags"],
            json!([]),
            "case {label:?}: expected the Tags member to be reported as the empty list, got {body}"
        );
    }
}

/// A `ResourceArn` the modelled `AmazonResourceName` `length 1..1024` admits reaches the
/// resource lookup — so a well-formed-but-unknown ARN of an admitted length is the modelled
/// `ResourceNotFoundException` (404) — while one OUTSIDE that bound is no ARN the API accepts
/// and is refused with a 400 `ValidationException` before any resource is resolved.
///
/// Each published bound is asserted AT the bound (admitted) and one past it (refused): a
/// 1-character value, a 1024-character value, a 1025-character value and the empty value.
#[tokio::test]
async fn the_modelled_arn_length_bounds_decide_refusal_from_not_found() {
    let router = router();
    let kept = vec![tag("env", "prod"), tag("team", "payments")];
    let live = create_resource(
        &router,
        "arn length bounds",
        Kind::Slo,
        "bounded-slo",
        Some(&kept),
    )
    .await;

    // An ARN of this service and account whose SLO name is padded until the whole string is
    // exactly `len` characters long: a value the length bound admits, naming no stored SLO.
    let padded = |len: usize| -> String {
        let prefix = format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/");
        let name_len = len - prefix.len();
        format!("{prefix}{}", "a".repeat(name_len))
    };

    // AT each published bound the value is admitted and the lookup decides the answer.
    let admitted = [
        ("the shortest value the bound admits", "x".to_string()),
        ("the longest value the bound admits", padded(1024)),
    ];
    for (label, resource_arn) in &admitted {
        assert!(
            (1..=1024).contains(&resource_arn.chars().count()),
            "case {label:?}: expected the row to sit inside the modelled bound, got {} characters",
            resource_arn.chars().count()
        );
        let (status, error) = list_tags_raw(&router, &arn_uri_segment(resource_arn)).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404 for an admitted value naming no resource, got {status} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected a ResourceNotFoundException, got {error}"
        );
    }

    // One past each published bound the value is refused outright.
    let refused = [
        ("one character past the maximum", padded(1025)),
        ("the empty value, one below the minimum", String::new()),
    ];
    for (label, resource_arn) in &refused {
        let (status, error) = list_tags_raw(&router, &arn_uri_segment(resource_arn)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a value the model refuses, got {status} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected a ValidationException, got {error}"
        );
    }

    // None of it reached the store: the one stored resource still reports every tag.
    let (status, after) = list_tags_raw(&router, &arn_uri_segment(&live)).await;
    assert_eq!(
        status, 200,
        "expected the stored SLO to still answer 200, got {status} with {after}"
    );
    assert_eq!(
        reported_tags(&after),
        expected_map(&kept),
        "expected the refused reads to change nothing, got {:?}",
        reported_tags(&after)
    );
}
