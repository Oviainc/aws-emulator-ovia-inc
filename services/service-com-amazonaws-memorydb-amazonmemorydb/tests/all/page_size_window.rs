// SPDX-License-Identifier: CC0-1.0

//! The page-size window each MemoryDB listing serves, and how an identifier refusal
//! names what it rejected — end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a paginated listing serves a `MaxResults` inside the window ITS OWN resource
//!   surface publishes and refuses one outside it with the modelled
//!   `InvalidParameterValueException` naming both of that window's bounds — every
//!   paginated listing the service implements, exercised at each bound of its own
//!   window, one past each bound and further out, each listing naming its page size in
//!   the noun its own refusal uses (the windows themselves, and that they differ per
//!   surface, are asserted in `tests/all/access_control_page_size.rs`);
//! * a served page size is APPLIED: the page holds exactly what the size allows (the
//!   whole set when the set is smaller), a `NextToken` is handed out exactly when the
//!   page stopped short of the set, and a request with no page size answers the whole
//!   set;
//! * a `NextToken` names a POSITION and nothing about the page that produced it: a walk
//!   that changes `MaxResults` between pages, to any other size that listing's window
//!   serves, still visits the whole set once in the listing's own order;
//! * an accepted page size really bounds the page: a listing whose set is larger than
//!   the page hands out a full page and a token, and the pages concatenate to the whole
//!   set;
//! * a name that is not a well-formed identifier is refused with the sentence the
//!   service words that kind of refusal in: a parameter group's is built from the REQUEST
//!   MEMBER the name arrived in, every other kind's from the service's own label for that
//!   identifier.

use serde_json::{json, Value};

use crate::cluster_lifecycle::{call, error_code, ok, router};

/// The page sizes ONE listing serves, with the noun its refusal names the page size
/// with: a size inside the window is served, one outside it is refused. Spelled out HERE
/// rather than taken from the crate under test, so the assertions below are independent
/// of the code that enforces the windows.
#[derive(Debug, Clone, Copy)]
struct Window {
    min: i32,
    max: i32,
    noun: &'static str,
}

/// The window the ACCESS-CONTROL listings serve (`DescribeUsers`, `DescribeACLs`).
const ACCESS_CONTROL: Window = Window {
    min: 1,
    max: 50,
    noun: "max results",
};

/// The window the CLUSTER-side listings serve (`DescribeClusters`, `DescribeEvents`,
/// `DescribeEngineVersions`).
const CLUSTER: Window = Window {
    min: 20,
    max: 100,
    noun: "max records",
};

/// The window the parameter-group surface serves (`DescribeParameterGroups`,
/// `DescribeParameters`): the cluster-side bounds under the other noun.
const PARAMETER: Window = Window {
    min: 20,
    max: 100,
    noun: "max results",
};

/// What a listing answers a page size outside its own window with — the rule itself,
/// naming both of that window's bounds in the noun the listing words it with.
fn window_message(window: Window) -> String {
    format!(
        "Invalid value for {}. Must be between {} and {}",
        window.noun, window.min, window.max
    )
}

/// The clauses of the identifier rule, which every spelling refusal ends with.
const IDENTIFIER_CLAUSES: &str = "must begin with a letter; must contain only ASCII \
                                  letters, digits, and hyphens; and must not end with a \
                                  hyphen or contain two consecutive hyphens.";

/// A parameter group family the engine catalog publishes.
const FAMILY: &str = "memorydb_valkey7";

/// A password that breaks none of the documented password rules.
const PASSWORD: &str = "Sim2realPassword-1";

/// The `message` of a failure body, under either spelling of the member.
fn message(body: &Value, case: &str) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {body}"))
        .to_string()
}

/// The items one page of `listing` reported, under the member that listing answers them
/// in.
fn page_items(body: &Value, items: &str, case: &str) -> Vec<Value> {
    body.get(items)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected {items} in {body}"))
        .clone()
}

/// Every paginated listing this service implements, with the member it answers its items
/// in, the page-size window ITS resource surface publishes, and the rest of the request
/// it needs (`DescribeParameters` names the group it lists for).
fn listings(group: &str) -> Vec<(&'static str, &'static str, Window, Value)> {
    vec![
        ("DescribeUsers", "Users", ACCESS_CONTROL, json!({})),
        ("DescribeACLs", "ACLs", ACCESS_CONTROL, json!({})),
        ("DescribeClusters", "Clusters", CLUSTER, json!({})),
        (
            "DescribeParameterGroups",
            "ParameterGroups",
            PARAMETER,
            json!({}),
        ),
        (
            "DescribeEngineVersions",
            "EngineVersions",
            CLUSTER,
            json!({}),
        ),
        ("DescribeEvents", "Events", CLUSTER, json!({})),
        (
            "DescribeParameters",
            "Parameters",
            PARAMETER,
            json!({ "ParameterGroupName": group }),
        ),
    ]
}

