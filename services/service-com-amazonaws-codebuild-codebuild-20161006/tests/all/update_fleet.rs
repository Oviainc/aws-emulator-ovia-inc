// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateFleet`, driven over the wire the router serves through
//! the harness the project -> build suite owns (`crate::project_build_lifecycle`) and the
//! fleet helpers `crate::fleet_lifecycle` owns.
//!
//! RULE under test: `UpdateFleet` resolves the fleet its `arn` addresses and replaces
//! exactly the settings the request carries — each member whole, a structure never
//! merged field by field — leaving every omitted setting and the fleet's identity (name,
//! id, ARN, `created`) as stored, moving `lastModified` and leaving the fleet applying
//! the change until the clock makes its return to `ACTIVE` due. An identifier that
//! addresses no fleet the account holds is refused with `ResourceNotFoundException`, and
//! a merged configuration that is not a valid compute fleet with `InvalidInputException`;
//! either refusal persists nothing.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::fleet_lifecycle::{batch_get_fleets, create_fleet, PAST_THE_FLEET_SETTLE_SPAN};
use crate::project_build_lifecycle::{call, emulator, error_type, expected_arn, REGION};
use crate::update_project::{all_members, instant};

/// The settings every fleet below is created with, so "a member the request did not
/// carry is unchanged" is an assertion with something to hold on to: each one is a
/// member `UpdateFleet` can also replace. The enum values are the model's
/// (`smithy_query(query="shape",shape="ComputeType,EnvironmentType,FleetOverflowBehavior")`).
fn stored_settings() -> Value {
    json!({
        "baseCapacity": 4,
        "environmentType": "LINUX_CONTAINER",
        "computeType": "BUILD_GENERAL1_SMALL",
        "overflowBehavior": "QUEUE",
        "imageId": "ami-0123456789abcdef0",
        "fleetServiceRole": "arn:aws:iam::000000000000:role/service-role/stored",
        "scalingConfiguration": {
            "scalingType": "TARGET_TRACKING_SCALING",
            "maxCapacity": 10,
            "targetTrackingScalingConfigs": [
                {"metricType": "FLEET_UTILIZATION_RATE", "targetValue": 0.5},
            ],
        },
        "vpcConfig": {"vpcId": "vpc-1", "subnets": ["subnet-1"], "securityGroupIds": ["sg-1"]},
        "proxyConfiguration": {
            "defaultBehavior": "ALLOW_ALL",
            "orderedProxyRules": [{"type": "DOMAIN", "effect": "DENY", "entities": ["old.test"]}],
        },
        "tags": [{"key": "team", "value": "build"}],
    })
}

/// Send one `UpdateFleet` for `identifier` with `extra`'s members merged in.
async fn update_fleet(
    router: &MultiServiceRouter,
    identifier: &str,
    extra: Value,
) -> (u16, Value) {
    let mut body = json!({"arn": identifier});
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extra) {
        target.extend(members);
    }
    call(router, "UpdateFleet", body).await
}

/// `UpdateFleet`, asserting it succeeded, handing back the fleet it returned.
async fn update_ok(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    extra: Value,
) -> Value {
    let (status, body) = update_fleet(router, identifier, extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateFleet to answer 200, got {status} with {body}"
    );
    body["fleet"].clone()
}

/// The fleet `identifier` addresses, as the read path reports it.
async fn fetch_fleet(router: &MultiServiceRouter, identifier: &str) -> Value {
    batch_get_fleets(router, &[identifier]).await["fleets"][0].clone()
}

/// Create a fleet carrying [`stored_settings`] and hand back `(its ARN, its record)`.
async fn seeded_fleet(router: &MultiServiceRouter, name: &str) -> (String, Value) {
    let created = create_fleet(router, name, stored_settings()).await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a created fleet carries an arn, got {created}"))
        .to_string();
    (arn, created)
}

/// Assert every member of `after` except the ones a successful update always moves
/// (`lastModified`, `status`) and the ones `changed` names reads exactly as in `before`.
fn only_the_requested_members_changed(case: &str, before: &Value, after: &Value, changed: &Value) {
    let changed = changed.as_object().expect("the expectation is an object");
    for (member, want) in changed {
        assert_eq!(
            &after[member], want,
            "case {case:?}: expected {member} = {want}, got {after}"
        );
    }
    for member in all_members(before, after) {
        if member == "lastModified" || member == "status" || changed.contains_key(&member) {
            continue;
        }
        assert_eq!(
            after[&member], before[&member],
            "case {case:?}: {member} was not in the request, so it must stay {:?}, got {:?}",
            before[&member], after[&member]
        );
    }
}

