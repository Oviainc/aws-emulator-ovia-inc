// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's MLflow app pair: `CreateMlflowApp` and the
//! reader that replays it, `DescribeMlflowApp`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, on
//! the shared `Emulator` harness in `inference_deployment_lifecycle`.
//!
//! The suites assert RULES over TABLES of inputs rather than one recorded call: the
//! minted ARN's shape for every name length and every signed region, the create/describe
//! round trip for every optional member and every value of the two enums involved, the
//! documented `ModelRegistrationMode` default, the `Creating` -> `Created` transition the
//! clock owes, the `ResourceNotFound` refusal for an ARN no app was created under, and
//! the store's bound.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::MLFLOW_APP_CREATED_AFTER` (300 ms).
const PAST_CREATED_DUE: std::time::Duration = std::time::Duration::from_millis(600);

/// An IAM role ARN satisfying `RoleArn`'s modelled pattern.
const ROLE: &str = "arn:aws:iam::123456789012:role/service-role/MlflowAppRole";

/// `ModelRegistrationMode`'s members, from
/// `smithy_query(query="shape",shape="ModelRegistrationMode")`.
const MODEL_REGISTRATION_MODES: [&str; 2] =
    ["AutoModelRegistrationEnabled", "AutoModelRegistrationDisabled"];

/// `AccountDefaultStatus`'s members, from
/// `smithy_query(query="shape",shape="AccountDefaultStatus")`.
const ACCOUNT_DEFAULT_STATUSES: [&str; 2] = ["ENABLED", "DISABLED"];

/// The members `CreateMlflowApp` accepts that the model documents NO default for, so an
/// absent one must stay absent in `DescribeMlflowApp`'s answer.
const OPTIONAL_MEMBERS_WITHOUT_A_DEFAULT: [&str; 4] = [
    "KmsKeyId",
    "WeeklyMaintenanceWindowStart",
    "AccountDefaultStatus",
    "DefaultDomainIdList",
];

/// The IAM identity ARN `state::caller_user_context` builds for the harness's signed
/// requests (`Credential=AKIDEXAMPLE/...`) and for an unsigned one, where
/// `emulator_server::extract_akid` answers `"default"`.
fn signed_identity_arn(principal: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/{principal}")
}

/// A minimal create body: the three required members only.
fn minimal(name: &str) -> Value {
    json!({
        "Name": name,
        "ArtifactStoreUri": "s3://mlflow-artifacts/apps/",
        "RoleArn": ROLE,
    })
}

/// Create an app and answer the `Arn` the operation minted.
async fn create(emulator: &Emulator, case: &str, body: &Value) -> String {
    let created = emulator.ok("CreateMlflowApp", body).await;
    let arn = str_field(&created, "Arn");
    assert!(
        !arn.is_empty(),
        "case {case:?}: CreateMlflowApp must answer an Arn, got {created:?}"
    );
    arn
}

/// The resource segment of `arn`, i.e. everything after `:mlflow-app/`.
fn mlflow_app_id(arn: &str) -> &str {
    arn.rsplit(":mlflow-app/").next().unwrap_or_default()
}

