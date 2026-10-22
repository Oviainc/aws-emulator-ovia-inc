// SPDX-License-Identifier: CC0-1.0

//! `ListSecurityPolicies`: paging the published security policy catalog.
//!
//! RULE: the security policies are service-managed, FIXED reference data — no Transfer
//! Family operation creates, updates or deletes one — so a listing answers the SAME
//! names, in the same order, however many servers a caller has created or moved onto
//! another policy, and every name it returns describes. The listing is paged by the
//! service's shared `List*` codec: at most `MaxResults` names per page, a `NextToken`
//! only while more remain, that token resuming the walk exactly once, and a token this
//! codec cannot have issued refused with the modelled `InvalidNextTokenException`.
//!
//! The rows differ along what the rule generalises over: the caller writes that mention
//! a policy (create with no policy, create with a named one, update onto another), the
//! page bound (both published bounds of `MaxResults`' modelled `range 1..1000`, and
//! sizes below, at and above the catalog size), and the shape of a `NextToken` (non-hex,
//! odd-length hex, non-UTF-8 bytes, and a well-formed one past the last policy).
//!
//! `DescribeSecurityPolicy`'s own properties are covered by
//! [`crate::security_policy_catalog`]; what this suite adds is the listing and its
//! paging. Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`].

use serde_json::json;
use service_com_amazonaws_transfer_transferservice::state::{
    security_policy_names, DEFAULT_SECURITY_POLICY_NAME,
};

use crate::harness::Emulator;

/// Drain every page of `ListSecurityPolicies`, returning the per-page name counts and
/// the concatenated names.
///
/// The crate's shared walk (`harness::drain_pages`) collects a MEMBER of each item, so
/// it reads a listing of structures; `SecurityPolicyNames` is a list of plain strings,
/// where the item IS the key, so this suite walks the pages itself rather than widening
/// the shared helper while the crate's test tree is under concurrent change.
async fn drain_policy_names(
    emulator: &Emulator,
    max_results: Option<i64>,
) -> (Vec<usize>, Vec<String>) {
    let mut page_sizes = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut next: Option<String> = None;

    loop {
        let mut request = json!({});
        if let Some(limit) = max_results {
            request["MaxResults"] = json!(limit);
        }
        if let Some(token) = &next {
            request["NextToken"] = json!(token);
        }
        let answer = emulator.call("ListSecurityPolicies", request).await;
        assert_eq!(
            answer.status, 200,
            "case \"MaxResults={max_results:?}\": expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let page = answer
            .get("SecurityPolicyNames")
            .as_array()
            .cloned()
            .unwrap_or_default();
        page_sizes.push(page.len());
        for name in &page {
            names.push(
                name.as_str()
                    .unwrap_or_else(|| {
                        panic!(
                            "case \"MaxResults={max_results:?}\": a listed name must be a \
                             string, got {name:?}"
                        )
                    })
                    .to_string(),
            );
        }
        match answer.get("NextToken").as_str() {
            Some(token) => next = Some(token.to_string()),
            None => break,
        }
        assert!(
            page_sizes.len() < 128,
            "case \"MaxResults={max_results:?}\": paging did not terminate"
        );
    }

    (page_sizes, names)
}

/// Every published name, in listing order, with no page bound.
async fn all_policy_names(emulator: &Emulator) -> Vec<String> {
    drain_policy_names(emulator, None).await.1
}

/// `SecurityPolicyName` is modelled `len 0..100`, pattern
/// `^Transfer[A-Za-z0-9]*SecurityPolicy-[A-Za-z0-9-]+$`.
fn is_modelled_policy_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("Transfer") else {
        return false;
    };
    let Some((infix, suffix)) = rest.split_once("SecurityPolicy-") else {
        return false;
    };
    name.len() <= 100
        && infix.chars().all(|c| c.is_ascii_alphanumeric())
        && !suffix.is_empty()
        && suffix.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// RULE: the listing answers the catalog — every published name, once, in the order the
/// catalog publishes them, each one satisfying `SecurityPolicyName`'s modelled shape,
/// and `SecurityPolicyNames` is reported because the model makes it required.
#[tokio::test]
async fn list_security_policies_answers_the_published_catalog() {
    let emulator = Emulator::new();

    // `SecurityPolicyNames` is modelled REQUIRED, so an unpaged listing carries it as
    // a list before anything else is asserted about its contents.
    let answer = emulator.call("ListSecurityPolicies", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case \"no paging members\": expected 200, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.get("SecurityPolicyNames").is_array(),
        "case \"no paging members\": SecurityPolicyNames is required, got {}",
        answer.body
    );
    assert!(
        answer.get("NextToken").is_null(),
        "case \"no paging members\": a catalog that fits one page must hand back no \
         token, got {}",
        answer.body
    );

    let listed = all_policy_names(&emulator).await;
    let published: Vec<String> = security_policy_names()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        listed, published,
        "expected the listing to answer the published catalog in its order, got {listed:?}"
    );

    // Strictly ascending and duplicate-free: that order is what a `NextToken` resumes
    // from, so a listing that repeats or reorders a name cannot be paged.
    let mut sorted = listed.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted, listed,
        "expected the listing in strictly ascending name order, got {listed:?}"
    );
    for name in &listed {
        assert!(
            is_modelled_policy_name(name),
            "case {name:?}: expected a name matching SecurityPolicyName's modelled \
             pattern ^Transfer[A-Za-z0-9]*SecurityPolicy-[A-Za-z0-9-]+$"
        );
    }

    // The default a server with no `SecurityPolicyName` gets must be listed, or a
    // caller could not find the policy its own server reports.
    assert!(
        listed.iter().any(|n| n == DEFAULT_SECURITY_POLICY_NAME),
        "expected the listing to carry the create-time default \
         {DEFAULT_SECURITY_POLICY_NAME:?}, got {listed:?}"
    );
}

