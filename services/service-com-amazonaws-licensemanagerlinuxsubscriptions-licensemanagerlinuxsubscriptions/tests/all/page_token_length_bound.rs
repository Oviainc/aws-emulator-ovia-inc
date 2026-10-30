// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule about every paged list of this service, driven over the
//! real wire path:
//!
//! * the cursor a page mints is a fixed-size value: its length does not grow with the
//!   request that selected the list, so a cursor the service hands out always satisfies the
//!   modelled `NextToken` bound and the continuation request carrying it is served rather
//!   than refused.
//!
//! The selections below span what the rule generalises over — how many filters a request
//! carries, how many values each carries and how long each value is, up to the largest
//! selection the model allows (`FilterList` has no `@length`, over `StringList` of up to
//! 100 entries of up to 100 characters), and both `Filters`-driven lists plus the
//! `SubscriptionProviderSources`-driven one, which share the one cursor helper.
//!
//! The cursor stays bound to the list that minted it: a token from one selection is still
//! refused for another, which the fixed-size list identity must not weaken.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{
    call, enable_discovery, harness, list_providers, seed_discovered_instance, seed_provider,
    Harness, Reply, RED_HAT,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::{
    DiscoveredInstanceRecord, MAX_NEXT_TOKEN,
};

/// `POST /subscription/ListLinuxSubscriptions` (`OPERATIONS.md`).
const SUBSCRIPTIONS: &str = "/subscription/ListLinuxSubscriptions";
/// `POST /subscription/ListLinuxSubscriptionInstances` (`OPERATIONS.md`).
const INSTANCES: &str = "/subscription/ListLinuxSubscriptionInstances";

/// The largest `Filter$Values` the model allows: `StringList` is `length 1..100` over a
/// `String` of `length 1..100` (smithy_query(query="shape",shape="StringList")).
const MAX_VALUES: usize = 100;
const MAX_VALUE_LENGTH: usize = 100;

/// The discovered instances every walk below pages over: three subscription names licensed
/// three ways, so both discovery lists hold three elements and a page size of one truncates
/// twice. Every record HOLDS each member the filters here address, so a `NotEqual` filter
/// naming values no record carries selects all three.
const DISCOVERED: &[(&str, &str, &str)] = &[
    ("i-0a1", "RHEL", "Subscription-included (Amazon EC2)"),
    (
        "i-0b2",
        "SUSE Linux Enterprise Server",
        "Subscription-included (Amazon EC2)",
    ),
    (
        "i-0c3",
        "Ubuntu Pro",
        "Bring Your Own Subscription model (BYOS)",
    ),
];

/// The one Region the fixture records were found in, and the Region the account's settings
/// discover, so the records are the inventory both lists project.
const REGION: &str = "us-west-2";

/// Put [`DISCOVERED`] in the store through the shared seeding seam (no operation of this
/// service writes a discovered instance), with the settings that make them discovered data.
async fn discover(h: &Harness) {
    enable_discovery(h, &[REGION]).await;
    for (instance_id, subscription_name, subscription_type) in DISCOVERED {
        seed_discovered_instance(
            h,
            DiscoveredInstanceRecord {
                instance_id: (*instance_id).to_string(),
                account_id: "000000000000".to_string(),
                ami_id: "ami-0001".to_string(),
                instance_type: "t3.micro".to_string(),
                status: "running".to_string(),
                region: REGION.to_string(),
                usage_operation: "RunInstances:0010".to_string(),
                product_codes: vec!["pc-linux".to_string()],
                subscription_name: (*subscription_name).to_string(),
                subscription_type: (*subscription_type).to_string(),
                ..DiscoveredInstanceRecord::default()
            },
        );
    }
}

/// `count` distinct values of exactly `length` characters each, every one inside the
/// modelled `StringList` member bounds and none of them a value any fixture record holds.
fn values(count: usize, length: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            let mut value = format!("{index:04}");
            while value.len() < length {
                value.push('f');
            }
            value.truncate(length);
            value
        })
        .collect()
}

/// A `NotEqual` filter on `name` carrying `count` values of `length` characters: it names
/// nothing any fixture record holds, so it selects every record that holds the member and
/// the walk still pages over the whole list.
fn filter(name: &str, count: usize, length: usize) -> Value {
    json!({ "Name": name, "Operator": "NotEqual", "Values": values(count, length) })
}

