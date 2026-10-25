// SPDX-License-Identifier: CC0-1.0

//! The request-derived IDENTIFIERS two CodeDeploy refusals carry, driven over
//! the wire.
//!
//! RULES under test:
//!
//! * a refusal about a deployment TARGET names the target the way a target is
//!   identified as a resource — by its ARN, resolved from the id the request
//!   carried and the compute platform whose id form it is in, in the region the
//!   request was signed for and the emulator's account — and names it by the ARN
//!   this service HOLDS where it holds one (a registered on-premises instance),
//!   falling back to the id as sent only where no ARN follows from the id alone;
//! * a refusal about one ENTRY of a submitted list names that entry's 1-based
//!   POSITION in the list, beside the value at fault, so a caller knows which
//!   entry to fix when several could be the one.
//!
//! Each suite is a TABLE: the target rows span the compute platforms (so the id
//! forms), both identifier sources, the region the request is signed for, and
//! both readers that model the refusal; the alarm rows span the list length, the
//! position of the offending entry, the name form, and all three operations that
//! accept an alarm configuration.
//!
//! The harness (`Emulator`, `error_code`, `s3_revision`, `seeded_group`) is the
//! lifecycle suite's and the fleet helpers are the target suites', so every case
//! goes through the same front door a client uses.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006::state::MAX_ALARM_NAME_LENGTH;

use crate::codedeploy_lifecycle::{error_code, s3_revision, seeded_group, Emulator};
use crate::deployment_targets::deployable;
use crate::list_deployment_targets::{deploy, group_with, register};

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// The region an ARN carries when the request carried no credential scope to
/// take one from, which is the emulator's documented default.
const DEFAULT_REGION: &str = "us-east-1";

/// The `$.message` of a failure body.
fn message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

/// Call `operation` signed for `region` (or unsigned, for `None`), require HTTP
/// 400 with `code`, and answer the message.
async fn refused(
    emulator: &Emulator,
    case: &str,
    region: Option<&str>,
    operation: &str,
    request: Value,
    code: &str,
) -> String {
    let (status, body) = emulator.call_in_region(region, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        code,
        "case {case:?}: expected {code} from {operation}, got {body:?}"
    );
    message(case, &body)
}

// ---------------------------------------------------------------------------
// A target refusal names the target by its ARN
// ---------------------------------------------------------------------------

/// What the refusal is expected to name the target by, per row.
#[derive(Debug)]
enum Identifier<'a> {
    /// An ARN that follows from the id and its platform's resource form: the AWS
    /// service and the resource part of it, rendered in the region the REQUEST
    /// was signed for.
    Derived { service: &'a str, resource: &'a str },
    /// The ARN the on-premises registry minted for this instance — a record this
    /// service holds, so the reading request's region cannot change it.
    Registered,
    /// The id as the request sent it: no ARN follows from the id alone.
    AsSent,
}

/// One row: the deployment to read against, the id to ask for, and what the
/// refusal must name that target by.
struct Case<'a> {
    case: &'a str,
    deployment: &'a str,
    target_id: &'a str,
    expected: Identifier<'a>,
}

