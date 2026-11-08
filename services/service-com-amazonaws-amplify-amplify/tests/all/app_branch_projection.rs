// SPDX-License-Identifier: CC0-1.0

//! What an Amplify `App` / `Branch` response CARRIES, member by member.
//!
//! Every case drives the real wire path (the shared harness in
//! `tests/all/app_branch_lifecycle.rs`), so what is asserted is the JSON an AWS SDK
//! client parses.
//!
//! The rule under test, over every operation that answers with an app or a branch:
//!
//! * a member the service has **no value** for is JSON `null` — never the empty
//!   string, the empty list or the empty map;
//! * a member whose value the service itself **determines** — a feature flag it has
//!   set, a configuration default it applies, a collection it maintains — is always
//!   present, carrying that value;
//! * a **read** reports the resource's tag map (the empty map when it has none),
//!   while the operation that wrote the record echoes only the tags the request set;
//! * `ListBranches` elements go through the same projection as `GetBranch`, so a
//!   listed branch and a fetched branch cannot disagree.
//!
//! Each table spans the class the rule generalises over — a member absent, set to a
//! non-empty value, or set to its empty value; a flag absent and both ways round;
//! every value of the two modelled enums that carry a documented default — rather
//! than the one request a recording happened to contain.

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{string_member, without_tags, Emulator};

/// Assert that `record` carries every member of `expected` with exactly that value.
///
/// A member expected as `null` must be PRESENT and null: `record.get(member)` is
/// `None` for an absent member, which is not `Some(&Value::Null)`, so the absent and
/// the null case cannot pass for one another.
fn assert_members(label: &str, operation: &str, record: &Value, expected: &Value) {
    for (member, want) in expected
        .as_object()
        .expect("every expectation table entry is a JSON object")
    {
        assert_eq!(
            record.get(member),
            Some(want),
            "case {label:?}: {operation} must report {member:?} as {want}, got {:?} \
             in {record:?}",
            record.get(member)
        );
    }
}

/// One `CreateApp` request and the `App` projection every operation must report for
/// the app it creates.
struct AppProjectionCase {
    case: &'static str,
    request: Value,
    /// Member name -> the exact JSON value the response must carry.
    expected: Value,
}