/// Set up a store every listing above has something to report from: a user, an ACL, a
/// cluster and a parameter group.
async fn seeded_store() -> (smithy_server_runtime::MultiServiceRouter, String) {
    let router = router();
    let group = "window-group".to_string();
    ok(
        &router,
        "CreateUser",
        &json!({
            "UserName": "window-user",
            "AccessString": "on ~* +@all",
            "AuthenticationMode": { "Type": "password", "Passwords": [PASSWORD] },
        }),
        "setup",
    )
    .await;
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "window-acl", "UserNames": ["window-user"] }),
        "setup",
    )
    .await;
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "window-cluster",
            "NodeType": "db.t4g.small",
            "ACLName": "window-acl",
        }),
        "setup",
    )
    .await;
    ok(
        &router,
        "CreateParameterGroup",
        &json!({ "ParameterGroupName": &group, "Family": FAMILY }),
        "setup",
    )
    .await;
    (router, group)
}

/// A page size inside a listing's own window is served by it, and it is APPLIED: the
/// page holds exactly as many items as the size allows — the whole set when the set is
/// smaller, the size itself when it is not — a `NextToken` is handed out exactly when the
/// page stopped short of the set, and a request carrying no page size at all is served
/// and answers the whole set.
#[tokio::test]
async fn a_page_size_inside_the_window_is_served_by_every_listing() {
    let (router, group) = seeded_store().await;

    for (operation, items, window, base) in listings(&group) {
        // The rows span this listing's window: each bound, one inside each bound, and
        // the middle.
        let accepted = [
            window.min,
            window.min + 1,
            (window.min + window.max) / 2,
            window.max - 1,
            window.max,
        ];
        // The whole set, read at the top of the window, so each page below is checked
        // against the set's OWN size rather than a bound that is trivially met.
        let case = format!("{operation} whole set");
        let mut request = base.clone();
        request["MaxResults"] = json!(window.max);
        let whole = ok(&router, operation, &request, &case).await;
        let total = page_items(&whole, items, &case).len();
        assert!(
            whole.get("NextToken").is_none(),
            "case {case:?}: a set that fits in the largest page is owed no token, got {whole}"
        );

        for page_size in accepted {
            let case = format!("{operation} MaxResults={page_size}");
            let mut request = base.clone();
            request["MaxResults"] = json!(page_size);
            let body = ok(&router, operation, &request, &case).await;
            let page = page_items(&body, items, &case);
            let expected = total.min(usize::try_from(page_size).unwrap_or(usize::MAX));
            assert_eq!(
                page.len(),
                expected,
                "case {case:?}: a page of a set of {total} holds {expected} items, got {}",
                page.len()
            );
            assert_eq!(
                body.get("NextToken").is_some(),
                page.len() < total,
                "case {case:?}: a token is handed out exactly when the set is longer than \
                 the page, got {body}"
            );
        }

        // No page size at all: served, and the whole set comes back — the window bounds
        // a value the request supplied rather than being a size the listing requires.
        let case = format!("{operation} with no MaxResults");
        let body = ok(&router, operation, &base, &case).await;
        assert_eq!(
            page_items(&body, items, &case).len(),
            total,
            "case {case:?}: a request with no page size answers the whole set of {total}, \
             got {body}"
        );
    }
}

/// A page size outside a listing's own window names no page: the listing refuses it with
/// the modelled invalid-value error, and the message states that window's own bounds in
/// the noun the listing names its page size with.
#[tokio::test]
async fn a_page_size_outside_the_window_is_refused_with_the_window_message() {
    let (router, group) = seeded_store().await;

    for (operation, _, window, base) in listings(&group) {
        // The rows span the class on both sides of this listing's window: one past each
        // bound, zero, a negative size, and far above the upper bound.
        let refused = [
            window.min - 1,
            0,
            -1,
            -4_096,
            window.max + 1,
            i32::from(i16::MAX),
        ];
        for page_size in refused {
            let case = format!("{operation} MaxResults={page_size}");
            let mut request = base.clone();
            request["MaxResults"] = json!(page_size);
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected the modelled invalid-value refusal, got {status} \
                 with {body}"
            );
            assert_eq!(
                message(&body, &case),
                window_message(window),
                "case {case:?}: expected the window's own bounds, got {body}"
            );
        }
    }
}