/// RULE: a create persists the resource and answers the ARN it minted —
/// `arn:aws:sagemaker:<signed region>:<account>:mlflow-app/<server-minted id>`, inside
/// `MlflowAppArn`'s modelled `len 1..128` for every accepted name, with the id in place
/// of the name because the model documents that the name is not part of the ARN.
#[tokio::test]
async fn a_minted_mlflow_app_arn_names_the_region_the_account_and_a_server_minted_id() {
    let emulator = Emulator::new();

    // Rows differ along the name's LENGTH and shape, including both published bounds of
    // `MlflowAppName` (len 1..256, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,255}$).
    let at_the_upper_bound = format!("a{}", "b".repeat(255));
    let cases: Vec<(&str, String)> = vec![
        ("name-at-the-lower-bound", "a".to_string()),
        ("short-name", "mlflow-app-one".to_string()),
        ("digits-only-name", "20261022".to_string()),
        ("hyphen-runs", "a--b---c".to_string()),
        ("name-at-the-upper-bound", at_the_upper_bound),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, name) in &cases {
        let arn = create(&emulator, case, &minimal(name)).await;

        let prefix = format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-app/");
        assert!(
            arn.starts_with(&prefix),
            "case {case:?}: expected an ARN starting {prefix}, got {arn:?}"
        );
        assert!(
            arn.len() <= 128,
            "case {case:?}: MlflowAppArn is modelled len 1..128, got {} bytes in {arn:?}",
            arn.len()
        );

        // The id is the ARN's identity: 32 lowercase hex characters, and never the name,
        // whatever the name's length.
        let id = mlflow_app_id(&arn);
        assert_eq!(
            id.len(),
            32,
            "case {case:?}: expected a 32-character id in the ARN, got {id:?}"
        );
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "case {case:?}: expected a lowercase hex id in the ARN, got {id:?}"
        );
        assert!(
            !arn.contains(name.as_str()) || name.len() == 1,
            "case {case:?}: the name must not be part of the ARN, got {arn:?}"
        );

        assert!(
            !seen.contains(&arn),
            "case {case:?}: every create must mint a distinct ARN, got {arn:?} twice"
        );
        seen.push(arn);
    }
}

/// RULE: the ARN's region is the one the caller's SigV4 credential scope names, and the
/// emulator's default region when the request carries no scope.
#[tokio::test]
async fn the_minted_arn_takes_its_region_from_the_signed_credential_scope() {
    let emulator = Emulator::new();

    // Rows: regions from three partitions' naming shapes, plus the unsigned request.
    for region in ["us-east-1", "eu-west-3", "ap-southeast-2", "us-gov-west-1"] {
        let created = emulator
            .call_in(region, "CreateMlflowApp", &minimal("region-probe"))
            .await;
        assert_eq!(
            created.0, 200,
            "case {region:?}: expected HTTP 200, got {} with {:?}",
            created.0, created.1
        );
        let arn = str_field(&created.1, "Arn");
        assert!(
            arn.starts_with(&format!("arn:aws:sagemaker:{region}:{ACCOUNT}:mlflow-app/")),
            "case {region:?}: the ARN must name the signed region, got {arn:?}"
        );

        // The app is readable through the ARN it was minted with.
        let described = emulator
            .call_in(region, "DescribeMlflowApp", &json!({ "Arn": arn.clone() }))
            .await;
        assert_eq!(
            described.0, 200,
            "case {region:?}: the app must describe back, got {} with {:?}",
            described.0, described.1
        );
    }

    let (status, created) = emulator
        .call_unsigned("CreateMlflowApp", &minimal("unsigned-probe"))
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected HTTP 200, got {status} with {created:?}"
    );
    let arn = str_field(&created, "Arn");
    assert!(
        arn.starts_with(&format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-app/")),
        "case unsigned: the ARN must fall back to the emulator's default region, got {arn:?}"
    );

    // An unsigned request presents no access key id, so the creator attribution names
    // the principal `extract_akid` falls back to rather than a signed one.
    let described = emulator
        .call_unsigned("DescribeMlflowApp", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        described.1["CreatedBy"]["IamIdentity"]["Arn"].as_str(),
        Some(signed_identity_arn("default").as_str()),
        "case unsigned: CreatedBy, got {:?}",
        described.1
    );
}

