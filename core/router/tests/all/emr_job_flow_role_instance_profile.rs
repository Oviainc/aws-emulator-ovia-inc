// SPDX-License-Identifier: CC0-1.0

//! Amazon EMR resolves a cluster's `JobFlowRole` against IAM before launching it.
//!
//! The rule under test: every Amazon EC2 instance of a cluster assumes the instance
//! profile the launch names — "also called instance profile and Amazon EC2 role" — and
//! Amazon EMR itself acts as the launch's `ServiceRole`, so `RunJobFlow` requires both
//! members and then resolves the instance profile against IAM, writing NO cluster when
//! any of those fails. Three error classes follow:
//!
//! 1. a launch naming none of a role it requires is HTTP 400 `ValidationException`
//!    carrying `<role> is required for creating cluster.`;
//! 2. a `JobFlowRole` naming no instance profile this account holds is HTTP 400
//!    `ValidationException` carrying `Invalid InstanceProfile: <value>.`;
//! 3. a profile that EXISTS and carries no role Amazon EC2 may assume is HTTP 400
//!    `AccessDeniedException` carrying `Failed to authorize instance profile <arn>`,
//!    whose variable part is the ARN IAM holds that profile under.
//!
//! A name IAM holds an EC2-assumable profile under is accepted and round-trips as the
//! cluster's `Ec2InstanceAttributes.IamInstanceProfile`.
//!
//! This suite needs TWO services on one engine — EMR reaches IAM through the
//! in-process callback, so a single-service router has no IAM to ask — which is why
//! it lives here rather than in either service's own crate. The rows span what the
//! rule generalises over: how the profile is named (friendly name, a different
//! casing of it, its ARN, the ARN of a profile under a path), whether anything of
//! that name exists (a profile EC2 may assume, a profile carrying no role, a profile
//! whose role trusts another principal, only a role of the same name, nothing at all),
//! and the requests that name no role at all.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built —
//! and the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The instance profile this account holds, under the default path.
const HELD_PROFILE: &str = "EMR_EC2_DefaultRole";

/// An instance profile created under a path, named by ARN below.
const PATHED_PROFILE: &str = "EMR_EC2_PathedRole";

/// The path [`PATHED_PROFILE`] is created under.
const PROFILE_PATH: &str = "/service-role/";

/// A name this account holds an IAM ROLE under, but no instance profile: EMR
/// resolves the profile, not the role, so a cluster naming it is refused.
const ROLE_ONLY: &str = "EMR_EC2_RoleWithoutProfile";

/// An instance profile this account holds that carries NO role: it exists, and there is
/// nothing in it for an Amazon EC2 instance to assume.
const ROLELESS_PROFILE: &str = "EMR_EC2_RolelessProfile";

/// An instance profile this account holds whose role trusts ANOTHER service principal:
/// it exists, and Amazon EC2 may not assume what it carries.
const FOREIGN_TRUST_PROFILE: &str = "EMR_EC2_LambdaTrustedProfile";

/// The service role every launch below names, which `RunJobFlow` requires too.
const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
/// The signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// An EC2 trust policy, so each role below is one an instance profile may carry.
///
/// Written compactly and with no `&`, `=`, `+` or `%`, so it travels in an
/// `awsQuery` form body as written.
fn ec2_trust_policy() -> String {
    trust_policy("ec2.amazonaws.com")
}

/// A trust policy admitting `principal` and nothing else, in the same compact form.
fn trust_policy(principal: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{principal}\"}},\"Action\":\"sts:AssumeRole\"}}]}}"
    )
}

/// The ARN of an instance profile this account holds under `path`.
fn instance_profile_arn(path: &str, name: &str) -> String {
    format!(
        "arn:aws:iam::{}:instance-profile{path}{name}",
        engine::emulator_account_id()
    )
}

/// Send a request and return the status with the body as text.
fn send(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    target: Option<&str>,
    content_type: &str,
    body: String,
) -> (u16, String) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one `awsQuery` IAM action: `POST /` with the parameters in the form body.
/// Every value here is free of `&`, `=`, `+` and `%`, so it needs no escaping.
fn iam(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    parameters: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push_str(&format!("&{key}={value}"));
    }
    send(
        router,
        runtime,
        "iam",
        None,
        "application/x-www-form-urlencoded",
        body,
    )
}

