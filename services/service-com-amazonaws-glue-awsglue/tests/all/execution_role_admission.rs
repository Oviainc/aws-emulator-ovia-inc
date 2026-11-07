// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ADMISSION of a caller-supplied execution role on the writes
//! that are not a crawler's: `CreateColumnStatisticsTaskSettings`, `CreateMLTransform`, the
//! `UpdateMLTransform` that replaces a transform's role, and `CreateSession`.
//!
//! The rule under test: a resource that Glue itself RUNS AS a caller-supplied `Role` — the
//! column statistics task settings of a table, a machine learning transform, an interactive
//! session — is registered only for a role this account's own IAM shows Glue can assume, so
//! the write resolves that role before it judges anything else about the request and refuses
//! it with `InvalidInputException` (HTTP 400), persisting nothing, when IAM ANSWERED that
//! Glue cannot assume it (this account holds no such role, or the trust document the role
//! carries does not admit the Glue service principal). Each operation words that refusal the
//! way the service words it, so the three sentences are asserted separately. A role nothing
//! can be shown about is not refused: an emulator with no IAM to ask refuses nothing, which
//! is why every other suite in this crate, which mounts Glue alone, still creates these
//! resources with roles no IAM record backs.
//!
//! The crawler writes are held to the same resolution by
//! [`crate::crawler_role_trust`], whose harness this suite reuses.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! operation is asked, what the role's trust document says (which principal, which action,
//! which effect, how many statements, whether it is a policy at all), whether the account
//! holds the role, and the form the member names it by (an ARN of this account, a bare role
//! name, another account's ARN). Every call goes over the wire through the shared
//! [`crate::wire`] harness, so what is asserted is HTTP behaviour and not a direct call into
//! a handler.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::GLUE_SERVICE_PRINCIPAL;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{role_arn, DEFAULT_PATH};
use smithy_server_runtime::MultiServiceRouter;

use crate::crawler_role_trust::{emulator_with_iam, glue_trust, seed_role, trust};
use crate::error_classification::without_diagnostic;
use crate::wire::{call, error_code, error_message, ok, ACCOUNT_ID};

/// The whole `Message` the column statistics task settings create refuses an un-assumable
/// role with — the service's fixed template for that check, written out here rather than
/// read from the crate's own constant so this suite pins the wording.
const ROLE_NOT_ASSUMABLE: &str =
    "Service is unable to assume provided role. Please verify role's TrustPolicy";

/// The component the session refusal's diagnostic clause names, and the status and code it
/// states: the clause is appended by the component that provisions the session.
const SESSION_DIAGNOSTIC_SERVICE: &str = "AWSGlueJobExecutor";
const SESSION_DIAGNOSTIC_STATUS: u16 = 400;
const SESSION_DIAGNOSTIC_CODE: &str = "InvalidInputException";

/// Which write a row asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Write {
    /// `CreateColumnStatisticsTaskSettings`, whose `Role` "is the role used for running the
    /// column statistics".
    ColumnStatisticsTaskSettings,
    /// `CreateMLTransform`, whose `Role` is what its task runs are performed as.
    MlTransform,
    /// `CreateSession`, whose `Role` the remote runtime environment is provisioned as.
    Session,
}

impl Write {
    /// The operation name on the wire.
    fn operation(self) -> &'static str {
        match self {
            Self::ColumnStatisticsTaskSettings => "CreateColumnStatisticsTaskSettings",
            Self::MlTransform => "CreateMLTransform",
            Self::Session => "CreateSession",
        }
    }

    /// The request body this write sends for the resource named `name` with `role`.
    fn body(self, name: &str, role: &str) -> Value {
        match self {
            Self::ColumnStatisticsTaskSettings => json!({
                "DatabaseName": DATABASE,
                "TableName": name,
                "Role": role,
            }),
            Self::MlTransform => json!({
                "Name": name,
                "InputRecordTables": [],
                "Parameters": {
                    "TransformType": "FIND_MATCHES",
                    "FindMatchesParameters": { "PrimaryKeyColumnName": "id" },
                },
                "Role": role,
            }),
            Self::Session => json!({
                "Id": name,
                "Role": role,
                "Command": { "Name": "glueetl" },
            }),
        }
    }
}

