// SPDX-License-Identifier: CC0-1.0

//! `CreateDomain` is modelled `@idempotent`, so a REPEAT of one request has the effect of
//! the single request: it resolves to the domain that request already created, mints no
//! second `domainId`, consumes no further slot of the account's per-Region domain quota
//! and writes nothing.
//!
//! The rule is asserted over a TABLE of requests rather than the one case the issue
//! recorded, the rows differing along what `CreateDomain` generalises over — which
//! optional members are present, the size of `name`, how many tags are carried, and
//! whether a web application is asked for — and over a second table of requests that
//! differ from one another in exactly ONE parameter, each of which is a DIFFERENT request
//! and so creates its own domain.
//!
//! Model citations
//! (`smithy_query(query="operation",operation="CreateDomain",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`):
//! `CreateDomain` is `POST /domain` answering 201, reports `"idempotent": true`, carries
//! no idempotency token, and its input members are `name` (required, `@length` 1..64),
//! `kmsKeyArn`, `tags` and `webAppSetupConfiguration` — the four parameters that identify
//! one request. The account's per-Region domain cap is
//! `state::MAX_DOMAINS_PER_ACCOUNT`, surfaced as HTTP 400 `ValidationException`.
//!
//! The harness (`Emulator`, `error_code`, `string_at`) and the domain-count and tag
//! helpers are the sibling suites': one way of driving this service, not two.

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::MAX_DOMAINS_PER_ACCOUNT;

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::resource_tagging::{domain_tags, tag_resource, tags_path};
use crate::tag_count_cap::held_domains;

/// A customer-managed key, as `KmsKeyArn`'s modelled pattern spells one
/// (`^arn:aws:kms:[a-z0-9-]+:[0-9]{12}:key/[a-f0-9-]+$`).
fn kms_key(key_id: &str) -> String {
    format!("arn:aws:kms:us-east-1:000000000000:key/{key_id}")
}

/// A `CreateWebAppConfiguration`: the three members that shape its request, each named so
/// a row can vary exactly one of them.
fn web_app(ehr_role: &str, idc_instance_id: &str, idc_region: &str) -> Value {
    json!({
        "ehrRole": ehr_role,
        "idcInstanceId": idc_instance_id,
        "idcRegion": idc_region,
    })
}

/// The web-app setup every row that asks for one carries.
fn default_web_app() -> Value {
    web_app(
        "arn:aws:iam::000000000000:role/ehr-operations",
        "ssoins-72230ed02aa4b4ea",
        "us-east-1",
    )
}

/// The requests whose repeat must resolve to the domain the first call created: the rows
/// differ along which optional members are present, the size of `name`, how many tags are
/// carried, and whether a web application is asked for. The issue's own case
/// (`{"name": "clinic-a"}`) is one row of several.
fn repeatable_creates() -> Vec<(&'static str, Value)> {
    vec![
        ("minimal", json!({ "name": "clinic-a" })),
        ("name at the modelled minimum (1)", json!({ "name": "c" })),
        (
            "name at the modelled maximum (64)",
            json!({ "name": "c".repeat(64) }),
        ),
        (
            "a customer-managed key",
            json!({ "name": "clinic-kms", "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789") }),
        ),
        (
            "one tag",
            json!({ "name": "clinic-tag", "tags": { "env": "test" } }),
        ),
        (
            "three tags",
            json!({ "name": "clinic-tags", "tags": { "env": "test", "team": "cardio", "cost": "42" } }),
        ),
        (
            "an empty tag map",
            json!({ "name": "clinic-no-tags", "tags": {} }),
        ),
        (
            "every optional member at once, including a web application",
            json!({
                "name": "clinic-everything",
                "kmsKeyArn": kms_key("beefbeef-0000-1111-2222-333344445555"),
                "tags": { "env": "prod" },
                "webAppSetupConfiguration": default_web_app(),
            }),
        ),
    ]
}

/// `GetDomain`, asserting the stored record reports every member the create answered with.
async fn assert_reads_back(emulator: &Emulator, case: &str, created: &Value) {
    let domain_id = string_at(created, "domainId");
    let (status, read) = emulator
        .send("GET", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetDomain {domain_id} must report the stored record, got {status} \
         with {read}"
    );
    let members = created
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the create must answer an object, got {created}"));
    for (member, value) in members {
        assert_eq!(
            read.get(member),
            Some(value),
            "case {case:?}: GetDomain must report the same {member} the create did, \
             got {read}"
        );
    }
}

