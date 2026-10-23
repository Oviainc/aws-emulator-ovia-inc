// SPDX-License-Identifier: CC0-1.0

//! What `ListRuleGroupsNamespaces` may answer with, end to end over HTTP.
//!
//! The rule under test: a reporting operation answers ONLY from the records inside the
//! scope the request names — here the one workspace `{workspaceId}` addresses — and every
//! caller-supplied input it consumes (`nextToken`, `name`, `maxResults`) can only NARROW
//! that set, never widen it to a record the request did not name; a value outside a
//! member's modelled constraint is refused with the modelled `ValidationException` (400)
//! and reports nothing at all.
//!
//! That is the property the release gate's `exfil_scan` finding on
//! `src/ops/list_rule_groups_namespaces.rs` leaves unstated: nothing a caller puts on the
//! query string can make the listing serve a namespace held by another workspace, and the
//! cursor it accepts is a filter over the named workspace's own records rather than a key
//! into the store. The existing `rule_groups_namespace_listing` suite walks only the
//! tokens this service itself issued for the SAME workspace; this one spans the tokens a
//! caller can send instead — a token another workspace's listing minted, a name only
//! another workspace holds, a path-traversal-shaped string, the empty string, and a token
//! at the modelled maximum length — plus each published bound of the two string members
//! and one past it.
//!
//! The request harness (`Amp`, `Reply`), the create helper and the listing helpers are
//! the ones `workspace_lifecycle`, `rule_groups_namespace_lifecycle` and
//! `rule_groups_namespace_listing` already build.

use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace};
use crate::rule_groups_namespace_listing::{list_namespaces, names_of, next_token};
use crate::workspace_lifecycle::Amp;

/// The namespaces the workspace under test holds, and the order its listing reports them
/// in (by name). The names span the character class the modelled `RuleGroupsNamespaceName`
/// pattern `[0-9A-Za-z][-.0-9A-Z_a-z]*` allows.
const HOST_NAMES: &[&str] = &["alpha", "alpha.rules", "beta", "rules1", "z"];

/// The namespaces a SECOND workspace holds, which no request naming the first may ever
/// report. One sorts before every name of [`HOST_NAMES`] and one after, so a cursor
/// borrowed from this workspace lands inside the other's range rather than past it.
const OTHER_NAMES: &[&str] = &["other-only", "zz-other"];

/// `PaginationToken`'s modelled bound: `@length(max: 1000)`.
const PAGINATION_TOKEN_MAX: usize = 1000;

/// `RuleGroupsNamespaceName`'s modelled bounds: `@length(min: 1, max: 128)`.
const NAMESPACE_NAME_MIN: usize = 1;
const NAMESPACE_NAME_MAX: usize = 128;

/// An ACTIVE workspace holding one namespace per name in `names`.
async fn seeded_workspace(amp: &Amp, region: &str, alias: &str, names: &[&str]) -> String {
    let workspace_id = amp.active_workspace(region, alias).await;
    for name in names {
        let created = create_namespace(
            amp,
            region,
            &workspace_id,
            &create_body(name, b"groups: []", &[]),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "seeding {name:?} in {alias:?}: the create must answer 202, got {created:?}"
        );
    }
    workspace_id
}

/// Every assertion that holds of a page whatever the caller sent: it reports only names
/// the NAMED workspace holds, in ascending order, and never a name only another workspace
/// holds.
fn assert_confined_to(case: &str, names: &[String], held: &[&str]) {
    for name in names {
        assert!(
            held.contains(&name.as_str()),
            "case {case:?}: the page may only report namespaces the named workspace holds \
             ({held:?}), got {names:?}"
        );
    }
    let mut ascending = names.to_vec();
    ascending.sort();
    assert_eq!(
        *names, ascending,
        "case {case:?}: a page is served in name order, got {names:?}"
    );
    assert_eq!(
        names.len(),
        names.iter().collect::<std::collections::BTreeSet<_>>().len(),
        "case {case:?}: a page must not repeat a namespace, got {names:?}"
    );
}

