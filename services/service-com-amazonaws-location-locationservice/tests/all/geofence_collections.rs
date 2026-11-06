// SPDX-License-Identifier: CC0-1.0

//! The geofence collection control plane, end to end over the wire.
//!
//! Every call goes through the shared harness (`crate::harness`), so each one is a
//! separate HTTP request against one shared engine: a value that survives from the
//! create call to the describe call survived because it was PERSISTED, not because a
//! handler echoed its own input back.
//!
//! The tables span the classes the rule generalises over — the optional members of
//! the create request (`Description`, `KmsKeyId`, `Tags`, the deprecated
//! `PricingPlan`), the punctuation a `ResourceName` admits (`^[-._\w]+$`), the
//! request region the ARN is built from, and the two retired `PricingPlan` values.
//! The request the issue report replays is one row of the round-trip table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one HTTP harness this crate's suites share, declared from `tests/all.rs`.
use crate::harness::{call, call_in_region, emulator, millis_of, Answer, ACCOUNT, REGION};

/// The sentence a retired `PricingPlan` is refused with.
const PRICING_PLAN_REFUSAL: &str = "'PricingPlan' must be RequestBasedUsage";

async fn create_collection(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/geofencing/v0/collections", Some(body)).await
}

async fn describe_collection(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/geofencing/v0/collections/{name}"),
        None,
    )
    .await
}

async fn list_collections(router: &MultiServiceRouter, body: Value) -> Answer {
    call(
        router,
        "POST",
        "/geofencing/v0/list-collections",
        Some(body),
    )
    .await
}

