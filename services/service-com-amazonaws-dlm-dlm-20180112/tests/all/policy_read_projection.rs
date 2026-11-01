// SPDX-License-Identifier: CC0-1.0

//! What a read of a lifecycle policy reports, end to end over HTTP.
//!
//! The rule under test: a read reports the policy as the service STORED it — every
//! member the request stated, unchanged, plus the members Amazon Data Lifecycle
//! Manager itself materializes when it accepts the policy (the custom policy
//! language, the resource location, and per SCHEDULE the copy-tags flag, the
//! creation location and the two rule intervals), with the policy's tag set always
//! reported (the empty map when it holds none) and the two timestamp members in the
//! `date-time` form their target shape declares.
//!
//! Every case drives the router, so each one exercises routing, deserialization, the
//! handler, the persisted record and serialization — the path a client takes. The
//! rows differ along what the rule generalises over: which optional members a
//! request states at all, how many schedules it carries, whether a creation rule is
//! interval- or cron-driven, whether retention is count- or age-based, the policy
//! type, and whether the policy is tagged.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The comparable timestamp key this crate's suites already share, so two renderings of
// one instant order by the instant rather than by their spelling.
use crate::lifecycle_policy_crud::timestamp_key;

/// The region these requests are signed for.
const REGION: &str = "eu-west-1";

/// A second region, for the cross-region copy rows.
const OTHER_REGION: &str = "us-west-2";

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AWSDataLifecycleManagerDefaultRole";

/// `PolicyDetails$PolicyLanguage`: "`STANDARD` To create a custom policy."
const CUSTOM_POLICY_LANGUAGE: &str = "STANDARD";

/// The in-Region location: `PolicyDetails$ResourceLocations` ("If the source
/// resources are located in a Region, specify `CLOUD`") and `CreateRule$Location`
/// ("Default: `CLOUD`").
const CLOUD: &str = "CLOUD";

/// `Schedule$CopyTags` targets the model's non-nullable `CopyTags` boolean, whose
/// documented default is false.
const SCHEDULE_COPY_TAGS_DEFAULT: bool = false;

/// The flag a cross-Region copy rule that states no `CopyTags` reports: the copy
/// carries the source's user-defined tags unless the rule turns that off.
const CROSS_REGION_COPY_TAGS_DEFAULT: bool = true;

/// `PolicyTypeValues$IMAGE_MANAGEMENT`, the AMI policy type. `CreateRule$Location`
/// is `[Custom snapshot policies only]`, so an AMI policy's creation rule reports
/// no location at all.
const IMAGE_POLICY_TYPE: &str = "IMAGE_MANAGEMENT";

/// The value a schedule rule's `Interval` carries when the request states none.
const UNSET_RULE_INTERVAL: i64 = 0;

/// Every `LocationValues` member, from
/// `smithy_query(query="shape", shape="LocationValues")`. A row below states each
/// one explicitly, so the stamped default cannot be mistaken for a clamp.
const LOCATION_VALUES: &[&str] = &["CLOUD", "OUTPOST_LOCAL", "LOCAL_ZONE"];

/// Every `ResourceLocationValues` member, from
/// `smithy_query(query="shape", shape="ResourceLocationValues")`.
const RESOURCE_LOCATION_VALUES: &[&str] = &["CLOUD", "OUTPOST", "LOCAL_ZONE"];

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn policy(&self) -> &Value {
        &self.body["Policy"]
    }

    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// A SigV4 credential scope naming this service in `region` — what routes the
/// request to dlm.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/policies", Some(request)).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/policies/{id}"), None).await
}

async fn list(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/policies", None).await
}

