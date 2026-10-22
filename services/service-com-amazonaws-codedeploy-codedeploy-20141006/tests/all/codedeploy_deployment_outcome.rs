// SPDX-License-Identifier: CC0-1.0

//! What a CodeDeploy deployment ENDS as, what a deployment group reads back as,
//! and which code CodeDeploy refuses a bad value with — driven over the wire
//! through the same front door as `codedeploy_lifecycle`, whose harness this
//! suite reuses.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, s3_revision, terminal_deployment, Emulator};

/// The service role every deployment group here is created with; CodeDeploy only
/// stores it.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// Create an application and a deployment group of it carrying `extras`.
async fn group_with(emulator: &Emulator, application: &str, platform: &str, extras: &Value) {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": "dg",
        "serviceRoleArn": SERVICE_ROLE,
    });
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    emulator.ok("CreateDeploymentGroup", request).await;
}

/// Read back the one deployment group `group_with` created.
async fn group_info(emulator: &Emulator, application: &str) -> Value {
    emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": "dg"}),
        )
        .await["deploymentGroupInfo"]
        .clone()
}

// ---------------------------------------------------------------------------
// The terminal status of a deployment
// ---------------------------------------------------------------------------

/// RULE: a deployment ends `Succeeded` only if its deployment group resolved at
/// least one target to deploy to; a group that resolves none ends the deployment
/// `Failed`, with the `NO_INSTANCES` error information saying so.
#[tokio::test]
async fn a_deployment_ends_failed_when_its_group_resolves_no_target() {
    let emulator = Emulator::new();

    // Rows differ along the compute platform and along how the group selects its
    // targets: an EC2 selector (tag filter, tag set, Auto Scaling group) resolves
    // nothing, because CodeDeploy's state keeps no EC2 inventory, and an
    // on-premises selector resolves only registered instances whose tags it
    // matches — none are registered here — while an ECS group's targets are the
    // `ecsServices` it carries and a Lambda group's target is the function its
    // revision names.
    let cases: &[(&str, &str, Value, &str)] = &[
        ("server with no targeting at all", "Server", json!({}), "Failed"),
        (
            "server with an ec2 tag filter",
            "Server",
            json!({"ec2TagFilters": [{"Key": "Name", "Value": "web", "Type": "KEY_AND_VALUE"}]}),
            "Failed",
        ),
        (
            "server with an ec2 tag set",
            "Server",
            json!({"ec2TagSet": {"ec2TagSetList": [[{"Key": "Name", "Value": "web", "Type": "KEY_AND_VALUE"}]]}}),
            "Failed",
        ),
        (
            "server with an auto scaling group",
            "Server",
            json!({"autoScalingGroups": ["asg-web"]}),
            "Failed",
        ),
        (
            "server with an on-premises tag filter",
            "Server",
            json!({"onPremisesInstanceTagFilters": [{"Key": "Name", "Value": "rack", "Type": "KEY_AND_VALUE"}]}),
            "Failed",
        ),
        ("lambda", "Lambda", json!({}), "Succeeded"),
        (
            "ecs with one service",
            "ECS",
            json!({"ecsServices": [{"clusterName": "cluster", "serviceName": "web"}]}),
            "Succeeded",
        ),
        (
            "ecs with two services",
            "ECS",
            json!({
                "ecsServices": [
                    {"clusterName": "cluster", "serviceName": "web"},
                    {"clusterName": "cluster", "serviceName": "api"},
                ]
            }),
            "Succeeded",
        ),
        ("ecs with no services", "ECS", json!({}), "Failed"),
    ];

    for (case, platform, extras, expected) in cases {
        let application = format!("outcome-{}", case.replace(' ', "-"));
        group_with(&emulator, &application, platform, extras).await;
        let created = emulator
            .ok(
                "CreateDeployment",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": "dg",
                    "revision": s3_revision("my-bucket", "app.zip"),
                }),
            )
            .await;
        let deployment_id = created["deploymentId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let info = terminal_deployment(&emulator, &deployment_id, case).await;
        assert_eq!(
            info["status"].as_str(),
            Some(*expected),
            "case {case:?}: expected the terminal status {expected}, got {info:?}"
        );
        if *expected == "Failed" {
            assert_eq!(
                info["errorInformation"]["code"].as_str(),
                Some("NO_INSTANCES"),
                "case {case:?}: expected the NO_INSTANCES error code, got {info:?}"
            );
        } else {
            assert!(
                info["errorInformation"].is_null(),
                "case {case:?}: expected no errorInformation, got {info:?}"
            );
        }
        assert!(
            info["completeTime"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: expected a completeTime with the terminal status, got {info:?}"
        );
    }
}

