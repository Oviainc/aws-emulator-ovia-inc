// SPDX-License-Identifier: CC0-1.0

//! The per-resource TAG CEILING, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. A GuardDuty resource carries at most the PUBLISHED number of tags: "a limit of
//!    50 tags per resource" (`smithy_query(query="doc",
//!    shape="ListTagsForResource")`). A write that would leave a resource carrying
//!    more is refused with HTTP 400 under the `InvalidInputException` class, which
//!    names the limit, echoes none of the caller's keys, and persists nothing.
//! 2. That ceiling is a property of the RESOURCE, so it holds however the tags
//!    arrive: through the `Tags` member of a create, through `TagResource`, and
//!    through several calls that each fit on their own.
//! 3. It is a DIFFERENT quantity from the `len 1..200` the model puts on `TagMap`
//!    (`smithy_query(shape="TagMap")`), which bounds how many pairs ONE request's
//!    map may carry: a request inside that bound is still refused when the resource
//!    it writes to would end up past the ceiling.
//! 4. A key the resource already carries is overwritten rather than appended, so it
//!    grows the resource by nothing and is accepted even at the ceiling.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, enabled_detector, error_code, new_router, published_rule_ids, Router,
};
use crate::resource_tag_addition::tag_in;
use crate::resource_tag_removal::{tagged_resources, tags_of};

/// The most tags one resource may carry, as this service's own documentation
/// publishes it: "Tagging is currently supported for detectors, finding filters, IP
/// sets, threat intel sets, and publishing destination, with a limit of 50 tags per
/// resource" (`smithy_query(query="doc", shape="ListTagsForResource")`). Written out
/// here rather than taken from the crate, so the figure is pinned at the wire.
const PUBLISHED_CEILING: usize = 50;

/// The most entries ONE request's map may carry: `TagMap` is `len 1..200`
/// (`smithy_query(shape="TagMap")`). A request of this size is inside the model's
/// bound and still past the ceiling above.
const TAG_MAP_MAX_ENTRIES: usize = 200;

/// The Region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// A KMS key ARN for the publishing destination create. Nothing is published, so the
/// value is only ever stored and reported back.
const KEY_ARN: &str = "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// An IAM role ARN of the form `CreateMalwareProtectionPlan.Role` takes.
const ROLE: &str = "arn:aws:iam::000000000000:role/GuardDutyMalwareProtectionRole";

/// `count` distinct tags, keyed `{prefix}-{index}`.
///
/// The keys are generated rather than listed because what the rule generalises over
/// is their COUNT; each is inside `TagKey`'s own `len 1..128` and pattern
/// (`smithy_query(shape="TagMap")`).
fn generated_tags(prefix: &str, count: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..count {
        map.insert(
            format!("{prefix}-{index}"),
            json!(format!("value-{index}")),
        );
    }
    Value::Object(map)
}

/// The keys a tag map carries, in the order the body holds them.
fn keys_of(tags: &Value) -> Vec<String> {
    tags.as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Rules 1 + 2 + 3: the way IN through a create. The ceiling is written out above
// rather than read from the crate, so these rows pin the published figure itself
// at the wire, and a map at `TagMap`'s own maximum is one of them.
// ---------------------------------------------------------------------------

/// One create that carries a `Tags` member.
struct CreateCase {
    case: &'static str,
    /// The URI the create is bound to.
    path: String,
    /// The create's body WITHOUT its `tags` member.
    body: Value,
    /// Where the create's response carries the id it minted.
    id_at: &'static [&'static str],
    /// The resource's OWN read URI, built from that id.
    read_path: Box<dyn Fn(&str) -> String>,
}

/// `CreateDetector`, which needs a store holding no detector of its own: GuardDuty
/// allows one detector per account per Region.
fn detector_create() -> CreateCase {
    CreateCase {
        case: "CreateDetector",
        path: "/detector".to_string(),
        body: json!({"enable": true}),
        id_at: &["detectorId"],
        read_path: Box::new(|id| format!("/detector/{id}")),
    }
}

