// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the bound Recycle Bin puts on how widely ONE resource tag
//! key and value pair may be re-used: "You can add the same tag key and value pair to a
//! maximum or five retention rules".
//! Source: smithy_query(query="doc", shape="CreateRuleRequest", member="ResourceTags")
//! and smithy_query(query="doc", shape="UpdateRuleRequest", member="ResourceTags").
//!
//! The rule asserted here: a request that would make a tag key and value pair the
//! selector of more retention rules than the quota allows is refused with HTTP 402
//! `ServiceQuotaExceededException` (Reason `SERVICE_QUOTA_EXCEEDED`) and changes
//! nothing — on BOTH operations that accept `ResourceTags`, `CreateRule` and
//! `UpdateRule`. A pair is identified by its key TOGETHER with its value, the rule being
//! updated never counts against itself, and the count is live: freeing a holder admits
//! one rule again.
//!
//! Each table row is a different FORM of pair — value present, value absent, the value
//! and key lengths at the bounds the model declares, a value carrying whitespace — and a
//! different resource type, so the rule is asserted over the class rather than over one
//! recorded tag pair. Every row drives the service through the real router
//! (`MultiServiceRouter::dispatch`), exactly as a signed SDK request would.

use std::time::Duration;

use serde_json::{json, Value};

use crate::retention_rule_lifecycle::{
    create_rule, delete_rule, get_rule, list_of, list_rules, retention, router, update_rule,
};
use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    MAX_RULES_PER_RESOURCE_TAG, RULE_STATUS_AVAILABLE, STATUS_TRANSITION,
};
use smithy_server_runtime::MultiServiceRouter;

/// `ResourceType` accepts exactly these values.
/// Source: smithy_query(query="shape", shape="ResourceType") — enum values.
const RESOURCE_TYPES: [&str; 3] = ["EBS_SNAPSHOT", "EBS_VOLUME", "EC2_IMAGE"];

/// `ResourceTagKey` is `^[\S\s]{1,128}$`.
/// Source: smithy_query(query="shape", shape="ResourceTag") — member `ResourceTagKey`.
const MAX_RESOURCE_TAG_KEY_LEN: usize = 128;

/// `ResourceTagValue` is `^[\S\s]{0,256}$`.
/// Source: smithy_query(query="shape", shape="ResourceTag") — member `ResourceTagValue`.
const MAX_RESOURCE_TAG_VALUE_LEN: usize = 256;

/// One resource tag pair under test: the key, and the value the request carries for it
/// (`None` when the request names no value, which the model allows).
#[derive(Debug, Clone)]
struct Pair {
    case: &'static str,
    key: String,
    value: Option<String>,
}

impl Pair {
    fn as_json(&self) -> Value {
        match &self.value {
            Some(value) => json!({ "ResourceTagKey": self.key, "ResourceTagValue": value }),
            None => json!({ "ResourceTagKey": self.key }),
        }
    }
}

/// The forms a pair can take, each one a row of every table below.
fn pairs() -> Vec<Pair> {
    vec![
        Pair {
            case: "a plain key and value pair",
            key: "retain".to_string(),
            value: Some("yes".to_string()),
        },
        Pair {
            case: "a pair whose request names no value",
            key: "cost-centre".to_string(),
            value: None,
        },
        Pair {
            case: "a pair with an empty value",
            key: "ledger".to_string(),
            value: Some(String::new()),
        },
        Pair {
            case: "a pair with a value at the published maximum length",
            key: "archive".to_string(),
            value: Some("v".repeat(MAX_RESOURCE_TAG_VALUE_LEN)),
        },
        Pair {
            case: "a pair with a key at the published maximum length",
            key: "k".repeat(MAX_RESOURCE_TAG_KEY_LEN),
            value: Some("yes".to_string()),
        },
        Pair {
            case: "a pair whose value carries whitespace and non-ASCII characters",
            key: "owner".to_string(),
            value: Some("équipe un".to_string()),
        },
    ]
}

