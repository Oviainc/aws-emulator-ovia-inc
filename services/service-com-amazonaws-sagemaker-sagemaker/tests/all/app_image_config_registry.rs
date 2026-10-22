// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's AppImageConfig registry:
//! `CreateAppImageConfig` + `DescribeAppImageConfig`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over TABLES of inputs rather than one recorded call: the ARN
//! form for every region and name shape, the create/describe round trip for every
//! optional member, the refusal of a name already in use, and the refusal of a read for
//! a name nothing is stored under.
//!
//! The sibling `inference_deployment_lifecycle` and `model_package_group_lifecycle`
//! suites each declare their own copy of these few request helpers (module-private, as
//! this crate's test suites keep them), so the ones below follow that pattern rather
//! than making one suite's internals the other's dependency.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `target` with a SigV4 credential scope naming `region`.
    async fn call_in(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    /// Call `target` unsigned: no credential scope, so the ARN region falls back to the
    /// emulator default.
    async fn call_unsigned(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", target, body).await
    }

    /// Call `target` and require a 200, returning the parsed output.
    async fn ok(&self, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

/// The constant message SageMaker's `ResourceInUse` carries (the service's own wording,
/// mirrored by `state::RESOURCE_IN_USE_MESSAGE`).
const NAME_IN_USE_MESSAGE: &str = "The ID or Name specified is already in use.";

/// The three optional app-configuration members and the tag list, as the modelled
/// `CreateAppImageConfigRequest` carries them (`smithy_query`
/// operation=CreateAppImageConfig). Every row a round-trip assertion checks is one of
/// these shapes.
const CONFIG_MEMBERS: [&str; 3] = [
    "KernelGatewayImageConfig",
    "JupyterLabAppImageConfig",
    "CodeEditorAppImageConfig",
];

/// `KernelSpecs` is `len 1..5` (`smithy_query` shape=KernelSpecs), so `count` rows span
/// the published bounds.
fn kernel_gateway(count: usize, with_file_system: bool) -> Value {
    let specs: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "Name": format!("kernel{index}"),
                "DisplayName": format!("Kernel {index}"),
            })
        })
        .collect();
    let mut config = json!({ "KernelSpecs": specs });
    if with_file_system {
        config["FileSystemConfig"] = json!({
            "MountPath": "/home/sagemaker-user",
            "DefaultUid": 1000,
            "DefaultGid": 100,
        });
    }
    config
}

/// A `Create` body naming `name` with `extra`'s members merged in.
fn create_body(name: &str, extra: &Value) -> Value {
    let mut body = json!({ "AppImageConfigName": name });
    for (key, value) in extra.as_object().expect("a JSON object of extra members") {
        body[key] = value.clone();
    }
    body
}

/// The round-trip table: rows differ along which app-configuration member is set, the
/// number of kernel specs, whether an EFS mount is configured, how the container runtime
/// is described, and the tag count.
fn round_trip_cases() -> Vec<(&'static str, Value)> {
    vec![
        // All three optional members absent: a name alone is a legal AppImageConfig.
        ("name-only", json!({})),
        (
            "kernel-gateway-one-kernel",
            json!({ "KernelGatewayImageConfig": kernel_gateway(1, false) }),
        ),
        (
            "kernel-gateway-five-kernels-and-efs",
            json!({ "KernelGatewayImageConfig": kernel_gateway(5, true) }),
        ),
        (
            "jupyter-lab-container-config",
            json!({
                "JupyterLabAppImageConfig": {
                    "FileSystemConfig": { "MountPath": "/home/user", "DefaultUid": 0, "DefaultGid": 0 },
                    "ContainerConfig": {
                        "ContainerEntrypoint": ["jupyter-lab"],
                        "ContainerArguments": ["--no-browser", "--port=8888"],
                        "ContainerEnvironmentVariables": { "LANG": "C.UTF-8" },
                    },
                },
            }),
        ),
        (
            "code-editor-empty-config",
            json!({ "CodeEditorAppImageConfig": {} }),
        ),
        (
            "code-editor-with-tags",
            json!({
                "CodeEditorAppImageConfig": {
                    "ContainerConfig": { "ContainerEntrypoint": ["code-editor"] },
                },
                "Tags": [{ "Key": "team", "Value": "ml" }, { "Key": "env", "Value": "test" }],
            }),
        ),
        (
            "every-app-config-member-set",
            json!({
                "KernelGatewayImageConfig": kernel_gateway(2, true),
                "JupyterLabAppImageConfig": {
                    "ContainerConfig": { "ContainerArguments": ["--ip=0.0.0.0"] },
                },
                "CodeEditorAppImageConfig": {
                    "FileSystemConfig": { "MountPath": "/mnt/efs" },
                },
            }),
        ),
    ]
}

