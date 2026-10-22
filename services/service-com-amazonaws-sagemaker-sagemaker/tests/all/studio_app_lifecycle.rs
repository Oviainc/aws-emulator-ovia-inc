// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker Studio's app lifecycle: `CreateApp`,
//! `DescribeApp` and `DeleteApp`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is the one
//! `inference_deployment_lifecycle` builds.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: an app
//! is identified by all five of `DomainId`, the owning `UserProfileName` **or**
//! `SpaceName`, `AppType` and `AppName`, so the round trip, the not-found refusal and
//! the release-on-delete are asserted for every app type in the modelled catalog, for
//! both owner kinds, and for a request that differs from a stored app in each single
//! component.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Every value `AppType` accepts, from the model
/// (`smithy_query(query="shape", shape="AppType", model_path=".../com-amazonaws-sagemaker-sagemaker.json")`,
/// mirrored by `ops::types::APP_TYPE_VALUES`).
const APP_TYPES: [&str; 9] = [
    "Canvas",
    "CodeEditor",
    "DetailedProfiler",
    "JupyterLab",
    "JupyterServer",
    "KernelGateway",
    "RSessionGateway",
    "RStudioServerPro",
    "TensorBoard",
];

/// Long enough to pass `state::APP_IN_SERVICE_AFTER` (300 ms).
const PAST_IN_SERVICE_DUE: Duration = Duration::from_millis(600);

/// The two members that can name an app's owner, per `CreateApp`'s input: exactly one of
/// them is set on any accepted request.
const OWNER_MEMBERS: [&str; 2] = ["UserProfileName", "SpaceName"];

/// One app, as the five members that identify it.
#[derive(Clone)]
struct AppRef {
    case: String,
    domain: String,
    owner_member: &'static str,
    owner_name: String,
    app_type: &'static str,
    app_name: String,
}

impl AppRef {
    fn new(
        case: &str,
        domain: &str,
        owner_member: &'static str,
        owner_name: &str,
        app_type: &'static str,
        app_name: &str,
    ) -> Self {
        Self {
            case: case.to_string(),
            domain: domain.to_string(),
            owner_member,
            owner_name: owner_name.to_string(),
            app_type,
            app_name: app_name.to_string(),
        }
    }

    /// The request body naming this app, i.e. the whole input of `DescribeApp` and
    /// `DeleteApp` and the identifying part of `CreateApp`'s.
    fn body(&self) -> Value {
        let mut members = Map::new();
        members.insert("DomainId".to_string(), json!(self.domain));
        members.insert(self.owner_member.to_string(), json!(self.owner_name));
        members.insert("AppType".to_string(), json!(self.app_type));
        members.insert("AppName".to_string(), json!(self.app_name));
        Value::Object(members)
    }

    /// The member that does NOT name this app's owner, which a reader must omit.
    fn other_owner_member(&self) -> &'static str {
        if self.owner_member == "UserProfileName" {
            "SpaceName"
        } else {
            "UserProfileName"
        }
    }

    /// `arn:aws:sagemaker:<region>:<account>:app/<DomainId>/<owner>/<AppType>/<AppName>`.
    fn arn_in(&self, region: &str) -> String {
        format!(
            "arn:aws:sagemaker:{region}:{ACCOUNT}:app/{}/{}/{}/{}",
            self.domain, self.owner_name, self.app_type, self.app_name
        )
    }

    fn arn(&self) -> String {
        self.arn_in("us-east-1")
    }
}

/// `base` with every member of `extra` added — how a `CreateApp` body is built from the
/// five identifying members plus the optional ones a row carries.
fn with_members(base: &Value, extra: &Value) -> Value {
    let mut merged = base
        .as_object()
        .expect("the identifying members form an object")
        .clone();
    for (member, value) in extra
        .as_object()
        .expect("the extra members form an object")
    {
        merged.insert(member.clone(), value.clone());
    }
    Value::Object(merged)
}

