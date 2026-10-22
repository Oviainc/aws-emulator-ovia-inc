// SPDX-License-Identifier: CC0-1.0

//! `DeleteSamplingRule`, and the tags a sampling rule carries, end to end over
//! the wire.
//!
//! Every case drives the SAME front door the production router does: a
//! `MultiServiceRouter` with this service mounted, addressed by an `xray`
//! endpoint host, speaking restJson1. The `Xray`/`Answer` harness is the
//! `pub(crate)` one `trace_ingest_and_groups.rs` exposes, so this suite adds
//! only the sampling-shaped helpers; `sampling_rules.rs` owns the create/list
//! rules, and this one owns what happens after.
//!
//! The rules these tables check:
//!
//! * a rule is selected by `RuleName` or `RuleARN` — "Specify a rule by either
//!   name or ARN, but not both" — so neither selector, both selectors, and a
//!   selector matching no stored rule are each refused with
//!   `InvalidRequestException` (400) and remove nothing;
//! * a delete answers the record it removed — the one the create stored — and
//!   removes exactly that rule: no later listing carries it, no later selector
//!   resolves it, and every other rule is untouched;
//! * a rule's `RuleARN` names the region the creating request addressed, and a
//!   reader in any region reports that stored ARN unchanged;
//! * a sampling rule is a taggable resource, so the tags its create carried are
//!   the ones its ARN reports, `TagResource` adds to that same set, and the ARN
//!   of a rule that was never created names no resource.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::trace_ingest_and_groups::{Answer, Xray, DEFAULT_REGION};

/// A complete, valid `SamplingRule` definition, so a case only has to say what
/// it varies.
fn rule(name: &str) -> Value {
    json!({
        "RuleName": name,
        "ResourceARN": "*",
        "Priority": 1000,
        "FixedRate": 0.1,
        "ReservoirSize": 2,
        "ServiceName": "*",
        "ServiceType": "*",
        "Host": "*",
        "HTTPMethod": "*",
        "URLPath": "*",
        "Version": 1
    })
}

/// `CreateSamplingRule` in `region`, with optional tags.
async fn create_rule_in(xray: &Xray, region: &str, rule: &Value, tags: Option<&Value>) -> Answer {
    let mut body = json!({ "SamplingRule": rule });
    if let Some(tags) = tags {
        body["Tags"] = tags.clone();
    }
    xray.call_in_region(region, "/CreateSamplingRule", &body)
        .await
}

/// `CreateSamplingRule` in the default region.
async fn create_rule(xray: &Xray, rule: &Value, tags: Option<&Value>) -> Answer {
    create_rule_in(xray, DEFAULT_REGION, rule, tags).await
}

/// `DeleteSamplingRule` in `region`.
async fn delete_rule_in(xray: &Xray, region: &str, body: &Value) -> Answer {
    xray.call_in_region(region, "/DeleteSamplingRule", body)
        .await
}

/// `DeleteSamplingRule` in the default region.
async fn delete_rule(xray: &Xray, body: &Value) -> Answer {
    delete_rule_in(xray, DEFAULT_REGION, body).await
}

/// `GetSamplingRules` in `region`, resuming from `token`.
async fn list_rules_in(xray: &Xray, region: &str, token: Option<&str>) -> Answer {
    let mut body = json!({});
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    xray.call_in_region(region, "/GetSamplingRules", &body).await
}

