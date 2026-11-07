// SPDX-License-Identifier: CC0-1.0

//! What a resource READ projects: which members the body carries, and in which form.
//!
//! RULE: an Amazon Location resource read carries exactly the members the service
//! populates for that shape — every `@timestampFormat("date-time")` timestamp as an ISO
//! 8601 string, the resource's tag map always (an empty object when the resource carries
//! no tags), and the deprecated-and-unused `PricingPlanDataSource` only on the describe
//! shapes that document it as always an empty string, never on a listing entry, which
//! the service does not populate it on.
//!
//! Every call goes through the shared harness (`crate::harness`), so each one is a
//! separate HTTP request against one shared engine: what a listing or a describe reports
//! was PERSISTED by the create call, not echoed back inside one request.
//!
//! The tables span what the rule generalises over: both control planes that publish a
//! listing (trackers and geofence collections), the tag-map sizes a resource can carry,
//! a present and an absent `Description`, the punctuation a `ResourceName` admits
//! (`^[-._\w]+$`), and the page the entry is read on (so an entry served from a
//! continuation page is projected the same way as one on the first page).

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one HTTP harness this crate's suites share, declared from `tests/all.rs`.
use crate::harness::{call, emulator, millis_of};

/// One control plane: how a resource is created, listed and described, and the member
/// set its listing entry must carry.
struct Plane {
    case: &'static str,
    create_path: &'static str,
    list_path: &'static str,
    /// The member a create request and a listing entry name the resource with.
    name_member: &'static str,
    /// The members a listing entry must carry, and only these. From the entry shape's
    /// members (`smithy_query(query="shape", shape="ListTrackersResponseEntry")` /
    /// `"ListGeofenceCollectionsResponseEntry"`) minus `PricingPlanDataSource`, the
    /// deprecated member the service does not populate on a listing entry.
    entry_members: &'static [&'static str],
}

fn planes() -> Vec<Plane> {
    vec![
        Plane {
            case: "tracker",
            create_path: "/tracking/v0/trackers",
            list_path: "/tracking/v0/list-trackers",
            name_member: "TrackerName",
            entry_members: &[
                "CreateTime",
                "Description",
                "PricingPlan",
                "TrackerName",
                "UpdateTime",
            ],
        },
        Plane {
            case: "geofence collection",
            create_path: "/geofencing/v0/collections",
            list_path: "/geofencing/v0/list-collections",
            name_member: "CollectionName",
            entry_members: &[
                "CollectionName",
                "CreateTime",
                "Description",
                "PricingPlan",
                "UpdateTime",
            ],
        },
    ]
}

/// The describe path of one resource of this plane.
fn describe_path(plane: &Plane, name: &str) -> String {
    match plane.name_member {
        "TrackerName" => format!("/tracking/v0/trackers/{name}"),
        _ => format!("/geofencing/v0/collections/{name}"),
    }
}

/// One resource to create. Rows differ along tag-map size, `Description` presence and
/// the punctuation the name carries.
struct ResourceCase {
    name: &'static str,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

fn resource_cases() -> &'static [ResourceCase] {
    &[
        ResourceCase {
            name: "a-untagged",
            description: None,
            tags: &[],
        },
        ResourceCase {
            name: "b.one_tag",
            description: Some("one tag"),
            tags: &[("stage", "prod")],
        },
        ResourceCase {
            name: "c-three-tags",
            description: Some(""),
            tags: &[("stage", "test"), ("team", "fleet"), ("zz", "")],
        },
        ResourceCase {
            name: "d_described",
            description: Some("described, untagged"),
            tags: &[],
        },
    ]
}

fn create_body(plane: &Plane, case: &ResourceCase) -> Value {
    let mut body = serde_json::Map::new();
    body.insert(plane.name_member.to_string(), json!(case.name));
    if let Some(description) = case.description {
        body.insert("Description".to_string(), json!(description));
    }
    if !case.tags.is_empty() {
        body.insert(
            "Tags".to_string(),
            Value::Object(
                case.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            ),
        );
    }
    Value::Object(body)
}

/// Create every row of the table on this plane, answering the epoch millis of each
/// resource's `CreateTime` by name.
async fn create_all(router: &MultiServiceRouter, plane: &Plane) -> Vec<(String, i64)> {
    let mut created = Vec::new();
    for case in resource_cases() {
        let answer = call(
            router,
            "POST",
            plane.create_path,
            Some(create_body(plane, case)),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}/{:?}: the create call must succeed, got {} {:?}",
            plane.case,
            case.name,
            answer.status,
            answer.body
        );
        let body = answer.json();
        created.push((case.name.to_string(), millis_of(&body, "CreateTime")));
    }
    created
}

