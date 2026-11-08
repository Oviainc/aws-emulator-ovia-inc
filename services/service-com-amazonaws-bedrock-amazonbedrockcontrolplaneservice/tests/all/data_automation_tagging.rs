// SPDX-License-Identifier: CC0-1.0

//! The Bedrock Data Automation half of the three tagging paths this service binds, driven
//! over the real wire path: both services mounted on ONE engine behind one
//! `MultiServiceRouter`, exactly as `core/router` assembles them, so what is asserted is
//! what a client of either endpoint sees.
//!
//! RULE. `POST /tagResource`, `POST /untagResource` and `POST /listTagsForResource` are
//! bound identically by this control plane and by `bedrock-data-automation`, both sign as
//! `bedrock`, and the family's claim rule gives the whole segment to ONE claimant — so the
//! TARGET named in the request's body decides which model serves it: a `resourceARN` of a
//! Bedrock Data Automation resource is served out of THAT model (its own
//! `TaggableResourceArn`, its own handler, its own store and its own refusal wording), and
//! every other request stays this service's.
//!
//! The suites are TABLE-driven. The rows differ along what the rule generalises over: the
//! resource FAMILY the ARN names (a blueprint, a data-automation project — and this
//! service's own resources and bare names, which must still be served here), the OPERATION
//! (tag, list, untag), the tag SET (a first set, a merge over it, a key removed, a key the
//! resource never carried), and the refusals (an ARN of that service naming no resource it
//! holds, and a tag collection past the modelled maximum).

use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice as bda;
use std::sync::Arc;

use crate::support::{error_code, Reply, ACCOUNT, REGION};

/// The most tags a Bedrock Data Automation resource holds: the maximum its modelled
/// `TagList` carries, which is the bound its own tagging handler enforces.
const BDA_MAX_TAGS: usize = bda::state::tagging::MAX_TAGS_PER_RESOURCE;

/// This control plane beside the Bedrock Data Automation build-time service on ONE shared
/// engine, which is how `core/router` mounts them: the two are told apart by the path space
/// their models publish, and on the three paths they BOTH publish by the ARN the body names.
struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(bda::EmulatedService::new(Arc::clone(&engine)));
    Emulator { router }
}

impl Emulator {
    /// One request signed for [`REGION`], as a client of either endpoint sends it.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(&value).expect("a serializable request body")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://bedrock.{REGION}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock/\
                     aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|value| error_code(value).to_string());
        let body: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(|value| error_code(value).to_string())
        });
        Reply {
            status,
            content_type,
            body,
            error,
        }
    }
}

/// A `TagList` out of `(key, value)` pairs.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect(),
    )
}

/// `n` distinct tags, each key and value inside the modelled `TagKey`/`TagValue` patterns.
fn many_tags(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| {
                json!({ "key": format!("key-{index}"), "value": format!("value-{index}") })
            })
            .collect(),
    )
}

async fn tag(emu: &Emulator, resource_arn: &str, tags: Value) -> Reply {
    emu.call(
        "POST",
        "/tagResource",
        Some(json!({ "resourceARN": resource_arn, "tags": tags })),
    )
    .await
}

async fn untag(emu: &Emulator, resource_arn: &str, keys: &[&str]) -> Reply {
    emu.call(
        "POST",
        "/untagResource",
        Some(json!({ "resourceARN": resource_arn, "tagKeys": keys })),
    )
    .await
}

async fn list_tags(emu: &Emulator, resource_arn: &str) -> Reply {
    emu.call(
        "POST",
        "/listTagsForResource",
        Some(json!({ "resourceARN": resource_arn })),
    )
    .await
}

