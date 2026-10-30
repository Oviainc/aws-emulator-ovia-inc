// SPDX-License-Identifier: CC0-1.0

//! What four CodeDeploy rejection paths answer on the wire, and what a tagging
//! call carrying an EMPTY list of changes answers.
//!
//! RULES under test:
//!
//! * a `…RequiredException` answers a member the request did NOT carry, so a tag
//!   member the request DID carry is a list of tag changes the call applies —
//!   empty included, which applies none and succeeds — and only an omitted tag
//!   member is `TagRequiredException`;
//! * `InvalidArnException` says `Invalid ARN: <value>` — this error class's fixed
//!   prefix followed by the `ResourceArn` as the request sent it — whichever
//!   operation read the ARN;
//! * `InvalidMinimumHealthyHostValueException` states the rule the (type, value)
//!   pair broke, naming the TYPE the request carried and never the value it
//!   rejected;
//! * `InvalidOperationException` for deleting a configuration CodeDeploy ships
//!   is the catalog's one fixed sentence, interpolating no name;
//! * `MultipleIamArnsProvidedException` states the refused registration and the
//!   remedy, interpolating neither ARN nor the instance name.
//!
//! Every suite is a TABLE whose rows differ along what its rule generalises
//! over: the operation that read the member, the resource kind, the ARN's form
//! and length, the minimum-healthy-hosts type and the side of the bound a value
//! falls on, the shipped configuration's compute platform, and the form of each
//! IAM ARN.
//!
//! The harness (`Emulator`, `error_code`) is the lifecycle suite's, so every
//! case goes through the same front door a client uses.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006::state;

use crate::codedeploy_lifecycle::{error_code, seeded_group, Emulator};

/// The region the harness's unsigned calls are served in, which is the one the
/// service builds an ARN with when a request carries no credential scope.
const REGION: &str = "us-east-1";

/// The modelled maximum length of a `ResourceArn`
/// (`smithy_query(query="operation",operation="TagResource")`: `Arn`,
/// length 1..1011), so a row can send a value AT the bound and still be refused
/// by its form rather than by its size.
const MAX_RESOURCE_ARN_LENGTH: usize = 1011;

/// The sentence AWS answers a tagging call that named no tag with, spelled out
/// here rather than read from the service, so a later edit to the service's own
/// wording fails this suite instead of passing it.
const TAG_NOT_SPECIFIED: &str = "A tag was not specified";

/// The sentence AWS answers a deletion aimed at a shipped deployment
/// configuration with. Spelled out, as [`TAG_NOT_SPECIFIED`] is.
const PREDEFINED_UNDELETABLE: &str = "Can not delete a predefined deployment configuration";

/// The sentence AWS answers a registration naming both IAM ARNs with. Spelled
/// out, as [`TAG_NOT_SPECIFIED`] is.
const BOTH_IAM_ARNS: &str = "The on-premises instance could not be registered because both an \
     IAM session ARN and an IAM user ARN were included in the request. Include only one IAM ARN \
     in the request, and then try again.";

/// A `Tags` list entry, as CodeDeploy's `Tag` shape names its members
/// (`smithy_query(query="shape",shape="Tag")`: `Key`, `Value`).
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
}

/// The `$.message` of a failure body: the sentence the refusal carries.
fn message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

/// Call `operation`, require HTTP 400 with `code`, and answer the message.
async fn refused(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    request: Value,
    code: &str,
) -> String {
    let (status, body) = emulator.call(operation, request).await;
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

/// Whether a success body carries no members, which is what an output shape with
/// none serializes to.
fn is_empty_body(body: &Value) -> bool {
    match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

/// An IAM user ARN for `name`: `arn:aws:iam::<account>:user/<name>`.
fn user_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:user/{name}", engine::emulator_account_id())
}

/// An IAM session ARN for `name`:
/// `arn:aws:sts::<account>:assumed-role/<role>/<session>`.
fn session_arn(name: &str) -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/OnPremRole/{name}",
        engine::emulator_account_id()
    )
}

/// Register `name` against an IAM user ARN built from it.
async fn register(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({"instanceName": name, "iamUserArn": user_arn(name)}),
        )
        .await;
}

