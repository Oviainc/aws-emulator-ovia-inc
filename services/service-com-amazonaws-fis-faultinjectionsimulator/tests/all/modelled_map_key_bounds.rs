// SPDX-License-Identifier: CC0-1.0

//! The modelled bounds of a MAP KEY, end to end over HTTP.
//!
//! The rule under test: a key of a map a request carries is judged by the modelled bounds
//! of that map's KEY target, exactly as a value is judged by its VALUE target — so a key
//! outside them is refused with 400 `ValidationException` and nothing is persisted, and a
//! key within them is stored and reported back by the resource's own reader. The boundary
//! validator reaches a map's value but not its key, so every operation of this service
//! that persists a map has to apply those bounds itself, and all of them have to agree
//! about what a key may be: the key `TagResource` refuses is the key
//! `CreateExperimentTemplate` and `StartExperiment` refuse, and a key none of them refuses
//! round-trips through every reader.
//!
//! The rows span what the rule generalises over:
//!
//! * WHICH map the key belongs to — the caller tags of a template, of an experiment and of
//!   a tag call, a template's target and action names, the `resourceTags` a target
//!   identifies resources by, and a target's and an action's parameter names — reached
//!   through the three operations that take a tag map plus the create and the update of a
//!   template;
//! * the key's SHAPE and SIZE: one character, the published ceiling of that map's key
//!   target, one character past it, empty, whitespace alone, whitespace inside, and a
//!   non-ASCII key at and one character past the ceiling (a Smithy `@length` counts
//!   CHARACTERS, so a multi-byte key is judged by how many it carries).
//!
//! Whether a row is admitted is COMPUTED from the published bounds of the map's key target
//! rather than written down per row ([`Site::max_length`], [`Site::admits_whitespace`],
//! [`admits`]), so one row's expectation cannot drift from the rule the others assert.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, send, token, Answer, FALLBACK_REGION, WAIT_DURATION,
};

/// A well-formed IAM role ARN for every template this suite creates. The emulator's IAM
/// store holds no role for it, which is what lets `StartExperiment` accept it.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-map-key-bounds";

/// `TagKey`'s published ceiling: `TagMap`'s key target is `@length(min: 0, max: 128)` with
/// `pattern ^[\s\S]+$`
/// (`smithy_query(query="shape", shape="TagMap", …)`), spelled out here rather than
/// imported from the crate so a wrong bound in `src/state.rs` fails this suite.
const TAG_KEY_CEILING: usize = 128;

/// The published ceiling of every OTHER map key of this service's experiment-template
/// surface: `ExperimentTemplateTargetName`, `ExperimentTemplateActionName`,
/// `ExperimentTemplateTargetParameterName` and `ExperimentTemplateActionParameterName` are
/// each `@length(min: 0, max: 64)` with `pattern ^[\S]+$`
/// (`smithy_query(query="shape", shape="CreateExperimentTemplateTargetInputMap,CreateExperimentTemplateActionInputMap,ExperimentTemplateTargetParameterMap,ExperimentTemplateActionParameterMap", …)`).
const NAME_CEILING: usize = 64;

/// A resource type the published catalog holds, requiring no target parameter of its own —
/// so a row about a PARAMETER name is about that name and nothing else.
const RESOURCE_TYPE: &str = "aws:ec2:instance";

/// Which map a row puts its key in, and through which call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Site {
    /// `CreateExperimentTemplate`'s `tags`.
    TemplateTags,
    /// `StartExperiment`'s `tags`.
    ExperimentTags,
    /// `TagResource`'s `tags` — the one path that already applied the bounds.
    AppliedTags,
    /// The key of a template's `targets` map: the target's NAME.
    TargetName,
    /// The key of a template's `actions` map: the action's NAME.
    ActionName,
    /// The `resourceTags` one target identifies its resources by — a `TagMap` too.
    TargetResourceTags,
    /// The key of one target's `parameters` map.
    TargetParameterName,
    /// The key of one action's `parameters` map.
    ActionParameterName,
    /// The key of the `actions` map an UPDATE carries, so the sequence that judges a
    /// created template judges an updated one alike.
    UpdatedActionName,
}