async fn update(router: &MultiServiceRouter, id: &str, request: Value) -> Answer {
    call(router, "PATCH", &format!("/policies/{id}"), Some(request)).await
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// Every member of `sent` is reported by `read`, with the same value: no submitted
/// member is dropped, reordered away or rewritten. Objects are compared member by
/// member (so a materialized member `sent` does not name is allowed), lists
/// element by element, and scalars by equality.
pub(crate) fn reports_everything_sent(sent: &Value, read: &Value, path: &str) -> Result<(), String> {
    match sent {
        Value::Object(members) => {
            let Some(reported) = read.as_object() else {
                return Err(format!("{path} must be an object, got {read:?}"));
            };
            for (name, value) in members {
                let Some(found) = reported.get(name) else {
                    return Err(format!("{path}/{name} was sent as {value:?} but is absent"));
                };
                reports_everything_sent(value, found, &format!("{path}/{name}"))?;
            }
            Ok(())
        }
        Value::Array(items) => {
            let Some(reported) = read.as_array() else {
                return Err(format!("{path} must be a list, got {read:?}"));
            };
            if reported.len() != items.len() {
                return Err(format!(
                    "{path} was sent with {} elements but {} are reported",
                    items.len(),
                    reported.len()
                ));
            }
            for (index, item) in items.iter().enumerate() {
                reports_everything_sent(item, &reported[index], &format!("{path}/{index}"))?;
            }
            Ok(())
        }
        scalar => {
            if scalar == read {
                Ok(())
            } else {
                Err(format!("{path} was sent as {scalar:?} but reports {read:?}"))
            }
        }
    }
}

/// The value `read` reports at `name`, or `Value::Null`.
fn member(read: &Value, name: &str) -> Value {
    read.get(name).cloned().unwrap_or(Value::Null)
}

/// The submitted members of one object, or an empty map when it is absent.
fn members(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// The `date-time` form: an RFC 3339 / ISO-8601 instant, which is what
/// `com.amazonaws.dlm#Timestamp`'s `@timestampFormat("date-time")` puts on the
/// wire. A JSON number (the JSON protocol's epoch-seconds default) is not one.
fn assert_is_date_time(case: &str, pointer: &str, value: &Value) {
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {pointer} must be a date-time STRING, got {value:?}"
        )
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {pointer} {text:?} is not a date-time ({error})")
    });
    assert!(
        text.ends_with('Z') && text.contains('T') && parsed.secs() > 0,
        "case {case:?}: {pointer} must be a UTC ISO-8601 instant, got {text:?}"
    );
}

/// A custom snapshot policy's `PolicyDetails`: an interval-driven creation rule
/// and count-based retention, with every defaultable member left out.
fn snapshot_details_with_defaults_omitted() -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    })
}

/// An AMI policy whose creation rule is a cron expression, so its `Interval` is
/// one of the members the service materializes.
fn image_details_with_cron() -> Value {
    json!({
        "PolicyType": "IMAGE_MANAGEMENT",
        "ResourceTypes": ["INSTANCE"],
        "TargetTags": [{"Key": "backup", "Value": "weekly"}],
        "Schedules": [{
            "Name": "weekly",
            "CreateRule": {"CronExpression": "cron(0 1 ? * MON *)"},
            "RetainRule": {"Count": 2},
        }],
    })
}

/// A custom AMI policy's `PolicyDetails` with an interval-driven creation rule
/// and every defaultable member left out.
fn image_details_with_defaults_omitted() -> Value {
    json!({
        "PolicyType": "IMAGE_MANAGEMENT",
        "ResourceTypes": ["INSTANCE"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    })
}

/// A custom snapshot policy whose creation rule is a cron expression, so its
/// `Interval` is one of the members the service materializes.
fn snapshot_details_with_cron() -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "weekly"}],
        "Schedules": [{
            "Name": "weekly",
            "CreateRule": {"CronExpression": "cron(0 1 ? * MON *)"},
            "RetainRule": {"Count": 2},
        }],
    })
}

/// One read-projection case: a `CreateLifecyclePolicy` body, and whether its
/// configuration is one the custom snapshot/AMI defaults apply to.
struct Case {
    case: &'static str,
    request: Value,
    /// False for a configuration that carries neither schedules nor resource
    /// locations — an event-based policy — where nothing may be invented.
    custom_snapshot_or_ami: bool,
}