/// The rows the round-trip and release rules are asserted over: every app type in the
/// catalog, alternating owner kind, domain ids and app names at their modelled extremes,
/// and the optional members present on some rows and absent on others.
///
/// Each row carries the members `DescribeApp` must echo back (`echoed`); `Tags` is
/// deliberately on a row whose `echoed` is empty, because `DescribeApp` models no tag
/// member.
fn round_trip_rows() -> Vec<(AppRef, Value, Value)> {
    // `AppName` is `len 0..63`; `DomainId` is `len 0..63, pattern ^d-(-*[a-z0-9]){1,61}$`
    // (smithy_query operation=CreateApp), so these are the longest accepted values.
    let longest_name = "a".repeat(63);
    let longest_domain = format!("d-{}", "x".repeat(61));

    vec![
        (
            AppRef::new(
                "canvas-user-profile",
                "d-abc123",
                "UserProfileName",
                "studio-user",
                "Canvas",
                "default",
            ),
            json!({}),
            json!({}),
        ),
        (
            AppRef::new(
                "code-editor-space-with-instance-type",
                "d-abc123",
                "SpaceName",
                "team-space",
                "CodeEditor",
                "editor-1",
            ),
            json!({ "ResourceSpec": { "InstanceType": "ml.t3.medium" } }),
            json!({ "ResourceSpec": { "InstanceType": "ml.t3.medium" } }),
        ),
        (
            AppRef::new(
                "detailed-profiler-shortest-domain",
                "d-0",
                "UserProfileName",
                "u",
                "DetailedProfiler",
                "profiler",
            ),
            json!({}),
            json!({}),
        ),
        (
            AppRef::new(
                "jupyter-lab-longest-domain-and-name",
                &longest_domain,
                "SpaceName",
                "space-with-hyphens",
                "JupyterLab",
                &longest_name,
            ),
            json!({}),
            json!({}),
        ),
        (
            AppRef::new(
                "jupyter-server-recovery-mode",
                "d-abc123",
                "UserProfileName",
                "studio-user",
                "JupyterServer",
                "default",
            ),
            json!({ "RecoveryMode": true }),
            json!({ "RecoveryMode": true }),
        ),
        (
            AppRef::new(
                "kernel-gateway-full-resource-spec",
                "d-abc123",
                "SpaceName",
                "team-space",
                "KernelGateway",
                "kg-1",
            ),
            json!({
                "ResourceSpec": {
                    "InstanceType": "ml.m5.large",
                    "SageMakerImageArn": format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image/datascience"),
                },
                "RecoveryMode": false,
            }),
            json!({
                "ResourceSpec": {
                    "InstanceType": "ml.m5.large",
                    "SageMakerImageArn": format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image/datascience"),
                },
                "RecoveryMode": false,
            }),
        ),
        (
            AppRef::new(
                "r-session-gateway-digits-and-hyphens",
                "d-9f8e7d6c",
                "UserProfileName",
                "user-42",
                "RSessionGateway",
                "r-session-1",
            ),
            json!({}),
            json!({}),
        ),
        (
            AppRef::new(
                "rstudio-server-pro-space",
                "d-abc123",
                "SpaceName",
                "rstudio-space",
                "RStudioServerPro",
                "rstudio",
            ),
            json!({}),
            json!({}),
        ),
        (
            AppRef::new(
                "tensorboard-shortest-name-with-tags",
                "d-abc123",
                "UserProfileName",
                "studio-user",
                "TensorBoard",
                "t",
            ),
            json!({ "Tags": [{ "Key": "team", "Value": "ml" }] }),
            json!({}),
        ),
    ]
}

