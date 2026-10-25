// SPDX-License-Identifier: CC0-1.0

//! An APPLICATION group's identity and its membership tag, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one row: a group
//! whose stored service configuration declares it an application group has an
//! identity that QUALIFIES its name with an application id minted once when the
//! group is created (`…:group/<name>/<id>`), and every `Group` structure it is
//! reported in carries the membership tag `awsApplication` bound to that same
//! identity; a group whose configuration declares nothing of the kind has neither.
//!
//! The rows differ along what the rule generalises over: whether the configuration
//! declares the application type at all, where in the configuration it sits, whether
//! it carries parameters, the alternative ways a group holds no such configuration (a
//! configuration of another type, a resource query, nothing, an empty list), the
//! region the group lives in and the length of its name.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, Emulator};
use super::tag_sync_tasks::{application_segment, group_arn, role_arn};

/// The service-configuration type that makes a group an application group.
///
/// Source: `smithy_query(query="doc",shape="GroupConfigurationType")` and
/// `GroupResources`'s own documentation, which names it as one of the three group
/// types it supports.
const APPLICATION_GROUP: &str = "AWS::ResourceGroups::ApplicationGroup";

/// The one key the modelled `ApplicationTagKey` admits (`^awsApplication$`).
///
/// Source: `smithy_query(query="shape",shape="ApplicationTag")`.
const APPLICATION_TAG_KEY: &str = "awsApplication";

/// The longest name segment the modelled `ApplicationArn` writes
/// (`group/[a-zA-Z0-9_\.-]{1,150}/…`), which is also the bound the qualified
/// alternative of the modelled `GroupArn` writes.
///
/// Source: `smithy_query(query="shape",shape="ApplicationArn")`.
const MAX_QUALIFIED_NAME: usize = 150;

/// One `CreateGroup` request, and whether the group it creates is an application
/// group.
struct GroupCase {
    label: &'static str,
    /// The region the creating caller's credential scope names, so the group's
    /// stored identity varies with it too.
    region: &'static str,
    name: String,
    request: Value,
    application: bool,
}

/// Groups spanning both sides of the rule: a configuration declaring the
/// application type alone, beside another item, and carrying parameters; and the
/// ways a group holds no such declaration at all.
fn group_cases() -> Vec<GroupCase> {
    let at_the_bound = "a".repeat(MAX_QUALIFIED_NAME);
    vec![
        GroupCase {
            label: "a configuration declaring only the application type",
            region: "us-east-1",
            name: "myapp".to_string(),
            request: json!({
                "Name": "myapp",
                "DisplayName": "My App",
                "Criticality": 1,
                "Configuration": [{ "Type": APPLICATION_GROUP }],
            }),
            application: true,
        },
        GroupCase {
            label: "the application type as the second configuration item",
            region: "eu-west-1",
            name: "app.two_2-second".to_string(),
            request: json!({
                "Name": "app.two_2-second",
                "Configuration": [
                    { "Type": "AWS::AppRegistry::Application" },
                    { "Type": APPLICATION_GROUP },
                ],
            }),
            application: true,
        },
        GroupCase {
            label: "the application type carrying parameters",
            region: "ap-southeast-2",
            name: "app-with-parameters".to_string(),
            request: json!({
                "Name": "app-with-parameters",
                "Configuration": [{
                    "Type": APPLICATION_GROUP,
                    "Parameters": [{ "Name": "allowed-resource-types", "Values": ["AWS::EC2::Host"] }],
                }],
            }),
            application: true,
        },
        GroupCase {
            label: "an application group whose name is as long as a qualified ARN admits",
            region: "us-gov-east-1",
            name: at_the_bound.clone(),
            request: json!({
                "Name": at_the_bound,
                "Configuration": [{ "Type": APPLICATION_GROUP }],
            }),
            application: true,
        },
        GroupCase {
            label: "a configuration of another type",
            region: "us-west-2",
            name: "hosts".to_string(),
            request: json!({
                "Name": "hosts",
                "Configuration": [{ "Type": "AWS::EC2::HostManagement" }],
            }),
            application: false,
        },
        GroupCase {
            label: "a tag-filter resource query",
            region: "us-east-2",
            name: "tag-query".to_string(),
            request: json!({
                "Name": "tag-query",
                "ResourceQuery": {
                    "Type": "TAG_FILTERS_1_0",
                    "Query": "{\"ResourceTypeFilters\":[\"AWS::AllSupported\"],\"TagFilters\":[]}",
                },
            }),
            application: false,
        },
        GroupCase {
            label: "neither a configuration nor a query",
            region: "eu-central-1",
            name: "plain".to_string(),
            request: json!({ "Name": "plain" }),
            application: false,
        },
        GroupCase {
            label: "an empty configuration list, which declares nothing",
            region: "ca-central-1",
            name: "empty-configuration".to_string(),
            request: json!({ "Name": "empty-configuration", "Configuration": [] }),
            application: false,
        },
    ]
}