/// The database every column statistics row addresses its table in.
const DATABASE: &str = "role_admission";

/// The ARN that addresses the role a `Role` member names: the value as sent when it already
/// names an ARN, and the ARN of a role of this account when it names a bare name. This is the
/// reading the refusals that NAME the role report it by.
fn role_arn_of(role: &str) -> String {
    if role.starts_with("arn:") {
        return role.to_string();
    }
    role_arn(DEFAULT_PATH, role)
}

/// Assert the refusal `body` carries the `Message` the write `write` words this refusal with
/// for `role`, and nothing else.
fn assert_refusal_message(case: &str, write: Write, role: &str, body: &Value) {
    let message = error_message(body);
    match write {
        // The crawler's constant, shared with this create: it names neither the role nor the
        // operation.
        Write::ColumnStatisticsTaskSettings => assert_eq!(
            message, ROLE_NOT_ASSUMABLE,
            "case {case:?}: expected Message {ROLE_NOT_ASSUMABLE:?}, got {message:?}"
        ),
        // The transform surface states the ARN that addresses the role.
        Write::MlTransform => {
            let expected = format!("Unable to assume role for arn: {}", role_arn_of(role));
            assert_eq!(
                message, expected,
                "case {case:?}: expected Message {expected:?}, got {message:?}"
            );
        }
        // The session surface states the requirement and carries the component's own
        // diagnostic clause, whose request id is the one per-run field.
        Write::Session => {
            let sentence = without_diagnostic(
                case,
                &message,
                SESSION_DIAGNOSTIC_SERVICE,
                SESSION_DIAGNOSTIC_STATUS,
                SESSION_DIAGNOSTIC_CODE,
            );
            let expected = format!(
                "Role {} should be given assume role permissions for Glue Service.\n",
                role_arn_of(role)
            );
            assert_eq!(
                sentence, expected,
                "case {case:?}: expected the sentence {expected:?} in front of the clause, got \
                 {sentence:?}"
            );
        }
    }
}

/// Assert that `name` names no resource of the write `write`: a refused create must have
/// persisted nothing.
async fn assert_absent(router: &MultiServiceRouter, case: &str, write: Write, name: &str) {
    match write {
        Write::ColumnStatisticsTaskSettings => {
            let (status, body) = call(
                router,
                "GetColumnStatisticsTaskSettings",
                &json!({ "DatabaseName": DATABASE, "TableName": name }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a refused create must store no settings, but the read answered \
                 {status} with {body}"
            );
            assert!(
                error_code(&body).contains("EntityNotFoundException"),
                "case {case:?}: expected the settings to be absent, got {body}"
            );
        }
        Write::MlTransform => {
            let listed = ok(router, "GetMLTransforms", &json!({})).await;
            let transforms = listed
                .get("Transforms")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                !transforms
                    .iter()
                    .any(|transform| transform.get("Name") == Some(&json!(name))),
                "case {case:?}: a refused create must store no transform, but GetMLTransforms \
                 reported {listed}"
            );
        }
        Write::Session => {
            let (status, body) = call(router, "GetSession", &json!({ "Id": name })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a refused create must store no session, but GetSession answered \
                 {status} with {body}"
            );
            assert!(
                error_code(&body).contains("EntityNotFoundException"),
                "case {case:?}: expected the session to be absent, got {body}"
            );
        }
    }
}

/// Store the database and the table the column statistics rows address, so an admitted
/// create has a table to hang its settings on.
async fn store_table(router: &MultiServiceRouter, table: &str) {
    let (status, body) = call(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    assert!(
        status == 200 || error_code(&body).ends_with("AlreadyExistsException"),
        "CreateDatabase must answer 200 or report the database is already stored, got {status} \
         with {body}"
    );
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": DATABASE,
            "TableInput": {
                "Name": table,
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Location": "s3://bucket/data/",
                    "Columns": [{ "Name": "id", "Type": "string" }],
                },
            },
        }),
    )
    .await;
}

