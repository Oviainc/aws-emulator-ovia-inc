// SPDX-License-Identifier: CC0-1.0

//! ONE wording for one constraint class, end to end over HTTP: a value longer than its
//! member's modelled `@length` maximum.
//!
//! The rule under test: such a value is refused with 400 `ValidationException` whose
//! message is `Maximum length exceeded: <member>` — the member that was too long and
//! nothing else, with the rejected value never echoed back — WHEREVER that modelled bound
//! is applied. Most members are judged by the boundary validator, but the validator does
//! not reach a map's KEY (it reaches the map's VALUE), so every operation that persists a
//! map applies its key target's bounds in the handler; a caller must not be able to tell
//! the two apart from the message, because it is the same modelled constraint either way.
//!
//! The rows span what the rule generalises over:
//!
//! * WHERE the over-long value sits — a map key the handler judges (the caller tags of a
//!   template, of an experiment and of a tag call, a template's target and action names,
//!   the `resourceTags` a target identifies resources by, a target's and an action's
//!   parameter names, and the `actions` map an update carries) and a member the boundary
//!   validator judges (`description`, `clientToken`, `roleArn`, an `@httpLabel` id and an
//!   `@httpQuery` filter);
//! * the member's own published ceiling, which differs per site (128, 64, 512, 1024,
//!   2048);
//! * HOW FAR past that ceiling the value goes — one character and far past it;
//! * whether the over-long value is ASCII or multi-byte, since a Smithy `@length` counts
//!   CHARACTERS;
//! * the complement at every published ceiling: a value exactly AT it is not a length
//!   refusal at all.
//!
//! The ceilings are spelled out here rather than imported from the crate, so a wrong bound
//! in `src/state.rs` or in the model fails this suite.

use serde_json::{json, Map, Value};

use crate::experiment_lifecycle::{
    create_template, emulator, send, token, Answer, FALLBACK_REGION, WAIT_DURATION,
};

/// `TagKey`'s published ceiling: `TagMap`'s key target is `@length(min: 0, max: 128)`
/// (`smithy_query(query="shape", shape="TagMap", …)`).
const TAG_KEY_CEILING: usize = 128;

/// The published ceiling of a target name, an action name and a parameter name:
/// `ExperimentTemplateTargetName`, `ExperimentTemplateActionName`,
/// `ExperimentTemplateTargetParameterName` and `ExperimentTemplateActionParameterName` are
/// each `@length(min: 0, max: 64)`.
const NAME_CEILING: usize = 64;

/// `ExperimentTemplateDescription`'s published ceiling (`@length(min: 0, max: 512)`).
const DESCRIPTION_CEILING: usize = 512;

/// `ClientToken`'s published ceiling (`@length(min: 1, max: 1024)`).
const CLIENT_TOKEN_CEILING: usize = 1024;

/// `RoleArn`'s published ceiling (`@length(min: 20, max: 2048)`).
const ROLE_ARN_CEILING: usize = 2048;

/// `ExperimentTemplateId`'s published ceiling (`@length(min: 0, max: 64)`) — the bound on
/// the `@httpLabel` a read names a template by and on the `@httpQuery` a list filters by.
const ID_CEILING: usize = 64;

/// A well-formed IAM role ARN for every template this suite creates.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-length-refusal";

/// A resource type the published catalog holds, requiring no target parameter of its own.
const RESOURCE_TYPE: &str = "aws:ec2:instance";

/// Where a row puts its over-long value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Site {
    /// A map KEY the handler judges: `CreateExperimentTemplate`'s `tags`.
    TemplateTagKey,
    /// A map KEY: `StartExperiment`'s `tags`.
    ExperimentTagKey,
    /// A map KEY: `TagResource`'s `tags`.
    AppliedTagKey,
    /// A map KEY: the name of a template's target.
    TargetName,
    /// A map KEY: the name of a template's action.
    ActionName,
    /// A map KEY: the `resourceTags` one target identifies its resources by.
    TargetResourceTagKey,
    /// A map KEY: the name of one target's parameter.
    TargetParameterName,
    /// A map KEY: the name of one action's parameter.
    ActionParameterName,
    /// A map KEY an UPDATE carries: the name of an action.
    UpdatedActionName,
    /// A body member the boundary validator judges.
    Description,
    /// A body member carrying an `@idempotencyToken`.
    ClientToken,
    /// A body member naming an IAM role.
    RoleArn,
    /// An `@httpLabel`: the id `GetExperimentTemplate` reads by.
    PathId,
    /// An `@httpQuery`: the template id `ListExperiments` filters by.
    QueryTemplateId,
}