/// The `(key, value)` tags `ListTagsForResource` reports for `resource_arn`, in
/// report order, requiring the read to succeed.
async fn resource_tags(emulator: &Emulator, resource_arn: &str) -> Vec<(String, String)> {
    let listed = emulator
        .ok("ListTagsForResource", json!({"ResourceArn": resource_arn}))
        .await;
    pairs(&listed["Tags"])
}

/// The `(key, value)` tags `GetOnPremisesInstance` reports for `instance`.
async fn instance_tags(emulator: &Emulator, instance: &str) -> Vec<(String, String)> {
    let info = emulator
        .ok("GetOnPremisesInstance", json!({"instanceName": instance}))
        .await;
    pairs(&info["instanceInfo"]["tags"])
}

/// The `(key, value)` pairs of a reported `TagList`.
fn pairs(tags: &Value) -> Vec<(String, String)> {
    tags.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["Key"].as_str().unwrap_or_default().to_string(),
                entry["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// A tag member the request carries is applied as given; only an omitted one
// is the "a tag was not specified" refusal
// ---------------------------------------------------------------------------

/// RULE: a tag member the request CARRIES is the list of tag changes the call
/// applies, so an empty list applies none: the call succeeds with an empty body
/// and the resource keeps exactly the tags it held.
#[tokio::test]
async fn an_empty_tag_list_applies_no_change_and_succeeds() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "empty-list-app", "Server").await;
    let application_arn = state::application_arn(REGION, "empty-list-app");
    let group_arn = state::deployment_group_arn(REGION, "empty-list-app", &group);
    for resource_arn in [&application_arn, &group_arn] {
        emulator
            .ok(
                "TagResource",
                json!({"ResourceArn": resource_arn, "Tags": [tag("Env", "prod")]}),
            )
            .await;
    }
    for instance in ["empty-list-host", "empty-list-host-2"] {
        register(&emulator, instance).await;
    }
    emulator
        .ok(
            "AddTagsToOnPremisesInstances",
            json!({
                "instanceNames": ["empty-list-host", "empty-list-host-2"],
                "tags": [tag("Fleet", "blue")],
            }),
        )
        .await;

    // Rows differ along the operation whose tag member is empty (both
    // resource-ARN operations and both on-premises ones), the resource kind the
    // call names (application, deployment group, one instance, two), and the
    // member's own name (`Tags`, `TagKeys`, `tags`).
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "an application, no tag to add",
            "TagResource",
            json!({"ResourceArn": application_arn, "Tags": []}),
        ),
        (
            "a deployment group, no tag to add",
            "TagResource",
            json!({"ResourceArn": group_arn, "Tags": []}),
        ),
        (
            "an application, no key to remove",
            "UntagResource",
            json!({"ResourceArn": application_arn, "TagKeys": []}),
        ),
        (
            "a deployment group, no key to remove",
            "UntagResource",
            json!({"ResourceArn": group_arn, "TagKeys": []}),
        ),
        (
            "one on-premises instance, no tag to add",
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": ["empty-list-host"], "tags": []}),
        ),
        (
            "two on-premises instances, no tag to remove",
            "RemoveTagsFromOnPremisesInstances",
            json!({
                "instanceNames": ["empty-list-host", "empty-list-host-2"],
                "tags": [],
            }),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, body) = emulator.call(operation, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} applies the changes it was given, got {status} {body:?}"
        );
        assert!(
            is_empty_body(&body),
            "case {case:?}: {operation} answers an output with no members, got {body:?}"
        );
    }

    // Nothing above changed a tag: every resource still reports exactly what it
    // was given before the empty calls.
    for (case, resource_arn) in [
        ("the application", &application_arn),
        ("the deployment group", &group_arn),
    ] {
        let reported = resource_tags(&emulator, resource_arn).await;
        assert_eq!(
            reported,
            vec![("Env".to_string(), "prod".to_string())],
            "case {case:?}: an empty list changed no tag, got {reported:?}"
        );
    }
    for instance in ["empty-list-host", "empty-list-host-2"] {
        let reported = instance_tags(&emulator, instance).await;
        assert_eq!(
            reported,
            vec![("Fleet".to_string(), "blue".to_string())],
            "case {instance:?}: an empty list changed no tag, got {reported:?}"
        );
    }

    // An empty list is a list of no changes, not a call that skips the rest of
    // the request: the resource the call names is still resolved, so a call
    // naming one the account does not hold is refused as it would be with tags.
    let absent: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an application the account does not hold",
            "TagResource",
            json!({
                "ResourceArn": state::application_arn(REGION, "no-such-app"),
                "Tags": [],
            }),
            "ApplicationDoesNotExistException",
        ),
        (
            "a deployment group the application does not hold",
            "UntagResource",
            json!({
                "ResourceArn": state::deployment_group_arn(
                    REGION,
                    "empty-list-app",
                    "no-such-group",
                ),
                "TagKeys": [],
            }),
            "DeploymentGroupDoesNotExistException",
        ),
        (
            "an instance no registration exists for",
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": ["no-such-host"], "tags": []}),
            "InstanceNotRegisteredException",
        ),
    ];
    for (case, operation, request, code) in absent {
        let _ = refused(&emulator, case, operation, request, code).await;
    }
}