/// RULE: the ARN an AppImageConfig create mints is
/// `arn:aws:sagemaker:<signed region>:<account>:app-image-config/<the stored name>`,
/// whatever the name's shape and whichever region the caller signed for (the emulator
/// default when the request names none), and the read reports that same ARN.
#[tokio::test]
async fn minted_app_image_config_arns_name_the_signed_region_and_the_stored_name() {
    // Rows differ along the name's shape: one character, digits, hyphens, and the
    // 63-character modelled maximum (`AppImageConfigName` is `len 0..63`, pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` — smithy_query operation=CreateAppImageConfig).
    let names = [
        "a",
        "Config9",
        "demo-app-image-config-with-hyphens",
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");
            let expected = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:app-image-config/{name}");

            let (status, created) = emulator
                .call_in(region, "CreateAppImageConfig", &create_body(name, &json!({})))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: CreateAppImageConfig, got {status} with {created}"
            );
            assert_eq!(
                str_field(&created, "AppImageConfigArn"),
                expected,
                "case {case}: AppImageConfigArn, got {created:?}"
            );

            let (status, described) = emulator
                .call_in(
                    region,
                    "DescribeAppImageConfig",
                    &json!({ "AppImageConfigName": name }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: DescribeAppImageConfig, got {status} with {described}"
            );
            assert_eq!(
                str_field(&described, "AppImageConfigArn"),
                expected,
                "case {case}: described AppImageConfigArn, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "AppImageConfigName"),
                name,
                "case {case}: described AppImageConfigName, got {described:?}"
            );
        }
    }

    // A credential scope whose region is not a region token cannot enter an ARN (the
    // region component is positional), so the emulator default stands in.
    for malformed in ["us east 1", "US-EAST-1", "us:east/1"] {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call_in(
                malformed,
                "CreateAppImageConfig",
                &create_body("odd-region", &json!({})),
            )
            .await;
        assert_eq!(
            status, 200,
            "case malformed-region={malformed}: CreateAppImageConfig, got {status} {body}"
        );
        assert_eq!(
            str_field(&body, "AppImageConfigArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:app-image-config/odd-region"),
            "case malformed-region={malformed}: AppImageConfigArn must fall back to the \
             default region, got {body:?}"
        );
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call_unsigned(
            "CreateAppImageConfig",
            &create_body("unsigned-config", &json!({})),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: CreateAppImageConfig, got {status} {body}"
    );
    assert_eq!(
        str_field(&body, "AppImageConfigArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:app-image-config/unsigned-config"),
        "case unsigned: AppImageConfigArn, got {body:?}"
    );
}

/// RULE: `DescribeAppImageConfig` replays every member `CreateAppImageConfig` stored —
/// each present app-configuration member exactly as sent, each absent one still absent —
/// under the name and ARN the create reported.
#[tokio::test]
async fn describe_app_image_config_replays_exactly_what_create_stored() {
    for (case, extra) in round_trip_cases() {
        let emulator = Emulator::new();
        let request = create_body(case, &extra);

        let created = emulator.ok("CreateAppImageConfig", &request).await;
        let arn = str_field(&created, "AppImageConfigArn");

        let described = emulator
            .ok(
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": case }),
            )
            .await;

        assert_eq!(
            str_field(&described, "AppImageConfigName"),
            case,
            "case {case}: AppImageConfigName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "AppImageConfigArn"),
            arn,
            "case {case}: the read must report the ARN the create minted, got {described:?}"
        );

        for member in CONFIG_MEMBERS {
            assert_eq!(
                described.get(member),
                request.get(member),
                "case {case}: {member} must round-trip unchanged, got {described:?}"
            );
        }

        // `Tags` is stored but is not a member of the describe response
        // (smithy_query operation=DescribeAppImageConfig), so it must not appear.
        assert!(
            described.get("Tags").is_none(),
            "case {case}: DescribeAppImageConfig models no Tags member, got {described:?}"
        );

        // A configuration AWS returns ready carries both timestamps, equal until an
        // update moves `LastModifiedTime`. Compared as the JSON the wire carried, so the
        // assertion is about the instant and not about float tolerance.
        let creation = described
            .get("CreationTime")
            .unwrap_or_else(|| panic!("case {case}: expected a CreationTime in {described:?}"));
        let modified = described
            .get("LastModifiedTime")
            .unwrap_or_else(|| panic!("case {case}: expected a LastModifiedTime in {described:?}"));
        assert!(
            creation.as_f64().is_some_and(|seconds| seconds > 0.0),
            "case {case}: CreationTime must be the instant of the create, got {creation:?}"
        );
        assert_eq!(
            creation, modified,
            "case {case}: expected LastModifiedTime == CreationTime {creation:?}, got {modified:?}"
        );

        // A read changes nothing a later caller sees: the second read is byte-identical.
        let again = emulator
            .ok(
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": case }),
            )
            .await;
        assert_eq!(
            again, described,
            "case {case}: a second read must report the same record, got {again:?}"
        );
    }
}