/// Whether `partition` is one the modelled ARN production `aws(-[a-z]+)*` admits.
fn is_modelled_partition(partition: &str) -> bool {
    let mut segments = partition.split('-');
    segments.next() == Some("aws")
        && segments.all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase()))
}

/// Whether `region` is one the modelled ARN production `[a-z]{2}(-[a-z]+)+-\d{1}`
/// admits.
fn is_modelled_region(region: &str) -> bool {
    let Some((head, digit)) = region.rsplit_once('-') else {
        return false;
    };
    let segments: Vec<&str> = head.split('-').collect();
    digit.len() == 1
        && digit.bytes().all(|b| b.is_ascii_digit())
        && segments.len() >= 2
        && segments[0].len() == 2
        && segments
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase()))
}

/// Whether `arn` satisfies the modelled `ApplicationArn`: length 12..1600 and
/// `^arn:aws(-[a-z]+)*:resource-groups:[a-z]{2}(-[a-z]+)+-\d{1}:[0-9]{12}:group/[a-zA-Z0-9_\.-]{1,150}/[a-zA-Z0-9]{22,26}$`
/// — the shape an application group's membership tag value must have.
///
/// Source: `smithy_query(query="shape",shape="ApplicationTag")`, whose value target
/// is `ApplicationArn`.
fn is_modelled_application_arn(arn: &str) -> bool {
    if !(12..=1600).contains(&arn.len()) {
        return false;
    }
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    if fields.len() != 6 || fields[0] != "arn" || fields[2] != "resource-groups" {
        return false;
    }
    if !is_modelled_partition(fields[1]) || !is_modelled_region(fields[3]) {
        return false;
    }
    if fields[4].len() != 12 || !fields[4].bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(named) = fields[5].strip_prefix("group/") else {
        return false;
    };
    let Some((name, id)) = named.rsplit_once('/') else {
        return false;
    };
    (1..=MAX_QUALIFIED_NAME).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        && (22..=26).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The `ApplicationTag` a `Group` structure carries, as a list of pairs (absent
/// reads as no pairs at all).
fn application_tag(group: &Value) -> Vec<(String, String)> {
    group["ApplicationTag"]
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or("<not a string>").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert `group` reports the identity and the membership tag `case` calls for.
fn assert_identity(case: &GroupCase, form: &str, group: &Value) {
    let label = case.label;
    let unqualified = group_arn(case.region, &case.name);
    let arn = group["GroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?} ({form}): expected a GroupArn, got {group}"));
    assert_eq!(
        group["Name"].as_str(),
        Some(case.name.as_str()),
        "case {label:?} ({form}): expected the group's own name, got {group}",
    );
    if !case.application {
        assert_eq!(
            arn, unqualified,
            "case {label:?} ({form}): expected the unqualified ARN {unqualified}, got {arn}",
        );
        assert_eq!(
            application_tag(group),
            Vec::new(),
            "case {label:?} ({form}): expected no ApplicationTag on a group that is not \
             an application group, got {group}",
        );
        return;
    }
    let id = arn
        .strip_prefix(&unqualified)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or_else(|| {
            panic!(
                "case {label:?} ({form}): expected an ARN qualifying {unqualified} with an \
                 application id, got {arn}"
            )
        });
    assert!(
        id.len() == 26 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "case {label:?} ({form}): expected the qualifying id to be the modelled \
         [a-z0-9]{{26}}, got {id:?}",
    );
    assert!(
        is_modelled_application_arn(arn),
        "case {label:?} ({form}): expected an ARN the modelled ApplicationArn admits, \
         got {arn}",
    );
    assert_eq!(
        application_tag(group),
        vec![(APPLICATION_TAG_KEY.to_string(), arn.to_string())],
        "case {label:?} ({form}): expected the membership tag to bind \
         {APPLICATION_TAG_KEY} to {arn}, got {group}",
    );
}

/// Every row of the table is a request this service accepts, and the table covers
/// both sides of the rule, so it spans the class rather than one point of it.
#[test]
fn the_table_covers_application_groups_and_groups_that_are_not() {
    for application in [true, false] {
        assert!(
            group_cases().iter().any(|case| case.application == application),
            "case {application:?}: expected a row whose group is an application group \
             exactly when {application}, got {:?}",
            group_cases().iter().map(|case| case.label).collect::<Vec<_>>(),
        );
    }
}

/// An application group's identity qualifies its name with an application id, and
/// every `Group` it is reported in — by the create, by a read, by a listing and by
/// an update — carries the membership tag bound to that identity. A group that is
/// not an application group has neither, and reading mints nothing.
#[tokio::test]
async fn an_application_group_reports_its_identity_and_membership_tag_everywhere() {
    let emulator = Emulator::new();
    let mut identities = BTreeSet::new();
    for case in group_cases() {
        let label = case.label;
        let (status, created) = emulator.call_in(case.region, "/groups", &case.request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateGroup to answer 200, got {status} {created}",
        );
        assert_identity(&case, "CreateGroup", &created["Group"]);
        let arn = created["Group"]["GroupArn"]
            .as_str()
            .expect("CreateGroup answers a GroupArn")
            .to_string();
        assert!(
            identities.insert(arn.clone()),
            "case {label:?}: expected a group's identity to be its own, got {arn} twice",
        );

        // Every identifier form the group answers to reports the same identity:
        // its name, its stored ARN, and — for an application group — the name
        // qualified by the application id, the form the modelled `GroupName`
        // admits beside a bare name.
        let mut identifiers = vec![
            ("GetGroup by name", json!({ "Group": case.name })),
            ("GetGroup by the stored ARN", json!({ "Group": arn })),
            ("GetGroup by the deprecated GroupName", json!({ "GroupName": case.name })),
        ];
        if case.application {
            let qualified_name = arn
                .rsplit_once(":group/")
                .map(|(_, named)| named.to_string())
                .expect("a group ARN names its group");
            identifiers.push((
                "GetGroup by the qualified name",
                json!({ "Group": qualified_name }),
            ));
        }
        for (form, body) in identifiers {
            let read = emulator.ok_in(case.region, "/get-group", &body).await;
            assert_identity(&case, form, &read["Group"]);
            assert_eq!(
                read["Group"], created["Group"],
                "case {label:?} ({form}): expected the stored group {}, got {}",
                created["Group"], read["Group"],
            );
        }

        // A listing serializes the same `Group`, so it carries the same tag.
        let listed = emulator.ok_in(case.region, "/groups-list", &json!({})).await;
        let in_listing = listed["Groups"]
            .as_array()
            .expect("ListGroups answers a Groups list")
            .iter()
            .find(|group| group["Name"].as_str() == Some(case.name.as_str()))
            .unwrap_or_else(|| panic!("case {label:?}: expected the group in {listed}"))
            .clone();
        assert_identity(&case, "ListGroups", &in_listing);

        // An update reports the group it changed, and changes neither its
        // identity nor its membership tag.
        let updated = emulator
            .ok_in(
                case.region,
                "/update-group",
                &json!({ "Group": case.name, "Description": "updated" }),
            )
            .await;
        assert_identity(&case, "UpdateGroup", &updated["Group"]);
    }
}

/// The application id is minted ONCE, when the group is created: a read never mints
/// one, never changes one, and never gives one to a group that is not an application
/// group.
#[tokio::test]
async fn the_application_identity_is_minted_once_and_reads_never_change_it() {
    let emulator = Emulator::new();
    for case in group_cases() {
        let label = case.label;
        let created = emulator.ok_in(case.region, "/groups", &case.request).await;
        let first = created["Group"].clone();
        for round in 0..3 {
            let read = emulator
                .ok_in(case.region, "/get-group", &json!({ "Group": case.name }))
                .await;
            assert_eq!(
                read["Group"], first,
                "case {label:?} (read {round}): expected the same group as at create \
                 {first}, got {}",
                read["Group"],
            );
        }
    }
}

/// Two application groups of the same name in different regions are two groups with
/// two identities, and each region reports only its own.
#[tokio::test]
async fn an_application_identity_belongs_to_one_region() {
    let emulator = Emulator::new();
    let name = "regional-app";
    let request = json!({ "Name": name, "Configuration": [{ "Type": APPLICATION_GROUP }] });
    let mut seen = Vec::new();
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let created = emulator.ok_in(region, "/groups", &request).await;
        let arn = created["Group"]["GroupArn"]
            .as_str()
            .expect("CreateGroup answers a GroupArn")
            .to_string();
        assert!(
            arn.starts_with(&format!("{}/", group_arn(region, name))),
            "case {region:?}: expected an ARN qualifying the group of that region, got {arn}",
        );
        assert_eq!(
            application_tag(&created["Group"]),
            vec![(APPLICATION_TAG_KEY.to_string(), arn.clone())],
            "case {region:?}: expected the membership tag to bind the region's own \
             identity, got {}",
            created["Group"],
        );
        assert!(
            !seen.contains(&arn),
            "case {region:?}: expected a distinct identity per region, got {arn} twice",
        );
        seen.push(arn);
    }
    // An identity minted in one region addresses nothing in another.
    for (region, arn) in ["us-east-1", "eu-west-1", "ap-southeast-2"].iter().zip(&seen) {
        let (status, answered) = emulator
            .call_in("us-west-2", "/get-group", &json!({ "Group": arn }))
            .await;
        assert_eq!(
            (status, error_code(&answered)),
            (404, "NotFoundException"),
            "case {region:?}: expected {arn} to address no group in us-west-2, \
             got {status} {answered}",
        );
    }
}

/// A name longer than a qualified ARN can carry cannot be given an application
/// identity, so an application group is refused for one — and only an application
/// group is: the same name is a perfectly good group otherwise.
#[tokio::test]
async fn a_name_a_qualified_arn_cannot_carry_is_refused_for_an_application_group() {
    let emulator = Emulator::new();
    // The published bound, one past it, and the longest name the modelled
    // `CreateGroupName` admits (`length 1..300`).
    let cases = [
        (MAX_QUALIFIED_NAME - 1, true),
        (MAX_QUALIFIED_NAME, true),
        (MAX_QUALIFIED_NAME + 1, false),
        (300, false),
    ];
    for (length, admitted) in cases {
        let name = "n".repeat(length);
        let (status, answered) = emulator
            .call(
                "/groups",
                &json!({ "Name": name, "Configuration": [{ "Type": APPLICATION_GROUP }] }),
            )
            .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {length}: expected an application group of that name to be created, \
                 got {status} {answered}",
            );
            assert!(
                is_modelled_application_arn(answered["Group"]["GroupArn"].as_str().unwrap_or("")),
                "case {length}: expected an ARN the modelled ApplicationArn admits, \
                 got {answered}",
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {length}: expected 400 BadRequestException for a name no qualified ARN \
             can carry, got {status} {answered}",
        );
        // A refused create stores nothing.
        let (status, read) = emulator.call("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            (status, error_code(&read)),
            (404, "NotFoundException"),
            "case {length}: expected the refused create to store nothing, got {status} {read}",
        );
        // The same name without the application declaration is still a group,
        // because the bound is the application identity's, not a group name's.
        let plain = emulator.ok("/groups", &json!({ "Name": name })).await;
        assert_eq!(
            plain["Group"]["GroupArn"].as_str(),
            Some(group_arn("us-east-1", &name).as_str()),
            "case {length}: expected a group of that name without the application \
             declaration, got {plain}",
        );
        assert_eq!(
            application_tag(&plain["Group"]),
            Vec::new(),
            "case {length}: expected no ApplicationTag on it, got {plain}",
        );
    }
}

