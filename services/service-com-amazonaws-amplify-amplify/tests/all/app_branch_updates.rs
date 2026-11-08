// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Amplify create/update WRITE path.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: a create or update operation computes the resource it stores
//! from the REQUEST. An update replaces the stored value of every member the request
//! carries (a map wholesale, not merged), leaves every member it omits exactly as it
//! was, and bumps `updateTime`; a create initialises each member from the request and
//! falls back to the service's own default only where the request omits it. A later
//! read of the same resource therefore returns what the write supplied.
//!
//! Each table spans the class the rule generalises over — which members a request
//! carries, the whole `Stage` enum, the published `TTL` bounds and one value past
//! them, maps that shrink/grow/empty, and booleans set explicitly in both directions
//! — rather than the one request the issue recorded.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

// The wire harness is the one `app_branch_lifecycle` already drives (same test
// target), so the two suites cannot drift on how a request reaches the service.
use crate::app_branch_lifecycle::{
    bool_member, string_member, timestamp_member, Emulator, Reply,
};

/// The members `UpdateApp` accepts but `App` never carries: both document "The token
/// is not stored."
/// (`smithy_query(query="operation",operation="UpdateApp")`.)
const UNSTORED_APP_MEMBERS: &[&str] = &["accessToken", "oauthToken"];

/// Every value the modelled `Stage` enum accepts, from
/// `smithy_query(query="shape",shape="Branch")` -> `stage` -> `Stage`.
const STAGE_VALUES: &[&str] = &[
    "BETA",
    "DEVELOPMENT",
    "EXPERIMENTAL",
    "PRODUCTION",
    "PULL_REQUEST",
];

/// The `App` members this service reports as JSON `null` when the record has no value
/// for them, so a request that supplies an EMPTY value reads back as `null` rather
/// than as `""` / `{}` (see `src/ops/types/projection.rs`, `AppProjection`).
const APP_NULL_WHEN_EMPTY: &[&str] = &["description", "repository", "environmentVariables"];

/// The same set for `Branch` (`BranchProjection`).
const BRANCH_NULL_WHEN_EMPTY: &[&str] = &[
    "activeJobId",
    "customDomains",
    "description",
    "environmentVariables",
    "framework",
];

/// The member a READ adds that a write does not: a reporting operation answers with
/// the resource's tag map (the empty map when it carries none), while the operation
/// that wrote the record echoes only the tags the request set
/// (`AppProjection::reported` / `BranchProjection::reported`).
const REPORTED_ONLY_MEMBER: &str = "tags";

/// The members a write does not ECHO: `basicAuthCredentials` is `@sensitive`
/// (`smithy_query(query="shape",shape="BasicAuthCredentials")`), and what the service
/// reports for it is a non-reversible reference to the credential it kept rather than the
/// credential the request sent — so a supplied value reads back as something that is
/// neither empty nor the submitted value. What the reference IS is pinned by
/// `tests/all/hosted_site_access_control.rs`; here the rule is only that it is not an
/// echo.
const NEVER_ECHOED_MEMBERS: &[&str] = &["basicAuthCredentials"];

/// Is `value` the empty value of its type — what the projection reports as `null`?
fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

/// The WIRE value a supplied member reads back as: the supplied value, or `null` when
/// it is the empty value of a member the service reports as `null`.
fn projected(member: &str, supplied: &Value, null_when_empty: &[&str]) -> Value {
    if null_when_empty.contains(&member) && is_empty_value(supplied) {
        Value::Null
    } else {
        supplied.clone()
    }
}

/// `record` without the member only a read carries, so a write's answer and a read's
/// answer are comparable.
fn without_reported_only(record: &Value) -> Value {
    let mut members = record.as_object().cloned().unwrap_or_default();
    members.remove(REPORTED_ONLY_MEMBER);
    Value::Object(members)
}

/// Create an app from `request` and return its minted `appId`.
async fn new_app(emulator: &Emulator, request: &Value) -> String {
    let reply = emulator.create_app(request).await;
    assert_eq!(
        reply.status, 200,
        "CreateApp {request:?}: expected HTTP 200, got {reply:?}"
    );
    string_member(&reply.member("app"), "appId")
}