/// A caller-supplied `nextToken` only ever narrows the page served for the workspace the
/// request names: one row per form a token can take that this workspace's own listing did
/// not mint, including one another workspace's listing did.
#[tokio::test]
async fn a_caller_supplied_cursor_only_narrows_the_named_workspaces_page() {
    let amp = Amp::new();
    let region = "us-west-2";
    let host = seeded_workspace(&amp, region, "cursor-host", HOST_NAMES).await;
    let other = seeded_workspace(&amp, region, "cursor-other", OTHER_NAMES).await;

    // A token each workspace's own listing issued, so the table can carry one this
    // service really minted — for the named workspace, and for the other one.
    let own_page = list_namespaces(&amp, region, &host, &[("maxResults", "1".to_string())]).await;
    let own_token = next_token(&own_page).unwrap_or_else(|| {
        panic!("the first of {} namespaces owes a nextToken, got {own_page:?}", HOST_NAMES.len())
    });
    let other_page = list_namespaces(&amp, region, &other, &[("maxResults", "1".to_string())]).await;
    let foreign_token = next_token(&other_page).unwrap_or_else(|| {
        panic!("the first of {} namespaces owes a nextToken, got {other_page:?}", OTHER_NAMES.len())
    });

    // The expected page of each row is written out rather than computed, so the table
    // states what the rule allows instead of restating the implementation.
    let rows: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "the cursor this workspace's own listing issued",
            own_token,
            vec!["alpha.rules", "beta", "rules1", "z"],
        ),
        (
            "a cursor another workspace's listing issued",
            foreign_token,
            vec!["rules1", "z"],
        ),
        (
            "the name of a namespace only another workspace holds",
            "zz-other".to_string(),
            vec![],
        ),
        (
            "the empty string",
            String::new(),
            HOST_NAMES.to_vec(),
        ),
        (
            "a path-traversal-shaped cursor",
            "../../cursor-other".to_string(),
            HOST_NAMES.to_vec(),
        ),
        (
            "a cursor of reserved characters",
            "?&=#%".to_string(),
            HOST_NAMES.to_vec(),
        ),
        (
            "a cursor at the modelled maximum length, sorting before every name",
            "0".repeat(PAGINATION_TOKEN_MAX),
            HOST_NAMES.to_vec(),
        ),
        (
            "a cursor at the modelled maximum length, sorting after every name",
            "z".repeat(PAGINATION_TOKEN_MAX),
            vec![],
        ),
    ];

    for (case, token, expected) in rows {
        let reply = list_namespaces(&amp, region, &host, &[("nextToken", token.clone())]).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a token inside its modelled bound is served, got {reply:?}"
        );
        let names = names_of(case, &reply);
        assert_eq!(
            names, expected,
            "case {case:?}: expected {expected:?}, got {names:?}"
        );
        assert_confined_to(case, &names, HOST_NAMES);

        // The same token, with the `name` filter as well: the filter narrows further and
        // still reaches nothing outside the named workspace.
        let filtered = list_namespaces(
            &amp,
            region,
            &host,
            &[
                ("nextToken", token.clone()),
                ("name", "alpha".to_string()),
            ],
        )
        .await;
        assert_eq!(
            filtered.status, 200,
            "case {case:?}: the same token with a name filter is served, got {filtered:?}"
        );
        let filtered_names = names_of(case, &filtered);
        for name in &filtered_names {
            assert!(
                name.starts_with("alpha"),
                "case {case:?}: the filter keeps only the names beginning with it, \
                 got {filtered_names:?}"
            );
        }
        assert_confined_to(case, &filtered_names, HOST_NAMES);
    }

    // The other workspace still reports exactly what it holds: nothing the reads above
    // sent changed what a later caller sees, in either workspace.
    let other_after = list_namespaces(&amp, region, &other, &[]).await;
    assert_eq!(
        names_of("the other workspace afterwards", &other_after),
        OTHER_NAMES.to_vec(),
        "the other workspace must still report its own namespaces, got {other_after:?}"
    );
    let host_after = list_namespaces(&amp, region, &host, &[]).await;
    assert_eq!(
        names_of("the named workspace afterwards", &host_after),
        HOST_NAMES.to_vec(),
        "the named workspace must still report every namespace it holds, got {host_after:?}"
    );
}

/// The two string members the caller sends carry modelled constraints —
/// `nextToken` is `@length(max: 1000)` and `name` is `@length(min: 1, max: 128)` with
/// pattern `[0-9A-Za-z][-.0-9A-Z_a-z]*`. Each published bound is served; a value past it,
/// or outside the character class, is refused with the modelled `ValidationException`
/// (400) and reports nothing.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_and_reports_nothing() {
    let amp = Amp::new();
    let region = "eu-central-1";
    let host = seeded_workspace(&amp, region, "bounds-host", HOST_NAMES).await;

    // (case, member, value, expected status)
    let rows: Vec<(&str, &str, String, u16)> = vec![
        (
            "nextToken at the modelled maximum length",
            "nextToken",
            "a".repeat(PAGINATION_TOKEN_MAX),
            200,
        ),
        (
            "nextToken one past the modelled maximum length",
            "nextToken",
            "a".repeat(PAGINATION_TOKEN_MAX + 1),
            400,
        ),
        (
            "name at the modelled minimum length",
            "name",
            "a".repeat(NAMESPACE_NAME_MIN),
            200,
        ),
        (
            "name at the modelled maximum length",
            "name",
            "a".repeat(NAMESPACE_NAME_MAX),
            200,
        ),
        (
            "name one past the modelled maximum length",
            "name",
            "a".repeat(NAMESPACE_NAME_MAX + 1),
            400,
        ),
        // `@pattern` is a FIND, not an anchored match (the trait is not implicitly
        // anchored and this one carries no `^`/`$`), so a value is inside the constraint
        // as soon as it CONTAINS a match: `-alpha` contains `alpha`. Such a value is
        // served — and matches no stored name, because a stored name begins with
        // `[0-9A-Za-z]` — rather than refused.
        (
            "name whose match starts past its first character",
            "name",
            "-alpha".to_string(),
            200,
        ),
        (
            "name carrying a character the class does not allow",
            "name",
            "alpha beta".to_string(),
            200,
        ),
        (
            "name carrying a path separator",
            "name",
            "alpha/rules".to_string(),
            200,
        ),
        // A value the pattern matches NOWHERE: every character is outside
        // `[0-9A-Za-z]`, so the member is outside its modelled constraint.
        (
            "name the pattern matches nowhere",
            "name",
            "-".to_string(),
            400,
        ),
        (
            "name of nothing but characters outside the pattern's first class",
            "name",
            "-.-".to_string(),
            400,
        ),
    ];

    for (case, member, value, expected_status) in rows {
        let reply = list_namespaces(&amp, region, &host, &[(member, value.clone())]).await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {reply:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {reply:?}"
            );
            assert!(
                reply.body["ruleGroupsNamespaces"].is_null(),
                "case {case:?}: a refused call reports no namespace at all, got {reply:?}"
            );
        } else {
            assert_confined_to(case, &names_of(case, &reply), HOST_NAMES);
        }
    }

    // The control: after every refusal the workspace still reports every namespace it
    // holds, so a refused read consumed nothing.
    let after = list_namespaces(&amp, region, &host, &[]).await;
    assert_eq!(
        names_of("after the refusals", &after),
        HOST_NAMES.to_vec(),
        "a refused read must leave what a later caller sees untouched, got {after:?}"
    );
}