/// What one row asks of the role resolution: the trust document the account holds for the
/// row's role, and whether the write must be admitted.
struct Row {
    /// The case name every assertion of the row reports.
    case: &'static str,
    /// The role the account holds under the row's role name, as its trust document — or
    /// `None` for a row whose role the account does not hold at all.
    held: Option<Option<String>>,
    /// Whether the write must be admitted.
    admitted: bool,
}

/// The rows every one of the three writes is put through.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a trust document admitting the Glue principal",
            held: Some(Some(glue_trust())),
            admitted: true,
        },
        Row {
            case: "a service wildcard in the action",
            held: Some(Some(trust("Allow", GLUE_SERVICE_PRINCIPAL, "sts:*"))),
            admitted: true,
        },
        Row {
            case: "the principal and the action named in another casing",
            held: Some(Some(trust("Allow", "GLUE.amazonaws.com", "STS:AssumeRole"))),
            admitted: true,
        },
        Row {
            case: "a document whose second statement admits the Glue principal",
            held: Some(Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": "ec2.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        },
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": GLUE_SERVICE_PRINCIPAL },
                            "Action": "sts:AssumeRole",
                        },
                    ],
                })
                .to_string(),
            )),
            admitted: true,
        },
        Row {
            case: "a trust document admitting another service only",
            held: Some(Some(trust(
                "Allow",
                "lambda.amazonaws.com",
                "sts:AssumeRole",
            ))),
            admitted: false,
        },
        Row {
            case: "a trust document naming another action",
            held: Some(Some(trust("Allow", GLUE_SERVICE_PRINCIPAL, "s3:GetObject"))),
            admitted: false,
        },
        Row {
            case: "a trust document explicitly denying the Glue principal",
            held: Some(Some(trust("Deny", GLUE_SERVICE_PRINCIPAL, "sts:AssumeRole"))),
            admitted: false,
        },
        Row {
            case: "a trust document carrying no statement",
            held: Some(Some(json!({ "Version": "2012-10-17" }).to_string())),
            admitted: false,
        },
        Row {
            case: "a trust document that is not a policy at all",
            held: Some(Some("this is not a policy".to_string())),
            admitted: false,
        },
        Row {
            case: "a role carrying no trust document",
            held: Some(None),
            admitted: false,
        },
        Row {
            case: "a role this account does not hold",
            held: None,
            admitted: false,
        },
    ]
}

/// Every form of trust document, and every absence of one, decides whether each of the three
/// writes is admitted with that role — and a refusal persists nothing.
#[tokio::test]
async fn a_resource_that_runs_as_a_role_is_created_only_for_one_iam_shows_glue_can_assume() {
    for write in [
        Write::ColumnStatisticsTaskSettings,
        Write::MlTransform,
        Write::Session,
    ] {
        let (router, engine) = emulator_with_iam();
        for (index, row) in rows().iter().enumerate() {
            let case = format!("{} / {}", write.operation(), row.case);
            let role_name = format!("admission-role-{index}-{:?}", write);
            let name = format!("admission_{index}");
            let role = match &row.held {
                Some(trust) => seed_role(&engine, &role_name, trust.clone()),
                // Not seeded, so IAM answers that this account holds no such role. The ARN
                // is the one IAM would hold it under.
                None => role_arn(DEFAULT_PATH, &role_name),
            };
            if write == Write::ColumnStatisticsTaskSettings {
                store_table(&router, &name).await;
            }

            let (status, body) = call(&router, write.operation(), &write.body(&name, &role)).await;
            if row.admitted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the write to be admitted, got {status} with {body}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case:?}: expected the write to be refused, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("InvalidInputException"),
                "case {case:?}: expected InvalidInputException, got {body}"
            );
            assert_refusal_message(&case, write, &role, &body);
            assert_absent(&router, &case, write, &name).await;
        }
    }
}