/// The table: every row is a policy whose read must satisfy the rule.
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            case: "a custom snapshot policy with every defaultable member omitted",
            request: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "nightly ebs",
                "State": "ENABLED",
                "PolicyDetails": snapshot_details_with_defaults_omitted(),
            }),
            custom_snapshot_or_ami: true,
        },
        Case {
            case: "an AMI policy whose creation rule is a cron expression",
            request: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "weekly amis",
                "State": "DISABLED",
                "Tags": {"team": "images"},
                "PolicyDetails": image_details_with_cron(),
            }),
            custom_snapshot_or_ami: true,
        },
        Case {
            case: "four schedules, each omitting a different defaultable member",
            request: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "many schedules",
                "State": "ENABLED",
                "PolicyDetails": {
                    "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
                    "ResourceTypes": ["VOLUME"],
                    "TargetTags": [{"Key": "backup", "Value": "daily"}],
                    // `ScheduleList` is `@length(min: 1, max: 4)`, so this row
                    // carries the largest list the model admits.
                    "Schedules": [
                        {
                            "Name": "first",
                            "CreateRule": {"Interval": 12, "IntervalUnit": "HOURS"},
                            "RetainRule": {"Count": 3},
                        },
                        {
                            "Name": "second",
                            "CopyTags": true,
                            "CreateRule": {"CronExpression": "cron(0 2 ? * SUN *)"},
                            "RetainRule": {"Interval": 2, "IntervalUnit": "WEEKS"},
                        },
                        {
                            "Name": "third",
                            "CreateRule": {
                                "Interval": 6,
                                "IntervalUnit": "HOURS",
                                "Location": "LOCAL_ZONE",
                            },
                            "RetainRule": {"Count": 1},
                        },
                        {
                            "Name": "fourth",
                            "CreateRule": {"Interval": 1, "IntervalUnit": "HOURS"},
                        },
                    ],
                },
            }),
            custom_snapshot_or_ami: true,
        },
        Case {
            case: "a schedule with cross-region copy rules the client configured",
            request: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "cross region copies",
                "State": "ENABLED",
                "PolicyDetails": {
                    "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
                    "ResourceTypes": ["VOLUME"],
                    "TargetTags": [{"Key": "backup", "Value": "daily"}],
                    "Schedules": [{
                        "Name": "copied",
                        "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
                        "RetainRule": {"Count": 4},
                        // Three rules (`CrossRegionCopyRules` is
                        // `@length(min: 0, max: 3)`): one asking for the tags,
                        // one refusing them, and one that states neither.
                        "CrossRegionCopyRules": [
                            {
                                "TargetRegion": OTHER_REGION,
                                "Encrypted": false,
                                "CopyTags": true,
                                "RetainRule": {"Interval": 3, "IntervalUnit": "DAYS"},
                            },
                            {
                                "Target": "eu-central-1",
                                "Encrypted": true,
                                "CopyTags": false,
                            },
                            {
                                "TargetRegion": "ap-south-1",
                                "Encrypted": false,
                            },
                        ],
                    }],
                },
            }),
            custom_snapshot_or_ami: true,
        },
        Case {
            case: "an event-based policy, which has neither schedules nor locations",
            request: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "event based copies",
                "State": "ENABLED",
                "PolicyDetails": {
                    "PolicyType": "EVENT_BASED_POLICY",
                    "EventSource": {
                        "Type": "MANAGED_CWE",
                        "Parameters": {
                            "EventType": "shareSnapshot",
                            "SnapshotOwner": ["000000000000"],
                            "DescriptionRegex": "^.*$",
                        },
                    },
                    "Actions": [{
                        "Name": "copy",
                        "CrossRegionCopy": [{
                            "Target": OTHER_REGION,
                            "EncryptionConfiguration": {"Encrypted": false},
                            "RetainRule": {"Interval": 1, "IntervalUnit": "DAYS"},
                        }],
                    }],
                },
            }),
            custom_snapshot_or_ami: false,
        },
    ];

    // One row per `LocationValues` / `ResourceLocationValues` member stated
    // explicitly: a value the client chose is never replaced by the default, and
    // the catalog is exercised past the one entry the issue cited.
    for (index, location) in LOCATION_VALUES.iter().enumerate() {
        let resource_location = RESOURCE_LOCATION_VALUES[index];
        cases.push(Case {
            case: "a schedule that names its own locations",
            request: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": format!("explicit {location}"),
                "State": "ENABLED",
                "Tags": {"where": *location},
                "PolicyDetails": {
                    "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
                    "PolicyLanguage": CUSTOM_POLICY_LANGUAGE,
                    "ResourceTypes": ["VOLUME"],
                    "ResourceLocations": [resource_location],
                    "TargetTags": [{"Key": "backup", "Value": "daily"}],
                    "Schedules": [{
                        "Name": "explicit",
                        "CopyTags": true,
                        "CreateRule": {
                            "Interval": 8,
                            "IntervalUnit": "HOURS",
                            "Location": *location,
                        },
                        "RetainRule": {"Interval": 9, "IntervalUnit": "DAYS"},
                    }],
                },
            }),
            custom_snapshot_or_ami: true,
        });
    }
    cases
}

