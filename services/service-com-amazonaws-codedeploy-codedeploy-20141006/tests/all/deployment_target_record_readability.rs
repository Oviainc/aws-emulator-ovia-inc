// SPDX-License-Identifier: CC0-1.0

//! What `GetDeploymentTarget` answers for an id across the window in which the
//! deployment has no per-target record to answer it from, driven over the wire.
//!
//! RULE: a read of ONE deployment target is keyed on the per-target RECORD and
//! on nothing else — while the deployment holds no record readable under the
//! requested id, whether because it reports no target under that id at all or
//! because it has not resolved its targets yet, the call is refused HTTP 400 /
//! `DeploymentTargetDoesNotExistException` naming the target by its ARN and the
//! deployment by its id; once the record is readable the same call answers it
//! with HTTP 200. The deployment's own progress selects no other refusal code
//! for this read, so one code covers the whole not-yet-readable window and a
//! client polling a target retries on that code and stops on the answer.
//!
//! The suites are TABLES: the rows span the compute platforms (so the id forms
//! and the target types), the number of targets a group resolves (none, one,
//! several), whether the id asked for is a target of the deployment at all, and
//! the deployment's own progress.
//!
//! The harness is [`crate::codedeploy_lifecycle`]'s and the fleet helpers are
//! [`crate::list_deployment_targets`]', so every case goes through the same
//! front door a client uses.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};
use crate::deployment_targets::{carried, deployable};
use crate::list_deployment_targets::{deploy, force_status, group_with, register};

/// The region an ARN carries when the request carried no credential scope to
/// take one from, which is the emulator's documented default.
const DEFAULT_REGION: &str = "us-east-1";

/// How this read's refusal identifies the target it names.
#[derive(Debug)]
enum Identifier {
    /// The ARN the on-premises registry minted for the instance, which is what
    /// `GetOnPremisesInstance` reports for it.
    Registered,
    /// The ARN the id's own resource form gives it in the emulator's account
    /// and the region the request was signed for. The forms are the ones
    /// `super::refusal_request_derived_identifiers` pins per platform.
    Derived {
        service: &'static str,
        resource: &'static str,
    },
    /// The id as the request sent it: no ARN follows from the id alone.
    AsSent,
}

/// One id to ask for, and what the refusal must name it by while no record of
/// it is readable.
struct Asked<'a> {
    target_id: &'a str,
    identifier: Identifier,
}

/// One row: a deployment group of `platform` selecting `fleet`, the id the
/// deployment resolves a target under (`None` for a group that resolves no
/// nameable target), and ids of the same platform's form that name no target of
/// it.
struct Case<'a> {
    case: &'a str,
    application: &'a str,
    platform: &'a str,
    extras: Value,
    /// On-premises instances to register, each tagged `Fleet=deployable`.
    fleet: &'a [&'a str],
    member: Option<Asked<'a>>,
    outsiders: &'a [Asked<'a>],
}

/// The ARN the on-premises registry minted for `instance`.
async fn registered_arn(emulator: &Emulator, instance: &str) -> String {
    emulator
        .ok("GetOnPremisesInstance", json!({"instanceName": instance}))
        .await["instanceInfo"]["instanceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The sentence the refusal must carry for `asked` against `deployment_id`.
async fn expected_message(emulator: &Emulator, asked: &Asked<'_>, deployment_id: &str) -> String {
    let identifier = match &asked.identifier {
        Identifier::Registered => registered_arn(emulator, asked.target_id).await,
        Identifier::Derived { service, resource } => format!(
            "arn:aws:{service}:{DEFAULT_REGION}:{}:{resource}",
            engine::emulator_account_id()
        ),
        Identifier::AsSent => asked.target_id.to_string(),
    };
    format!("The target {identifier} is not part of deployment {deployment_id}")
}

/// Read one target, answering the HTTP status and the body.
async fn read(emulator: &Emulator, deployment_id: &str, target_id: &str) -> (u16, Value) {
    emulator
        .call(
            "GetDeploymentTarget",
            json!({"deploymentId": deployment_id, "targetId": target_id}),
        )
        .await
}

/// The `$.message` of a failure body.
fn message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

/// Require that reading `asked` is refused as an id no readable record exists
/// under, and that the refusal names the target as the row says.
async fn assert_unreadable(
    emulator: &Emulator,
    case: &str,
    deployment_id: &str,
    asked: &Asked<'_>,
) {
    let (status, body) = read(emulator, deployment_id, asked.target_id).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "DeploymentTargetDoesNotExistException"),
        "case {case:?}: expected 400 DeploymentTargetDoesNotExistException for an id no \
         readable record exists under, got {status} {body:?}"
    );
    let expected = expected_message(emulator, asked, deployment_id).await;
    assert_eq!(
        message(case, &body),
        expected,
        "case {case:?}: expected the refusal to name the target and the deployment as \
         {expected:?}, got {body:?}"
    );
}

