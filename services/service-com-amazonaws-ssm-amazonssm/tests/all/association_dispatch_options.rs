// SPDX-License-Identifier: CC0-1.0

//! The dispatch options a State Manager association carries, end to end: the CloudWatch
//! alarm configuration applied to its runs (`AlarmConfiguration`), the accounts and Regions
//! it is dispatched to (`TargetLocations`) and the role it is dispatched under
//! (`AssociationDispatchAssumeRole`).
//!
//! These are the members of an association that [`crate::state_manager_associations`] does
//! not span, pinned here against the one rule every optional member of an association
//! follows: a member a create supplies is held exactly as supplied and reported by
//! `DescribeAssociation`, a member it omits is reported by neither, a member modelled with a
//! default is reported at that default, and a value outside a member's modelled constraint
//! is refused before anything is persisted.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router — through the harness that suite publishes, so the two suites share
//! one dispatch and one reading of the store.
//!
//! The suites are tables. Each row is one form these members take — alarm polling failing or
//! being ignored, one target location or several, excluded accounts, child organizational
//! units, a dispatch role, every option at once and none of them — rather than the single
//! create a recording happened to make.

use serde_json::{json, Value};

use crate::state_manager_associations::{
    create, created, described, error_code, service, stored_associations, without_overview, AGENT,
    NODE, OTHER_REGION, REGION,
};

/// Another account, to span a dispatch that reaches beyond the account the request was
/// signed for; 12 digits, as the modelled `ExcludeAccount` pattern spells an account.
const OTHER_ACCOUNT: &str = "210987654321";

/// An organizational unit, of the other form that same pattern admits.
const ORGANIZATIONAL_UNIT: &str = "ou-1234-56789012";

/// The value at `path`, a JSON pointer into the description, or `Value::Null` for a member
/// the description does not carry.
fn at(description: &Value, path: &str) -> Value {
    description.pointer(path).cloned().unwrap_or(Value::Null)
}