/// Assert the whole rule against one read `policy`, created from `request`.
fn assert_read_satisfies_the_rule(case: &str, request: &Value, policy: &Value) {
    // Nothing the request stated is lost. This is the half of the rule that is
    // about SUBMITTED state — a cross-region copy rule's flags included.
    for (name, sent) in members(request) {
        let pointer = match name.as_str() {
            "Description" | "ExecutionRoleArn" | "State" | "Tags" | "PolicyDetails" => {
                format!("/{name}")
            }
            other => panic!("case {case:?}: this test states a member with no mapping: {other:?}"),
        };
        if let Err(problem) = reports_everything_sent(&sent, &at(policy, &pointer), &pointer) {
            panic!("case {case:?}: {problem}");
        }
    }

    // The tag set is always reported: the empty map for a policy with none.
    let expected_tags = request.get("Tags").cloned().unwrap_or_else(|| json!({}));
    assert_eq!(
        member(policy, "Tags"),
        expected_tags,
        "case {case:?}: Tags must report the policy's tag set, got {:?}",
        member(policy, "Tags")
    );

    // `StatusMessage` describes the policy's current status, which for a policy
    // nothing has failed is its activation state.
    assert_eq!(
        member(policy, "StatusMessage"),
        member(policy, "State"),
        "case {case:?}: StatusMessage must describe the policy's status, got {:?} for state {:?}",
        member(policy, "StatusMessage"),
        member(policy, "State")
    );

    // Both timestamps are `date-time` strings, never epoch numbers.
    for pointer in ["/DateCreated", "/DateModified"] {
        assert_is_date_time(case, pointer, &at(policy, pointer));
    }

    let sent_details = at(request, "/PolicyDetails");
    let read_details = at(policy, "/PolicyDetails");

    // Every custom policy reports the custom policy language.
    assert_eq!(
        member(&read_details, "PolicyLanguage"),
        json!(CUSTOM_POLICY_LANGUAGE),
        "case {case:?}: a custom policy's PolicyLanguage is {CUSTOM_POLICY_LANGUAGE:?}, got {:?}",
        member(&read_details, "PolicyLanguage")
    );

    // A schedule is never invented: a policy reports exactly as many as it stored.
    assert_eq!(
        member(&read_details, "Schedules").as_array().map(Vec::len),
        member(&sent_details, "Schedules").as_array().map(Vec::len),
        "case {case:?}: Schedules must report the schedules the request stated"
    );

    // `ResourceLocations` is `[Custom snapshot and AMI policies only]`: a policy
    // that configures schedules reports one location, stated or materialized;
    // one that configures none (an event-based policy) reports only what it was
    // sent.
    let sent_locations = member(&sent_details, "ResourceLocations");
    let configures_schedules = member(&sent_details, "Schedules") != Value::Null;
    let expected_locations = match (&sent_locations, configures_schedules) {
        (Value::Null, true) => json!([CLOUD]),
        _ => sent_locations.clone(),
    };
    assert_eq!(
        member(&read_details, "ResourceLocations"),
        expected_locations,
        "case {case:?}: ResourceLocations must name the stored location, got {:?}",
        member(&read_details, "ResourceLocations")
    );

    // And per schedule, for EVERY element of the list.
    let sent_schedules = member(&sent_details, "Schedules");
    let Some(sent_schedules) = sent_schedules.as_array() else {
        return;
    };
    let read_schedules = member(&read_details, "Schedules");
    let read_schedules = read_schedules
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Schedules must be reported as a list"));
    for (index, sent_schedule) in sent_schedules.iter().enumerate() {
        let read_schedule = &read_schedules[index];
        let expected_copy_tags = member(sent_schedule, "CopyTags");
        let expected_copy_tags = if expected_copy_tags == Value::Null {
            json!(SCHEDULE_COPY_TAGS_DEFAULT)
        } else {
            expected_copy_tags
        };
        assert_eq!(
            member(read_schedule, "CopyTags"),
            expected_copy_tags,
            "case {case:?}: Schedules[{index}].CopyTags must be reported, got {:?}",
            member(read_schedule, "CopyTags")
        );

        if sent_schedule.get("CreateRule").is_some() {
            let sent_rule = member(sent_schedule, "CreateRule");
            let read_rule = member(read_schedule, "CreateRule");
            // `CreateRule$Location` is `[Custom snapshot policies only]`, so an
            // AMI policy's creation rule reports only what the request stated.
            let creates_snapshots = member(&sent_details, "PolicyType") != json!(IMAGE_POLICY_TYPE);
            let expected_location = match member(&sent_rule, "Location") {
                Value::Null if creates_snapshots => json!(CLOUD),
                stated => stated,
            };
            assert_eq!(
                member(&read_rule, "Location"),
                expected_location,
                "case {case:?}: Schedules[{index}].CreateRule.Location must be reported, got {:?}",
                member(&read_rule, "Location")
            );
            let expected_interval = match member(&sent_rule, "Interval") {
                Value::Null => json!(UNSET_RULE_INTERVAL),
                stated => stated,
            };
            assert_eq!(
                member(&read_rule, "Interval"),
                expected_interval,
                "case {case:?}: Schedules[{index}].CreateRule.Interval must be reported, got {:?}",
                member(&read_rule, "Interval")
            );
        }

        if sent_schedule.get("RetainRule").is_some() {
            let sent_rule = member(sent_schedule, "RetainRule");
            let read_rule = member(read_schedule, "RetainRule");
            let expected_interval = match member(&sent_rule, "Interval") {
                Value::Null => json!(UNSET_RULE_INTERVAL),
                stated => stated,
            };
            assert_eq!(
                member(&read_rule, "Interval"),
                expected_interval,
                "case {case:?}: Schedules[{index}].RetainRule.Interval must be reported, got {:?}",
                member(&read_rule, "Interval")
            );
            // Nothing is invented for the count: a request that stated none
            // reports none.
            assert_eq!(
                member(&read_rule, "Count"),
                member(&sent_rule, "Count"),
                "case {case:?}: Schedules[{index}].RetainRule.Count must be what was stored"
            );
        }

        // And per cross-Region copy rule, for every element of that list too.
        let sent_copies = member(sent_schedule, "CrossRegionCopyRules");
        if let Some(sent_copies) = sent_copies.as_array() {
            let read_copies = member(read_schedule, "CrossRegionCopyRules");
            let read_copies = read_copies.as_array().unwrap_or_else(|| {
                panic!("case {case:?}: Schedules[{index}].CrossRegionCopyRules must be a list")
            });
            for (copy_index, sent_copy) in sent_copies.iter().enumerate() {
                let expected_copy_tags = match member(sent_copy, "CopyTags") {
                    Value::Null => json!(CROSS_REGION_COPY_TAGS_DEFAULT),
                    stated => stated,
                };
                assert_eq!(
                    member(&read_copies[copy_index], "CopyTags"),
                    expected_copy_tags,
                    "case {case:?}: Schedules[{index}].CrossRegionCopyRules[{copy_index}].CopyTags \
                     must be reported, got {:?}",
                    member(&read_copies[copy_index], "CopyTags")
                );
            }
        }
    }
}