// ---------------------------------------------------------------------------

/// RULE: an on-premises instance is a deployment target only while it is
/// REGISTERED and its tags satisfy the group's on-premises selector — so a group
/// whose filter the instance's tags match deploys to it and ends `Succeeded`,
/// while one filtering on a tag no registered instance carries resolves none and
/// ends `Failed`.
#[tokio::test]
async fn an_on_premises_instance_is_a_target_only_when_the_group_selects_it() {
    // Rows differ along whether the registered instance is TAGGED for the
    // group's filter, and along the `TagFilterType` the filter uses.
    //
    // One row: the case name, the group's single on-premises tag filter, the
    // `(key, value)` tags put on the registered instance, and the terminal status
    // the deployment must reach.
    type SelectionCase<'a> = (&'a str, Value, &'a [(&'a str, &'a str)], &'a str);
    let cases: &[SelectionCase] = &[
        (
            "untagged, so the filter matches nothing",
            json!({"Key": "Env", "Value": "prod", "Type": "KEY_AND_VALUE"}),
            &[],
            "Failed",
        ),
        (
            "tagged with another value",
            json!({"Key": "Env", "Value": "prod", "Type": "KEY_AND_VALUE"}),
            &[("Env", "staging")],
            "Failed",
        ),
        (
            "tagged with the key and the value the filter names",
            json!({"Key": "Env", "Value": "prod", "Type": "KEY_AND_VALUE"}),
            &[("Env", "prod")],
            "Succeeded",
        ),
        (
            "tagged with the key a KEY_ONLY filter names",
            json!({"Key": "Rack", "Value": "ignored", "Type": "KEY_ONLY"}),
            &[("Rack", "seven")],
            "Succeeded",
        ),
        (
            "tagged with the value a VALUE_ONLY filter names",
            json!({"Key": "ignored", "Value": "seven", "Type": "VALUE_ONLY"}),
            &[("Rack", "seven")],
            "Succeeded",
        ),
    ];

    for (case, filter, tags, expected) in cases {
        // One emulator per row: the registry is shared state, and each row is
        // about the instance IT registered.
        let emulator = Emulator::new();
        let application = "on-premises";
        group_with(
            &emulator,
            application,
            "Server",
            &json!({"onPremisesInstanceTagFilters": [filter]}),
        )
        .await;
        let instance = "rack-seven";
        emulator
            .ok(
                "RegisterOnPremisesInstance",
                json!({
                    "instanceName": instance,
                    "iamUserArn": format!(
                        "arn:aws:iam::{}:user/{instance}",
                        engine::emulator_account_id()
                    ),
                }),
            )
            .await;
        if !tags.is_empty() {
            emulator
                .ok(
                    "AddTagsToOnPremisesInstances",
                    json!({
                        "instanceNames": [instance],
                        "tags": tags
                            .iter()
                            .map(|(key, value)| json!({"Key": key, "Value": value}))
                            .collect::<Vec<Value>>(),
                    }),
                )
                .await;
        }
        // The tags a deployment resolves against are the STORED ones.
        let registered = emulator
            .ok("GetOnPremisesInstance", json!({"instanceName": instance}))
            .await;
        assert_eq!(
            registered["instanceInfo"]["tags"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            tags.len(),
            "case {case:?}: expected the instance to carry {} tag(s), got {registered:?}",
            tags.len()
        );

        let created = emulator
            .ok(
                "CreateDeployment",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": "dg",
                    "revision": s3_revision("my-bucket", "app.zip"),
                }),
            )
            .await;
        let info = terminal_deployment(
            &emulator,
            created["deploymentId"].as_str().unwrap_or_default(),
            case,
        )
        .await;
        assert_eq!(
            info["status"].as_str(),
            Some(*expected),
            "case {case:?}: expected the terminal status {expected}, got {info:?}"
        );
        if *expected == "Failed" {
            assert_eq!(
                info["errorInformation"]["code"].as_str(),
                Some("NO_INSTANCES"),
                "case {case:?}: expected NO_INSTANCES, got {info:?}"
            );
        } else {
            assert!(
                info["errorInformation"].is_null(),
                "case {case:?}: expected no errorInformation, got {info:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Deployment-group response normalization
// ---------------------------------------------------------------------------

/// RULE: a deployment group's instance selectors are BOUNDED — a tag set carries
/// at most the 3 tag groups `TagSetListLimitExceededException` names, and one
/// filter list at most 10 filters — so the work of resolving a deployment's
/// targets from them cannot grow without limit; the limit itself is accepted and
/// only one past it is refused, with the modelled error for that member.
#[tokio::test]
async fn a_deployment_groups_selectors_are_bounded() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "bounded"}))
        .await;

    // A list of `count` distinct tag filters, as the member's `TagFilter` /
    // `EC2TagFilter` shape names its members (`Key`, `Value`, `Type`).
    let filters = |count: usize| -> Value {
        (0..count)
            .map(|index| json!({"Key": format!("k{index}"), "Value": "v", "Type": "KEY_AND_VALUE"}))
            .collect::<Vec<Value>>()
            .into()
    };
    // `count` tag groups, each holding one filter.
    let groups = |count: usize| -> Value {
        (0..count)
            .map(|index| json!([{"Key": format!("g{index}"), "Value": "v", "Type": "KEY_AND_VALUE"}]))
            .collect::<Vec<Value>>()
            .into()
    };

    // Rows differ along the selector carrying the list, and sit at the published
    // limit (accepted) and one past it (refused).
    let cases: &[(&str, Value, Option<&str>)] = &[
        ("ec2 filters at the limit", json!({"ec2TagFilters": filters(10)}), None),
        (
            "ec2 filters past the limit",
            json!({"ec2TagFilters": filters(11)}),
            Some("InvalidEC2TagException"),
        ),
        (
            "on-premises filters at the limit",
            json!({"onPremisesInstanceTagFilters": filters(10)}),
            None,
        ),
        (
            "on-premises filters past the limit",
            json!({"onPremisesInstanceTagFilters": filters(11)}),
            Some("InvalidTagException"),
        ),
        (
            "an ec2 tag set at the limit",
            json!({"ec2TagSet": {"ec2TagSetList": groups(3)}}),
            None,
        ),
        (
            "an ec2 tag set past the limit",
            json!({"ec2TagSet": {"ec2TagSetList": groups(4)}}),
            Some("TagSetListLimitExceededException"),
        ),
        (
            "an on-premises tag set past the limit",
            json!({"onPremisesTagSet": {"onPremisesTagSetList": groups(4)}}),
            Some("TagSetListLimitExceededException"),
        ),
        (
            "a tag set group past the filter limit",
            json!({"onPremisesTagSet": {"onPremisesTagSetList": [filters(11)]}}),
            Some("InvalidTagException"),
        ),
    ];

    for (index, (case, extras, expected)) in cases.iter().enumerate() {
        let mut request = json!({
            "applicationName": "bounded",
            // A distinct name per row, so an accepted row does not collide with
            // the one before it.
            "deploymentGroupName": format!("dg-{index}"),
            "serviceRoleArn": SERVICE_ROLE,
        });
        for (member, value) in extras.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        let (status, body) = emulator.call("CreateDeploymentGroup", request).await;
        match expected {
            Some(code) => assert_eq!(
                (status, error_code(&body).as_str()),
                (400, *code),
                "case {case:?}: expected {code}, got {status} {body:?}"
            ),
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the limit itself to be accepted, got {status} {body:?}"
            ),
        }
    }
}