/// RULE: `CreateApp` stores an app under all five members that identify it and mints
/// `arn:aws:sagemaker:<region>:<account>:app/<DomainId>/<owner>/<AppType>/<AppName>`;
/// `DescribeApp` naming those same five members replays what the create stored — the
/// ARN, the identifying members (with only the owner member that was set), the optional
/// members it carried, the stored creation time, and the launching `Pending` status.
#[tokio::test]
async fn describe_app_replays_exactly_what_create_app_stored() {
    let emulator = Emulator::new();

    for (app, extra, echoed) in round_trip_rows() {
        let case = &app.case;
        let created = emulator
            .ok("CreateApp", &with_members(&app.body(), &extra))
            .await;
        assert_eq!(
            str_field(&created, "AppArn"),
            app.arn(),
            "case {case}: CreateApp AppArn, got {created:?}"
        );

        let described = emulator.ok("DescribeApp", &app.body()).await;
        assert_eq!(
            str_field(&described, "AppArn"),
            app.arn(),
            "case {case}: DescribeApp AppArn, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "DomainId"),
            app.domain,
            "case {case}: DomainId, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "AppType"),
            app.app_type,
            "case {case}: AppType, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "AppName"),
            app.app_name,
            "case {case}: AppName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, app.owner_member),
            app.owner_name,
            "case {case}: {}, got {described:?}",
            app.owner_member
        );
        assert!(
            described.get(app.other_owner_member()).is_none(),
            "case {case}: {} must be omitted for an app the other member owns, got {described:?}",
            app.other_owner_member()
        );
        assert_eq!(
            str_field(&described, "Status"),
            "Pending",
            "case {case}: a just-created app is still launching, got {described:?}"
        );
        assert!(
            described.get("CreationTime").is_some_and(Value::is_number),
            "case {case}: CreationTime must be a stored timestamp, got {described:?}"
        );
        for (member, value) in echoed.as_object().expect("echoed members form an object") {
            assert_eq!(
                described.get(member),
                Some(value),
                "case {case}: {member} must be replayed as stored, got {described:?}"
            );
        }
        // Nothing health-checked the app and no user touched it, so the emulator reports
        // no activity timestamps and no failure for an app that is launching.
        for member in ["LastHealthCheckTimestamp", "LastUserActivityTimestamp", "FailureReason"] {
            assert!(
                described.get(member).is_none(),
                "case {case}: {member} must be omitted when nothing produced one, got {described:?}"
            );
        }

        let again = emulator.ok("DescribeApp", &app.body()).await;
        assert_eq!(
            again, described,
            "case {case}: a second DescribeApp must answer identically"
        );
    }
}

/// RULE: an app's ARN carries the region the caller signed for, so a client reading an
/// app back never sees an ARN naming another region.
#[tokio::test]
async fn an_app_arn_names_the_region_the_caller_signed_for() {
    let emulator = Emulator::new();

    for region in ["us-east-1", "eu-west-2", "ap-southeast-4"] {
        let app = AppRef::new(
            region,
            "d-abc123",
            "UserProfileName",
            "studio-user",
            "JupyterLab",
            "regional",
        );
        let (status, created) = emulator.call_in(region, "CreateApp", &app.body()).await;
        assert_eq!(
            status, 200,
            "case {region}: expected HTTP 200 from CreateApp, got {status} with {created:?}"
        );
        assert_eq!(
            str_field(&created, "AppArn"),
            app.arn_in(region),
            "case {region}: AppArn, got {created:?}"
        );

        let (status, deleted) = emulator.call_in(region, "DeleteApp", &app.body()).await;
        assert_eq!(
            status, 200,
            "case {region}: expected HTTP 200 from DeleteApp, got {status} with {deleted:?}"
        );
    }
}

/// RULE: a newly created app is reported `Pending`, and the `InService` transition the
/// clock owes becomes visible once its span has elapsed — without any call having to make
/// it happen, and stably thereafter.
#[tokio::test]
async fn an_app_advances_from_pending_to_in_service_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let app = AppRef::new(
        "clock-owed-launch",
        "d-abc123",
        "SpaceName",
        "team-space",
        "JupyterLab",
        "waiter",
    );
    emulator.ok("CreateApp", &app.body()).await;

    let pending = emulator.ok("DescribeApp", &app.body()).await;
    assert_eq!(
        str_field(&pending, "Status"),
        "Pending",
        "case immediately-after-create: Status, got {pending:?}"
    );

    tokio::time::sleep(PAST_IN_SERVICE_DUE).await;

    let settled = emulator.ok("DescribeApp", &app.body()).await;
    assert_eq!(
        str_field(&settled, "Status"),
        "InService",
        "case after-the-due-span: Status, got {settled:?}"
    );
    let again = emulator.ok("DescribeApp", &app.body()).await;
    assert_eq!(
        again, settled,
        "case repeated-read: a settled app must read back identically"
    );
}