/// The ARN the on-premises registry minted for `instance`, as
/// `GetOnPremisesInstance` reports it.
async fn registered_arn(emulator: &Emulator, instance: &str) -> String {
    emulator
        .ok("GetOnPremisesInstance", json!({"instanceName": instance}))
        .await["instanceInfo"]["instanceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: `DeploymentTargetDoesNotExistException` names the deployment and
/// identifies the target by its ARN — the one the on-premises registry holds for
/// a registered instance, else the one the id's own resource form gives it in
/// the request's region and the emulator's account — and both readers of a
/// single target word it identically.
#[tokio::test]
async fn a_target_refusal_identifies_the_target_by_its_arn() {
    let emulator = Emulator::new();
    // The fleet: one instance the EC2/on-premises group selects (a real target
    // of its deployment) and one registered instance it does not select — a
    // target of no deployment, whose ARN this service nevertheless holds.
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    register(&emulator, "host-spare", &[("Fleet", "spare")]).await;
    group_with(&emulator, "arn-server-app", "Server", &deployable()).await;
    let server = deploy(&emulator, "arn-server-app").await;
    group_with(
        &emulator,
        "arn-ecs-app",
        "ECS",
        &json!({"ecsServices": [{"clusterName": "prod", "serviceName": "web"}]}),
    )
    .await;
    let ecs = deploy(&emulator, "arn-ecs-app").await;
    group_with(&emulator, "arn-lambda-app", "Lambda", &json!({})).await;
    let lambda = deploy(&emulator, "arn-lambda-app").await;
    let spare_arn = registered_arn(&emulator, "host-spare").await;
    assert!(
        !spare_arn.is_empty(),
        "the registry must report an instanceArn for a registered instance, got {spare_arn:?}"
    );

    // Rows differ along the compute platform the id is read against (so along
    // the id FORM and the kind of resource it names), along the source of the
    // ARN (derived from the id, or held by this service), and along the id's own
    // shape: the current 17-hex-digit Amazon EC2 instance id, the older
    // 8-digit one, the reserved prefix in upper case, a cluster/service pair
    // carrying the punctuation a resource name admits, and a host name no
    // registration exists for.
    let cases: &[Case] = &[
        Case {
            case: "an Amazon EC2 instance id no deployment ever targeted",
            deployment: &server,
            target_id: "i-0123456789abcdef0",
            expected: Identifier::Derived {
                service: "ec2",
                resource: "instance/i-0123456789abcdef0",
            },
        },
        Case {
            case: "an Amazon EC2 instance id in the shorter published form",
            deployment: &server,
            target_id: "i-12ab34cd",
            expected: Identifier::Derived {
                service: "ec2",
                resource: "instance/i-12ab34cd",
            },
        },
        Case {
            // The prefix this service RESERVES for instance ids is reserved
            // whatever its case, so an id carrying it in upper case is an
            // Amazon EC2 instance too.
            case: "an Amazon EC2 instance id whose reserved prefix is upper case",
            deployment: &server,
            target_id: "I-0ABCDEF1234567890",
            expected: Identifier::Derived {
                service: "ec2",
                resource: "instance/I-0ABCDEF1234567890",
            },
        },
        Case {
            case: "a registered on-premises instance this deployment does not target",
            deployment: &server,
            target_id: "host-spare",
            expected: Identifier::Registered,
        },
        Case {
            case: "an on-premises host name no registration exists for",
            deployment: &server,
            target_id: "host-never-registered",
            expected: Identifier::AsSent,
        },
        Case {
            case: "an Amazon ECS cluster/service pair this deployment does not target",
            deployment: &ecs,
            target_id: "prod:api",
            expected: Identifier::Derived {
                service: "ecs",
                resource: "service/prod/api",
            },
        },
        Case {
            case: "an Amazon ECS pair carrying the punctuation a resource name admits",
            deployment: &ecs,
            target_id: "staging-1:web_api",
            expected: Identifier::Derived {
                service: "ecs",
                resource: "service/staging-1/web_api",
            },
        },
        Case {
            // A Lambda deployment's one target is named inside the revision,
            // which CodeDeploy holds no content of, so every id asked for is a
            // target this deployment does not hold.
            case: "a Lambda function name, which no id of this deployment reaches",
            deployment: &lambda,
            target_id: "orderProcessor",
            expected: Identifier::Derived {
                service: "lambda",
                resource: "function:orderProcessor",
            },
        },
    ];

    // The region a derived ARN carries is the REQUEST's: both an unsigned call,
    // which gets the emulator's default, and a call whose credential scope names
    // another region.
    for signed_for in [None, Some("eu-west-2")] {
        let region = signed_for.unwrap_or(DEFAULT_REGION);
        for row in cases {
            let case = format!("{} signed for {region}", row.case);
            let identifier = match &row.expected {
                Identifier::Derived { service, resource } => {
                    format!("arn:aws:{service}:{region}:{}:{resource}", account())
                }
                Identifier::Registered => spare_arn.clone(),
                Identifier::AsSent => row.target_id.to_string(),
            };
            let expected = format!(
                "The target {identifier} is not part of deployment {}",
                row.deployment
            );
            // Both readers of a single target model this refusal, and both
            // answer it from the one builder, so neither can word it
            // differently.
            let requests = [
                (
                    "GetDeploymentTarget",
                    json!({"deploymentId": row.deployment, "targetId": row.target_id}),
                ),
                (
                    "BatchGetDeploymentTargets",
                    json!({"deploymentId": row.deployment, "targetIds": [row.target_id]}),
                ),
            ];
            for (operation, request) in requests {
                let got = refused(
                    &emulator,
                    &case,
                    signed_for,
                    operation,
                    request,
                    "DeploymentTargetDoesNotExistException",
                )
                .await;
                assert_eq!(
                    got, expected,
                    "case {case:?} on {operation}: expected {expected:?}, got {got:?}"
                );
            }
        }
    }

    // The ARN a refusal names a REGISTERED instance by is the registry's own
    // string, not one rebuilt per request: it names the registration, so it
    // carries the region that registration was made in whichever region the
    // reading request is signed for.
    assert!(
        spare_arn.contains(DEFAULT_REGION),
        "the registration was made unsigned, so its ARN carries {DEFAULT_REGION}, got \
         {spare_arn:?}"
    );

    // The rule is about an identifier, not about the refusal: an id that DOES
    // name a target of the deployment is still answered, so nothing above turned
    // a readable target into a refusal.
    let answered = emulator
        .call(
            "GetDeploymentTarget",
            json!({"deploymentId": server, "targetId": "host-a"}),
        )
        .await;
    assert!(
        answered.0 == 200 || error_code(&answered.1) == "DeploymentNotStartedException",
        "case \"a target the deployment holds\": expected the target to be readable (or the \
         deployment not to have started yet), got {} {:?}",
        answered.0,
        answered.1
    );
}

// ---------------------------------------------------------------------------
// An alarm refusal names the position of the entry at fault
// ---------------------------------------------------------------------------

/// An `AlarmConfiguration` over `names`, enabled — which is the state the alarm
/// rules apply to.
fn alarm_configuration(names: &[String]) -> Value {
    let alarms: Vec<Value> = names.iter().map(|name| json!({"name": name})).collect();
    json!({"enabled": true, "alarms": alarms})
}

/// RULE: `InvalidAlarmConfigException` for an alarm list naming one alarm twice
/// says `Name <name> of Alarm <position> already exists.` — the repeated name
/// and the 1-based position, in the submitted list, of the entry that repeats it
/// — and every operation that accepts an alarm configuration answers the same
/// sentence.
#[tokio::test]
async fn a_duplicate_alarm_is_refused_naming_the_repeat_and_its_position() {
    let emulator = Emulator::new();
    let application = "alarm-positions";
    let group = seeded_group(&emulator, application, "Server").await;

    // A name of exactly the published maximum
    // (`smithy_query(query="shape",shape="Alarm")`: "Maximum length is 255
    // characters"), so the sentence is pinned for a name at the bound too.
    let longest = "n".repeat(MAX_ALARM_NAME_LENGTH);
    // Ten alarms, the published maximum for a deployment group, whose last entry
    // repeats the first: the position reported is the repeat's, not the
    // original's, however far apart they are.
    let mut ten: Vec<String> = (0..9).map(|index| format!("alarm-{index}")).collect();
    ten.push("alarm-0".to_string());

    // Rows differ along the list LENGTH, the POSITION of the repeat, how many
    // distinct names precede it, whether a name repeats more than twice, and the
    // name's own form.
    let cases: Vec<(&str, Vec<String>, &str, usize)> = vec![
        (
            "a two-entry list whose second entry repeats the first",
            vec!["sim-alarm".to_string(), "sim-alarm".to_string()],
            "sim-alarm",
            2,
        ),
        (
            "a three-entry list whose last entry repeats the first",
            vec!["a".to_string(), "b".to_string(), "a".to_string()],
            "a",
            3,
        ),
        (
            "a four-entry list whose third entry repeats the second",
            vec![
                "first".to_string(),
                "second".to_string(),
                "second".to_string(),
                "third".to_string(),
            ],
            "second",
            3,
        ),
        (
            "a name repeated three times, reported at its FIRST repeat",
            vec!["dup".to_string(), "dup".to_string(), "dup".to_string()],
            "dup",
            2,
        ),
        (
            "a name carrying every punctuation character a resource name admits",
            vec!["a+b=c,d.e@f_g-h".to_string(), "a+b=c,d.e@f_g-h".to_string()],
            "a+b=c,d.e@f_g-h",
            2,
        ),
        (
            "a name of exactly the published maximum length",
            vec![longest.clone(), longest.clone()],
            longest.as_str(),
            2,
        ),
        (
            "ten alarms, the published maximum, whose last repeats the first",
            ten.clone(),
            "alarm-0",
            10,
        ),
    ];

    for (row, names, repeated, position) in &cases {
        let expected = format!("Name {repeated} of Alarm {position} already exists.");
        let configuration = alarm_configuration(names);
        // Every operation that accepts an alarm configuration: the group's own,
        // on creation and on update, and a deployment's override of it.
        let requests: Vec<(&str, Value)> = vec![
            (
                "CreateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": "alarm-positions-new-dg",
                    "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
                    "alarmConfiguration": configuration,
                }),
            ),
            (
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "alarmConfiguration": configuration,
                }),
            ),
            (
                "CreateDeployment",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": group,
                    "revision": s3_revision("my-bucket", "app.zip"),
                    "overrideAlarmConfiguration": configuration,
                }),
            ),
        ];
        for (operation, request) in requests {
            let case = format!("{row} on {operation}");
            let got = refused(
                &emulator,
                &case,
                None,
                operation,
                request,
                "InvalidAlarmConfigException",
            )
            .await;
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected:?}, got {got:?}"
            );
        }
    }

    // A refused update stored nothing: the group still carries the one alarm it
    // was created without, so the position in the message is all the request
    // left behind.
    let stored = emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": group}),
        )
        .await["deploymentGroupInfo"]["alarmConfiguration"]
        .clone();
    assert!(
        stored.is_null(),
        "case \"after the refusals\": the group must still hold the configuration it was created \
         with, got {stored:?}"
    );

    // The alarm list the group DOES accept is the same list with the repeat
    // renamed: what was refused is the repetition and nothing else.
    let accepted = emulator
        .call(
            "UpdateDeploymentGroup",
            json!({
                "applicationName": application,
                "currentDeploymentGroupName": group,
                "alarmConfiguration": alarm_configuration(&[
                    "sim-alarm".to_string(),
                    "sim-alarm-2".to_string(),
                ]),
            }),
        )
        .await;
    assert_eq!(
        accepted.0, 200,
        "case \"the same list with the repeat renamed\": expected it to be accepted, got {} {:?}",
        accepted.0, accepted.1
    );
}

