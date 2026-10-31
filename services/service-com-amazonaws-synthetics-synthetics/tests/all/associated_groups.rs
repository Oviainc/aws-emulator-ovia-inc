// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListAssociatedGroups` — the membership relation
//! `AssociateResource` writes, read from the CANARY's side.
//!
//! The rule asserted here, stated once and then exercised over a SET of inputs rather than
//! over one recorded request: `ListAssociatedGroups` reports, from persisted state, the
//! identity (`GroupSummary`: `Id`, `Name`, `Arn`) of exactly those groups whose membership
//! holds the named canary — in one total, stable order, paged by `MaxResults`/`NextToken` —
//! and reports a canary this store does not hold as the modelled 404.
//!
//! The rows differ along what that rule generalises over: how many groups the canary
//! belongs to (none, one, several, the documented maximum), which groups those are when a
//! group holds several canaries, the page size, and the ARN the lookup is addressed by.
//!
//! Membership is written through `AssociateResource` and read back through BOTH readers of
//! the same stored relation, so the two sides cannot disagree: `ListGroupResources` (group
//! → canaries) and `ListAssociatedGroups` (canary → groups).
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the emulator
//! boots, through the harness the lifecycle suite defines.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::canary_lifecycle::{Answer, Synthetics};
use crate::group_membership::{
    associate, canary, canary_arn, create_group, error_of, list_resources, ok, resources_of,
    CANARY_NOT_FOUND, MAX_GROUPS_PER_CANARY,
};

/// Comfortably longer than the provisioning span the emulator owes a freshly created
/// canary, so a canary created at the start of a step is out of `CREATING` — and therefore
/// deletable — by the next one.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// `POST /resource/{ResourceArn}/groups` — the `ListAssociatedGroups` route, from the
/// operation's own `@http` binding
/// (`smithy_query(query="operation",operation="ListAssociatedGroups")`).
async fn list_associated(aws: &Synthetics, resource_arn: &str, body: Value) -> Answer {
    aws.call(
        "POST",
        &format!("/resource/{resource_arn}/groups"),
        Some(body),
    )
    .await
}

/// One group's identity as `ListAssociatedGroups` reports it: the three members
/// `GroupSummary` declares (`smithy_query(query="shape",shape="GroupSummary")`).
///
/// `Name` comes first so the derived order is the store's own: groups are keyed by name,
/// "the names for all groups in your account … must be unique", so sorting a set of these
/// by name is the total order a listing is expected to arrive in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Summary {
    name: String,
    id: String,
    arn: String,
}

/// The `Groups` member of a `ListAssociatedGroups` answer, in the order it arrived.
fn groups_of(case: &str, answer: &Answer) -> Vec<Summary> {
    answer
        .at("Groups")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a Groups array, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|entry| {
            let member = |name: &str| {
                entry[name].as_str().map(str::to_string).unwrap_or_else(|| {
                    panic!("case {case}: expected a string {name}, got {entry:?}")
                })
            };
            Summary {
                id: member("Id"),
                name: member("Name"),
                arn: member("Arn"),
            }
        })
        .collect()
}

/// The identity a `CreateGroup` (or `GetGroup`) answer reports for its group, so a listing
/// is compared against what the create itself stored rather than against a literal.
fn summary_of_group(answer: &Answer) -> Summary {
    Summary {
        id: answer.string_at("Group.Id"),
        name: answer.string_at("Group.Name"),
        arn: answer.string_at("Group.Arn"),
    }
}