/// Every entry of the listing, walked across pages of `max_results` when it is given.
async fn list_entries(
    router: &MultiServiceRouter,
    plane: &Plane,
    max_results: Option<u32>,
) -> Vec<Value> {
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = serde_json::Map::new();
        if let Some(limit) = max_results {
            body.insert("MaxResults".to_string(), json!(limit));
        }
        if let Some(cursor) = &token {
            body.insert("NextToken".to_string(), json!(cursor));
        }
        let answer = call(router, "POST", plane.list_path, Some(Value::Object(body))).await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}/{max_results:?}: the list call must succeed, got {} {:?}",
            plane.case,
            answer.status,
            answer.body
        );
        let page = answer.json();
        let listed = page.get("Entries").and_then(Value::as_array).unwrap_or_else(|| {
            panic!(
                "case {:?}/{max_results:?}: expected an Entries list, got {page}",
                plane.case
            )
        });
        entries.extend(listed.iter().cloned());
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    entries
}

/// The member names of a JSON object.
fn members(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

/// RULE: a listing entry carries exactly the members the service populates for it — the
/// deprecated, unused `PricingPlanDataSource` is not one of them, so it is absent from
/// every entry on every page.
#[tokio::test]
async fn a_listing_entry_carries_only_the_members_the_service_populates() {
    for plane in planes() {
        let router = emulator();
        let created = create_all(&router, &plane).await;
        let expected: BTreeSet<String> = plane
            .entry_members
            .iter()
            .map(|member| (*member).to_string())
            .collect();

        // Page sizes spanning one entry per page, a non-divisor of the collection size,
        // the exact size, the modelled maximum, and no MaxResults at all: an entry is
        // projected the same way wherever it is served from.
        for max_results in [Some(1), Some(3), Some(4), Some(100), None] {
            let entries = list_entries(&router, &plane, max_results).await;
            assert_eq!(
                entries.len(),
                created.len(),
                "case {:?}/{max_results:?}: expected {} entries, got {entries:?}",
                plane.case,
                created.len()
            );

            for (entry, (name, create_time)) in entries.iter().zip(created.iter()) {
                let case = format!("{}/{max_results:?}/{name}", plane.case);
                assert_eq!(
                    members(entry),
                    expected,
                    "case {case:?}: expected the entry members {expected:?}, got {entry}"
                );
                assert_eq!(
                    entry.get(plane.name_member).and_then(Value::as_str),
                    Some(name.as_str()),
                    "case {case:?}: expected the entry to name {name:?}, got {entry}"
                );
                // `PricingPlan` on both entry shapes is documented "Always returns
                // RequestBasedUsage".
                assert_eq!(
                    entry.get("PricingPlan").and_then(Value::as_str),
                    Some("RequestBasedUsage"),
                    "case {case:?}: expected PricingPlan RequestBasedUsage, got {entry}"
                );
                // `millis_of` fails unless the member is an ISO 8601 date-time STRING,
                // so this pins the wire form as well as the instant.
                assert_eq!(
                    millis_of(entry, "CreateTime"),
                    *create_time,
                    "case {case:?}: expected the stored CreateTime {create_time}, got {entry}"
                );
                assert_eq!(
                    millis_of(entry, "UpdateTime"),
                    *create_time,
                    "case {case:?}: a never-updated resource's UpdateTime must equal its \
                     CreateTime {create_time}, got {entry}"
                );
            }
        }
    }
}

/// RULE: a describe read always projects the resource's tag map — an empty object when
/// it carries no tags — reports its timestamps as ISO 8601 strings, and carries the
/// deprecated `PricingPlanDataSource` as the empty string its member doc states.
#[tokio::test]
async fn a_describe_read_projects_the_tag_map_and_the_documented_fixed_members() {
    for plane in planes() {
        let router = emulator();
        let created = create_all(&router, &plane).await;

        for (case, (name, create_time)) in resource_cases().iter().zip(created.iter()) {
            let label = format!("{}/{name}", plane.case);
            let answer = call(&router, "GET", &describe_path(&plane, name), None).await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: the describe call must succeed, got {} {:?}",
                answer.status, answer.body
            );
            let body = answer.json();

            let reported = body.get("Tags").unwrap_or_else(|| {
                panic!("case {label:?}: Tags must always be projected, got {body}")
            });
            let expected: Value = Value::Object(
                case.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            );
            assert_eq!(
                reported, &expected,
                "case {label:?}: expected Tags {expected}, got {reported}"
            );

            // `DescribeTrackerResponse$PricingPlanDataSource` and the geofence
            // collection's: "No longer used. Always returns an empty string."
            assert_eq!(
                body.get("PricingPlanDataSource").and_then(Value::as_str),
                Some(""),
                "case {label:?}: expected PricingPlanDataSource as the empty string, got {body}"
            );
            assert_eq!(
                body.get("PricingPlan").and_then(Value::as_str),
                Some("RequestBasedUsage"),
                "case {label:?}: expected PricingPlan RequestBasedUsage, got {body}"
            );
            assert_eq!(
                millis_of(&body, "CreateTime"),
                *create_time,
                "case {label:?}: expected the CreateTime the create call reported, got {body}"
            );
            assert_eq!(
                millis_of(&body, "UpdateTime"),
                *create_time,
                "case {label:?}: a never-updated resource's UpdateTime must equal its \
                 CreateTime, got {body}"
            );
        }
    }
}
