// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DNS Firewall domain-list slice:
//! `CreateFirewallDomainList` and `GetFirewallDomainList`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};

use super::harness::{
    call, emulator, error_type, expect_error, ok, parse_time, string_member, ACCOUNT, REGION,
};

/// The `FirewallDomainList` of a create/get response.
fn domain_list(body: &Value) -> &Value {
    body.get("FirewallDomainList")
        .expect("the response carries a FirewallDomainList")
}

/// One row of the create table: the request, and the `Name` the stored list must
/// report back.
struct CreateCase {
    case: &'static str,
    body: Value,
}

/// A created domain list is persisted: every member of the response is computed
/// from the request plus what the model documents, and `GetFirewallDomainList`
/// answers the very same list.
///
/// The table spans what the rule generalises over: the shape of `Name` (every
/// character class the model's `Name` pattern
/// `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$` admits — letters, digits beside
/// letters, hyphen, underscore, space — and its published maximum length of 64
/// as well as the minimum of one character), and `Tags` absent, empty and
/// carrying several entries.
#[tokio::test]
async fn a_created_domain_list_is_read_back_from_state() {
    // 64 characters, the maximum the model's `Name` (length 0..64) allows.
    let longest_name = "a".repeat(64);
    let cases = vec![
        CreateCase {
            case: "letters only, no Tags",
            body: json!({
                "CreatorRequestId": "req-letters",
                "Name": "blocked-domains",
            }),
        },
        CreateCase {
            case: "digits beside letters, underscores and hyphens",
            body: json!({
                "CreatorRequestId": "req-mixed",
                "Name": "list_2-v3",
                "Tags": [{ "Key": "team", "Value": "net" }],
            }),
        },
        CreateCase {
            case: "spaces in the name, several tags",
            body: json!({
                "CreatorRequestId": "req-spaces",
                "Name": "my blocked list",
                "Tags": [
                    { "Key": "team", "Value": "net" },
                    { "Key": "env", "Value": "prod" },
                    // `TagValue` is length 0..256, so the empty value is legal.
                    { "Key": "note", "Value": "" },
                ],
            }),
        },
        CreateCase {
            case: "one character, an empty tag list",
            body: json!({
                "CreatorRequestId": "req-shortest",
                "Name": "a",
                "Tags": [],
            }),
        },
        CreateCase {
            case: "the longest Name the model allows",
            body: json!({
                "CreatorRequestId": "req-longest",
                "Name": longest_name,
            }),
        },
        CreateCase {
            case: "an apostrophe, which the pattern admits",
            body: json!({
                "CreatorRequestId": "req-apostrophe",
                "Name": "ops' list",
            }),
        },
        CreateCase {
            case: "a CreatorRequestId that reads like a date/time stamp",
            body: json!({
                "CreatorRequestId": "2026-10-24T06:21:31Z",
                "Name": "stamped",
            }),
        },
    ];

    let router = emulator();
    let mut seen_ids: Vec<String> = Vec::new();

    for CreateCase { case, body } in cases {
        let created = ok(&router, "CreateFirewallDomainList", body.clone()).await;
        let made = domain_list(&created).clone();
        let id = string_member(&made, "Id");

        assert!(
            id.starts_with("rslvr-fdl-"),
            "case {case:?}: a firewall domain list id is minted as rslvr-fdl-<hex>, got {id:?}"
        );
        assert!(
            !id.is_empty() && id.len() <= 64,
            "case {case:?}: Id must satisfy ResourceId's length 1..64, got {id:?}"
        );
        assert_eq!(
            string_member(&made, "Arn"),
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:firewall-domain-list/{id}"),
            "case {case:?}: the Arn must be rooted in the signed region and this id, got {made}"
        );
        for member in ["Name", "CreatorRequestId"] {
            assert_eq!(
                made.get(member),
                body.get(member),
                "case {case:?}: {member} must come back exactly as requested, got {made}"
            );
        }
        assert_eq!(
            made.get("DomainCount"),
            Some(&json!(0)),
            "case {case:?}: a list is created empty, so DomainCount is 0, got {made}"
        );
        assert_eq!(
            string_member(&made, "Status"),
            "COMPLETE",
            "case {case:?}: an empty list needs no import, so it is settled at once, got {made}"
        );
        // "The owner of the list, used only for lists that are not managed by
        // you", and the managed-list members beside it, say nothing about a list
        // this account just created; `StatusMessage` carries "additional
        // information about the status of the list, IF AVAILABLE", and a settled
        // empty list has none.
        for member in [
            "ManagedOwnerName",
            "ManagedListType",
            "Category",
            "StatusMessage",
        ] {
            assert_eq!(
                made.get(member),
                None,
                "case {case:?}: {member} must be unset for a list this account just created, got {made}"
            );
        }
        let created_at = parse_time(case, "CreationTime", &string_member(&made, "CreationTime"));
        let modified_at = parse_time(
            case,
            "ModificationTime",
            &string_member(&made, "ModificationTime"),
        );
        assert_eq!(
            created_at, modified_at,
            "case {case:?}: a list nobody has modified carries one timestamp, got {made}"
        );

        assert!(
            !seen_ids.contains(&id),
            "case {case:?}: each create mints a distinct Id, {id:?} is already among {seen_ids:?}"
        );
        seen_ids.push(id.clone());

        // Round trip: the reader answers the record the create wrote, and the
        // status of an empty list is terminal, so the two responses are equal.
        let fetched = ok(
            &router,
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": id }),
        )
        .await;
        assert_eq!(
            domain_list(&fetched),
            &made,
            "case {case:?}: GetFirewallDomainList must answer the stored list, got {fetched}"
        );
    }

    // Every list created above is still readable after the later creates, so a
    // create never disturbs the records beside it.
    for id in &seen_ids {
        let fetched = ok(
            &router,
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": id }),
        )
        .await;
        assert_eq!(
            string_member(domain_list(&fetched), "Id"),
            *id,
            "case \"all lists survive\": {id} must still be readable, got {fetched}"
        );
    }
}