/// RULE (the same one, over the sibling per-entry refusals of the same list): a
/// refusal about one entry of the submitted alarm list names that entry's
/// 1-based position, whichever way that entry is at fault.
#[tokio::test]
async fn an_alarm_entry_refusal_names_the_position_of_the_entry_at_fault() {
    let emulator = Emulator::new();
    let application = "alarm-entry-faults";
    let group = seeded_group(&emulator, application, "Server").await;

    // One character past the published maximum the model's `Alarm$name` states.
    let over_long = "n".repeat(MAX_ALARM_NAME_LENGTH + 1);
    // Rows differ along HOW the entry is at fault (no name at all, a name past
    // the bound) and along its position in the list.
    let cases: Vec<(&str, Vec<String>, String)> = vec![
        (
            "the only entry has no name",
            vec![String::new()],
            "The alarm configuration is invalid: alarm 1 was specified with no name".to_string(),
        ),
        (
            "the second of three entries has no name",
            vec!["ok".to_string(), String::new(), "also-ok".to_string()],
            "The alarm configuration is invalid: alarm 2 was specified with no name".to_string(),
        ),
        (
            "the third entry's name is one character past the bound",
            vec!["ok".to_string(), "also-ok".to_string(), over_long.clone()],
            format!(
                "The alarm configuration is invalid: the name of alarm 3 exceeds the maximum \
                 length of {MAX_ALARM_NAME_LENGTH} characters"
            ),
        ),
        (
            "the first entry's name is past the bound",
            vec![over_long.clone(), "ok".to_string()],
            format!(
                "The alarm configuration is invalid: the name of alarm 1 exceeds the maximum \
                 length of {MAX_ALARM_NAME_LENGTH} characters"
            ),
        ),
    ];

    for (row, names, expected) in &cases {
        let got = refused(
            &emulator,
            row,
            None,
            "UpdateDeploymentGroup",
            json!({
                "applicationName": application,
                "currentDeploymentGroupName": group,
                "alarmConfiguration": alarm_configuration(names),
            }),
            "InvalidAlarmConfigException",
        )
        .await;
        assert_eq!(
            got, *expected,
            "case {row:?}: expected {expected:?}, got {got:?}"
        );
    }
}
