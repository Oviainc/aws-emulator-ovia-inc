// SPDX-License-Identifier: CC0-1.0

//! `ListDeploymentGroups`: the deployment groups stored under ONE application,
//! reported in the order they were created, one page at a time.
//!
//! Every case goes through the same front door a client uses — the router, the
//! boundary validator and the awsJson1_1 envelope — by reusing the harness the
//! lifecycle suite owns.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// The names in a listing's `deploymentGroups` member.
fn group_names(body: &Value) -> Vec<String> {
    body["deploymentGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect()
}

/// Register `application` and then create `groups` on it, in that order.
///
/// The lifecycle suite's `seeded_group` creates exactly ONE group under a name
/// derived from the application, which cannot express the orderings and name
/// forms these tables need.
async fn seed(emulator: &Emulator, application: &str, groups: &[&str]) {
    emulator
        .ok("CreateApplication", json!({"applicationName": application}))
        .await;
    for group in groups {
        emulator
            .ok(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": group,
                    "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
                }),
            )
            .await;
    }
}

/// `groups` as owned names, for comparison against a listing.
fn owned(groups: &[&str]) -> Vec<String> {
    groups.iter().map(|name| (*name).to_string()).collect()
}

/// RULE: a listing answers the names of the deployment groups stored under the
/// application the request names — all of them, only that application's, in the
/// order they were created — alongside that application's name.
#[tokio::test]
async fn listing_answers_the_groups_stored_under_the_named_application() {
    let emulator = Emulator::new();
    // Rows differ along the number of groups, how their creation order relates
    // to their sort order, and the FORM of the names: the punctuation row spans
    // every non-alphanumeric character `DeploymentGroupName`'s modelled pattern
    // allows and the bounds row sits on each end of its modelled length
    // (`smithy_query(query="shape",shape="DeploymentGroupName")`: length 1..100,
    // pattern ^[A-Za-z0-9+=,.@_-]*$).
    let max_group = "g".repeat(100);
    let max_application = "a".repeat(100);
    let cases: &[(&str, &[&str])] = &[
        ("list-none", &[]),
        ("list-one", &["only-group"]),
        ("list-unsorted", &["zulu", "alpha", "mike"]),
        (
            "list-punctuation",
            &["g+1", "g=2", "g,3", "g.4", "g@5", "g_6", "g-7"],
        ),
        ("list-bounds", &["g", "ggg"]),
        (max_application.as_str(), &["max-length-application"]),
    ];

    for (application, groups) in cases {
        seed(&emulator, application, groups).await;
        let listed = emulator
            .ok(
                "ListDeploymentGroups",
                json!({"applicationName": application}),
            )
            .await;
        assert_eq!(
            listed["applicationName"].as_str(),
            Some(*application),
            "case {application:?}: expected the application name reported back, got {listed:?}"
        );
        assert_eq!(
            group_names(&listed),
            owned(groups),
            "case {application:?}: expected {groups:?} in creation order, got {listed:?}"
        );
        assert!(
            listed["nextToken"].is_null(),
            "case {application:?}: a single page must carry no nextToken, got {listed:?}"
        );
    }

    // A group whose name sits on the modelled maximum lists like any other.
    seed(&emulator, "list-max-group", &[max_group.as_str()]).await;
    let listed = emulator
        .ok(
            "ListDeploymentGroups",
            json!({"applicationName": "list-max-group"}),
        )
        .await;
    assert_eq!(
        group_names(&listed),
        owned(&[max_group.as_str()]),
        "case 100-character group name: expected it listed, got {listed:?}"
    );

    // Per application, not per account: the same group name under two
    // applications is two records, and each listing answers only its own.
    seed(&emulator, "iso-a", &["shared", "a-only"]).await;
    seed(&emulator, "iso-b", &["shared"]).await;
    for (application, expected) in [
        ("iso-a", vec!["shared", "a-only"]),
        ("iso-b", vec!["shared"]),
    ] {
        let listed = emulator
            .ok(
                "ListDeploymentGroups",
                json!({"applicationName": application}),
            )
            .await;
        assert_eq!(
            group_names(&listed),
            owned(&expected),
            "case {application:?}: expected only its own groups {expected:?}, got {listed:?}"
        );
    }

    // Reporting, not mutating: listing twice answers the same thing, and the
    // earlier applications are untouched by every application seeded since.
    let first = emulator
        .ok(
            "ListDeploymentGroups",
            json!({"applicationName": "list-unsorted"}),
        )
        .await;
    let second = emulator
        .ok(
            "ListDeploymentGroups",
            json!({"applicationName": "list-unsorted"}),
        )
        .await;
    assert_eq!(
        group_names(&first),
        group_names(&second),
        "case repeated listing: a read must not change what the next caller sees, got \
         {first:?} then {second:?}"
    );
    assert_eq!(
        group_names(&second),
        owned(&["zulu", "alpha", "mike"]),
        "case repeated listing: expected the creation order preserved, got {second:?}"
    );

    // A group created after a listing shows up in the next one, appended.
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": "list-unsorted",
                "deploymentGroupName": "late",
                "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
            }),
        )
        .await;
    let after = emulator
        .ok(
            "ListDeploymentGroups",
            json!({"applicationName": "list-unsorted"}),
        )
        .await;
    assert_eq!(
        group_names(&after),
        owned(&["zulu", "alpha", "mike", "late"]),
        "case group created after a listing: expected it appended, got {after:?}"
    );
}

