// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DataZone resource-tagging operations
//! (`TagResource`, `ListTagsForResource`, `UntagResource`).
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router, with only response bodies as evidence — using the harness in
//! `tests/all/domain_project_lifecycle.rs` rather than a second copy of it.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one request:
//! the tagging operations address a resource by the ARN that resource's own
//! create/get reports and operate on that resource's single stored tag set —
//! `TagResource` merges the supplied keys into it (replacing the value of a key
//! already present, leaving every other key alone), `UntagResource` removes exactly
//! the named keys, and `ListTagsForResource` reports the stored set, so a tag written
//! by one call is observable by every later reader and by no other resource.
//!
//! An ARN that names no resource this emulator stores is refused with
//! `ResourceNotFoundException` (404); text that is not an ARN of this service is
//! refused with `ValidationException` (400) by the two operations whose model declares
//! it, and with `ResourceNotFoundException` by `UntagResource`, which declares no
//! client-input error at all (smithy_query operation="UntagResource").

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

use crate::domain_project_lifecycle::{urlencode, Answer, Emulator};

/// The `/tags/{resourceArn}` URI for `arn` (smithy_query operation="TagResource":
/// `resourceArn` is an `@httpLabel`, so it occupies exactly one path segment).
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", urlencode(arn))
}

/// The `DELETE /tags/{resourceArn}?tagKeys=…` URI: `tagKeys` is an `@httpQuery` list,
/// so one parameter per key.
fn untag_uri(arn: &str, keys: &[&str]) -> String {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", urlencode(key)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{query}", tags_uri(arn))
}

/// A `tags` request member built from pairs, so a case can carry a computed key.
fn tags_body(pairs: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    json!({ "tags": Value::Object(map) })
}