/// An Amazon ECS deployment group's one service, whose target id is the
/// `<clustername>:<servicename>` pair the model documents.
fn ecs_service() -> Value {
    json!({"ecsServices": [{"clusterName": "prod", "serviceName": "web"}]})
}

/// An on-premises selector no registered instance satisfies, so the group
/// resolves no target at all.
fn selects_nothing() -> Value {
    json!({
        "onPremisesInstanceTagFilters": [
            {"Key": "Fleet", "Value": "retired", "Type": "KEY_AND_VALUE"},
        ]
    })
}

/// The rows every suite here walks.
fn cases() -> Vec<Case<'static>> {
    vec![
        Case {
            case: "an EC2/on-premises group resolving one registered instance",
            application: "record-one-app",
            platform: "Server",
            extras: deployable(),
            fleet: &["host-a"],
            member: Some(Asked {
                target_id: "host-a",
                identifier: Identifier::Registered,
            }),
            outsiders: &[
                Asked {
                    // An Amazon EC2 instance id: the reserved form, so no
                    // on-premises registration can hold it.
                    target_id: "i-0123456789abcdef0",
                    identifier: Identifier::Derived {
                        service: "ec2",
                        resource: "instance/i-0123456789abcdef0",
                    },
                },
                Asked {
                    target_id: "host-never-registered",
                    identifier: Identifier::AsSent,
                },
            ],
        },
        Case {
            case: "an EC2/on-premises group resolving several instances",
            application: "record-many-app",
            platform: "Server",
            extras: deployable(),
            fleet: &["host-a", "host-b", "host-c"],
            // A member that is neither the first nor the last the group
            // resolved, so nothing about the answer can depend on position.
            member: Some(Asked {
                target_id: "host-b",
                identifier: Identifier::Registered,
            }),
            outsiders: &[Asked {
                target_id: "i-12ab34cd",
                identifier: Identifier::Derived {
                    service: "ec2",
                    resource: "instance/i-12ab34cd",
                },
            }],
        },
        Case {
            // The zero-instance group: it resolves no target, so every id is
            // one it holds no record under, however far it has got.
            case: "an EC2/on-premises group whose selector matches no instance",
            application: "record-empty-app",
            platform: "Server",
            extras: selects_nothing(),
            fleet: &["host-a"],
            member: None,
            outsiders: &[Asked {
                target_id: "host-a",
                identifier: Identifier::Registered,
            }],
        },
        Case {
            case: "an Amazon ECS group resolving its one service",
            application: "record-ecs-app",
            platform: "ECS",
            extras: ecs_service(),
            fleet: &[],
            member: Some(Asked {
                target_id: "prod:web",
                identifier: Identifier::Derived {
                    service: "ecs",
                    resource: "service/prod/web",
                },
            }),
            outsiders: &[Asked {
                target_id: "prod:api",
                identifier: Identifier::Derived {
                    service: "ecs",
                    resource: "service/prod/api",
                },
            }],
        },
        Case {
            // A Lambda deployment's target is named inside the revision, which
            // CodeDeploy holds no content of, so no id reaches a record.
            case: "a Lambda group, whose target no id names",
            application: "record-lambda-app",
            platform: "Lambda",
            extras: json!({}),
            fleet: &[],
            member: None,
            outsiders: &[Asked {
                target_id: "orderProcessor",
                identifier: Identifier::Derived {
                    service: "lambda",
                    resource: "function:orderProcessor",
                },
            }],
        },
    ]
}