/// `GetLifecyclePolicy` reports the stored policy: the submitted members plus the
/// materialized ones, over the whole table.
#[tokio::test]
async fn a_read_reports_the_stored_policy_with_its_materialized_members() {
    let router = emulator();
    for Case {
        case,
        request,
        custom_snapshot_or_ami,
    } in cases()
    {
        let created = create(&router, request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateLifecyclePolicy, got {} with {:?}",
            created.status, created.body
        );
        let id = created.policy_id();
        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetLifecyclePolicy, got {} with {:?}",
            read.status, read.body
        );
        assert_read_satisfies_the_rule(case, &request, read.policy());

        // An event-based policy gets nothing invented for it.
        if !custom_snapshot_or_ami {
            assert_eq!(
                at(read.policy(), "/PolicyDetails/ResourceLocations"),
                Value::Null,
                "case {case:?}: an event-based policy has no resource location, got {:?}",
                at(read.policy(), "/PolicyDetails/ResourceLocations")
            );
            assert_eq!(
                at(read.policy(), "/PolicyDetails/Schedules"),
                Value::Null,
                "case {case:?}: an event-based policy has no schedules, got {:?}",
                at(read.policy(), "/PolicyDetails/Schedules")
            );
        }

        // Reading twice is reading one record: the projection is stored, not
        // recomputed per call, so nothing drifts between reads.
        let again = get(&router, &id).await;
        assert_eq!(
            again.policy(),
            read.policy(),
            "case {case:?}: two reads of one policy must answer the same projection"
        );
    }
}