/// The `NextToken` of a 200 body, if it carries one.
fn next_token(reply: &Reply, case: &str) -> Option<String> {
    reply
        .ok(case)
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The identities a list body reports under `key`: the `InstanceID` of an instance entry,
/// the `Name` of a subscription entry, the ARN of a registration entry.
fn reported(reply: &Reply, key: &str, case: &str) -> Vec<String> {
    let body = reply.ok(case);
    body.get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} in {body:?}"))
        .iter()
        .map(|entry| {
            for member in ["InstanceID", "Name", "SubscriptionProviderArn"] {
                if let Some(value) = entry.get(member).and_then(Value::as_str) {
                    return value.to_string();
                }
            }
            panic!("case {case:?}: no identity member in {entry:?}")
        })
        .collect()
}

/// Walk `path` one element per page with `selection` as the rest of the request body, and
/// answer every identity the walk reported together with every cursor it minted.
///
/// Each continuation goes back over the wire carrying the cursor the previous page handed
/// out, so the request boundary's `@length` check on `NextToken` applies to it exactly as
/// it would to a client's.
async fn walk(
    h: &Harness,
    path: &str,
    key: &str,
    selection: &Value,
    case: &str,
) -> (Vec<String>, Vec<String>) {
    let mut walked: Vec<String> = Vec::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = selection.clone();
        body["MaxResults"] = json!(1);
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let reply = call(h, path, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a continuation carrying the cursor the service minted must be \
             served, got {} with body {:?}",
            reply.status, reply.body
        );
        walked.extend(reported(&reply, key, case));
        token = next_token(&reply, case);
        if let Some(token) = &token {
            tokens.push(token.clone());
        }
        assert!(
            tokens.len() <= DISCOVERED.len() + 2,
            "case {case:?}: the walk did not terminate after {} pages",
            tokens.len() + 1
        );
        if token.is_none() {
            return (walked, tokens);
        }
    }
}

