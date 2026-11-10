// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the one constraint class the validating walk in front of a
//! handler cannot report: a MAP whose KEY shape is an enumerated type.
//!
//! The rule: `LogPublishingOptions` is keyed by `LogType`
//! (`smithy_query(query="shape",shape="LogPublishingOptions,LogType")`), so a key outside
//! that value set is a constraint the MODEL declares — but the walk treats a map's key as
//! a path SEGMENT rather than as a constrained leaf, so nothing ahead of the handler
//! checks it. Being an INPUT constraint it is evaluated BEFORE the domain the call names
//! is resolved: the refusal names the member, the map the request carried and the key set
//! a log type has to come from, and never reports the domain as missing.
//!
//! The rows span what the rule generalises over: which key is outside the set, how many
//! entries the map carries, which members of an entry's structure the request set, and
//! whether the domain the call names exists. The control rows are the other half of the
//! rule — every key the model DOES declare is accepted and read back, and a map whose keys
//! are all inside the set leaves the domain lookup to answer for itself.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::LOG_TYPE_VALUES;

use super::domain_configuration_rules::assert_refusal;
use super::domain_lifecycle::{
    baseline_create, create_baseline, error_code, log_group_arn, Emulator, SETTLE_WAIT,
};
use super::service_software_update::message;

/// The CloudWatch Logs log group the accepted rows publish to. An enabled option's
/// destination has to be a group that EXISTS, so the suite creates it through that
/// service's own `CreateLogGroup` ([`Emulator::created_log_group`]) before it is named.
const LOG_GROUP_NAME: &str = "/aws/opensearch/probe";

/// The destination the REFUSED rows carry. Those rows are refused on the map's KEY before
/// any destination is resolved, so this value is echoed back into the clause rather than
/// looked up — which is why it is a literal the expectations below quote.
const LOG_GROUP: &str = "arn:aws:logs:us-east-1:000000000000:log-group:/aws/opensearch/probe";

/// The accepted key set as a refusal lists it: the order the front door's own hash table
/// over the admissible values is walked, which is neither the model's declaration order
/// nor alphabetical. `TASK_DETAILS_LOGS` is in it because this surface's own recorded
/// refusal lists it beside the four the model declares (the legacy 2015-01-01 surface's
/// recorded refusal lists only those four).
const LOG_TYPE_SET: &str =
    "[INDEX_SLOW_LOGS, TASK_DETAILS_LOGS, ES_APPLICATION_LOGS, AUDIT_LOGS, SEARCH_SLOW_LOGS]";

/// One row: the `LogPublishingOptions` a request carries, and the map as the refusal
/// quotes it back.
struct Case {
    case: &'static str,
    submitted: Value,
    quoted: &'static str,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a log type the model does not declare",
            submitted: json!({
                "ACCESS_LOGS": {"CloudWatchLogsLogGroupArn": LOG_GROUP, "Enabled": true}
            }),
            quoted: "{ACCESS_LOGS=LogPublishingOption(cloudWatchLogsLogGroupArn=arn:aws:logs:\
                     us-east-1:000000000000:log-group:/aws/opensearch/probe, enabled=true)}",
        },
        Case {
            case: "a modelled log type in the wrong case",
            submitted: json!({"audit_logs": {"Enabled": true}}),
            quoted: "{audit_logs=LogPublishingOption(cloudWatchLogsLogGroupArn=null, \
                     enabled=true)}",
        },
        Case {
            case: "a modelled log type with a typo",
            submitted: json!({"INDEX_SLOW_LOG": {"Enabled": false}}),
            quoted: "{INDEX_SLOW_LOG=LogPublishingOption(cloudWatchLogsLogGroupArn=null, \
                     enabled=false)}",
        },
        Case {
            case: "an empty key",
            submitted: json!({"": {"CloudWatchLogsLogGroupArn": LOG_GROUP, "Enabled": true}}),
            quoted: "{=LogPublishingOption(cloudWatchLogsLogGroupArn=arn:aws:logs:us-east-1:\
                     000000000000:log-group:/aws/opensearch/probe, enabled=true)}",
        },
        Case {
            case: "one key inside the set and one outside it",
            submitted: json!({
                "AUDIT_LOGS": {"CloudWatchLogsLogGroupArn": LOG_GROUP, "Enabled": true},
                "ACCESS_LOGS": {"Enabled": false}
            }),
            quoted: "{ACCESS_LOGS=LogPublishingOption(cloudWatchLogsLogGroupArn=null, \
                     enabled=false), AUDIT_LOGS=LogPublishingOption(cloudWatchLogsLogGroupArn=\
                     arn:aws:logs:us-east-1:000000000000:log-group:/aws/opensearch/probe, \
                     enabled=true)}",
        },
    ]
}