/// Create an app plus one branch from `request` and return `(appId, branchName)`.
async fn new_branch(emulator: &Emulator, request: &Value) -> (String, String) {
    let app_id = new_app(emulator, &base_app_request()).await;
    let reply = emulator.create_branch(&app_id, request).await;
    assert_eq!(
        reply.status, 200,
        "CreateBranch {request:?}: expected HTTP 200, got {reply:?}"
    );
    let branch_name = string_member(&reply.member("branch"), "branchName");
    (app_id, branch_name)
}

fn object(label: &str, record: &Value) -> Map<String, Value> {
    record
        .as_object()
        .unwrap_or_else(|| panic!("case {label:?}: expected a JSON object, got {record:?}"))
        .clone()
}

/// The app every update case starts from: the optional members are PRESENT and the
/// booleans are ON, so a case that sets them is a real change and a case that omits
/// them can be caught resetting them.
fn base_app_request() -> Value {
    json!({
        "name": "update-base",
        "description": "the app before the update",
        "repository": "https://example.invalid/org/update-base",
        "platform": "WEB",
        "environmentVariables": { "A": "1", "B": "2" },
        "enableBranchAutoBuild": true,
        "enableBranchAutoDeletion": true,
        "enableBasicAuth": true,
        "basicAuthCredentials": "dXNlcjpwYXNz",
        // The custom-header document in the form the API REPORTS it (the rules as JSON),
        // which is a document it also reads — so what a reader is answered with is what
        // was submitted. A document written as YAML is reported as the equivalent JSON,
        // which `tests/all/hosted_site_custom_headers.rs` pins.
        "customHeaders": "[{\"pattern\":\"**/*\",\"headers\":[{\"key\":\"X-Base\",\"value\":\"1\"}]}]",
        "buildSpec": "version: 1\n",
        "cacheConfig": { "type": "AMPLIFY_MANAGED" },
        "jobConfig": { "buildComputeType": "STANDARD_8GB" },
    })
}

/// The branch every update case starts from, for the same reason.
fn base_branch_request() -> Value {
    json!({
        "branchName": "main",
        "description": "the branch before the update",
        "stage": "DEVELOPMENT",
        "ttl": "5",
        "framework": "React",
        "displayName": "main-display",
        "environmentVariables": { "A": "1", "B": "2" },
        "enableAutoBuild": true,
        "enableNotification": true,
        "enableBasicAuth": true,
        "basicAuthCredentials": "dXNlcjpwYXNz",
        "enablePerformanceMode": true,
        "enableSkewProtection": true,
        "enablePullRequestPreview": true,
        "buildSpec": "version: 1\n",
    })
}

/// Assert the rule on one update: `after` is `before` with EVERY member `request`
/// supplied replaced, every other member untouched, and `updateTime` not older.
///
/// `null_when_empty` names the members this service reports as JSON `null` when they
/// hold no value, so a supplied empty value is expected back as `null`.
fn assert_update_applied(
    label: &str,
    before: &Value,
    after: &Value,
    request: &Value,
    unstored_members: &[&str],
    null_when_empty: &[&str],
) {
    let before = object(label, before);
    let after = object(label, after);
    let request = object(label, request);

    // Every member the request supplied is the one the record now carries — except a
    // member the service documents it does not store, which must appear nowhere.
    for (member, supplied) in &request {
        if unstored_members.contains(&member.as_str()) {
            assert!(
                !after.contains_key(member),
                "case {label:?}: `{member}` is not stored, so the record must not carry it, \
                 got {:?}",
                after.get(member)
            );
            continue;
        }
        if NEVER_ECHOED_MEMBERS.contains(&member.as_str()) {
            let reported = after.get(member).and_then(Value::as_str).unwrap_or_default();
            let submitted = supplied.as_str().unwrap_or_default();
            assert!(
                !reported.is_empty()
                    && reported != submitted
                    && !reported.contains(submitted),
                "case {label:?}: `{member}` must be reported as a reference to the \
                 credential rather than as the submitted {submitted:?}, got {:?}",
                after.get(member)
            );
            continue;
        }
        let expected = projected(member, supplied, null_when_empty);
        assert_eq!(
            after.get(member),
            Some(&expected),
            "case {label:?}: `{member}` must be the supplied {supplied:?} (on the wire \
             {expected:?}), got {:?}",
            after.get(member)
        );
    }

    // Every member the request did NOT supply is exactly what it was: an update is
    // partial, so it never resets a member the caller said nothing about.
    for (member, previous) in &before {
        if member == "updateTime" || request.contains_key(member) {
            continue;
        }
        assert_eq!(
            after.get(member),
            Some(previous),
            "case {label:?}: `{member}` was not supplied, so it must stay {previous:?}, got {:?}",
            after.get(member)
        );
    }

    // And nothing was invented: every member present afterwards was either already
    // there or supplied by this request.
    let invented: BTreeSet<&String> = after
        .keys()
        .filter(|member| !before.contains_key(*member) && !request.contains_key(*member))
        .collect();
    assert!(
        invented.is_empty(),
        "case {label:?}: the update invented members {invented:?} in {after:?}"
    );

    // `createTime` records when the resource was created and never moves; `updateTime`
    // records the last write, so it is never older than it was.
    assert_eq!(
        after.get("createTime"),
        before.get("createTime"),
        "case {label:?}: createTime must not move, got {:?}",
        after.get("createTime")
    );
    let before_update = timestamp_member(&Value::Object(before), "updateTime");
    let after_update = timestamp_member(&Value::Object(after), "updateTime");
    assert!(
        after_update >= before_update,
        "case {label:?}: updateTime must not go backwards, was {before_update}, got {after_update}"
    );
}

