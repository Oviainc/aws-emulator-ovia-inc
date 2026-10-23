// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's MLflow tracking servers:
//! `CreateMlflowTrackingServer` and the `DescribeMlflowTrackingServer` that reads one
//! back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! create/describe round trip for every documented size and for every optional member
//! present and absent, the ARN form for every signed region and name shape, the
//! documented defaults, the not-found and already-exists refusals, and the
//! `Creating` -> `Created` transition the clock owes.

use serde_json::{json, Value};

// The crate's ONE wire harness, DEFINED and exported by
// `tests/all/inference_deployment_lifecycle.rs` and consumed here: the same
// `MultiServiceRouter` the production router assembles, the signed and unsigned call
// paths, the error-code reader and the account id every emulator ARN carries. Reused
// rather than copied, so this suite drives exactly the dispatch path the others do —
// including its wait constant, since every clock-owed span in this service is the same
// 300 ms and one "long enough to out-wait a settle span" value serves them all.
use super::inference_deployment_lifecycle::{
    error_code, Emulator, ACCOUNT, PAST_IN_SERVICE_DUE as PAST_SETTLE_DUE,
};

/// The two `TrackingServerStatus` members a created server passes through, and the two
/// `IsTrackingServerActive` members, from
/// `smithy_query(query="shape",shape="TrackingServerStatus,IsTrackingServerActive")`.
const STATUS_CREATING: &str = "Creating";
/// See [`STATUS_CREATING`].
const STATUS_CREATED: &str = "Created";
/// See [`STATUS_CREATING`].
const ACTIVE: &str = "Active";
/// See [`STATUS_CREATING`].
const INACTIVE: &str = "Inactive";

/// A role ARN inside `RoleArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`).
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/mlflow-artifacts")
}