/// RULE: `DeleteApp` is refused with HTTP 400 `ResourceNotFound` — never a 501 and never
/// an empty 200 — for every key no app is stored under: one nothing created, one that
/// differs from a stored app in any single identifying member, and one a previous delete
/// already released. The refusal's message is built from the request, naming the app's
/// ARN: `App [<arn>] does not exist.`
#[tokio::test]
async fn deleting_an_app_that_is_not_stored_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();

    let stored = AppRef::new(
        "stored",
        "d-abc123",
        "UserProfileName",
        "studio-user",
        "JupyterServer",
        "default",
    );
    emulator.ok("CreateApp", &stored.body()).await;

    // Rows: the empty-store case, then one row per identifying member, each differing
    // from the stored app in that member alone, so no single component can be ignored.
    let mut cases = vec![
        AppRef::new(
            "nothing-created",
            "d-999999",
            "UserProfileName",
            "nobody",
            "Canvas",
            "absent",
        ),
        AppRef::new(
            "other-domain",
            "d-def456",
            "UserProfileName",
            "studio-user",
            "JupyterServer",
            "default",
        ),
        AppRef::new(
            "other-user-profile",
            "d-abc123",
            "UserProfileName",
            "other-user",
            "JupyterServer",
            "default",
        ),
        AppRef::new(
            "owner-is-a-space-not-a-user-profile",
            "d-abc123",
            "SpaceName",
            "studio-user",
            "JupyterServer",
            "default",
        ),
        AppRef::new(
            "other-app-name",
            "d-abc123",
            "UserProfileName",
            "studio-user",
            "JupyterServer",
            "other",
        ),
    ];
    // One row per app type the stored app is not, so the type is part of the key for the
    // whole modelled catalog rather than for one value.
    cases.extend(
        APP_TYPES
            .iter()
            .filter(|app_type| **app_type != stored.app_type)
            .map(|app_type| {
                AppRef::new(
                    &format!("other-app-type-{app_type}"),
                    "d-abc123",
                    "UserProfileName",
                    "studio-user",
                    app_type,
                    "default",
                )
            }),
    );

    for app in &cases {
        let case = &app.case;
        let (status, parsed) = emulator.call("DeleteApp", &app.body()).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 from DeleteApp, got {status} with {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case}: error code, got {parsed:?}"
        );
        assert_eq!(
            message(&parsed),
            format!("App [{}] does not exist.", app.arn()),
            "case {case}: the refusal must name the app it could not find, got {parsed:?}"
        );
    }

    // The stored app is still there: none of the refused deletes touched it.
    let described = emulator.ok("DescribeApp", &stored.body()).await;
    assert_eq!(
        str_field(&described, "AppArn"),
        stored.arn(),
        "case stored-app-survives: AppArn, got {described:?}"
    );

    // And a key a delete already released is refused the same way.
    let (status, parsed) = emulator.call("DeleteApp", &stored.body()).await;
    assert_eq!(
        status, 200,
        "case first-delete: expected HTTP 200, got {status} with {parsed:?}"
    );
    let (status, parsed) = emulator.call("DeleteApp", &stored.body()).await;
    assert_eq!(
        status, 400,
        "case second-delete: expected HTTP 400, got {status} with {parsed:?}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFound",
        "case second-delete: error code, got {parsed:?}"
    );
}

