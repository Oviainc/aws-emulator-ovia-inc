// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of ARN resolution across a domain's deletion window.
//!
//! The RULE under test, asserted over tables rather than one recorded request: an
//! ARN-addressed operation resolves a resource for exactly as long as the service HOLDS
//! its record, not for as long as that record reports a particular lifecycle status — so
//! `ListTagsForResource` (and the `TagResource` that writes the set it reports) resolves
//! an ARN at precisely the instants the resource's OWN reader resolves the resource, and
//! the two stop together when the record is purged.
//!
//! The rows differ along what the rule generalises over: how many tag entries the resource
//! carries (none, one, several, the per-resource cap), the FORM of the keys and the values
//! (ASCII, non-ASCII, spaces and symbols, an empty value, each at its published maximum
//! length), whether the domain's deletion completes inside the `DeleteDomain` call (a
//! domain with no web application, which reaches the terminal status at once) or passes
//! through the non-terminal `DELETING` (a domain owning a web application, torn down
//! outside this service), the region the requests are signed for, and which resource kind
//! the ARN names — a `Domain`, whose record outlives the deleting call, and a
//! `Subscription`, whose own reader stops serving the moment that call records the
//! deletion.
//!
//! Sources for every bound and catalog written by hand here:
//!
//! * `smithy_query(query="shape",shape="DomainStatus",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `ACTIVE | DELETING | DELETED`, so `DELETING` is a state of its own and distinct
//!   from the terminal `DELETED`.
//! * `smithy_query(query="operation",operation="ListTagsForResource,TagResource,GetDomain",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `GET /tags/{resourceArn}` answering 200 and `@readonly`, `POST /tags/{resourceArn}`
//!   answering 204, both with `errors: []` (so the 404 asserted here is the shape the
//!   SERVICE models, widened in), and `GET /domain/{domainId}` answering 200 with a
//!   required `status` and a `tags` map, or the modelled `ResourceNotFoundException` (404).
//! * `smithy_query(query="shape",shape="TagMap",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — key `@length 1..128`, value `@length 0..256`, and no length on the map itself, so
//!   the count cap is the service's own (`state::MAX_TAGS_PER_RESOURCE`).

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    DOMAIN_DELETION_SPAN, MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, tag_map, Emulator};
use crate::list_tags_for_resource::list_tags;
use crate::resource_tagging::{domain_tags, entries, tag_resource, tags_path};

/// Long enough for the `DELETING` -> purged transition the clock owes to have fallen due,
/// taken from the span the crate itself waits rather than restated as a literal.
fn past_the_deletion_span() -> Duration {
    DOMAIN_DELETION_SPAN + Duration::from_millis(400)
}

/// The `webAppSetupConfiguration` that makes a domain own a resource provisioned OUTSIDE
/// this service, which is what keeps its deletion from completing inside the
/// `DeleteDomain` call.
fn web_app_setup() -> Value {
    json!({
        "ehrRole": "arn:aws:iam::000000000000:role/ehr-operations",
        "idcInstanceId": "ssoins-72230ed02aa4b4ea",
        "idcRegion": "us-east-1",
    })
}

/// What the two readers of ONE domain answer: `GetDomain`, which resolves the record by
/// id, and `ListTagsForResource`, which resolves the SAME resource by ARN.
///
/// Asserts the invariant itself — either both resolve or both refuse with the service's
/// `ResourceNotFoundException` (404) — and, when they resolve, that they report the same
/// tag set entry for entry. Returns the status the record reports and that set, or `None`
/// when the resource resolves nowhere.
async fn resolution(
    emulator: &Emulator,
    case: &str,
    phase: &str,
    domain_id: &str,
    arn: &str,
) -> Option<(String, BTreeMap<String, String>)> {
    let (by_id, record) = emulator
        .send("GET", &format!("/domain/{domain_id}"), None)
        .await;
    let (by_arn, tags) = emulator.send("GET", &tags_path(arn), None).await;
    assert_eq!(
        by_arn, by_id,
        "case {case:?} at {phase:?}: ListTagsForResource must resolve {arn} exactly when \
         GetDomain resolves the domain, got {by_arn} with {tags} against {by_id} with {record}"
    );
    if by_id == 404 {
        assert_eq!(
            (error_code(&record).as_str(), error_code(&tags).as_str()),
            ("ResourceNotFoundException", "ResourceNotFoundException"),
            "case {case:?} at {phase:?}: a resource this service no longer holds is refused \
             through both readers with the service's 404, got {record} and {tags}"
        );
        return None;
    }
    assert_eq!(
        by_id, 200,
        "case {case:?} at {phase:?}: the readers must answer 200 or 404, got {by_id} with {record}"
    );
    let through_arn = list_tags(emulator, case, arn).await;
    let through_record = domain_tags(emulator, domain_id).await;
    assert_eq!(
        through_arn, through_record,
        "case {case:?} at {phase:?}: both readers of one tag store must report the same \
         entries, got {through_arn:?} by ARN and {through_record:?} by id"
    );
    Some((string_at(&record, "status"), through_arn))
}