impl Site {
    fn as_str(self) -> &'static str {
        match self {
            Self::TemplateTags => "CreateExperimentTemplate.tags",
            Self::ExperimentTags => "StartExperiment.tags",
            Self::AppliedTags => "TagResource.tags",
            Self::TargetName => "CreateExperimentTemplate.targets",
            Self::ActionName => "CreateExperimentTemplate.actions",
            Self::TargetResourceTags => "CreateExperimentTemplate.targets.resourceTags",
            Self::TargetParameterName => "CreateExperimentTemplate.targets.parameters",
            Self::ActionParameterName => "CreateExperimentTemplate.actions.parameters",
            Self::UpdatedActionName => "UpdateExperimentTemplate.actions",
        }
    }

    /// The published ceiling of this map's key target.
    fn max_length(self) -> usize {
        match self {
            Self::TemplateTags
            | Self::ExperimentTags
            | Self::AppliedTags
            | Self::TargetResourceTags => TAG_KEY_CEILING,
            Self::TargetName
            | Self::ActionName
            | Self::TargetParameterName
            | Self::ActionParameterName
            | Self::UpdatedActionName => NAME_CEILING,
        }
    }

    /// Whether this map's key target admits whitespace: a `TagKey`'s `^[\s\S]+$` matches
    /// any character, while every name key is `^[\S]+$` and carries none.
    fn admits_whitespace(self) -> bool {
        self.max_length() == TAG_KEY_CEILING
    }

    /// The member a refusal has to name, so a caller can tell WHICH map it was about.
    fn member_names(self) -> &'static str {
        match self {
            Self::TemplateTags | Self::ExperimentTags | Self::AppliedTags => "tags",
            Self::TargetName => "targets",
            Self::ActionName | Self::UpdatedActionName => "actions",
            Self::TargetResourceTags => "resourceTags",
            Self::TargetParameterName | Self::ActionParameterName => "parameters",
        }
    }

    /// The value a row's key carries, when that map's value is a string.
    fn stored_value(self) -> Option<&'static str> {
        match self {
            Self::TemplateTags
            | Self::ExperimentTags
            | Self::AppliedTags
            | Self::TargetResourceTags => Some("x"),
            Self::TargetParameterName | Self::ActionParameterName => Some("v"),
            Self::TargetName | Self::ActionName | Self::UpdatedActionName => None,
        }
    }
}

/// Every site, so each table runs over all of them.
fn sites() -> Vec<Site> {
    vec![
        Site::TemplateTags,
        Site::ExperimentTags,
        Site::AppliedTags,
        Site::TargetName,
        Site::ActionName,
        Site::TargetResourceTags,
        Site::TargetParameterName,
        Site::ActionParameterName,
        Site::UpdatedActionName,
    ]
}

/// One key shape, built against the ceiling of whichever map's key target it is put in.
struct Shape {
    case: &'static str,
    key: fn(usize) -> String,
}

/// The key shapes every site is driven with. `ceiling` is that site's published ceiling,
/// so one shape means the same thing wherever it is used.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            case: "one character",
            key: |_| "k".to_string(),
        },
        Shape {
            case: "a plain key of several characters",
            key: |_| "Owner".to_string(),
        },
        Shape {
            case: "at the published ceiling",
            key: |ceiling| "k".repeat(ceiling),
        },
        Shape {
            case: "one character past the published ceiling",
            key: |ceiling| "k".repeat(ceiling + 1),
        },
        Shape {
            case: "empty",
            key: |_| String::new(),
        },
        Shape {
            case: "whitespace alone",
            key: |_| " ".to_string(),
        },
        Shape {
            case: "whitespace inside",
            key: |_| "two words".to_string(),
        },
        Shape {
            case: "a trailing space",
            key: |_| "trailing ".to_string(),
        },
        Shape {
            // A Smithy `@length` counts characters, so this is at the ceiling even though
            // it is three times as many bytes.
            case: "non-ASCII at the published ceiling",
            key: |ceiling| "環".repeat(ceiling),
        },
        Shape {
            case: "non-ASCII one character past the published ceiling",
            key: |ceiling| "環".repeat(ceiling + 1),
        },
    ]
}

/// The rule itself: `key` is admissible in `site`'s map exactly when it is within that
/// map's key target's published `@length` and satisfies its published pattern.
fn admits(site: Site, key: &str) -> bool {
    let length = key.chars().count();
    (1..=site.max_length()).contains(&length)
        && (site.admits_whitespace() || !key.chars().any(char::is_whitespace))
}

/// A one-entry JSON object, built without the `json!` macro so the key can be any string.
fn one_entry(key: &str, value: Value) -> Value {
    let mut map = Map::new();
    map.insert(key.to_string(), value);
    Value::Object(map)
}

/// The `aws:fis:wait` action every template in this suite declares, with `extra` as an
/// additional parameter name when a row is about one.
fn wait_action(extra: Option<&str>) -> Value {
    let mut parameters = Map::new();
    parameters.insert("duration".to_string(), json!(WAIT_DURATION));
    if let Some(extra) = extra {
        parameters.insert(extra.to_string(), json!("v"));
    }
    json!({ "actionId": "aws:fis:wait", "parameters": Value::Object(parameters) })
}