/// RULE: an AppImageConfig name is unique to the account, so a create for a name already
/// stored is refused with `ResourceInUse` (HTTP 400) and leaves the stored record exactly
/// as the first create left it.
#[tokio::test]
async fn creating_an_app_image_config_whose_name_is_taken_is_refused_as_resource_in_use() {
    // Rows differ along what the second create sends: the same body, a different app
    // configuration member, a different kernel count, and an empty one. None may land.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "identical-body",
            json!({ "KernelGatewayImageConfig": kernel_gateway(1, false) }),
            json!({ "KernelGatewayImageConfig": kernel_gateway(1, false) }),
        ),
        (
            "different-member",
            json!({ "KernelGatewayImageConfig": kernel_gateway(2, true) }),
            json!({ "CodeEditorAppImageConfig": {} }),
        ),
        (
            "different-kernel-count",
            json!({ "KernelGatewayImageConfig": kernel_gateway(5, false) }),
            json!({ "KernelGatewayImageConfig": kernel_gateway(3, false) }),
        ),
        (
            "name-only-second",
            json!({ "JupyterLabAppImageConfig": { "ContainerConfig": { "ContainerArguments": ["--x"] } } }),
            json!({}),
        ),
        (
            "name-only-first",
            json!({}),
            json!({ "KernelGatewayImageConfig": kernel_gateway(1, true) }),
        ),
    ];

    for (case, first, second) in cases {
        let emulator = Emulator::new();
        let name = "taken-name";
        let first_request = create_body(name, &first);

        emulator.ok("CreateAppImageConfig", &first_request).await;
        let before = emulator
            .ok(
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": name }),
            )
            .await;

        let (status, refusal) = emulator
            .call("CreateAppImageConfig", &create_body(name, &second))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for a name already in use, got {status} with {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ResourceInUse",
            "case {case}: expected the modelled ResourceInUse code, got {refusal:?}"
        );
        // SageMaker's `ResourceInUse` message is a constant of the service, identical for
        // every resource kind and name and repeating nothing from the request.
        assert_eq!(
            message(&refusal),
            NAME_IN_USE_MESSAGE,
            "case {case}: ResourceInUse message, got {refusal:?}"
        );

        // The refused create wrote nothing: the first record survives untouched.
        let after = emulator
            .ok(
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": name }),
            )
            .await;
        assert_eq!(
            after, before,
            "case {case}: a refused create must not overwrite the stored record, got {after:?}"
        );
    }
}

/// RULE: reading an AppImageConfig under a name nothing is stored under is refused with
/// `ResourceNotFound` (HTTP 400), never answered with an empty 200 — including when other
/// configurations exist and when one exists under a name differing only in case.
#[tokio::test]
async fn describing_an_unknown_app_image_config_is_refused_as_resource_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateAppImageConfig", &create_body("present", &json!({})))
        .await;
    emulator
        .ok(
            "CreateAppImageConfig",
            &create_body(
                "AlsoPresent",
                &json!({ "KernelGatewayImageConfig": kernel_gateway(1, false) }),
            ),
        )
        .await;

    // Rows differ along how the name misses: never created, a prefix of a stored name, a
    // suffix, and a case variant (names are keyed byte-for-byte).
    for missing in ["absent", "presen", "present2", "PRESENT", "alsopresent"] {
        let (status, refusal) = emulator
            .call(
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": missing }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case missing={missing}: expected HTTP 400, got {status} with {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ResourceNotFound",
            "case missing={missing}: expected the modelled ResourceNotFound code, got {refusal:?}"
        );
        assert!(
            message(&refusal).contains(missing),
            "case missing={missing}: the refusal must name what was looked up, got {:?}",
            message(&refusal)
        );
    }

    // The failed reads changed nothing: both stored configurations still read back.
    for present in ["present", "AlsoPresent"] {
        let described = emulator
            .ok(
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": present }),
            )
            .await;
        assert_eq!(
            str_field(&described, "AppImageConfigName"),
            present,
            "case present={present}: still readable, got {described:?}"
        );
    }
}

/// RULE: AppImageConfigs are independent records keyed by name, so creating several
/// leaves each readable with its own ARN and its own configuration.
#[tokio::test]
async fn app_image_configs_are_independent_records_keyed_by_name() {
    let emulator = Emulator::new();
    let cases = round_trip_cases();

    for (case, extra) in &cases {
        emulator
            .ok("CreateAppImageConfig", &create_body(case, extra))
            .await;
    }

    for (case, extra) in &cases {
        let request = create_body(case, extra);
        let described = emulator
            .ok(
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": case }),
            )
            .await;
        assert_eq!(
            str_field(&described, "AppImageConfigArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:app-image-config/{case}"),
            "case {case}: ARN after {} creates, got {described:?}",
            cases.len()
        );
        for member in CONFIG_MEMBERS {
            assert_eq!(
                described.get(member),
                request.get(member),
                "case {case}: {member} must be this record's own, got {described:?}"
            );
        }
    }
}