/// Every OTHER create that carries a `Tags` member, against a detector this creates
/// in `router` first. The detector itself is created untagged, so each row starts
/// from a resource whose only tags are the ones its own create carries.
async fn dependent_creates(router: &Router) -> Vec<CreateCase> {
    let detector = enabled_detector(router).await;
    let base = format!("/detector/{detector}");
    let rule_id = published_rule_ids(router)
        .await
        .into_iter()
        .next()
        .expect("the catalogue publishes at least one rule");

    // The lists and the entity sets share the create shape; only the collection they
    // land in differs.
    let list_body = |name: &str| {
        json!({
            "name": name,
            "format": "TXT",
            "location": "s3://guardduty-lists/list.txt",
            "activate": false,
        })
    };

    let filter_base = base.clone();
    let ip_set_base = base.clone();
    let threat_intel_base = base.clone();
    let trusted_base = base.clone();
    let threat_base = base.clone();
    let destination_base = base.clone();
    vec![
        CreateCase {
            case: "CreateFilter",
            path: format!("{base}/filter"),
            body: json!({
                "name": "ceiling-filter",
                "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 4}}},
            }),
            id_at: &["name"],
            read_path: Box::new(move |id| format!("{filter_base}/filter/{id}")),
        },
        CreateCase {
            case: "CreateIPSet",
            path: format!("{base}/ipset"),
            body: list_body("ceiling-ip-set"),
            id_at: &["ipSetId"],
            read_path: Box::new(move |id| format!("{ip_set_base}/ipset/{id}")),
        },
        CreateCase {
            case: "CreateThreatIntelSet",
            path: format!("{base}/threatintelset"),
            body: list_body("ceiling-threat-intel-set"),
            id_at: &["threatIntelSetId"],
            read_path: Box::new(move |id| format!("{threat_intel_base}/threatintelset/{id}")),
        },
        CreateCase {
            case: "CreateTrustedEntitySet",
            path: format!("{base}/trustedentityset"),
            body: list_body("ceiling-trusted-set"),
            id_at: &["trustedEntitySetId"],
            read_path: Box::new(move |id| format!("{trusted_base}/trustedentityset/{id}")),
        },
        CreateCase {
            case: "CreateThreatEntitySet",
            path: format!("{base}/threatentityset"),
            body: list_body("ceiling-threat-set"),
            id_at: &["threatEntitySetId"],
            read_path: Box::new(move |id| format!("{threat_base}/threatentityset/{id}")),
        },
        CreateCase {
            case: "CreatePublishingDestination",
            path: format!("{base}/publishingDestination"),
            body: json!({
                "destinationType": "S3",
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::guardduty-findings",
                    "kmsKeyArn": KEY_ARN,
                },
            }),
            id_at: &["destinationId"],
            read_path: Box::new(move |id| {
                format!("{destination_base}/publishingDestination/{id}")
            }),
        },
        CreateCase {
            case: "CreateMalwareProtectionPlan",
            path: "/malware-protection-plan".to_string(),
            body: json!({
                "role": ROLE,
                "protectedResource": {"s3Bucket": {"bucketName": "guardduty-tag-ceiling"}},
            }),
            id_at: &["malwareProtectionPlanId"],
            read_path: Box::new(|id| format!("/malware-protection-plan/{id}")),
        },
        CreateCase {
            case: "CreateCustomDetectionRuleAssociation",
            path: "/custom-detection-rule/association".to_string(),
            body: json!({"ruleId": rule_id.clone(), "mode": "DRY_RUN"}),
            id_at: &["ruleAssociation", "associationId"],
            read_path: Box::new(move |id| {
                format!("/custom-detection-rule/rule/{rule_id}/association/{id}")
            }),
        },
    ]
}

/// Send `case`'s create carrying `tags`.
async fn create_with(router: &Router, case: &CreateCase, tags: &Value) -> (u16, Value) {
    let mut body = case.body.clone();
    body.as_object_mut()
        .expect("every create body here is a JSON object")
        .insert("tags".to_string(), tags.clone());
    call(router, "POST", &case.path, Some(body)).await
}

/// The id a create minted, from wherever its response carries it.
fn minted_id(body: &Value, case: &CreateCase) -> String {
    let mut cursor = body;
    for member in case.id_at {
        cursor = &cursor[*member];
    }
    cursor
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a minted id at {:?}, got {body:?}",
                case.case, case.id_at
            )
        })
        .to_string()
}

/// The `tags` member the resource's own read reports.
async fn read_tags(router: &Router, path: &str, case: &str) -> Value {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the read to answer 200, got {status} {body:?}"
    );
    let tags = body.get("tags").cloned().unwrap_or(Value::Null);
    assert!(
        tags.is_object(),
        "case {case:?}: every read reports a tag map, got {tags:?}"
    );
    tags
}