/// One target of the published [`RESOURCE_TYPE`], identifying its resources by tags.
fn target(resource_tags: Value, parameters: Option<Value>) -> Value {
    let mut target = json!({
        "resourceType": RESOURCE_TYPE,
        "selectionMode": "ALL",
        "resourceTags": resource_tags,
    });
    if let Some(parameters) = parameters {
        target["parameters"] = parameters;
    }
    target
}

/// The `@httpLabel` form of an ARN: `resourceArn` is a NON-greedy label, so a client
/// percent-encodes the `:` and `/` it carries before putting it in the path.
fn label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// The create body for `site`, carrying `key` wherever that site puts it. A site that is
/// not about the create gets a plain admissible template.
fn create_body(site: Site, case: &str, key: &str) -> Value {
    let mut body = json!({
        "clientToken": token(&slug(case)),
        "description": "the map-key-bounds suite's template",
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": { "wait": wait_action(None) },
    });
    match site {
        Site::TemplateTags => body["tags"] = one_entry(key, json!("x")),
        Site::TargetName => {
            body["targets"] = one_entry(key, target(json!({ "Stage": "test" }), None));
        }
        Site::ActionName => body["actions"] = one_entry(key, wait_action(None)),
        Site::TargetResourceTags => {
            body["targets"] = json!({ "instances": target(one_entry(key, json!("x")), None) });
        }
        Site::TargetParameterName => {
            body["targets"] = json!({
                "instances": target(
                    json!({ "Stage": "test" }),
                    Some(one_entry(key, json!("v"))),
                )
            });
        }
        Site::ActionParameterName => {
            body["actions"] = json!({ "wait": wait_action(Some(key)) });
        }
        Site::ExperimentTags | Site::AppliedTags | Site::UpdatedActionName => {}
    }
    body
}

/// The map of the reported record that has to carry a row's key.
fn carrier(site: Site, record: &Value) -> &Value {
    match site {
        Site::TemplateTags | Site::ExperimentTags | Site::AppliedTags => &record["tags"],
        Site::TargetName => &record["targets"],
        Site::ActionName | Site::UpdatedActionName => &record["actions"],
        Site::TargetResourceTags => &record["targets"]["instances"]["resourceTags"],
        Site::TargetParameterName => &record["targets"]["instances"]["parameters"],
        Site::ActionParameterName => &record["actions"]["wait"]["parameters"],
    }
}