/// The `App` projection table.
///
/// The `cacheConfig` rows cover BOTH values of the modelled `CacheConfigType` enum
/// and the `jobConfig` rows cover ALL THREE values of `BuildComputeType`
/// (`smithy_query(query="shape",shape="CacheConfigType,BuildComputeType")`), including
/// the two sizes no recording cites. The default each member takes when the request
/// omits it is the one its own model documentation names: `CacheConfig$type`
/// ("The `AMPLIFY_MANAGED_NO_COOKIES` cache configuration type ... This is the default
/// setting") and `JobConfig$buildComputeType` ("If you don't specify a value, Amplify
/// uses the `STANDARD_8GB` default").
fn app_projection_cases() -> Vec<AppProjectionCase> {
    vec![
        AppProjectionCase {
            case: "nothing but the required name",
            request: json!({ "name": "projection-default" }),
            expected: json!({
                // No value: reported as null, not as "" / {}.
                "description": null,
                "repository": null,
                "environmentVariables": null,
                // Determined by the service: always present.
                "customHeaders": "",
                "customRules": [],
                "enableAutoBranchCreation": false,
                "enableBranchAutoDeletion": false,
                "cacheConfig": { "type": "AMPLIFY_MANAGED_NO_COOKIES" },
                "jobConfig": { "buildComputeType": "STANDARD_8GB" },
            }),
        },
        AppProjectionCase {
            case: "every null-reported member carries a value",
            request: json!({
                "name": "projection-valued",
                "description": "a described app",
                "repository": "https://example.invalid/org/projection",
                "environmentVariables": { "KEY": "VAL" },
            }),
            expected: json!({
                "description": "a described app",
                "repository": "https://example.invalid/org/projection",
                "environmentVariables": { "KEY": "VAL" },
                "customHeaders": "",
                "customRules": [],
                "enableAutoBranchCreation": false,
                "enableBranchAutoDeletion": false,
                "cacheConfig": { "type": "AMPLIFY_MANAGED_NO_COOKIES" },
                "jobConfig": { "buildComputeType": "STANDARD_8GB" },
            }),
        },
        // The record holds a @required member as a bare `String` / `HashMap`, so
        // "set to the empty value" and "never set" are the same record: the app has
        // no description, no repository and no environment, and each is reported as
        // null.
        AppProjectionCase {
            case: "the null-reported members set to their empty value",
            request: json!({
                "name": "projection-emptied",
                "description": "",
                "repository": "",
                "environmentVariables": {},
            }),
            expected: json!({
                "description": null,
                "repository": null,
                "environmentVariables": null,
            }),
        },
        AppProjectionCase {
            case: "both auto-branch flags on, with headers and rules",
            request: json!({
                "name": "projection-flags",
                "enableAutoBranchCreation": true,
                "enableBranchAutoDeletion": true,
                "autoBranchCreationPatterns": ["feature/*"],
                "customHeaders": "customHeaders:\n  - pattern: '**'\n",
                "customRules": [{ "source": "/old", "target": "/new", "status": "301" }],
            }),
            expected: json!({
                "enableAutoBranchCreation": true,
                "enableBranchAutoDeletion": true,
                "autoBranchCreationPatterns": ["feature/*"],
                // `customHeaders` carries a DOCUMENT, which the service parses: what it
                // reports is the rule list it read, as JSON, whatever notation the
                // request wrote it in (here YAML). `customRules` is a modelled LIST and
                // is echoed element for element.
                "customHeaders": "[{\"pattern\":\"**\",\"headers\":[]}]",
                "customRules": [{ "source": "/old", "target": "/new", "status": "301" }],
                "cacheConfig": { "type": "AMPLIFY_MANAGED_NO_COOKIES" },
                "jobConfig": { "buildComputeType": "STANDARD_8GB" },
            }),
        },
        AppProjectionCase {
            case: "the cookie-including cache type and the 16 GB build instance",
            request: json!({
                "name": "projection-cache-managed",
                "cacheConfig": { "type": "AMPLIFY_MANAGED" },
                "jobConfig": { "buildComputeType": "LARGE_16GB" },
            }),
            expected: json!({
                "cacheConfig": { "type": "AMPLIFY_MANAGED" },
                "jobConfig": { "buildComputeType": "LARGE_16GB" },
            }),
        },
        AppProjectionCase {
            case: "the cookie-excluding cache type and the 72 GB build instance",
            request: json!({
                "name": "projection-cache-no-cookies",
                "cacheConfig": { "type": "AMPLIFY_MANAGED_NO_COOKIES" },
                "jobConfig": { "buildComputeType": "XLARGE_72GB" },
            }),
            expected: json!({
                "cacheConfig": { "type": "AMPLIFY_MANAGED_NO_COOKIES" },
                "jobConfig": { "buildComputeType": "XLARGE_72GB" },
            }),
        },
    ]
}

#[tokio::test]
async fn every_app_returning_operation_projects_the_same_record() {
    let emulator = Emulator::new();

    for case in app_projection_cases() {
        let label = case.case;

        let created = emulator.create_app(&case.request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateApp, got {created:?}"
        );
        let app = created.member("app");
        let app_id = string_member(&app, "appId");
        assert_members(label, "CreateApp", &app, &case.expected);

        // The canonical read.
        let fetched = emulator.get_app(&app_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected HTTP 200 from GetApp, got {fetched:?}"
        );
        assert_members(label, "GetApp", &fetched.member("app"), &case.expected);

        // The list element: the same record, projected the same way.
        let listed = emulator.list_apps("").await;
        let element = listed
            .body
            .get("apps")
            .and_then(Value::as_array)
            .expect("ListApps answers with an apps list")
            .iter()
            .find(|element| element.get("appId") == app.get("appId"))
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: ListApps must list the created app"));
        assert_members(label, "ListApps", &element, &case.expected);
        assert_eq!(
            element,
            fetched.member("app"),
            "case {label:?}: a listed app and a fetched app are the same record"
        );

        // And the record the delete answers with.
        let deleted = emulator.delete_app(&app_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected HTTP 200 from DeleteApp, got {deleted:?}"
        );
        assert_members(label, "DeleteApp", &deleted.member("app"), &case.expected);
        assert_eq!(
            without_tags(&deleted.member("app")),
            without_tags(&app),
            "case {label:?}: DeleteApp must answer with the record as it stood, got {:?}",
            deleted.member("app")
        );
    }
}