/// RULE: a delete releases exactly the app its five members name — that app becomes
/// unreadable (HTTP 400 `ResourceNotFound`) and its key is free to create again, while
/// every app differing in any single member is untouched.
#[tokio::test]
async fn deleting_an_app_releases_it_and_leaves_every_other_app_alone() {
    let emulator = Emulator::new();

    // Four apps that pairwise differ in exactly one identifying member, so a delete
    // keyed on the whole tuple can be told apart from one keyed on any part of it.
    let apps = [
        AppRef::new(
            "target",
            "d-abc123",
            "UserProfileName",
            "studio-user",
            "JupyterServer",
            "shared-name",
        ),
        AppRef::new(
            "same-name-other-type",
            "d-abc123",
            "UserProfileName",
            "studio-user",
            "KernelGateway",
            "shared-name",
        ),
        AppRef::new(
            "same-name-owned-by-a-space",
            "d-abc123",
            "SpaceName",
            "studio-user",
            "JupyterServer",
            "shared-name",
        ),
        AppRef::new(
            "same-name-other-domain",
            "d-def456",
            "UserProfileName",
            "studio-user",
            "JupyterServer",
            "shared-name",
        ),
    ];
    for app in &apps {
        emulator.ok("CreateApp", &app.body()).await;
    }

    emulator.ok("DeleteApp", &apps[0].body()).await;

    let (status, parsed) = emulator.call("DescribeApp", &apps[0].body()).await;
    assert_eq!(
        status, 400,
        "case deleted-app: expected HTTP 400 from DescribeApp, got {status} with {parsed:?}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFound",
        "case deleted-app: error code, got {parsed:?}"
    );
    assert_eq!(
        message(&parsed),
        format!("App [{}] does not exist.", apps[0].arn()),
        "case deleted-app: the refusal must name the released app, got {parsed:?}"
    );

    for app in &apps[1..] {
        let described = emulator.ok("DescribeApp", &app.body()).await;
        assert_eq!(
            str_field(&described, "AppArn"),
            app.arn(),
            "case {}: a sibling app must be untouched by the delete, got {described:?}",
            app.case
        );
    }

    // The released key is free again, and the app created under it is a fresh one.
    let recreated = emulator.ok("CreateApp", &apps[0].body()).await;
    assert_eq!(
        str_field(&recreated, "AppArn"),
        apps[0].arn(),
        "case recreate-after-delete: AppArn, got {recreated:?}"
    );
    let described = emulator.ok("DescribeApp", &apps[0].body()).await;
    assert_eq!(
        str_field(&described, "Status"),
        "Pending",
        "case recreate-after-delete: the new app is launching, got {described:?}"
    );
}