/// The tag set an answer reports, as a comparable map.
fn reported_tags(answer: &Answer) -> BTreeMap<String, String> {
    answer
        .body
        .get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

impl Emulator {
    /// A settled domain plus the ARN its own `GetDomain` reports — the ARN every
    /// tagging call in this suite addresses it by.
    async fn available_domain_arn(&self, name: &str) -> String {
        let id = self.available_domain(name).await;
        let got = self.call("GET", &format!("/v2/domains/{id}"), None).await;
        assert_eq!(
            got.status, 200,
            "reading domain {name:?}: expected 200, got {} {:?}",
            got.status, got.body
        );
        let arn = got.s("arn");
        assert!(
            arn.starts_with("arn:aws:datazone:"),
            "domain {name:?}: expected a DataZone ARN, got {arn:?}"
        );
        arn
    }
}

/// One `TagResource` request. The rows differ along what the rule generalises over:
/// how many keys, whether a key is already stored, and the value and key FORMS the
/// model admits.
struct TagCase {
    label: &'static str,
    pairs: Vec<(String, String)>,
}

fn tag_cases() -> Vec<TagCase> {
    let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
    vec![
        TagCase {
            label: "one tag onto an untagged resource",
            pairs: vec![pair("team", "data")],
        },
        TagCase {
            label: "two further, distinct keys",
            pairs: vec![pair("env", "prod"), pair("owner", "analytics")],
        },
        TagCase {
            label: "a key already stored takes the new value",
            pairs: vec![pair("team", "platform")],
        },
        TagCase {
            // `TagValue` is length 0..256 with pattern `^[\w \.:/=+@-]*$`
            // (smithy_query shape="TagValue"), so the empty value is admissible.
            label: "an empty value",
            pairs: vec![pair("retired", "")],
        },
        TagCase {
            // Every character class `TagKey`/`TagValue` allow:
            // `^[\w \.:/=+@-]+$` (smithy_query shape="TagKey").
            label: "the whole admissible character class, including spaces",
            pairs: vec![pair("cost center .:/=+@-_1", "a b.c:d/e=f+g@h-i_9")],
        },
        TagCase {
            // The published bounds: `TagKey` is length 1..128, `TagValue` 0..256
            // (smithy_query shape="TagKey" / "TagValue").
            label: "a 128-character key and a 256-character value",
            pairs: vec![("k".repeat(128), "v".repeat(256))],
        },
        TagCase {
            label: "a one-character key and a one-character value",
            pairs: vec![pair("k", "v")],
        },
        TagCase {
            label: "no tags at all, which changes nothing",
            pairs: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn tag_resource_merges_into_the_stored_set_and_every_reader_reports_it() {
    let dz = Emulator::new();
    let arn = dz.available_domain_arn("tagging-merge").await;

    // The tag set the store should hold after each row, maintained independently of
    // the service.
    let mut expected: BTreeMap<String, String> = BTreeMap::new();

    for case in tag_cases() {
        let tagged = dz
            .call("POST", &tags_uri(&arn), Some(tags_body(&case.pairs)))
            .await;
        assert_eq!(
            tagged.status, 200,
            "case {:?}: expected 200 from TagResource, got {} {:?}",
            case.label, tagged.status, tagged.body
        );
        for (key, value) in &case.pairs {
            expected.insert(key.clone(), value.clone());
        }

        let listed = dz.call("GET", &tags_uri(&arn), None).await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200 from ListTagsForResource, got {} {:?}",
            case.label, listed.status, listed.body
        );
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {:?}: ListTagsForResource must report the merged tag set, got {:?}",
            case.label,
            reported_tags(&listed)
        );

        // The tag set belongs to the resource, so the resource's own reader reports
        // the very same map.
        let domain_id = arn.rsplit('/').next().unwrap_or_default().to_string();
        let got = dz
            .call("GET", &format!("/v2/domains/{domain_id}"), None)
            .await;
        assert_eq!(
            reported_tags(&got),
            expected,
            "case {:?}: GetDomain must report the same tag set as ListTagsForResource, got {:?}",
            case.label,
            reported_tags(&got)
        );

        // A read changes nothing a later caller sees.
        let again = dz.call("GET", &tags_uri(&arn), None).await;
        assert_eq!(
            reported_tags(&again),
            expected,
            "case {:?}: listing tags twice must answer identically, got {:?}",
            case.label,
            reported_tags(&again)
        );
    }
}

#[tokio::test]
async fn tags_given_at_create_are_the_resources_initial_tag_set() {
    let dz = Emulator::new();
    // Rows differ along the tag count a create carries, including none at all.
    let cases: Vec<(&str, Value)> = vec![
        ("no tags", json!({ "name": "create-no-tags" })),
        (
            "one tag",
            json!({ "name": "create-one-tag", "tags": { "team": "data" } }),
        ),
        (
            "three tags",
            json!({ "name": "create-three-tags", "tags": { "a": "1", "b": "2", "c": "" } }),
        ),
    ];

    for (label, body) in cases {
        let created = dz.call("POST", "/v2/domains", Some(body.clone())).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected 201 from CreateDomain, got {} {:?}",
            created.status, created.body
        );
        let arn = created.s("arn");
        let expected: BTreeMap<String, String> = body
            .get("tags")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let listed = dz.call("GET", &tags_uri(&arn), None).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected 200 from ListTagsForResource, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {label:?}: ListTagsForResource must report the tags the create stored, got {:?}",
            reported_tags(&listed)
        );
    }
}

