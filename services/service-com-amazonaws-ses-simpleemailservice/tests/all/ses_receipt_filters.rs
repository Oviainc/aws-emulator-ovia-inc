// SPDX-License-Identifier: CC0-1.0

//! The receipt IP address filter plane, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE each operation follows over
//! a SET of inputs, not one recorded request:
//!
//! * a filter is recorded only under a name of the documented form, only when
//!   no filter already holds that name, only when its `Cidr` names an IP
//!   address or a range of them, and only while the account is under its filter
//!   limit — each refusal carrying the error for its own cause and storing
//!   nothing;
//! * `ListReceiptFilters` reports every stored filter exactly as it was stored;
//! * `DeleteReceiptFilter` leaves no filter under the named name, whether or
//!   not one was there.

use crate::harness::{call, element, elements, emulator, field, Field};
use service_com_amazonaws_ses_simpleemailservice::state::{
    MAX_RECEIPT_FILTERS, MAX_RECEIPT_FILTER_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

/// `CreateReceiptFilter`.
async fn create_filter(
    router: &MultiServiceRouter,
    name: &str,
    policy: &str,
    cidr: &str,
) -> (u16, String) {
    let params: Vec<Field> = vec![
        field("Filter.Name", name),
        field("Filter.IpFilter.Policy", policy),
        field("Filter.IpFilter.Cidr", cidr),
    ];
    call(router, "CreateReceiptFilter", &params).await
}

/// `ListReceiptFilters`.
async fn list_filters(router: &MultiServiceRouter) -> (u16, String) {
    call(router, "ListReceiptFilters", &[]).await
}

/// `DeleteReceiptFilter`.
async fn delete_filter(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(router, "DeleteReceiptFilter", &[field("FilterName", name)]).await
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope.
fn error(body: &str) -> (Option<String>, Option<String>) {
    (element(body, "Code"), element(body, "Message"))
}

/// Every filter `ListReceiptFilters` reports, as `(name, policy, cidr)` in the
/// order the response lists them.
async fn listed(router: &MultiServiceRouter) -> Vec<(String, String, String)> {
    let (status, body) = list_filters(router).await;
    assert_eq!(
        status, 200,
        "ListReceiptFilters: expected 200, got {status} with {body}"
    );
    let filters = element(&body, "Filters").unwrap_or_default();
    elements(&filters, "member")
        .into_iter()
        .map(|member| {
            let ip_filter = element(&member, "IpFilter").unwrap_or_default();
            (
                element(&member, "Name").unwrap_or_default(),
                element(&ip_filter, "Policy").unwrap_or_default(),
                element(&ip_filter, "Cidr").unwrap_or_default(),
            )
        })
        .collect()
}

/// A created filter is reported back by `ListReceiptFilters` with the name,
/// policy and CIDR it was stored with — over every name form the model
/// documents, both policies, and a single address as well as a range.
#[tokio::test]
async fn a_created_filter_round_trips_through_the_listing() {
    let router = emulator();
    // The rows span what the rule generalises over: the documented name form
    // (`ReceiptFilter$Name`: ASCII letters, numbers, underscores or dashes,
    // starting and ending with a letter or number, 64 characters or fewer),
    // both values of `ReceiptFilterPolicy` (`Allow`, `Block`), and the two
    // forms `ReceiptIpFilter$Cidr` documents — a single IP address and a range
    // in CIDR notation — including each published prefix bound (/0 and /32).
    let max_name = "n".repeat(MAX_RECEIPT_FILTER_NAME);
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a", "Allow", "10.0.0.1"),
        ("9", "Block", "192.168.0.0/24"),
        ("single-address", "Allow", "203.0.113.7"),
        ("slash-thirty-two", "Block", "203.0.113.7/32"),
        ("slash-zero", "Allow", "0.0.0.0/0"),
        ("slash-eight", "Block", "10.0.0.0/8"),
        ("Mixed_Case-2024", "Allow", "198.51.100.0/25"),
        ("host-bits-kept", "Block", "10.0.0.1/24"),
        (max_name.as_str(), "Allow", "172.16.0.0/12"),
    ];
    for (name, policy, cidr) in &cases {
        let (status, body) = create_filter(&router, name, policy, cidr).await;
        assert_eq!(
            status, 200,
            "case ({name:?}, {policy:?}, {cidr:?}): expected 200 from CreateReceiptFilter, \
             got {status} with {body}"
        );
    }

    let reported = listed(&router).await;
    assert_eq!(
        reported.len(),
        cases.len(),
        "expected every created filter in the listing, got {reported:?}"
    );
    for (name, policy, cidr) in &cases {
        let found = reported.iter().find(|(stored, _, _)| stored == name);
        let Some((_, stored_policy, stored_cidr)) = found else {
            panic!("case {name:?}: expected the filter in the listing, got {reported:?}");
        };
        assert_eq!(
            stored_policy, policy,
            "case {name:?}: expected the stored policy {policy:?}, got {stored_policy:?}"
        );
        assert_eq!(
            stored_cidr, cidr,
            "case {name:?}: expected the stored CIDR {cidr:?}, got {stored_cidr:?}"
        );
    }
    // The store is keyed by name, so the listing is in one stable order.
    let mut sorted = reported.clone();
    sorted.sort();
    assert_eq!(
        reported, sorted,
        "the listing is served in one stable (lexicographic) order, got {reported:?}"
    );
}

/// A `Cidr` that names neither an IP address nor a range of them is refused,
/// and the filter is not stored.
#[tokio::test]
async fn a_cidr_outside_the_documented_form_is_refused() {
    let router = emulator();
    // One row per way a value fails to name an address or a range: not an
    // address at all, an octet past 255, too few octets, a prefix one past the
    // published maximum (/32), a negative and a non-numeric prefix, a doubled
    // prefix, and an address of another family (receipt filtering is over the
    // IPv4 addresses the member documents).
    let rejected = [
        "not-a-cidr",
        "10.0.0.256",
        "10.0.0",
        "10.0.0.1/33",
        "10.0.0.1/-1",
        "10.0.0.1/x",
        "10.0.0.1/24/8",
        "10.0.0.1 ",
        "2001:db8::1",
        "2001:db8::/32",
    ];
    for (index, cidr) in rejected.iter().enumerate() {
        let name = format!("rejected-cidr-{index}");
        let (status, body) = create_filter(&router, &name, "Block", cidr).await;
        assert_eq!(
            status, 400,
            "case {cidr:?}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("InvalidParameterValue"),
            "case {cidr:?}: expected InvalidParameterValue, got {body}"
        );
        // The refusal is the fixed wording plus the request's own `Cidr`: the
        // value the caller has to fix.
        assert_eq!(
            message.as_deref(),
            Some(format!("Invalid CIDR block: {cidr}").as_str()),
            "case {cidr:?}: expected the refusal to name the bad CIDR, got {message:?}"
        );

        let reported = listed(&router).await;
        assert!(
            reported.is_empty(),
            "case {cidr:?}: a refused filter must store nothing, got {reported:?}"
        );
    }
}

/// A name outside the documented form is refused, and nothing is stored under
/// it.
#[tokio::test]
async fn a_name_outside_the_documented_form_is_refused() {
    let router = emulator();
    // One row per clause of the documented rule: a character outside
    // letters/numbers/underscore/dash (space, dot, plus), a name that does not
    // START with a letter or number, one that does not END with one, a
    // non-ASCII letter, and one character past the published maximum.
    let rejected: Vec<String> = vec![
        "has space".to_string(),
        "dotted.name".to_string(),
        "plus+name".to_string(),
        "_leading".to_string(),
        "-leading".to_string(),
        "trailing_".to_string(),
        "trailing-".to_string(),
        "ünïcode".to_string(),
        "y".repeat(MAX_RECEIPT_FILTER_NAME + 1),
    ];
    for name in &rejected {
        let (status, body) = create_filter(&router, name, "Allow", "10.0.0.1/24").await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error(&body).0.as_deref(),
            Some("InvalidParameterValue"),
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );

        let reported = listed(&router).await;
        assert!(
            reported.is_empty(),
            "case {name:?}: a refused name must store nothing, got {reported:?}"
        );
    }
}