/// Set a row up and answer its deployment id.
async fn seed(emulator: &Emulator, row: &Case<'_>) -> String {
    for instance in row.fleet {
        register(emulator, instance, &[("Fleet", "deployable")]).await;
    }
    group_with(emulator, row.application, row.platform, &row.extras).await;
    deploy(emulator, row.application).await
}

// ---------------------------------------------------------------------------
// The not-yet-readable window
// ---------------------------------------------------------------------------

/// RULE: while the deployment has resolved no per-target record, EVERY id is
/// refused the same way — a target of the deployment and an id that is a target
/// of nothing get the same code and the same ARN-bearing sentence, so the
/// refusal says nothing about the deployment's progress.
#[tokio::test]
async fn an_id_with_no_readable_record_is_refused_the_one_code() {
    for row in cases() {
        let emulator = Emulator::new();
        let deployment_id = seed(&emulator, &row).await;
        // The deployment is held before the instant it is owed its start, so
        // the window under test does not depend on the machine's timing.
        force_status(&emulator, &deployment_id, "Created", false);

        let mut asked: Vec<&Asked<'_>> = row.outsiders.iter().collect();
        if let Some(member) = row.member.as_ref() {
            asked.push(member);
        }
        for one in &asked {
            let case = format!("{} asked for {:?} unresolved", row.case, one.target_id);
            assert_unreadable(&emulator, &case, &deployment_id, one).await;
            // Reading it again answers the same thing: a read moves nothing.
            assert_unreadable(&emulator, &case, &deployment_id, one).await;
        }

        // The whole point of the one code: a member and an outsider are
        // indistinguishable while neither is readable.
        if let (Some(member), Some(outsider)) = (row.member.as_ref(), row.outsiders.first()) {
            let (_, member_body) = read(&emulator, &deployment_id, member.target_id).await;
            let (_, outsider_body) = read(&emulator, &deployment_id, outsider.target_id).await;
            assert_eq!(
                error_code(&member_body),
                error_code(&outsider_body),
                "case {:?}: expected a member and an outsider to be refused with the same code \
                 while neither is readable, got {member_body:?} and {outsider_body:?}",
                row.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Once the record exists
// ---------------------------------------------------------------------------

/// RULE: once the deployment has resolved its targets the SAME call answers the
/// member's own record with HTTP 200, while an id that is a target of nothing
/// stays refused with the one code — at every progress the deployment reaches
/// after it started.
#[tokio::test]
async fn a_resolved_member_reads_back_while_an_outsider_stays_refused() {
    // The stages differ along the deployment's progress once it has started,
    // over the model's `DeploymentStatus`
    // (`smithy_query(query="shape",shape="DeploymentStatus")`: Created | Queued
    // | InProgress | Baking | Succeeded | Failed | Stopped | Ready), paired
    // with the `TargetStatus` that progress gives a target.
    let stages: &[(&str, &str)] = &[
        ("InProgress", "InProgress"),
        ("Baking", "InProgress"),
        ("Ready", "InProgress"),
        ("Succeeded", "Succeeded"),
        ("Failed", "Failed"),
    ];

    for row in cases() {
        let emulator = Emulator::new();
        let deployment_id = seed(&emulator, &row).await;
        for (status, target_status) in stages {
            force_status(&emulator, &deployment_id, status, true);
            let case = format!("{} at {status}", row.case);

            for outsider in row.outsiders {
                assert_unreadable(&emulator, &case, &deployment_id, outsider).await;
            }

            let Some(member) = row.member.as_ref() else {
                continue;
            };
            let (code, body) = read(&emulator, &deployment_id, member.target_id).await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected the resolved member {:?} to read back, got {code} \
                 {body:?}",
                member.target_id
            );
            let target = &body["deploymentTarget"];
            let carried = carried(&case, target).1;
            assert_eq!(
                carried["targetId"].as_str(),
                Some(member.target_id),
                "case {case:?}: expected the record of the id asked for, got {target:?}"
            );
            assert_eq!(
                carried["deploymentId"].as_str(),
                Some(deployment_id.as_str()),
                "case {case:?}: expected the record to name its deployment, got {target:?}"
            );
            assert_eq!(
                carried["status"].as_str(),
                Some(*target_status),
                "case {case:?}: expected the target status {target_status}, got {target:?}"
            );
        }
    }
}