#[tokio::test]
async fn a_domains_arn_resolves_for_as_long_as_the_service_holds_the_record() {
    // A key and a value at each published maximum length, built here rather than taken
    // from a recorded request.
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let at_the_cap = tag_map(MAX_TAGS_PER_RESOURCE, 10, 16);

    // (case, does the domain own a web application?, the tags the create writes, region)
    let rows: Vec<(&str, bool, Value, &str)> = vec![
        (
            "a web-app domain with one tag",
            true,
            json!({ "env": "prod" }),
            "us-east-1",
        ),
        (
            "a web-app domain with several tags of mixed form",
            true,
            json!({
                "env": "prod",
                "clínica": "ü-ok",
                "cost center": "acct/42 +x",
                "retired": "",
            }),
            "eu-central-1",
        ),
        (
            "a web-app domain carrying no tags at all",
            true,
            json!({}),
            "ap-south-1",
        ),
        (
            "a web-app domain with a key and a value at their maximum length",
            true,
            json!({ longest_key.clone(): longest_value.clone() }),
            "us-west-2",
        ),
        (
            "a web-app domain at the per-resource tag cap",
            true,
            at_the_cap.clone(),
            "ap-southeast-4",
        ),
        (
            "a domain with no web application, whose deletion completes in the call",
            false,
            json!({ "env": "prod" }),
            "us-east-2",
        ),
    ];

    for (case, owns_web_app, tags, region) in &rows {
        let emulator = Emulator::new(region);
        let mut request = json!({ "name": "clinic", "tags": tags });
        if *owns_web_app {
            request["webAppSetupConfiguration"] = web_app_setup();
        }
        let created = emulator.create_domain(&request).await;
        let domain_id = string_at(&created, "domainId");
        let arn = string_at(&created, "arn");
        let mut expected: BTreeMap<String, String> = entries(tags).into_iter().collect();

        // While the domain is live both readers resolve it and agree on its tags.
        let (status, reported) = resolution(&emulator, case, "created", &domain_id, &arn)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: a created domain must resolve"));
        assert_eq!(
            (status.as_str(), &reported),
            ("ACTIVE", &expected),
            "case {case:?}: a created domain is ACTIVE and reports the set the create \
             wrote, got {status:?} and {reported:?}"
        );

        let (status, deleted) = emulator
            .send("DELETE", &format!("/domain/{domain_id}"), None)
            .await;
        assert_eq!(status, 200, "case {case:?}: DeleteDomain got {deleted}");
        // Which status the delete reports follows from what the domain owns: only the
        // teardown that outlives the call passes through the non-terminal state.
        let expected_status = if *owns_web_app { "DELETING" } else { "DELETED" };
        assert_eq!(
            string_at(&deleted, "status"),
            expected_status,
            "case {case:?}: DeleteDomain must report {expected_status}, got {deleted}"
        );

        if *owns_web_app {
            // Inside the window the record is still HELD, so the resource is still
            // addressable by ARN — a deletion in flight is not a resource that is gone.
            let (status, reported) =
                resolution(&emulator, case, "the deletion window", &domain_id, &arn)
                    .await
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: a domain whose deletion is in flight is still held, \
                             so both readers must resolve it"
                        )
                    });
            assert_eq!(
                (status.as_str(), &reported),
                ("DELETING", &expected),
                "case {case:?}: the deletion must not change the tag set it reports, \
                 got {status:?} and {reported:?}"
            );

            // The ARN-addressed WRITE resolves the same record: an overwrite when the
            // resource already sits at the count cap, a new key otherwise, so the row at
            // the cap is not refused for the wrong reason.
            let (key, value) = match expected.keys().next() {
                Some(existing) if expected.len() >= MAX_TAGS_PER_RESOURCE => (
                    existing.clone(),
                    "overwritten-while-deleting".to_string(),
                ),
                _ => ("phase".to_string(), "deleting".to_string()),
            };
            tag_resource(
                &emulator,
                case,
                &arn,
                &json!({ key.clone(): value.clone() }),
            )
            .await;
            expected.insert(key, value);
            let (_, reported) = resolution(
                &emulator,
                case,
                "after a write inside the deletion window",
                &domain_id,
                &arn,
            )
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the record is still held after a write"));
            assert_eq!(
                reported, expected,
                "case {case:?}: a write accepted on the ARN must be the set both readers \
                 then report, got {reported:?}"
            );
        } else {
            // A deletion that finished inside the call left no record behind, so the ARN
            // stops resolving at the same instant the record does.
            assert!(
                resolution(
                    &emulator,
                    case,
                    "after a deletion that completed in the call",
                    &domain_id,
                    &arn,
                )
                .await
                .is_none(),
                "case {case:?}: a domain purged inside the deleting call resolves nowhere"
            );
        }

        // Once the purge the clock owes has fallen due the record is gone, and the ARN
        // stops resolving with it — through both readers, at the same instant.
        // Real elapsed time is required (the handlers read `SystemTime::now()`), and the
        // workspace `tokio` dependency does not enable the `time` feature, so this is a
        // plain blocking sleep rather than `tokio::time::sleep(..).await`.
        std::thread::sleep(past_the_deletion_span());
        assert!(
            resolution(
                &emulator,
                case,
                "past the deletion span",
                &domain_id,
                &arn,
            )
            .await
            .is_none(),
            "case {case:?}: a purged record resolves nowhere"
        );
        // And the write path refuses the purged ARN exactly as the read does.
        let (status, body) = emulator
            .send(
                "POST",
                &tags_path(&arn),
                Some(&json!({ "tags": { "env": "should-not-persist" } })),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: TagResource on a purged ARN must be refused, got {status} \
             with {body}"
        );
    }
}