/// A tag-sync task nests inside the group's own identity: an application group
/// already carries the application id its task ARN is written under, so the task's
/// ARN extends the group's stored ARN rather than qualifying it a second time.
#[tokio::test]
async fn a_tag_sync_task_nests_inside_the_application_identity() {
    let emulator = Emulator::new();
    for (label, request, application) in [
        (
            "an application group",
            json!({ "Name": "task-app", "Configuration": [{ "Type": APPLICATION_GROUP }] }),
            true,
        ),
        (
            "a group that is not an application group",
            json!({ "Name": "task-plain" }),
            false,
        ),
    ] {
        let name = request["Name"].as_str().expect("a Name").to_string();
        let created = emulator.ok("/groups", &request).await;
        let group_identity = created["Group"]["GroupArn"]
            .as_str()
            .expect("CreateGroup answers a GroupArn")
            .to_string();
        let started = emulator
            .ok(
                "/start-tag-sync-task",
                &json!({
                    "Group": name,
                    "RoleArn": role_arn("ResourceGroupsTagSync"),
                    "TagKey": "Stage",
                    "TagValue": "Test",
                }),
            )
            .await;
        let task_arn = started["TaskArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: expected a TaskArn, got {started}"))
            .to_string();
        assert_eq!(
            started["GroupArn"].as_str(),
            Some(group_identity.as_str()),
            "case {label:?}: expected the task to report the group's stored ARN \
             {group_identity}, got {started}",
        );
        let unqualified = group_arn("us-east-1", &name);
        let qualifying = application_segment(&task_arn, &unqualified).unwrap_or_else(|| {
            panic!(
                "case {label:?}: expected a task ARN nested under {unqualified} as the \
                 modelled TagSyncTaskArn, got {task_arn}"
            )
        });
        if application {
            assert_eq!(
                group_identity,
                format!("{unqualified}/{qualifying}"),
                "case {label:?}: expected the task ARN to carry the group's own \
                 application id, got {task_arn}",
            );
            assert!(
                task_arn.starts_with(&format!("{group_identity}/tag-sync-task/")),
                "case {label:?}: expected the task ARN to extend {group_identity}, \
                 got {task_arn}",
            );
        } else {
            assert_eq!(
                group_identity, unqualified,
                "case {label:?}: expected the group's identity to stay unqualified, \
                 got {group_identity}",
            );
            assert_eq!(
                application_tag(&emulator.ok("/get-group", &json!({ "Group": name })).await
                    ["Group"]),
                Vec::new(),
                "case {label:?}: expected hosting a task to give the group no \
                 membership tag",
            );
        }
        // The task is readable by its own ARN whichever group hosts it.
        let read = emulator
            .ok("/get-tag-sync-task", &json!({ "TaskArn": task_arn }))
            .await;
        assert_eq!(
            read["GroupArn"].as_str(),
            Some(group_identity.as_str()),
            "case {label:?}: expected GetTagSyncTask to report {group_identity}, got {read}",
        );
    }
}
