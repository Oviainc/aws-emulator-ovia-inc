// SPDX-License-Identifier: CC0-1.0

//! What a `StartExperiment` `clientToken` is compared against, end to end over HTTP.
//!
//! RULE: an `@idempotencyToken` identifies the REQUEST it was first used with, and two
//! requests are the same request when they leave the same resource behind — so the
//! comparison is made over what the call PERSISTS, with the defaults the service fills in
//! and the optional members normalised as the store holds them, and not over the bytes the
//! request carried. A retry that spells out a default the first call left to the service,
//! or that sends an explicitly empty map where the first sent none, therefore REPLAYS the
//! experiment the token already started (HTTP 200), while a request that would have
//! started a DIFFERENT experiment is the parameter mismatch FIS refuses with
//! `ConflictException` (HTTP 409), having started nothing.
//!
//! The members `StartExperiment` normalises are the two it fills in or defaults:
//! `experimentOptions.actionsMode`, which resolves to [`RESOLVED_DEFAULT_ACTIONS_MODE`]
//! when the request names none, and `tags`, which an omitted and an explicitly empty map
//! leave as the same empty tag map.
//!
//! The rows are the CROSS PRODUCT of the spellings below — differing along whether the
//! option structure is carried at all, which admissible mode it names, and whether the tag
//! map is absent, empty, or carries one or two entries with which values — and every row's
//! expectation is computed from the normalisation itself rather than written out, so a
//! spelling pair is a replay exactly when the two starts would persist one experiment.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, get_experiment, send, start_experiment, Answer, FALLBACK_REGION,
    WAIT_DURATION,
};

/// The `experimentOptions.actionsMode` an experiment runs under when the request names
/// none: FIS runs the template's actions unless it is asked to skip them. `ActionsMode`
/// admits `run-all` and `skip-all` (`smithy_query(query="shape",shape="ActionsMode")`).
/// Spelled out here rather than imported from the crate, so a changed default fails this
/// suite.
const RESOLVED_DEFAULT_ACTIONS_MODE: &str = "run-all";

/// A role ARN the emulator's IAM store holds no role for: with no trust policy to read,
/// `StartExperiment` has no evidence the role withholds the grant and the start is
/// accepted, which is what every row here needs.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-token-normalization";

/// The `clientToken` both starts of every row carry.
const SHARED_TOKEN: &str = "normalised-start-token";

/// One SPELLING of the two members a start normalises before it persists them.
#[derive(Debug, Clone, Copy)]
struct Spelling {
    case: &'static str,
    /// The `experimentOptions.actionsMode` the request names, or `None` when the request
    /// carries no `experimentOptions` member at all.
    actions_mode: Option<&'static str>,
    /// The `tags` the request carries: `None` for an omitted map, `Some(&[])` for an
    /// explicitly empty one.
    tags: Option<&'static [(&'static str, &'static str)]>,
}

impl Spelling {
    /// The `StartExperiment` body this spelling sends. A member the spelling omits stays
    /// absent on the wire, which is the whole point of the row.
    fn body(&self, template_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("clientToken".to_string(), json!(SHARED_TOKEN));
        body.insert("experimentTemplateId".to_string(), json!(template_id));
        if let Some(actions_mode) = self.actions_mode {
            body.insert(
                "experimentOptions".to_string(),
                json!({ "actionsMode": actions_mode }),
            );
        }
        if let Some(tags) = self.tags {
            let carried: BTreeMap<&str, &str> = tags.iter().copied().collect();
            body.insert("tags".to_string(), json!(carried));
        }
        Value::Object(body)
    }

    /// What a start carrying this spelling PERSISTS of the two members it normalises: the
    /// resolved actions mode, and the tag map the experiment is tagged with.
    fn stored(&self) -> (&'static str, BTreeMap<&'static str, &'static str>) {
        (
            self.actions_mode.unwrap_or(RESOLVED_DEFAULT_ACTIONS_MODE),
            self.tags.unwrap_or_default().iter().copied().collect(),
        )
    }
}

/// The spellings the rule is asserted over, pairwise. See the suite docs for what they
/// span.
fn spellings() -> [Spelling; 9] {
    [
        Spelling {
            case: "no options and no tags",
            actions_mode: None,
            tags: None,
        },
        Spelling {
            case: "the default spelled out with an empty tag map",
            actions_mode: Some(RESOLVED_DEFAULT_ACTIONS_MODE),
            tags: Some(&[]),
        },
        Spelling {
            case: "the default spelled out with no tags",
            actions_mode: Some(RESOLVED_DEFAULT_ACTIONS_MODE),
            tags: None,
        },
        Spelling {
            case: "no options with an empty tag map",
            actions_mode: None,
            tags: Some(&[]),
        },
        Spelling {
            case: "skip-all with no tags",
            actions_mode: Some("skip-all"),
            tags: None,
        },
        Spelling {
            case: "skip-all with an empty tag map",
            actions_mode: Some("skip-all"),
            tags: Some(&[]),
        },
        Spelling {
            case: "no options with one tag",
            actions_mode: None,
            tags: Some(&[("Run", "first")]),
        },
        Spelling {
            case: "the default spelled out with one tag",
            actions_mode: Some(RESOLVED_DEFAULT_ACTIONS_MODE),
            tags: Some(&[("Run", "first")]),
        },
        Spelling {
            case: "no options with two tags",
            actions_mode: None,
            tags: Some(&[("Phase", "one"), ("Run", "first")]),
        },
    ]
}

