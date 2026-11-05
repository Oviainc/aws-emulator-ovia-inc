// SPDX-License-Identifier: CC0-1.0

//! One wording for every input constraint the SLO WRITERS enforce in a handler, over HTTP.
//!
//! `CreateServiceLevelObjective` and `UpdateServiceLevelObjective` enforce three documented
//! input rules the model cannot state — the two SLI configs are mutually exclusive, a create
//! must define one of them, and an update may not supply one of the other kind than the
//! stored record — beside the documented tag maximum and every constraint the model DOES
//! state. A rule that constrains ONE member is refused in the `N validation error(s)
//! detected:` envelope over a `Value at '<memberPath>' failed to satisfy constraint:
//! <requirement>` clause that names the member the request must fix, in the lower-camel
//! spelling the boundary validator uses, and nothing of the rejected value. The two rules that
//! constrain no single member — a definition that must populate one of a PAIR, and an update
//! that would switch the stored SLO's evaluation kind — state the rule itself instead; the
//! suite that owns that prose is `refusal_prose`.
//!
//! The rows span what the rule generalises over: which OPERATION refuses (the create, the
//! update), which KIND of rule fires (two mutually exclusive members, a member one of a pair
//! must supply, a member inadmissible for the stored resource's kind, a documented list
//! maximum), WHICH member is at fault, the FORM of the offending config (an Application
//! Signals entity or the caller's own metric queries), which other members ride along — plus,
//! per operation, a control row refused by the boundary validator itself for a constraint the
//! model DOES state, which is the wording all of them have to match.
//!
//! The served rows pin the other side of each rule: the same operations with one config, with
//! a config of the stored kind, with none at all and with the documented number of tags are
//! all answered.

use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::state::MAX_TAGS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

use crate::refusal_prose::{kind_switch_refusal, MISSING_SLI_CONFIG, PERIOD_BASED, REQUEST_BASED};
use crate::resource_tags::{tag, tag_list};
use crate::slo_evaluation_kind::{
    request_based_sli_config, request_based_sli_config_with_queries, sli_config_with_queries,
};
use crate::slo_lifecycle::{call, create, router, sli_config};

/// The modelled `ServiceLevelObjectiveName` pattern, without the anchors the requirement
/// drops (`smithy_query(query="operation", operation="CreateServiceLevelObjective")`: `Name`
/// has pattern `^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$`).
const NAME_PATTERN: &str = "[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]";

/// A `Name` the modelled pattern refuses: it requires at least two characters.
const NAME_TOO_SHORT: &str = "a";

/// The modelled maximum length of `Description`
/// (`smithy_query(query="operation", operation="UpdateServiceLevelObjective")`: `Description`
/// has `length 1..1024`).
const DESCRIPTION_MAX: usize = 1024;

/// The message a refusal of `member_path` for `requirement` must carry: the AWS envelope over
/// one clause, the way the boundary validator's own refusals are rendered.
fn one_clause(member_path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member_path}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// `count` distinct tags, which is how a row exceeds the documented tag maximum.
fn generated_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| tag(&format!("key-{index:03}"), &format!("value-{index:03}")))
        .collect()
}

/// The member path a clause names, as spelled between the quotes.
fn member_path_of(message: &str) -> Option<&str> {
    let (_, after) = message.split_once("Value at '")?;
    after.split_once('\'').map(|(path, _)| path)
}