/// One row of the create-then-describe table.
struct CollectionCase {
    name: &'static str,
    request: fn() -> Value,
    /// What `DescribeGeofenceCollection` must report: an absent `Description` reads
    /// back as the empty string, because the read models it as required.
    description: &'static str,
    kms_key_id: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// The create-then-describe table.
///
/// The rows differ along the optional members the create request may carry and the
/// punctuation a `CollectionName` admits, so what the read reports comes from the
/// store in each shape of request rather than from one.
fn collection_cases() -> Vec<CollectionCase> {
    vec![
        CollectionCase {
            // Every optional member absent: what the read reports then comes from
            // the service, not from the request.
            name: "minimal",
            request: || json!({ "CollectionName": "minimal" }),
            description: "",
            kms_key_id: None,
            tags: &[],
        },
        CollectionCase {
            name: "fleet.collection_1",
            request: || {
                json!({
                    "CollectionName": "fleet.collection_1",
                    "Description": "delivery fences",
                    "Tags": { "team": "logistics", "env": "staging", "cost-center": "42" },
                })
            },
            description: "delivery fences",
            kms_key_id: None,
            tags: &[
                ("cost-center", "42"),
                ("env", "staging"),
                ("team", "logistics"),
            ],
        },
        CollectionCase {
            name: "Encrypted-Collection",
            request: || {
                json!({
                    "CollectionName": "Encrypted-Collection",
                    "Description": "",
                    // A key ARN is one of the four forms the member documents
                    // ("Enter a key ID, key ARN, alias name, or alias ARN").
                    "KmsKeyId": "arn:aws:kms:us-west-2:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                    // Deprecated, and the only value it still admits.
                    "PricingPlan": "RequestBasedUsage",
                    "Tags": { "owner": "geofencing" },
                })
            },
            description: "",
            kms_key_id: Some(
                "arn:aws:kms:us-west-2:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
            tags: &[("owner", "geofencing")],
        },
        CollectionCase {
            name: "alias_key-collection.2",
            request: || {
                json!({
                    "CollectionName": "alias_key-collection.2",
                    "Description": "an alias name names the key",
                    "KmsKeyId": "alias/location-geofences",
                })
            },
            description: "an alias name names the key",
            kms_key_id: Some("alias/location-geofences"),
            tags: &[],
        },
    ]
}

/// The `Tags` a response reports, sorted, so a table row compares against a map
/// whatever order the wire carried.
fn reported_tags(value: &Value) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = value
        .get("Tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    tags.sort();
    tags
}

#[tokio::test]
async fn create_then_describe_round_trips_the_stored_collection() {
    let router = emulator();

    for case in collection_cases() {
        let created = create_collection(&router, (case.request)()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateGeofenceCollection must succeed, got {} {:?}",
            case.name, created.status, created.body
        );
        let created = created.json();
        let expected_arn = format!(
            "arn:aws:geo:{REGION}:{ACCOUNT}:geofence-collection/{}",
            case.name
        );
        assert_eq!(
            created.get("CollectionName").and_then(Value::as_str),
            Some(case.name),
            "case {:?}: CreateGeofenceCollection must answer the created name, got {created}",
            case.name
        );
        assert_eq!(
            created.get("CollectionArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {:?}: expected CollectionArn {expected_arn}, got {created}",
            case.name
        );
        let create_time = millis_of(&created, "CreateTime");

        let described = describe_collection(&router, case.name).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: DescribeGeofenceCollection must succeed, got {} {:?}",
            case.name, described.status, described.body
        );
        let described = described.json();

        assert_eq!(
            described.get("CollectionName").and_then(Value::as_str),
            Some(case.name),
            "case {:?}: DescribeGeofenceCollection must report the stored name, got {described}",
            case.name
        );
        assert_eq!(
            described.get("CollectionArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {:?}: DescribeGeofenceCollection must report the stored ARN, got {described}",
            case.name
        );
        assert_eq!(
            described.get("Description").and_then(Value::as_str),
            Some(case.description),
            "case {:?}: expected Description {:?}, got {described}",
            case.name,
            case.description
        );
        assert_eq!(
            described.get("KmsKeyId").and_then(Value::as_str),
            case.kms_key_id,
            "case {:?}: expected KmsKeyId {:?}, got {described}",
            case.name,
            case.kms_key_id
        );
        // `DescribeGeofenceCollectionResponse$PricingPlan` doc: "Always returns
        // RequestBasedUsage"; `PricingPlanDataSource`: "Always returns an empty
        // string".
        assert_eq!(
            described.get("PricingPlan").and_then(Value::as_str),
            Some("RequestBasedUsage"),
            "case {:?}: PricingPlan must always be RequestBasedUsage, got {described}",
            case.name
        );
        assert_eq!(
            described
                .get("PricingPlanDataSource")
                .and_then(Value::as_str),
            Some(""),
            "case {:?}: PricingPlanDataSource must always be the empty string, got {described}",
            case.name
        );
        // "The number of geofences in the geofence collection": nothing has put a
        // geofence into this one.
        assert_eq!(
            described.get("GeofenceCount").and_then(Value::as_i64),
            Some(0),
            "case {:?}: a collection nothing wrote a geofence to must report 0 geofences, got {described}",
            case.name
        );

        let expected_tags: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            reported_tags(&described),
            expected_tags,
            "case {:?}: expected Tags {expected_tags:?}, got {:?}",
            case.name,
            reported_tags(&described)
        );

        assert_eq!(
            millis_of(&described, "CreateTime"),
            create_time,
            "case {:?}: DescribeGeofenceCollection must report the CreateTime the create stored, got {described}",
            case.name
        );
        assert_eq!(
            millis_of(&described, "UpdateTime"),
            create_time,
            "case {:?}: a never-updated collection's UpdateTime must equal its CreateTime, got {described}",
            case.name
        );
    }
}

#[tokio::test]
async fn each_collection_is_stored_under_its_own_name() {
    // One emulator, every row of the table created on it: a create must not
    // overwrite or hide a collection created before it.
    let router = emulator();
    for case in collection_cases() {
        let created = create_collection(&router, (case.request)()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateGeofenceCollection must succeed, got {} {:?}",
            case.name, created.status, created.body
        );
    }

    for case in collection_cases() {
        let described = describe_collection(&router, case.name).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: every created collection must still be readable, got {} {:?}",
            case.name, described.status, described.body
        );
        assert_eq!(
            described.json().get("Description").and_then(Value::as_str),
            Some(case.description),
            "case {:?}: expected the Description stored for this name, got {:?}",
            case.name,
            described.body
        );
    }
}

#[tokio::test]
async fn collection_arn_carries_the_region_the_request_is_addressed_to() {
    // One ARN per request region, so the region segment is read from the request
    // rather than baked in.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let router = emulator();
        let body = bytes::Bytes::from(
            serde_json::to_vec(&json!({ "CollectionName": "regional" }))
                .expect("body serializes"),
        );
        let created =
            call_in_region(&router, "POST", "/geofencing/v0/collections", body, region).await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: CreateGeofenceCollection must succeed, got {} {:?}",
            created.status, created.body
        );
        let expected = format!("arn:aws:geo:{region}:{ACCOUNT}:geofence-collection/regional");
        assert_eq!(
            created.json().get("CollectionArn").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {region:?}: expected CollectionArn {expected}, got {:?}",
            created.body
        );
        // The ARN the create answered is the one the store kept.
        assert_eq!(
            describe_collection(&router, "regional")
                .await
                .json()
                .get("CollectionArn")
                .and_then(Value::as_str),
            Some(expected.as_str()),
            "case {region:?}: the stored ARN must be the one the create answered"
        );
    }
}