#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys() {
    let dz = Emulator::new();
    let arn = dz.available_domain_arn("tagging-untag").await;

    let seed: Vec<(String, String)> = [
        ("a", "1"),
        ("b", "2"),
        ("c", "3"),
        ("d", "4"),
        ("cost center", "finance"),
        ("plus+key", "plus+value"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let tagged = dz
        .call("POST", &tags_uri(&arn), Some(tags_body(&seed)))
        .await;
    assert_eq!(
        tagged.status, 200,
        "seeding tags: expected 200, got {} {:?}",
        tagged.status, tagged.body
    );
    let mut expected: BTreeMap<String, String> = seed.into_iter().collect();

    // The rows differ along how many keys one call names, whether a named key is
    // stored at all, whether it repeats, and what characters it carries.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("one stored key", vec!["a"]),
        ("two stored keys at once", vec!["b", "c"]),
        ("a key that is not stored (idempotent)", vec!["absent"]),
        ("the same key twice in one call", vec!["d", "d"]),
        ("a key carrying a space", vec!["cost center"]),
        ("a key carrying a plus", vec!["plus+key"]),
        ("one stored key, now removed twice", vec!["a"]),
    ];

    for (label, keys) in cases {
        let untagged = dz.call("DELETE", &untag_uri(&arn, &keys), None).await;
        assert_eq!(
            untagged.status, 200,
            "case {label:?}: expected 200 from UntagResource, got {} {:?}",
            untagged.status, untagged.body
        );
        for key in &keys {
            expected.remove(*key);
        }

        let listed = dz.call("GET", &tags_uri(&arn), None).await;
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {label:?}: UntagResource must remove exactly the named keys, got {:?}",
            reported_tags(&listed)
        );
    }
    assert!(
        expected.is_empty(),
        "the table must end with every seeded key removed, {expected:?} left"
    );
}

#[tokio::test]
async fn a_tag_set_belongs_to_the_resource_its_arn_names() {
    let dz = Emulator::new();
    // Rows differ in how many tags each resource carries, so a shared tag set or a
    // last-write-wins store cannot pass.
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "first",
            vec![("owner".to_string(), "alpha".to_string())],
        ),
        (
            "second",
            vec![
                ("owner".to_string(), "beta".to_string()),
                ("extra".to_string(), "yes".to_string()),
            ],
        ),
        ("third", Vec::new()),
    ];

    let mut tagged: Vec<(&str, String, BTreeMap<String, String>)> = Vec::new();
    for (label, pairs) in &cases {
        let arn = dz
            .available_domain_arn(&format!("tagging-scope-{label}"))
            .await;
        let answer = dz.call("POST", &tags_uri(&arn), Some(tags_body(pairs))).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected 200 from TagResource, got {} {:?}",
            answer.status, answer.body
        );
        tagged.push((label, arn, pairs.iter().cloned().collect()));
    }

    for (label, arn, expected) in &tagged {
        let listed = dz.call("GET", &tags_uri(arn), None).await;
        assert_eq!(
            &reported_tags(&listed),
            expected,
            "case {label:?}: a resource must report only its own tags, got {:?}",
            reported_tags(&listed)
        );
    }

    // Untagging one resource leaves the others exactly as they were.
    let (_, first_arn, _) = &tagged[0];
    let untagged = dz
        .call("DELETE", &untag_uri(first_arn, &["owner"]), None)
        .await;
    assert_eq!(
        untagged.status, 200,
        "untagging the first resource: expected 200, got {} {:?}",
        untagged.status, untagged.body
    );
    for (label, arn, expected) in tagged.iter().skip(1) {
        let listed = dz.call("GET", &tags_uri(arn), None).await;
        assert_eq!(
            &reported_tags(&listed),
            expected,
            "case {label:?}: untagging another resource must not change this one, got {:?}",
            reported_tags(&listed)
        );
    }
}

