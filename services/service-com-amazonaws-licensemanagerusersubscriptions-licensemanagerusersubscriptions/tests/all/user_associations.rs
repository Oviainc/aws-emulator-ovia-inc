// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListUserAssociations`, driven over the real wire path: every call
//! goes through the router's `dispatch`, so the request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * the list REPORTS the associations state holds for the `(IdentityProvider, InstanceId)`
//!   pair the request names — what `AssociateUser` persisted is what the list answers,
//!   member for member, in association order, and an association of another directory or
//!   another instance is not in that list;
//! * `Filters` select on the attributes the operation's model documents (`Status`,
//!   `Username`, `Domain`) and refuse any other attribute, or a comparison this service
//!   does not implement, whatever the store holds;
//! * `MaxResults`/`NextToken` page the list exactly once over every element, and a token
//!   that is not a position in THIS list is refused;
//! * the status transition the clock owes is materialised on a later read — an
//!   association is answered `ASSOCIATING`, then `ASSOCIATED`, and one `DisassociateUser`
//!   ended stays reported in `DISASSOCIATING` until the teardown the clock owes completes
//!   — and nothing else a read could change is changed: two identical calls answer
//!   identically;
//! * the request is authorized before anything else about it is examined — an `InstanceId`
//!   the account's EC2 fleet does not hold fails closed — and a directory no live
//!   registration is for is the modelled `ResourceNotFoundException`.
//!
//! The harness (router + engine + the account's directory fleet the registrations resolve
//! against) is the one in [`super::identity_provider_subscriptions`]; only the rules are
//! new here.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::{
    seed_account_instances, IDENTITY_PROVIDER_NOT_REGISTERED,
};
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState as ServiceState;

// ONE wire harness per service: these come from the sibling suite rather than a second
// copy of the same dispatch plumbing.
use crate::identity_provider_subscriptions::{
    arn_prefix, as_str, call, harness, provider_union, register, Harness, Reply,
    PAST_THE_OWED_TRANSITION, PRODUCTS,
};

/// A directory of the account's fleet (`Active`, so a registration resolves for it), and a
/// second one, so a rule that names "the" directory is exercised on more than one.
const DIRECTORY_A: &str = "d-0123456789";
const DIRECTORY_B: &str = "d-00000000ff";
/// A third directory the harness puts in the account's fleet but no row registers an
/// identity provider for, so it is a resolvable directory with no registration.
const UNREGISTERED_DIRECTORY: &str = "d-123456789a";

/// The status a user association carries while the instance is being configured, and the
/// one it reaches once that completed. Spelled out here rather than read from the
/// production constants, so a change to them fails a test instead of silently changing
/// what this suite accepts.
const STATUS_ASSOCIATING: &str = "ASSOCIATING";
const STATUS_ASSOCIATED: &str = "ASSOCIATED";
/// The status an association carries from the moment `DisassociateUser` accepted its
/// disassociation until the teardown that status announces has completed.
const STATUS_DISASSOCIATING: &str = "DISASSOCIATING";

/// The sentence this service fails closed with (`scaffolded::UNAUTHORIZED_EXCEPTION_MESSAGE`),
/// and the status it reports such a refusal with (`scaffolded::SERVICE_REFUSAL_STATUS`).
const UNAUTHORIZED_MESSAGE: &str =
    "Unauthorized. Please check the request and access policy and try again.";
const REFUSAL_STATUS: u16 = 400;

async fn associate(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/AssociateUser", body).await
}

async fn disassociate(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/DisassociateUser", body).await
}

/// `ListUserAssociations` is bound to POST /user/ListUserAssociations (smithy_query:
/// operation ListUserAssociations, http.uri).
async fn list_associations(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/ListUserAssociations", body).await
}

/// One EC2 instance of the account's seeded fleet (`state::seed_account_instances`): the
/// only instances an association — or a list of associations — is authorized for.
fn held_instance(index: usize) -> String {
    seed_account_instances()[index].0.clone()
}

/// One association to create. The rows differ along what the rules generalise over: the
/// user name's form, which instance of the fleet it is on, which directory it is through,
/// whether a `Domain` is supplied, and whether tags are.
#[derive(Debug, Clone, Copy)]
struct Association {
    case: &'static str,
    username: &'static str,
    /// Index into the account's seeded EC2 fleet, so the row names an instance the
    /// authorization gate holds without hard-coding its id.
    instance: usize,
    directory: &'static str,
    domain: Option<&'static str>,
    tagged: bool,
}