/// The `message` an error body carries.
fn message(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// The `ConflictException` message a token carried by a different request earns.
fn token_conflict_message() -> String {
    format!("clientToken {SHARED_TOKEN} is not assigned to this resource.")
}

/// The `experimentOptions.actionsMode` a reported experiment carries.
fn reported_actions_mode(experiment: &Value) -> &str {
    experiment["experimentOptions"]["actionsMode"]
        .as_str()
        .unwrap_or_default()
}

/// The tags a reported experiment carries. An absent member and a serialized empty object
/// both say the experiment is tagged with nothing.
fn reported_tags(experiment: &Value) -> BTreeMap<String, String> {
    experiment["tags"]
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The borrowed tag map of a spelling, as the reported one compares.
fn owned(tags: &BTreeMap<&str, &str>) -> BTreeMap<String, String> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// How many experiments the store holds.
async fn experiment_count(router: &MultiServiceRouter) -> usize {
    let listed = send(router, "GET", "/experiments?maxResults=100", None, FALLBACK_REGION).await;
    assert_eq!(
        listed.status, 200,
        "expected the experiments to be listed, got {} with {:?}",
        listed.status, listed.body
    );
    listed
        .member("experiments")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// Create the template every start of a row runs, and report its minted id.
async fn template(router: &MultiServiceRouter, case: &str) -> String {
    let answer = create_template(
        router,
        json!({
            "clientToken": format!("template-{}", case.replace(' ', "-")),
            "description": format!("start experiment token normalization: {case}"),
            "roleArn": ROLE_ARN,
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            // `aws:fis:wait` takes a required `duration`, an ISO 8601 duration within the
            // documented 1 to 720 minutes.
            "actions": {
                "wait": {
                    "actionId": "aws:fis:wait",
                    "parameters": { "duration": WAIT_DURATION }
                }
            }
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.id in {:?}", answer.body))
        .to_string()
}

/// RULE, over every pair of spellings: a retry is a replay exactly when the two starts
/// would persist the same experiment, and a mismatch is refused having started nothing.
#[tokio::test]
async fn a_retry_the_start_would_persist_identically_replays_the_experiment() {
    let spellings = spellings();
    for first in spellings {
        for second in spellings {
            let case = format!("{} then {}", first.case, second.case);
            // A fresh emulator per row: the token of one row says nothing about another's.
            let router = emulator();
            let template_id = template(&router, &case).await;

            let started = start_experiment(&router, first.body(&template_id)).await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: expected HTTP 200 from the first start, got {} with {:?}",
                started.status, started.body
            );
            let experiment = started.member("experiment").clone();
            let id = experiment["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no experiment.id in {experiment:?}"))
                .to_string();

            // What the first start PERSISTED of the two normalised members, which is what
            // the token's request is afterwards compared against.
            let (stored_mode, stored_tags) = first.stored();
            assert_eq!(
                reported_actions_mode(&experiment),
                stored_mode,
                "case {case:?}: expected the resolved actions mode, got {experiment:?}"
            );
            assert_eq!(
                reported_tags(&experiment),
                owned(&stored_tags),
                "case {case:?}: expected the stored tag map, got {experiment:?}"
            );

            let retried = start_experiment(&router, second.body(&template_id)).await;
            if first.stored() == second.stored() {
                assert_eq!(
                    retried.status, 200,
                    "case {case:?}: two starts that persist one experiment are one \
                     request, so the retry replays it; got {} with {:?}",
                    retried.status, retried.body
                );
                let replayed = retried.member("experiment").clone();
                assert_eq!(
                    replayed["id"].as_str().unwrap_or_default(),
                    id.as_str(),
                    "case {case:?}: expected the experiment the token started, got \
                     {replayed:?}"
                );
                assert_eq!(
                    reported_actions_mode(&replayed),
                    stored_mode,
                    "case {case:?}: expected the replay to report the stored actions \
                     mode, got {replayed:?}"
                );
                assert_eq!(
                    reported_tags(&replayed),
                    owned(&stored_tags),
                    "case {case:?}: expected the replay to report the stored tag map, got \
                     {replayed:?}"
                );
            } else {
                assert_eq!(
                    retried.status, 409,
                    "case {case:?}: a token carried by a request that would start another \
                     experiment is refused; got {} with {:?}",
                    retried.status, retried.body
                );
                assert_eq!(
                    retried.error_code(),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {:?}",
                    retried.body
                );
                assert_eq!(
                    message(&retried),
                    token_conflict_message(),
                    "case {case:?}: expected the token-mismatch message, got {:?}",
                    retried.body
                );
                assert!(
                    retried.member("experiment").is_null(),
                    "case {case:?}: a conflict carries no experiment, got {:?}",
                    retried.body
                );
            }

            // Whatever the answer: the store holds exactly the one experiment the token
            // started, still carrying what the FIRST start persisted.
            let read = get_experiment(&router, &id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected the started experiment to be readable, got {} \
                 with {:?}",
                read.status, read.body
            );
            let read_experiment = read.member("experiment").clone();
            assert_eq!(
                reported_actions_mode(&read_experiment),
                stored_mode,
                "case {case:?}: expected the stored actions mode to be unchanged, got \
                 {read_experiment:?}"
            );
            assert_eq!(
                reported_tags(&read_experiment),
                owned(&stored_tags),
                "case {case:?}: expected the stored tag map to be unchanged, got \
                 {read_experiment:?}"
            );
            assert_eq!(
                experiment_count(&router).await,
                1,
                "case {case:?}: a replayed or refused retry starts no second experiment"
            );
        }
    }
}