#[tokio::test]
async fn a_subscriptions_arn_stops_resolving_with_its_own_reader() {
    // The other half of the same rule: the ARN reader follows the resource's OWN reader,
    // so it neither outlives it nor stops before it. `DeleteDomain` marks every
    // subscription of the domain `DELETED` while the DOMAIN's record is still held, so in
    // that one window the subscription ARNs must stop resolving even though the domain's
    // does not.
    //
    // The rows differ along how many subscriptions the domain holds, so the rule is not
    // checked on a single-element collection only.
    for (case, count, region) in [
        ("one subscription", 1, "us-east-1"),
        ("three subscriptions", 3, "eu-west-1"),
    ] {
        let emulator = Emulator::new(region);
        let created = emulator
            .create_domain(&json!({
                "name": "clinic",
                "tags": { "env": "prod" },
                "webAppSetupConfiguration": web_app_setup(),
            }))
            .await;
        let domain_id = string_at(&created, "domainId");
        let domain_arn = string_at(&created, "arn");

        let mut subscriptions: Vec<(String, String)> = Vec::new();
        for index in 0..count {
            let (status, subscription) = emulator
                .send(
                    "POST",
                    &format!("/domains/{domain_id}/subscriptions"),
                    Some(&json!({})),
                )
                .await;
            assert_eq!(
                status, 201,
                "case {case:?} #{index}: CreateSubscription got {subscription}"
            );
            let subscription_id = string_at(&subscription, "subscriptionId");
            let subscription_arn = string_at(&subscription, "arn");
            // Each subscription carries a tag set of its own, so a reader that resolved
            // the wrong record would report the wrong entries rather than nothing.
            tag_resource(
                &emulator,
                case,
                &subscription_arn,
                &json!({ "tier": format!("tier-{index}") }),
            )
            .await;
            subscriptions.push((subscription_id, subscription_arn));
        }

        // While the domain is live, both of a subscription's readers resolve it.
        for (index, (subscription_id, subscription_arn)) in subscriptions.iter().enumerate() {
            let (status, body) = emulator
                .send(
                    "GET",
                    &format!("/domains/{domain_id}/subscriptions/{subscription_id}"),
                    None,
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} #{index}: GetSubscription got {body}"
            );
            let reported = list_tags(&emulator, case, subscription_arn).await;
            assert_eq!(
                reported,
                [("tier".to_string(), format!("tier-{index}"))]
                    .into_iter()
                    .collect::<BTreeMap<_, _>>(),
                "case {case:?} #{index}: the subscription must report its own set, \
                 got {reported:?}"
            );
        }

        let (status, deleted) = emulator
            .send("DELETE", &format!("/domain/{domain_id}"), None)
            .await;
        assert_eq!(status, 200, "case {case:?}: DeleteDomain got {deleted}");
        assert_eq!(
            string_at(&deleted, "status"),
            "DELETING",
            "case {case:?}: a web-app domain's deletion passes through DELETING, \
             got {deleted}"
        );

        // In that window the domain's own ARN still resolves...
        assert!(
            resolution(
                &emulator,
                case,
                "the domain in its deletion window",
                &domain_id,
                &domain_arn,
            )
            .await
            .is_some(),
            "case {case:?}: the domain's record is still held, so its ARN resolves"
        );
        // ...and each subscription's does not, because its own reader no longer does.
        for (index, (subscription_id, subscription_arn)) in subscriptions.iter().enumerate() {
            let (by_id, record) = emulator
                .send(
                    "GET",
                    &format!("/domains/{domain_id}/subscriptions/{subscription_id}"),
                    None,
                )
                .await;
            let (by_arn, tags) = emulator
                .send("GET", &tags_path(subscription_arn), None)
                .await;
            assert_eq!(
                (by_id, error_code(&record).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?} #{index}: a subscription of a domain being deleted is gone \
                 for its own reader, got {by_id} with {record}"
            );
            assert_eq!(
                (by_arn, error_code(&tags).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?} #{index}: so its ARN must stop resolving too, got {by_arn} \
                 with {tags}"
            );
        }
    }
}