/// `Username`, `InstanceId` and `Domain` are plain `String`s in this service's model (no
/// pattern to honour), so the rows vary in FORM and CASE rather than against a constraint.
const ASSOCIATIONS: &[Association] = &[
    Association {
        case: "no domain, first instance, first directory",
        username: "alice",
        instance: 0,
        directory: DIRECTORY_A,
        domain: None,
        tagged: false,
    },
    Association {
        case: "a domain-qualified user in the same scope, tagged",
        username: "bob",
        instance: 0,
        directory: DIRECTORY_A,
        domain: Some("corp.example"),
        tagged: true,
    },
    Association {
        case: "a user name that extends another row's, so a prefix search selects both",
        username: "alice.admin",
        instance: 0,
        directory: DIRECTORY_A,
        domain: Some("corp.example"),
        tagged: false,
    },
    Association {
        case: "an upper-case user name, and a domain only this row carries",
        username: "CAROL",
        instance: 0,
        directory: DIRECTORY_A,
        domain: Some("eu.corp.example"),
        tagged: false,
    },
    Association {
        case: "the same user name on a second instance of the fleet",
        username: "alice",
        instance: 1,
        directory: DIRECTORY_A,
        domain: None,
        tagged: false,
    },
    Association {
        case: "the same user name on the same instance through a second directory",
        username: "alice",
        instance: 0,
        directory: DIRECTORY_B,
        domain: Some("other.example"),
        tagged: true,
    },
    Association {
        case: "a third instance, through the second directory",
        username: "dave",
        instance: 2,
        directory: DIRECTORY_B,
        domain: None,
        tagged: false,
    },
];

impl Association {
    fn request(&self) -> Value {
        let mut body = json!({
            "Username": self.username,
            "InstanceId": held_instance(self.instance),
            "IdentityProvider": provider_union(self.directory),
        });
        if let Some(domain) = self.domain {
            body["Domain"] = json!(domain);
        }
        if self.tagged {
            body["Tags"] = json!({ "team": "licensing", "case": self.case });
        }
        body
    }

    /// Whether this row is in the list a request for `(directory, instance)` answers.
    fn in_scope(&self, directory: &str, instance: usize) -> bool {
        self.directory == directory && self.instance == instance
    }
}

/// The `(directory, instance)` scopes the table covers, in table order.
fn scopes() -> Vec<(&'static str, usize)> {
    let mut scopes: Vec<(&'static str, usize)> = Vec::new();
    for row in ASSOCIATIONS {
        if !scopes.contains(&(row.directory, row.instance)) {
            scopes.push((row.directory, row.instance));
        }
    }
    scopes
}

/// A `ListUserAssociations` request for one scope, plus whatever members a case adds.
fn list_request(directory: &str, instance: usize, members: &[(&str, Value)]) -> Value {
    let mut body = json!({
        "InstanceId": held_instance(instance),
        "IdentityProvider": provider_union(directory),
    });
    for (name, value) in members {
        body[*name] = value.clone();
    }
    body
}

/// One filter, in the wire form `Filter` models: `Attribute`, an optional `Operation` and
/// `Value`.
fn filter(attribute: &str, operation: Option<&str>, value: &str) -> Value {
    let mut filter = json!({ "Attribute": attribute, "Value": value });
    if let Some(operation) = operation {
        filter["Operation"] = json!(operation);
    }
    filter
}

fn settings() -> Value {
    json!({ "Subnets": ["subnet-0123456789abcdef0"], "SecurityGroupId": "sg-0123abcd" })
}

/// Register an identity provider for `directory` — the registration a user association
/// resolves through — and answer nothing: a row's association is what the tests read.
async fn register_directory(h: &Harness, directory: &str, product: &str) {
    register(
        h,
        &json!({
            "IdentityProvider": provider_union(directory),
            "Product": product,
            "Settings": settings(),
        }),
    )
    .await
    .ok("the registration the associations resolve through");
}

/// Create every association in the table, in order, and answer the
/// `InstanceUserSummary` each `AssociateUser` returned, keyed by case.
async fn seed(h: &Harness) -> Vec<(&'static str, Value)> {
    register_directory(h, DIRECTORY_A, PRODUCTS[0]).await;
    register_directory(h, DIRECTORY_B, PRODUCTS[1]).await;
    let mut created = Vec::new();
    for row in ASSOCIATIONS {
        let reply = associate(h, &row.request()).await;
        let summary = reply.ok(row.case)["InstanceUserSummary"].clone();
        created.push((row.case, summary));
    }
    created
}

fn summaries<'a>(reply: &'a Reply, case: &str) -> &'a Vec<Value> {
    reply.ok(case)["InstanceUserSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: no InstanceUserSummaries array in {:?}",
                reply.body
            )
        })
}