/// One create, and what the association it persists reports for its dispatch options.
struct Row {
    case: &'static str,
    /// The dispatch members the create carries, merged onto a create of [`AGENT`].
    members: Value,
    /// `(JSON pointer, value)` pairs the association reports.
    echoed: Vec<(&'static str, Value)>,
    /// Pointers at which the association reports nothing, because nothing set them.
    unset: Vec<&'static str>,
    /// Pointers of booleans the create left at the default its member is modelled with,
    /// which the read reports AT that default (`false`) rather than leaving out.
    defaulted: Vec<&'static str>,
}

/// One row per form the dispatch options take.
fn rows() -> Vec<Row> {
    let account = engine::emulator_account_id();
    let dispatch_role = format!("arn:aws:iam::{account}:role/StateManagerDispatch");
    vec![
        Row {
            case: "none of the dispatch options",
            members: json!({}),
            echoed: vec![],
            unset: vec![
                "/AlarmConfiguration",
                "/TargetLocations",
                "/AssociationDispatchAssumeRole",
                "/TriggeredAlarms",
            ],
            defaulted: vec![],
        },
        Row {
            case: "an alarm configuration whose poll failure stops the run",
            members: json!({"AlarmConfiguration": {"Alarms": [{"Name": "association-errors"}]}}),
            echoed: vec![(
                "/AlarmConfiguration/Alarms/0/Name",
                json!("association-errors"),
            )],
            unset: vec![
                "/TargetLocations",
                "/AssociationDispatchAssumeRole",
                // "the alarms that were invoked by the association" — nothing has run this
                // association, so no alarm has been invoked.
                "/TriggeredAlarms",
            ],
            defaulted: vec!["/AlarmConfiguration/IgnorePollAlarmFailure"],
        },
        Row {
            case: "an alarm configuration that keeps running when the alarm cannot be polled",
            members: json!({"AlarmConfiguration": {
                "IgnorePollAlarmFailure": true,
                "Alarms": [{"Name": "association-errors"}],
            }}),
            echoed: vec![
                ("/AlarmConfiguration/IgnorePollAlarmFailure", json!(true)),
                (
                    "/AlarmConfiguration/Alarms/0/Name",
                    json!("association-errors"),
                ),
            ],
            unset: vec!["/TargetLocations", "/AssociationDispatchAssumeRole"],
            defaulted: vec![],
        },
        Row {
            case: "one target location, with its own role and its own limits",
            members: json!({"TargetLocations": [{
                "Accounts": [account],
                "Regions": [REGION, OTHER_REGION],
                "TargetLocationMaxConcurrency": "2",
                "TargetLocationMaxErrors": "1",
                "ExecutionRoleName": "AWS-SystemsManager-AutomationExecutionRole",
            }]}),
            echoed: vec![
                ("/TargetLocations/0/Accounts", json!([account])),
                ("/TargetLocations/0/Regions", json!([REGION, OTHER_REGION])),
                ("/TargetLocations/0/TargetLocationMaxConcurrency", json!("2")),
                ("/TargetLocations/0/TargetLocationMaxErrors", json!("1")),
                (
                    "/TargetLocations/0/ExecutionRoleName",
                    json!("AWS-SystemsManager-AutomationExecutionRole"),
                ),
            ],
            unset: vec![
                "/TargetLocations/1",
                "/TargetLocations/0/ExcludeAccounts",
                "/AlarmConfiguration",
                "/AssociationDispatchAssumeRole",
            ],
            defaulted: vec!["/TargetLocations/0/IncludeChildOrganizationUnits"],
        },
        Row {
            case: "two target locations, the second with child OUs and excluded accounts",
            members: json!({"TargetLocations": [
                {"Regions": [REGION]},
                {
                    "Accounts": [OTHER_ACCOUNT],
                    "Regions": [OTHER_REGION],
                    "IncludeChildOrganizationUnits": true,
                    "ExcludeAccounts": [ORGANIZATIONAL_UNIT, OTHER_ACCOUNT],
                },
            ]}),
            echoed: vec![
                ("/TargetLocations/0/Regions", json!([REGION])),
                ("/TargetLocations/1/Accounts", json!([OTHER_ACCOUNT])),
                ("/TargetLocations/1/Regions", json!([OTHER_REGION])),
                (
                    "/TargetLocations/1/IncludeChildOrganizationUnits",
                    json!(true),
                ),
                (
                    "/TargetLocations/1/ExcludeAccounts",
                    json!([ORGANIZATIONAL_UNIT, OTHER_ACCOUNT]),
                ),
            ],
            unset: vec![
                "/TargetLocations/2",
                "/TargetLocations/0/Accounts",
                "/AlarmConfiguration",
            ],
            defaulted: vec!["/TargetLocations/0/IncludeChildOrganizationUnits"],
        },
        Row {
            case: "a dispatch role the association is dispatched under",
            members: json!({"AssociationDispatchAssumeRole": dispatch_role.clone()}),
            echoed: vec![(
                "/AssociationDispatchAssumeRole",
                json!(dispatch_role.clone()),
            )],
            unset: vec!["/AlarmConfiguration", "/TargetLocations"],
            defaulted: vec![],
        },
        Row {
            case: "every dispatch option at once",
            members: json!({
                "AssociationDispatchAssumeRole": dispatch_role.clone(),
                "AlarmConfiguration": {"Alarms": [{"Name": "every-option"}]},
                "TargetLocations": [{
                    "Accounts": [account, OTHER_ACCOUNT],
                    "Regions": [OTHER_REGION],
                    "TargetsMaxConcurrency": "100%",
                    "TargetsMaxErrors": "0",
                }],
            }),
            echoed: vec![
                ("/AssociationDispatchAssumeRole", json!(dispatch_role)),
                ("/AlarmConfiguration/Alarms/0/Name", json!("every-option")),
                (
                    "/TargetLocations/0/Accounts",
                    json!([account, OTHER_ACCOUNT]),
                ),
                ("/TargetLocations/0/TargetsMaxConcurrency", json!("100%")),
                ("/TargetLocations/0/TargetsMaxErrors", json!("0")),
            ],
            unset: vec!["/TargetLocations/0/ExecutionRoleName", "/TriggeredAlarms"],
            defaulted: vec!["/AlarmConfiguration/IgnorePollAlarmFailure"],
        },
    ]
}

/// A create holds the dispatch options it was given, answers with what it held, and the
/// reader reports those same options back — whatever form they took.
#[tokio::test]
async fn the_dispatch_options_a_create_holds_are_the_ones_the_reader_reports() {
    for row in rows() {
        let (router, engine) = service();
        let mut request =
            json!({"Name": AGENT, "Targets": [{"Key": "InstanceIds", "Values": [NODE]}]});
        for (member, value) in row
            .members
            .as_object()
            .expect("the row's members are an object")
        {
            request[member] = value.clone();
        }

        let created = created(&router, row.case, &request).await;
        for (path, wanted) in &row.echoed {
            assert_eq!(
                &at(&created, path),
                wanted,
                "case {:?}: expected the create to report {wanted:?} at {path}, got {:?}",
                row.case,
                at(&created, path)
            );
        }
        for path in &row.unset {
            assert_eq!(
                at(&created, path),
                Value::Null,
                "case {:?}: expected nothing at {path}, got {:?}",
                row.case,
                at(&created, path)
            );
        }
        for path in &row.defaulted {
            assert_eq!(
                at(&created, path),
                json!(false),
                "case {:?}: expected the modelled default at {path}, got {:?}",
                row.case,
                at(&created, path)
            );
        }
        assert_eq!(
            stored_associations(&engine),
            1,
            "case {:?}: the create persists exactly one association",
            row.case
        );

        // The round trip: the reader reports the association the create persisted, by its
        // id and at the one version the create minted.
        let association_id = created["AssociationId"]
            .as_str()
            .unwrap_or("<unset>")
            .to_string();
        for selector in [None, Some("$LATEST"), Some("1")] {
            let mut read_request = json!({"AssociationId": association_id});
            if let Some(selector) = selector {
                read_request["AssociationVersion"] = json!(selector);
            }
            let read = described(&router, row.case, &read_request).await;
            assert_eq!(
                without_overview(&read),
                without_overview(&created),
                "case {:?}: a read at version {selector:?} reports the dispatch options the \
                 create persisted, got {read:?}",
                row.case
            );
        }
    }
}

/// A dispatch option outside its modelled constraint fails the whole create before anything
/// is persisted.
#[tokio::test]
async fn a_dispatch_option_outside_its_modelled_constraint_is_refused() {
    let (router, engine) = service();

    // One row per constraint the model states for these members: `AlarmList` holds exactly
    // one alarm (length 1..1), `AlarmName` is not blank (pattern ^(?!\s*$).+$),
    // `AssociationDispatchAssumeRoleArn` is an IAM role ARN (pattern
    // ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$), `TargetLocations` holds at least one
    // location (length 1..100), `Accounts` and `Regions` at least one entry each (length
    // 1..50), `ExcludeAccount` is an account or an organizational unit (pattern
    // ^((ou-[a-z0-9]{4,32}-[a-z0-9]{8,32})|(\d{12}))$) and `MaxConcurrency` is a count or a
    // percentage (pattern ^([1-9][0-9]*|[1-9][0-9]%|[1-9]%|100%)$).
    let rows: Vec<(&str, Value)> = vec![
        (
            "an alarm configuration naming two alarms",
            json!({"AlarmConfiguration": {"Alarms": [{"Name": "one"}, {"Name": "two"}]}}),
        ),
        (
            "an alarm configuration naming no alarm",
            json!({"AlarmConfiguration": {"Alarms": []}}),
        ),
        (
            "an alarm with no name",
            json!({"AlarmConfiguration": {"Alarms": [{"Name": ""}]}}),
        ),
        (
            "a dispatch role that is not an IAM role ARN",
            json!({"AssociationDispatchAssumeRole": "StateManagerDispatch"}),
        ),
        (
            "a dispatch role naming a user rather than a role",
            json!({"AssociationDispatchAssumeRole": format!(
                "arn:aws:iam::{}:user/StateManagerDispatch",
                engine::emulator_account_id()
            )}),
        ),
        ("no target location at all", json!({"TargetLocations": []})),
        (
            "a target location naming no account",
            json!({"TargetLocations": [{"Accounts": [], "Regions": [REGION]}]}),
        ),
        (
            "a target location naming no Region",
            json!({"TargetLocations": [{"Accounts": [OTHER_ACCOUNT], "Regions": []}]}),
        ),
        (
            "a target location excluding something that is neither an account nor an OU",
            json!({"TargetLocations": [{
                "Regions": [REGION],
                "ExcludeAccounts": ["not-an-account"],
            }]}),
        ),
        (
            "a target location whose concurrency is neither a count nor a percentage",
            json!({"TargetLocations": [{
                "Regions": [REGION],
                "TargetLocationMaxConcurrency": "0%",
            }]}),
        ),
    ];

    for (case, members) in rows {
        let mut request = json!({"Name": AGENT});
        for (member, value) in members.as_object().expect("the row's members are an object") {
            request[member] = value.clone();
        }
        let (status, parsed) = create(&router, &request).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {parsed:?}"
        );
        assert_eq!(
            stored_associations(&engine),
            0,
            "case {case:?}: a refused create persists no association"
        );
    }

    // The same create without the malformed member is served, so each refusal above is the
    // member and not the rest of the request.
    created(
        &router,
        "the create the rows differ from",
        &json!({"Name": AGENT}),
    )
    .await;
    assert_eq!(
        stored_associations(&engine),
        1,
        "the well-formed create persists its association"
    );
}