/// A retried create with the same `CreatorRequestId` resolves to the list that
/// request already made: `CreatorRequestId` "allows you to retry failed requests
/// without the risk of running the operation twice."
#[tokio::test]
async fn a_repeated_creator_request_id_does_not_create_a_second_list() {
    let router = emulator();

    for (case, body) in [
        (
            "retried with the same Name",
            json!({ "CreatorRequestId": "req-retried", "Name": "once-only" }),
        ),
        (
            "retried with tags",
            json!({
                "CreatorRequestId": "req-retried-tagged",
                "Name": "once-only-tagged",
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
    ] {
        let first = ok(&router, "CreateFirewallDomainList", body.clone()).await;
        let second = ok(&router, "CreateFirewallDomainList", body).await;
        assert_eq!(
            domain_list(&first),
            domain_list(&second),
            "case {case:?}: the retry must resolve to the list the first call made, got {second}"
        );
    }
}

/// `GetFirewallDomainList` reports a `FirewallDomainListId` the store holds no
/// list under with the `ResourceNotFoundException` it models — never an empty
/// success, and never a list it invented.
#[tokio::test]
async fn an_unknown_domain_list_id_is_reported_as_missing() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": "req-present", "Name": "present" }),
    )
    .await;
    let present = string_member(domain_list(&created), "Id");

    for (case, id) in [
        ("an id nothing minted", "rslvr-fdl-0123456789abcdef".to_string()),
        ("a Resolver rule id", "rslvr-rr-0123456789abcdef".to_string()),
        ("one character", "x".to_string()),
        // The stored id with its last character dropped: a near miss is still a
        // miss.
        ("a near miss of a stored id", present[..present.len() - 1].to_string()),
    ] {
        expect_error(
            &router,
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": id }),
            case,
            "ResourceNotFoundException",
        )
        .await;
    }

    // The refusals above are about the id, not about refusing every read.
    let fetched = ok(
        &router,
        "GetFirewallDomainList",
        json!({ "FirewallDomainListId": present }),
    )
    .await;
    assert_eq!(
        string_member(domain_list(&fetched), "Id"),
        present,
        "case \"the stored id\": the read must answer the stored list, got {fetched}"
    );
}

/// A `Name` outside the model's constraints is refused at the boundary with HTTP
/// 400, and nothing is stored for it.
///
/// The rows walk the published bounds of `Name` (length 0..64, pattern
/// `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$`): one character past the maximum, the
/// empty string, and the all-digits form the pattern's negative lookahead
/// excludes.
#[tokio::test]
async fn a_name_outside_the_modelled_constraints_is_refused() {
    let router = emulator();

    for (case, name) in [
        ("one character past the 64-character maximum", "a".repeat(65)),
        ("far past the maximum", "b".repeat(200)),
        ("empty, which the pattern's + excludes", String::new()),
        ("only digits", "12345".to_string()),
        ("one digit", "7".to_string()),
        ("a character outside the pattern", "bad/name".to_string()),
        ("a dot, which the pattern excludes", "list.one".to_string()),
        ("a non-ASCII letter", "liste-é".to_string()),
    ] {
        let (status, body) = call(
            &router,
            "CreateFirewallDomainList",
            json!({ "CreatorRequestId": format!("req-{case}"), "Name": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a Name outside the modelled constraints must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {body}"
        );
    }

    // The longest legal name is accepted, so the refusals above are about the
    // constraint and not about refusing long names outright.
    let created = ok(
        &router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": "req-at-the-bound", "Name": "c".repeat(64) }),
    )
    .await;
    assert_eq!(
        string_member(domain_list(&created), "Name").len(),
        64,
        "case \"at the bound\": a 64-character Name is legal, got {created}"
    );
}

/// Many creates in one store each mint their own list and each read back: the
/// emulator's own ceiling on how many lists it holds is far above the handful a
/// caller makes, so an ordinary workload is served throughout.
#[tokio::test]
async fn many_creates_in_one_store_are_each_served_and_distinct() {
    let router = emulator();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..25 {
        let created = ok(
            &router,
            "CreateFirewallDomainList",
            json!({
                "CreatorRequestId": format!("req-bulk-{index}"),
                "Name": format!("bulk-{index}"),
            }),
        )
        .await;
        assert_eq!(
            string_member(domain_list(&created), "Status"),
            "COMPLETE",
            "case \"bulk create {index}\": every create below the cap is served, got {created}"
        );
        let id = string_member(domain_list(&created), "Id");
        assert!(
            !ids.contains(&id),
            "case \"bulk create {index}\": each create mints a distinct Id, got {id:?} among {ids:?}"
        );
        ids.push(id);
    }

    for (index, id) in ids.iter().enumerate() {
        let fetched = ok(
            &router,
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": id }),
        )
        .await;
        assert_eq!(
            string_member(domain_list(&fetched), "Name"),
            format!("bulk-{index}"),
            "case \"bulk read {index}\": the read must answer the list that create wrote, got {fetched}"
        );
    }
}