fn listed_usernames(reply: &Reply, case: &str) -> Vec<String> {
    summaries(reply, case)
        .iter()
        .map(|summary| as_str(summary, "Username", case).to_string())
        .collect()
}

fn listed_arns(reply: &Reply, case: &str) -> Vec<String> {
    summaries(reply, case)
        .iter()
        .map(|summary| as_str(summary, "InstanceUserArn", case).to_string())
        .collect()
}

/// A summary with its clock-owed `Status` taken out, so what the create returned and what
/// the list reports can be compared member for member without racing the transition the
/// clock owes.
fn without_status(summary: &Value) -> Value {
    let mut summary = summary.clone();
    if let Some(object) = summary.as_object_mut() {
        object.remove("Status");
    }
    summary
}

/// RULE: a scoped reporting list answers from state — for the
/// `(IdentityProvider, InstanceId)` pair the request names, it reports exactly the
/// associations state holds for that pair, every member as the create persisted it, in
/// association order; a pair with no association is an empty list, not a refusal.
#[tokio::test]
async fn every_association_in_scope_is_reported_exactly_as_stored() {
    let h = harness();

    // A scope with no association is an empty list and no token — what AWS answers for an
    // instance through which nobody has been associated.
    register_directory(&h, DIRECTORY_A, PRODUCTS[0]).await;
    let empty = list_associations(&h, &list_request(DIRECTORY_A, 0, &[])).await;
    assert!(
        summaries(&empty, "a scope with no association").is_empty(),
        "an instance with no association must answer an empty InstanceUserSummaries, got {:?}",
        empty.body
    );
    assert!(
        empty
            .ok("a scope with no association")
            .get("NextToken")
            .is_none(),
        "a complete page must carry no NextToken, got {:?}",
        empty.body
    );

    let h = harness();
    let created = seed(&h).await;

    for (directory, instance) in scopes() {
        let case = format!("scope {directory}/instance {instance}");
        let reply = list_associations(&h, &list_request(directory, instance, &[])).await;
        let listed = summaries(&reply, &case);
        let expected: Vec<&Association> = ASSOCIATIONS
            .iter()
            .filter(|row| row.in_scope(directory, instance))
            .collect();
        assert_eq!(
            listed.len(),
            expected.len(),
            "case {case:?}: expected the {} association(s) of this scope, got {listed:?}",
            expected.len()
        );

        for (row, summary) in expected.iter().zip(listed.iter()) {
            assert_eq!(
                as_str(summary, "Username", row.case),
                row.username,
                "case {:?}: the list must be in association order and carry the stored user name, got {summary:?}",
                row.case
            );
            assert_eq!(
                as_str(summary, "InstanceId", row.case),
                held_instance(row.instance),
                "case {:?}: expected the stored instance, got {summary:?}",
                row.case
            );
            assert_eq!(
                summary.get("IdentityProvider").cloned(),
                Some(provider_union(row.directory)),
                "case {:?}: expected the stored identity provider union, got {summary:?}",
                row.case
            );
            assert_eq!(
                summary.get("Domain").and_then(Value::as_str),
                row.domain,
                "case {:?}: Domain must be the stored one, or absent, got {summary:?}",
                row.case
            );
            let arn = as_str(summary, "InstanceUserArn", row.case);
            assert!(
                arn.starts_with(&arn_prefix("instance-user")),
                "case {:?}: expected an ARN under {}, got {arn:?}",
                row.case,
                arn_prefix("instance-user")
            );
            assert!(
                summary.get("StatusMessage").is_none(),
                "case {:?}: a StatusMessage must not be invented, got {summary:?}",
                row.case
            );
            assert!(
                summary.get("DisassociationDate").is_none(),
                "case {:?}: a live association has no DisassociationDate, got {summary:?}",
                row.case
            );
            let status = as_str(summary, "Status", row.case);
            assert!(
                status == STATUS_ASSOCIATING || status == STATUS_ASSOCIATED,
                "case {:?}: expected {STATUS_ASSOCIATING} or {STATUS_ASSOCIATED}, got {status:?}",
                row.case
            );

            // The round trip: what the create answered is what the list reports, member for
            // member, apart from the one member the clock moves.
            let from_create = created
                .iter()
                .find(|(case, _)| *case == row.case)
                .map(|(_, summary)| summary)
                .unwrap_or_else(|| panic!("case {:?}: nothing was created for it", row.case));
            assert_eq!(
                without_status(summary),
                without_status(from_create),
                "case {:?}: the list must report what AssociateUser persisted, got {summary:?}",
                row.case
            );
        }
    }

    // Every association is reported by exactly ONE scope, so the scoping selects rather
    // than duplicates: the ARNs of all the scopes' lists are the ARNs of every row.
    let mut all = Vec::new();
    for (directory, instance) in scopes() {
        let case = format!("scope {directory}/instance {instance}");
        let reply = list_associations(&h, &list_request(directory, instance, &[])).await;
        all.extend(listed_arns(&reply, &case));
    }
    let mut unique = all.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ASSOCIATIONS.len(),
        "each association must be reported by exactly one scope, got {all:?}"
    );
}