/// RULE: a member the request carries replaces the stored setting whole; every member it
/// omits is reported exactly as stored, and the read path hands back the same record. The
/// table varies the member, its value, its type (number, enum string, string, structure,
/// list), its entry count and its form (set, replaced, emptied).
#[tokio::test]
async fn update_fleet_replaces_the_members_it_was_given_and_keeps_the_rest() {
    let router = emulator();

    // The scaling configuration the stored fleet carries, as the OUTPUT shape reports
    // it: `desiredCapacity` is the machines the fleet wants, which is its `baseCapacity`.
    let stored_scaling = |desired: i64| {
        json!({
            "scalingType": "TARGET_TRACKING_SCALING",
            "maxCapacity": 10,
            "targetTrackingScalingConfigs": [
                {"metricType": "FLEET_UTILIZATION_RATE", "targetValue": 0.5},
            ],
            "desiredCapacity": desired,
        })
    };

    // (case, the members the request carries, the members that must change and to what)
    let cases: Vec<(&str, Value, Value)> = vec![
        ("nothing but the arn", json!({}), json!({})),
        (
            "a bigger baseCapacity",
            json!({"baseCapacity": 8}),
            json!({"baseCapacity": 8, "scalingConfiguration": stored_scaling(8)}),
        ),
        (
            "baseCapacity at the floor",
            json!({"baseCapacity": 1}),
            json!({"baseCapacity": 1, "scalingConfiguration": stored_scaling(1)}),
        ),
        (
            "a replacement environment type",
            json!({"environmentType": "ARM_CONTAINER"}),
            json!({"environmentType": "ARM_CONTAINER"}),
        ),
        (
            "a replacement compute type",
            json!({"computeType": "BUILD_GENERAL1_LARGE"}),
            json!({"computeType": "BUILD_GENERAL1_LARGE"}),
        ),
        (
            "an attribute-driven compute type with its configuration",
            json!({
                "computeType": "ATTRIBUTE_BASED_COMPUTE",
                "computeConfiguration": {
                    "vCpu": 8,
                    "memory": 16,
                    "disk": 128,
                    "machineType": "GENERAL",
                },
            }),
            json!({
                "computeType": "ATTRIBUTE_BASED_COMPUTE",
                "computeConfiguration": {
                    "vCpu": 8,
                    "memory": 16,
                    "disk": 128,
                    "machineType": "GENERAL",
                },
            }),
        ),
        (
            "a custom instance type with its configuration",
            json!({
                "computeType": "CUSTOM_INSTANCE_TYPE",
                "computeConfiguration": {"instanceType": "m6i.large"},
            }),
            json!({
                "computeType": "CUSTOM_INSTANCE_TYPE",
                "computeConfiguration": {"instanceType": "m6i.large"},
            }),
        ),
        (
            "a replacement overflow behavior",
            json!({"overflowBehavior": "ON_DEMAND"}),
            json!({"overflowBehavior": "ON_DEMAND"}),
        ),
        (
            "a replacement image id",
            json!({"imageId": "ami-00000000000000001"}),
            json!({"imageId": "ami-00000000000000001"}),
        ),
        (
            "a replacement fleet service role",
            json!({"fleetServiceRole": "arn:aws:iam::000000000000:role/service-role/replaced"}),
            json!({"fleetServiceRole": "arn:aws:iam::000000000000:role/service-role/replaced"}),
        ),
        // A structure replaces WHOLE: the stored target-tracking configs are gone,
        // because the configuration the request carried declares none.
        (
            "a scaling configuration replacing the stored one whole",
            json!({"scalingConfiguration": {
                "scalingType": "TARGET_TRACKING_SCALING",
                "maxCapacity": 20,
            }}),
            json!({"scalingConfiguration": {
                "scalingType": "TARGET_TRACKING_SCALING",
                "maxCapacity": 20,
                "desiredCapacity": 4,
            }}),
        ),
        (
            "a replacement vpc configuration with two subnets",
            json!({"vpcConfig": {
                "vpcId": "vpc-2",
                "subnets": ["subnet-2", "subnet-3"],
                "securityGroupIds": ["sg-2"],
            }}),
            json!({"vpcConfig": {
                "vpcId": "vpc-2",
                "subnets": ["subnet-2", "subnet-3"],
                "securityGroupIds": ["sg-2"],
            }}),
        ),
        (
            "a replacement proxy configuration with two rules",
            json!({"proxyConfiguration": {
                "defaultBehavior": "DENY_ALL",
                "orderedProxyRules": [
                    {"type": "DOMAIN", "effect": "ALLOW", "entities": ["new.test"]},
                    {"type": "IP", "effect": "ALLOW", "entities": ["10.0.0.0/8"]},
                ],
            }}),
            json!({"proxyConfiguration": {
                "defaultBehavior": "DENY_ALL",
                "orderedProxyRules": [
                    {"type": "DOMAIN", "effect": "ALLOW", "entities": ["new.test"]},
                    {"type": "IP", "effect": "ALLOW", "entities": ["10.0.0.0/8"]},
                ],
            }}),
        ),
        (
            "two tags replacing one",
            json!({"tags": [{"key": "a", "value": "1"}, {"key": "b", "value": "2"}]}),
            json!({"tags": [{"key": "a", "value": "1"}, {"key": "b", "value": "2"}]}),
        ),
        ("tags emptied", json!({"tags": []}), json!({"tags": []})),
        (
            "two members at once",
            json!({"baseCapacity": 6, "imageId": "ami-00000000000000002"}),
            json!({
                "baseCapacity": 6,
                "imageId": "ami-00000000000000002",
                "scalingConfiguration": stored_scaling(6),
            }),
        ),
    ];

    for (index, (case, request, expected)) in cases.iter().enumerate() {
        let name = format!("replace-{index:02}");
        let (arn, stored) = seeded_fleet(&router, &name).await;

        let updated = update_ok(&router, case, &arn, request.clone()).await;

        only_the_requested_members_changed(case, &stored, &updated, expected);
        assert_eq!(
            updated["status"]["statusCode"], "UPDATING",
            "case {case:?}: a fleet applying a change reports UPDATING, got {updated}"
        );
        assert!(
            instant(&updated, "lastModified") > instant(&stored, "lastModified"),
            "case {case:?}: a successful update must move lastModified past {}, got {}",
            instant(&stored, "lastModified"),
            instant(&updated, "lastModified")
        );

        // The read path reports the record this call persisted. `status` is excluded
        // because the clock may have made the fleet's return to ACTIVE due between the
        // two calls — the suite below is what pins that transition.
        let fetched = fetch_fleet(&router, &arn).await;
        for member in all_members(&updated, &fetched) {
            if member == "status" {
                continue;
            }
            assert_eq!(
                fetched[&member], updated[&member],
                "case {case:?}: BatchGetFleets must report the updated {member}, got {fetched}"
            );
        }
    }
}