/// `ListAssociatedGroups` reports exactly the groups the canary is a member of, with the
/// identity each group's own create reported, and it reports nothing for a canary that is a
/// member of none — over a table whose rows differ in how many groups the canary belongs to
/// and in which groups those are.
#[tokio::test]
async fn list_associated_groups_reports_the_groups_the_canary_is_a_member_of() {
    let region = "eu-west-1";
    let aws = Synthetics::new(region);

    // Four groups, so a canary can be in none, one or several of them, and a group can
    // hold more than one canary.
    let mut groups: Vec<Summary> = Vec::new();
    for name in ["alpha-group", "beta-group", "gamma-group", "delta-group"] {
        let created = ok(
            &format!("create-group/{name}"),
            create_group(&aws, json!({ "Name": name })).await,
        );
        groups.push(summary_of_group(&created));
    }
    let by_name = |name: &str| {
        groups
            .iter()
            .find(|group| group.name == name)
            .cloned()
            .unwrap_or_else(|| panic!("the loop above created {name}"))
    };

    // Rows: a canary in no group, in one, in two, in three — and `shared-canary` proves a
    // group holding several canaries still reports per canary.
    let membership: [(&str, Vec<&str>); 4] = [
        ("lonely-canary", vec![]),
        ("single-canary", vec!["beta-group"]),
        ("shared-canary", vec!["alpha-group", "beta-group"]),
        (
            "busy-canary",
            vec!["delta-group", "gamma-group", "alpha-group"],
        ),
    ];

    let mut arns: Vec<(&str, String)> = Vec::new();
    for (name, member_of) in &membership {
        let arn = canary(&aws, region, name).await;
        for group in member_of {
            ok(
                &format!("associate/{name}/{group}"),
                associate(&aws, group, &arn).await,
            );
        }
        arns.push((name, arn));
    }

    for ((name, member_of), (_, arn)) in membership.iter().zip(&arns) {
        let case = format!("list/{name}");
        let answer = ok(&case, list_associated(&aws, arn, json!({})).await);
        let reported = groups_of(&case, &answer);

        // Expected: the identity each group's own create reported, in ascending group-name
        // order — the one total order the store keeps.
        let mut expected: Vec<Summary> = member_of.iter().map(|group| by_name(group)).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case}: expected the {} group(s) this canary was associated with, in one \
             total order, got {reported:?}",
            member_of.len()
        );
        assert!(
            answer.at("NextToken").is_null(),
            "case {case}: expected no NextToken for a complete page, got {:?}",
            answer.at("NextToken")
        );

        // The read is reporting only: a second call answers the same thing, and the other
        // reader of the same stored relation agrees.
        let again = ok(&case, list_associated(&aws, arn, json!({})).await);
        assert_eq!(
            groups_of(&case, &again),
            reported,
            "case {case}: expected a repeated read to report the same groups"
        );
        for group in member_of {
            let row = format!("{case}/resources-of-{group}");
            let listed = ok(&row, list_resources(&aws, group, json!({})).await);
            assert!(
                resources_of(&row, &listed).contains(arn),
                "case {row}: expected the group side of the relation to hold this canary, \
                 got {:?}",
                listed.at("Resources")
            );
        }
    }

    // An association does not outlive the canary: deleting it drops its memberships, so a
    // canary recreated under the same ARN is a member of nothing.
    tokio::time::sleep(PAST_PROVISIONING).await;
    let (_, busy_arn) = arns
        .iter()
        .find(|(name, _)| *name == "busy-canary")
        .expect("the loop above created busy-canary");
    ok("delete-canary", aws.delete("busy-canary").await);
    let recreated = canary(&aws, region, "busy-canary").await;
    assert_eq!(
        &recreated, busy_arn,
        "case recreate: expected the same ARN for the same canary name, got {recreated:?}"
    );
    let after = ok(
        "list/after-delete",
        list_associated(&aws, busy_arn, json!({})).await,
    );
    assert!(
        groups_of("list/after-delete", &after).is_empty(),
        "case list/after-delete: expected a deleted canary to leave no membership behind, \
         got {:?}",
        after.at("Groups")
    );
}