/// RULE: a `CreateApp` whose five identifying members are already in use is refused with
/// the modelled `ResourceInUse`, so the running app is never silently replaced.
#[tokio::test]
async fn creating_an_app_whose_key_is_in_use_is_refused_with_resource_in_use() {
    let emulator = Emulator::new();

    for (app, extra, _) in round_trip_rows() {
        let case = &app.case;
        emulator
            .ok("CreateApp", &with_members(&app.body(), &extra))
            .await;

        // A second create of the same key, carrying different optional members, must
        // neither succeed nor overwrite what is stored.
        let (status, parsed) = emulator
            .call(
                "CreateApp",
                &with_members(
                    &app.body(),
                    &json!({ "ResourceSpec": { "InstanceType": "ml.g5.xlarge" } }),
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 from the duplicate CreateApp, got {status} with {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceInUse",
            "case {case}: error code, got {parsed:?}"
        );
        // The wording of a duplicate-create refusal is not a documented rule, so only
        // the ARN it is built from is asserted.
        assert!(
            message(&parsed).contains(&app.arn()),
            "case {case}: the refusal must name the app already in use, got {parsed:?}"
        );

        let described = emulator.ok("DescribeApp", &app.body()).await;
        let expected_spec = extra.get("ResourceSpec");
        assert_eq!(
            described.get("ResourceSpec"),
            expected_spec,
            "case {case}: the stored app must keep the first create's ResourceSpec, got {described:?}"
        );
    }
}

/// RULE: an app operation naming neither `UserProfileName` nor `SpaceName`, or naming
/// both, is refused with HTTP 400 `ValidationException`: each member's model doc makes it
/// required when the other is absent, so the owner is exactly one of them.
#[tokio::test]
async fn an_app_request_must_name_exactly_one_owner() {
    let emulator = Emulator::new();

    // A stored app, so a refusal cannot be mistaken for "not found".
    let stored = AppRef::new(
        "stored",
        "d-abc123",
        "UserProfileName",
        "studio-user",
        "JupyterServer",
        "default",
    );
    emulator.ok("CreateApp", &stored.body()).await;

    let neither = json!({
        "DomainId": "d-abc123",
        "AppType": "JupyterServer",
        "AppName": "default",
    });
    let both = json!({
        "DomainId": "d-abc123",
        "UserProfileName": "studio-user",
        "SpaceName": "team-space",
        "AppType": "JupyterServer",
        "AppName": "default",
    });

    // Rows: every operation that resolves an app, with each malformed owner combination.
    for target in ["CreateApp", "DescribeApp", "DeleteApp"] {
        for (case, body) in [("neither-owner", &neither), ("both-owners", &both)] {
            let (status, parsed) = emulator.call(target, body).await;
            assert_eq!(
                status, 400,
                "case {target}/{case}: expected HTTP 400, got {status} with {parsed:?}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {target}/{case}: error code, got {parsed:?}"
            );
            for member in OWNER_MEMBERS {
                assert!(
                    message(&parsed).contains(member),
                    "case {target}/{case}: the refusal must name {member}, got {parsed:?}"
                );
            }
        }
    }

    // The stored app is unchanged by any of those refusals.
    let described = emulator.ok("DescribeApp", &stored.body()).await;
    assert_eq!(
        str_field(&described, "AppArn"),
        stored.arn(),
        "case stored-app-survives: AppArn, got {described:?}"
    );
}

/// RULE: an app request outside a member's modelled constraint is refused with HTTP 400
/// before any state is written.
#[tokio::test]
async fn app_requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();

    // Rows: one past each published bound — `AppName` / `DomainId` are `len 0..63`,
    // `DomainId` matches `^d-(-*[a-z0-9]){1,61}$`, and `AppType` is the enum whose values
    // `APP_TYPES` lists (smithy_query operation=CreateApp).
    let too_long_name = "a".repeat(64);
    let too_long_domain = format!("d-{}", "x".repeat(62));
    let cases: Vec<(&str, Value)> = vec![
        (
            "app-name-one-past-63",
            json!({
                "DomainId": "d-abc123",
                "UserProfileName": "studio-user",
                "AppType": "JupyterServer",
                "AppName": too_long_name,
            }),
        ),
        (
            "domain-id-one-past-63",
            json!({
                "DomainId": too_long_domain,
                "UserProfileName": "studio-user",
                "AppType": "JupyterServer",
                "AppName": "default",
            }),
        ),
        (
            "domain-id-outside-the-pattern",
            json!({
                "DomainId": "not-a-domain",
                "UserProfileName": "studio-user",
                "AppType": "JupyterServer",
                "AppName": "default",
            }),
        ),
        (
            "app-type-outside-the-catalog",
            json!({
                "DomainId": "d-abc123",
                "UserProfileName": "studio-user",
                "AppType": "NotAnAppType",
                "AppName": "default",
            }),
        ),
    ];

    for target in ["CreateApp", "DescribeApp", "DeleteApp"] {
        for (case, body) in &cases {
            let (status, parsed) = emulator.call(target, body).await;
            assert_eq!(
                status, 400,
                "case {target}/{case}: expected HTTP 400, got {status} with {parsed:?}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {target}/{case}: error code, got {parsed:?}"
            );
        }
    }
}

/// RULE: the app store holds at most its quota of records; the create past it is refused
/// with the modelled `ResourceLimitExceeded` and stores nothing, so no caller can grow the
/// emulator's state without end by varying the app's name.
#[tokio::test]
async fn a_create_past_the_app_quota_is_refused_with_the_modelled_limit_failure() {
    // `state::APP_QUOTA`.
    const QUOTA: usize = 1_000;

    let emulator = Emulator::new();
    let app_at = |index: usize| {
        AppRef::new(
            "quota",
            "d-abc123",
            "UserProfileName",
            "studio-user",
            "JupyterServer",
            &format!("app{index}"),
        )
    };

    for index in 0..QUOTA {
        emulator.ok("CreateApp", &app_at(index).body()).await;
    }

    let overflow = app_at(QUOTA);
    let (status, parsed) = emulator.call("CreateApp", &overflow.body()).await;
    assert_eq!(
        status, 400,
        "case one-past-the-quota: expected HTTP 400, got {status} with {parsed:?}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceLimitExceeded",
        "case one-past-the-quota: error code, got {parsed:?}"
    );

    // Nothing was stored for the refused create.
    let (status, parsed) = emulator.call("DescribeApp", &overflow.body()).await;
    assert_eq!(
        status, 400,
        "case refused-create-stored-nothing: expected HTTP 400, got {status} with {parsed:?}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFound",
        "case refused-create-stored-nothing: error code, got {parsed:?}"
    );

    // Deleting one app frees exactly one slot.
    emulator.ok("DeleteApp", &app_at(0).body()).await;
    emulator.ok("CreateApp", &overflow.body()).await;
}