/// The tags a listing reported, as `(key, value)` pairs in the order served.
fn listed(case: &str, reply: &Reply) -> Vec<(String, String)> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ListTagsForResource must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .body
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no tags in {:?}", reply.body))
        .iter()
        .map(|tag| {
            (
                tag.get("key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                tag.get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// A Bedrock Data Automation blueprint, created through that service's own
/// `PUT /blueprints/`, carrying the tags its create named. Returns its ARN.
async fn given_blueprint(emu: &Emulator, name: &str, tags: Value) -> String {
    let reply = emu
        .call(
            "PUT",
            "/blueprints/",
            Some(json!({
                "blueprintName": name,
                "type": "DOCUMENT",
                "schema": r#"{"class":"invoice"}"#,
                "tags": tags,
            })),
        )
        .await;
    assert_eq!(
        reply.status, 201,
        "the fixture blueprint must be created, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .body
        .get("blueprint")
        .and_then(|blueprint| blueprint.get("blueprintArn"))
        .and_then(Value::as_str)
        .expect("the create answers the blueprint's ARN")
        .to_string()
}

/// A Bedrock Data Automation project, created through that service's own
/// `PUT /data-automation-projects/`. Returns its ARN.
async fn given_project(emu: &Emulator, name: &str) -> String {
    let reply = emu
        .call(
            "PUT",
            "/data-automation-projects/",
            Some(json!({
                "projectName": name,
                "standardOutputConfiguration": {},
            })),
        )
        .await;
    assert_eq!(
        reply.status, 201,
        "the fixture project must be created, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .body
        .get("projectArn")
        .and_then(Value::as_str)
        .expect("the create answers the project's ARN")
        .to_string()
}

/// One resource of the Bedrock Data Automation service a tagging call can address: the
/// case it stands for, its ARN, and the tags its create carried.
struct Target {
    case: &'static str,
    arn: String,
    created_with: Vec<(String, String)>,
}

/// RULE: a tagging call naming a Bedrock Data Automation resource is SERVED — tags merge
/// into the list the resource holds, a listing reports exactly that list, and untagging
/// drops exactly the keys it names — for every family of that service a tagging call can
/// address.
#[tokio::test]
async fn tagging_a_data_automation_resource_round_trips_through_its_own_model() {
    let emu = emulator();

    let targets: Vec<Target> = vec![
        Target {
            case: "a blueprint, created with tags",
            arn: given_blueprint(&emu, "tagged-blueprint", tag_list(&[("team", "data-eng")]))
                .await,
            created_with: vec![("team".to_string(), "data-eng".to_string())],
        },
        Target {
            case: "a blueprint, created with none",
            arn: given_blueprint(&emu, "untagged-blueprint", tag_list(&[])).await,
            created_with: Vec::new(),
        },
        Target {
            case: "a data-automation project",
            arn: given_project(&emu, "tagged-project").await,
            created_with: Vec::new(),
        },
    ];

    for Target {
        case,
        arn,
        created_with,
    } in targets
    {
        // The tags the CREATE carried are the list the resource holds.
        assert_eq!(
            listed(case, &list_tags(&emu, &arn).await),
            created_with,
            "case {case:?}: the listing must report the tags the create carried"
        );

        // A first set, then a merge over it: a key already held takes the new value in
        // place, a key that is not held is added, and nothing else moves.
        let set = tag(&emu, &arn, tag_list(&[("env", "test"), ("owner", "ada")])).await;
        assert_eq!(
            set.status, 200,
            "case {case:?}: TagResource must answer 200, got {} {:?}",
            set.status, set.body
        );
        let merged = tag(&emu, &arn, tag_list(&[("env", "prod")])).await;
        assert_eq!(
            merged.status, 200,
            "case {case:?}: a merging TagResource must answer 200, got {} {:?}",
            merged.status, merged.body
        );
        let mut expected = created_with.clone();
        expected.push(("env".to_string(), "prod".to_string()));
        expected.push(("owner".to_string(), "ada".to_string()));
        assert_eq!(
            listed(case, &list_tags(&emu, &arn).await),
            expected,
            "case {case:?}: the listing must report one value per key, the last set"
        );

        // Untagging drops exactly the keys it names, and a key the resource never carried
        // removes nothing.
        let dropped = untag(&emu, &arn, &["env", "never-set"]).await;
        assert_eq!(
            dropped.status, 200,
            "case {case:?}: UntagResource must answer 200, got {} {:?}",
            dropped.status, dropped.body
        );
        expected.retain(|(key, _)| key != "env");
        assert_eq!(
            listed(case, &list_tags(&emu, &arn).await),
            expected,
            "case {case:?}: the listing must report the remaining tags"
        );
    }
}

/// RULE: an ARN of a Bedrock Data Automation resource type naming no resource that store
/// holds is refused 404 `ResourceNotFoundException`, by every one of the three operations,
/// with that service's recorded sentence for THIS error class — `<Kind> <resourceARN> does
/// not exist`, the kind word for the resource kind the ARN spells around the ARN the request
/// carried — never the control plane's `ValidationException` about a pattern that admits no
/// such resource type.
///
/// Each expectation is the WHOLE sentence built HERE from the row's own ARN, not read back
/// off the service.
#[tokio::test]
async fn an_unheld_data_automation_arn_is_a_not_found_from_its_own_model() {
    let emu = emulator();

    // (case, the ARN the call addresses, the kind word its refusal names it by)
    let absent: Vec<(&str, String, &str)> = vec![
        (
            "a blueprint ARN naming no blueprint",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:blueprint/000000000000-0000"),
            "Blueprint",
        ),
        (
            "a project ARN naming no project",
            format!(
                "arn:aws:bedrock:{REGION}:{ACCOUNT}:data-automation-project/000000000000-0000"
            ),
            "Project",
        ),
        (
            "a library ARN naming no library",
            format!(
                "arn:aws:bedrock:{REGION}:{ACCOUNT}:data-automation-library/000000000000-0000"
            ),
            "Library",
        ),
        (
            "an invocation ARN naming no optimization job",
            format!(
                "arn:aws:bedrock:{REGION}:{ACCOUNT}:blueprint-optimization-invocation/\
                 000000000000-0000"
            ),
            "Invocation",
        ),
    ];

    for (case, arn, kind) in absent {
        let expected = format!("{kind} {arn} does not exist");
        let calls: Vec<(&str, Reply)> = vec![
            ("TagResource", tag(&emu, &arn, tag_list(&[("a", "b")])).await),
            ("UntagResource", untag(&emu, &arn, &["a"]).await),
            ("ListTagsForResource", list_tags(&emu, &arn).await),
        ];
        for (operation, reply) in calls {
            let case = format!("{case} / {operation}");
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected 404, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                reply.error
            );
            let message = reply
                .body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {case:?}: expected that service's recorded sentence, got {message:?}"
            );
        }
    }
}

/// RULE: a tag collection past the maximum the Bedrock Data Automation model declares is
/// refused 400 `ValidationException` naming ONLY that member — one validation error, about
/// `tags`, with no value rendered for a collection and no second finding about the
/// `resourceARN` that model admits.
#[tokio::test]
async fn a_tag_collection_past_the_modelled_maximum_is_the_only_finding() {
    let emu = emulator();
    let arn = given_blueprint(&emu, "tag-bound-blueprint", tag_list(&[])).await;

    // (case, how many tags the request carries, whether it is refused)
    let rows: [(&str, usize, bool); 3] = [
        ("one under the maximum", BDA_MAX_TAGS - 1, false),
        ("exactly the maximum", BDA_MAX_TAGS, false),
        ("one over the maximum", BDA_MAX_TAGS + 1, true),
    ];
    for (case, count, refused) in rows {
        let reply = tag(&emu, &arn, many_tags(count)).await;
        if !refused {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                reply.status, reply.body
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            reply.error
        );
        let message = reply
            .body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message,
            format!(
                "1 validation error detected: Value at 'tags' failed to satisfy constraint: \
                 Member must have length less than or equal to {BDA_MAX_TAGS}"
            ),
            "case {case:?}: expected the one clause about the collection, got {message:?}"
        );
    }

    // The refused request left the resource's tags exactly as they were: the last ACCEPTED
    // call's set, not the oversized one.
    let held = listed("after the refusal", &list_tags(&emu, &arn).await);
    assert_eq!(
        held.len(),
        BDA_MAX_TAGS,
        "case \"after the refusal\": expected the accepted set to stand, got {held:?}"
    );
}

/// RULE: a MERGE that would leave the resource holding more than the maximum its tag
/// collection carries is over the service's quota for tags on one resource — refused 402
/// `ServiceQuotaExceededException` with that service's fixed quota sentence — while a merge
/// that lands on the maximum is served. Only the keys the resource does not already hold
/// grow it, so re-setting a key it holds is never over the quota.
#[tokio::test]
async fn a_merge_past_the_tag_quota_is_refused_and_applies_nothing() {
    let emu = emulator();
    let arn = given_blueprint(&emu, "tag-quota-blueprint", tag_list(&[])).await;

    // Three quarters of the quota is held before any row runs.
    let held_count = BDA_MAX_TAGS * 3 / 4;
    let first = tag(&emu, &arn, many_tags(held_count)).await;
    assert_eq!(
        first.status, 200,
        "the held set must be accepted, got {} {:?}",
        first.status, first.body
    );

    // (case, the tags the merge applies, whether the merge is over the quota)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "re-setting keys the resource already holds",
            many_tags(held_count),
            false,
        ),
        (
            "new keys landing exactly on the quota",
            Value::Array(
                (held_count..BDA_MAX_TAGS)
                    .map(|index| json!({ "key": format!("extra-{index}"), "value": "v" }))
                    .collect(),
            ),
            false,
        ),
        (
            "one new key past the quota",
            tag_list(&[("one-too-many", "v")]),
            true,
        ),
    ];
    for (case, tags, over_quota) in rows {
        let reply = tag(&emu, &arn, tags).await;
        if !over_quota {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                reply.status, reply.body
            );
            continue;
        }
        assert_eq!(
            reply.status, 402,
            "case {case:?}: expected 402, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {case:?}: expected ServiceQuotaExceededException, got {:?}",
            reply.error
        );
        let message = reply
            .body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message,
            format!(
                "the quota of {BDA_MAX_TAGS} tags per resource has been reached; remove a tag \
                 before adding another"
            ),
            "case {case:?}: expected that service's quota sentence, got {message:?}"
        );
    }

    // The refused merge applied nothing: the resource still holds exactly the quota, and
    // the key it named is not among them.
    let held = listed("after the refused merge", &list_tags(&emu, &arn).await);
    assert_eq!(
        held.len(),
        BDA_MAX_TAGS,
        "case \"after the refused merge\": expected the quota's worth of tags, got {}",
        held.len()
    );
    assert!(
        !held.iter().any(|(key, _)| key == "one-too-many"),
        "case \"after the refused merge\": the refused key must not have been applied"
    );
}