/// A name a filter already holds is a naming conflict, and the stored filter is
/// left exactly as it was — whatever the second request asked for.
#[tokio::test]
async fn a_conflicting_name_is_refused_and_leaves_the_stored_filter_alone() {
    let router = emulator();
    // The rows differ in how the second request differs from the first: the
    // same filter again, the other policy, another CIDR, and both at once.
    let conflicts = [
        ("Allow", "10.0.0.1/24"),
        ("Block", "10.0.0.1/24"),
        ("Allow", "192.0.2.0/24"),
        ("Block", "198.51.100.1"),
    ];
    for (index, (policy, cidr)) in conflicts.iter().enumerate() {
        let name = format!("conflict-{index}");
        let (status, body) = create_filter(&router, &name, "Allow", "10.0.0.1/24").await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 recording the first filter, got {status} with {body}"
        );

        let (status, body) = create_filter(&router, &name, policy, cidr).await;
        assert_eq!(
            status, 400,
            "case {name:?} again as ({policy:?}, {cidr:?}): expected HTTP 400, got {status} \
             with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("AlreadyExists"),
            "case {name:?} again: expected the modelled AlreadyExists, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Filter already exists: {name}").as_str()),
            "case {name:?} again: expected the conflicting name in the message, got {body}"
        );

        let reported = listed(&router).await;
        let found = reported.iter().find(|(stored, _, _)| *stored == name);
        assert_eq!(
            found,
            Some(&(name.clone(), "Allow".to_string(), "10.0.0.1/24".to_string())),
            "case {name:?}: the refused create must leave the first filter alone, \
             got {reported:?}"
        );
    }
}