/// RULE: a request that carries NO tag member names no tag at all, which is
/// `TagRequiredException` with that error's own sentence — the one refusal of
/// this class, whichever tagging operation was called.
#[tokio::test]
async fn an_omitted_tag_member_is_the_tag_required_refusal() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "omitted-member-app", "Server").await;
    let application_arn = state::application_arn(REGION, "omitted-member-app");
    let group_arn = state::deployment_group_arn(REGION, "omitted-member-app", &group);
    register(&emulator, "omitted-member-host").await;

    // Rows differ along the operation, the member it omits (`Tags`, `TagKeys`,
    // `tags`) and the resource the call names.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "TagResource without Tags",
            "TagResource",
            json!({"ResourceArn": application_arn}),
        ),
        (
            "TagResource on a deployment group without Tags",
            "TagResource",
            json!({"ResourceArn": group_arn}),
        ),
        (
            "UntagResource without TagKeys",
            "UntagResource",
            json!({"ResourceArn": application_arn}),
        ),
        (
            "AddTagsToOnPremisesInstances without tags",
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": ["omitted-member-host"]}),
        ),
        (
            "RemoveTagsFromOnPremisesInstances without tags",
            "RemoveTagsFromOnPremisesInstances",
            json!({"instanceNames": ["omitted-member-host"]}),
        ),
    ];

    for (case, operation, request) in cases {
        let got = refused(
            &emulator,
            case,
            operation,
            request,
            "TagRequiredException",
        )
        .await;
        assert_eq!(
            got, TAG_NOT_SPECIFIED,
            "case {case:?}: expected {TAG_NOT_SPECIFIED:?}, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// InvalidArnException: the class's fixed prefix and the value as sent
// ---------------------------------------------------------------------------

/// RULE: a `ResourceArn` that is not in the ARN format is refused with
/// `Invalid ARN: <value>` — the fixed prefix of that error class followed by the
/// value exactly as the request sent it — from every operation that reads a
/// `ResourceArn`.
#[tokio::test]
async fn an_unparseable_resource_arn_is_refused_with_the_classs_prefix() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "arn-worded"}))
        .await;

    // A value at the modelled maximum length: the boundary admits it, so the
    // handler refuses it by its FORM and the sentence still quotes it whole.
    let at_length_bound = "n".repeat(MAX_RESOURCE_ARN_LENGTH);
    // Rows differ along WHY the value is not an ARN (not one at all, a URL, too
    // few fields, an empty partition, a known type carrying no identifier), its
    // punctuation and its length.
    let arns: Vec<(&str, String)> = vec![
        ("a bare resource name", "arn-worded".to_string()),
        (
            "a URL",
            "https://codedeploy.example.invalid/applications/arn-worded".to_string(),
        ),
        (
            "an ARN missing its resource part",
            "arn:aws:codedeploy:us-east-1:000000000000".to_string(),
        ),
        (
            "an ARN with an empty partition",
            "arn::codedeploy:us-east-1:000000000000:application:arn-worded".to_string(),
        ),
        (
            "an application ARN carrying no name",
            "arn:aws:codedeploy:us-east-1:000000000000:application:".to_string(),
        ),
        (
            "a deployment group ARN carrying no group name",
            "arn:aws:codedeploy:us-east-1:000000000000:deploymentgroup:arn-worded".to_string(),
        ),
        (
            "a value carrying every punctuation a name admits",
            "a+b=c,d.e@f_g-h".to_string(),
        ),
        ("a value at the modelled length bound", at_length_bound),
    ];

    for (form, resource_arn) in &arns {
        let expected = format!("Invalid ARN: {resource_arn}");
        // Every operation that reads a `ResourceArn` answers the same sentence.
        let requests: Vec<(&str, Value)> = vec![
            (
                "TagResource",
                json!({"ResourceArn": resource_arn, "Tags": [tag("Env", "prod")]}),
            ),
            (
                "UntagResource",
                json!({"ResourceArn": resource_arn, "TagKeys": ["Env"]}),
            ),
            (
                "ListTagsForResource",
                json!({"ResourceArn": resource_arn}),
            ),
        ];
        for (operation, request) in requests {
            let case = format!("{form} on {operation}");
            let got = refused(
                &emulator,
                &case,
                operation,
                request,
                "InvalidArnException",
            )
            .await;
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected:?}, got {got:?}"
            );
        }
    }

    // The rule is about the form of the value, not about tagging: an ARN that IS
    // in the format still reaches the resource it names.
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({
                "ResourceArn": state::application_arn(REGION, "arn-worded"),
                "Tags": [tag("Env", "prod")],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a well-formed application ARN\": expected it to be served, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// InvalidMinimumHealthyHostValueException: the type, never the value
// ---------------------------------------------------------------------------

/// The sentence a `FLEET_PERCENT` value outside the percentage range is refused
/// with, spelled out as [`TAG_NOT_SPECIFIED`] is.
const PERCENT_OUT_OF_RANGE: &str = "The value for the minimum healthy hosts with type of \
     FLEET_PERCENT should be positive and less than 100";

/// The sentence a negative `HOST_COUNT` is refused with: a count of instances
/// carries no upper bound, so the sentence states the end there is.
const COUNT_OUT_OF_RANGE: &str =
    "The value for the minimum healthy hosts with type of HOST_COUNT should be positive";

/// RULE: a `minimumHealthyHosts` value outside what its type can mean is refused
/// with `InvalidMinimumHealthyHostValueException` stating the rule and the TYPE
/// the request named; the rejected value never appears, and a value inside the
/// type's published range is accepted.
#[tokio::test]
async fn an_out_of_range_minimum_healthy_hosts_value_states_its_type_only() {
    let emulator = Emulator::new();

    // The two types `MinimumHealthyHostsType` carries
    // (`smithy_query(query="shape",shape="MinimumHealthyHostsType")`:
    // FLEET_PERCENT | HOST_COUNT) and, per row, a value outside what that type
    // can mean: FLEET_PERCENT is a percentage of the fleet, HOST_COUNT a count
    // of instances. Rows differ along the type, the side of the range the value
    // falls on and how far past it it falls — including one just past each
    // published bound.
    let refusals: &[(&str, &str, i64, &str)] = &[
        (
            "a percentage one past the top bound",
            "FLEET_PERCENT",
            101,
            PERCENT_OUT_OF_RANGE,
        ),
        (
            "a percentage far past the top bound",
            "FLEET_PERCENT",
            10_000,
            PERCENT_OUT_OF_RANGE,
        ),
        (
            "a percentage one below zero",
            "FLEET_PERCENT",
            -1,
            PERCENT_OUT_OF_RANGE,
        ),
        (
            "a negative percentage",
            "FLEET_PERCENT",
            -5,
            PERCENT_OUT_OF_RANGE,
        ),
        (
            "a count one below zero",
            "HOST_COUNT",
            -1,
            COUNT_OUT_OF_RANGE,
        ),
        (
            "a count far below zero",
            "HOST_COUNT",
            -4_096,
            COUNT_OUT_OF_RANGE,
        ),
    ];

    for (index, (case, kind, value, expected)) in refusals.iter().enumerate() {
        let name = format!("out-of-range-{index}");
        let got = refused(
            &emulator,
            case,
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": name,
                "computePlatform": "Server",
                "minimumHealthyHosts": {"type": kind, "value": value},
            }),
            "InvalidMinimumHealthyHostValueException",
        )
        .await;
        assert_eq!(
            got, *expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
        assert!(
            !got.contains(&value.to_string()),
            "case {case:?}: the sentence must not quote the rejected value {value}, got {got:?}"
        );
        // A refused request created nothing.
        let (status, body) = emulator
            .call("GetDeploymentConfig", json!({"deploymentConfigName": name}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "DeploymentConfigDoesNotExistException"),
            "case {case:?}: a refused create stores nothing, got {status} {body:?}"
        );
    }

    // Each published bound, and the value just inside it, is accepted — and the
    // configuration reads back carrying exactly the pair it was created with.
    let accepted: &[(&str, &str, i64)] = &[
        ("the bottom of the percentage range", "FLEET_PERCENT", 0),
        ("one inside the top of it", "FLEET_PERCENT", 99),
        ("the top of the percentage range", "FLEET_PERCENT", 100),
        ("no healthy instance required", "HOST_COUNT", 0),
        ("one healthy instance required", "HOST_COUNT", 1),
        // A count of instances carries no upper bound, which is why its refusal
        // states none.
        ("a count far above any percentage", "HOST_COUNT", 10_000),
    ];
    for (index, (case, kind, value)) in accepted.iter().enumerate() {
        let name = format!("in-range-{index}");
        emulator
            .ok(
                "CreateDeploymentConfig",
                json!({
                    "deploymentConfigName": name,
                    "computePlatform": "Server",
                    "minimumHealthyHosts": {"type": kind, "value": value},
                }),
            )
            .await;
        let read = emulator
            .ok("GetDeploymentConfig", json!({"deploymentConfigName": name}))
            .await;
        let stored = &read["deploymentConfigInfo"]["minimumHealthyHosts"];
        assert_eq!(
            (stored["type"].as_str(), stored["value"].as_i64()),
            (Some(*kind), Some(*value)),
            "case {case:?}: the configuration reports the pair it was created with, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// InvalidOperationException: the shipped catalog's one sentence
// ---------------------------------------------------------------------------

/// RULE: deleting a deployment configuration CodeDeploy itself ships is refused
/// with `InvalidOperationException` carrying one fixed sentence about predefined
/// configurations, naming no configuration, and the configuration stays
/// readable.
#[tokio::test]
async fn deleting_a_shipped_configuration_is_refused_with_one_fixed_sentence() {
    let emulator = Emulator::new();

    // Rows span the shipped catalog's three compute-platform families and both
    // kinds of shipped configuration (a minimum-healthy-hosts one and a
    // traffic-shifting one), as the AWS CodeDeploy User Guide's "deployment
    // configurations" pages list them.
    let shipped = [
        "CodeDeployDefault.OneAtATime",
        "CodeDeployDefault.HalfAtATime",
        "CodeDeployDefault.AllAtOnce",
        "CodeDeployDefault.LambdaAllAtOnce",
        "CodeDeployDefault.LambdaLinear10PercentEvery1Minute",
        "CodeDeployDefault.ECSCanary10Percent5Minutes",
    ];

    for name in shipped {
        let got = refused(
            &emulator,
            name,
            "DeleteDeploymentConfig",
            json!({"deploymentConfigName": name}),
            "InvalidOperationException",
        )
        .await;
        assert_eq!(
            got, PREDEFINED_UNDELETABLE,
            "case {name:?}: expected {PREDEFINED_UNDELETABLE:?}, got {got:?}"
        );
        assert!(
            !got.contains(name),
            "case {name:?}: the sentence must not name the configuration, got {got:?}"
        );
        // The refusal removed nothing.
        let read = emulator
            .ok("GetDeploymentConfig", json!({"deploymentConfigName": name}))
            .await;
        assert_eq!(
            read["deploymentConfigInfo"]["deploymentConfigName"].as_str(),
            Some(name),
            "case {name:?}: a refused delete leaves the configuration readable, got {read:?}"
        );
    }

    // The rule is about the shipped catalog: a configuration a client created is
    // deleted, so nothing above refuses a deletion by itself.
    emulator
        .ok(
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": "client-made-config",
                "computePlatform": "Server",
                "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 1},
            }),
        )
        .await;
    let (status, body) = emulator
        .call(
            "DeleteDeploymentConfig",
            json!({"deploymentConfigName": "client-made-config"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a configuration a client created\": expected it to be deletable, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// MultipleIamArnsProvidedException: the refused action and the remedy
// ---------------------------------------------------------------------------

/// RULE: a registration carrying BOTH an IAM user ARN and an IAM session ARN is
/// refused with `MultipleIamArnsProvidedException` stating that the instance
/// could not be registered because both were included and that only one belongs
/// in the request — one fixed sentence, interpolating neither ARN nor the
/// instance name — and nothing is registered.
#[tokio::test]
async fn a_registration_naming_both_iam_arns_states_the_action_and_the_remedy() {
    let emulator = Emulator::new();

    // Rows differ along the instance name's form and along each ARN's own form:
    // the plain pair, ARNs in another partition and account, a session ARN whose
    // role path is nested, and a pair in which one ARN is not even in the format
    // the member documents — the refusal is that two were given, so the form of
    // either cannot change it.
    let cases: Vec<(&str, &str, String, String)> = vec![
        (
            "a plain pair",
            "both-arns-plain",
            user_arn("op-plain"),
            session_arn("session-plain"),
        ),
        (
            "every punctuation character the instance name admits",
            "both+a=b,c.d@e_f-g",
            user_arn("op-punctuated"),
            session_arn("session-punctuated"),
        ),
        (
            "ARNs in another partition and account",
            "both-arns-elsewhere",
            "arn:aws-cn:iam::111122223333:user/op-cn".to_string(),
            "arn:aws-cn:sts::111122223333:assumed-role/OnPremRole/session-cn".to_string(),
        ),
        (
            "a session ARN carrying a nested role path",
            "both-arns-nested",
            user_arn("op-nested"),
            format!(
                "arn:aws:sts::{}:assumed-role/fleet/eu/OnPremRole/session-nested",
                engine::emulator_account_id()
            ),
        ),
        (
            "a user ARN that is not in the documented form",
            "both-arns-malformed",
            "not-an-arn".to_string(),
            session_arn("session-beside-a-bad-user-arn"),
        ),
    ];

    for (case, instance, user, session) in &cases {
        let got = refused(
            &emulator,
            case,
            "RegisterOnPremisesInstance",
            json!({
                "instanceName": instance,
                "iamUserArn": user,
                "iamSessionArn": session,
            }),
            "MultipleIamArnsProvidedException",
        )
        .await;
        assert_eq!(
            got, BOTH_IAM_ARNS,
            "case {case:?}: expected {BOTH_IAM_ARNS:?}, got {got:?}"
        );
        for quoted in [instance.to_string(), user.clone(), session.clone()] {
            // Checked only for a value long enough to be distinctive: a short
            // one is prose the fixed sentence contains of its own.
            if quoted.chars().count() > 3 {
                assert!(
                    !got.contains(&quoted),
                    "case {case:?}: the sentence must not quote {quoted:?}, got {got:?}"
                );
            }
        }
        // The refusal registered nothing: the name is still unregistered.
        let (status, body) = emulator
            .call("GetOnPremisesInstance", json!({"instanceName": instance}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InstanceNotRegisteredException"),
            "case {case:?}: a refused registration stores nothing, got {status} {body:?}"
        );
    }

    // The rule is about naming BOTH: either one on its own registers the
    // instance, so nothing above refuses a registration by itself.
    let singles: Vec<(&str, &str, Value)> = vec![
        (
            "an IAM user ARN alone",
            "one-arn-user",
            json!({"iamUserArn": user_arn("op-alone")}),
        ),
        (
            "an IAM session ARN alone",
            "one-arn-session",
            json!({"iamSessionArn": session_arn("session-alone")}),
        ),
    ];
    for (case, instance, member) in singles {
        let mut request = json!({"instanceName": instance});
        for (name, value) in member.as_object().expect("the row's member is an object") {
            request[name.as_str()] = value.clone();
        }
        let (status, body) = emulator.call("RegisterOnPremisesInstance", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the registration to be made, got {status} {body:?}"
        );
        let info = emulator
            .ok("GetOnPremisesInstance", json!({"instanceName": instance}))
            .await;
        assert_eq!(
            info["instanceInfo"]["instanceName"].as_str(),
            Some(instance),
            "case {case:?}: the registration reads back, got {info:?}"
        );
    }
}