/// An update that replaces the configuration stores the materialized members for
/// the REPLACEMENT, and an update that moves the activation state moves the status
/// description with it.
///
/// The replacement states the policy's OWN type and target, since "You cannot
/// update the policy type or the resource type"
/// (`smithy_query(query="doc", shape="UpdateLifecyclePolicyRequest",
/// member="PolicyDetails")`); what it changes is which members the service has to
/// materialize, with one row per kind of custom policy those defaults apply to.
#[tokio::test]
async fn an_update_restamps_the_materialized_members() {
    let router = emulator();
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a snapshot policy replaced by a cron-driven snapshot configuration",
            snapshot_details_with_defaults_omitted(),
            snapshot_details_with_cron(),
        ),
        (
            "an AMI policy replaced by a cron-driven AMI configuration",
            image_details_with_defaults_omitted(),
            image_details_with_cron(),
        ),
    ];

    for (case, created, replacement) in rows {
        let request = json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "nightly ebs",
            "State": "ENABLED",
            "PolicyDetails": created,
        });
        let id = create(&router, request.clone()).await.policy_id();

        let patched = update(
            &router,
            &id,
            json!({"State": "DISABLED", "PolicyDetails": replacement.clone()}),
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateLifecyclePolicy, got {} with {:?}",
            patched.status, patched.body
        );

        let read = get(&router, &id).await;
        let after = json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "nightly ebs",
            "State": "DISABLED",
            "PolicyDetails": replacement,
        });
        assert_read_satisfies_the_rule(case, &after, read.policy());
        assert_eq!(
            at(read.policy(), "/StatusMessage"),
            json!("DISABLED"),
            "case {case:?}: an update that disables a policy moves its status description, \
             got {:?}",
            at(read.policy(), "/StatusMessage")
        );
        // "The local date and time when the lifecycle policy was last modified": the
        // update moved it, and both members are still `date-time` strings. Ordered
        // through the suite's shared comparable key, because the `date-time` rendering
        // trims trailing fractional zeros — so `"…:00.5Z"` sorts AFTER `"…:00.52Z"` as
        // raw text while naming the earlier instant.
        assert!(
            timestamp_key(&at(read.policy(), "/DateModified"))
                >= timestamp_key(&at(read.policy(), "/DateCreated")),
            "case {case:?}: DateModified {:?} must not precede DateCreated {:?}",
            at(read.policy(), "/DateModified"),
            at(read.policy(), "/DateCreated")
        );
    }
}

/// `GetLifecyclePolicies` summaries report each policy's tag set, by the same rule
/// the full read follows: the empty map for a policy with no tags.
#[tokio::test]
async fn every_summary_reports_its_policys_tags() {
    let router = emulator();
    let rows: Vec<(&str, Value)> = vec![
        ("no tags at all", json!({})),
        ("one tag", json!({"team": "backups"})),
        (
            "several tags",
            json!({"team": "backups", "env": "prod", "cost-centre": "42"}),
        ),
    ];

    let mut expected: Vec<(&str, String, Value)> = Vec::new();
    for (case, tags) in rows {
        let mut request = json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": format!("policy with {case}"),
            "State": "ENABLED",
            "PolicyDetails": snapshot_details_with_defaults_omitted(),
        });
        if !members(&tags).is_empty() {
            request["Tags"] = tags.clone();
        }
        let id = create(&router, request).await.policy_id();
        expected.push((case, id, tags));
    }

    let listed = list(&router).await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 from GetLifecyclePolicies, got {} with {:?}",
        listed.status, listed.body
    );
    let summaries = listed.body["Policies"]
        .as_array()
        .unwrap_or_else(|| panic!("GetLifecyclePolicies must return a Policies list"))
        .clone();

    for (case, id, tags) in expected {
        let summary = summaries
            .iter()
            .find(|summary| summary["PolicyId"] == json!(id))
            .unwrap_or_else(|| panic!("case {case:?}: {id} is missing from {summaries:?}"));
        assert_eq!(
            member(summary, "Tags"),
            tags,
            "case {case:?}: a summary must report its policy's tag set, got {:?}",
            member(summary, "Tags")
        );
        // And the full read of the same policy agrees with the summary.
        assert_eq!(
            at(get(&router, &id).await.policy(), "/Tags"),
            tags,
            "case {case:?}: the summary and the full read must report one tag set"
        );
    }
}
