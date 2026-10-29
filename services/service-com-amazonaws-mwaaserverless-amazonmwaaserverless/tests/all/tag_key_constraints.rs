// SPDX-License-Identifier: CC0-1.0

//! The modelled constraints of a `Tags` map's KEY, on every operation that
//! accepts the map.
//!
//! RULE under test: a map member's KEY is constrained by the shape the model
//! makes the key target, exactly as its VALUE is constrained by the value
//! shape, so every key a request supplies is checked against that shape before
//! anything is stored — a key outside it is HTTP 400 `ValidationException`
//! (`Reason` = `fieldValidationFailed`, a `FieldList` entry naming the member
//! that carried the map) and writes nothing, and a key inside it is persisted
//! and readable back. Here that shape is `TagKey`, `@length(min: 1, max: 128)`
//! plus `@pattern(^.*$)`, which the key of `Tags` and the element of `TagKeys`
//! BOTH target — so the keys `CreateWorkflow` and `TagResource` accept are
//! exactly the keys `UntagResource` can name, and no stored key is unremovable.
//!
//! The table spans the class the rule generalises over: the key's LENGTH across
//! the published range (below the minimum, at the minimum, at the maximum, one
//! past it and far past it), its FORM (plain, punctuated, upper-case, numeric,
//! spaced, non-ASCII — so length counts characters rather than bytes), and the
//! POSITION of the offending key in the map (alone, and beside a legal one).
//! Every row is driven over the real wire path through the shared `Harness` and
//! read back through the implemented `ListTagsForResource`, so what is asserted
//! is what a client sees.

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    M_TAGS_KEY, REASON_FIELD_VALIDATION_FAILED,
};

use crate::resource_tag_listing::{list_tags, workflow_with_tags};
use crate::workflow_lifecycle::{role_arn, Harness};

/// `TagKey`'s published length minimum (`smithy_query(query="shape",
/// shape="Tags")`: the key targets `TagKey`, `@length(min: 1, max: 128)`).
const MIN_TAG_KEY_LEN: usize = 1;

/// `TagKey`'s published length maximum, from the same shape.
const MAX_TAG_KEY_LEN: usize = 128;

/// One row: a label, a tag key, and whether the modelled key shape admits it.
struct KeyCase {
    label: String,
    key: String,
    accepted: bool,
}

fn case(label: &str, key: String, accepted: bool) -> KeyCase {
    KeyCase {
        label: label.to_string(),
        key,
        accepted,
    }
}

/// Every row of the table: each published bound, one step outside each, and the
/// key forms `@pattern(^.*$)` admits.
fn key_cases() -> Vec<KeyCase> {
    // A multi-byte key: 4 characters per repeat, 8 bytes per repeat. At 32
    // repeats it is exactly MAX_TAG_KEY_LEN CHARACTERS and twice that in bytes,
    // so a row that passes proves the bound counts characters.
    let wide_at_max = "ключ".repeat(MAX_TAG_KEY_LEN / 4);
    let wide_past_max = format!("{wide_at_max}х");

    vec![
        case(
            &format!(
                "an empty key, one below TagKey's minimum of {MIN_TAG_KEY_LEN} character"
            ),
            String::new(),
            false,
        ),
        case(
            &format!("a key of {MIN_TAG_KEY_LEN} character, TagKey's minimum"),
            "k".repeat(MIN_TAG_KEY_LEN),
            true,
        ),
        case(
            &format!("a key of {MAX_TAG_KEY_LEN} characters, TagKey's maximum"),
            "k".repeat(MAX_TAG_KEY_LEN),
            true,
        ),
        case(
            &format!(
                "a key of {} characters, one past TagKey's maximum of {MAX_TAG_KEY_LEN}",
                MAX_TAG_KEY_LEN + 1
            ),
            "k".repeat(MAX_TAG_KEY_LEN + 1),
            false,
        ),
        case(
            // 256 is `TagValue`'s maximum, not the key's: the key position is
            // not bounded by the value shape.
            "a key of 256 characters, far past TagKey's maximum",
            "k".repeat(256),
            false,
        ),
        case(
            &format!(
                "a non-ASCII key of {MAX_TAG_KEY_LEN} characters and {} bytes, at the maximum",
                wide_at_max.len()
            ),
            wide_at_max,
            true,
        ),
        case(
            &format!(
                "a non-ASCII key of {} characters, one past the maximum",
                MAX_TAG_KEY_LEN + 1
            ),
            wide_past_max,
            false,
        ),
        case(
            "a punctuated key, which @pattern(^.*$) admits",
            "Key.With-Dots_1".to_string(),
            true,
        ),
        case("a spaced key", "spaced key".to_string(), true),
        case("an all-digit key", "123".to_string(), true),
        case("an upper-case key", "UPPER".to_string(), true),
    ]
}