/// `ListAssociatedGroups` pages over the membership set: every page size covers the whole
/// set exactly once in one total order, a token is issued only while entries remain, and a
/// token this operation never issued is a bad request rather than an empty page.
#[tokio::test]
async fn list_associated_groups_pages_over_the_whole_membership_set() {
    let region = "eu-central-1";
    let aws = Synthetics::new(region);

    let arn = canary(&aws, region, "paged-canary").await;
    // The documented maximum a canary can reach — "Any single canary can be a member of up
    // to 10 groups" — so the page sizes below genuinely split the set. The names are
    // deliberately NOT in creation order, so an answer in name order is not an answer in
    // insertion order.
    let mut expected: Vec<Summary> = Vec::new();
    for index in 0..MAX_GROUPS_PER_CANARY {
        let name = format!("group-{:02}", (MAX_GROUPS_PER_CANARY - index) * 7 % 97);
        let created = ok(
            &format!("create-group/{index}"),
            create_group(&aws, json!({ "Name": name.clone() })).await,
        );
        ok(
            &format!("associate/{index}"),
            associate(&aws, &name, &arn).await,
        );
        expected.push(summary_of_group(&created));
    }
    expected.sort();

    // `MaxGroupResults` is `@range 1..20`: both published bounds, plus sizes that divide
    // the set evenly and unevenly.
    for size in [1usize, 3, 4, 10, 20] {
        let case = format!("page-size/{size}");
        let mut seen: Vec<Summary> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({ "MaxResults": size });
            if let Some(next) = &token {
                body["NextToken"] = json!(next);
            }
            let answer = ok(&case, list_associated(&aws, &arn, body).await);
            let page = groups_of(&case, &answer);
            assert!(
                page.len() <= size,
                "case {case}: expected at most {size} entries per page, got {}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            token = answer.at("NextToken").as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= expected.len() + 1,
                "case {case}: expected paging to terminate, got {pages} pages"
            );
        }
        assert_eq!(
            seen, expected,
            "case {case}: expected every group exactly once in ascending name order, got \
             {seen:?}"
        );
        let expected_pages = expected.len().div_ceil(size);
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} page(s), got {pages}"
        );
    }

    // One past each published bound of `MaxGroupResults` (`@range 1..20`) is refused at the
    // request boundary, so no page is served for it at all.
    for (row, size) in [("below-the-floor", 0i32), ("past-the-ceiling", 21)] {
        let case = format!("max-results/{row}");
        let answer = list_associated(&aws, &arn, json!({ "MaxResults": size })).await;
        error_of(&case, &answer, 400, "ValidationException");
    }

    // A token this operation never issued is a bad request, not an empty page. A KEY token
    // naming a group the canary no longer belongs to is NOT forged: a key boundary stays
    // exact across associations, which is the point of the key form.
    for (row, forged) in [
        ("opaque", "not-a-token".to_string()),
        ("wrong-prefix", format!("zz#{}", expected[0].name)),
        // A token the SIBLING paged reader of this relation issues: one operation's token
        // does not decode over another's collection.
        ("other-operations-prefix", format!("rs#{arn}")),
        ("unparsable-offset", "ix#three".to_string()),
        ("offset-past-the-end", format!("ix#{}", expected.len() + 1)),
    ] {
        let case = format!("forged-token/{row}");
        let answer = list_associated(&aws, &arn, json!({ "NextToken": forged })).await;
        error_of(&case, &answer, 400, "ValidationException");
    }
}

/// A canary this store does not hold is reported as the modelled 404, with the service's
/// fixed, identifier-free sentence — over a table of legal `CanaryArn`s that name another
/// canary, another Region or another account.
#[tokio::test]
async fn list_associated_groups_reports_a_missing_canary_as_not_found() {
    let region = "us-east-1";
    let aws = Synthetics::new(region);

    let live = canary(&aws, region, "live-canary").await;
    let group = ok(
        "create-group",
        create_group(&aws, json!({ "Name": "live-group" })).await,
    );
    let identity = summary_of_group(&group);
    ok("associate", associate(&aws, &identity.id, &live).await);

    // Every row is a legal `CanaryArn`
    // (`^arn:(aws[a-zA-Z-]*)?:synthetics:…:\d{12}:canary:[0-9a-z_\-]{1,255}$`), so each one
    // reaches the handler's lookup rather than the request boundary.
    let missing = [
        ("other-name", canary_arn(region, "no-such-canary")),
        ("other-region", canary_arn("eu-west-3", "live-canary")),
        (
            "other-account",
            "arn:aws:synthetics:us-east-1:999999999999:canary:live-canary".to_string(),
        ),
        ("single-character", canary_arn(region, "x")),
        ("longest-name", canary_arn(region, &"c".repeat(255))),
    ];
    let mut sentences: BTreeSet<String> = BTreeSet::new();
    for (row, resource_arn) in &missing {
        let case = format!("list/{row}");
        let answer = list_associated(&aws, resource_arn, json!({})).await;
        let message = error_of(&case, &answer, 404, "ResourceNotFoundException")
            .unwrap_or_else(|| panic!("case {case}: expected a Message, got {:?}", answer.body));
        assert_eq!(
            message, CANARY_NOT_FOUND,
            "case {case}: expected the fixed missing-canary sentence, got {message:?}"
        );
        sentences.insert(message);
    }
    assert_eq!(
        sentences.len(),
        1,
        "case request-independence: expected one fixed message across every ARN, got \
         {sentences:?}"
    );

    // The canary that DOES exist still reports its one membership: nothing above changed
    // the store.
    let listed = ok("list/live", list_associated(&aws, &live, json!({})).await);
    assert_eq!(
        groups_of("list/live", &listed),
        vec![identity],
        "case list/live: expected the one association that was accepted"
    );
    // And the group side of the relation still holds exactly that canary.
    assert_eq!(
        resources_of(
            "list/live-resources",
            &ok(
                "list/live-resources",
                list_resources(&aws, "live-group", json!({})).await
            )
        ),
        vec![live.clone()],
        "case list/live-resources: expected the group to hold the one associated canary"
    );
}