/// Drive one EMR operation: `POST /` with `X-Amz-Target` and an awsJson1_1 body.
fn emr(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let (status, text) = send(
        router,
        runtime,
        "elasticmapreduce",
        Some(&format!("ElasticMapReduce.{operation}")),
        "application/x-amz-json-1.1",
        body.to_string(),
    );
    let parsed = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// The awsJson1_1 error code, without the namespace the envelope may prefix.
fn error_code(payload: &serde_json::Value) -> String {
    payload
        .get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The error message, whichever casing the envelope carries it under.
fn error_message(payload: &serde_json::Value) -> String {
    payload
        .get("Message")
        .or_else(|| payload.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A minimal `RunJobFlow` body, naming `job_flow_role` when there is one.
fn run_job_flow_body(name: &str, job_flow_role: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "ServiceRole": SERVICE_ROLE,
        "Instances": {
            "InstanceCount": 1,
            "MasterInstanceType": "m5.xlarge",
            "KeepJobFlowAliveWhenNoSteps": true,
        }
    });
    if let Some(role) = job_flow_role {
        body["JobFlowRole"] = serde_json::Value::String(role.to_string());
    }
    body
}

/// One row: the `JobFlowRole` a `RunJobFlow` names, and what the launch is answered
/// with.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// The profile IAM holds and Amazon EC2 may assume, so the cluster is launched.
    Launched,
    /// The request named no instance profile at all.
    Required,
    /// The value names no instance profile this account holds; the value the sentence
    /// echoes.
    Invalid(String),
    /// The named profile could not be authorized; the ARN the refusal must report.
    Unauthorized(String),
}

/// One row of the table.
struct ProfileCase {
    case: &'static str,
    /// The cluster's name, so each row writes (or fails to write) its own cluster.
    cluster: &'static str,
    /// The `JobFlowRole` the request carries, if any.
    job_flow_role: Option<String>,
    expected: Expected,
}

/// This account's instance-profile ARN for the profile NAME `name` — what EMR reports
/// for a `JobFlowRole` that is a name rather than an ARN.
fn named_profile_arn(name: &str) -> String {
    instance_profile_arn("/", name)
}

fn profile_cases() -> Vec<ProfileCase> {
    vec![
        ProfileCase {
            case: "an instance profile IAM holds, by name",
            cluster: "held-by-name",
            job_flow_role: Some(HELD_PROFILE.to_string()),
            expected: Expected::Launched,
        },
        ProfileCase {
            // IAM entity names "are not distinguished by case".
            case: "the same instance profile in a different case",
            cluster: "held-other-case",
            job_flow_role: Some(HELD_PROFILE.to_lowercase()),
            expected: Expected::Launched,
        },
        ProfileCase {
            case: "an instance profile IAM holds, by ARN",
            cluster: "held-by-arn",
            job_flow_role: Some(instance_profile_arn("/", HELD_PROFILE)),
            expected: Expected::Launched,
        },
        ProfileCase {
            case: "an instance profile under a path, by ARN",
            cluster: "pathed-by-arn",
            job_flow_role: Some(instance_profile_arn(PROFILE_PATH, PATHED_PROFILE)),
            expected: Expected::Launched,
        },
        ProfileCase {
            case: "no JobFlowRole at all, which every cluster needs one of",
            cluster: "no-role",
            job_flow_role: None,
            expected: Expected::Required,
        },
        ProfileCase {
            // `@length(0..10280)` admits the empty string, which names nothing.
            case: "an empty JobFlowRole",
            cluster: "empty-role",
            job_flow_role: Some(String::new()),
            expected: Expected::Required,
        },
        ProfileCase {
            case: "a name IAM holds a role under but no instance profile",
            cluster: "role-only",
            job_flow_role: Some(ROLE_ONLY.to_string()),
            expected: Expected::Invalid(ROLE_ONLY.to_string()),
        },
        ProfileCase {
            case: "an instance profile that was never created",
            cluster: "absent-profile",
            job_flow_role: Some("no-such-profile".to_string()),
            expected: Expected::Invalid("no-such-profile".to_string()),
        },
        ProfileCase {
            case: "an instance profile ARN naming a profile that was never created",
            cluster: "absent-profile-arn",
            job_flow_role: Some(instance_profile_arn("/", "also-no-such-profile")),
            expected: Expected::Invalid(instance_profile_arn("/", "also-no-such-profile")),
        },
        ProfileCase {
            // An ARN of another vendor names no IAM instance profile, whatever its
            // last path segment happens to spell.
            case: "an ARN of another service whose last segment is a held profile name",
            cluster: "foreign-vendor-arn",
            job_flow_role: Some(format!("arn:aws:s3:::bucket/{HELD_PROFILE}")),
            expected: Expected::Invalid(format!("arn:aws:s3:::bucket/{HELD_PROFILE}")),
        },
        ProfileCase {
            // The IAM ROLE ARN of the role the held profile carries: an instance
            // profile and a role are different resources.
            case: "an IAM role ARN rather than an instance-profile ARN",
            cluster: "role-arn",
            job_flow_role: Some(format!(
                "arn:aws:iam::{}:role/{HELD_PROFILE}",
                engine::emulator_account_id()
            )),
            expected: Expected::Invalid(format!(
                "arn:aws:iam::{}:role/{HELD_PROFILE}",
                engine::emulator_account_id()
            )),
        },
        ProfileCase {
            // The profile EXISTS and carries nothing an instance could assume, which is
            // the authorization failure rather than an unresolvable name.
            case: "an instance profile carrying no role at all",
            cluster: "roleless-profile",
            job_flow_role: Some(ROLELESS_PROFILE.to_string()),
            expected: Expected::Unauthorized(named_profile_arn(ROLELESS_PROFILE)),
        },
        ProfileCase {
            // It exists and its role trusts another service principal, so Amazon EC2
            // may not assume it.
            case: "an instance profile whose role trusts another principal",
            cluster: "foreign-trust-profile",
            job_flow_role: Some(FOREIGN_TRUST_PROFILE.to_string()),
            expected: Expected::Unauthorized(named_profile_arn(FOREIGN_TRUST_PROFILE)),
        },
        ProfileCase {
            // Named by ARN, the refusal still reports the ARN IAM holds it under.
            case: "the same unauthorizable profile named by ARN",
            cluster: "foreign-trust-profile-arn",
            job_flow_role: Some(instance_profile_arn("/", FOREIGN_TRUST_PROFILE)),
            expected: Expected::Unauthorized(named_profile_arn(FOREIGN_TRUST_PROFILE)),
        },
    ]
}

/// Every cluster `ListClusters` reports, by name.
fn cluster_names(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
) -> Vec<String> {
    let (status, body) = emr(router, runtime, "ListClusters", serde_json::json!({}));
    assert_eq!(status, 200, "ListClusters must answer 200, got {status} {body}");
    body["Clusters"]
        .as_array()
        .map(|clusters| {
            clusters
                .iter()
                .filter_map(|cluster| cluster["Name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_cluster_is_launched_only_for_an_instance_profile_iam_holds() {
    std::thread::Builder::new()
        .name("emr-job-flow-role".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Setup: two instance profiles, each built the way AWS builds one —
            // CreateRole, CreateInstanceProfile, AddRoleToInstanceProfile — plus a
            // role that is never added to any profile.
            let policy = ec2_trust_policy();
            for (profile, path) in [(HELD_PROFILE, "/"), (PATHED_PROFILE, PROFILE_PATH)] {
                let (status, body) = iam(
                    &router,
                    &runtime,
                    "CreateRole",
                    &[("RoleName", profile), ("AssumeRolePolicyDocument", &policy)],
                );
                assert_eq!(status, 200, "setup: CreateRole {profile}: got {status} {body}");
                let (status, body) = iam(
                    &router,
                    &runtime,
                    "CreateInstanceProfile",
                    &[("InstanceProfileName", profile), ("Path", path)],
                );
                assert_eq!(
                    status, 200,
                    "setup: CreateInstanceProfile {profile}: got {status} {body}"
                );
                let (status, body) = iam(
                    &router,
                    &runtime,
                    "AddRoleToInstanceProfile",
                    &[("InstanceProfileName", profile), ("RoleName", profile)],
                );
                assert_eq!(
                    status, 200,
                    "setup: AddRoleToInstanceProfile {profile}: got {status} {body}"
                );

                // Round trip: the created profile is readable through IAM's own
                // reader, carrying the path it was created under and the role it
                // was given.
                let (status, body) = iam(
                    &router,
                    &runtime,
                    "GetInstanceProfile",
                    &[("InstanceProfileName", profile)],
                );
                assert_eq!(
                    status, 200,
                    "GetInstanceProfile {profile} must answer 200, got {status} {body}"
                );
                for expected in [
                    format!("<InstanceProfileName>{profile}</InstanceProfileName>"),
                    format!("<Path>{path}</Path>"),
                    format!("<RoleName>{profile}</RoleName>"),
                    format!("<Arn>{}</Arn>", instance_profile_arn(path, profile)),
                ] {
                    assert!(
                        body.contains(&expected),
                        "GetInstanceProfile {profile} must report {expected:?}, got {body}"
                    );
                }
            }
            let (status, body) = iam(
                &router,
                &runtime,
                "CreateRole",
                &[
                    ("RoleName", ROLE_ONLY),
                    ("AssumeRolePolicyDocument", &policy),
                ],
            );
            assert_eq!(status, 200, "setup: CreateRole {ROLE_ONLY}: got {status} {body}");

            // An instance profile carrying NO role: it exists, and there is nothing in
            // it for an Amazon EC2 instance to assume.
            let (status, body) = iam(
                &router,
                &runtime,
                "CreateInstanceProfile",
                &[("InstanceProfileName", ROLELESS_PROFILE), ("Path", "/")],
            );
            assert_eq!(
                status, 200,
                "setup: CreateInstanceProfile {ROLELESS_PROFILE}: got {status} {body}"
            );

            // An instance profile whose role trusts ANOTHER service principal, so
            // Amazon EC2 may not assume it.
            let foreign = trust_policy("lambda.amazonaws.com");
            for (action, parameters) in [
                (
                    "CreateRole",
                    vec![
                        ("RoleName", FOREIGN_TRUST_PROFILE),
                        ("AssumeRolePolicyDocument", foreign.as_str()),
                    ],
                ),
                (
                    "CreateInstanceProfile",
                    vec![("InstanceProfileName", FOREIGN_TRUST_PROFILE), ("Path", "/")],
                ),
                (
                    "AddRoleToInstanceProfile",
                    vec![
                        ("InstanceProfileName", FOREIGN_TRUST_PROFILE),
                        ("RoleName", FOREIGN_TRUST_PROFILE),
                    ],
                ),
            ] {
                let (status, body) = iam(&router, &runtime, action, &parameters);
                assert_eq!(
                    status, 200,
                    "setup: IAM {action} for {FOREIGN_TRUST_PROFILE}: got {status} {body}"
                );
            }

            for case in profile_cases() {
                let (status, body) = emr(
                    &router,
                    &runtime,
                    "RunJobFlow",
                    run_job_flow_body(case.cluster, case.job_flow_role.as_deref()),
                );
                let listed = cluster_names(&router, &runtime);
                match &case.expected {
                    Expected::Launched => {
                        assert_eq!(
                            status, 200,
                            "case {:?}: RunJobFlow must succeed, got {status} {body}",
                            case.case
                        );
                        let cluster_id =
                            body["JobFlowId"].as_str().unwrap_or_default().to_string();
                        assert!(
                            !cluster_id.is_empty(),
                            "case {:?}: RunJobFlow must answer a JobFlowId, got {body}",
                            case.case
                        );
                        assert!(
                            listed.iter().any(|name| name == case.cluster),
                            "case {:?}: the cluster must be listed, got {listed:?}",
                            case.case
                        );
                        let (status, described) = emr(
                            &router,
                            &runtime,
                            "DescribeCluster",
                            serde_json::json!({ "ClusterId": cluster_id }),
                        );
                        assert_eq!(
                            status, 200,
                            "case {:?}: DescribeCluster must answer 200, got {status} \
                             {described}",
                            case.case
                        );
                        let reported = described["Cluster"]["Ec2InstanceAttributes"]
                            ["IamInstanceProfile"]
                            .as_str()
                            .map(str::to_string);
                        assert_eq!(
                            reported, case.job_flow_role,
                            "case {:?}: the cluster must report the JobFlowRole it was \
                             given, got {described}",
                            case.case
                        );
                    }
                    Expected::Required => {
                        assert_eq!(
                            (status, error_code(&body).as_str()),
                            (400, "ValidationException"),
                            "case {:?}: a launch naming no instance profile must be \
                             refused 400 ValidationException, got {status} {body}",
                            case.case
                        );
                        assert_eq!(
                            error_message(&body),
                            "InstanceProfile is required for creating cluster.",
                            "case {:?}: expected the required-profile refusal, got {body}",
                            case.case
                        );
                    }
                    Expected::Unauthorized(arn) => {
                        assert_eq!(
                            (status, error_code(&body).as_str()),
                            (400, "AccessDeniedException"),
                            "case {:?}: an unauthorizable instance profile must be \
                             refused 400 AccessDeniedException, got {status} {body}",
                            case.case
                        );
                        assert_eq!(
                            error_message(&body),
                            format!("Failed to authorize instance profile {arn}"),
                            "case {:?}: expected the unauthorized-profile refusal \
                             naming {arn:?}, got {body}",
                            case.case
                        );
                    }
                    Expected::Invalid(value) => {
                        assert_eq!(
                            (status, error_code(&body).as_str()),
                            (400, "ValidationException"),
                            "case {:?}: a value naming no instance profile must be \
                             refused 400 ValidationException, got {status} {body}",
                            case.case
                        );
                        assert_eq!(
                            error_message(&body),
                            format!("Invalid InstanceProfile: {value}."),
                            "case {:?}: expected the unresolvable-profile refusal \
                             echoing {value:?}, got {body}",
                            case.case
                        );
                    }
                }
                if case.expected != Expected::Launched {
                    assert!(
                        !listed.iter().any(|name| name == case.cluster),
                        "case {:?}: a refused launch must write no cluster, got {listed:?}",
                        case.case
                    );
                }
            }
        })
        .expect("spawn the job-flow-role thread")
        .join()
        .expect("every JobFlowRole must be resolved against IAM");
}