/// RULE: a deployment group's instance-selection and trigger lists read back as
/// the list the group holds, the empty list included — a member sent empty and
/// the same member omitted give the group the same thing, so they round-trip to
/// the same representation.
#[tokio::test]
async fn a_deployment_groups_lists_read_back_empty_rather_than_absent() {
    let emulator = Emulator::new();

    // The list members of `DeploymentGroupInfo` that are not scoped to one
    // compute platform (`smithy_query(query="shape",shape="DeploymentGroupInfo")`).
    let always_present = [
        "ec2TagFilters",
        "onPremisesInstanceTagFilters",
        "autoScalingGroups",
        "triggerConfigurations",
    ];

    // Rows differ along the form the request gave those members: omitted
    // entirely, sent as an explicit empty list, and sent with an entry.
    let populated = json!({
        "ec2TagFilters": [{"Key": "Name", "Value": "web", "Type": "KEY_AND_VALUE"}],
        "onPremisesInstanceTagFilters": [{"Key": "Rack", "Value": "7", "Type": "KEY_AND_VALUE"}],
        "autoScalingGroups": ["asg-web"],
        "triggerConfigurations": [{
            "triggerName": "on-failure",
            "triggerTargetArn": "arn:aws:sns:us-west-2:123456789012:deploys",
            "triggerEvents": ["DeploymentFailure"],
        }],
    });
    let cases: &[(&str, Value, usize)] = &[
        ("members omitted", json!({}), 0),
        (
            "members sent empty",
            json!({
                "ec2TagFilters": [],
                "onPremisesInstanceTagFilters": [],
                "autoScalingGroups": [],
                "triggerConfigurations": [],
            }),
            0,
        ),
        ("members sent with an entry", populated, 1),
    ];

    let mut representations: Vec<(&str, Vec<Value>)> = Vec::new();
    for (case, extras, expected_len) in cases {
        let application = format!("lists-{}", case.replace(' ', "-"));
        group_with(&emulator, &application, "Server", extras).await;
        let info = group_info(&emulator, &application).await;
        let mut members = Vec::new();
        for member in always_present {
            let value = &info[member];
            assert_eq!(
                value.as_array().map(Vec::len),
                Some(*expected_len),
                "case {case:?}: expected {member} as a list of {expected_len}, got {info:?}"
            );
            members.push(value.clone());
        }
        representations.push((case, members));
    }

    // Absent and explicitly empty are the SAME document.
    let omitted = &representations[0].1;
    let empty = &representations[1].1;
    assert_eq!(
        omitted, empty,
        "case absent vs empty: expected one representation, got {omitted:?} and {empty:?}"
    );

    // `ecsServices` is the counter-example the model scopes to one platform, so
    // an EC2/on-premises group does not report it at all.
    let info = group_info(&emulator, "lists-members-omitted").await;
    assert!(
        info["ecsServices"].is_null(),
        "case ecsServices on a Server group: expected the member absent, got {info:?}"
    );
}

