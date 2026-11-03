// SPDX-License-Identifier: CC0-1.0

//! WHICH optional members a `CreateDomain` request carries is part of that request's
//! identity: omitting an optional member and supplying it are two different requests,
//! whatever was supplied — so the second of the two is never served as an `@idempotent`
//! repeat of the first, and creates its own domain.
//!
//! The rule is asserted over a TABLE rather than one recorded pair: every row is a
//! request carrying all three optional members, the rows differing along what a supplied
//! value can be (a map with no entries, one entry, several entries, an entry with an
//! empty value, and the two ends of `name`'s modelled length), and every row is driven
//! once per optional member — the member dropped from the request — in BOTH orderings,
//! so the omitted and the supplied spelling are told apart however they arrive.
//!
//! Model citations
//! (`smithy_query(query="shape",shape="CreateDomainInput",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`):
//! `CreateDomainInput` declares `name` (`@required`, `@length` 1..64) and the three
//! members below, none of them `@required` and none carrying a `@default` — `kmsKeyArn`,
//! `tags` (a `TagMap` whose `TagValue` has `@length` min 0, so an entry with an empty
//! value is a value a caller may supply) and `webAppSetupConfiguration`.
//! `smithy_query(query="operation",operation="CreateDomain")` reports
//! `"idempotent": true` and no idempotency token, so the parameters a request carried are
//! all there is to identify it by.
//!
//! The harness (`Emulator`, `string_at`) and the domain-count and tag helpers are the
//! sibling suites': one way of driving this service, not two.

use serde_json::{json, Value};

use crate::domain_subscription_lifecycle::{string_at, Emulator};
use crate::resource_tagging::domain_tags;
use crate::tag_count_cap::held_domains;

/// Every input member of `CreateDomain` that is OPTIONAL — neither `@required` nor
/// `@default` in `CreateDomainInput` (see the module header's citation). The rule is one
/// rule for all three, so the table below is driven once per entry of this list, and
/// `tags` is one entry of it rather than the case the issue happened to record.
const OPTIONAL_MEMBERS: [&str; 3] = ["kmsKeyArn", "tags", "webAppSetupConfiguration"];

/// A customer-managed key, as `KmsKeyArn`'s modelled pattern spells one
/// (`^arn:aws:kms:[a-z0-9-]+:[0-9]{12}:key/[a-f0-9-]+$`).
fn kms_key(key_id: &str) -> String {
    format!("arn:aws:kms:us-east-1:000000000000:key/{key_id}")
}

/// The `CreateWebAppConfiguration` every row supplies.
fn web_app() -> Value {
    json!({
        "ehrRole": "arn:aws:iam::000000000000:role/ehr-operations",
        "idcInstanceId": "ssoins-72230ed02aa4b4ea",
        "idcRegion": "us-east-1",
    })
}

/// One request carrying all three optional members, with `name` and `tags` as given.
fn request(name: &str, tags: Value) -> Value {
    json!({
        "name": name,
        "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
        "tags": tags,
        "webAppSetupConfiguration": web_app(),
    })
}

/// The requests one of whose optional members is then dropped: the rows differ along what
/// a SUPPLIED value is — including the emptiest one `tags` admits, which is the value an
/// absent map was collapsed onto — and along the two ends of `name`'s modelled length.
fn carrying_every_optional_member() -> Vec<(&'static str, Value)> {
    vec![
        ("an empty tag map", request("clinic", json!({}))),
        ("one tag entry", request("clinic", json!({ "env": "prod" }))),
        (
            "three tag entries",
            request(
                "clinic",
                json!({ "env": "prod", "team": "cardio", "cost": "42" }),
            ),
        ),
        (
            // `TagValue` is `@length` min 0, so an empty value is suppliable and is not
            // the same as the key being absent.
            "a tag entry with an empty value",
            request("clinic", json!({ "env": "" })),
        ),
        (
            "name at the modelled minimum (1)",
            request("c", json!({})),
        ),
        (
            "name at the modelled maximum (64)",
            request(&"c".repeat(64), json!({ "env": "prod" })),
        ),
    ]
}

/// `body` without `member` — the SAME request with one optional member not carried.
fn without(body: &Value, member: &str) -> Value {
    let mut dropped = body.clone();
    let object = dropped
        .as_object_mut()
        .unwrap_or_else(|| panic!("the request must be an object, got {body}"));
    assert!(
        object.remove(member).is_some(),
        "the row must carry {member} for dropping it to be a difference, got {body}"
    );
    dropped
}