/// The same decision, over the forms a `Role` member may name a role by, for each write that
/// accepts a bare name as well as an ARN.
///
/// `CreateMLTransform$Role` is "the name or Amazon Resource Name (ARN) of the IAM role"
/// (`smithy_query operation=CreateMLTransform`) and
/// `CreateColumnStatisticsTaskSettings$Role` is a `NameString`, so a bare name and this
/// account's role ARN name the same role and are judged alike — while an ARN of ANOTHER
/// account names a role this emulator's IAM cannot be asked about, so nothing is shown and
/// nothing is refused. `CreateSession$Role` is modelled as an ARN outright
/// (`^arn:aws[^:]*:iam::[0-9]*:role/.+$`), so only its ARN forms are rowed.
#[tokio::test]
async fn the_role_member_is_resolved_whether_it_names_a_role_or_its_arn() {
    for write in [
        Write::ColumnStatisticsTaskSettings,
        Write::MlTransform,
        Write::Session,
    ] {
        let (router, engine) = emulator_with_iam();
        seed_role(&engine, "form-glue-role", Some(glue_trust()));
        seed_role(
            &engine,
            "form-ec2-role",
            Some(trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole")),
        );

        // (case, the `Role` value, whether the write must be admitted)
        let mut forms: Vec<(&str, String, bool)> = vec![
            (
                "this account's ARN of a role trusting Glue",
                role_arn(DEFAULT_PATH, "form-glue-role"),
                true,
            ),
            (
                "the same role under a service-role path, which IAM holds by name",
                format!("arn:aws:iam::{ACCOUNT_ID}:role/service-role/form-glue-role"),
                true,
            ),
            (
                "this account's ARN of a role not trusting Glue",
                role_arn(DEFAULT_PATH, "form-ec2-role"),
                false,
            ),
            (
                "this account's ARN of a role it does not hold",
                role_arn(DEFAULT_PATH, "form-absent-role"),
                false,
            ),
            (
                // Another account's IAM is not this emulator's to ask, so nothing is shown
                // about the role and the write goes ahead.
                "another account's role ARN, which this IAM cannot be asked about",
                "arn:aws:iam::123456789012:role/their-glue-role".to_string(),
                true,
            ),
        ];
        if write != Write::Session {
            forms.push((
                "a bare role name, held and trusting Glue",
                "form-glue-role".to_string(),
                true,
            ));
            forms.push((
                "a bare role name, held but not trusting Glue",
                "form-ec2-role".to_string(),
                false,
            ));
            forms.push((
                "a bare role name this account holds no role under",
                "form-absent-role".to_string(),
                false,
            ));
        }

        for (index, (form, role, admitted)) in forms.iter().enumerate() {
            let case = format!("{} / {form}", write.operation());
            let name = format!("form_{index}");
            if write == Write::ColumnStatisticsTaskSettings {
                store_table(&router, &name).await;
            }

            let (status, body) = call(&router, write.operation(), &write.body(&name, role)).await;
            if *admitted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the write to be admitted, got {status} with {body}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case:?}: expected the write to be refused, got {status} with {body}"
            );
            assert_refusal_message(&case, write, role, &body);
            assert_absent(&router, &case, write, &name).await;
        }
    }
}