/// RULE: the catalog is reference data no caller can change, so no write a caller can
/// make that mentions a policy changes what the listing answers.
///
/// The rows are those writes: a server created with no `SecurityPolicyName` (it takes
/// the default), servers created with published policies, and an `UpdateServer` that
/// moves a server onto another one. The listing is re-read after each row, so each
/// assertion observes the state its own row left.
#[tokio::test]
async fn the_listed_catalog_is_unchanged_by_caller_writes() {
    let emulator = Emulator::new();
    let before = all_policy_names(&emulator).await;
    assert!(
        !before.is_empty(),
        "expected the catalog to publish at least one policy, got {before:?}"
    );

    // Row 1: a server with no policy, plus servers with several published ones.
    let default_server = emulator.create_server(json!({})).await;
    for name in before
        .iter()
        .filter(|n| n.starts_with("TransferSecurityPolicy-"))
        .take(3)
    {
        emulator
            .create_server(json!({"SecurityPolicyName": name}))
            .await;
    }
    let after_creates = all_policy_names(&emulator).await;
    assert_eq!(
        after_creates, before,
        "case \"after creates\": the catalog must be unchanged by caller writes, got \
         {after_creates:?}"
    );

    // Row 2: an `UpdateServer` that moves a server onto another published policy.
    let moved_to = before
        .iter()
        .rev()
        .find(|n| n.starts_with("TransferSecurityPolicy-"))
        .expect("the catalog publishes a server policy");
    let update = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &default_server, "SecurityPolicyName": moved_to}),
        )
        .await;
    assert_eq!(
        update.status, 200,
        "case \"after an update\": UpdateServer expected 200, got {} with body {}",
        update.status, update.body
    );
    let after_update = all_policy_names(&emulator).await;
    assert_eq!(
        after_update, before,
        "case \"after an update\": the catalog must be unchanged by caller writes, got \
         {after_update:?}"
    );

    // Row 3: deleting a server releases nothing from the catalog either.
    let delete = emulator
        .call("DeleteServer", json!({"ServerId": &default_server}))
        .await;
    assert_eq!(
        delete.status, 200,
        "case \"after a delete\": DeleteServer expected 200, got {} with body {}",
        delete.status, delete.body
    );
    let after_delete = all_policy_names(&emulator).await;
    assert_eq!(
        after_delete, before,
        "case \"after a delete\": the catalog must be unchanged by caller writes, got \
         {after_delete:?}"
    );
}