/// RULE: `Filters` select the associations whose documented filterable attribute satisfies
/// every filter in the set; a set is a conjunction, and an absent `Operation` is equality.
#[tokio::test]
async fn filters_select_on_the_attributes_the_model_documents() {
    let h = harness();
    seed(&h).await;
    // Past the transition the clock owes, so a `Status` filter is asserted against the
    // status every association has reached rather than racing it.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    // The attributes `ListUserAssociations` documents as filterable: "You can use the
    // following filters to streamline results: Status Username Domain" (smithy_query doc,
    // ListUserAssociationsRequest$Filters).
    type Predicate = fn(&Association) -> bool;
    let cases: &[(&str, Value, Predicate)] = &[
        (
            "Status, operation omitted (equality)",
            json!([filter("Status", None, STATUS_ASSOCIATED)]),
            |_row| true,
        ),
        (
            "Status eq a status no association has reached",
            json!([filter("Status", Some("eq"), STATUS_ASSOCIATING)]),
            |_row| false,
        ),
        (
            "Status ne",
            json!([filter("Status", Some("ne"), STATUS_ASSOCIATING)]),
            |_row| true,
        ),
        (
            "Username equality",
            json!([filter("Username", Some("eq"), "alice")]),
            |row| row.username == "alice",
        ),
        (
            "Username Equals (PascalCase spelling)",
            json!([filter("Username", Some("Equals"), "bob")]),
            |row| row.username == "bob",
        ),
        (
            "Username BeginsWith selects a name and the one that extends it",
            json!([filter("Username", Some("BeginsWith"), "alice")]),
            |row| row.username.starts_with("alice"),
        ),
        (
            "Username Contains",
            json!([filter("Username", Some("Contains"), "admin")]),
            |row| row.username.contains("admin"),
        ),
        (
            "Username equality is case sensitive",
            json!([filter("Username", None, "carol")]),
            |_row| false,
        ),
        (
            "Username equality on the upper-case spelling",
            json!([filter("Username", None, "CAROL")]),
            |row| row.username == "CAROL",
        ),
        (
            "Domain equality",
            json!([filter("Domain", None, "corp.example")]),
            |row| row.domain == Some("corp.example"),
        ),
        (
            "Domain Contains, which a sub-domain also satisfies",
            json!([filter("Domain", Some("Contains"), "corp.example")]),
            |row| row.domain.is_some_and(|d| d.contains("corp.example")),
        ),
        (
            "Domain equality on the empty value selects the associations carrying none",
            json!([filter("Domain", None, "")]),
            |row| row.domain.is_none(),
        ),
        (
            "Domain ne",
            json!([filter("Domain", Some("ne"), "corp.example")]),
            |row| row.domain != Some("corp.example"),
        ),
        (
            "two filters are a conjunction",
            json!([
                filter("Username", Some("BeginsWith"), "alice"),
                filter("Domain", None, "corp.example")
            ]),
            |row| row.username.starts_with("alice") && row.domain == Some("corp.example"),
        ),
        (
            "a filter nothing satisfies",
            json!([filter("Username", None, "nobody")]),
            |_row| false,
        ),
    ];

    for (case, filters, matches) in cases {
        // Every case is asserted on each scope, so a filter is shown to select WITHIN the
        // scope rather than across the store.
        for (directory, instance) in scopes() {
            let case = format!("{case} on {directory}/instance {instance}");
            let reply = list_associations(
                &h,
                &list_request(directory, instance, &[("Filters", filters.clone())]),
            )
            .await;
            let expected: Vec<String> = ASSOCIATIONS
                .iter()
                .filter(|row| row.in_scope(directory, instance) && matches(row))
                .map(|row| row.username.to_string())
                .collect();
            assert_eq!(
                listed_usernames(&reply, &case),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                reply.body
            );
        }
    }
}