#[tokio::test]
async fn an_identical_repeat_of_a_create_resolves_to_the_domain_the_first_one_created() {
    let cases = repeatable_creates();
    assert!(
        cases.len() <= MAX_DOMAINS_PER_ACCOUNT,
        "the table must fit the account's cap of {MAX_DOMAINS_PER_ACCOUNT} domains, \
         got {} rows",
        cases.len()
    );

    // ONE emulator for every row, so the count of domains the account holds after the
    // table is what proves a repeat consumed no slot.
    let emulator = Emulator::new("us-east-1");
    for (index, (case, request)) in cases.iter().enumerate() {
        let first = emulator.create_domain(request).await;
        // Two further byte-identical sends of the same request — an SDK that retried a
        // 201 it never saw, twice.
        for attempt in 1..=2 {
            let (status, repeated) = emulator.send("POST", "/domain", Some(request)).await;
            assert_eq!(
                status, 201,
                "case {case:?}: repeat {attempt} must be served as the first call was, \
                 got {status} with {repeated}"
            );
            assert_eq!(
                repeated, first,
                "case {case:?}: repeat {attempt} must answer the domain the first call \
                 created, got {repeated}"
            );
        }
        assert_eq!(
            held_domains(&emulator).await,
            index + 1,
            "case {case:?}: the repeats must leave the account holding one domain per \
             logical create, got {} after row {index}",
            held_domains(&emulator).await
        );
        assert_reads_back(&emulator, case, &first).await;
    }
}

#[tokio::test]
async fn a_create_differing_in_any_one_parameter_is_a_different_request() {
    let base = json!({
        "name": "clinic",
        "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
        "tags": { "env": "prod" },
        "webAppSetupConfiguration": default_web_app(),
    });
    // One row per parameter `CreateDomain` accepts, each differing from `base` in exactly
    // that parameter — including the two ways an optional member can differ: a different
    // value, and no value at all.
    let variants: Vec<(&str, Value)> = vec![
        (
            "a different name",
            json!({
                "name": "clinic-two",
                "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
                "tags": { "env": "prod" },
                "webAppSetupConfiguration": default_web_app(),
            }),
        ),
        (
            "a different kmsKeyArn",
            json!({
                "name": "clinic",
                "kmsKeyArn": kms_key("00000000-1111-2222-3333-444444444444"),
                "tags": { "env": "prod" },
                "webAppSetupConfiguration": default_web_app(),
            }),
        ),
        (
            "no kmsKeyArn at all",
            json!({
                "name": "clinic",
                "tags": { "env": "prod" },
                "webAppSetupConfiguration": default_web_app(),
            }),
        ),
        (
            "a different tag value",
            json!({
                "name": "clinic",
                "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
                "tags": { "env": "test" },
                "webAppSetupConfiguration": default_web_app(),
            }),
        ),
        (
            "a different tag key",
            json!({
                "name": "clinic",
                "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
                "tags": { "stage": "prod" },
                "webAppSetupConfiguration": default_web_app(),
            }),
        ),
        (
            "one more tag entry",
            json!({
                "name": "clinic",
                "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
                "tags": { "env": "prod", "team": "cardio" },
                "webAppSetupConfiguration": default_web_app(),
            }),
        ),
        (
            "no tags at all",
            json!({
                "name": "clinic",
                "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
                "webAppSetupConfiguration": default_web_app(),
            }),
        ),
        (
            "a different ehrRole",
            json!({
                "name": "clinic",
                "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
                "tags": { "env": "prod" },
                "webAppSetupConfiguration": web_app(
                    "arn:aws:iam::000000000000:role/ehr-readonly",
                    "ssoins-72230ed02aa4b4ea",
                    "us-east-1",
                ),
            }),
        ),
        (
            "a different idcInstanceId",
            json!({
                "name": "clinic",
                "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
                "tags": { "env": "prod" },
                "webAppSetupConfiguration": web_app(
                    "arn:aws:iam::000000000000:role/ehr-operations",
                    "ssoins-0000000000000001",
                    "us-east-1",
                ),
            }),
        ),
        (
            "a different idcRegion",
            json!({
                "name": "clinic",
                "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
                "tags": { "env": "prod" },
                "webAppSetupConfiguration": web_app(
                    "arn:aws:iam::000000000000:role/ehr-operations",
                    "ssoins-72230ed02aa4b4ea",
                    "eu-west-1",
                ),
            }),
        ),
        (
            "no web application at all",
            json!({
                "name": "clinic",
                "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
                "tags": { "env": "prod" },
            }),
        ),
    ];

    for (case, variant) in &variants {
        // A fresh emulator per row, so no row's two domains count against the next row's
        // account-wide cap.
        let emulator = Emulator::new("us-east-1");
        let first = string_at(&emulator.create_domain(&base).await, "domainId");
        let other = string_at(&emulator.create_domain(variant).await, "domainId");
        assert_ne!(
            first, other,
            "case {case:?}: a request differing in one parameter must create its own \
             domain, got {other} for both"
        );
        assert_eq!(
            held_domains(&emulator).await,
            2,
            "case {case:?}: the account must hold both domains, got {} held",
            held_domains(&emulator).await
        );
        // And each one's own repeat still resolves to its own domain, so the two
        // requests are told apart in both directions.
        for (label, request, expected) in [
            ("the base request", &base, &first),
            ("the varied request", variant, &other),
        ] {
            let repeated = string_at(&emulator.create_domain(request).await, "domainId");
            assert_eq!(
                &repeated, expected,
                "case {case:?}: repeating {label} must resolve to {expected}, got {repeated}"
            );
        }
    }
}