/// One `CreateBranch` request and the `Branch` projection every operation must
/// report for the branch it creates.
struct BranchProjectionCase {
    case: &'static str,
    request: Value,
    /// Member name -> the exact JSON value the response must carry.
    expected: Value,
}

/// The `Branch` projection table: it varies the null-reported members between
/// absent, set and set-to-empty, `enablePerformanceMode` between absent, off and on,
/// and `backend` between absent and carrying a stack ARN.
fn branch_projection_cases() -> Vec<BranchProjectionCase> {
    vec![
        BranchProjectionCase {
            case: "nothing but the required branchName",
            request: json!({ "branchName": "default-projection" }),
            expected: json!({
                // No value: reported as null.
                "description": null,
                "activeJobId": null,
                "customDomains": null,
                "environmentVariables": null,
                "framework": null,
                // Determined by the service: always present.
                "enablePerformanceMode": false,
                // Every modelled member reaches the wire, nested ones included: an
                // unset `Backend$stackArn` is an explicit null, not a missing key.
                "backend": { "stackArn": null },
            }),
        },
        BranchProjectionCase {
            case: "every null-reported member carries a value",
            request: json!({
                "branchName": "valued-projection",
                "description": "a described branch",
                "environmentVariables": { "A": "1", "B": "2" },
                "framework": "React",
            }),
            expected: json!({
                "description": "a described branch",
                "environmentVariables": { "A": "1", "B": "2" },
                "framework": "React",
                "activeJobId": null,
                "customDomains": null,
                "enablePerformanceMode": false,
                "backend": { "stackArn": null },
            }),
        },
        BranchProjectionCase {
            case: "the null-reported members set to their empty value",
            request: json!({
                "branchName": "emptied-projection",
                "description": "",
                "environmentVariables": {},
                "framework": "",
            }),
            expected: json!({
                "description": null,
                "environmentVariables": null,
                "activeJobId": null,
                "customDomains": null,
                "framework": null,
            }),
        },
        BranchProjectionCase {
            case: "performance mode switched off explicitly",
            request: json!({
                "branchName": "perf-off-projection",
                "enablePerformanceMode": false,
            }),
            expected: json!({ "enablePerformanceMode": false }),
        },
        BranchProjectionCase {
            case: "performance mode switched on",
            request: json!({
                "branchName": "perf-on-projection",
                "enablePerformanceMode": true,
            }),
            expected: json!({ "enablePerformanceMode": true }),
        },
        // `Backend$stackArn` is modelled `length 20..2048, pattern
        // ^arn:aws:cloudformation:[a-z0-9-]+:\d{12}:stack/.+/.+$`
        // (`smithy_query(query="shape",shape="Backend")`).
        BranchProjectionCase {
            case: "a branch whose backend names a CloudFormation stack",
            request: json!({
                "branchName": "backed-projection",
                "backend": {
                    "stackArn": "arn:aws:cloudformation:us-east-1:000000000000:stack/amplify-demo/1234",
                },
            }),
            expected: json!({
                "backend": {
                    "stackArn": "arn:aws:cloudformation:us-east-1:000000000000:stack/amplify-demo/1234",
                },
            }),
        },
    ]
}

#[tokio::test]
async fn every_branch_returning_operation_projects_the_same_record() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("branch-projection-host").await;

    for case in branch_projection_cases() {
        let label = case.case;

        let created = emulator.create_branch(&app_id, &case.request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );
        let branch = created.member("branch");
        let branch_name = string_member(&branch, "branchName");
        assert_members(label, "CreateBranch", &branch, &case.expected);

        let fetched = emulator.get_branch(&app_id, &branch_name).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected HTTP 200 from GetBranch, got {fetched:?}"
        );
        assert_members(label, "GetBranch", &fetched.member("branch"), &case.expected);
    }

    // Cause 7 of the issue: a list element is the SAME projection as the read, so a
    // client that lists and then fetches sees no drift on any branch of the table.
    let listed = emulator.list_branches(&app_id, "").await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 from ListBranches, got {listed:?}"
    );
    let elements = listed
        .body
        .get("branches")
        .and_then(Value::as_array)
        .expect("ListBranches answers with a branches list")
        .clone();
    assert_eq!(
        elements.len(),
        branch_projection_cases().len(),
        "ListBranches must list every branch of the table, got {elements:?}"
    );
    for element in elements {
        let branch_name = string_member(&element, "branchName");
        let fetched = emulator.get_branch(&app_id, &branch_name).await;
        assert_eq!(
            element,
            fetched.member("branch"),
            "case {branch_name:?}: a listed branch and a fetched branch are the same \
             record, got {element:?}"
        );
    }
}