/// RULE: every name the listing returns describes — the listing and
/// `DescribeSecurityPolicy` read ONE catalog, so neither can name a policy the other
/// does not have.
///
/// The rows are the whole listing, read off the wire, so a policy added to the catalog
/// is covered without touching this test.
#[tokio::test]
async fn every_listed_name_describes() {
    let emulator = Emulator::new();

    for name in all_policy_names(&emulator).await {
        let answer = emulator
            .call(
                "DescribeSecurityPolicy",
                json!({"SecurityPolicyName": &name}),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: a listed name must describe, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("SecurityPolicy").get("SecurityPolicyName"),
            Some(&json!(name)),
            "case {name:?}: expected the described policy to carry the name listed, \
             got {}",
            answer.body
        );
    }
}

/// RULE: a listing returns at most `MaxResults` names, hands back a `NextToken` only
/// while more remain, and resuming from that token walks the whole catalog exactly once
/// in the same order.
///
/// The rows are the page bounds: both published bounds of `MaxResults`' modelled
/// `range 1..1000`, and sizes below, at and above the catalog size, plus no bound at all.
#[tokio::test]
async fn list_security_policies_honours_every_page_bound_and_resumes_from_its_token() {
    let emulator = Emulator::new();
    let all = all_policy_names(&emulator).await;
    let total = all.len();
    let size = i64::try_from(total).expect("the catalog is small");

    for limit in [
        Some(1),
        Some(2),
        Some(3),
        // One below the catalog size, but never below `MaxResults`' modelled minimum.
        Some((size - 1).max(1)),
        Some(size),
        Some(size + 1),
        Some(1000),
        None,
    ] {
        let (page_sizes, names) = drain_policy_names(&emulator, limit).await;
        let case = format!("MaxResults={limit:?}");

        assert_eq!(
            names, all,
            "case {case:?}: expected every policy once in name order, got {names:?}"
        );
        match limit {
            Some(limit) => {
                let limit = usize::try_from(limit).expect("positive");
                for page in &page_sizes {
                    assert!(
                        *page <= limit,
                        "case {case:?}: a page carried {page} names, above the bound {limit}"
                    );
                }
                let want_pages = total.div_ceil(limit);
                assert_eq!(
                    page_sizes.len(),
                    want_pages,
                    "case {case:?}: expected {want_pages} page(s), got {page_sizes:?}"
                );
            }
            None => assert_eq!(
                page_sizes,
                vec![total],
                "case {case:?}: expected a single unbounded page, got {page_sizes:?}"
            ),
        }
    }
}

/// RULE: a `NextToken` the paging codec cannot have issued is refused with
/// `InvalidNextTokenException` (400) — the listing is never silently restarted — a
/// `MaxResults` outside the modelled `range 1..1000` is refused at the boundary while
/// both published bounds are accepted, and a well-formed token naming a position past
/// the last policy resumes after it rather than starting over.
#[tokio::test]
async fn list_security_policies_refuses_an_unissued_token_and_a_bad_page_bound() {
    let emulator = Emulator::new();

    // Rows: not hex at all, odd-length hex, and valid hex whose bytes are not UTF-8.
    for token in ["not-a-token", "zz", "abc", "c0ffee00", "ff"] {
        let answer = emulator
            .call("ListSecurityPolicies", json!({"NextToken": token}))
            .await;
        let case = format!("NextToken={token:?}");
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException, got {}",
            answer.body
        );
    }

    // One PAST each published bound of `MaxResults`' modelled range 1..1000.
    for bad in [0, 1001] {
        let answer = emulator
            .call("ListSecurityPolicies", json!({"MaxResults": bad}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case \"MaxResults={bad}\": expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
    }

    // Both published bounds themselves are accepted.
    for good in [1, 1000] {
        let answer = emulator
            .call("ListSecurityPolicies", json!({"MaxResults": good}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case \"MaxResults={good}\": expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
    }

    // A token the codec could have issued (valid hex, valid UTF-8) that names a
    // position past every published name is a POSITION, not a lookup: the listing
    // resumes after it, which is an empty last page and no further token.
    let past_the_end = "7a7a7a"; // hex of "zzz", which sorts after every policy name
    let answer = emulator
        .call("ListSecurityPolicies", json!({"NextToken": past_the_end}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"NextToken past the end\": expected HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.get("SecurityPolicyNames").as_array(),
        Some(&vec![]),
        "case \"NextToken past the end\": expected an empty page, got {}",
        answer.body
    );
    assert!(
        answer.get("NextToken").is_null(),
        "case \"NextToken past the end\": expected no further token, got {}",
        answer.body
    );
}
