// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's notebook instances:
//! `CreateNotebookInstance` -> `DescribeNotebookInstance`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is this crate's
//! shared one, from `tests/all/inference_deployment_lifecycle.rs`; only the
//! notebook-instance helpers below are local.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! and URL form for every name shape and signed region; the create/describe round trip
//! for every optional member the model declares; the documented default for every
//! member a create may omit; the `Pending` -> `InService` transition the clock owes; and
//! the refusals (unknown name, name already in use, `Disabled` internet access without a
//! subnet, and each modelled bound).

use std::time::Duration;

use serde_json::{json, Value};

// The harness this crate's suites share, rather than a fourth copy of it.
use super::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::NOTEBOOK_INSTANCE_IN_SERVICE_AFTER` (300 ms).
pub(crate) const PAST_IN_SERVICE_DUE: Duration = Duration::from_millis(600);

/// A valid `InstanceType` for a notebook instance (from the modelled `InstanceType`
/// enum, `smithy_query shape=InstanceType`).
pub(crate) const SMALL_INSTANCE: &str = "ml.t3.medium";

/// A role ARN satisfying `RoleArn`'s modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
pub(crate) const ROLE: &str = "arn:aws:iam::000000000000:role/SageMakerNotebookRole";

/// Create a notebook instance named `name` and wait out the span the clock owes its
/// `Pending` -> `InService` transition, so the reader reports a provisioned instance.
async fn create_in_service(emulator: &Emulator, name: &str, extra: &Value) -> Value {
    let created = emulator
        .ok("CreateNotebookInstance", &create_body(name, extra))
        .await;
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
    created
}

/// `DescribeNotebookInstance` for `name`, requiring a 200.
pub(crate) async fn describe(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok(
            "DescribeNotebookInstance",
            &json!({ "NotebookInstanceName": name }),
        )
        .await
}

/// A `CreateNotebookInstance` body: the three required members plus whatever `extra`
/// carries.
pub(crate) fn create_body(name: &str, extra: &Value) -> Value {
    let mut body = json!({
        "NotebookInstanceName": name,
        "InstanceType": SMALL_INSTANCE,
        "RoleArn": ROLE,
    });
    for (key, value) in extra
        .as_object()
        .expect("a JSON object of extra members")
        .iter()
    {
        body[key] = value.clone();
    }
    body
}

/// RULE: a notebook instance's ARN is
/// `arn:aws:sagemaker:<signed region>:<account>:notebook-instance/<name>` and its `Url`
/// is `<name>.notebook.<signed region>.sagemaker.aws`, whatever the name's shape and
/// whatever region the caller signed for (the emulator default when unsigned).
#[tokio::test]
async fn a_notebook_instances_arn_and_url_name_the_signed_region_and_the_stored_name() {
    // Rows differ along the axes the rule generalises over: the name's shape (one
    // character, digits, hyphens, and the 63-character modelled maximum) and the region.
    // `NotebookInstanceName` is `len 0..63` (smithy_query
    // operation=CreateNotebookInstance), so the last row sits ON the upper bound.
    let longest = format!("{}b", "a".repeat(62));
    let names = [
        "n",
        "nb1",
        "demo-notebook-with-hyphens",
        longest.as_str(),
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");

            let (status, created) = emulator
                .call_in(region, "CreateNotebookInstance", &create_body(name, &json!({})))
                .await;
            assert_eq!(status, 200, "case {case}: CreateNotebookInstance {created}");
            let expected_arn =
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:notebook-instance/{name}");
            assert_eq!(
                str_field(&created, "NotebookInstanceArn"),
                expected_arn,
                "case {case}: NotebookInstanceArn, got {created:?}"
            );

            tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
            let described = describe(&emulator, name).await;
            assert_eq!(
                str_field(&described, "NotebookInstanceArn"),
                expected_arn,
                "case {case}: DescribeNotebookInstance must replay the minted ARN, got \
                 {described:?}"
            );
            assert_eq!(
                str_field(&described, "Url"),
                format!("{name}.notebook.{region}.sagemaker.aws"),
                "case {case}: Url, got {described:?}"
            );
        }
    }

    // An unsigned request names no region, so both the ARN and the URL carry the
    // emulator's default.
    let emulator = Emulator::new();
    let (status, created) = emulator
        .call_unsigned(
            "CreateNotebookInstance",
            &create_body("unsigned-nb", &json!({})),
        )
        .await;
    assert_eq!(status, 200, "case unsigned: CreateNotebookInstance {created}");
    assert_eq!(
        str_field(&created, "NotebookInstanceArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:notebook-instance/unsigned-nb"),
        "case unsigned: NotebookInstanceArn, got {created:?}"
    );
    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;
    let (status, described) = emulator
        .call_unsigned(
            "DescribeNotebookInstance",
            &json!({ "NotebookInstanceName": "unsigned-nb" }),
        )
        .await;
    assert_eq!(status, 200, "case unsigned: DescribeNotebookInstance {described}");
    assert_eq!(
        str_field(&described, "Url"),
        "unsigned-nb.notebook.us-east-1.sagemaker.aws",
        "case unsigned: Url must fall back to the default region, got {described:?}"
    );
}

/// RULE: `DescribeNotebookInstance` replays every member `CreateNotebookInstance`
/// stored, under the output member name the model gives it, and a second read answers
/// identically.
#[tokio::test]
async fn describe_notebook_instance_replays_exactly_what_create_stored() {
    // Rows differ along the optional members a create may carry: none, VPC placement,
    // code repositories (one default plus the modelled maximum of three additional),
    // encryption, the no-longer-supported accelerator list, IMDS version, platform, and
    // the volume size. The pairs are (request member, output member) — SageMaker renames
    // two of them on the way out.
    let cases: Vec<(&str, Value)> = vec![
        ("required-members-only", json!({})),
        (
            "vpc-placement",
            json!({
                "SubnetId": "subnet-0abc1234",
                "SecurityGroupIds": ["sg-00000001", "sg-00000002"],
                "DirectInternetAccess": "Disabled",
                "IpAddressType": "dualstack",
            }),
        ),
        (
            "code-repositories",
            json!({
                "DefaultCodeRepository": "https://git-codecommit.us-east-1.amazonaws.com/v1/repos/main",
                "AdditionalCodeRepositories": [
                    "https://github.com/aws/amazon-sagemaker-examples",
                    "extra-repo-two",
                    "extra-repo-three",
                ],
            }),
        ),
        (
            "encryption-and-root-access",
            json!({
                "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
                "RootAccess": "Disabled",
                "VolumeSizeInGB": 120,
                "PlatformIdentifier": "notebook-al2-v2",
            }),
        ),
        (
            "accelerators-imds-and-tags",
            json!({
                // From the modelled `NotebookInstanceAcceleratorType` enum.
                "AcceleratorTypes": ["ml.eia2.medium", "ml.eia1.large"],
                "InstanceMetadataServiceConfiguration": {
                    "MinimumInstanceMetadataServiceVersion": "2",
                },
                "Tags": [{ "Key": "team", "Value": "ml" }, { "Key": "env", "Value": "test" }],
                "LifecycleConfigName": "install-packages",
            }),
        ),
    ];

    // (request member, output member) for every member that round-trips.
    let echoed = [
        ("InstanceType", "InstanceType"),
        ("RoleArn", "RoleArn"),
        ("SubnetId", "SubnetId"),
        ("SecurityGroupIds", "SecurityGroups"),
        ("IpAddressType", "IpAddressType"),
        ("DirectInternetAccess", "DirectInternetAccess"),
        ("KmsKeyId", "KmsKeyId"),
        ("RootAccess", "RootAccess"),
        ("VolumeSizeInGB", "VolumeSizeInGB"),
        ("PlatformIdentifier", "PlatformIdentifier"),
        ("AcceleratorTypes", "AcceleratorTypes"),
        ("DefaultCodeRepository", "DefaultCodeRepository"),
        ("AdditionalCodeRepositories", "AdditionalCodeRepositories"),
        (
            "InstanceMetadataServiceConfiguration",
            "InstanceMetadataServiceConfiguration",
        ),
        ("LifecycleConfigName", "NotebookInstanceLifecycleConfigName"),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        let request = create_body(case, &extra);
        let created = create_in_service(&emulator, case, &extra).await;
        let described = describe(&emulator, case).await;

        assert_eq!(
            str_field(&described, "NotebookInstanceName"),
            case,
            "case {case}: NotebookInstanceName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "NotebookInstanceArn"),
            str_field(&created, "NotebookInstanceArn"),
            "case {case}: the reader must replay the ARN the create minted, got \
             {described:?}"
        );
        assert!(
            described.get("CreationTime").is_some_and(Value::is_number),
            "case {case}: CreationTime must be a stored timestamp, got {described:?}"
        );

        for (sent, reported) in echoed {
            if let Some(expected) = request.get(sent) {
                assert_eq!(
                    described.get(reported),
                    Some(expected),
                    "case {case}: {reported} must replay the {sent} the create carried, \
                     got {described:?}"
                );
            }
        }

        // A list member the create never carried is omitted rather than reported empty,
        // and `Tags` is stored but is not part of this operation's modelled output.
        for absent in ["SecurityGroups", "AcceleratorTypes", "AdditionalCodeRepositories"] {
            let sent = match absent {
                "SecurityGroups" => "SecurityGroupIds",
                other => other,
            };
            if request.get(sent).is_none() {
                assert!(
                    described.get(absent).is_none(),
                    "case {case}: {absent} must be omitted when the create carried none, \
                     got {described:?}"
                );
            }
        }
        assert!(
            described.get("Tags").is_none(),
            "case {case}: Tags is not a modelled output member, got {described:?}"
        );

        // A second read sees the same values: a read changes nothing.
        let again = describe(&emulator, case).await;
        assert_eq!(
            again, described,
            "case {case}: a second DescribeNotebookInstance must answer identically"
        );
    }
}

/// RULE: a member a create omits comes back at the default the model documents for it,
/// never absent and never at some other value.
#[tokio::test]
async fn omitted_members_come_back_at_their_documented_defaults() {
    let emulator = Emulator::new();
    create_in_service(&emulator, "defaults", &json!({})).await;
    let described = describe(&emulator, "defaults").await;

    // One row per documented default (the member docs quoted on the `DEFAULT_*`
    // constants in `src/state.rs`).
    let defaults = [
        ("IpAddressType", json!("ipv4")),
        ("DirectInternetAccess", json!("Enabled")),
        ("RootAccess", json!("Enabled")),
        ("PlatformIdentifier", json!("notebook-al2023-v1")),
        ("VolumeSizeInGB", json!(5)),
        (
            "InstanceMetadataServiceConfiguration",
            json!({ "MinimumInstanceMetadataServiceVersion": "1" }),
        ),
    ];
    for (member, expected) in defaults {
        assert_eq!(
            described.get(member),
            Some(&expected),
            "case default-{member}: expected {expected}, got {described:?}"
        );
    }

    // An explicit value overrides every one of them, so the default is a fallback and
    // not a hardcoded answer. The VPC placement carries a security group, which a
    // placement in a subnet is refused without.
    let explicit = json!({
        "IpAddressType": "dualstack",
        "SubnetId": "subnet-0abc1234",
        "SecurityGroupIds": ["sg-00000001"],
        "DirectInternetAccess": "Disabled",
        "RootAccess": "Disabled",
        "PlatformIdentifier": "notebook-al2-v1",
        "VolumeSizeInGB": 30,
        "InstanceMetadataServiceConfiguration": {
            "MinimumInstanceMetadataServiceVersion": "2",
        },
    });
    create_in_service(&emulator, "explicit", &explicit).await;
    let described = describe(&emulator, "explicit").await;
    for (member, value) in explicit.as_object().expect("an object of members") {
        // `SecurityGroupIds` is the one member SageMaker renames on the way out
        // (`SecurityGroups`), which the round-trip case above covers.
        if member == "SecurityGroupIds" {
            continue;
        }
        assert_eq!(
            described.get(member),
            Some(value),
            "case explicit-{member}: expected {value}, got {described:?}"
        );
    }
}

/// RULE: a fresh notebook instance is `Pending` and reports no Jupyter URL or network
/// interface; once the span the clock owes has passed it is `InService` and reports
/// both, with `LastModifiedTime` moved to the instant the transition was due and
/// `CreationTime` left where it was.
#[tokio::test]
async fn a_notebook_instance_advances_from_pending_to_in_service_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateNotebookInstance", &create_body("waiter", &json!({})))
        .await;

    let pending = describe(&emulator, "waiter").await;
    assert_eq!(
        str_field(&pending, "NotebookInstanceStatus"),
        "Pending",
        "case immediately-after-create: NotebookInstanceStatus, got {pending:?}"
    );
    for member in ["Url", "NetworkInterfaceId"] {
        assert!(
            pending.get(member).is_none(),
            "case immediately-after-create: {member} must be absent while the instance \
             is still provisioning, got {pending:?}"
        );
    }

    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

    let settled = describe(&emulator, "waiter").await;
    assert_eq!(
        str_field(&settled, "NotebookInstanceStatus"),
        "InService",
        "case after-the-due-span: NotebookInstanceStatus, got {settled:?}"
    );
    let created_at = settled
        .get("CreationTime")
        .and_then(Value::as_f64)
        .expect("CreationTime");
    let modified_at = settled
        .get("LastModifiedTime")
        .and_then(Value::as_f64)
        .expect("LastModifiedTime");
    assert!(
        modified_at > created_at,
        "case after-the-due-span: LastModifiedTime {modified_at} must move past \
         CreationTime {created_at} when the status changes"
    );
    assert_eq!(
        settled.get("CreationTime"),
        pending.get("CreationTime"),
        "case after-the-due-span: CreationTime must not move, got {settled:?}"
    );

    // The network interface the create minted: `eni-` plus 17 hex digits, stable across
    // reads.
    let eni = str_field(&settled, "NetworkInterfaceId");
    let digits = eni.strip_prefix("eni-").unwrap_or_default();
    assert!(
        digits.len() == 17 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "case after-the-due-span: NetworkInterfaceId must be `eni-` plus 17 hex digits, \
         got {eni:?}"
    );

    let again = describe(&emulator, "waiter").await;
    assert_eq!(
        again, settled,
        "case repeated-read: a settled notebook instance must read back identically"
    );

    // Two instances of one emulator get two different network interfaces.
    create_in_service(&emulator, "sibling", &json!({})).await;
    let sibling = describe(&emulator, "sibling").await;
    assert_ne!(
        str_field(&sibling, "NetworkInterfaceId"),
        eni,
        "case two-instances: each instance gets its own network interface, got \
         {sibling:?}"
    );
}

/// RULE: a reader resolving a `NotebookInstanceName` nothing created is refused with a
/// 400 `ValidationException` naming the ARN it could not find.
#[tokio::test]
async fn an_absent_notebook_instance_is_refused_with_a_400_validation_exception() {
    let emulator = Emulator::new();
    // Rows: a name nothing ever created, and the name of an instance that was never
    // created in THIS emulator but is a legal name of every shape.
    for name in ["absent", "a", "never-created-notebook"] {
        let (status, parsed) = emulator
            .call(
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name}: expected HTTP 400 for an absent notebook instance, got {status} \
             {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {name}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(&format!("{ACCOUNT}:notebook-instance/{name}")),
            "case {name}: the refusal must name the notebook instance it could not find, \
             got {parsed:?}"
        );
    }
}

/// RULE: a create whose name is already in use is refused, so the stored instance is
/// never silently overwritten.
#[tokio::test]
async fn creating_a_name_already_in_use_is_refused_and_keeps_the_stored_instance() {
    let emulator = Emulator::new();
    create_in_service(&emulator, "taken", &json!({ "VolumeSizeInGB": 50 })).await;
    let before = describe(&emulator, "taken").await;

    // Rows: a second create differing in each member it could change.
    let attempts = [
        json!({}),
        json!({ "VolumeSizeInGB": 400 }),
        json!({ "RootAccess": "Disabled" }),
    ];
    for attempt in attempts {
        let (status, parsed) = emulator
            .call("CreateNotebookInstance", &create_body("taken", &attempt))
            .await;
        assert_eq!(
            status, 400,
            "case {attempt}: a duplicate create must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {attempt}: error code, got {parsed:?}"
        );
        let after = describe(&emulator, "taken").await;
        assert_eq!(
            after, before,
            "case {attempt}: the refused create must leave the stored instance untouched"
        );
    }
}

/// RULE: `DirectInternetAccess` may be `Disabled` only alongside a `SubnetId` (the
/// member's own documentation), so the pairing decides the outcome and nothing else does.
#[tokio::test]
async fn direct_internet_access_disabled_is_refused_without_a_subnet() {
    // Rows: every combination of the two members the rule relates. A row that names a
    // subnet carries a security group too, which a VPC placement is refused without.
    let cases = [
        ("disabled-without-subnet", json!({ "DirectInternetAccess": "Disabled" }), false),
        (
            "disabled-with-subnet",
            json!({
                "DirectInternetAccess": "Disabled",
                "SubnetId": "subnet-0abc1234",
                "SecurityGroupIds": ["sg-00000001"],
            }),
            true,
        ),
        ("enabled-without-subnet", json!({ "DirectInternetAccess": "Enabled" }), true),
        (
            "enabled-with-subnet",
            json!({
                "DirectInternetAccess": "Enabled",
                "SubnetId": "subnet-0abc1234",
                "SecurityGroupIds": ["sg-00000001"],
            }),
            true,
        ),
        ("absent-without-subnet", json!({}), true),
    ];

    for (case, extra, accepted) in cases {
        let emulator = Emulator::new();
        let (status, parsed) = emulator
            .call("CreateNotebookInstance", &create_body(case, &extra))
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} {parsed}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: Disabled internet access without a subnet must be refused, got \
             {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: error code, got {parsed:?}"
        );
        // Nothing was written: the reader still refuses the name.
        let (status, parsed) = emulator
            .call(
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": case }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a refused create must write nothing, got {status} {parsed}"
        );
    }
}

/// RULE: a request outside a member's modelled constraint is refused at the boundary
/// with HTTP 400, before any state is written; a request ON each published bound is
/// accepted.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();

    // Rows: one past each published bound — `NotebookInstanceName` len 0..63 and its
    // pattern, `VolumeSizeInGB` range 5..16384, `AdditionalCodeRepositories` len 0..3,
    // `SecurityGroupIds` len 0..5, and the `InstanceType` enum.
    let too_long = "a".repeat(64);
    let four_repos: Vec<Value> = (0..4).map(|index| json!(format!("repo{index}"))).collect();
    let six_groups: Vec<Value> = (0..6).map(|index| json!(format!("sg-{index}"))).collect();
    let cases: Vec<(&str, Value)> = vec![
        ("name-past-63", json!({ "NotebookInstanceName": too_long })),
        (
            "name-with-an-illegal-character",
            json!({ "NotebookInstanceName": "not a notebook name" }),
        ),
        ("volume-below-5", json!({ "VolumeSizeInGB": 4 })),
        ("volume-above-16384", json!({ "VolumeSizeInGB": 16385 })),
        ("four-additional-repositories", json!({ "AdditionalCodeRepositories": four_repos })),
        ("six-security-groups", json!({ "SecurityGroupIds": six_groups })),
        ("instance-type-outside-the-enum", json!({ "InstanceType": "ml.not.real" })),
        ("role-arn-that-is-not-a-role", json!({ "RoleArn": "not-an-arn" })),
    ];

    for (case, extra) in cases {
        let body = create_body(case, &extra);
        let (status, parsed) = emulator.call("CreateNotebookInstance", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: CreateNotebookInstance must be refused, got {status} {parsed}"
        );
        let (status, parsed) = emulator
            .call(
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": case }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a refused request must write nothing, got {status} {parsed}"
        );
    }

    // ON the bounds: each published extreme is accepted and round-trips.
    let accepted = [
        ("volume-on-the-lower-bound", json!({ "VolumeSizeInGB": 5 })),
        ("volume-on-the-upper-bound", json!({ "VolumeSizeInGB": 16384 })),
        (
            "three-additional-repositories",
            json!({ "AdditionalCodeRepositories": ["one", "two", "three"] }),
        ),
        (
            "five-security-groups",
            json!({ "SecurityGroupIds": ["sg-1", "sg-2", "sg-3", "sg-4", "sg-5"] }),
        ),
    ];
    for (case, extra) in accepted {
        emulator.ok("CreateNotebookInstance", &create_body(case, &extra)).await;
        let described = describe(&emulator, case).await;
        for (member, value) in extra.as_object().expect("an object of members") {
            let reported = match member.as_str() {
                "SecurityGroupIds" => "SecurityGroups",
                other => other,
            };
            assert_eq!(
                described.get(reported),
                Some(value),
                "case {case}: {reported} must round-trip, got {described:?}"
            );
        }
    }
}