/// RULE: an identifier resolves only to the fleet it addresses exactly, so one the
/// account holds no fleet for is refused with the modelled `ResourceNotFoundException`
/// (HTTP 400) naming the identifier the caller gave, and persists nothing. The rows vary
/// the way the identifier misses: the resource type, the missing fleet id, the service,
/// the account, the region, the fleet id itself, and a bare name nothing holds.
#[tokio::test]
async fn an_identifier_addressing_no_fleet_is_refused_and_changes_nothing() {
    let router = emulator();
    let (arn, created) = seeded_fleet(&router, "held-fleet").await;

    let cases: Vec<(&str, String)> = vec![
        (
            "an ARN of another resource type",
            expected_arn("project/held-fleet"),
        ),
        (
            "a fleet ARN carrying no fleet id",
            expected_arn("fleet/held-fleet"),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:codepipeline:{REGION}:000000000000:fleet/held-fleet:1"),
        ),
        (
            "this fleet's ARN in another account",
            arn.replace(engine::EMULATOR_ACCOUNT_ID, "000000000001"),
        ),
        (
            "this fleet's ARN in another region",
            arn.replace(REGION, "eu-west-1"),
        ),
        (
            "this fleet's ARN with another fleet id",
            format!(
                "{}:00000000-0000-8000-8000-000000000000",
                expected_arn("fleet/held-fleet")
            ),
        ),
        ("a name the account holds no fleet for", "no-such-fleet".to_string()),
    ];

    for (case, identifier) in cases {
        let (status, body) = update_fleet(&router, &identifier, json!({"baseCapacity": 9})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(format!("Fleet {identifier} does not exist")),
            "case {case:?}: the refusal names the identifier the caller gave, got {body}"
        );

        // The fleet the account does hold is untouched: nothing was resolved, so
        // nothing was written.
        let fetched = fetch_fleet(&router, &arn).await;
        assert_eq!(
            fetched["lastModified"], created["lastModified"],
            "case {case:?}: a refused update must not move lastModified, got {fetched}"
        );
        assert_eq!(
            fetched["baseCapacity"], created["baseCapacity"],
            "case {case:?}: a refused update must persist nothing, got {fetched}"
        );
    }
}