/// One unresolvable `resourceArn`, with the answer each tagging operation owes it.
struct RefusalCase {
    label: &'static str,
    candidate: String,
    /// What `TagResource` and `ListTagsForResource` answer: both model
    /// `ValidationException`, so text that is not an ARN of this service is a bad
    /// request to them.
    tag_and_list: (u16, &'static str),
    /// What `UntagResource` answers: it models no client-input error, so every ARN it
    /// cannot resolve gets the one refusal it does model.
    untag: (u16, &'static str),
}

#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_refused() {
    const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");
    const INVALID: (u16, &str) = (400, "ValidationException");

    let dz = Emulator::new();
    let arn = dz.available_domain_arn("tagging-refusals").await;

    // The rows differ along HOW the ARN fails to name a stored resource: well formed
    // but unknown, well formed but elsewhere, and not an ARN of this service at all.
    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            label: "a well-formed DataZone ARN naming no stored domain",
            candidate: "arn:aws:datazone:us-west-2:000000000000:domain/dzd_absent".to_string(),
            tag_and_list: NOT_FOUND,
            untag: NOT_FOUND,
        },
        RefusalCase {
            label: "a stored domain's ARN in another region",
            candidate: arn.replace("us-west-2", "eu-west-1"),
            tag_and_list: NOT_FOUND,
            untag: NOT_FOUND,
        },
        RefusalCase {
            label: "text that is not an ARN at all",
            candidate: "not-an-arn".to_string(),
            tag_and_list: INVALID,
            untag: NOT_FOUND,
        },
        RefusalCase {
            label: "an ARN of another service",
            candidate: "arn:aws:s3:::example-bucket".to_string(),
            tag_and_list: INVALID,
            untag: NOT_FOUND,
        },
        RefusalCase {
            label: "a DataZone ARN with an empty resource part",
            candidate: "arn:aws:datazone:us-west-2:000000000000:".to_string(),
            tag_and_list: INVALID,
            untag: NOT_FOUND,
        },
        RefusalCase {
            label: "an ARN missing its partition",
            candidate: "arn::datazone:us-west-2:000000000000:domain/dzd_x".to_string(),
            tag_and_list: INVALID,
            untag: NOT_FOUND,
        },
    ];

    for case in cases {
        let (label, candidate, resolved, untag_expected) =
            (case.label, case.candidate, case.tag_and_list, case.untag);
        let tagged = dz
            .call(
                "POST",
                &tags_uri(&candidate),
                Some(json!({ "tags": { "team": "data" } })),
            )
            .await;
        assert_eq!(
            (tagged.status, tagged.error_code.as_str()),
            resolved,
            "case {label:?}: unexpected TagResource answer {:?}",
            tagged.body
        );

        let listed = dz.call("GET", &tags_uri(&candidate), None).await;
        assert_eq!(
            (listed.status, listed.error_code.as_str()),
            resolved,
            "case {label:?}: unexpected ListTagsForResource answer {:?}",
            listed.body
        );

        let untagged = dz
            .call("DELETE", &untag_uri(&candidate, &["team"]), None)
            .await;
        assert_eq!(
            (untagged.status, untagged.error_code.as_str()),
            untag_expected,
            "case {label:?}: unexpected UntagResource answer {:?}",
            untagged.body
        );
    }

    // A refused tag call wrote nothing to the resource that does exist.
    let listed = dz.call("GET", &tags_uri(&arn), None).await;
    assert_eq!(
        reported_tags(&listed),
        BTreeMap::new(),
        "a refused call must not have tagged the stored resource, got {:?}",
        reported_tags(&listed)
    );
}