/// The `UpdateApp` table: rows differ along WHICH members the request carries, the
/// whole `Platform` enum, a map that shrinks / empties / grows, a boolean set
/// explicitly in both directions, structure and list members, and `name` at the
/// modelled `length` maximum of 255
/// (`smithy_query(query="operation",operation="UpdateApp")`).
fn app_update_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "replace the environment map with one new key",
            json!({ "environmentVariables": { "C": "3" } }),
        ),
        (
            "replace the environment map with an empty map",
            json!({ "environmentVariables": {} }),
        ),
        (
            "grow the environment map to four keys",
            json!({ "environmentVariables": { "A": "9", "B": "2", "C": "3", "D": "4" } }),
        ),
        (
            "rename and re-describe the app",
            json!({ "name": "renamed-app", "description": "a new description" }),
        ),
        (
            "name at the modelled length maximum",
            json!({ "name": "n".repeat(255) }),
        ),
        (
            "move the app to the WEB_COMPUTE platform",
            json!({ "platform": "WEB_COMPUTE", "repository": "https://example.invalid/org/ssr" }),
        ),
        (
            "move the app to the WEB_DYNAMIC platform",
            json!({ "platform": "WEB_DYNAMIC" }),
        ),
        (
            "turn every boolean off",
            json!({
                "enableBranchAutoBuild": false,
                "enableBranchAutoDeletion": false,
                "enableBasicAuth": false,
                "enableAutoBranchCreation": false,
            }),
        ),
        (
            "turn the auto-branch-creation booleans on",
            json!({
                "enableAutoBranchCreation": true,
                "autoBranchCreationPatterns": ["feature/*", "release/*"],
                "autoBranchCreationConfig": {
                    "stage": "EXPERIMENTAL",
                    "framework": "React",
                    "enableAutoBuild": true,
                },
            }),
        ),
        (
            "replace the structure and list members",
            json!({
                "cacheConfig": { "type": "AMPLIFY_MANAGED_NO_COOKIES" },
                "jobConfig": { "buildComputeType": "XLARGE_72GB" },
                "customRules": [
                    { "source": "/<*>", "target": "/index.html", "status": "404-200" },
                    { "source": "/old", "target": "/new" },
                ],
                "customHeaders": "[{\"pattern\":\"/*\",\"headers\":[]}]",
                "buildSpec": "version: 2\n",
            }),
        ),
        (
            "replace the role ARNs and the basic-auth credentials",
            json!({
                "computeRoleArn": "arn:aws:iam::000000000000:role/compute",
                "iamServiceRoleArn": "arn:aws:iam::000000000000:role/service",
                "basicAuthCredentials": "YWRtaW46c2VjcmV0",
            }),
        ),
        ("no member at all", json!({})),
        (
            "only the credentials the service does not store",
            json!({ "accessToken": "ghp_000000000000000000000000000000000000" }),
        ),
    ]
}