/// The SLO a row's request addresses: the config member a setup create supplies, and the
/// config itself. A row that refuses a CREATE stores nothing first.
type Stored = (&'static str, Value);

/// One refused row: the request, the whole message the refusal must carry, the member the
/// clause must name when the rule constrains ONE member (`None` for a rule that constrains
/// none, whose refusal states the rule instead), plus the caller input the message must not
/// echo.
struct Refused {
    label: &'static str,
    stored: Option<Stored>,
    body: Value,
    member_path: Option<&'static str>,
    message: String,
    rejected: Vec<String>,
}

/// One served row: a request on the admitted side of the same rule.
struct Served {
    label: &'static str,
    stored: Option<Stored>,
    body: Value,
}

/// A period-based config whose threshold is the row's, so two rows never send one request.
fn period_based(threshold: f64) -> Value {
    sli_config("checkout", "GET /cart", threshold)
}

/// The refused rows, over every input constraint the two SLO writers enforce.
fn refused_rows() -> Vec<Refused> {
    let exclusive = one_clause(
        "requestBasedSliConfig",
        "Member must be null when sliConfig is provided",
    );
    vec![
        Refused {
            label: "create, both configs on entities",
            stored: None,
            body: json!({
                "SliConfig": period_based(100.0),
                "RequestBasedSliConfig": request_based_sli_config("checkout"),
            }),
            member_path: Some("requestBasedSliConfig"),
            message: exclusive.clone(),
            rejected: vec!["checkout".to_string(), "GET /cart".to_string()],
        },
        Refused {
            label: "create, both configs spelling their own metric queries",
            stored: None,
            body: json!({
                "SliConfig": sli_config_with_queries(),
                "RequestBasedSliConfig": request_based_sli_config_with_queries(),
            }),
            member_path: Some("requestBasedSliConfig"),
            message: exclusive.clone(),
            rejected: vec!["5XXError".to_string(), "AWS/ApiGateway".to_string()],
        },
        Refused {
            label: "create, neither config, name only",
            stored: None,
            body: json!({}),
            member_path: None,
            message: MISSING_SLI_CONFIG.to_string(),
            rejected: Vec::new(),
        },
        Refused {
            label: "create, neither config, every other member filled",
            stored: None,
            body: json!({
                "Description": "an objective with no indicator",
                "Goal": { "AttainmentGoal": 99.9, "WarningThreshold": 30.0 },
                "BurnRateConfigurations": [{ "LookBackWindowMinutes": 60 }],
                "Tags": tag_list(&[tag("team", "checkout")]),
                "CreateRecommendedSlo": true,
                "AutoInvestigationEnabled": true,
            }),
            member_path: None,
            message: MISSING_SLI_CONFIG.to_string(),
            rejected: vec![
                "an objective with no indicator".to_string(),
                "99.9".to_string(),
                "team".to_string(),
            ],
        },
        Refused {
            label: "create, one tag over the documented maximum",
            stored: None,
            body: json!({
                "SliConfig": period_based(110.0),
                "Tags": tag_list(&generated_tags(MAX_TAGS_PER_RESOURCE + 1)),
            }),
            member_path: Some("tags"),
            message: one_clause(
                "tags",
                &format!(
                    "Member must have length less than or equal to {MAX_TAGS_PER_RESOURCE}"
                ),
            ),
            rejected: vec!["key-000".to_string()],
        },
        // The create's control row: a constraint the MODEL states, refused by the boundary
        // validator before the handler runs — the wording every single-member row above has
        // to match.
        Refused {
            label: "create, a Name the modelled pattern refuses",
            stored: None,
            body: json!({ "Name": NAME_TOO_SHORT, "SliConfig": period_based(120.0) }),
            member_path: Some("name"),
            message: one_clause(
                "name",
                &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            ),
            rejected: Vec::new(),
        },
        Refused {
            label: "update, both configs on a period-based record",
            stored: Some(("SliConfig", period_based(130.0))),
            body: json!({
                "SliConfig": period_based(140.0),
                "RequestBasedSliConfig": request_based_sli_config("checkout"),
            }),
            member_path: Some("requestBasedSliConfig"),
            message: exclusive.clone(),
            rejected: vec!["checkout".to_string()],
        },
        Refused {
            label: "update, both configs on a request-based record",
            stored: Some(("RequestBasedSliConfig", request_based_sli_config("orders"))),
            body: json!({
                "SliConfig": sli_config_with_queries(),
                "RequestBasedSliConfig": request_based_sli_config_with_queries(),
            }),
            member_path: Some("requestBasedSliConfig"),
            message: exclusive,
            rejected: vec!["orders".to_string()],
        },
        Refused {
            label: "update, a request-based config on a period-based record",
            stored: Some(("SliConfig", period_based(150.0))),
            body: json!({ "RequestBasedSliConfig": request_based_sli_config("checkout") }),
            member_path: None,
            message: kind_switch_refusal(PERIOD_BASED, REQUEST_BASED),
            rejected: vec!["checkout".to_string()],
        },
        Refused {
            label: "update, a request-based query config on a period-based record, bundled",
            stored: Some(("SliConfig", sli_config_with_queries())),
            body: json!({
                "RequestBasedSliConfig": request_based_sli_config_with_queries(),
                "Description": "after",
                "Goal": { "AttainmentGoal": 95.0 },
                "AutoInvestigationEnabled": true,
            }),
            member_path: None,
            message: kind_switch_refusal(PERIOD_BASED, REQUEST_BASED),
            rejected: vec!["after".to_string(), "95".to_string()],
        },
        Refused {
            label: "update, a period-based config on a request-based record",
            stored: Some((
                "RequestBasedSliConfig",
                request_based_sli_config_with_queries(),
            )),
            body: json!({ "SliConfig": period_based(160.0) }),
            member_path: None,
            message: kind_switch_refusal(REQUEST_BASED, PERIOD_BASED),
            rejected: vec!["160".to_string()],
        },
        Refused {
            label: "update, a period-based entity config on a request-based record",
            stored: Some(("RequestBasedSliConfig", request_based_sli_config("payments"))),
            body: json!({ "SliConfig": sli_config_with_queries() }),
            member_path: None,
            message: kind_switch_refusal(REQUEST_BASED, PERIOD_BASED),
            rejected: vec!["payments".to_string()],
        },
        // The update's control row: a constraint the MODEL states on this operation.
        Refused {
            label: "update, a Description longer than the model admits",
            stored: Some(("SliConfig", period_based(170.0))),
            body: json!({ "Description": "d".repeat(DESCRIPTION_MAX + 1) }),
            member_path: Some("description"),
            message: one_clause(
                "description",
                &format!("Member must have length less than or equal to {DESCRIPTION_MAX}"),
            ),
            rejected: Vec::new(),
        },
    ]
}

/// The served rows: a request on the admitted side of each rule above.
fn served_rows() -> Vec<Served> {
    vec![
        Served {
            label: "create, the period-based config alone",
            stored: None,
            body: json!({ "SliConfig": period_based(200.0) }),
        },
        Served {
            label: "create, the request-based config alone",
            stored: None,
            body: json!({ "RequestBasedSliConfig": request_based_sli_config("checkout") }),
        },
        Served {
            label: "create, exactly the documented number of tags",
            stored: None,
            body: json!({
                "SliConfig": period_based(210.0),
                "Tags": tag_list(&generated_tags(MAX_TAGS_PER_RESOURCE)),
            }),
        },
        Served {
            label: "update, a config of the stored kind",
            stored: Some(("SliConfig", period_based(220.0))),
            body: json!({ "SliConfig": sli_config_with_queries() }),
        },
        Served {
            label: "update, a request-based config of the stored kind",
            stored: Some(("RequestBasedSliConfig", request_based_sli_config("orders"))),
            body: json!({ "RequestBasedSliConfig": request_based_sli_config_with_queries() }),
        },
        Served {
            label: "update, no config at all",
            stored: Some(("SliConfig", period_based(230.0))),
            body: json!({ "Description": "after" }),
        },
    ]
}

/// Store the row's SLO, if it has one, and answer the record the create wrote.
async fn stage(
    router: &MultiServiceRouter,
    stored: Option<&Stored>,
    name: &str,
    label: &str,
) -> Option<Value> {
    let (member, config) = stored?;
    let member: &str = member;
    let (status, created) = create(
        router,
        json!({ "Name": name, "Description": "before", member: config }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the setup create to succeed, got {status} with {created}"
    );
    Some(created["Slo"].clone())
}

/// Read a stored SLO back: the status and the body `GetServiceLevelObjective` answers.
async fn get_slo(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "GET", &format!("/slo/{name}"), None).await
}

/// Send the row's request: a create when it stores nothing first, an update of the staged
/// record otherwise.
async fn send(
    router: &MultiServiceRouter,
    stored: Option<&Stored>,
    name: &str,
    body: &Value,
) -> (u16, Value) {
    if stored.is_some() {
        call(router, "PATCH", &format!("/slo/{name}"), Some(body.clone())).await
    } else {
        let mut body = body.clone();
        if body["Name"].is_null() {
            body["Name"] = json!(name);
        }
        create(router, body).await
    }
}

/// Every input constraint the SLO writers enforce is refused with the wording its RULE calls
/// for: a constraint on one member in the `ValidationException` envelope the boundary
/// validator uses — the clause, the member path and the requirement, and nothing of the
/// rejected value — and a rule on no single member in its own sentence. A refused create
/// stores nothing and a refused update leaves the stored record exactly as it was.
#[tokio::test]
async fn every_slo_writer_constraint_states_its_member_and_requirement() {
    for (index, case) in refused_rows().into_iter().enumerate() {
        let label = case.label;
        let router = router();
        let name = format!("refused-{index:02}");
        let before = stage(&router, case.stored.as_ref(), &name, label).await;

        let (status, answer) = send(&router, case.stored.as_ref(), &name, &case.body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected a ValidationException, got {answer}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, case.message,
            "case {label:?}: expected {:?}, got {message:?}",
            case.message
        );
        for rejected in &case.rejected {
            assert!(
                !message.contains(rejected.as_str()),
                "case {label:?}: the message must not echo the rejected value {rejected:?}, got \
                 {message:?}"
            );
        }

        let (status, after) = get_slo(&router, &name).await;
        match before {
            None => assert_eq!(
                status, 404,
                "case {label:?}: expected a refused create to store nothing, got {status} with \
                 {after}"
            ),
            Some(before) => assert_eq!(
                after["Slo"], before,
                "case {label:?}: expected a refused update to leave the record untouched, \
                 expected {before:?}, got {:?}",
                after["Slo"]
            ),
        }
    }
}

/// Every refusal that names a member spells its member path the way the boundary validator
/// spells one for this service: lower camel case, a `.` between the steps of a nested member
/// and no other separator — so a client reads one spelling of one member whichever side
/// refused it. A row whose rule constrains no single member names none, and is skipped.
#[tokio::test]
async fn every_slo_writer_refusal_spells_its_member_path_the_one_way() {
    for (index, case) in refused_rows().into_iter().enumerate() {
        let Some(expected_path) = case.member_path else {
            continue;
        };
        let label = case.label;
        let router = router();
        let name = format!("path-{index:02}");
        let _staged = stage(&router, case.stored.as_ref(), &name, label).await;

        let (_, answer) = send(&router, case.stored.as_ref(), &name, &case.body).await;
        let message = answer["message"].as_str().unwrap_or_default().to_string();
        let path = member_path_of(&message)
            .unwrap_or_else(|| panic!("case {label:?}: no member path in {message:?}"));
        assert_eq!(
            path, expected_path,
            "case {label:?}: expected the clause to name {expected_path:?}, got {path:?}"
        );
        for step in path.split('.') {
            let first = step.chars().next().unwrap_or(' ');
            assert!(
                first.is_ascii_lowercase(),
                "case {label:?}: expected every step of the member path to start lower case, got \
                 {path:?}"
            );
            assert!(
                step.chars().all(|character| character.is_ascii_alphanumeric()),
                "case {label:?}: expected the member path to carry no separator but '.', got \
                 {path:?}"
            );
        }
    }
}

/// The admitted side of each rule is still served, so the wording above is asserted about the
/// requests that are refused rather than about every request to these operations.
#[tokio::test]
async fn the_admitted_side_of_each_slo_writer_rule_is_served() {
    for (index, case) in served_rows().into_iter().enumerate() {
        let label = case.label;
        let router = router();
        let name = format!("served-{index:02}");
        let _staged = stage(&router, case.stored.as_ref(), &name, label).await;

        let (status, answer) = send(&router, case.stored.as_ref(), &name, &case.body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the request to be served, got {status} with {answer}"
        );

        // Round trip: the record the writer answered is the one a read answers.
        let (status, fetched) = get_slo(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the written SLO to be readable, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["Slo"], answer["Slo"],
            "case {label:?}: expected a Get to answer the written record, got {:?}",
            fetched["Slo"]
        );
    }
}