impl Site {
    fn as_str(self) -> &'static str {
        match self {
            Self::TemplateTagKey => "CreateExperimentTemplate.tags key",
            Self::ExperimentTagKey => "StartExperiment.tags key",
            Self::AppliedTagKey => "TagResource.tags key",
            Self::TargetName => "CreateExperimentTemplate.targets key",
            Self::ActionName => "CreateExperimentTemplate.actions key",
            Self::TargetResourceTagKey => "CreateExperimentTemplate.targets.resourceTags key",
            Self::TargetParameterName => "CreateExperimentTemplate.targets.parameters key",
            Self::ActionParameterName => "CreateExperimentTemplate.actions.parameters key",
            Self::UpdatedActionName => "UpdateExperimentTemplate.actions key",
            Self::Description => "CreateExperimentTemplate.description",
            Self::ClientToken => "CreateExperimentTemplate.clientToken",
            Self::RoleArn => "CreateExperimentTemplate.roleArn",
            Self::PathId => "GetExperimentTemplate.id",
            Self::QueryTemplateId => "ListExperiments.experimentTemplateId",
        }
    }

    /// The published ceiling of the shape this site's value targets.
    fn ceiling(self) -> usize {
        match self {
            Self::TemplateTagKey
            | Self::ExperimentTagKey
            | Self::AppliedTagKey
            | Self::TargetResourceTagKey => TAG_KEY_CEILING,
            Self::TargetName
            | Self::ActionName
            | Self::TargetParameterName
            | Self::ActionParameterName
            | Self::UpdatedActionName => NAME_CEILING,
            Self::Description => DESCRIPTION_CEILING,
            Self::ClientToken => CLIENT_TOKEN_CEILING,
            Self::RoleArn => ROLE_ARN_CEILING,
            Self::PathId | Self::QueryTemplateId => ID_CEILING,
        }
    }

    /// The member the refusal has to name: the last named member of the position the
    /// over-long value sits at, which for a nested map is the map's own member name.
    fn member(self) -> &'static str {
        match self {
            Self::TemplateTagKey | Self::ExperimentTagKey | Self::AppliedTagKey => "tags",
            Self::TargetName => "targets",
            Self::ActionName | Self::UpdatedActionName => "actions",
            Self::TargetResourceTagKey => "resourceTags",
            Self::TargetParameterName | Self::ActionParameterName => "parameters",
            Self::Description => "description",
            Self::ClientToken => "clientToken",
            Self::RoleArn => "roleArn",
            Self::PathId => "id",
            Self::QueryTemplateId => "experimentTemplateId",
        }
    }

    /// Whether this site's value can be carried as multi-byte text. A value in the request
    /// BODY can; one in the request URI is spelled out in ASCII by this suite, so a row
    /// about it is about the bound and not about percent-encoding.
    fn carries_non_ascii(self) -> bool {
        !matches!(self, Self::PathId | Self::QueryTemplateId)
    }

    /// The status a value of exactly this site's ceiling is ADMITTED with, or `None` when
    /// this site answers something else at that length. The other sites answer something
    /// else for a reason of their own — a parameter
    /// name no published entry takes, a `roleArn` that is not a role ARN, an id naming no
    /// record — which is still not a length refusal, and that is all those rows assert.
    /// `TagResource` is the one admitting site whose success is bodyless, so it answers
    /// 204 No Content rather than 200.
    fn admitted_status(self) -> Option<u16> {
        match self {
            Self::AppliedTagKey => Some(204),
            Self::TemplateTagKey
            | Self::ExperimentTagKey
            | Self::TargetResourceTagKey
            | Self::TargetName
            | Self::ActionName
            | Self::UpdatedActionName
            | Self::Description
            | Self::ClientToken => Some(200),
            _ => None,
        }
    }
}