#[tokio::test]
async fn recreating_a_collection_is_a_conflict_and_leaves_the_stored_one_alone() {
    let router = emulator();
    let first = create_collection(
        &router,
        json!({
            "CollectionName": "twice",
            "Description": "the stored one",
            "Tags": { "generation": "first" },
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {} {:?}",
        first.status, first.body
    );
    let stored_create_time = millis_of(&first.json(), "CreateTime");

    // A second create for the same name, with different members, must not
    // overwrite anything.
    for description in ["the overwrite that must not happen", ""] {
        let again = create_collection(
            &router,
            json!({
                "CollectionName": "twice",
                "Description": description,
                "Tags": { "generation": "second" },
            }),
        )
        .await;
        assert_eq!(
            again.status, 409,
            "case {description:?}: re-creating a name must conflict, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {description:?}: expected ConflictException, got {:?} ({:?})",
            again.error_code(),
            again.body
        );
        assert_eq!(
            again.message(),
            "Geofence collection already exists: twice.",
            "case {description:?}: expected the conflict to name the resource and the requested name, got {:?}",
            again.message()
        );
    }

    let described = describe_collection(&router, "twice").await;
    let described = described.json();
    assert_eq!(
        described.get("Description").and_then(Value::as_str),
        Some("the stored one"),
        "the conflicting creates must not change the stored Description, got {described}"
    );
    assert_eq!(
        reported_tags(&described),
        vec![("generation".to_string(), "first".to_string())],
        "the conflicting creates must not change the stored Tags, got {:?}",
        reported_tags(&described)
    );
    assert_eq!(
        millis_of(&described, "CreateTime"),
        stored_create_time,
        "the conflicting creates must not change the stored CreateTime, got {described}"
    );
}

#[tokio::test]
async fn describing_a_collection_that_was_never_created_is_not_found() {
    let router = emulator();
    // One collection exists; the names below satisfy `^[-._\w]+$` but were never
    // created, including case variants of the one that was: the lookup is by exact
    // name.
    let created = create_collection(&router, json!({ "CollectionName": "Exists" })).await;
    assert_eq!(
        created.status, 200,
        "the positive control must be created, got {} {:?}",
        created.status, created.body
    );

    for name in ["missing", "exists", "EXISTS", "Exists.2", "x"] {
        let described = describe_collection(&router, name).await;
        assert_eq!(
            described.status, 404,
            "case {name:?}: an unknown collection must be 404, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.error_code(),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {:?} ({:?})",
            described.error_code(),
            described.body
        );
        let expected = format!("Geofence collection not found: {name}.");
        assert_eq!(
            described.message(),
            expected,
            "case {name:?}: expected message {expected:?}, got {:?}",
            described.message()
        );
    }
}

#[tokio::test]
async fn a_retired_pricing_plan_is_refused_and_the_one_it_still_admits_is_not() {
    // `PricingPlan` doc: "No longer used. If included, the only allowed value is
    // RequestBasedUsage." The two rows are the enum's other members
    // (`smithy_query shape=PricingPlan`).
    for plan in ["MobileAssetManagement", "MobileAssetTracking"] {
        let router = emulator();
        let refused = create_collection(
            &router,
            json!({ "CollectionName": "retired", "PricingPlan": plan }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {plan:?}: expected a 400 refusal, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {plan:?}: expected error code ValidationException, got {:?} ({:?})",
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.message(),
            PRICING_PLAN_REFUSAL,
            "case {plan:?}: expected message {PRICING_PLAN_REFUSAL:?}, got {:?}",
            refused.message()
        );
        let body = refused.json();
        assert_eq!(
            body.get("reason").and_then(Value::as_str),
            Some("FieldValidationFailed"),
            "case {plan:?}: expected reason FieldValidationFailed, got {body}"
        );
        assert_eq!(
            body.get("fieldList")
                .and_then(Value::as_array)
                .map(|entries| entries
                    .iter()
                    .map(|entry| (
                        entry.get("name").and_then(Value::as_str).unwrap_or_default(),
                        entry
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    ))
                    .collect::<Vec<_>>()),
            Some(vec![("PricingPlan", PRICING_PLAN_REFUSAL)]),
            "case {plan:?}: fieldList must name the offending member and repeat the sentence, got {body}"
        );

        // A refused create stores nothing: the read answers 404, not a resource.
        let described = describe_collection(&router, "retired").await;
        assert_eq!(
            described.status, 404,
            "case {plan:?}: a refused create must persist nothing, got {} {:?}",
            described.status, described.body
        );
    }

    // Present and allowed, and absent, are both accepted.
    for plan in [Some("RequestBasedUsage"), None] {
        let router = emulator();
        let mut body = json!({ "CollectionName": "allowed" });
        if let Some(plan) = plan {
            body["PricingPlan"] = json!(plan);
        }
        let created = create_collection(&router, body).await;
        assert_eq!(
            created.status, 200,
            "case {plan:?}: the allowed PricingPlan must be accepted, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            describe_collection(&router, "allowed").await.status,
            200,
            "case {plan:?}: the accepted create must have persisted the collection"
        );
    }
}

#[tokio::test]
async fn a_read_does_not_change_what_a_later_reader_sees() {
    let router = emulator();
    create_collection(
        &router,
        json!({ "CollectionName": "stable", "Description": "unchanging", "Tags": { "k": "v" } }),
    )
    .await;

    let first = describe_collection(&router, "stable").await.json();
    for round in 1..=3 {
        let again = describe_collection(&router, "stable").await;
        assert_eq!(
            again.status, 200,
            "round {round}: the read must keep succeeding, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.json(),
            first,
            "round {round}: DescribeGeofenceCollection must report the same resource every time, got {:?}",
            again.body
        );
    }
}

// ---------------------------------------------------------------------------
// ListGeofenceCollections
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_geofence_collections_pages_over_every_stored_collection() {
    let router = emulator();
    let names: Vec<String> = (1..=7).map(|i| format!("collection-{i:02}")).collect();
    for name in &names {
        assert_eq!(
            create_collection(&router, json!({ "CollectionName": name }))
                .await
                .status,
            200,
            "creating {name} must succeed"
        );
    }

    // Page sizes spanning 1, divisors and non-divisors of the stored count, the
    // exact count, the modelled maximum (`MaxResults` is `range 1..100`), and no
    // MaxResults at all.
    for max_results in [Some(1), Some(2), Some(3), Some(7), Some(100), None] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = serde_json::Map::new();
            if let Some(limit) = max_results {
                body.insert("MaxResults".to_string(), json!(limit));
            }
            if let Some(cursor) = &token {
                body.insert("NextToken".to_string(), json!(cursor));
            }
            let answer = list_collections(&router, Value::Object(body)).await;
            assert_eq!(
                answer.status, 200,
                "case {max_results:?}: ListGeofenceCollections must succeed, got {} {:?}",
                answer.status, answer.body
            );
            let page = answer.json();
            let entries = page
                .get("Entries")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {max_results:?}: Entries must be a list in {page}"))
                .clone();
            if let Some(limit) = max_results {
                assert!(
                    entries.len() as i32 <= limit,
                    "case {max_results:?}: a page must not exceed MaxResults, got {} entries",
                    entries.len()
                );
            }
            for entry in &entries {
                let name = entry
                    .get("CollectionName")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {max_results:?}: entry has no CollectionName: {entry}")
                    })
                    .to_string();
                // Every entry is computed from the stored collection.
                assert_eq!(
                    entry.get("Description").and_then(Value::as_str),
                    Some(""),
                    "case {max_results:?}: entry {name} must carry the stored Description, got {entry}"
                );
                assert_eq!(
                    entry.get("PricingPlan").and_then(Value::as_str),
                    Some("RequestBasedUsage"),
                    "case {max_results:?}: entry {name} must always report RequestBasedUsage, got {entry}"
                );
                assert_eq!(
                    millis_of(entry, "CreateTime"),
                    millis_of(entry, "UpdateTime"),
                    "case {max_results:?}: entry {name} was never updated, so its timestamps must match"
                );
                seen.push(name);
            }
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {max_results:?}: pagination did not terminate, saw {seen:?}"
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, names,
            "case {max_results:?}: paging must yield every stored collection exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_collection_cursor_this_service_did_not_mint_is_refused() {
    let router = emulator();
    assert_eq!(
        create_collection(&router, json!({ "CollectionName": "collection-01" }))
            .await
            .status,
        200
    );

    // Each row is a token shape a client could send that this service never mints:
    // no tag, this collection's tag with no page key, another list's tag, and a bare
    // resource name.
    for token in [
        "not-a-token",
        "geofence-collections",
        "geofence-collections:",
        "trackers:collection-01",
        "collection-01",
    ] {
        let answer = list_collections(&router, json!({ "NextToken": token })).await;
        assert_eq!(
            answer.status, 400,
            "case {token:?}: a malformed NextToken must be refused with 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}