/// An accepted page size is APPLIED, not merely tolerated: a listing whose set is larger
/// than the page hands out exactly that many items plus a token, and the pages
/// concatenate to the unpaged listing with nothing repeated or dropped.
#[tokio::test]
async fn an_accepted_page_size_bounds_the_page() {
    let (router, group) = seeded_store().await;
    let case = "parameters paged at the smallest served size";
    let whole: Vec<Value> = page_items(
        &ok(
            &router,
            "DescribeParameters",
            &json!({ "ParameterGroupName": &group }),
            case,
        )
        .await,
        "Parameters",
        case,
    );
    assert!(
        i32::try_from(whole.len()).unwrap_or(i32::MAX) > PARAMETER.min,
        "case {case:?}: the family must publish more parameters than the smallest page \
         for this to page at all, got {}",
        whole.len()
    );

    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let mut request = json!({ "ParameterGroupName": &group, "MaxResults": PARAMETER.min });
        if let Some(token) = token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let body = ok(&router, "DescribeParameters", &request, case).await;
        let page = page_items(&body, "Parameters", case);
        token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_some() {
            // A page that is not the last one is FULL: the size asked for bounded it.
            assert_eq!(
                i32::try_from(page.len()).unwrap_or(i32::MAX),
                PARAMETER.min,
                "case {case:?}: a page before the last holds exactly {} items, got {}",
                PARAMETER.min,
                page.len()
            );
        }
        walked.extend(page);
        pages += 1;
        assert!(
            pages <= whole.len(),
            "case {case:?}: the walk must end, got {pages} pages"
        );
        if token.is_none() {
            break;
        }
    }
    assert!(
        pages > 1,
        "case {case:?}: a set larger than the page must take several pages, got {pages}"
    );
    assert_eq!(
        walked, whole,
        "case {case:?}: the pages together must be the whole listing, in order"
    );
}

/// A token names a POSITION in the listing and nothing about the page that produced it:
/// a walk may change `MaxResults` at every step — to any other size that listing's window
/// serves — and still visit the whole set exactly once, in the listing's own order, ending
/// with no token left. That is what lets a client lower or raise its page size mid-walk.
#[tokio::test]
async fn a_token_resumes_the_listing_at_any_served_page_size() {
    let (router, group) = seeded_store().await;
    // More groups than the SMALLEST page this surface serves, so the group listing hands
    // out a token at the bottom of its window too.
    for index in 0..PARAMETER.min {
        let name = format!("token-group-{index:02}");
        ok(
            &router,
            "CreateParameterGroup",
            &json!({ "ParameterGroupName": name, "Family": FAMILY }),
            &name,
        )
        .await;
    }

    // (first page size, every later page size): both bounds of this surface's window in
    // both directions and a size in between, so a walk narrows, widens and holds still.
    let steps: &[(i32, i32)] = &[
        (PARAMETER.min, PARAMETER.min),
        (PARAMETER.min, PARAMETER.max),
        (PARAMETER.min, (PARAMETER.min + PARAMETER.max) / 2),
        (PARAMETER.min + 1, PARAMETER.min),
        ((PARAMETER.min + PARAMETER.max) / 2, PARAMETER.min),
    ];
    // The two listings with a set larger than the smallest page: one the SERVICE
    // provides (a family's engine defaults) and one a CALLER created (the groups above).
    let listings: &[(&str, &str, Value)] = &[
        (
            "DescribeParameters",
            "Parameters",
            json!({ "ParameterGroupName": &group }),
        ),
        ("DescribeParameterGroups", "ParameterGroups", json!({})),
    ];

    for (operation, items, base) in listings {
        // The whole set in one page: the order and content every walk must reproduce.
        let case = format!("{operation} whole set");
        let mut request = base.clone();
        request["MaxResults"] = json!(PARAMETER.max);
        let answered = ok(&router, operation, &request, &case).await;
        let whole = page_items(&answered, items, &case);
        assert!(
            answered.get("NextToken").is_none()
                && i32::try_from(whole.len()).unwrap_or(i32::MAX) > PARAMETER.min,
            "case {case:?}: the set must be larger than the smallest page and fit in the \
             largest, got {} items in {answered}",
            whole.len()
        );

        for (first, rest) in steps.iter().copied() {
            let case = format!("{operation} first page {first}, later pages {rest}");
            let mut walked: Vec<Value> = Vec::new();
            let mut size = first;
            let mut token: Option<String> = None;
            loop {
                let mut request = base.clone();
                request["MaxResults"] = json!(size);
                if let Some(token) = token.as_deref() {
                    request["NextToken"] = json!(token);
                }
                let body = ok(&router, operation, &request, &case).await;
                let items_read = page_items(&body, items, &case);
                assert!(
                    !items_read.is_empty(),
                    "case {case:?}: every page of the walk must make progress, got {body}"
                );
                walked.extend(items_read);
                assert!(
                    walked.len() <= whole.len(),
                    "case {case:?}: the walk must not outgrow the set, visited {} items",
                    walked.len()
                );
                match body.get("NextToken").and_then(Value::as_str) {
                    Some(next) => {
                        token = Some(next.to_string());
                        // Every later page is asked for at the OTHER size: the token has
                        // to resume the listing whatever page size carries it.
                        size = rest;
                    }
                    None => break,
                }
            }
            assert_eq!(
                walked, whole,
                "case {case:?}: the walk must visit the whole set once, in the listing's \
                 own order"
            );
        }
    }
}