/// Deleting a filter leaves no filter under that name and touches no other —
/// and deleting a name the account does not hold is reported as done too.
#[tokio::test]
async fn deleting_a_filter_removes_only_that_filter() {
    let router = emulator();
    let stored = [
        ("first", "Allow", "10.0.0.1"),
        ("second", "Block", "10.0.1.0/24"),
        ("third", "Allow", "192.0.2.0/25"),
    ];
    for (name, policy, cidr) in &stored {
        let (status, body) = create_filter(&router, name, policy, cidr).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from CreateReceiptFilter, got {status} with {body}"
        );
    }

    // Absent names first, including one no account could hold: a name the
    // account does not hold is nothing to remove, and the list is untouched.
    for absent in ["fourth", "first-but-longer", "not a legal name!"] {
        let (status, body) = delete_filter(&router, absent).await;
        assert_eq!(
            status, 200,
            "case absent {absent:?}: expected 200 from DeleteReceiptFilter, got {status} \
             with {body}"
        );
        assert_eq!(
            listed(&router).await.len(),
            stored.len(),
            "case absent {absent:?}: deleting nothing must leave every filter in place"
        );
    }

    for (index, (name, _, _)) in stored.iter().enumerate() {
        let (status, body) = delete_filter(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from DeleteReceiptFilter, got {status} with {body}"
        );
        let reported = listed(&router).await;
        assert!(
            !reported.iter().any(|(listed, _, _)| listed == name),
            "case {name:?}: the deleted filter must be gone, got {reported:?}"
        );
        assert_eq!(
            reported.len(),
            stored.len() - index - 1,
            "case {name:?}: only that filter is removed, got {reported:?}"
        );
        // Deleting it a second time is reported as done, since the end state
        // the caller asked for already holds.
        let (status, body) = delete_filter(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?} twice: expected 200 from DeleteReceiptFilter, got {status} \
             with {body}"
        );
    }
    assert!(
        listed(&router).await.is_empty(),
        "every filter was deleted, so the listing is empty"
    );

    // And the name is free again: the create that conflicted before succeeds.
    let (status, body) = create_filter(&router, "first", "Block", "10.0.0.2").await;
    assert_eq!(
        status, 200,
        "re-creating a deleted filter: expected 200, got {status} with {body}"
    );
    assert_eq!(
        listed(&router).await,
        vec![(
            "first".to_string(),
            "Block".to_string(),
            "10.0.0.2".to_string()
        )],
        "the re-created filter is the one that was just written"
    );
}

/// The account's filter limit bounds the store: the published number of filters
/// is accepted, and the one past it is refused as `LimitExceeded`.
#[tokio::test]
async fn the_filter_limit_bounds_the_store() {
    let router = emulator();
    // SES documents 100 receipt filters per account, which the crate publishes
    // as `MAX_RECEIPT_FILTERS`; every one of them is accepted.
    for index in 0..MAX_RECEIPT_FILTERS {
        let name = format!("limit-filter-{index:03}");
        let cidr = format!("10.{}.{}.0/24", index / 256, index % 256);
        let policy = if index % 2 == 0 { "Allow" } else { "Block" };
        let (status, body) = create_filter(&router, &name, policy, &cidr).await;
        assert_eq!(
            status, 200,
            "case filter {index} of {MAX_RECEIPT_FILTERS}: expected 200, got {status} \
             with {body}"
        );
    }

    let (status, body) = create_filter(&router, "one-past-the-bound", "Allow", "10.9.9.0/24").await;
    assert_eq!(
        status, 400,
        "case one past the bound: expected HTTP 400, got {status} with {body}"
    );
    let (code, message) = error(&body);
    assert_eq!(
        code.as_deref(),
        Some("LimitExceeded"),
        "case one past the bound: expected the modelled LimitExceeded, got {body}"
    );
    assert_eq!(
        message.as_deref(),
        Some(
            format!(
                "Filter limit exceeded: this account may hold {MAX_RECEIPT_FILTERS} receipt \
                 filters."
            )
            .as_str()
        ),
        "case one past the bound: expected the limit in the message, got {body}"
    );

    // At the bound a conflicting name is still a conflict, not a limit: the
    // store would not grow.
    let (status, body) = create_filter(&router, "limit-filter-000", "Block", "10.0.0.1").await;
    assert_eq!(
        status, 400,
        "case conflict at the bound: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error(&body).0.as_deref(),
        Some("AlreadyExists"),
        "case conflict at the bound: expected AlreadyExists, got {body}"
    );

    // Every filter stored up to the bound is still reported, and one delete
    // makes room for exactly one more.
    assert_eq!(
        listed(&router).await.len(),
        MAX_RECEIPT_FILTERS,
        "the store holds exactly the filters that were accepted"
    );
    let (status, body) = delete_filter(&router, "limit-filter-000").await;
    assert_eq!(
        status, 200,
        "freeing one slot: expected 200 from DeleteReceiptFilter, got {status} with {body}"
    );
    let (status, body) = create_filter(&router, "one-past-the-bound", "Allow", "10.9.9.0/24").await;
    assert_eq!(
        status, 200,
        "case one filter below the bound again: expected 200, got {status} with {body}"
    );
}