/// Every site, so each table runs over all of them.
fn sites() -> Vec<Site> {
    vec![
        Site::TemplateTagKey,
        Site::ExperimentTagKey,
        Site::AppliedTagKey,
        Site::TargetName,
        Site::ActionName,
        Site::TargetResourceTagKey,
        Site::TargetParameterName,
        Site::ActionParameterName,
        Site::UpdatedActionName,
        Site::Description,
        Site::ClientToken,
        Site::RoleArn,
        Site::PathId,
        Site::QueryTemplateId,
    ]
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

/// A `clientToken` (`pattern ^[\S]+$`) derived from a case name.
fn slug(case: &str) -> String {
    case.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// An admissible create body: the rows that are not ABOUT the create send this one to get
/// a resource to work on.
fn plain_body(case: &str) -> Value {
    json!({
        "clientToken": token(&slug(case)),
        "description": "the length-refusal suite's template",
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": { "wait": wait_action(None) },
    })
}

/// What the emulator answers when `site` carries `value`.
async fn answer_for(site: Site, case: &str, value: &str) -> Answer {
    // One emulator per row, so no row can see another's resources or tokens.
    let router = emulator();
    let mut body = plain_body(case);
    match site {
        Site::TemplateTagKey => {
            body["tags"] = one_entry(value, json!("x"));
            create_template(&router, body).await
        }
        Site::TargetName => {
            body["targets"] = one_entry(value, target(json!({ "Stage": "test" }), None));
            create_template(&router, body).await
        }
        Site::ActionName => {
            body["actions"] = one_entry(value, wait_action(None));
            create_template(&router, body).await
        }
        Site::TargetResourceTagKey => {
            body["targets"] = json!({ "instances": target(one_entry(value, json!("x")), None) });
            create_template(&router, body).await
        }
        Site::TargetParameterName => {
            body["targets"] = json!({
                "instances": target(
                    json!({ "Stage": "test" }),
                    Some(one_entry(value, json!("v"))),
                )
            });
            create_template(&router, body).await
        }
        Site::ActionParameterName => {
            body["actions"] = json!({ "wait": wait_action(Some(value)) });
            create_template(&router, body).await
        }
        Site::Description => {
            body["description"] = json!(value);
            create_template(&router, body).await
        }
        Site::ClientToken => {
            body["clientToken"] = json!(value);
            create_template(&router, body).await
        }
        Site::RoleArn => {
            body["roleArn"] = json!(value);
            create_template(&router, body).await
        }
        Site::ExperimentTagKey => {
            let created = create_template(&router, body).await;
            let template_id = minted_id(case, &created, "experimentTemplate");
            send(
                &router,
                "POST",
                "/experiments",
                Some(json!({
                    "clientToken": token(&format!("start-{}", slug(case))),
                    "experimentTemplateId": template_id,
                    "tags": one_entry(value, json!("x")),
                })),
                FALLBACK_REGION,
            )
            .await
        }
        Site::AppliedTagKey => {
            let created = create_template(&router, body).await;
            let arn = created.member("experimentTemplate")["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no template arn in {:?}", created.body))
                .to_string();
            send(
                &router,
                "POST",
                &format!("/tags/{}", label(&arn)),
                Some(json!({ "tags": one_entry(value, json!("x")) })),
                FALLBACK_REGION,
            )
            .await
        }
        Site::UpdatedActionName => {
            let created = create_template(&router, body).await;
            let id = minted_id(case, &created, "experimentTemplate");
            send(
                &router,
                "PATCH",
                &format!("/experimentTemplates/{id}"),
                Some(json!({ "actions": one_entry(value, wait_action(None)) })),
                FALLBACK_REGION,
            )
            .await
        }
        Site::PathId => {
            send(
                &router,
                "GET",
                &format!("/experimentTemplates/{value}"),
                None,
                FALLBACK_REGION,
            )
            .await
        }
        Site::QueryTemplateId => {
            send(
                &router,
                "GET",
                &format!("/experiments?experimentTemplateId={value}"),
                None,
                FALLBACK_REGION,
            )
            .await
        }
    }
}

/// The id a create reported.
fn minted_id(case: &str, answer: &Answer, member: &str) -> String {
    answer.member(member)["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no {member}.id in {:?}", answer.body))
        .to_string()
}

/// The `message` an answer carries.
fn message(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every site, driven one character and far past its published ceiling, in ASCII and in
/// multi-byte text: one wording for all of them.
#[tokio::test]
async fn a_value_past_its_members_length_maximum_is_refused_by_one_wording() {
    // How far past the ceiling, and what the over-long value is made of. A Smithy
    // `@length` counts CHARACTERS, so the multi-byte rows are over the bound by exactly as
    // many characters as the ASCII ones while carrying three times the bytes.
    let overshoots: [(&str, usize); 2] = [("one character past", 1), ("far past", 40)];
    let fillers: [(&str, char); 2] = [("ascii", 'k'), ("multi-byte", '環')];

    for site in sites() {
        for (overshoot_case, overshoot) in overshoots {
            for (filler_case, filler) in fillers {
                if filler != 'k' && !site.carries_non_ascii() {
                    continue;
                }
                let case = format!("{} / {overshoot_case} / {filler_case}", site.as_str());
                let value = String::from(filler).repeat(site.ceiling() + overshoot);
                let answer = answer_for(site, &case, &value).await;

                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {:?}",
                    answer.body
                );
                let stated = message(&answer);
                assert_eq!(
                    stated,
                    format!("Maximum length exceeded: {}", site.member()),
                    "case {case:?}: expected the one length wording naming {:?}, got \
                     {stated:?}",
                    site.member()
                );
                assert!(
                    !stated.contains(&value),
                    "case {case:?}: the refused value must not be echoed back, got \
                     {stated:?}"
                );
                assert!(
                    !stated.contains(&(site.ceiling() + overshoot).to_string()),
                    "case {case:?}: the refusal names the member and nothing else, got \
                     {stated:?}"
                );
            }
        }
    }
}

/// The complement at every published ceiling: a value of exactly that many characters is
/// not a length refusal, so the wording above is earned by being PAST the bound and not by
/// being near it.
#[tokio::test]
async fn a_value_at_its_members_length_maximum_is_no_length_refusal() {
    for site in sites() {
        let case = format!("{} / at the published ceiling", site.as_str());
        let value = "k".repeat(site.ceiling());
        let answer = answer_for(site, &case, &value).await;
        let stated = message(&answer);
        assert_ne!(
            stated,
            format!("Maximum length exceeded: {}", site.member()),
            "case {case:?}: a value AT its ceiling is no length refusal, got {:?}",
            answer.body
        );
        if let Some(admitted) = site.admitted_status() {
            assert_eq!(
                answer.status, admitted,
                "case {case:?}: a value at its published ceiling is accepted, got {} with \
                 {:?}",
                answer.status, answer.body
            );
        }
    }
}