/// RULE: a describe replays exactly what the create stored — every member the request
/// carried comes back unchanged, and a member the request omitted stays absent unless
/// the model documents a default for it.
#[tokio::test]
async fn describe_mlflow_app_replays_exactly_what_create_mlflow_app_stored() {
    let emulator = Emulator::new();

    // Rows differ along WHICH optional members the request carries, the enum values it
    // picks, the artifact-store scheme (`S3Uri` accepts `s3` and `https`) and the
    // maintenance window (both ends of `WeeklyMaintenanceWindowStart`'s pattern).
    let cases: Vec<(&str, Value)> = vec![
        ("required-members-only", minimal("replay-minimal")),
        (
            "every-optional-member",
            json!({
                "Name": "replay-full",
                "ArtifactStoreUri": "s3://mlflow-artifacts/full/",
                "RoleArn": ROLE,
                "KmsKeyId": "alias/mlflow-key",
                "ModelRegistrationMode": "AutoModelRegistrationEnabled",
                "WeeklyMaintenanceWindowStart": "Tue:03:30",
                "AccountDefaultStatus": "ENABLED",
                "DefaultDomainIdList": ["d-abc123456789", "d-def987654321"],
                "Tags": [{ "Key": "team", "Value": "ml" }],
            }),
        ),
        (
            "https-artifact-store-and-disabled-defaults",
            json!({
                "Name": "replay-https",
                "ArtifactStoreUri": "https://mlflow-artifacts.s3.us-east-1.amazonaws.com/x",
                "RoleArn": ROLE,
                "ModelRegistrationMode": "AutoModelRegistrationDisabled",
                "AccountDefaultStatus": "DISABLED",
                "DefaultDomainIdList": [],
                "WeeklyMaintenanceWindowStart": "Sun:00:00",
            }),
        ),
        (
            "window-at-the-last-minute-of-the-week",
            json!({
                "Name": "replay-window",
                "ArtifactStoreUri": "s3://mlflow-artifacts/window/",
                "RoleArn": "arn:aws:iam::123456789012:role/Minimal",
                "WeeklyMaintenanceWindowStart": "Sat:23:59",
                "KmsKeyId": "",
            }),
        ),
    ];

    for (case, body) in &cases {
        let arn = create(&emulator, case, body).await;
        let described = emulator
            .ok("DescribeMlflowApp", &json!({ "Arn": arn.clone() }))
            .await;

        assert_eq!(
            described.get("Arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: Arn must be the one the create minted, got {described:?}"
        );
        for member in ["Name", "ArtifactStoreUri", "RoleArn"] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case:?}: {member} must replay the request, got {described:?}"
            );
        }
        for member in OPTIONAL_MEMBERS_WITHOUT_A_DEFAULT {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case:?}: {member} must replay the request exactly (absent when the \
                 request omitted it, since the model documents no default), got {described:?}"
            );
        }
        assert!(
            described.get("CreationTime").and_then(Value::as_f64).is_some(),
            "case {case:?}: CreationTime must be reported, got {described:?}"
        );
        assert!(
            described
                .get("LastModifiedTime")
                .and_then(Value::as_f64)
                .is_some(),
            "case {case:?}: LastModifiedTime must be reported, got {described:?}"
        );
        // The creator attribution is the identity the CREATE signed with, and the only
        // modification so far is that create, so both members carry it.
        assert_eq!(
            described["CreatedBy"]["IamIdentity"]["Arn"].as_str(),
            Some(signed_identity_arn("AKIDEXAMPLE").as_str()),
            "case {case:?}: CreatedBy must name the signing identity, got {described:?}"
        );
        assert_eq!(
            described.get("LastModifiedBy"),
            described.get("CreatedBy"),
            "case {case:?}: LastModifiedBy must match CreatedBy while the create is the \
             only modification, got {described:?}"
        );
    }
}