/// The whole message a map-key refusal states for `quoted`.
fn expected(quoted: &str) -> String {
    format!(
        "1 validation error detected: Value '{quoted}' at 'logPublishingOptions' failed to \
         satisfy constraint: Map keys must satisfy constraint: [Member must satisfy enum value \
         set: {LOG_TYPE_SET}]"
    )
}

/// RULE: a `LogPublishingOptions` key outside the set this surface admits is refused as
/// invalid input, on the create and on the update — and on the update BEFORE the domain
/// the call names is resolved, so a call naming a domain that does not exist is still the
/// input refusal.
#[tokio::test]
async fn a_log_type_outside_the_modelled_set_is_refused_before_the_domain_is_resolved() {
    let emulator = Emulator::new();

    for Case {
        case,
        submitted,
        quoted,
    } in cases()
    {
        let update = emulator
            .update_config("no-such-domain", json!({"LogPublishingOptions": submitted}))
            .await;
        let row = format!("UpdateDomainConfig on an absent domain: {case}");
        assert_refusal(&update, "ValidationException", &row);
        assert_eq!(
            message(&update, &row),
            expected(quoted),
            "case {row}: expected {:?}, got {:?}",
            expected(quoted),
            message(&update, &row)
        );

        let mut body = baseline_create("log-key-probe");
        body["LogPublishingOptions"] = submitted;
        let created = emulator.create(body).await;
        let row = format!("CreateDomain: {case}");
        assert_refusal(&created, "ValidationException", &row);
        assert_eq!(
            message(&created, &row),
            expected(quoted),
            "case {row}: expected {:?}, got {:?}",
            expected(quoted),
            message(&created, &row)
        );
    }

    // No refused create stored a domain: the refusal ran before the store was reached.
    // Asserted on the refusal CLASS rather than on its HTTP status, which is a mapping of
    // its own and not what this rule is about.
    let read = emulator.describe("log-key-probe").await;
    assert_eq!(
        error_code(&read),
        "ResourceNotFoundException",
        "a refused create must store no domain, got {} with {:?}",
        read.status,
        read.body
    );
}

/// The other half of the rule: every key this surface admits is accepted and read back,
/// and a map whose keys are all inside the set leaves the domain lookup to answer.
#[tokio::test]
async fn every_modelled_log_type_is_accepted_and_the_lookup_still_answers() {
    let emulator = Emulator::with_log_groups();
    emulator.created_log_group(LOG_GROUP_NAME).await;
    let log_group = log_group_arn(LOG_GROUP_NAME);
    let name = "log-key-accepted";
    create_baseline(&emulator, name).await;
    tokio::time::sleep(SETTLE_WAIT).await;

    // The key shape's whole value set, read off the generated constant rather than
    // listed here, so a value the model gains is exercised too — plus the one value the
    // live surface admits that the model has not caught up with.
    for log_type in LOG_TYPE_VALUES
        .iter()
        .copied()
        .chain(["TASK_DETAILS_LOGS"])
    {
        let answer = emulator
            .update_config(
                name,
                json!({
                    "LogPublishingOptions": {
                        log_type: {"CloudWatchLogsLogGroupArn": log_group, "Enabled": true}
                    }
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {log_type}: an admitted log type must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        let read = emulator.describe_config(name).await;
        assert_eq!(
            read.body["DomainConfig"]["LogPublishingOptions"]["Options"][log_type]["Enabled"],
            json!(true),
            "case {log_type}: the stored option must read back, got {:?}",
            read.body["DomainConfig"]["LogPublishingOptions"]
        );
        tokio::time::sleep(SETTLE_WAIT).await;
    }

    let absent = emulator
        .update_config(
            "no-such-domain",
            json!({"LogPublishingOptions": {"AUDIT_LOGS": {"Enabled": false}}}),
        )
        .await;
    assert_eq!(
        error_code(&absent),
        "ResourceNotFoundException",
        "a map inside the key set on an absent domain must answer the domain's own refusal, \
         got {} / {:?} with {:?}",
        absent.status,
        error_code(&absent),
        absent.body
    );
}