#[tokio::test]
async fn a_minted_cursor_fits_the_modelled_next_token_bound_whatever_the_selection() {
    let h = harness();
    discover(&h).await;

    // (what the row varies, the list, its items member, the selection) — the rows differ
    // along how many filters the request carries, how many values each carries and how long
    // each value is, from the smallest selection to the largest the model allows, over both
    // `Filters`-driven lists.
    let instance_ids: Vec<String> = DISCOVERED
        .iter()
        .map(|(instance_id, _, _)| (*instance_id).to_string())
        .collect();
    let mut subscription_names: Vec<String> = DISCOVERED
        .iter()
        .map(|(_, name, _)| (*name).to_string())
        .collect();
    subscription_names.sort();

    let rows: Vec<(&str, &str, &str, Value, &Vec<String>)> = vec![
        ("no filter", INSTANCES, "Instances", json!({}), &instance_ids),
        (
            "one filter, one short value",
            INSTANCES,
            "Instances",
            json!({ "Filters": [filter("InstanceID", 1, 1)] }),
            &instance_ids,
        ),
        (
            "one filter at the modelled StringList maximum",
            INSTANCES,
            "Instances",
            json!({ "Filters": [filter("InstanceID", MAX_VALUES, MAX_VALUE_LENGTH)] }),
            &instance_ids,
        ),
        (
            "three maximal filters (FilterList carries no bound)",
            INSTANCES,
            "Instances",
            json!({
                "Filters": [
                    filter("InstanceID", MAX_VALUES, MAX_VALUE_LENGTH),
                    filter("AmiID", MAX_VALUES, MAX_VALUE_LENGTH),
                    filter("InstanceType", MAX_VALUES, MAX_VALUE_LENGTH),
                ]
            }),
            &instance_ids,
        ),
        (
            "no filter",
            SUBSCRIPTIONS,
            "Subscriptions",
            json!({}),
            &subscription_names,
        ),
        (
            "one filter at the modelled StringList maximum",
            SUBSCRIPTIONS,
            "Subscriptions",
            json!({ "Filters": [filter("Subscription", MAX_VALUES, MAX_VALUE_LENGTH)] }),
            &subscription_names,
        ),
    ];

    // Per list, the lengths of the cursors a walk mints, for every selection walked: the
    // list identity a cursor binds to is a fixed-size value, so two selections of one list
    // mint cursors of the same length however far apart their sizes are.
    let mut lengths: BTreeMap<&str, Vec<(String, Vec<usize>)>> = BTreeMap::new();
    for (variation, path, key, selection, expected) in rows {
        let case = &format!("{path} with {variation}");
        let (walked, tokens) = walk(&h, path, key, &selection, case).await;

        assert_eq!(
            walked, *expected,
            "case {case:?}: expected the walk to report {expected:?}, got {walked:?}"
        );
        assert_eq!(
            tokens.len(),
            expected.len() - 1,
            "case {case:?}: expected a cursor on every page but the last, got {} cursor(s)",
            tokens.len()
        );
        for token in &tokens {
            assert!(
                (1..=MAX_NEXT_TOKEN).contains(&token.len()),
                "case {case:?}: a minted cursor must be 1..={MAX_NEXT_TOKEN} characters, got \
                 {} characters",
                token.len()
            );
        }
        lengths.entry(path).or_default().push((
            case.clone(),
            tokens.iter().map(String::len).collect(),
        ));
    }

    for (path, walks) in &lengths {
        let (first_case, first_lengths) = walks
            .first()
            .expect("every list above was walked at least once");
        for (case, walk_lengths) in walks {
            assert_eq!(
                walk_lengths, first_lengths,
                "case {case:?}: a cursor's length must not depend on the request, but on \
                 {path} it minted {walk_lengths:?} characters where {first_case:?} minted \
                 {first_lengths:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_cursor_from_a_maximal_selection_is_still_bound_to_its_own_list() {
    let h = harness();
    discover(&h).await;

    let maximal = json!({
        "MaxResults": 1,
        "Filters": [filter("InstanceID", MAX_VALUES, MAX_VALUE_LENGTH)],
    });
    let case = "a maximal selection's cursor";
    let token = next_token(&call(&h, INSTANCES, &maximal).await, case).unwrap_or_else(|| {
        panic!("case {case:?}: expected a NextToken on a truncated page")
    });

    // A different selection of the same list, and a different list: neither honours the
    // cursor, which is what the fixed-size list identity still has to distinguish.
    for (label, path, mut body) in [
        (
            "another selection of the same list",
            INSTANCES,
            json!({
                "MaxResults": 1,
                "Filters": [filter("InstanceID", MAX_VALUES - 1, MAX_VALUE_LENGTH)],
            }),
        ),
        ("another list", SUBSCRIPTIONS, json!({ "MaxResults": 1 })),
    ] {
        body["NextToken"] = json!(token);
        call(&h, path, &body)
            .await
            .error(&format!("{case} on {label}"), 400, "ValidationException");
    }
}

#[tokio::test]
async fn the_provider_list_mints_a_cursor_of_the_same_fixed_size() {
    let h = harness();
    let mut arns: Vec<String> = (0..3)
        .map(|index| {
            seed_provider(
                &h,
                &format!("00000000-0000-4000-8000-{:012x}", 0x2000 + index),
                &format!("arn:aws:secretsmanager:{REGION}:000000000000:secret:rhel-{index}"),
                &[],
            )
        })
        .collect();
    arns.sort();

    // The source selection is an enum list, so it cannot reach the size a `Filters`
    // selection can — but the cursor comes from the same helper, so it is the same fixed
    // size, and the walk it drives reports every registration exactly once.
    for (label, selection) in [
        ("no source selection", json!({})),
        (
            "one source",
            json!({ "SubscriptionProviderSources": [RED_HAT] }),
        ),
    ] {
        let case = &format!("the provider list with {label}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = selection.clone();
            body["MaxResults"] = json!(1);
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let reply = list_providers(&h, &body).await;
            walked.extend(reported(&reply, "RegisteredSubscriptionProviders", case));
            token = next_token(&reply, case);
            pages += 1;
            if let Some(token) = &token {
                assert!(
                    (1..=MAX_NEXT_TOKEN).contains(&token.len()),
                    "case {case:?}: a minted cursor must be 1..={MAX_NEXT_TOKEN} characters, \
                     got {} characters",
                    token.len()
                );
            }
            assert!(
                pages <= arns.len() + 2,
                "case {case:?}: the walk did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, arns,
            "case {case:?}: expected the walk to report {arns:?}, got {walked:?}"
        );
    }
}