/// A `Tags` map carrying one entry.
fn one_tag(key: &str, value: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(key.to_string(), serde_json::Value::String(value.to_string()));
    serde_json::Value::Object(map)
}

/// Call `CreateWorkflow` with `tags` and report its status and body.
async fn create_with_tags(
    harness: &Harness,
    name: &str,
    tags: &serde_json::Value,
) -> (u16, serde_json::Value) {
    harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/d.yaml"},
                "RoleArn": role_arn(),
                "Tags": tags,
            }),
        )
        .await
}

/// Call `TagResource` with `tags` and report its status and body.
async fn tag(harness: &Harness, arn: &str, tags: &serde_json::Value) -> (u16, serde_json::Value) {
    harness
        .call(
            "TagResource",
            serde_json::json!({"ResourceArn": arn, "Tags": tags}),
        )
        .await
}

/// Call `UntagResource` with `keys` and report its status and body.
async fn untag(harness: &Harness, arn: &str, keys: &[String]) -> (u16, serde_json::Value) {
    harness
        .call(
            "UntagResource",
            serde_json::json!({"ResourceArn": arn, "TagKeys": keys}),
        )
        .await
}

/// How many workflows the endpoint holds.
async fn workflow_count(harness: &Harness) -> usize {
    let (status, listed) = harness
        .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
        .await;
    assert_eq!(
        status, 200,
        "expected ListWorkflows to succeed, got {status} {listed:?}"
    );
    listed["Workflows"].as_array().map_or(0, Vec::len)
}

/// The whole modelled refusal: 400 `ValidationException`, `Reason` =
/// `fieldValidationFailed`, and a `FieldList` entry naming the member whose map
/// carried the key, with a message that names it too.
fn assert_tags_refusal(label: &str, field: &str, status: u16, answered: &serde_json::Value) {
    assert_eq!(
        status, 400,
        "case {label:?}: expected HTTP 400, got {status} {answered:?}"
    );
    assert_eq!(
        answered["__type"].as_str(),
        Some("ValidationException"),
        "case {label:?}: expected a ValidationException, got {:?}",
        answered["__type"]
    );
    assert_eq!(
        answered["Reason"].as_str(),
        Some(REASON_FIELD_VALIDATION_FAILED),
        "case {label:?}: expected Reason {REASON_FIELD_VALIDATION_FAILED:?}, got {:?}",
        answered["Reason"]
    );
    let field_list = answered["FieldList"].as_array().cloned().unwrap_or_default();
    let entry = field_list
        .iter()
        .find(|entry| {
            entry["Name"]
                .as_str()
                .is_some_and(|name| name.starts_with(field))
        })
        .unwrap_or_else(|| {
            panic!(
                "case {label:?}: expected a FieldList entry naming {field:?}, got {field_list:?}"
            )
        });
    let message = entry["Message"].as_str().unwrap_or_default();
    assert!(
        message.to_lowercase().contains(&field.to_lowercase()),
        "case {label:?}: the field message must name the member it refused, got {message:?}"
    );
}