#[tokio::test]
async fn update_app_persists_every_supplied_member_and_get_app_reads_it_back() {
    let emulator = Emulator::new();

    for (label, request) in app_update_cases() {
        // A fresh app per row: each case is judged against the state IT started from.
        let app_id = new_app(&emulator, &base_app_request()).await;
        let before = emulator.get_app(&app_id).await.member("app");

        let updated = emulator.update_app(&app_id, &request).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateApp, got {updated:?}"
        );

        // The read-back is the point: the response may only report what was STORED.
        // The read additionally carries the resource's tag map, which the writer's
        // answer does not, so the comparison leaves that member out.
        let after = emulator.get_app(&app_id).await.member("app");
        assert_eq!(
            without_reported_only(&updated.member("app")),
            without_reported_only(&after),
            "case {label:?}: UpdateApp's app must be the one GetApp serves, got {:?}",
            updated.member("app")
        );
        assert_update_applied(
            label,
            &before,
            &after,
            &request,
            UNSTORED_APP_MEMBERS,
            APP_NULL_WHEN_EMPTY,
        );
    }
}

/// The `UpdateBranch` table: rows differ along WHICH members the request carries,
/// every value of the `Stage` enum, the `TTL` bounds, a map that shrinks / empties /
/// grows, booleans set explicitly in both directions, and the structure member.
fn branch_update_cases() -> Vec<(String, Value)> {
    let mut cases = vec![
        (
            "promote the stage and change the ttl".to_string(),
            json!({ "stage": "BETA", "ttl": "10" }),
        ),
        (
            "replace the environment map with one new key".to_string(),
            json!({ "environmentVariables": { "C": "3" } }),
        ),
        (
            "replace the environment map with an empty map".to_string(),
            json!({ "environmentVariables": {} }),
        ),
        (
            "grow the environment map to three keys".to_string(),
            json!({ "environmentVariables": { "A": "9", "B": "2", "C": "3" } }),
        ),
        (
            "ttl at the modelled length minimum (the empty value)".to_string(),
            json!({ "ttl": "" }),
        ),
        ("ttl of zero seconds".to_string(), json!({ "ttl": "0" })),
        (
            "ttl at the modelled length maximum of 32 digits".to_string(),
            json!({ "ttl": "9".repeat(32) }),
        ),
        (
            "change the framework to another value".to_string(),
            json!({ "framework": "Next.js - SSR" }),
        ),
        (
            "clear the framework with the empty value".to_string(),
            json!({ "framework": "" }),
        ),
        (
            "turn every boolean off".to_string(),
            json!({
                "enableAutoBuild": false,
                "enableNotification": false,
                "enableBasicAuth": false,
                "enablePerformanceMode": false,
                "enableSkewProtection": false,
                "enablePullRequestPreview": false,
            }),
        ),
        (
            "re-describe, rename the display and set the pull-request environment".to_string(),
            json!({
                "description": "a new description",
                "displayName": "renamed-display",
                "pullRequestEnvironmentName": "pr-env",
                "buildSpec": "version: 2\n",
            }),
        ),
        (
            "attach a backend and the role ARNs".to_string(),
            json!({
                "backend": { "stackArn": "arn:aws:cloudformation:us-east-1:000000000000:stack/amplify/1" },
                "backendEnvironmentArn": "arn:aws:amplify:us-east-1:000000000000:apps/d1/backendenvironments/dev",
                "computeRoleArn": "arn:aws:iam::000000000000:role/compute",
                "basicAuthCredentials": "YWRtaW46c2VjcmV0",
            }),
        ),
        ("no member at all".to_string(), json!({})),
    ];
    // Every stage the model accepts is reachable by an update, not just the one the
    // issue recorded.
    for stage in STAGE_VALUES {
        cases.push((format!("set the stage to {stage}"), json!({ "stage": stage })));
    }
    cases
}

#[tokio::test]
async fn update_branch_persists_every_supplied_member_and_get_branch_reads_it_back() {
    let emulator = Emulator::new();

    for (label, request) in branch_update_cases() {
        let label = label.as_str();
        // A fresh app + branch per row: each case is judged against its own state.
        let (app_id, branch_name) = new_branch(&emulator, &base_branch_request()).await;
        let before = emulator.get_branch(&app_id, &branch_name).await.member("branch");

        let updated = emulator.update_branch(&app_id, &branch_name, &request).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateBranch, got {updated:?}"
        );

        let after = emulator.get_branch(&app_id, &branch_name).await.member("branch");
        assert_eq!(
            without_reported_only(&updated.member("branch")),
            without_reported_only(&after),
            "case {label:?}: UpdateBranch's branch must be the one GetBranch serves, got {:?}",
            updated.member("branch")
        );
        assert_update_applied(
            label,
            &before,
            &after,
            &request,
            &[],
            BRANCH_NULL_WHEN_EMPTY,
        );
    }
}