/// One tag table row: the tags a create request carries, what the write response must
/// echo, and what every read must report.
struct TagCase {
    case: &'static str,
    tags: Option<Value>,
    /// The WIRE value of `tags` on the write response: `null` when the request set
    /// none, since every modelled member reaches the wire.
    echoed: Value,
    reported: Value,
}

/// A read reports the resource's tag map — the empty map when it has none — while
/// the create/delete response echoes only what the request set. The table covers no
/// tags, one tag and three.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "no tags at all",
            tags: None,
            echoed: Value::Null,
            reported: json!({}),
        },
        TagCase {
            case: "one tag",
            tags: Some(json!({ "team": "web" })),
            echoed: json!({ "team": "web" }),
            reported: json!({ "team": "web" }),
        },
        TagCase {
            case: "three tags",
            tags: Some(json!({ "team": "web", "env": "test", "owner": "nobody" })),
            echoed: json!({ "team": "web", "env": "test", "owner": "nobody" }),
            reported: json!({ "team": "web", "env": "test", "owner": "nobody" }),
        },
    ]
}

#[tokio::test]
async fn a_read_reports_the_tag_map_and_a_write_echoes_what_the_request_set() {
    let emulator = Emulator::new();

    for (index, case) in tag_cases().into_iter().enumerate() {
        let label = case.case;

        let mut request = json!({ "name": format!("tagged-app-{index}") });
        let mut branch_request = json!({ "branchName": format!("tagged-branch-{index}") });
        if let Some(tags) = &case.tags {
            request["tags"] = tags.clone();
            branch_request["tags"] = tags.clone();
        }

        let created = emulator.create_app(&request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateApp, got {created:?}"
        );
        let app = created.member("app");
        let app_id = string_member(&app, "appId");
        assert_eq!(
            app.get("tags"),
            Some(&case.echoed),
            "case {label:?}: CreateApp must echo the tags the request set, got {:?}",
            app.get("tags")
        );

        for (operation, record) in [
            ("GetApp", emulator.get_app(&app_id).await.member("app")),
            (
                "ListApps",
                emulator
                    .list_apps("")
                    .await
                    .body
                    .get("apps")
                    .and_then(Value::as_array)
                    .expect("ListApps answers with an apps list")
                    .iter()
                    .find(|element| element.get("appId") == app.get("appId"))
                    .cloned()
                    .expect("ListApps lists the created app"),
            ),
        ] {
            assert_eq!(
                record.get("tags"),
                Some(&case.reported),
                "case {label:?}: {operation} must report the app's tag map, got {:?}",
                record.get("tags")
            );
        }

        let created_branch = emulator.create_branch(&app_id, &branch_request).await;
        assert_eq!(
            created_branch.status, 200,
            "case {label:?}: expected HTTP 200 from CreateBranch, got {created_branch:?}"
        );
        let branch = created_branch.member("branch");
        let branch_name = string_member(&branch, "branchName");
        assert_eq!(
            branch.get("tags"),
            Some(&case.echoed),
            "case {label:?}: CreateBranch must echo the tags the request set, got {:?}",
            branch.get("tags")
        );

        for (operation, record) in [
            (
                "GetBranch",
                emulator
                    .get_branch(&app_id, &branch_name)
                    .await
                    .member("branch"),
            ),
            (
                "ListBranches",
                emulator
                    .list_branches(&app_id, "")
                    .await
                    .body
                    .get("branches")
                    .and_then(Value::as_array)
                    .expect("ListBranches answers with a branches list")
                    .first()
                    .cloned()
                    .expect("ListBranches lists the created branch"),
            ),
        ] {
            assert_eq!(
                record.get("tags"),
                Some(&case.reported),
                "case {label:?}: {operation} must report the branch's tag map, got {:?}",
                record.get("tags")
            );
        }
    }
}