/// A `clientToken` (`pattern ^[\S]+$`) derived from a case name.
fn slug(case: &str) -> String {
    case.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The template `id` names, as `GetExperimentTemplate` reports it.
async fn read_template(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let answer = send(
        router,
        "GET",
        &format!("/experimentTemplates/{id}"),
        None,
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experimentTemplate").clone()
}

/// The experiment `id` names, as `GetExperiment` reports it.
async fn read_experiment(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let answer = send(
        router,
        "GET",
        &format!("/experiments/{id}"),
        None,
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetExperiment, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experiment").clone()
}

/// How many records a list reports under `member`.
async fn listed(router: &MultiServiceRouter, case: &str, uri: &str, member: &str) -> usize {
    let answer = send(router, "GET", uri, None, FALLBACK_REGION).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GET {uri}, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .member(member)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {member} list in {:?}", answer.body))
        .len()
}

/// The id a create or a start reported.
fn minted_id(case: &str, answer: &Answer, member: &str) -> String {
    answer.member(member)["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no {member}.id in {:?}", answer.body))
        .to_string()
}

/// Assert `answer` is the 400 `ValidationException` an out-of-bounds key earns, naming the
/// map it was carried in.
fn assert_refused(site: Site, case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 for a key outside {}'s bounds, got {} with {:?}",
        site.as_str(),
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    let message = answer.body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(site.member_names()),
        "case {case:?}: expected the refusal to name {:?}, got {message:?}",
        site.member_names()
    );
}

/// Assert the stored `record` carries `key` with the value that site's map holds.
fn assert_carries(site: Site, case: &str, key: &str, record: &Value) {
    let carrier = carrier(site, record);
    let entry = carrier.get(key);
    assert!(
        entry.is_some(),
        "case {case:?}: expected the stored {} to carry the key, got {carrier:?}",
        site.as_str()
    );
    if let Some(expected) = site.stored_value() {
        assert_eq!(
            entry.and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected the stored key to carry {expected:?}, got {entry:?}"
        );
    }
}

/// Drive one row: put `key` in `site`'s map and assert the rule — admitted and read back,
/// or refused with nothing persisted.
async fn run(site: Site, case: &str, key: &str) {
    // One emulator per row, so "nothing was persisted" is about this row's call alone.
    let router = emulator();
    let admitted = admits(site, key);

    match site {
        Site::TemplateTags
        | Site::TargetName
        | Site::ActionName
        | Site::TargetResourceTags
        | Site::TargetParameterName
        | Site::ActionParameterName => {
            let answer = create_template(&router, create_body(site, case, key)).await;
            if !admitted {
                assert_refused(site, case, &answer);
                assert_eq!(
                    listed(&router, case, "/experimentTemplates", "experimentTemplates").await,
                    0,
                    "case {case:?}: a refused create must persist no template"
                );
                return;
            }
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from CreateExperimentTemplate, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            let id = minted_id(case, &answer, "experimentTemplate");
            assert_carries(site, case, key, &read_template(&router, case, &id).await);
        }
        Site::ExperimentTags => {
            let created = create_template(&router, create_body(site, case, key)).await;
            let template_id = minted_id(case, &created, "experimentTemplate");
            let answer = send(
                &router,
                "POST",
                "/experiments",
                Some(json!({
                    "clientToken": token(&format!("start-{}", slug(case))),
                    "experimentTemplateId": template_id,
                    "tags": one_entry(key, json!("x")),
                })),
                FALLBACK_REGION,
            )
            .await;
            if !admitted {
                assert_refused(site, case, &answer);
                assert_eq!(
                    listed(&router, case, "/experiments", "experiments").await,
                    0,
                    "case {case:?}: a refused start must persist no experiment"
                );
                return;
            }
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
                answer.status, answer.body
            );
            let id = minted_id(case, &answer, "experiment");
            assert_carries(site, case, key, &read_experiment(&router, case, &id).await);
        }
        Site::AppliedTags => {
            let mut body = create_body(site, case, key);
            body["tags"] = json!({ "Suite": "map-key-bounds" });
            let created = create_template(&router, body).await;
            let id = minted_id(case, &created, "experimentTemplate");
            let arn = created.member("experimentTemplate")["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no arn in {:?}", created.body))
                .to_string();
            let answer = send(
                &router,
                "POST",
                &format!("/tags/{}", label(&arn)),
                Some(json!({ "tags": one_entry(key, json!("x")) })),
                FALLBACK_REGION,
            )
            .await;
            let stored = read_template(&router, case, &id).await;
            if !admitted {
                assert_refused(site, case, &answer);
                assert_eq!(
                    stored["tags"],
                    json!({ "Suite": "map-key-bounds" }),
                    "case {case:?}: a refused tag call must leave the stored tags alone, \
                     got {:?}",
                    stored["tags"]
                );
                return;
            }
            assert_eq!(
                answer.status, 204,
                "case {case:?}: expected HTTP 204 from TagResource, got {} with {:?}",
                answer.status, answer.body
            );
            assert_carries(site, case, key, &stored);
        }
        Site::UpdatedActionName => {
            let created = create_template(&router, create_body(site, case, key)).await;
            let id = minted_id(case, &created, "experimentTemplate");
            let before = read_template(&router, case, &id).await;
            let answer = send(
                &router,
                "PATCH",
                &format!("/experimentTemplates/{id}"),
                Some(json!({ "actions": one_entry(key, wait_action(None)) })),
                FALLBACK_REGION,
            )
            .await;
            let after = read_template(&router, case, &id).await;
            if !admitted {
                assert_refused(site, case, &answer);
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused update must change nothing, got {after:?}"
                );
                return;
            }
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 from UpdateExperimentTemplate, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert_carries(site, case, key, &after);
        }
    }
}

/// Every key shape, in every map this service persists one in.
#[tokio::test]
async fn a_map_key_is_judged_by_its_key_targets_modelled_bounds() {
    for site in sites() {
        for shape in shapes() {
            let key = (shape.key)(site.max_length());
            let case = format!("{} / {}", site.as_str(), shape.case);
            run(site, &case, &key).await;
        }
    }
}

/// The three operations that take a tag map agree about what a tag key is: the same key is
/// admitted by all three or refused by all three, so a key one of them persisted can never
/// be one another refuses on the same resource.
#[tokio::test]
async fn every_tag_writing_path_agrees_about_a_tag_key() {
    let tag_sites = [Site::TemplateTags, Site::ExperimentTags, Site::AppliedTags];
    for shape in shapes() {
        let key = (shape.key)(TAG_KEY_CEILING);
        for site in tag_sites {
            let case = format!("tag paths agree / {} / {}", site.as_str(), shape.case);
            // Each path is driven with the identical key, and `admits` — one rule, not one
            // per operation — is what every one of them is asserted against.
            run(site, &case, &key).await;
        }
    }
}