#[tokio::test]
async fn a_second_update_sees_what_the_first_one_wrote() {
    let emulator = Emulator::new();
    let (app_id, branch_name) = new_branch(&emulator, &base_branch_request()).await;

    // Converge-to-desired-state: each step reads the branch back and updates one more
    // member, so a dropped write shows up as the next step's input being stale.
    let steps = [
        json!({ "stage": "BETA", "ttl": "10" }),
        json!({ "environmentVariables": { "C": "3" } }),
        json!({ "stage": "PRODUCTION" }),
        json!({ "ttl": "0", "framework": "Vue" }),
    ];
    let mut expected = json!({
        "stage": "DEVELOPMENT",
        "ttl": "5",
        "framework": "React",
        "environmentVariables": { "A": "1", "B": "2" },
    });
    for (index, step) in steps.iter().enumerate() {
        let label = format!("step {index}: {step:?}");
        for (member, value) in step.as_object().expect("a JSON object") {
            expected[member] = value.clone();
        }
        let updated = emulator.update_branch(&app_id, &branch_name, step).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateBranch, got {updated:?}"
        );

        let branch = emulator.get_branch(&app_id, &branch_name).await.member("branch");
        for (member, value) in expected.as_object().expect("a JSON object") {
            assert_eq!(
                branch.get(member),
                Some(value),
                "case {label:?}: `{member}` must be {value:?} after every write so far, got {:?}",
                branch.get(member)
            );
        }
    }
}

/// `CreateBranch` initialises the branch from the request: `enableAutoBuild` decides
/// in both directions, and an omitted member takes the service's default (auto-build
/// ON for a newly connected branch), never the Rust type's zero.
#[tokio::test]
async fn create_branch_takes_enable_auto_build_from_the_request_or_the_service_default() {
    let emulator = Emulator::new();
    let app_id = new_app(&emulator, &base_app_request()).await;

    for (label, request, expected) in [
        (
            "enableAutoBuild omitted",
            json!({ "branchName": "omitted" }),
            true,
        ),
        (
            "enableAutoBuild true",
            json!({ "branchName": "explicit-true", "enableAutoBuild": true }),
            true,
        ),
        (
            "enableAutoBuild false",
            json!({ "branchName": "explicit-false", "enableAutoBuild": false }),
            false,
        ),
    ] {
        let created = emulator.create_branch(&app_id, &request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );
        let branch = created.member("branch");
        assert_eq!(
            bool_member(&branch, "enableAutoBuild"),
            expected,
            "case {label:?}: expected enableAutoBuild {expected}, got {:?}",
            branch.get("enableAutoBuild")
        );

        // Read back through GetBranch: the create response must be what was stored,
        // apart from the tag map only a read carries.
        let branch_name = string_member(&branch, "branchName");
        let read = emulator.get_branch(&app_id, &branch_name).await.member("branch");
        assert_eq!(
            without_reported_only(&read),
            without_reported_only(&branch),
            "case {label:?}: GetBranch must serve the created branch, got {read:?}"
        );
    }
}

/// An update's answer carries the resource's tag map — `UpdateApp` and `UpdateBranch`
/// have no `tags` input member, so what they report is the resource's own tags, the
/// empty map included, exactly as a read of that resource reports them.
#[tokio::test]
async fn an_update_answers_the_resources_tag_map() {
    let emulator = Emulator::new();

    for (label, tags) in [
        ("an untagged resource", json!({})),
        ("one tag", json!({ "team": "platform" })),
        (
            "three tags",
            json!({ "team": "platform", "env": "test", "cost-centre": "42" }),
        ),
    ] {
        // `tags` is set at CREATE time (the only place these shapes accept it).
        let mut app_request = base_app_request();
        app_request["tags"] = tags.clone();
        let app_id = new_app(&emulator, &app_request).await;

        let updated_app = emulator
            .update_app(&app_id, &json!({ "description": "touched" }))
            .await;
        assert_eq!(
            updated_app.member("app").get("tags"),
            Some(&tags),
            "case {label:?}: UpdateApp must answer the app's tag map, got {:?}",
            updated_app.member("app").get("tags")
        );

        let mut branch_request = base_branch_request();
        branch_request["tags"] = tags.clone();
        let created_branch = emulator.create_branch(&app_id, &branch_request).await;
        assert_eq!(
            created_branch.status, 200,
            "case {label:?}: expected HTTP 200 from CreateBranch, got {created_branch:?}"
        );
        let branch_name = string_member(&created_branch.member("branch"), "branchName");

        let updated_branch = emulator
            .update_branch(&app_id, &branch_name, &json!({ "stage": "BETA" }))
            .await;
        assert_eq!(
            updated_branch.member("branch").get("tags"),
            Some(&tags),
            "case {label:?}: UpdateBranch must answer the branch's tag map, got {:?}",
            updated_branch.member("branch").get("tags")
        );
    }
}