/// A key derived from `key` by marking it with `suffix`, kept inside the published
/// maximum key length so the derived pair is still a request the model admits.
fn derived_key(key: &str, suffix: &str) -> String {
    let keep = MAX_RESOURCE_TAG_KEY_LEN - suffix.len();
    let mut derived: String = key.chars().take(keep).collect();
    derived.push_str(suffix);
    derived
}

/// Long enough past a rule's status transition span that `available` is owed.
fn past_the_transition() -> Duration {
    STATUS_TRANSITION + Duration::from_millis(200)
}

/// Create a rule of `resource_type` selecting on `tags`, asserting it was accepted, and
/// answer its identifier.
async fn create_selecting(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    tags: &[Value],
) -> String {
    let (status, body) = create_rule(
        router,
        json!({
            "ResourceType": resource_type,
            "RetentionPeriod": retention(7),
            "ResourceTags": tags,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateRule, got {status} / {body:?}"
    );
    body["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no Identifier: {body:?}"))
        .to_string()
}

/// How many rules `pair` currently selects, read back through `ListRules`.
async fn rules_selecting(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    pair: &Pair,
) -> usize {
    let (status, body) = list_rules(
        router,
        json!({ "ResourceType": resource_type, "ResourceTags": [pair.as_json()] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListRules, got {status} / {body:?}"
    );
    list_of(&body, "Rules").len()
}

/// Assert a response is the modelled quota refusal: HTTP 402, with the Reason the model
/// declares for `ServiceQuotaExceededException`.
fn assert_quota_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 402,
        "case {case:?}: expected HTTP 402, got {status} / {body:?}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("SERVICE_QUOTA_EXCEEDED"),
        "case {case:?}: expected Reason SERVICE_QUOTA_EXCEEDED, got {:?}",
        body["Reason"]
    );
    assert!(
        body["Message"].as_str().is_some_and(|m| !m.is_empty()),
        "case {case:?}: expected a non-empty Message, got {:?}",
        body["Message"]
    );
}

/// Saturate `pair`: create exactly the quota of rules selecting on it, and answer their
/// identifiers.
async fn saturate(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    pair: &Pair,
) -> Vec<String> {
    let mut holders = Vec::new();
    for nth in 0..MAX_RULES_PER_RESOURCE_TAG {
        let holder_case = format!("{case}: holder {nth} of the quota");
        holders.push(
            create_selecting(router, &holder_case, resource_type, &[pair.as_json()]).await,
        );
    }
    assert_eq!(
        rules_selecting(router, case, resource_type, pair).await,
        MAX_RULES_PER_RESOURCE_TAG,
        "case {case:?}: the quota of rules must be selecting on the pair after setup"
    );
    holders
}

#[tokio::test]
async fn create_rule_is_refused_once_a_resource_tag_pair_selects_its_quota_of_rules() {
    for (row, pair) in pairs().into_iter().enumerate() {
        // A fresh store per row, so one row's holders cannot be counted into another's.
        let router = router();
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let case = pair.case;
        saturate(&router, case, resource_type, &pair).await;

        // One rule past the quota is refused.
        let (status, body) = create_rule(
            &router,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(7),
                "ResourceTags": [pair.as_json()],
            }),
        )
        .await;
        assert_quota_refusal(&format!("{case}: one rule past the quota"), status, &body);
        // The refused create persisted nothing.
        assert_eq!(
            rules_selecting(&router, case, resource_type, &pair).await,
            MAX_RULES_PER_RESOURCE_TAG,
            "case {case:?}: a refused CreateRule must leave the pair selecting only the quota"
        );

        // The bound is per PAIR, not per key: the same key with a different value, and a
        // different key altogether, are both still free.
        let other_value = Pair {
            case,
            key: pair.key.clone(),
            value: Some("a-different-value".to_string()),
        };
        let other_key = Pair {
            case,
            key: derived_key(&pair.key, "-other"),
            value: pair.value.clone(),
        };
        for (what, free) in [
            ("the same key with another value", &other_value),
            ("another key with the same value", &other_key),
        ] {
            let free_case = format!("{case}: {what}");
            create_selecting(&router, &free_case, resource_type, &[free.as_json()]).await;
        }

        // And a Region-level rule — one selecting on no pair at all — is never bounded by
        // this quota.
        let (status, body) = create_rule(
            &router,
            json!({ "ResourceType": resource_type, "RetentionPeriod": retention(7) }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: a rule with no resource tags must be accepted, \
             got {status} / {body:?}"
        );
    }
}