/// RULE: a filter naming an attribute the operation does not expose, or a comparison the
/// service does not implement, is a `ValidationException` (HTTP 400) — and because
/// well-formedness does not depend on the store, it is refused on a scope that holds no
/// association too.
#[tokio::test]
async fn a_filter_the_operation_does_not_expose_is_refused() {
    let cases = [
        (
            "an attribute of another list operation",
            filter("InstanceId", None, "i-0abc1234"),
        ),
        (
            "another list operation's attribute",
            filter("Product", None, PRODUCTS[0]),
        ),
        (
            "a response member that is not filterable",
            filter("InstanceUserArn", None, "arn:aws:x:y:z:instance-user/iu-1"),
        ),
        (
            "a plural of a documented attribute",
            filter("Usernames", None, "alice"),
        ),
        (
            "the wrong case of a documented attribute",
            filter("username", None, "alice"),
        ),
        (
            "another wrong case of a documented attribute",
            filter("DOMAIN", None, "corp.example"),
        ),
        ("an empty attribute name", filter("", None, "alice")),
        (
            "a comparison this service does not implement",
            filter("Username", Some("like"), "alice"),
        ),
        ("an empty operation", filter("Username", Some(""), "alice")),
    ];

    for (case, bogus) in cases {
        // Refused whatever the store holds: once on a scope with associations, once on a
        // scope that has none.
        let h = harness();
        seed(&h).await;
        for (store, instance) in [("a scope with associations", 0), ("an empty scope", 2)] {
            let case = format!("{case} on {store}");
            let reply = list_associations(
                &h,
                &list_request(DIRECTORY_A, instance, &[("Filters", json!([bogus.clone()]))]),
            )
            .await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: ValidationException is modelled at HTTP 400, got {reply:?}"
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {reply:?}"
            );
        }
    }
}

/// RULE: `MaxResults` + `NextToken` walk the scope's list exactly once over every element,
/// whatever the page size, and the last page carries no token.
#[tokio::test]
async fn paging_walks_every_association_in_scope_exactly_once() {
    let h = harness();
    seed(&h).await;

    for (directory, instance) in scopes() {
        let all: Vec<String> = ASSOCIATIONS
            .iter()
            .filter(|row| row.in_scope(directory, instance))
            .map(|row| row.username.to_string())
            .collect();

        for page_size in 1..=all.len() + 1 {
            let case = format!("{directory}/instance {instance}, page size {page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut members: Vec<(&str, Value)> = vec![("MaxResults", json!(page_size))];
                if let Some(token) = &token {
                    members.push(("NextToken", json!(token)));
                }
                let reply = list_associations(&h, &list_request(directory, instance, &members)).await;
                let page = summaries(&reply, &case);
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: a page may not exceed MaxResults, got {page:?}"
                );
                seen.extend(listed_usernames(&reply, &case));
                token = reply
                    .body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let remaining = all.len() - seen.len();
                assert_eq!(
                    token.is_some(),
                    remaining > 0,
                    "case {case:?}: a token must be present exactly while elements remain ({remaining} left), got {reply:?}"
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                seen, all,
                "case {case:?}: paging must walk every association of the scope exactly once, in association order"
            );
        }
    }
}