/// The role is resolved BEFORE the request's other members, so a request that is wrong in two
/// ways at once reports the role — and the same request with an assumable role is still
/// refused for that other member, which is what makes this about precedence rather than
/// about the other check having gone away.
///
/// The rows differ along which other member is also wrong, and on which write.
#[tokio::test]
async fn the_role_is_judged_before_the_other_members_of_the_request() {
    let (router, engine) = emulator_with_iam();
    let assumable = seed_role(&engine, "ordering-glue-role", Some(glue_trust()));
    let unassumable = seed_role(
        &engine,
        "ordering-ec2-role",
        Some(trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole")),
    );
    store_table(&router, "ordering_table").await;

    // (case, the write, the body the row sends beside `Role`, the Message the operation
    // answers when the role IS assumable)
    let rows: Vec<(&str, Write, Value, &str)> = vec![
        (
            "settings for a table the catalog does not hold",
            Write::ColumnStatisticsTaskSettings,
            json!({ "DatabaseName": DATABASE, "TableName": "no_such_table" }),
            "Table passed as input within input database not found.",
        ),
        (
            "settings whose Schedule is no cron expression",
            Write::ColumnStatisticsTaskSettings,
            json!({
                "DatabaseName": DATABASE,
                "TableName": "ordering_table",
                "Schedule": "every monday",
            }),
            "Schedule every monday is not a cron expression of the form \
             cron(<minutes> <hours> <day-of-month> <month> <day-of-week> <year>)",
        ),
        (
            "a session whose GlueVersion names no runtime",
            Write::Session,
            json!({ "Id": "ordering_session", "Command": { "Name": "glueetl" }, "GlueVersion": "1.0" }),
            "GlueVersion 1.0 is not supported for sessions, whose GlueVersion must be greater \
             than 2.0",
        ),
    ];

    for (case, write, members, other_message) in rows {
        // With the un-assumable role, the ROLE is what the refusal reports.
        let mut request = members.clone();
        request["Role"] = json!(unassumable);
        let (status, body) = call(&router, write.operation(), &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the write to be refused, got {status} with {body}"
        );
        assert_refusal_message(case, write, &unassumable, &body);

        // With the assumable role, the SAME request is refused for the other member.
        let mut request = members.clone();
        request["Role"] = json!(assumable);
        let (status, body) = call(&router, write.operation(), &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the write to be refused for the other member, got {status} \
             with {body}"
        );
        assert_eq!(
            error_message(&body),
            other_message,
            "case {case:?}: expected Message {other_message:?}, got {:?}",
            error_message(&body)
        );
    }
}

/// `UpdateMLTransform` resolves a `Role` it would REPLACE the stored one with the same way
/// the create does: an un-assumable role is refused and the stored transform keeps the role
/// it had, while an update that names no role at all is untouched by the resolution.
///
/// The rows differ along which role the update names and whether it names one at all.
#[tokio::test]
async fn an_ml_transform_update_that_replaces_the_role_is_held_to_the_same_resolution() {
    let (router, engine) = emulator_with_iam();
    let assumable = seed_role(&engine, "update-glue-role", Some(glue_trust()));
    let other_assumable = seed_role(&engine, "update-glue-role-two", Some(glue_trust()));
    let unassumable = seed_role(
        &engine,
        "update-ec2-role",
        Some(trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole")),
    );
    let absent = role_arn(DEFAULT_PATH, "update-absent-role");

    // (case, the members the update carries beside `TransformId`, the role the transform must
    // carry afterwards)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a replacement role Glue can assume",
            json!({ "Role": other_assumable }),
            other_assumable.clone(),
        ),
        (
            "a replacement role Glue cannot assume",
            json!({ "Role": unassumable }),
            assumable.clone(),
        ),
        (
            "a replacement role this account does not hold",
            json!({ "Role": absent }),
            assumable.clone(),
        ),
        (
            "an update that names no role",
            json!({ "Description": "changed" }),
            assumable.clone(),
        ),
    ];

    for (index, (case, members, expected_role)) in rows.iter().enumerate() {
        let name = format!("update_transform_{index}");
        let created = ok(
            &router,
            "CreateMLTransform",
            &Write::MlTransform.body(&name, &assumable),
        )
        .await;
        let transform_id = created["TransformId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create must answer a TransformId"))
            .to_string();

        let mut request = members.clone();
        request["TransformId"] = json!(transform_id);
        let (status, body) = call(&router, "UpdateMLTransform", &request).await;
        let refused = *expected_role == assumable && members.get("Role").is_some();
        if refused {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the update to be refused, got {status} with {body}"
            );
            let role = members["Role"].as_str().expect("the row names a role");
            assert_refusal_message(case, Write::MlTransform, role, &body);
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be admitted, got {status} with {body}"
            );
        }

        let read = ok(
            &router,
            "GetMLTransform",
            &json!({ "TransformId": transform_id }),
        )
        .await;
        assert_eq!(
            read["Role"],
            json!(expected_role),
            "case {case:?}: expected the stored transform to carry {expected_role:?}, got {read}"
        );
    }
}