/// The `RuleName` of one `SamplingRuleRecord`.
fn rule_name(record: &Value) -> &str {
    record
        .get("SamplingRule")
        .and_then(|rule| rule.get("RuleName"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// A member of the `SamplingRule` inside one `SamplingRuleRecord`.
fn rule_member<'a>(record: &'a Value, member: &str) -> &'a Value {
    record
        .get("SamplingRule")
        .and_then(|rule| rule.get(member))
        .unwrap_or(&Value::Null)
}

/// The ARN a rule created through `region`'s endpoint carries:
/// `sampling-rule/<RuleName>` under the canonical emulator account.
fn expected_rule_arn(region: &str, rule_name: &str) -> String {
    format!(
        "arn:aws:xray:{region}:{}:sampling-rule/{rule_name}",
        ::emulator_server::emulator_account_id()
    )
}

/// Every stored rule in `region`, by `RuleName`, across as many pages as the
/// listing takes. The walk also asserts that no rule is served twice.
async fn all_rules_in(xray: &Xray, region: &str, case: &str) -> BTreeMap<String, Value> {
    let mut rules: BTreeMap<String, Value> = BTreeMap::new();
    let mut token: Option<String> = None;
    for page in 0..32 {
        let answer = list_rules_in(xray, region, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: listing page {page} must answer 200, got {answer:?}"
        );
        let records = answer
            .get("SamplingRuleRecords")
            .as_array()
            .cloned()
            .unwrap_or_default();
        for record in records {
            let name = rule_name(&record).to_string();
            let previous = rules.insert(name.clone(), record);
            assert!(
                previous.is_none(),
                "case {case:?}: rule {name:?} was listed twice across pages"
            );
        }
        match answer.get("NextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return rules,
        }
    }
    panic!("case {case:?}: the listing never terminated: {rules:?}");
}

/// Every stored rule in the default region.
async fn all_rules(xray: &Xray, case: &str) -> BTreeMap<String, Value> {
    all_rules_in(xray, DEFAULT_REGION, case).await
}

#[tokio::test]
async fn delete_removes_exactly_the_selected_rule_and_answers_its_record() {
    let xray = Xray::new();
    for name in ["by-name", "by-arn", "kept"] {
        let answer = create_rule(&xray, &rule(name), None).await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: the fixture rule must be created, got {answer:?}"
        );
    }
    let before = all_rules(&xray, "before-delete").await;

    // The two selectors the model documents, each on its own rule.
    let cases: &[(&str, Value)] = &[
        ("by-name", json!({ "RuleName": "by-name" })),
        (
            "by-arn",
            json!({ "RuleARN": expected_rule_arn(DEFAULT_REGION, "by-arn") }),
        ),
    ];
    for (case, body) in cases {
        let answer = delete_rule(&xray, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: deleting a stored rule must answer 200, got {answer:?}"
        );
        assert_eq!(
            answer.get("SamplingRuleRecord"),
            &before[*case],
            "case {case:?}: the delete answers the record it removed, got {answer:?}"
        );

        // Gone: the same delete now matches nothing.
        let again = delete_rule(&xray, body).await;
        assert_eq!(
            again.status, 400,
            "case {case:?}: deleting a removed rule must be refused, got {again:?}"
        );
        assert_eq!(
            again.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case:?}: a selector matching nothing is an invalid request, got {again:?}"
        );
    }

    let listed = all_rules(&xray, "after-delete").await;
    assert_eq!(
        listed.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["Default", "kept"],
        "only the selected rules were removed, listing holds {:?}",
        listed.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        listed.get("kept"),
        before.get("kept"),
        "the rule nobody deleted is unchanged, got {:?}",
        listed.get("kept")
    );
}