/// RULE: a page size that cannot make progress, and a token that is not a position in THIS
/// list — another scope's, another filter set's, or none of ours — are refused with
/// `ValidationException` (HTTP 400) rather than silently restarting the list.
#[tokio::test]
async fn a_page_request_that_names_no_position_in_this_list_is_refused() {
    let h = harness();
    seed(&h).await;

    // A token minted by the list of ANOTHER scope: the same directory on another instance
    // of the fleet, and the same instance through another directory.
    let token_of = |reply: &Reply, case: &str| -> String {
        reply.ok(case)["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a truncated page carries a NextToken"))
            .to_string()
    };
    let first_page = list_associations(
        &h,
        &list_request(DIRECTORY_A, 0, &[("MaxResults", json!(1))]),
    )
    .await;
    let own = token_of(&first_page, "the scope's own first page");
    let other_directory = list_associations(
        &h,
        &list_request(DIRECTORY_B, 0, &[("MaxResults", json!(1))]),
    )
    .await;
    let other_filters = list_associations(
        &h,
        &list_request(
            DIRECTORY_A,
            0,
            &[
                ("MaxResults", json!(1)),
                (
                    "Filters",
                    json!([filter("Username", Some("BeginsWith"), "alice")]),
                ),
            ],
        ),
    )
    .await;
    let other_filters = token_of(&other_filters, "a filtered first page");

    // The scope's own token IS honoured, so the rows below pin a refusal and not a
    // blanket rejection of every token.
    let resumed = list_associations(
        &h,
        &list_request(
            DIRECTORY_A,
            0,
            &[("MaxResults", json!(1)), ("NextToken", json!(own))],
        ),
    )
    .await;
    assert_eq!(
        listed_usernames(&resumed, "the scope's own token"),
        vec![ASSOCIATIONS[1].username.to_string()],
        "the list's own cursor must resume it, got {:?}",
        resumed.body
    );

    let cases: [(&str, Vec<(&str, Value)>); 6] = [
        ("a page size of zero", vec![("MaxResults", json!(0))]),
        ("a negative page size", vec![("MaxResults", json!(-1))]),
        (
            "a token that is not hex",
            vec![("NextToken", json!("not-a-token"))],
        ),
        (
            "a hex token from no list of ours",
            vec![("NextToken", json!("6f746865723a30"))],
        ),
        (
            "a token minted by another scope's list",
            vec![(
                "NextToken",
                json!(other_directory
                    .body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .unwrap_or("6f746865723a30")),
            )],
        ),
        (
            "a token minted under other filters",
            vec![("NextToken", json!(other_filters))],
        ),
    ];
    for (case, members) in cases {
        let reply = list_associations(&h, &list_request(DIRECTORY_A, 0, &members)).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected a 400 refusal, got {reply:?}"
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {reply:?}"
        );
    }
}

/// RULE: the list reports the status the CLOCK reached — an association is answered in the
/// non-terminal `ASSOCIATING` it was created in, and a later read sees the `ASSOCIATED` the
/// clock owed — and an association `DisassociateUser` ended is no longer in the list.
#[tokio::test]
async fn the_list_reports_the_status_the_clock_reached() {
    let h = harness();
    register_directory(&h, DIRECTORY_A, PRODUCTS[0]).await;

    // Two rows, so the transition is asserted on more than one association: one with a
    // domain and one without.
    let rows = [ASSOCIATIONS[0], ASSOCIATIONS[1]];
    for row in &rows {
        associate(&h, &row.request()).await.ok(row.case);
    }

    let immediate = list_associations(&h, &list_request(DIRECTORY_A, 0, &[])).await;
    for (row, summary) in rows.iter().zip(summaries(&immediate, "immediately").iter()) {
        assert_eq!(
            as_str(summary, "Status", row.case),
            STATUS_ASSOCIATING,
            "case {:?}: a caller reading straight after the association must still see {STATUS_ASSOCIATING}, got {summary:?}",
            row.case
        );
    }

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let later = list_associations(&h, &list_request(DIRECTORY_A, 0, &[])).await;
    for (row, summary) in rows.iter().zip(summaries(&later, "past the span").iter()) {
        assert_eq!(
            as_str(summary, "Status", row.case),
            STATUS_ASSOCIATED,
            "case {:?}: the transition the clock owed must be materialised on a later read, got {summary:?}",
            row.case
        );
        assert!(
            as_str(summary, "AssociationDate", row.case)
                .ends_with('Z'),
            "case {:?}: AssociationDate must be the stored ISO-8601 instant, got {summary:?}",
            row.case
        );
    }

    // Each of the two ways `DisassociateUser` addresses an association: by its ARN, and by
    // the identifier triple. A disassociation is ASYNCHRONOUS, so both stay REPORTED in the
    // non-terminal `DISASSOCIATING` while the teardown the clock owes runs — a caller
    // polling this list sees it — and leave the list once that teardown has completed.
    let arns = listed_arns(&later, "past the span");
    disassociate(&h, &json!({ "InstanceUserArn": arns[0] }))
        .await
        .ok("by ARN");
    disassociate(
        &h,
        &json!({
            "Username": rows[1].username,
            "InstanceId": held_instance(rows[1].instance),
            "IdentityProvider": provider_union(rows[1].directory),
        }),
    )
    .await
    .ok("by the identifier triple");
    let tearing_down = list_associations(&h, &list_request(DIRECTORY_A, 0, &[])).await;
    let listed = summaries(&tearing_down, "while the teardown runs");
    assert_eq!(
        listed.len(),
        rows.len(),
        "an association being torn down must still be reported, got {:?}",
        tearing_down.body
    );
    for (row, summary) in rows.iter().zip(listed.iter()) {
        assert_eq!(
            as_str(summary, "Status", row.case),
            STATUS_DISASSOCIATING,
            "case {:?}: a caller polling straight after the disassociation must see {STATUS_DISASSOCIATING}, got {summary:?}",
            row.case
        );
        assert!(
            as_str(summary, "DisassociationDate", row.case).ends_with('Z'),
            "case {:?}: the instant the disassociation was accepted must be reported, got {summary:?}",
            row.case
        );
    }

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let after = list_associations(&h, &list_request(DIRECTORY_A, 0, &[])).await;
    assert!(
        summaries(&after, "after both teardowns completed").is_empty(),
        "an association whose teardown the clock completed must leave the list, got {:?}",
        after.body
    );
}