#[tokio::test]
async fn a_repeat_at_the_account_cap_is_served_and_consumes_no_slot() {
    let emulator = Emulator::new("us-east-1");
    // Fill the cap with distinct requests, so every slot is taken by a logical create.
    let mut ids: Vec<String> = Vec::new();
    for index in 0..MAX_DOMAINS_PER_ACCOUNT {
        let created = emulator
            .create_domain(&json!({ "name": format!("clinic-{index}") }))
            .await;
        ids.push(string_at(&created, "domainId"));
    }

    // Every one of those requests repeated AT the cap: a repeat grows nothing, so the cap
    // it does not grow cannot refuse it.
    for (index, expected) in ids.iter().enumerate() {
        let request = json!({ "name": format!("clinic-{index}") });
        let (status, repeated) = emulator.send("POST", "/domain", Some(&request)).await;
        assert_eq!(
            status, 201,
            "case {request}: a repeat at the cap must be served, got {status} with {repeated}"
        );
        assert_eq!(
            &string_at(&repeated, "domainId"),
            expected,
            "case {request}: the repeat must answer the domain it already created, \
             got {repeated}"
        );
    }

    // A request the account has NOT already created is a first call, and the cap refuses
    // it as before.
    let (status, refused) = emulator
        .send("POST", "/domain", Some(&json!({ "name": "one-too-many" })))
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "a new create at the cap must still be refused, got {status} with {refused}"
    );
    assert_eq!(
        held_domains(&emulator).await,
        MAX_DOMAINS_PER_ACCOUNT,
        "neither the repeats nor the refusal may change what the account holds, got {} held",
        held_domains(&emulator).await
    );
}

#[tokio::test]
async fn a_repeat_is_recognised_per_region_and_only_while_its_domain_is_live() {
    let request = json!({ "name": "clinic-a", "tags": { "env": "prod" } });

    // The endpoint is regional: the same request addressed to another Region addresses
    // that Region's inventory, so it is a first call there.
    let emulator = Emulator::new("us-east-1");
    let home = string_at(&emulator.create_domain(&request).await, "domainId");
    let away = string_at(
        &emulator.create_domain_in("eu-west-1", &request).await,
        "domainId",
    );
    assert_ne!(
        home, away,
        "the same request in another Region must create that Region's own domain, \
         got {home} for both"
    );
    for (region, expected) in [("us-east-1", &home), ("eu-west-1", &away)] {
        let repeated = string_at(
            &emulator.create_domain_in(region, &request).await,
            "domainId",
        );
        assert_eq!(
            &repeated, expected,
            "the repeat in {region} must resolve to that Region's domain, got {repeated}"
        );
    }

    // Once the domain the first call created is gone, the same request is a first call
    // again: there is no longer a domain for it to resolve to.
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{home}"), None)
        .await;
    assert_eq!(
        status, 200,
        "DeleteDomain {home} got {status} with {deleted}"
    );
    let after = string_at(&emulator.create_domain(&request).await, "domainId");
    assert_ne!(
        after, home,
        "after the domain it created is gone the same request must create a new one, \
         got {after}"
    );
    let repeated = string_at(&emulator.create_domain(&request).await, "domainId");
    assert_eq!(
        repeated, after,
        "and the new domain is what the next repeat resolves to, got {repeated}"
    );
}

#[tokio::test]
async fn a_repeat_writes_nothing_so_a_tag_set_after_the_create_survives_it() {
    let emulator = Emulator::new("us-east-1");
    let request = json!({ "name": "clinic-a", "tags": { "env": "prod" } });
    let created = emulator.create_domain(&request).await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");

    tag_resource(
        &emulator,
        "a tag added after the create",
        &arn,
        &json!({ "team": "cardio" }),
    )
    .await;

    let (status, repeated) = emulator.send("POST", "/domain", Some(&request)).await;
    assert_eq!(
        status, 201,
        "the repeat must be served, got {status} with {repeated}"
    );
    assert_eq!(
        repeated, created,
        "the repeat must answer the domain the create made, got {repeated}"
    );

    let held = domain_tags(&emulator, &domain_id).await;
    let expected = [
        ("env".to_string(), "prod".to_string()),
        ("team".to_string(), "cardio".to_string()),
    ];
    assert_eq!(
        held,
        expected.iter().cloned().collect(),
        "a repeat writes nothing, so the set the domain holds is the merged one, \
         got {held:?}"
    );
    // The same set through the tag reader the repeat left untouched.
    let (status, listed) = emulator.send("GET", &tags_path(&arn), None).await;
    assert_eq!(status, 200, "ListTagsForResource got {status} with {listed}");
    for (key, value) in &expected {
        assert_eq!(
            listed.get("tags").and_then(|tags| tags.get(key)),
            Some(&Value::String(value.clone())),
            "ListTagsForResource must report {key}={value} after the repeat, got {listed}"
        );
    }
}