/// What one row of the merged-configuration table expects back.
enum MergedOutcome {
    /// The request is accepted and the fleet changed.
    Accepted,
    /// Refused by the `FleetCapacity` floor, which CodeBuild words with one fixed
    /// sentence naming neither the member nor the value sent.
    BelowFloor,
    /// Refused by a rule whose refusal names the member it is about.
    NamesMember(&'static str),
}

/// RULE: the configuration the fleet CARRIES once the request has been applied — not the
/// request alone — is what has to be a valid compute fleet, so an update is refused with
/// the modelled `InvalidInputException` exactly when the merge of request and stored
/// record breaks one of the fleet rules (a `FleetCapacity` member below one machine, a
/// `maxCapacity` below the capacity the fleet is provisioned at, or an attribute-driven
/// `computeType` with no `computeConfiguration` to describe its machines), and persists
/// nothing. Each row changes ONE side of the merge, so a rule read off the request alone
/// fails here.
#[tokio::test]
async fn the_merged_configuration_is_what_the_fleet_rules_are_applied_to() {
    let router = emulator();

    // (case, the members the request carries, what the answer must be)
    let cases: Vec<(&str, Value, MergedOutcome)> = vec![
        (
            "baseCapacity one past the floor",
            json!({"baseCapacity": 0}),
            MergedOutcome::BelowFloor,
        ),
        (
            "baseCapacity negative",
            json!({"baseCapacity": -3}),
            MergedOutcome::BelowFloor,
        ),
        (
            "maxCapacity one past the floor",
            json!({"scalingConfiguration": {"maxCapacity": 0}}),
            MergedOutcome::BelowFloor,
        ),
        (
            "maxCapacity below the STORED baseCapacity",
            json!({"scalingConfiguration": {"maxCapacity": 3}}),
            MergedOutcome::NamesMember("scalingConfiguration.maxCapacity"),
        ),
        (
            "baseCapacity raised past the STORED maxCapacity",
            json!({"baseCapacity": 11}),
            MergedOutcome::NamesMember("scalingConfiguration.maxCapacity"),
        ),
        (
            "an attribute-driven compute type the STORED fleet has no configuration for",
            json!({"computeType": "ATTRIBUTE_BASED_COMPUTE"}),
            MergedOutcome::NamesMember("computeConfiguration"),
        ),
        (
            "a custom instance type the STORED fleet has no configuration for",
            json!({"computeType": "CUSTOM_INSTANCE_TYPE"}),
            MergedOutcome::NamesMember("computeConfiguration"),
        ),
        (
            "baseCapacity at the floor",
            json!({"baseCapacity": 1}),
            MergedOutcome::Accepted,
        ),
        (
            "baseCapacity at the STORED maxCapacity",
            json!({"baseCapacity": 10}),
            MergedOutcome::Accepted,
        ),
        (
            "maxCapacity equal to the STORED baseCapacity",
            json!({"scalingConfiguration": {"maxCapacity": 4}}),
            MergedOutcome::Accepted,
        ),
        (
            "an attribute-driven compute type carrying its configuration",
            json!({
                "computeType": "ATTRIBUTE_BASED_COMPUTE",
                "computeConfiguration": {"vCpu": 2, "memory": 4, "disk": 32},
            }),
            MergedOutcome::Accepted,
        ),
    ];

    for (index, (case, request, outcome)) in cases.into_iter().enumerate() {
        let name = format!("merged-{index:02}");
        let (arn, created) = seeded_fleet(&router, &name).await;

        let (status, body) = update_fleet(&router, &arn, request).await;
        if matches!(outcome, MergedOutcome::Accepted) {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        match outcome {
            MergedOutcome::Accepted => unreachable!("handled above"),
            MergedOutcome::BelowFloor => assert_eq!(
                message,
                state::capacity_below_floor_message(),
                "case {case:?}: expected the capacity-floor refusal, got {message:?}"
            ),
            MergedOutcome::NamesMember(member) => assert!(
                message.contains(member),
                "case {case:?}: the refusal must name {member:?}, got {message:?}"
            ),
        }

        // A refused update leaves the fleet exactly as it was.
        let fetched = fetch_fleet(&router, &arn).await;
        for member in all_members(&created, &fetched) {
            // `status` is the one member the CLOCK moves without any call.
            if member == "status" {
                continue;
            }
            assert_eq!(
                fetched[&member], created[&member],
                "case {case:?}: a refused update must leave {member} as stored, got {fetched}"
            );
        }
    }
}

/// RULE (invariant): a fleet's machines are provisioned, so the settings an update stored
/// are not in force when it answers: the fleet reports `UPDATING` and the move back to
/// `ACTIVE` is owed by the CLOCK — materialised from the stored instants when the fleet is
/// read, once, and identically on every later read, without moving `created` or
/// `lastModified`.
#[tokio::test]
async fn an_updated_fleet_is_updating_until_the_clock_returns_it_to_active() {
    let router = emulator();
    let (arn, created) = seeded_fleet(&router, "settle-after-update").await;

    tokio::time::sleep(PAST_THE_FLEET_SETTLE_SPAN).await;
    let active = fetch_fleet(&router, &arn).await;
    assert_eq!(
        active["status"]["statusCode"], "ACTIVE",
        "a provisioned fleet reports ACTIVE before the update, got {active}"
    );

    let updated = update_ok(&router, "an updated fleet", &arn, json!({"baseCapacity": 7})).await;
    assert_eq!(
        updated["status"]["statusCode"], "UPDATING",
        "the fleet an update answers with is still applying the change, got {updated}"
    );

    // Inside the span the caller still sees the transient status.
    let early = fetch_fleet(&router, &arn).await;
    assert_eq!(
        early["status"]["statusCode"], "UPDATING",
        "a fleet read inside the update span is still UPDATING, got {early}"
    );

    tokio::time::sleep(PAST_THE_FLEET_SETTLE_SPAN).await;
    let settled = fetch_fleet(&router, &arn).await;
    assert_eq!(
        settled["status"]["statusCode"], "ACTIVE",
        "an updated fleet returns to ACTIVE, got {settled}"
    );
    assert!(
        settled["status"]["context"].is_null(),
        "an ACTIVE fleet carries no failure context, got {settled}"
    );
    assert_eq!(
        settled["baseCapacity"], json!(7),
        "settling must not undo what the update stored, got {settled}"
    );
    assert_eq!(
        settled["created"], created["created"],
        "an update never moves the creation instant, got {settled}"
    );
    assert_eq!(
        settled["lastModified"], updated["lastModified"],
        "settling is not a modification of the fleet, got {settled}"
    );

    // Read again: identical, because it was computed once from the stored instants.
    let reread = fetch_fleet(&router, &arn).await;
    assert_eq!(
        reread, settled,
        "a settled fleet must never be re-fabricated per call, got {reread}"
    );
}

/// RULE (invariant): `lastModified` is strictly increasing across the successful updates
/// of one fleet, while its identity — name, id, ARN and `created` — never moves, whatever
/// the update carried.
#[tokio::test]
async fn repeated_updates_keep_the_fleet_identity_and_advance_last_modified() {
    let router = emulator();
    let (arn, created) = seeded_fleet(&router, "repeated-updates").await;

    let requests = [
        json!({}),
        json!({"baseCapacity": 2}),
        json!({"overflowBehavior": "ON_DEMAND"}),
        json!({"tags": []}),
        json!({"imageId": "ami-00000000000000003"}),
    ];

    let mut previous = created.clone();
    for (index, request) in requests.iter().enumerate() {
        let case = format!("update {index}");
        let updated = update_ok(&router, &case, &arn, request.clone()).await;
        for member in ["name", "id", "arn", "created"] {
            assert_eq!(
                updated[member], created[member],
                "case {case:?}: {member} must never move, got {updated}"
            );
        }
        assert!(
            instant(&updated, "lastModified") > instant(&previous, "lastModified"),
            "case {case:?}: lastModified must advance past {}, got {}",
            instant(&previous, "lastModified"),
            instant(&updated, "lastModified")
        );
        previous = updated;
    }
}