/// Call `target` in `region` through the shared harness and require a 200, returning the
/// parsed output. The one helper this suite adds to [`Emulator`]: the shared harness has
/// `call_in` for a region-signed call and `ok` for a `us-east-1` one, and these tests
/// assert the region the ARN carries, so they need both at once.
async fn ok_in(emulator: &Emulator, region: &str, target: &str, body: &Value) -> Value {
    let (status, parsed) = emulator.call_in(region, target, body).await;
    assert_eq!(
        status, 200,
        "case {target} in {region} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// A member of a response body, with JSON `null` read as the absence it stands for.
fn field(body: &Value, member: &str) -> Value {
    match body.get(member) {
        None | Some(Value::Null) => Value::Null,
        Some(value) => value.clone(),
    }
}

/// The output members this emulator never fills, each for a reason the handler states:
/// there is no MLflow UI to address, no Studio user profile behind an IAM-signed call,
/// and no maintenance attempt whose status to report.
const ALWAYS_ABSENT: [&str; 4] = [
    "TrackingServerUrl",
    "CreatedBy",
    "LastModifiedBy",
    "TrackingServerMaintenanceStatus",
];

/// One row of the round-trip table: a create request, and the members
/// `DescribeMlflowTrackingServer` must report for it.
struct RoundTrip {
    case: &'static str,
    region: &'static str,
    create: Value,
    /// Members the describe must report, member name -> value. Any member not listed
    /// here, and not a timestamp or the ARN, must be absent.
    expected: Vec<(&'static str, Value)>,
}

/// RULE: a tracking server reads back exactly what its create stored, with the model's
/// documented default for every member the request left out.
#[tokio::test]
async fn describe_replays_what_create_stored_with_the_documented_defaults() {
    // `TrackingServerSize`'s whole value set, from
    // smithy_query(query="shape",shape="TrackingServerSize"): Small | Medium | Large.
    // Every one of the three is a row below, including the two the issue did not cite.
    //
    // `TrackingServerName` is `len 1..256, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,255}$`,
    // so the rows span a single character, a hyphenated name and both published bounds.
    let longest_name = format!("a{}", "b".repeat(255));
    let rows = vec![
        RoundTrip {
            case: "only-the-required-members",
            region: "us-east-1",
            create: json!({
                "TrackingServerName": "minimal",
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
                "RoleArn": role_arn(),
            }),
            expected: vec![
                ("TrackingServerName", json!("minimal")),
                ("ArtifactStoreUri", json!("s3://artifacts-bucket/mlflow")),
                ("RoleArn", json!(role_arn())),
                // Documented defaults: size "Small", AutomaticModelRegistration False,
                // S3BucketOwnerVerification True, S3BucketOwnerAccountId the caller's.
                ("TrackingServerSize", json!("Small")),
                ("AutomaticModelRegistration", json!(false)),
                ("S3BucketOwnerVerification", json!(true)),
                ("S3BucketOwnerAccountId", json!(ACCOUNT)),
            ],
        },
        RoundTrip {
            case: "every-optional-member-set",
            region: "eu-west-1",
            create: json!({
                "TrackingServerName": "every-member-set",
                "ArtifactStoreUri": "s3://other-bucket/prefix/deeper",
                "RoleArn": role_arn(),
                "TrackingServerSize": "Medium",
                "MlflowVersion": "2.13.2",
                "AutomaticModelRegistration": true,
                "WeeklyMaintenanceWindowStart": "Mon:01:30",
                "S3BucketOwnerAccountId": "210987654321",
                "S3BucketOwnerVerification": false,
                "Tags": [{ "Key": "team", "Value": "ml" }, { "Key": "env", "Value": "test" }],
            }),
            expected: vec![
                ("TrackingServerName", json!("every-member-set")),
                ("ArtifactStoreUri", json!("s3://other-bucket/prefix/deeper")),
                ("RoleArn", json!(role_arn())),
                ("TrackingServerSize", json!("Medium")),
                ("MlflowVersion", json!("2.13.2")),
                ("AutomaticModelRegistration", json!(true)),
                ("WeeklyMaintenanceWindowStart", json!("Mon:01:30")),
                ("S3BucketOwnerAccountId", json!("210987654321")),
                ("S3BucketOwnerVerification", json!(false)),
            ],
        },
        RoundTrip {
            case: "largest-size-shortest-name-https-artifact-store",
            region: "ap-southeast-2",
            create: json!({
                "TrackingServerName": "a",
                // `S3Uri` is `pattern ^(https|s3)://([^/]+)/?(.*)$`, so an https form is
                // as legal an artifact store as an s3 one.
                "ArtifactStoreUri": "https://artifacts-bucket.s3.amazonaws.com/mlflow",
                "RoleArn": role_arn(),
                "TrackingServerSize": "Large",
                // One past nothing: `MlflowVersion` is `pattern ^[0-9]*.[0-9]*.[0-9]*$`.
                "MlflowVersion": "0.0.0",
                "WeeklyMaintenanceWindowStart": "Sun:23:59",
                "Tags": [],
            }),
            expected: vec![
                ("TrackingServerName", json!("a")),
                (
                    "ArtifactStoreUri",
                    json!("https://artifacts-bucket.s3.amazonaws.com/mlflow"),
                ),
                ("RoleArn", json!(role_arn())),
                ("TrackingServerSize", json!("Large")),
                ("MlflowVersion", json!("0.0.0")),
                ("WeeklyMaintenanceWindowStart", json!("Sun:23:59")),
                ("AutomaticModelRegistration", json!(false)),
                ("S3BucketOwnerVerification", json!(true)),
                ("S3BucketOwnerAccountId", json!(ACCOUNT)),
            ],
        },
        RoundTrip {
            case: "name-at-the-256-character-bound",
            region: "us-west-2",
            create: json!({
                "TrackingServerName": longest_name.clone(),
                "ArtifactStoreUri": "s3://bucket",
                "RoleArn": role_arn(),
                "TrackingServerSize": "Small",
            }),
            expected: vec![
                ("TrackingServerName", json!(longest_name.clone())),
                ("ArtifactStoreUri", json!("s3://bucket")),
                ("RoleArn", json!(role_arn())),
                ("TrackingServerSize", json!("Small")),
                ("AutomaticModelRegistration", json!(false)),
                ("S3BucketOwnerVerification", json!(true)),
                ("S3BucketOwnerAccountId", json!(ACCOUNT)),
            ],
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        let created =
            ok_in(&emulator, row.region, "CreateMlflowTrackingServer", &row.create).await;
        let name = row.create["TrackingServerName"]
            .as_str()
            .expect("the row names the tracking server");
        let expected_arn =
            format!("arn:aws:sagemaker:{}:{ACCOUNT}:mlflow-tracking-server/{name}", row.region);
        assert_eq!(
            field(&created, "TrackingServerArn"),
            json!(expected_arn),
            "case {}: TrackingServerArn, got {created:?}",
            row.case
        );

        let described = ok_in(
            &emulator,
            row.region,
            "DescribeMlflowTrackingServer",
            &json!({ "TrackingServerName": name }),
        )
        .await;
        assert_eq!(
            field(&described, "TrackingServerArn"),
            json!(expected_arn),
            "case {}: the describe must report the created ARN, got {described:?}",
            row.case
        );
        for (member, value) in &row.expected {
            assert_eq!(
                field(&described, member),
                *value,
                "case {}: {member}, got {described:?}",
                row.case
            );
        }
        // A member the create never named is absent, never defaulted to a value the
        // model documents no default for.
        let named: Vec<&str> = row.expected.iter().map(|(member, _)| *member).collect();
        for member in ["MlflowVersion", "WeeklyMaintenanceWindowStart"] {
            if !named.contains(&member) {
                assert_eq!(
                    field(&described, member),
                    Value::Null,
                    "case {}: {member} was not asked for, so it must be absent, got {described:?}",
                    row.case
                );
            }
        }
        for member in ALWAYS_ABSENT {
            assert_eq!(
                field(&described, member),
                Value::Null,
                "case {}: {member} must be absent, got {described:?}",
                row.case
            );
        }
        // Both timestamps are reported. `LastModifiedTime` equals `CreationTime` while
        // the server is still `Creating`, and moves to the instant the `Created`
        // transition was due once the clock owes it — never backwards.
        let creation_time = field(&described, "CreationTime")
            .as_f64()
            .unwrap_or_else(|| panic!("case {}: CreationTime, got {described:?}", row.case));
        let last_modified = field(&described, "LastModifiedTime")
            .as_f64()
            .unwrap_or_else(|| panic!("case {}: LastModifiedTime, got {described:?}", row.case));
        assert!(
            last_modified >= creation_time,
            "case {}: LastModifiedTime {last_modified} must not precede CreationTime \
             {creation_time}",
            row.case
        );

        // A freshly created server is `Creating` until the clock owes it `Created`
        // (`state::TRACKING_SERVER_CREATED_AFTER`); whichever of the two this read
        // observes, `IsActive` follows the status, which is the rule here rather than
        // the timing — the transition itself has its own test.
        let status = field(&described, "TrackingServerStatus");
        let expected_active = match status.as_str() {
            Some(STATUS_CREATING) => INACTIVE,
            Some(STATUS_CREATED) => ACTIVE,
            _ => panic!(
                "case {}: TrackingServerStatus must be Creating or Created, got {described:?}",
                row.case
            ),
        };
        assert_eq!(
            field(&described, "IsActive"),
            json!(expected_active),
            "case {}: IsActive must follow TrackingServerStatus {status}, got {described:?}",
            row.case
        );
    }
}

/// RULE: the ARN a create mints and a describe replays names the resource kind, the
/// region the caller signed for (the emulator default when the call is unsigned), the
/// emulator's account and the stored name — and nothing about the request changes that
/// form.
#[tokio::test]
async fn the_minted_arn_names_the_kind_the_signed_region_the_account_and_the_name() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, &str, &str)> = vec![
        ("default-region", "us-east-1", "ts-use1"),
        ("another-partition-region", "eu-central-1", "ts-euc1"),
        ("far-region-hyphenated-name", "ap-northeast-1", "ts-apne-1-server"),
        ("digits-only-name", "us-west-2", "20261022"),
        ("mixed-case-name", "sa-east-1", "TrackingServerOne"),
    ];

    for (case, region, name) in &rows {
        let body = json!({
            "TrackingServerName": name,
            "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
            "RoleArn": role_arn(),
        });
        let created = ok_in(&emulator, region, "CreateMlflowTrackingServer", &body).await;
        let expected = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:mlflow-tracking-server/{name}");
        assert_eq!(
            field(&created, "TrackingServerArn"),
            json!(expected),
            "case {case}: TrackingServerArn, got {created:?}"
        );
        let described = ok_in(
            &emulator,
            region,
            "DescribeMlflowTrackingServer",
            &json!({ "TrackingServerName": name }),
        )
        .await;
        assert_eq!(
            field(&described, "TrackingServerArn"),
            json!(expected),
            "case {case}: the describe must replay the same ARN, got {described:?}"
        );
    }

    // An unsigned call names no region, so the ARN carries the emulator's default.
    let (status, created) = emulator
        .call_unsigned(
            "CreateMlflowTrackingServer",
            &json!({
                "TrackingServerName": "unsigned-server",
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected HTTP 200, got {status} with {created}"
    );
    assert_eq!(
        field(&created, "TrackingServerArn"),
        json!(format!(
            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-tracking-server/unsigned-server"
        )),
        "case unsigned: the ARN must carry the emulator's default region, got {created:?}"
    );
}

/// RULE: a reader refuses a name no create stored, with the modelled `ResourceNotFound`
/// at HTTP 400, and the refusal does not bring the resource into being.
#[tokio::test]
async fn describing_an_unknown_tracking_server_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateMlflowTrackingServer",
            &json!({
                "TrackingServerName": "present",
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
                "RoleArn": role_arn(),
            }),
        )
        .await;

    // Rows: a name never created, one differing from a stored name only in case, one
    // differing by a single character, and the longest legal name.
    let longest = format!("z{}", "y".repeat(255));
    let cases: Vec<(&str, String)> = vec![
        ("never-created", "absent".to_string()),
        ("case-differs-from-a-stored-name", "Present".to_string()),
        ("one-character-off", "presen".to_string()),
        ("longest-legal-name", longest),
    ];

    for (case, name) in &cases {
        let (status, parsed) = emulator
            .call(
                "DescribeMlflowTrackingServer",
                &json!({ "TrackingServerName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case}: expected the modelled ResourceNotFound, got {parsed}"
        );
    }

    // The stored server is untouched by every refusal above.
    let described = emulator
        .ok(
            "DescribeMlflowTrackingServer",
            &json!({ "TrackingServerName": "present" }),
        )
        .await;
    assert_eq!(
        field(&described, "TrackingServerName"),
        json!("present"),
        "a refused read must leave the stored server readable, got {described:?}"
    );
}

/// RULE: a create under a name already in use is refused and keeps the stored server as
/// the first create left it.
#[tokio::test]
async fn creating_a_name_already_in_use_is_refused_and_keeps_the_stored_server() {
    let emulator = Emulator::new();
    let first = json!({
        "TrackingServerName": "taken",
        "ArtifactStoreUri": "s3://first-bucket/mlflow",
        "RoleArn": role_arn(),
        "TrackingServerSize": "Small",
    });
    emulator.ok("CreateMlflowTrackingServer", &first).await;

    // Rows: the same request again, and one differing in every optional member — a
    // name in use is refused however the rest of the request varies.
    let retries: Vec<(&str, Value)> = vec![
        ("identical-request", first.clone()),
        (
            "same-name-different-everything-else",
            json!({
                "TrackingServerName": "taken",
                "ArtifactStoreUri": "s3://second-bucket/other",
                "RoleArn": role_arn(),
                "TrackingServerSize": "Large",
                "MlflowVersion": "2.16.2",
                "AutomaticModelRegistration": true,
                "WeeklyMaintenanceWindowStart": "Tue:03:30",
                "S3BucketOwnerVerification": false,
            }),
        ),
    ];

    for (case, body) in &retries {
        let (status, parsed) = emulator.call("CreateMlflowTrackingServer", body).await;
        assert_eq!(
            status, 400,
            "case {case}: a name in use must be refused, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: expected ValidationException, got {parsed}"
        );
    }

    let described = emulator
        .ok(
            "DescribeMlflowTrackingServer",
            &json!({ "TrackingServerName": "taken" }),
        )
        .await;
    for (member, value) in [
        ("ArtifactStoreUri", json!("s3://first-bucket/mlflow")),
        ("TrackingServerSize", json!("Small")),
        ("AutomaticModelRegistration", json!(false)),
        ("S3BucketOwnerVerification", json!(true)),
    ] {
        assert_eq!(
            field(&described, member),
            value,
            "case refused-retry: {member} must still be what the first create stored, \
             got {described:?}"
        );
    }
    assert_eq!(
        field(&described, "MlflowVersion"),
        Value::Null,
        "case refused-retry: the refused request's members must not be stored, got {described:?}"
    );
}

/// RULE: a resource the service reports in a non-terminal status advances to its settled
/// status once the clock owes the transition — the caller that reads straight after the
/// create still observes the transient status, and a settled resource then reads back
/// identically.
#[tokio::test]
async fn a_tracking_server_advances_from_creating_to_created_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateMlflowTrackingServer",
            &json!({
                "TrackingServerName": "waiter",
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
                "RoleArn": role_arn(),
            }),
        )
        .await;

    let creating = emulator
        .ok(
            "DescribeMlflowTrackingServer",
            &json!({ "TrackingServerName": "waiter" }),
        )
        .await;
    assert_eq!(
        field(&creating, "TrackingServerStatus"),
        json!("Creating"),
        "case immediately-after-create: TrackingServerStatus, got {creating:?}"
    );
    assert_eq!(
        field(&creating, "IsActive"),
        json!("Inactive"),
        "case immediately-after-create: IsActive, got {creating:?}"
    );

    tokio::time::sleep(PAST_SETTLE_DUE).await;

    let settled = emulator
        .ok(
            "DescribeMlflowTrackingServer",
            &json!({ "TrackingServerName": "waiter" }),
        )
        .await;
    assert_eq!(
        field(&settled, "TrackingServerStatus"),
        json!("Created"),
        "case after-the-due-span: TrackingServerStatus, got {settled:?}"
    );
    assert_eq!(
        field(&settled, "IsActive"),
        json!("Active"),
        "case after-the-due-span: IsActive, got {settled:?}"
    );
    let created_at = field(&settled, "CreationTime")
        .as_f64()
        .expect("CreationTime is a timestamp");
    let modified_at = field(&settled, "LastModifiedTime")
        .as_f64()
        .expect("LastModifiedTime is a timestamp");
    assert!(
        modified_at > created_at,
        "case after-the-due-span: LastModifiedTime {modified_at} must move past \
         CreationTime {created_at} when the status changes"
    );
    assert_eq!(
        field(&settled, "CreationTime"),
        field(&creating, "CreationTime"),
        "case after-the-due-span: CreationTime must not move, got {settled:?}"
    );

    let again = emulator
        .ok(
            "DescribeMlflowTrackingServer",
            &json!({ "TrackingServerName": "waiter" }),
        )
        .await;
    assert_eq!(
        again, settled,
        "case repeated-read: a settled tracking server must read back identically"
    );
}

/// RULE: a request outside a member's modelled constraint is refused at the boundary with
/// HTTP 400, before any state is written.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();

    // Rows: one past each published bound these two operations state — name
    // `len 1..256, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,255}$`, `ArtifactStoreUri`
    // `pattern ^(https|s3)://([^/]+)/?(.*)$`, `RoleArn` `len 20..2048, pattern
    // ^arn:aws[a-z\-]*:iam::\d{12}:role/?...$`, `MlflowVersion`
    // `pattern ^[0-9]*.[0-9]*.[0-9]*$`, `WeeklyMaintenanceWindowStart`
    // `pattern ^(Mon|Tue|Wed|Thu|Fri|Sat|Sun):([01]\d|2[0-3]):([0-5]\d)$`,
    // `S3BucketOwnerAccountId` `len 12..12, pattern ^\d+$`, plus the enum value set.
    let too_long_name = "a".repeat(257);
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "name-past-256",
            "CreateMlflowTrackingServer",
            json!({
                "TrackingServerName": too_long_name,
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
                "RoleArn": role_arn(),
            }),
        ),
        (
            "empty-name",
            "CreateMlflowTrackingServer",
            json!({
                "TrackingServerName": "",
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
                "RoleArn": role_arn(),
            }),
        ),
        (
            "name-with-an-illegal-character",
            "CreateMlflowTrackingServer",
            json!({
                "TrackingServerName": "not a server name",
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
                "RoleArn": role_arn(),
            }),
        ),
        (
            "artifact-store-uri-with-an-unsupported-scheme",
            "CreateMlflowTrackingServer",
            json!({
                "TrackingServerName": "bad-uri",
                "ArtifactStoreUri": "ftp://artifacts-bucket/mlflow",
                "RoleArn": role_arn(),
            }),
        ),
        (
            "role-arn-that-is-not-an-iam-role",
            "CreateMlflowTrackingServer",
            json!({
                "TrackingServerName": "bad-role",
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
                "RoleArn": format!("arn:aws:s3:::{ACCOUNT}-bucket"),
            }),
        ),
        (
            "size-outside-the-enum",
            "CreateMlflowTrackingServer",
            json!({
                "TrackingServerName": "bad-size",
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
                "RoleArn": role_arn(),
                "TrackingServerSize": "ExtraLarge",
            }),
        ),
        (
            "maintenance-window-hour-past-23",
            "CreateMlflowTrackingServer",
            json!({
                "TrackingServerName": "bad-window",
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
                "RoleArn": role_arn(),
                "WeeklyMaintenanceWindowStart": "Mon:24:00",
            }),
        ),
        (
            "account-id-shorter-than-12-digits",
            "CreateMlflowTrackingServer",
            json!({
                "TrackingServerName": "bad-account",
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
                "RoleArn": role_arn(),
                "S3BucketOwnerAccountId": "12345678901",
            }),
        ),
        (
            "missing-the-required-artifact-store",
            "CreateMlflowTrackingServer",
            json!({ "TrackingServerName": "no-store", "RoleArn": role_arn() }),
        ),
        (
            "missing-the-required-role",
            "CreateMlflowTrackingServer",
            json!({
                "TrackingServerName": "no-role",
                "ArtifactStoreUri": "s3://artifacts-bucket/mlflow",
            }),
        ),
        (
            "describe-with-an-illegal-name",
            "DescribeMlflowTrackingServer",
            json!({ "TrackingServerName": "not a server name" }),
        ),
    ];

    for (case, target, body) in &cases {
        let (status, parsed) = emulator.call(target, body).await;
        assert_eq!(
            status, 400,
            "case {case}: {target} must be refused, got {status} with {parsed}"
        );
        // The code as well as the status: a constraint refusal is a `ValidationException`,
        // so a row answered by some other 400 (a `ResourceNotFound` for the describe row,
        // say, which would mean the illegal name reached the store) fails here.
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: {target} must be refused as a constraint violation, got {parsed}"
        );
    }

    // Nothing above reached state: every name a refused create named is still unknown.
    for name in [
        "bad-uri",
        "bad-role",
        "bad-size",
        "bad-window",
        "bad-account",
        "no-store",
        "no-role",
    ] {
        let (status, parsed) = emulator
            .call(
                "DescribeMlflowTrackingServer",
                &json!({ "TrackingServerName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name}: a refused create must write nothing, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {name}: a refused create must write nothing, got {parsed}"
        );
    }
}

/// RULE: each tracking server is its own record, so creating one never changes what a
/// sibling reports.
#[tokio::test]
async fn servers_created_side_by_side_keep_their_own_members() {
    let emulator = Emulator::new();

    // Rows differing along size, artifact store, version, registration and verification:
    // the whole set of members a create can vary, over several resources at once.
    let rows: Vec<(&str, Value)> = vec![
        (
            "small",
            json!({
                "TrackingServerName": "small",
                "ArtifactStoreUri": "s3://bucket-a/one",
                "RoleArn": role_arn(),
                "TrackingServerSize": "Small",
                "MlflowVersion": "2.13.2",
            }),
        ),
        (
            "medium",
            json!({
                "TrackingServerName": "medium",
                "ArtifactStoreUri": "s3://bucket-b/two",
                "RoleArn": role_arn(),
                "TrackingServerSize": "Medium",
                "AutomaticModelRegistration": true,
            }),
        ),
        (
            "large",
            json!({
                "TrackingServerName": "large",
                "ArtifactStoreUri": "s3://bucket-c/three",
                "RoleArn": role_arn(),
                "TrackingServerSize": "Large",
                "S3BucketOwnerVerification": false,
            }),
        ),
    ];

    for (_, body) in &rows {
        emulator.ok("CreateMlflowTrackingServer", body).await;
    }

    for (case, body) in &rows {
        let name = body["TrackingServerName"].as_str().expect("a name");
        let described = emulator
            .ok(
                "DescribeMlflowTrackingServer",
                &json!({ "TrackingServerName": name }),
            )
            .await;
        for member in [
            "ArtifactStoreUri",
            "TrackingServerSize",
            "MlflowVersion",
            "AutomaticModelRegistration",
            "S3BucketOwnerVerification",
        ] {
            let expected = match body.get(member) {
                Some(value) => value.clone(),
                // The members this row left out take the documented default, or stay
                // absent where the model documents none.
                None => match member {
                    "AutomaticModelRegistration" => json!(false),
                    "S3BucketOwnerVerification" => json!(true),
                    _ => Value::Null,
                },
            };
            assert_eq!(
                field(&described, member),
                expected,
                "case {case}: {member} must be this server's own value, got {described:?}"
            );
        }
    }
}