#[tokio::test]
async fn an_update_to_a_resource_that_does_not_exist_is_not_found() {
    let emulator = Emulator::new();
    let (app_id, branch_name) = new_branch(&emulator, &base_branch_request()).await;
    let other_app = new_app(&emulator, &base_app_request()).await;

    // `dmissing0000000` satisfies AppId's modelled pattern ^d[a-z0-9]+$, so this is a
    // well-formed id for an app that does not exist — not a refused request.
    let absent_app = "dmissing0000000";
    let cases: Vec<(&str, Reply, String)> = vec![
        (
            "UpdateApp on an app that does not exist",
            emulator.update_app(absent_app, &json!({ "name": "x" })).await,
            format!("App {absent_app} not found."),
        ),
        (
            "UpdateBranch on an app that does not exist",
            emulator
                .update_branch(absent_app, &branch_name, &json!({ "stage": "BETA" }))
                .await,
            format!("App {absent_app} not found."),
        ),
        (
            "UpdateBranch on a branch this app does not have",
            emulator
                .update_branch(&app_id, "no-such-branch", &json!({ "stage": "BETA" }))
                .await,
            "Branch no-such-branch not found.".to_string(),
        ),
        (
            "UpdateBranch on another app's branch",
            emulator
                .update_branch(&other_app, &branch_name, &json!({ "stage": "BETA" }))
                .await,
            format!("Branch {branch_name} not found."),
        ),
    ];

    for (label, reply, expected_message) in cases {
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected HTTP 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: expected NotFoundException, got {:?}",
            reply.error_code
        );
        // The message names the entity that is actually absent and the identifier the
        // request used for it — the same construction every other 404 in this service
        // uses (`crate::state::not_found_message`).
        let actual = reply.message();
        assert_eq!(
            actual.as_deref(),
            Some(expected_message.as_str()),
            "case {label:?}: expected the request-derived message {expected_message:?}, \
             got {actual:?}"
        );
    }

    // The refused updates wrote nothing: the branch still carries its created state.
    let branch = emulator.get_branch(&app_id, &branch_name).await.member("branch");
    assert_eq!(
        string_member(&branch, "stage"),
        "DEVELOPMENT",
        "a refused UpdateBranch must not change the branch, got {:?}",
        branch.get("stage")
    );
}

#[tokio::test]
async fn an_update_outside_a_modelled_constraint_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let (app_id, branch_name) = new_branch(&emulator, &base_branch_request()).await;

    // `TTL` is modelled `length 0..32, pattern ^\d*$` and `Stage` is a closed enum
    // (`smithy_query(query="operation",operation="UpdateBranch")`), so a value past
    // the published bound, one outside the pattern, and one outside the enum are all
    // refused — the row at the bound itself passes in the table above.
    for (label, request) in [
        ("ttl one digit past the modelled maximum", json!({ "ttl": "9".repeat(33) })),
        ("ttl that is not digits", json!({ "ttl": "ten" })),
        ("stage outside the modelled enum", json!({ "stage": "STAGING" })),
        ("displayName past the modelled maximum", json!({ "displayName": "d".repeat(256) })),
    ] {
        let refused = emulator.update_branch(&app_id, &branch_name, &request).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: expected HTTP 400, got {refused:?}"
        );

        let branch = emulator.get_branch(&app_id, &branch_name).await.member("branch");
        assert_eq!(
            string_member(&branch, "ttl"),
            "5",
            "case {label:?}: a refused update must not change the branch, got {:?}",
            branch.get("ttl")
        );
        assert_eq!(
            string_member(&branch, "stage"),
            "DEVELOPMENT",
            "case {label:?}: a refused update must not change the branch, got {:?}",
            branch.get("stage")
        );
    }
}