#[tokio::test]
async fn update_rule_is_refused_when_it_would_oversubscribe_a_resource_tag_pair() {
    for (row, pair) in pairs().into_iter().enumerate() {
        let router = router();
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let case = pair.case;
        saturate(&router, case, resource_type, &pair).await;

        // A rule selecting on a pair of its own, which the update would move onto the
        // saturated one.
        let own = Pair {
            case,
            key: derived_key(&pair.key, "-own"),
            value: Some("mine".to_string()),
        };
        let identifier =
            create_selecting(&router, &format!("{case}: the rule to update"), resource_type, &[
                own.as_json(),
            ])
            .await;
        // Let the rule reach `available`, so a refused update that wrongly re-entered
        // `pending` would show.
        tokio::time::sleep(past_the_transition()).await;

        let (status, body) = update_rule(
            &router,
            &identifier,
            json!({ "ResourceTags": [pair.as_json()] }),
        )
        .await;
        assert_quota_refusal(&format!("{case}: update onto a saturated pair"), status, &body);

        // The refused update changed nothing a later caller sees.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRule, got {status} / {fetched:?}"
        );
        assert_eq!(
            fetched["ResourceTags"],
            json!([own.as_json()]),
            "case {case:?}: a refused UpdateRule must leave the stored ResourceTags alone, \
             got {:?}",
            fetched["ResourceTags"]
        );
        assert_eq!(
            fetched["Status"].as_str(),
            Some(RULE_STATUS_AVAILABLE),
            "case {case:?}: a refused UpdateRule must not move the rule's status, got {:?}",
            fetched["Status"]
        );
        assert_eq!(
            rules_selecting(&router, case, resource_type, &pair).await,
            MAX_RULES_PER_RESOURCE_TAG,
            "case {case:?}: a refused UpdateRule must leave the pair selecting only the quota"
        );

        // Every other member of the same rule still updates, and so does a move onto a
        // pair that is below the quota.
        let (status, updated) = update_rule(
            &router,
            &identifier,
            json!({ "Description": "still updatable", "RetentionPeriod": retention(30) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an update touching no resource tags must be accepted, \
             got {status} / {updated:?}"
        );
        let free = Pair {
            case,
            key: derived_key(&pair.key, "-free"),
            value: pair.value.clone(),
        };
        let (status, updated) = update_rule(
            &router,
            &identifier,
            json!({ "ResourceTags": [free.as_json()] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an update onto a pair below the quota must be accepted, \
             got {status} / {updated:?}"
        );
        assert_eq!(
            updated["ResourceTags"],
            json!([free.as_json()]),
            "case {case:?}: the accepted update must report the new ResourceTags, got {:?}",
            updated["ResourceTags"]
        );
    }
}

#[tokio::test]
async fn a_rule_never_counts_against_the_resource_tag_pairs_it_already_selects_on() {
    for (row, pair) in pairs().into_iter().enumerate() {
        let router = router();
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let case = pair.case;
        let holders = saturate(&router, case, resource_type, &pair).await;

        // One of the holders re-sends the very pair it already selects on, which leaves
        // the number of rules selecting on the pair exactly where it was.
        let holder = &holders[holders.len() - 1];
        let (status, updated) = update_rule(
            &router,
            holder,
            json!({ "ResourceTags": [pair.as_json()], "Description": "re-sent" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: re-sending a rule's own pair must be accepted, \
             got {status} / {updated:?}"
        );
        assert_eq!(
            updated["ResourceTags"],
            json!([pair.as_json()]),
            "case {case:?}: the rule must still select on its own pair, got {:?}",
            updated["ResourceTags"]
        );
        assert_eq!(
            rules_selecting(&router, case, resource_type, &pair).await,
            MAX_RULES_PER_RESOURCE_TAG,
            "case {case:?}: re-sending a rule's own pair must not change the holder count"
        );

        // A holder may also take the saturated pair ALONGSIDE a new one it does not hold
        // yet: only the pairs over the quota are refused.
        let extra = Pair {
            case,
            key: derived_key(&pair.key, "-extra"),
            value: Some("also".to_string()),
        };
        let (status, updated) = update_rule(
            &router,
            holder,
            json!({ "ResourceTags": [pair.as_json(), extra.as_json()] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a holder adding a free pair beside its own must be accepted, \
             got {status} / {updated:?}"
        );
    }
}

#[tokio::test]
async fn the_resource_tag_pair_count_is_live_so_freeing_a_holder_admits_one_rule() {
    for (row, pair) in pairs().into_iter().enumerate() {
        let router = router();
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let case = pair.case;
        let holders = saturate(&router, case, resource_type, &pair).await;

        // A sixth rule, parked on a pair of its own, cannot move onto the saturated one.
        let own = Pair {
            case,
            key: derived_key(&pair.key, "-parked"),
            value: Some("mine".to_string()),
        };
        let identifier = create_selecting(
            &router,
            &format!("{case}: the parked rule"),
            resource_type,
            &[own.as_json()],
        )
        .await;
        let (status, body) = update_rule(
            &router,
            &identifier,
            json!({ "ResourceTags": [pair.as_json()] }),
        )
        .await;
        assert_quota_refusal(&format!("{case}: before a holder is freed"), status, &body);

        // Free one holder; the same update is then served, and the pair is back at its
        // quota afterwards, so the next rule is refused again.
        let (status, _) = delete_rule(&router, &holders[0]).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected HTTP 204 from DeleteRule, got {status}"
        );
        let (status, updated) = update_rule(
            &router,
            &identifier,
            json!({ "ResourceTags": [pair.as_json()] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update into the freed slot must be accepted, \
             got {status} / {updated:?}"
        );
        assert_eq!(
            rules_selecting(&router, case, resource_type, &pair).await,
            MAX_RULES_PER_RESOURCE_TAG,
            "case {case:?}: the pair must be back at its quota of holders"
        );
        let (status, body) = create_rule(
            &router,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(7),
                "ResourceTags": [pair.as_json()],
            }),
        )
        .await;
        assert_quota_refusal(&format!("{case}: past the quota again"), status, &body);
    }
}

#[tokio::test]
async fn a_pair_a_request_names_twice_is_one_pair() {
    for (row, pair) in pairs().into_iter().enumerate() {
        let router = router();
        let resource_type = RESOURCE_TYPES[row % RESOURCE_TYPES.len()];
        let case = pair.case;
        // One holder short of the quota.
        for nth in 0..MAX_RULES_PER_RESOURCE_TAG - 1 {
            let holder_case = format!("{case}: holder {nth} below the quota");
            create_selecting(&router, &holder_case, resource_type, &[pair.as_json()]).await;
        }

        // A rule selects on a pair whether it names it once or twice, so a request
        // repeating the pair is the quota's last holder, not one past it.
        let identifier = create_selecting(
            &router,
            &format!("{case}: the pair named twice"),
            resource_type,
            &[pair.as_json(), pair.as_json()],
        )
        .await;
        assert_eq!(
            rules_selecting(&router, case, resource_type, &pair).await,
            MAX_RULES_PER_RESOURCE_TAG,
            "case {case:?}: a repeated pair must count as one holder"
        );
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRule, got {status} / {fetched:?}"
        );

        // And the next rule naming it — twice again — is over the quota.
        let (status, body) = create_rule(
            &router,
            json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(7),
                "ResourceTags": [pair.as_json(), pair.as_json()],
            }),
        )
        .await;
        assert_quota_refusal(&format!("{case}: the repeated pair past the quota"), status, &body);
    }
}