/// RULE: the listing is `@paginated` on `nextToken` alone, so a token CONTINUES
/// that application's listing — every stored name reported exactly once, in
/// creation order, across the pages — and a token this listing did not issue is
/// refused.
#[tokio::test]
async fn listing_groups_pages_without_repeating_or_restarting() {
    let emulator = Emulator::new();
    // One full page plus a few, so the listing cannot fit in a single page.
    let total = service_com_amazonaws_codedeploy_codedeploy_20141006::state::PAGE_SIZE + 5;
    let groups: Vec<String> = (0..total).map(|index| format!("paged-{index:03}")).collect();
    let borrowed: Vec<&str> = groups.iter().map(String::as_str).collect();
    seed(&emulator, "paged-app", &borrowed).await;

    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut first_token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({"applicationName": "paged-app"});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = emulator.ok("ListDeploymentGroups", request).await;
        let names = group_names(&page);
        assert!(
            !names.is_empty(),
            "case page {pages}: expected a non-empty page, got {page:?}"
        );
        collected.extend(names);
        pages += 1;
        assert!(
            pages <= 5,
            "case {total} groups: paging did not terminate after {pages} pages"
        );
        token = page["nextToken"].as_str().map(str::to_string);
        if first_token.is_none() {
            first_token = token.clone();
        }
        if token.is_none() {
            break;
        }
    }
    assert!(
        pages > 1,
        "case {total} groups: expected more than one page, got {pages}"
    );
    assert_eq!(
        collected, groups,
        "case {total} groups: expected every name exactly once in creation order, got {} names",
        collected.len()
    );

    // A token names a position in ONE application's listing, so replaying it
    // against an application that does not hold that group is not a page.
    let issued = first_token.expect("the first page of a multi-page listing issues a token");
    seed(&emulator, "other-app", &["elsewhere"]).await;
    let refusals: &[(&str, Value)] = &[
        ("not a token at all", json!("not-a-token")),
        ("the token prefix alone", json!("p")),
        ("a token with a malformed payload", json!("pzzz-")),
        ("an empty token", json!("")),
        ("another application's token", json!(issued)),
    ];
    for (case, bogus) in refusals {
        let (status, body) = emulator
            .call(
                "ListDeploymentGroups",
                json!({"applicationName": "other-app", "nextToken": bogus}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidNextTokenException"),
            "case {case}: expected 400 InvalidNextTokenException, got {status} {body:?}"
        );
    }
}

/// RULE: a listing refuses an application name it cannot accept with
/// CodeDeploy's own code for that member, and an application it does not hold
/// with `ApplicationDoesNotExistException` — never an empty page.
#[tokio::test]
async fn a_listing_refuses_a_name_it_cannot_accept_or_does_not_hold() {
    let emulator = Emulator::new();
    seed(&emulator, "refusal-app", &["dg"]).await;
    // One row per refusal kind: the member absent, empty, past its modelled
    // maximum, outside its modelled pattern, and well formed but unknown.
    let too_long = "n".repeat(101);
    let cases: &[(&str, Value, &str)] = &[
        (
            "applicationName absent",
            json!({}),
            "ApplicationNameRequiredException",
        ),
        (
            "applicationName empty",
            json!({"applicationName": ""}),
            "ApplicationNameRequiredException",
        ),
        (
            "applicationName one past the modelled maximum of 100",
            json!({"applicationName": &too_long}),
            "InvalidApplicationNameException",
        ),
        (
            "applicationName outside the modelled pattern",
            json!({"applicationName": "has spaces"}),
            "InvalidApplicationNameException",
        ),
        (
            "an application that was never registered",
            json!({"applicationName": "never-created"}),
            "ApplicationDoesNotExistException",
        ),
        (
            "an application that was deleted",
            json!({"applicationName": "deleted-app"}),
            "ApplicationDoesNotExistException",
        ),
    ];

    seed(&emulator, "deleted-app", &["dg"]).await;
    emulator
        .ok("DeleteApplication", json!({"applicationName": "deleted-app"}))
        .await;

    for (case, request, expected) in cases {
        let (status, body) = emulator
            .call("ListDeploymentGroups", request.clone())
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case}: expected 400 {expected}, got {status} {body:?}"
        );
    }
}