#[tokio::test]
async fn a_delete_whose_selector_the_model_forbids_is_refused() {
    let xray = Xray::new();
    let created = create_rule(&xray, &rule("target"), None).await;
    assert_eq!(
        created.status, 200,
        "the fixture rule must be created: {created:?}"
    );

    // "Specify a rule by either name or ARN, but not both" — and a selector
    // naming no stored rule matches nothing.
    let cases: &[(&str, Value)] = &[
        ("neither-selector", json!({})),
        (
            "both-selectors",
            json!({
                "RuleName": "target",
                "RuleARN": expected_rule_arn(DEFAULT_REGION, "target")
            }),
        ),
        ("unknown-name", json!({ "RuleName": "never-created" })),
        (
            "unknown-arn",
            json!({ "RuleARN": expected_rule_arn(DEFAULT_REGION, "never-created") }),
        ),
        (
            "arn-of-another-service",
            json!({ "RuleARN": "arn:aws:ec2:us-east-1:000000000000:instance/i-1" }),
        ),
        (
            "arn-of-another-region",
            json!({ "RuleARN": expected_rule_arn("eu-west-1", "target") }),
        ),
    ];

    for (case, body) in cases {
        let answer = delete_rule(&xray, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: the delete must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case:?}: the refusal is an invalid request, got {answer:?}"
        );
        assert_eq!(
            answer.get("SamplingRuleRecord"),
            &Value::Null,
            "case {case:?}: a refused delete returns no record, got {answer:?}"
        );
    }

    let listed = all_rules(&xray, "forbidden-selectors").await;
    assert!(
        listed.contains_key("target"),
        "a refused delete removes nothing, listing holds {:?}",
        listed.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_rules_arn_names_the_region_it_was_created_in_and_does_not_move() {
    let xray = Xray::new();
    // The rows are the regional endpoints a rule can be created through.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"];
    for region in regions {
        let name = format!("rule-{region}");
        let answer = create_rule_in(&xray, region, &rule(&name), None).await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: creating a rule must answer 200, got {answer:?}"
        );
        assert_eq!(
            rule_member(answer.get("SamplingRuleRecord"), "RuleARN"),
            &json!(expected_rule_arn(region, &name)),
            "case {region:?}: the create answers this region's ARN, got {answer:?}"
        );
    }

    // And a reader anywhere reports each rule's stored ARN unchanged: a read
    // does not re-derive it from the reader's own region.
    for reader in regions {
        let listed = all_rules_in(&xray, reader, reader).await;
        assert_eq!(
            listed.len(),
            regions.len() + 1,
            "case {reader:?}: one store serves every endpoint, beside the service-managed Default rule, listing holds {:?}",
            listed.keys().collect::<Vec<_>>()
        );
        for (name, record) in &listed {
            // The service-managed rule is DERIVED, so the ARN a reader sees is
            // its own region's rather than a stored one.
            if name == "Default" {
                assert_eq!(
                    rule_member(record, "RuleARN"),
                    &json!(expected_rule_arn(reader, "Default")),
                    "case {reader:?}: the Default rule is read with this region's ARN, got {:?}",
                    rule_member(record, "RuleARN")
                );
                continue;
            }
            let created_in = name
                .strip_prefix("rule-")
                .expect("the fixture names carry their region");
            assert_eq!(
                rule_member(record, "RuleARN"),
                &json!(expected_rule_arn(created_in, name)),
                "case {reader:?}: rule {name:?} keeps the ARN it was stored with, got {:?}",
                rule_member(record, "RuleARN")
            );
        }
    }

    // The rule a region's own ARN names is the one that region's delete removes.
    let removed = delete_rule_in(
        &xray,
        "eu-west-1",
        &json!({ "RuleARN": expected_rule_arn("eu-west-1", "rule-eu-west-1") }),
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "the endpoint's own ARN selects the rule, got {removed:?}"
    );
    assert_eq!(
        rule_member(removed.get("SamplingRuleRecord"), "RuleARN"),
        &json!(expected_rule_arn("eu-west-1", "rule-eu-west-1")),
        "the delete answers the removed rule's stored ARN, got {removed:?}"
    );
}

#[tokio::test]
async fn a_rules_tags_are_reachable_by_its_arn() {
    let xray = Xray::new();
    let created = create_rule(
        &xray,
        &rule("tagged"),
        Some(&json!([
            { "Key": "team", "Value": "payments" },
            { "Key": "stage", "Value": "prod" }
        ])),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture rule must be created: {created:?}"
    );

    // A sampling rule is a taggable X-Ray resource, so the tags a create
    // carried are the ones its ARN reports, and `TagResource` adds to them.
    let arn = expected_rule_arn(DEFAULT_REGION, "tagged");
    let listed = xray
        .call_in_region(
            DEFAULT_REGION,
            "/ListTagsForResource",
            &json!({ "ResourceARN": arn }),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "a created rule's ARN must resolve for tagging, got {listed:?}"
    );
    let mut tags: Vec<(String, String)> = listed
        .get("Tags")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    tags.sort();
    assert_eq!(
        tags,
        vec![
            ("stage".to_string(), "prod".to_string()),
            ("team".to_string(), "payments".to_string())
        ],
        "the rule reports the tags its create carried, got {listed:?}"
    );

    let tagged = xray
        .call_in_region(
            DEFAULT_REGION,
            "/TagResource",
            &json!({ "ResourceARN": arn, "Tags": [{ "Key": "owner", "Value": "sre" }] }),
        )
        .await;
    assert_eq!(
        tagged.status, 200,
        "tagging a stored rule must answer 200, got {tagged:?}"
    );
    let after = xray
        .call_in_region(
            DEFAULT_REGION,
            "/ListTagsForResource",
            &json!({ "ResourceARN": arn }),
        )
        .await;
    let keys: Vec<String> = after
        .get("Tags")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| tag["Key"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        keys.contains(&"owner".to_string()),
        "the added tag must be readable back, got {after:?}"
    );

    // And the ARN of a rule that is not stored names no taggable resource.
    let unknown = xray
        .call_in_region(
            DEFAULT_REGION,
            "/ListTagsForResource",
            &json!({ "ResourceARN": expected_rule_arn(DEFAULT_REGION, "never-created") }),
        )
        .await;
    assert_ne!(
        unknown.status, 200,
        "the ARN of a rule that was never created must not resolve, got {unknown:?}"
    );
}