/// RULE: only an ARN of a Bedrock Data Automation resource is served out of that model —
/// this service's OWN tagging surface is untouched, so a request naming one of its
/// resources, or naming no resource of either service, is answered by this service exactly
/// as it was.
#[tokio::test]
async fn this_services_own_tagging_surface_is_untouched() {
    let emu = emulator();

    // A guardrail of this control plane, tagged through the same three paths.
    let created = emu
        .call(
            "POST",
            "/guardrails",
            Some(json!({
                "name": "tagging-guardrail",
                "blockedInputMessaging": "blocked in",
                "blockedOutputsMessaging": "blocked out",
            })),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "the fixture guardrail must be created, got {} {:?}",
        created.status, created.body
    );
    let guardrail_arn = created
        .body
        .get("guardrailArn")
        .and_then(Value::as_str)
        .expect("the create answers the guardrail's ARN")
        .to_string();

    let set = tag(&emu, &guardrail_arn, tag_list(&[("team", "trust")])).await;
    assert_eq!(
        set.status, 200,
        "case \"this service's own guardrail\": TagResource must answer 200, got {} {:?}",
        set.status, set.body
    );
    assert_eq!(
        listed(
            "this service's own guardrail",
            &list_tags(&emu, &guardrail_arn).await
        ),
        vec![("team".to_string(), "trust".to_string())],
        "case \"this service's own guardrail\": the listing must report the tag it set"
    );

    // A value neither model's taggable-resource form admits is still refused by THIS
    // service, which claims the path: the request is not a Bedrock Data Automation one.
    let refused = tag(
        &emu,
        "arn:aws:s3:::not-a-bedrock-resource",
        tag_list(&[("team", "trust")]),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case \"another vendor's ARN\": expected 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ValidationException"),
        "case \"another vendor's ARN\": expected ValidationException, got {:?}",
        refused.error
    );
}
