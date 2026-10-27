// SPDX-License-Identifier: CC0-1.0

//! How `PutConformancePack` WORDS the refusal of a pack whose deployment workflow is
//! still in flight, on the wire.
//!
//! The rule pinned here: the `ResourceInUseException` a conformance pack operation
//! answers for a pack whose workflow has not settled NAMES that pack — the fixed
//! sentence plus the conformance pack name the request carried — so a caller deploying
//! several packs can tell which one is busy from the refusal alone. The message is
//! built from the request, so it is never one constant sentence for every pack in the
//! account.
//!
//! The table's rows differ along the one variable part the rule generalises over: the
//! form and the length of the conformance pack name, across the span
//! `ConformancePackName` allows (`smithy_query(query="operation",
//! operation="PutConformancePack")`: `@length(min: 1, max: 256)`,
//! `pattern "^[a-zA-Z][-a-zA-Z0-9]*$"`) — one character, a plain word, hyphens and
//! digits, mixed case, and a name at the length maximum.
//!
//! The positive controls the message assertion used to mask are pinned beside it: the
//! deployment the refusal left in flight still reaches `CREATE_COMPLETE`, and the rule
//! declared by the REFUSED template is never created.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — reusing the
//! `Emulator` harness `config_setup_and_rule_loop` defines, the `refusal` helper
//! `config_connectors` shares, and the template helpers `conformance_packs` shares.

use serde_json::json;
use service_com_amazonaws_configservice_starlingdoveservice::state;

use crate::config_connectors::refusal;
use crate::config_setup_and_rule_loop::Emulator;
use crate::conformance_packs::{pack_from_body, past_the_deployment, rule_template};

/// The rule name the ACCEPTED template declares.
const DEPLOYED_RULE: &str = "the-deployed-rule";

/// The rule name the REFUSED template declares, which no deployment ever creates.
const REFUSED_RULE: &str = "the-refused-rule";

/// The message `PutConformancePack` must answer for a pack named `name` whose workflow
/// is in flight, written out here rather than read from the service's own helper so this
/// suite PINS the wording instead of restating it.
fn in_use_message(name: &str) -> String {
    format!(
        "A workflow is currently in progress for the Conformance Pack: {name}. Please try later."
    )
}

/// Every conformance pack name the table drives, with the case label that names it.
fn pack_names() -> Vec<(&'static str, String)> {
    vec![
        ("one character, the length minimum", "P".to_string()),
        ("a plain word", "DeploymentPack".to_string()),
        ("hyphens and digits", "config-pack-09".to_string()),
        ("mixed case", "SecurityBaselineV2".to_string()),
        // 256 characters: the leading letter the pattern requires plus 255 more.
        ("the length maximum", format!("P{}", "a".repeat(255))),
    ]
}

/// The names of the Config rules this account holds.
async fn rule_names(emulator: &Emulator) -> Vec<String> {
    let described = emulator.ok("DescribeConfigRules", &json!({})).await;
    described["ConfigRules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|rule| rule["ConfigRuleName"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn the_in_use_refusal_names_the_pack_whose_workflow_is_in_progress() {
    for (case, name) in pack_names() {
        let emulator = Emulator::new();
        emulator
            .ok(
                "PutConformancePack",
                &pack_from_body(&name, &rule_template(DEPLOYED_RULE)),
            )
            .await;

        let (status, code, message) = refusal(
            &emulator,
            "PutConformancePack",
            &pack_from_body(&name, &rule_template(REFUSED_RULE)),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceInUseException"),
            "case {case:?}: a put for a pack whose deployment is in flight is refused 400 \
             ResourceInUseException, got {status} {code} {message:?}"
        );
        let expected = in_use_message(&name);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn the_in_use_refusal_is_not_one_constant_sentence_for_every_pack() {
    // The rule's point: the refusal DISTINGUISHES the busy pack. Two packs deployed in
    // the same account, each refused while its own deployment is in flight, answer two
    // different messages, each naming its own pack.
    let emulator = Emulator::new();
    let mut messages: Vec<String> = Vec::new();
    for (case, name) in pack_names() {
        emulator
            .ok(
                "PutConformancePack",
                &pack_from_body(&name, &rule_template(DEPLOYED_RULE)),
            )
            .await;
        let (_, _, message) = refusal(
            &emulator,
            "PutConformancePack",
            &pack_from_body(&name, &rule_template(REFUSED_RULE)),
        )
        .await;
        assert!(
            message.contains(&name),
            "case {case:?}: the refusal names the pack the request named, got {message:?}"
        );
        messages.push(message);
    }
    let mut distinct = messages.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        messages.len(),
        "each pack's refusal is its own message, got {messages:?}"
    );
}

#[tokio::test]
async fn the_refused_put_changes_nothing_the_deployment_in_flight_started() {
    // The positive controls the message assertion masked: the deployment the refusal
    // left in flight still reaches CREATE_COMPLETE, with the rule its OWN template
    // declared, and the refused template's rule is never created.
    for (case, name) in pack_names() {
        let emulator = Emulator::new();
        emulator
            .ok(
                "PutConformancePack",
                &pack_from_body(&name, &rule_template(DEPLOYED_RULE)),
            )
            .await;
        let (status, code, _) = refusal(
            &emulator,
            "PutConformancePack",
            &pack_from_body(&name, &rule_template(REFUSED_RULE)),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceInUseException"),
            "case {case:?}: the second put is refused, got {status} {code}"
        );

        tokio::time::sleep(past_the_deployment()).await;
        let body = emulator
            .ok(
                "DescribeConformancePackStatus",
                &json!({ "ConformancePackNames": [name] }),
            )
            .await;
        assert_eq!(
            body["ConformancePackStatusDetails"][0]["ConformancePackState"].as_str(),
            Some(state::CONFORMANCE_PACK_STATE_CREATE_COMPLETE),
            "case {case:?}: the deployment the refusal left in flight still completes, got {body}"
        );

        let rules = rule_names(&emulator).await;
        assert!(
            rules.iter().any(|rule| rule.starts_with(DEPLOYED_RULE)),
            "case {case:?}: the accepted template's rule is deployed, got {rules:?}"
        );
        assert!(
            !rules.iter().any(|rule| rule.starts_with(REFUSED_RULE)),
            "case {case:?}: the refused template's rule is never created, got {rules:?}"
        );
    }
}