/// A name that is not a well-formed identifier is refused with the sentence the service
/// words that kind of refusal in — a parameter group's names the REQUEST MEMBER the name
/// arrived in, a cluster's its own label for the identifier plus the service's verdict,
/// the RBAC kinds' that label alone — over every form of malformed name, and a
/// well-formed name is accepted by the same path.
#[tokio::test]
async fn a_malformed_identifier_is_refused_in_the_wording_for_that_kind_of_resource() {
    let router = router();

    // The rows span the spelling clause: every part of the rule, broken one at a time.
    let names = [
        ("beginning with a digit", "9lives"),
        ("beginning with a hyphen", "-leading"),
        ("holding an underscore", "under_score"),
        ("holding a dot", "dotted.name"),
        ("holding a space", "two words"),
        ("ending with a hyphen", "trailing-"),
        ("two consecutive hyphens", "double--hyphen"),
    ];
    // One row per create that validates a name, with the sentence that create's refusal
    // states the rule in: the parameter-group surface names the member, the RBAC and
    // cluster surfaces their own label for the identifier.
    let creates: &[(&str, String)] = &[
        (
            "CreateParameterGroup",
            format!(
                "The parameter ParameterGroupName is not a valid identifier. Identifiers \
                 {IDENTIFIER_CLAUSES}"
            ),
        ),
        ("CreateUser", format!("User id {IDENTIFIER_CLAUSES}")),
        ("CreateACL", format!("ACL name {IDENTIFIER_CLAUSES}")),
        (
            "CreateCluster",
            format!(
                "Cluster name specified is not a valid identifier. Identifiers \
                 {IDENTIFIER_CLAUSES}"
            ),
        ),
    ];

    for (operation, wanted) in creates {
        for (name_case, name) in names {
            let case = format!("{operation}/{name_case}");
            let (status, body) = call(&router, operation, &create_body(operation, name)).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected the modelled invalid-value refusal, got {status} \
                 with {body}"
            );
            assert_eq!(
                &message(&body, &case),
                wanted,
                "case {case:?}: expected the refusal this kind of resource is worded \
                 with, got {body}"
            );
        }
        // The same path accepts a name the rule admits, so the refusals above are the
        // rule and not a blanket one.
        let case = format!("{operation}/well formed");
        ok(
            &router,
            operation,
            &create_body(operation, "well-formed-1"),
            &case,
        )
        .await;
    }
}

/// The create request each name-taking operation carries `name` in. `CreateCluster` is
/// built on the `open-access` ACL every account already has, so the only value under
/// test is the name.
fn create_body(operation: &str, name: &str) -> Value {
    match operation {
        "CreateUser" => json!({
            "UserName": name,
            "AccessString": "on ~* +@all",
            "AuthenticationMode": { "Type": "password", "Passwords": [PASSWORD] },
        }),
        "CreateACL" => json!({ "ACLName": name }),
        "CreateParameterGroup" => json!({ "ParameterGroupName": name, "Family": FAMILY }),
        "CreateCluster" => json!({
            "ClusterName": name,
            "NodeType": "db.t4g.small",
            "ACLName": "open-access",
        }),
        other => panic!("case {other:?}: not a name-taking create"),
    }
}