#[tokio::test]
async fn a_create_up_to_the_ceiling_stores_every_tag_it_carries() {
    for count in [1, PUBLISHED_CEILING - 1, PUBLISHED_CEILING] {
        let tags = generated_tags("created", count);

        // `CreateDetector` needs a store with no detector in it, so it runs on one of
        // its own; every other create hangs off a detector `dependent_creates` makes.
        let mut routers: Vec<(Router, Vec<CreateCase>)> = Vec::new();
        let detector_router = new_router();
        routers.push((detector_router, vec![detector_create()]));
        let shared = new_router();
        let dependents = dependent_creates(&shared).await;
        routers.push((shared, dependents));

        for (router, cases) in &routers {
            for case in cases {
                let label = format!("{} with {count} tags", case.case);
                let (status, body) = create_with(router, case, &tags).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: a map of {count} tags is at or below the ceiling of \
                     {PUBLISHED_CEILING}, so the create must be accepted, got {status} {body:?}"
                );
                let id = minted_id(&body, case);
                let actual = read_tags(router, &(case.read_path)(&id), &label).await;
                assert_eq!(
                    actual, tags,
                    "case {label:?}: expected the {count} tags the create carried, got {actual:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_create_past_the_ceiling_is_refused_and_persists_nothing() {
    // One past the ceiling, well past it, and a map at the modelled maximum of
    // `TagMap` — which the boundary validator accepts and the resource cannot hold.
    for count in [
        PUBLISHED_CEILING + 1,
        PUBLISHED_CEILING + 11,
        TAG_MAP_MAX_ENTRIES,
    ] {
        let tags = generated_tags("refused", count);
        let keys = keys_of(&tags);

        let mut routers: Vec<(Router, Vec<CreateCase>)> = Vec::new();
        let detector_router = new_router();
        routers.push((detector_router, vec![detector_create()]));
        let shared = new_router();
        let dependents = dependent_creates(&shared).await;
        routers.push((shared, dependents));

        for (router, cases) in &routers {
            for case in cases {
                let label = format!("{} with {count} tags", case.case);
                let (status, body) = create_with(router, case, &tags).await;
                assert_eq!(
                    status, 400,
                    "case {label:?}: a map of {count} tags is past the ceiling of \
                     {PUBLISHED_CEILING}, so the create must be refused, got {status} {body:?}"
                );
                let class = error_code(&body);
                assert_eq!(
                    class, INVALID_INPUT,
                    "case {label:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
                );
                let message = body["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(&PUBLISHED_CEILING.to_string()),
                    "case {label:?}: the refusal must name the limit {PUBLISHED_CEILING}, \
                     got {message:?}"
                );
                for key in keys.iter().take(3) {
                    assert!(
                        !message.contains(key.as_str()),
                        "case {label:?}: the refusal must not echo the caller's keys, \
                         got {message:?}"
                    );
                }

                // Nothing was persisted: the same create with one tag is accepted —
                // its name, bucket or pair is not taken — and the resource it mints
                // carries exactly that one tag.
                let single = generated_tags("accepted", 1);
                let (status, body) = create_with(router, case, &single).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: a refused create must leave nothing behind, so the \
                     same create with one tag must be accepted, got {status} {body:?}"
                );
                let id = minted_id(&body, case);
                let actual = read_tags(router, &(case.read_path)(&id), &label).await;
                assert_eq!(
                    actual, single,
                    "case {label:?}: expected only the one tag the accepted create \
                     carried, got {actual:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rules 2 + 4: the way IN through `TagResource`, over the same ceiling.
// ---------------------------------------------------------------------------

/// One `TagResource` call of a row: how many FRESH keys it carries, how many keys the
/// resource ALREADY carries it names again, and whether it is accepted.
struct Addition {
    fresh: usize,
    carried: usize,
    accepted: bool,
}

/// The rows differ along everything the rule generalises over: how many tags the
/// create stored, how many a later add carries, whether those keys are fresh or
/// already carried, how many calls the tags arrive in, and whether the request's own
/// size is inside the modelled bound on `TagMap`.
fn addition_rows() -> Vec<(&'static str, usize, Vec<Addition>)> {
    let add = |fresh: usize, carried: usize, accepted: bool| Addition {
        fresh,
        carried,
        accepted,
    };
    vec![
        (
            "one fresh key onto a resource created at the ceiling",
            PUBLISHED_CEILING,
            vec![add(1, 0, false)],
        ),
        (
            "only keys it already carries, at the ceiling",
            PUBLISHED_CEILING,
            vec![add(0, 5, true)],
        ),
        (
            "the one key that still fits",
            PUBLISHED_CEILING - 1,
            vec![add(1, 0, true)],
        ),
        (
            "two keys where only one would fit",
            PUBLISHED_CEILING - 1,
            vec![add(2, 0, false)],
        ),
        (
            "filling from one tag exactly to the ceiling",
            1,
            vec![add(PUBLISHED_CEILING - 1, 0, true)],
        ),
        (
            "one key past the ceiling in a single call",
            1,
            vec![add(PUBLISHED_CEILING, 0, false)],
        ),
        (
            "a request at the modelled map maximum, far past the ceiling",
            1,
            vec![add(TAG_MAP_MAX_ENTRIES, 0, false)],
        ),
        (
            "reaching the ceiling over three calls, then one key more",
            2,
            vec![
                add(24, 0, true),
                add(24, 0, true),
                add(1, 0, false),
                add(0, 2, true),
            ],
        ),
        (
            "fresh and carried keys together: only the fresh ones count",
            PUBLISHED_CEILING - 2,
            vec![add(2, 10, true)],
        ),
    ]
}

/// The map one addition carries: `fresh` keys the resource does not hold, named for
/// the call they arrive in, plus the first `carried` keys of `created` with a new
/// value, so an accepted overwrite is observable in the read.
fn addition_map(created: &Value, call_index: usize, addition: &Addition) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..addition.fresh {
        map.insert(
            format!("add{call_index}-{index}"),
            json!(format!("added-{call_index}-{index}")),
        );
    }
    for key in keys_of(created).into_iter().take(addition.carried) {
        map.insert(key, json!(format!("overwritten-{call_index}")));
    }
    Value::Object(map)
}

#[tokio::test]
async fn tagging_a_resource_holds_the_same_ceiling_the_create_does() {
    for (row, created_count, additions) in addition_rows() {
        // Each row gets a store of its own, so what a read reports is this row's work.
        let router = new_router();
        let created = generated_tags("created", created_count);
        let (_, resources) = tagged_resources(&router, REGION, &created).await;

        for resource in &resources {
            let arn = resource.arn(REGION);
            let mut expected = created
                .as_object()
                .cloned()
                .expect("the created tag map is a JSON object");

            for (call_index, addition) in additions.iter().enumerate() {
                let label = format!("{row} on {} (call {call_index})", resource.case);
                let request = addition_map(&created, call_index, addition);
                let (status, body) = tag_in(&router, REGION, &arn, &request).await;

                if addition.accepted {
                    assert_eq!(
                        status, 204,
                        "case {label:?}: this add keeps the resource at or below the \
                         ceiling of {PUBLISHED_CEILING}, so it must be accepted, got \
                         {status} {body:?}"
                    );
                    for (key, value) in request.as_object().expect("a request map") {
                        expected.insert(key.clone(), value.clone());
                    }
                } else {
                    assert_eq!(
                        status, 400,
                        "case {label:?}: this add would take the resource past the ceiling \
                         of {PUBLISHED_CEILING}, so it must be refused, got {status} {body:?}"
                    );
                    let class = error_code(&body);
                    assert_eq!(
                        class, INVALID_INPUT,
                        "case {label:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
                    );
                    let message = body["message"].as_str().unwrap_or_default();
                    assert!(
                        message.contains(&PUBLISHED_CEILING.to_string()),
                        "case {label:?}: the refusal must name the limit \
                         {PUBLISHED_CEILING}, got {message:?}"
                    );
                    for key in keys_of(&request).iter().take(3) {
                        assert!(
                            !message.contains(key.as_str()),
                            "case {label:?}: the refusal must not echo the caller's keys, \
                             got {message:?}"
                        );
                    }
                }

                // Accepted or refused, the resource's own read is what says what it
                // carries: a refused add persists NONE of its keys, not even the ones
                // that would have fitted.
                let actual = tags_of(&router, REGION, resource, &label).await;
                assert_eq!(
                    actual,
                    Value::Object(expected.clone()),
                    "case {label:?}: expected {} tags, got {actual:?}",
                    expected.len()
                );
            }
        }
    }
}