/// RULE: `ModelRegistrationMode` is the one create member with a DOCUMENTED default —
/// the model says "If not specified, AutomaticModelRegistration defaults to
/// AutoModelRegistrationDisabled" — so an absent member describes as
/// `AutoModelRegistrationDisabled` and a named one is replayed unchanged.
#[tokio::test]
async fn the_documented_model_registration_default_applies_only_when_the_request_omits_it() {
    let emulator = Emulator::new();

    // Rows: the absent member, plus every member of the modelled enum.
    let mut cases: Vec<(&str, Option<&str>, &str)> = vec![(
        "absent",
        None,
        "AutoModelRegistrationDisabled",
    )];
    for mode in MODEL_REGISTRATION_MODES {
        cases.push((mode, Some(mode), mode));
    }

    for (case, requested, expected) in &cases {
        let mut body = minimal("registration-mode-probe");
        if let Some(mode) = requested {
            body["ModelRegistrationMode"] = json!(mode);
        }
        let arn = create(&emulator, case, &body).await;
        let described = emulator
            .ok("DescribeMlflowApp", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            str_field(&described, "ModelRegistrationMode"),
            *expected,
            "case {case:?}: expected ModelRegistrationMode {expected}, got {described:?}"
        );
    }

    // And the enum member that is NOT documented as a default acquires no value of its
    // own: `AccountDefaultStatus` stays absent when the request omits it, for either
    // value it could have carried.
    for status in ACCOUNT_DEFAULT_STATUSES {
        let mut body = minimal("account-default-probe");
        body["AccountDefaultStatus"] = json!(status);
        let arn = create(&emulator, status, &body).await;
        let described = emulator
            .ok("DescribeMlflowApp", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            str_field(&described, "AccountDefaultStatus"),
            status,
            "case {status:?}: AccountDefaultStatus must replay the request, got {described:?}"
        );
    }
    let arn = create(&emulator, "account-default-absent", &minimal("no-account-default")).await;
    let described = emulator
        .ok("DescribeMlflowApp", &json!({ "Arn": arn }))
        .await;
    assert!(
        described.get("AccountDefaultStatus").is_none(),
        "case account-default-absent: the member must stay absent, got {described:?}"
    );
}

/// RULE: an MLflow app's identity is the ARN the create minted, not its name: two creates
/// naming the same app yield two independently readable resources.
#[tokio::test]
async fn every_create_mints_an_independent_app_even_for_a_name_already_in_use() {
    let emulator = Emulator::new();

    // Rows: the same name created repeatedly, each row carrying a different artifact
    // store so the describes can be told apart.
    let mut arns: Vec<(String, String)> = Vec::new();
    for index in 0..4 {
        let uri = format!("s3://mlflow-artifacts/same-name-{index}/");
        let mut body = minimal("same-name");
        body["ArtifactStoreUri"] = json!(uri.clone());
        let arn = create(&emulator, &format!("duplicate-{index}"), &body).await;
        assert!(
            !arns.iter().any(|(seen, _)| seen == &arn),
            "case duplicate-{index}: expected a distinct ARN, got {arn:?} twice"
        );
        arns.push((arn, uri));
    }

    for (index, (arn, uri)) in arns.iter().enumerate() {
        let described = emulator
            .ok("DescribeMlflowApp", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            str_field(&described, "Name"),
            "same-name",
            "case duplicate-{index}: Name, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "ArtifactStoreUri"),
            *uri,
            "case duplicate-{index}: each app keeps its own artifact store, got {described:?}"
        );
    }
}