/// RULE: an `autoRollbackConfiguration` is stored and reported only when it
/// carries information — it lists a triggering event, or it is enabled — so a
/// disabled one listing none is normalised away, and one that is enabled without
/// any event, which could never fire, is refused with the modelled
/// `InvalidAutoRollbackConfigException`.
#[tokio::test]
async fn an_information_free_auto_rollback_configuration_is_normalised_away() {
    let emulator = Emulator::new();

    // Rows differ along `enabled` and along the number of events listed. The
    // event values are the model's `AutoRollbackEvent` enum
    // (`smithy_query(query="shape",shape="AutoRollbackEvent")`:
    // DEPLOYMENT_FAILURE | DEPLOYMENT_STOP_ON_ALARM | DEPLOYMENT_STOP_ON_REQUEST)
    // — the rows below use the two the issue did not cite as well.
    #[derive(Debug)]
    enum Expected {
        /// The member is not reported at all.
        Omitted,
        /// The member is reported with these events.
        Kept(&'static [&'static str]),
        /// The request is refused with `InvalidAutoRollbackConfigException`.
        Refused,
    }
    let cases: &[(&str, Option<Value>, Expected)] = &[
        ("omitted", None, Expected::Omitted),
        (
            "disabled with an empty event list",
            Some(json!({"enabled": false, "events": []})),
            Expected::Omitted,
        ),
        (
            "disabled with no event list",
            Some(json!({"enabled": false})),
            Expected::Omitted,
        ),
        (
            "an empty object",
            Some(json!({})),
            Expected::Omitted,
        ),
        (
            "disabled but listing an event",
            Some(json!({"enabled": false, "events": ["DEPLOYMENT_STOP_ON_REQUEST"]})),
            Expected::Kept(&["DEPLOYMENT_STOP_ON_REQUEST"]),
        ),
        (
            "enabled with one event",
            Some(json!({"enabled": true, "events": ["DEPLOYMENT_STOP_ON_ALARM"]})),
            Expected::Kept(&["DEPLOYMENT_STOP_ON_ALARM"]),
        ),
        (
            "enabled with every event",
            Some(json!({"enabled": true, "events": [
                "DEPLOYMENT_FAILURE", "DEPLOYMENT_STOP_ON_ALARM", "DEPLOYMENT_STOP_ON_REQUEST",
            ]})),
            Expected::Kept(&[
                "DEPLOYMENT_FAILURE",
                "DEPLOYMENT_STOP_ON_ALARM",
                "DEPLOYMENT_STOP_ON_REQUEST",
            ]),
        ),
        (
            "enabled with an empty event list",
            Some(json!({"enabled": true, "events": []})),
            Expected::Refused,
        ),
        (
            "enabled with no event list",
            Some(json!({"enabled": true})),
            Expected::Refused,
        ),
    ];

    for (case, config, expected) in cases {
        let application = format!("rollback-{}", case.replace(' ', "-"));
        emulator
            .ok("CreateApplication", json!({"applicationName": application}))
            .await;
        let mut group_request = json!({
            "applicationName": application,
            "deploymentGroupName": "dg",
            "serviceRoleArn": SERVICE_ROLE,
        });
        if let Some(config) = config {
            group_request["autoRollbackConfiguration"] = config.clone();
        }

        if matches!(expected, Expected::Refused) {
            let (status, body) = emulator
                .call("CreateDeploymentGroup", group_request.clone())
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidAutoRollbackConfigException"),
                "case {case:?} on CreateDeploymentGroup: expected \
                 InvalidAutoRollbackConfigException, got {status} {body:?}"
            );
            // The SAME member on CreateDeployment is refused the same way, so a
            // group has to exist for the deployment to name.
            emulator
                .ok(
                    "CreateDeploymentGroup",
                    json!({
                        "applicationName": application,
                        "deploymentGroupName": "dg",
                        "serviceRoleArn": SERVICE_ROLE,
                    }),
                )
                .await;
            let mut deployment_request = json!({
                "applicationName": application,
                "deploymentGroupName": "dg",
                "revision": s3_revision("my-bucket", "app.zip"),
            });
            deployment_request["autoRollbackConfiguration"] =
                config.clone().expect("a refused row carries a configuration");
            let (status, body) = emulator.call("CreateDeployment", deployment_request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidAutoRollbackConfigException"),
                "case {case:?} on CreateDeployment: expected \
                 InvalidAutoRollbackConfigException, got {status} {body:?}"
            );
            continue;
        }

        emulator
            .ok("CreateDeploymentGroup", group_request)
            .await;
        let info = group_info(&emulator, &application).await;
        let mut deployment_request = json!({
            "applicationName": application,
            "deploymentGroupName": "dg",
            "revision": s3_revision("my-bucket", "app.zip"),
        });
        if let Some(config) = config {
            deployment_request["autoRollbackConfiguration"] = config.clone();
        }
        let created = emulator.ok("CreateDeployment", deployment_request).await;
        let deployment = emulator
            .ok(
                "GetDeployment",
                json!({"deploymentId": created["deploymentId"]}),
            )
            .await["deploymentInfo"]
            .clone();

        for (read, read_case) in [(&info, "GetDeploymentGroup"), (&deployment, "GetDeployment")] {
            match expected {
                Expected::Omitted => assert!(
                    read["autoRollbackConfiguration"].is_null(),
                    "case {case:?} on {read_case}: expected no autoRollbackConfiguration, \
                     got {read:?}"
                ),
                Expected::Kept(events) => {
                    let got: Vec<&str> = read["autoRollbackConfiguration"]["events"]
                        .as_array()
                        .map(|list| list.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    assert_eq!(
                        got, *events,
                        "case {case:?} on {read_case}: expected the events {events:?}, got {read:?}"
                    );
                }
                Expected::Refused => unreachable!("refused rows return above"),
            }
        }
    }

    // The refusal is a check of the request's own member, so it runs only once
    // the resources the request NAMES have been resolved: the same configuration
    // against an application that does not exist is answered for the application.
    let (status, body) = emulator
        .call(
            "CreateDeploymentGroup",
            json!({
                "applicationName": "rollback-ghost",
                "deploymentGroupName": "dg",
                "serviceRoleArn": SERVICE_ROLE,
                "autoRollbackConfiguration": {"enabled": true, "events": []},
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ApplicationDoesNotExistException"),
        "case an unknown application with a refused rollback configuration: expected \
         ApplicationDoesNotExistException, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

/// RULE: an operation taking a `deploymentId` gates its FORMAT before looking it
/// up — a value that is not shaped like a minted `d-XXXXXXXXX` id is refused
/// with `InvalidDeploymentIdException`, an absent or empty one with
/// `DeploymentIdRequiredException`, and only a well-formed id is discriminated
/// on existence with `DeploymentDoesNotExistException`.
#[tokio::test]
async fn a_malformed_deployment_id_is_refused_before_the_lookup() {
    let emulator = Emulator::new();
    let application = "ids";
    group_with(&emulator, application, "Server", &json!({})).await;
    let created = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": "dg",
                "revision": s3_revision("my-bucket", "app.zip"),
            }),
        )
        .await;
    let minted = created["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows differ along HOW the id departs from the minted format: no prefix, the
    // wrong prefix, too short, too long, the wrong alphabet, and far too long.
    let overlong = format!("d-{}", "A".repeat(500));
    let cases: &[(&str, &str)] = &[
        ("not-a-deployment-id", "InvalidDeploymentIdException"),
        ("ABCDEFGHI", "InvalidDeploymentIdException"),
        ("x-ABCDEFGHI", "InvalidDeploymentIdException"),
        ("d-ABCDEFGH", "InvalidDeploymentIdException"),
        ("d-ABCDEFGHIJ", "InvalidDeploymentIdException"),
        ("d-abcdefghi", "InvalidDeploymentIdException"),
        ("d-ABCDEFGH!", "InvalidDeploymentIdException"),
        ("d-ABCDEF GH", "InvalidDeploymentIdException"),
        (overlong.as_str(), "InvalidDeploymentIdException"),
        ("   ", "DeploymentIdRequiredException"),
        ("", "DeploymentIdRequiredException"),
        // Well formed, so it is answered on existence instead.
        ("d-ABCDEFGHI", "DeploymentDoesNotExistException"),
        ("d-999999999", "DeploymentDoesNotExistException"),
    ];

    for (id, expected) in cases {
        let (status, body) = emulator
            .call("GetDeployment", json!({"deploymentId": id}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case id {id:?}: expected {expected}, got {status} {body:?}"
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.chars().count() < 200,
            "case id {id:?}: expected a bounded refusal message, got {} characters",
            message.chars().count()
        );
    }

    // The id the service itself minted is well formed and found.
    let found = emulator
        .ok("GetDeployment", json!({"deploymentId": minted}))
        .await;
    assert_eq!(
        found["deploymentInfo"]["deploymentId"].as_str(),
        Some(minted.as_str()),
        "case the minted id: expected the stored deployment, got {found:?}"
    );
}

/// RULE: `CreateDeployment` resolves every resource its request NAMES — the
/// application, then that application's deployment group, then the deployment
/// configuration — before it validates the request's own members, so a request
/// against a resource that does not exist is answered for that resource.
#[tokio::test]
async fn create_deployment_resolves_the_resources_it_names_before_its_members() {
    let emulator = Emulator::new();
    group_with(&emulator, "ordered", "Server", &json!({})).await;

    // Every row omits or breaks the revision as well, so the refusal the handler
    // answers with shows which check ran first.
    let cases: &[(&str, Value, &str)] = &[
        (
            "unknown application, no revision",
            json!({"applicationName": "ghost", "deploymentGroupName": "dg"}),
            "ApplicationDoesNotExistException",
        ),
        (
            "unknown application, empty revision",
            json!({"applicationName": "ghost", "deploymentGroupName": "dg", "revision": {}}),
            "ApplicationDoesNotExistException",
        ),
        (
            "unknown group, no revision",
            json!({"applicationName": "ordered", "deploymentGroupName": "ghost-dg"}),
            "DeploymentGroupDoesNotExistException",
        ),
        (
            "unknown configuration, no revision",
            json!({
                "applicationName": "ordered",
                "deploymentGroupName": "dg",
                "deploymentConfigName": "CodeDeployDefault.NotAThing",
            }),
            "DeploymentConfigDoesNotExistException",
        ),
        (
            "every resource resolves, no revision",
            json!({"applicationName": "ordered", "deploymentGroupName": "dg"}),
            "RevisionRequiredException",
        ),
        (
            "every resource resolves, empty revision",
            json!({"applicationName": "ordered", "deploymentGroupName": "dg", "revision": {}}),
            "InvalidRevisionException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator.call("CreateDeployment", request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case:?}: expected {expected}, got {status} {body:?}"
        );
    }
}

/// RULE: a value outside one of this service's enumerated members is refused
/// with the modelled `Invalid…Exception` the operations carrying that member
/// declare for it, never with a generic `ValidationException` no operation
/// models.
#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused_with_the_modelled_code() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "enums"}))
        .await;

    // Rows span the enumerated members of this service's inputs, each with the
    // code its operations model (see the crate root's OPERATIONS.md): a scalar
    // enum, an enum inside a list, and an enum inside a structure in a list.
    let cases: &[(&str, Value, &str)] = &[
        (
            "ListOnPremisesInstances",
            json!({"registrationStatus": "NotAnEnumValue"}),
            "InvalidRegistrationStatusException",
        ),
        (
            "ListOnPremisesInstances",
            json!({"tagFilters": [{"Key": "k", "Value": "v", "Type": "KEY_AND_MAYBE_VALUE"}]}),
            "InvalidTagFilterException",
        ),
        (
            "ListApplicationRevisions",
            json!({"applicationName": "enums", "sortBy": "whenever"}),
            "InvalidSortByException",
        ),
        (
            "ListApplicationRevisions",
            json!({"applicationName": "enums", "sortOrder": "sideways"}),
            "InvalidSortOrderException",
        ),
        (
            "ListApplicationRevisions",
            json!({"applicationName": "enums", "deployed": "maybe"}),
            "InvalidDeployedStateFilterException",
        ),
        (
            "ListDeploymentInstances",
            json!({"deploymentId": "d-ABCDEFGHI", "instanceStatusFilter": ["Exploded"]}),
            "InvalidInstanceStatusException",
        ),
        (
            "ListDeploymentInstances",
            json!({"deploymentId": "d-ABCDEFGHI", "instanceTypeFilter": ["Sideways"]}),
            "InvalidInstanceTypeException",
        ),
        (
            "ContinueDeployment",
            json!({"deploymentId": "d-ABCDEFGHI", "deploymentWaitType": "FOREVER_WAIT"}),
            "InvalidDeploymentWaitTypeException",
        ),
        (
            "PutLifecycleEventHookExecutionStatus",
            json!({"deploymentId": "d-ABCDEFGHI", "status": "Exploded"}),
            "InvalidLifecycleEventHookExecutionStatusException",
        ),
        (
            "ListDeployments",
            json!({"includeOnlyStatuses": ["Exploded"]}),
            "InvalidDeploymentStatusException",
        ),
    ];

    for (operation, request, expected) in cases {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {operation}/{request}: expected {expected}, got {status} {body:?}"
        );
    }

    // The refusal is about the VALUE, not about the operation: the same member
    // carrying a value the enum does list is not refused at the boundary at all,
    // so the operation itself gets to answer — whatever that answer is while the
    // operation is still a scaffold.
    let (status, body) = emulator
        .call(
            "ListOnPremisesInstances",
            json!({"registrationStatus": "Registered"}),
        )
        .await;
    assert_ne!(
        status, 400,
        "case a value the enum lists: expected no boundary refusal, got {status} {body:?}"
    );
    assert!(
        !error_code(&body).starts_with("Invalid"),
        "case a value the enum lists: expected no Invalid…Exception, got {status} {body:?}"
    );
}