#[tokio::test]
async fn tags_of_a_deleted_resource_are_gone_with_it() {
    let dz = Emulator::new();
    let id = dz.available_domain("tagging-deleted").await;
    let arn = dz
        .call("GET", &format!("/v2/domains/{id}"), None)
        .await
        .s("arn");
    let tagged = dz
        .call(
            "POST",
            &tags_uri(&arn),
            Some(json!({ "tags": { "team": "data" } })),
        )
        .await;
    assert_eq!(
        tagged.status, 200,
        "tagging the domain: expected 200, got {} {:?}",
        tagged.status, tagged.body
    );

    let deleted = dz.call("DELETE", &format!("/v2/domains/{id}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "deleting the domain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    // The delete the clock owes finishes, and the resource is gone with its tags.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;

    for (operation, method, uri) in [
        ("ListTagsForResource", "GET", tags_uri(&arn)),
        ("UntagResource", "DELETE", untag_uri(&arn, &["team"])),
    ] {
        let answer = dz.call(method, &uri, None).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "{operation} on a deleted resource: unexpected answer {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_stores_nothing() {
    let dz = Emulator::new();
    let arn = dz.available_domain_arn("tagging-reserved").await;
    let kept = vec![("team".to_string(), "data".to_string())];
    let seeded = dz
        .call("POST", &tags_uri(&arn), Some(tags_body(&kept)))
        .await;
    assert_eq!(
        seeded.status, 200,
        "seeding one tag: expected 200, got {} {:?}",
        seeded.status, seeded.body
    );

    // AWS reserves the `aws:` tag-key prefix for system tags on every service, in any
    // letter case; the rows differ in case and in whether the request also carries an
    // admissible key, which must not be stored either.
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "a reserved key alone",
            vec![("aws:createdBy".to_string(), "someone".to_string())],
        ),
        (
            "a reserved key in upper case",
            vec![("AWS:cloudformation".to_string(), "stack".to_string())],
        ),
        (
            "a reserved key beside an admissible one",
            vec![
                ("aws:region".to_string(), "us-west-2".to_string()),
                ("would-be-stored".to_string(), "no".to_string()),
            ],
        ),
    ];

    for (label, pairs) in cases {
        let answer = dz
            .call("POST", &tags_uri(&arn), Some(tags_body(&pairs)))
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected a refusal, got {} {:?}",
            answer.status,
            answer.body
        );
        let listed = dz.call("GET", &tags_uri(&arn), None).await;
        assert_eq!(
            reported_tags(&listed),
            kept.iter().cloned().collect::<BTreeMap<_, _>>(),
            "case {label:?}: a refused call must store nothing, got {:?}",
            reported_tags(&listed)
        );
    }

    // A key that merely CONTAINS `aws:` elsewhere is admissible.
    let allowed = vec![("team:aws:zone".to_string(), "west".to_string())];
    let answer = dz
        .call("POST", &tags_uri(&arn), Some(tags_body(&allowed)))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: the prefix rule is on the START of the key only, got {} {:?}",
        "aws: inside the key", answer.status, answer.body
    );
}

#[tokio::test]
async fn a_resource_carries_at_most_the_bounded_number_of_tags() {
    let max = service_com_amazonaws_datazone_datazone::state::MAX_TAGS_PER_RESOURCE;
    let dz = Emulator::new();
    let arn = dz.available_domain_arn("tagging-bound").await;

    // Exactly the bound is accepted.
    let full: Vec<(String, String)> = (0..max)
        .map(|i| (format!("key{i}"), format!("value{i}")))
        .collect();
    let tagged = dz
        .call("POST", &tags_uri(&arn), Some(tags_body(&full)))
        .await;
    assert_eq!(
        tagged.status, 200,
        "case {max} tags: expected 200 from TagResource, got {} {:?}",
        tagged.status, tagged.body
    );
    let expected: BTreeMap<String, String> = full.iter().cloned().collect();

    // Replacing a key already stored never grows the set, so it is still accepted.
    let replaced = dz
        .call(
            "POST",
            &tags_uri(&arn),
            Some(tags_body(&[("key0".to_string(), "replaced".to_string())])),
        )
        .await;
    assert_eq!(
        replaced.status, 200,
        "case replace at the bound: expected 200, got {} {:?}",
        replaced.status, replaced.body
    );

    // One key past the bound is refused, and the stored set is left as it was.
    let over = dz
        .call(
            "POST",
            &tags_uri(&arn),
            Some(tags_body(&[("one-too-many".to_string(), "x".to_string())])),
        )
        .await;
    assert_eq!(
        (over.status, over.error_code.as_str()),
        (400, "ValidationException"),
        "case {} tags: expected a refusal, got {} {:?}",
        max + 1,
        over.status,
        over.body
    );

    let listed = dz.call("GET", &tags_uri(&arn), None).await;
    let mut still_expected = expected;
    still_expected.insert("key0".to_string(), "replaced".to_string());
    assert_eq!(
        reported_tags(&listed),
        still_expected,
        "a refused call must leave the stored tag set unchanged, got {:?}",
        reported_tags(&listed)
    );
}