/// RULE: a reader asked for an ARN no app was created under refuses with the operation's
/// modelled `ResourceNotFound` (HTTP 400) and names the ARN it could not find.
#[tokio::test]
async fn describing_an_arn_no_app_was_created_under_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    let existing = create(&emulator, "existing", &minimal("exists")).await;
    let existing_id = mlflow_app_id(&existing).to_string();

    // Rows: an empty store's ARN shape, an unknown id beside a known one, the known id
    // in ANOTHER region (the ARN is the identity, so the region is part of it), and an
    // id of a different length — every row satisfies `MlflowAppArn`'s modelled pattern.
    let cases: Vec<(&str, String)> = vec![
        (
            "unknown-id",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-app/ffffffffffffffffffffffffffffffff"),
        ),
        (
            "known-id-in-another-region",
            format!("arn:aws:sagemaker:eu-west-1:{ACCOUNT}:mlflow-app/{existing_id}"),
        ),
        (
            "known-id-under-another-account",
            format!("arn:aws:sagemaker:us-east-1:210987654321:mlflow-app/{existing_id}"),
        ),
        (
            "short-id",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-app/a"),
        ),
    ];

    for (case, arn) in &cases {
        let (status, parsed) = emulator
            .call("DescribeMlflowApp", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled refusal's HTTP 400, got {status} with {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case:?}: expected the operation's modelled error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(arn.as_str()),
            "case {case:?}: the refusal must name the ARN it could not find, got {:?}",
            message(&parsed)
        );
    }

    // The refusals changed nothing: the app that does exist still reads back.
    let described = emulator
        .ok("DescribeMlflowApp", &json!({ "Arn": existing.clone() }))
        .await;
    assert_eq!(
        str_field(&described, "Arn"),
        existing,
        "case existing-after-refusals: the stored app must be untouched, got {described:?}"
    );
}

/// RULE: a create answers a resource in a non-terminal status, and the clock — not the
/// read — owes it the terminal one: `Creating` immediately after the call, `Created` once
/// the due span has passed, with `CreationTime` fixed and `LastModifiedTime` stable
/// across later reads.
#[tokio::test]
async fn an_mlflow_app_advances_from_creating_to_created_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let arn = create(&emulator, "transition", &minimal("transition-app")).await;

    let creating = emulator
        .ok("DescribeMlflowApp", &json!({ "Arn": arn.clone() }))
        .await;
    assert_eq!(
        str_field(&creating, "Status"),
        "Creating",
        "case immediately-after-create: Status, got {creating:?}"
    );

    // A second read before the span elapses still observes the transient status, so the
    // transition follows the clock rather than the number of reads.
    let still_creating = emulator
        .ok("DescribeMlflowApp", &json!({ "Arn": arn.clone() }))
        .await;
    assert_eq!(
        str_field(&still_creating, "Status"),
        "Creating",
        "case second-read-before-the-due-span: Status, got {still_creating:?}"
    );

    tokio::time::sleep(PAST_CREATED_DUE).await;

    let settled = emulator
        .ok("DescribeMlflowApp", &json!({ "Arn": arn.clone() }))
        .await;
    assert_eq!(
        str_field(&settled, "Status"),
        "Created",
        "case after-the-due-span: Status, got {settled:?}"
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
        creating.get("CreationTime"),
        "case after-the-due-span: CreationTime must not move, got {settled:?}"
    );

    let again = emulator
        .ok("DescribeMlflowApp", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        again, settled,
        "case repeated-read: a settled app must read back identically"
    );
}

/// RULE: the store is bounded — the create past its quota is refused with the operation's
/// modelled `ResourceLimitExceeded` and stores nothing, so no caller can grow the
/// emulator's state without end by repeating the call.
#[tokio::test]
async fn a_create_past_the_mlflow_app_quota_is_refused_with_the_modelled_limit_failure() {
    // The bound the handler enforces, read from the code rather than restated, so the
    // test keeps exercising the boundary if the bound changes.
    let quota = service_com_amazonaws_sagemaker_sagemaker::state::MLFLOW_APP_QUOTA;

    let emulator = Emulator::new();
    for index in 0..quota {
        emulator
            .ok("CreateMlflowApp", &minimal(&format!("q{index:04}")))
            .await;
    }

    let (status, parsed) = emulator
        .call("CreateMlflowApp", &minimal("one-past-the-quota"))
        .await;
    assert_eq!(
        status, 400,
        "case one-past-the-quota: a create past the quota must be refused, got {status} {parsed:?}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceLimitExceeded",
        "case one-past-the-quota: the refusal must use the operation's modelled limit \
         error, got {parsed:?}"
    );
    assert!(
        parsed.get("Arn").is_none(),
        "case one-past-the-quota: a refused create mints no ARN, got {parsed:?}"
    );
}