/// RULE: a reporting operation changes nothing a later caller sees — repeated reads, with
/// and without filters or paging, answer identically and leave the store as it was.
#[tokio::test]
async fn listing_associations_changes_nothing_a_later_caller_sees() {
    let h = harness();
    seed(&h).await;
    // Past the clock-owed transition first, so what is asserted below is that the READ
    // changes nothing — not that the clock does not.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let requests = [
        ("unfiltered", list_request(DIRECTORY_A, 0, &[])),
        (
            "filtered",
            list_request(
                DIRECTORY_A,
                0,
                &[("Filters", json!([filter("Domain", None, "corp.example")]))],
            ),
        ),
        (
            "first page",
            list_request(DIRECTORY_A, 0, &[("MaxResults", json!(2))]),
        ),
        ("another scope", list_request(DIRECTORY_B, 2, &[])),
    ];
    for (case, request) in &requests {
        let first = list_associations(&h, request).await;
        let second = list_associations(&h, request).await;
        assert_eq!(
            first.ok(case),
            second.ok(case),
            "case {case:?}: a read must answer the same twice, got {:?} then {:?}",
            first.body,
            second.body
        );
    }

    let store = h.engine.state::<ServiceState>();
    let guard = store.lock();
    assert_eq!(
        guard.instance_user_order.len(),
        ASSOCIATIONS.len(),
        "a read must not add or drop an association: {:?}",
        guard.instance_user_order
    );
    for arn in &guard.instance_user_order {
        let record = guard
            .instance_users
            .get(arn)
            .unwrap_or_else(|| panic!("{arn} is indexed but not held"));
        assert_eq!(
            record.status, STATUS_ASSOCIATED,
            "a read must not move a status, got {record:?}"
        );
        assert!(
            record.disassociation_date.is_none(),
            "a read must not end an association, got {record:?}"
        );
    }
}