#[tokio::test]
async fn a_tag_key_outside_its_modelled_constraint_is_refused_by_tag_resource() {
    for KeyCase {
        label,
        key,
        accepted,
    } in key_cases()
    {
        if accepted {
            continue;
        }
        // A fresh emulator per row, so one row's resource cannot answer another's
        // call.
        let harness = Harness::new("us-west-2");
        let carried = serde_json::json!({"env": "prod"});
        let arn = workflow_with_tags(&harness, &label, "tagged", Some(carried.clone())).await;

        let (status, answered) = tag(&harness, &arn, &one_tag(&key, "v")).await;
        assert_tags_refusal(&label, "Tags", status, &answered);

        // A refused call writes nothing: the tag set is exactly what it was.
        let (_, tags) = list_tags(&harness, &arn).await;
        assert_eq!(
            tags, carried,
            "case {label:?}: a refused tagging call must leave the tag set at {carried:?}, got \
             {tags:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_key_outside_its_modelled_constraint_is_refused_by_create_workflow() {
    for KeyCase {
        label,
        key,
        accepted,
    } in key_cases()
    {
        if accepted {
            continue;
        }
        let harness = Harness::new("eu-west-1");

        let (status, answered) = create_with_tags(&harness, "created", &one_tag(&key, "v")).await;
        assert_tags_refusal(&label, "Tags", status, &answered);

        // A refused create persists no workflow, so none can be holding the key.
        assert_eq!(
            workflow_count(&harness).await,
            0,
            "case {label:?}: a refused create must persist no workflow"
        );
    }
}

#[tokio::test]
async fn a_tag_key_outside_its_modelled_constraint_is_refused_beside_a_legal_one() {
    let harness = Harness::new("us-east-1");
    let carried = serde_json::json!({"env": "prod"});
    let arn = workflow_with_tags(&harness, "mixed", "mixed", Some(carried.clone())).await;

    for KeyCase {
        label,
        key,
        accepted,
    } in key_cases()
    {
        if accepted {
            continue;
        }
        // The offending key sits beside a key the shape admits, and in both
        // orders of the map: one bad entry refuses the whole call, and the legal
        // entry beside it is not written either.
        let mut mixed = serde_json::Map::new();
        mixed.insert("legal".to_string(), serde_json::json!("v"));
        mixed.insert(key.clone(), serde_json::json!("v"));
        let label = format!("{label}, beside a legal key");

        let (status, answered) = tag(&harness, &arn, &serde_json::Value::Object(mixed)).await;
        assert_tags_refusal(&label, "Tags", status, &answered);

        let (_, tags) = list_tags(&harness, &arn).await;
        assert_eq!(
            tags, carried,
            "case {label:?}: a refused tagging call must write neither entry, got {tags:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_key_inside_its_modelled_constraint_is_persisted_by_both_writers() {
    for KeyCase {
        label,
        key,
        accepted,
    } in key_cases()
    {
        if !accepted {
            continue;
        }
        let expected = one_tag(&key, "v");

        // The create path: the key arrives with the resource and is readable back.
        let harness = Harness::new("us-west-2");
        let (status, created) = create_with_tags(&harness, "created", &expected).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a key inside TagKey's constraint must be accepted by \
             CreateWorkflow, got {status} {created:?}"
        );
        let arn = created["WorkflowArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: a created workflow has an ARN: {created:?}"))
            .to_string();
        let (_, tags) = list_tags(&harness, &arn).await;
        assert_eq!(
            tags, expected,
            "case {label:?}: expected the created tag set {expected:?}, got {tags:?}"
        );

        // The tagging path: the same key onto a resource that carries none.
        let harness = Harness::new("us-west-2");
        let arn = workflow_with_tags(&harness, &label, "tagged", None).await;
        let (status, answered) = tag(&harness, &arn, &expected).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a key inside TagKey's constraint must be accepted by TagResource, \
             got {status} {answered:?}"
        );
        let (_, tags) = list_tags(&harness, &arn).await;
        assert_eq!(
            tags, expected,
            "case {label:?}: expected the stored tag set {expected:?}, got {tags:?}"
        );
    }
}

/// The two sides of one shape agree: `Tags`' key and `TagKeys`' element both
/// target `TagKey`, so a key a write accepts can always be named for removal,
/// and a key a write refuses is refused for removal too.
#[tokio::test]
async fn every_key_a_write_accepts_untag_resource_can_remove() {
    for KeyCase {
        label,
        key,
        accepted,
    } in key_cases()
    {
        let harness = Harness::new("us-east-1");

        if accepted {
            let stored = one_tag(&key, "v");
            let arn = workflow_with_tags(&harness, &label, "removable", Some(stored.clone())).await;
            let (status, answered) = untag(&harness, &arn, std::slice::from_ref(&key)).await;
            assert_eq!(
                status, 200,
                "case {label:?}: a stored key must be removable, got {status} {answered:?}"
            );
            let (_, tags) = list_tags(&harness, &arn).await;
            assert_eq!(
                tags,
                serde_json::json!({}),
                "case {label:?}: expected the key to be removed, got {tags:?}"
            );
            continue;
        }

        // The other direction: a key the writers refuse is refused here too, so
        // the two ends of the bound cannot disagree.
        let arn = workflow_with_tags(&harness, &label, "removable", None).await;
        let (status, answered) = untag(&harness, &arn, std::slice::from_ref(&key)).await;
        assert_tags_refusal(&label, "TagKeys", status, &answered);
    }
}

/// The shadow schema the handlers hand the walk for a `Tags` KEY carries the
/// model's own key constraints, so the bound enforced on a key cannot drift from
/// the one the model declares (or from the one `TagKeys` enforces).
#[test]
fn the_tags_key_shadow_schema_carries_the_model_s_own_key_constraints() {
    use ::aws_smithy_schema_server::constraints::{LengthTrait, PatternTrait};

    let model = crate::validation_errors::smithy_model();
    let key_target = model["shapes"]["com.amazonaws.mwaaserverless#Tags"]["key"]["target"]
        .as_str()
        .expect("the model declares a target for the Tags key")
        .to_string();
    let declared = &model["shapes"][&key_target]["traits"];
    let min = declared["smithy.api#length"]["min"].as_i64();
    let max = declared["smithy.api#length"]["max"].as_i64();
    let pattern = declared["smithy.api#pattern"].as_str();

    // The table above is written against these published bounds.
    assert_eq!(
        (min, max),
        (Some(MIN_TAG_KEY_LEN as i64), Some(MAX_TAG_KEY_LEN as i64)),
        "the model's key bounds must be the ones this suite tests, got {min:?}..{max:?}"
    );

    let traits = M_TAGS_KEY
        .traits()
        .expect("the Tags key shadow schema carries constraints");
    let carried_length = traits
        .get(&LengthTrait::TRAIT_ID)
        .and_then(|found| found.as_any().downcast_ref::<LengthTrait>())
        .copied()
        .expect("the Tags key shadow schema carries the key's @length");
    assert_eq!(
        (carried_length.min, carried_length.max),
        (min, max),
        "the Tags key shadow schema must carry the model's @length {min:?}..{max:?}, got \
         {:?}..{:?}",
        carried_length.min,
        carried_length.max
    );
    let carried_pattern = traits
        .get(&PatternTrait::TRAIT_ID)
        .and_then(|found| found.as_any().downcast_ref::<PatternTrait>())
        .map(|found| found.regex);
    assert_eq!(
        carried_pattern, pattern,
        "the Tags key shadow schema must carry the model's @pattern {pattern:?}, got \
         {carried_pattern:?}"
    );

    // The key is reported under the member a request carried the map in.
    assert_eq!(
        M_TAGS_KEY.member_name(),
        Some("Tags"),
        "a bad key must be reported at the member that carried the map, got {:?}",
        M_TAGS_KEY.member_name()
    );

    // And `TagKeys`' element targets the same shape, which is why the two sides
    // of the bound agree.
    let element_target = model["shapes"]["com.amazonaws.mwaaserverless#TagKeys"]["member"]
        ["target"]
        .as_str()
        .expect("the model declares a target for the TagKeys element");
    assert_eq!(
        element_target, key_target,
        "the Tags key and the TagKeys element must target one shape, got {element_target:?} and \
         {key_target:?}"
    );
}