#[tokio::test]
async fn omitting_an_optional_member_and_supplying_it_are_different_requests() {
    for (case, supplied) in carrying_every_optional_member() {
        for member in OPTIONAL_MEMBERS {
            let omitted = without(&supplied, member);
            // Both orderings: whichever of the two spellings arrives first, the other is
            // still a first call and not a repeat of it.
            for (order, first_request, second_request) in [
                ("supplied then omitted", &supplied, &omitted),
                ("omitted then supplied", &omitted, &supplied),
            ] {
                let case = format!("case {case:?} dropping {member:?}, {order}");
                // A fresh emulator per ordering, so no earlier pair's domains count
                // against this one's account-wide cap.
                let emulator = Emulator::new("us-east-1");
                let first = emulator.create_domain(first_request).await;
                let second = emulator.create_domain(second_request).await;
                let (first_id, second_id) =
                    (string_at(&first, "domainId"), string_at(&second, "domainId"));
                assert_ne!(
                    first_id, second_id,
                    "{case}: a request differing in which optional members it carries \
                     must create its own domain, got {second_id} for both"
                );
                let held = held_domains(&emulator).await;
                assert_eq!(
                    held, 2,
                    "{case}: the account must hold both domains, got {held} held"
                );
                // The ARNs and creation stamps are the two domains' own, not one
                // domain's reported twice.
                assert_ne!(
                    string_at(&first, "arn"),
                    string_at(&second, "arn"),
                    "{case}: each domain must carry its own ARN, got {second}"
                );
                // And each spelling's own repeat still resolves to the domain THAT
                // spelling created, so the two are told apart in both directions.
                for (label, repeated_request, expected) in [
                    ("the first spelling", first_request, &first),
                    ("the second spelling", second_request, &second),
                ] {
                    let repeated = emulator.create_domain(repeated_request).await;
                    assert_eq!(
                        &repeated, expected,
                        "{case}: repeating {label} must resolve to the domain it created, \
                         got {repeated}"
                    );
                }
                let held_after_repeats = held_domains(&emulator).await;
                assert_eq!(
                    held_after_repeats, 2,
                    "{case}: the repeats must consume no further slot, got \
                     {held_after_repeats} held"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_two_tags_spellings_differ_as_requests_and_not_in_what_is_stored() {
    // Dropping `tags` changes which request this is, not the tag set the new domain ends
    // up holding: a map with no entries and no map at all both leave the domain with
    // nothing tagged.
    let emulator = Emulator::new("us-east-1");
    let supplied = json!({ "name": "clinic", "tags": {} });
    let omitted = json!({ "name": "clinic" });

    let with_empty_map = emulator.create_domain(&supplied).await;
    let with_no_map = emulator.create_domain(&omitted).await;
    for (case, created) in [
        ("tags supplied as an empty map", &with_empty_map),
        ("tags not carried at all", &with_no_map),
    ] {
        let domain_id = string_at(created, "domainId");
        let held = domain_tags(&emulator, &domain_id).await;
        assert!(
            held.is_empty(),
            "case {case:?}: the domain must hold no tags, got {held:?}"
        );
    }
    assert_ne!(
        string_at(&with_empty_map, "domainId"),
        string_at(&with_no_map, "domainId"),
        "the two spellings are different requests, so each must have its own domain, \
         got {with_no_map}"
    );

    // A third spelling of the same name — one tag entry — is a third request, so the
    // account holds three domains and each repeat still answers its own.
    let with_one_entry = emulator
        .create_domain(&json!({ "name": "clinic", "tags": { "env": "prod" } }))
        .await;
    let held = held_domains(&emulator).await;
    assert_eq!(
        held, 3,
        "each spelling must hold its own domain, got {held} held"
    );
    for (case, request, expected) in [
        ("an empty map", supplied, &with_empty_map),
        ("no map at all", omitted, &with_no_map),
        (
            "one entry",
            json!({ "name": "clinic", "tags": { "env": "prod" } }),
            &with_one_entry,
        ),
    ] {
        let repeated = emulator.create_domain(&request).await;
        assert_eq!(
            &repeated, expected,
            "case {case:?}: the repeat must resolve to the domain that spelling created, \
             got {repeated}"
        );
    }
}