/// RULE: the request is authorized before anything else about it is examined — an
/// `InstanceId` the account's EC2 fleet does not hold is refused with the fixed
/// `UnauthorizedException` sentence, whatever else the request says.
#[tokio::test]
async fn the_instance_gate_fails_closed_before_anything_else_is_examined() {
    let h = harness();
    seed(&h).await;

    let held = held_instance(0);
    let near_miss = format!("{}0", &held[..held.len() - 1]);
    // `InstanceId` is a plain `String` in this service's model, so the rows vary in FORM
    // and SIZE rather than against a pattern: EC2's two id lengths, a near miss on an
    // instance the fleet does hold, its uppercase spelling, and a value that is not an
    // instance id at all.
    let unheld: [(&str, String); 5] = [
        ("the 17-hex-digit id form", "i-0123456789abcdef0".to_string()),
        ("the 8-hex-digit id form", "i-0abc1234".to_string()),
        ("a near miss on a held instance", near_miss),
        (
            "an uppercase spelling of a held instance",
            held.to_ascii_uppercase(),
        ),
        ("a value that is not an instance id", "not-an-instance".to_string()),
    ];
    // What else is wrong with the request: nothing, a filter the operation does not
    // expose, a page size that cannot make progress, a token of no list, and an identity
    // provider that names no directory at all. Every row gets the SAME answer.
    let also_wrong: [(&str, Value); 5] = [
        ("nothing else", json!({})),
        (
            "a filter the operation does not expose",
            json!({ "Filters": [filter("Nope", None, "x")] }),
        ),
        ("a page size of zero", json!({ "MaxResults": 0 })),
        ("a token of no list", json!({ "NextToken": "not-a-token" })),
        (
            "an identity provider naming no directory",
            json!({ "IdentityProvider": { "ActiveDirectoryIdentityProvider": {} } }),
        ),
    ];

    for (row, (class, instance_id)) in unheld.iter().enumerate() {
        let (also, extra) = &also_wrong[row % also_wrong.len()];
        let case = format!("{class}, {also}");
        let mut body = json!({
            "InstanceId": instance_id,
            "IdentityProvider": provider_union(DIRECTORY_A),
        });
        for (name, value) in extra.as_object().expect("a JSON object of members") {
            body[name.as_str()] = value.clone();
        }
        let reply = list_associations(&h, &body).await;
        assert_eq!(
            reply.status, REFUSAL_STATUS,
            "case {case:?}: expected the service's refusal status, got {reply:?}"
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("UnauthorizedException"),
            "case {case:?}: expected UnauthorizedException, got {reply:?}"
        );
        assert_eq!(
            reply.message(),
            Some(UNAUTHORIZED_MESSAGE),
            "case {case:?}: the fail-closed sentence must disclose nothing, got {reply:?}"
        );
    }

    // The gate is not a blanket refusal: the same request against an instance the fleet
    // DOES hold is answered from state.
    let allowed = list_associations(&h, &list_request(DIRECTORY_A, 0, &[])).await;
    assert!(
        !summaries(&allowed, "an authorized scope").is_empty(),
        "an authorized scope must be answered from state, got {:?}",
        allowed.body
    );
}

/// RULE: an identity provider that no LIVE registration of this account is for has no
/// associations to report, which is the modelled `ResourceNotFoundException` (HTTP 404) —
/// the same refusal `AssociateUser` answers for the same resolution; and a provider union
/// that names no directory at all is a `ValidationException`.
#[tokio::test]
async fn an_unregistered_identity_provider_is_a_not_found() {
    let h = harness();
    seed(&h).await;

    // The rows differ in HOW the provider fails to resolve to a registration: a directory
    // of the account's fleet nobody registered, an id the fleet does not hold at all, and
    // a self-managed provider addressed by domain name.
    let unregistered: [(&str, Value); 4] = [
        (
            "a directory of the account's fleet with no registration",
            provider_union(UNREGISTERED_DIRECTORY),
        ),
        (
            "a directory the account's fleet does not hold",
            provider_union("d-9999999999"),
        ),
        (
            "a shared directory nobody registered",
            provider_union("sd-abcdef0123"),
        ),
        (
            "a self-managed provider addressed by domain name",
            json!({
                "ActiveDirectoryIdentityProvider": {
                    "ActiveDirectorySettings": { "DomainName": "self.managed.example" }
                }
            }),
        ),
    ];
    for (case, provider) in &unregistered {
        let reply = list_associations(
            &h,
            &json!({ "InstanceId": held_instance(0), "IdentityProvider": provider }),
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: ResourceNotFoundException is modelled at HTTP 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {reply:?}"
        );
        assert_eq!(
            reply.message(),
            Some(IDENTITY_PROVIDER_NOT_REGISTERED),
            "case {case:?}: the user-association plane states this refusal in one sentence, got {reply:?}"
        );
    }

    // A provider union that names neither a `DirectoryId` nor a domain name addresses no
    // directory at all: a bad parameter, not a missing resource.
    let reply = list_associations(
        &h,
        &json!({
            "InstanceId": held_instance(0),
            "IdentityProvider": { "ActiveDirectoryIdentityProvider": {} },
        }),
    )
    .await;
    assert_eq!(
        reply.status, 400,
        "an identity provider naming no directory is a 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "an identity provider naming no directory is a ValidationException, got {reply:?}"
    );
}
