// SPDX-License-Identifier: CC0-1.0

//! The order an experiment template's actions declare through `startAfter`, end to end over
//! HTTP.
//!
//! The rule under test: `startAfter` is "the name of the action that must be completed
//! before the current action starts"
//! (`smithy_query` query="doc" shape="ExperimentTemplateAction" member="startAfter"), so the
//! order it declares has to be one a run can carry out — the actions of one template, read
//! as a graph whose edges are its `startAfter` entries, are ACYCLIC. A set of actions that
//! reaches itself describes a run in which every action of that set waits for another action
//! of the set to complete first, so none of them can ever start; the template is refused
//! with the same 400 `ValidationException` the sibling rule two lines away gives a
//! `startAfter` that names no declared action, by whichever call built it.
//!
//! The rows differ along what the rule generalises over: the SHAPE of the action graph (a
//! single action, actions in parallel, a chain, a diamond, a long chain, a duplicated edge),
//! the LENGTH of the cycle when there is one (an action waiting for itself, two actions
//! waiting for each other, a three-action cycle), WHERE the cycle sits (the whole graph, one
//! of several components, reachable only from an action that is not itself on it) and HOW
//! MANY actions the template declares around it.
//!
//! Both calls that build a template are driven for every row, because the admissibility of a
//! template is a property of the template and not of the call: `CreateExperimentTemplate`
//! builds it outright and `UpdateExperimentTemplate` builds it by replacing the stored
//! actions, and the two must answer alike. An admitted graph is persisted and reported back
//! by `GetExperimentTemplate`; a refused one leaves the stored template exactly as it was.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, send, token, Answer, FALLBACK_REGION,
};

/// A well-formed IAM role ARN: no row here is about the execution role.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-action-order";

/// A `duration` inside the 1..720 minutes FIS documents for `aws:fis:wait`, carried by
/// every action of every row: no row here is about an action's parameters.
const WAIT: &str = "PT1M";

/// One row's action graph: an action name, and the actions its `startAfter` names in the
/// order the template records them. An entry with no names carries no `startAfter` at all.
type Graph = &'static [(&'static str, &'static [&'static str])];

/// One action graph, and whether the order it declares is one a run can carry out.
struct Row {
    case: &'static str,
    graph: Graph,
    /// `None` for a graph the service admits; otherwise the action the refusal names, which
    /// is an action ON the cycle — the first one in name order that the walk closes a cycle
    /// at, and never an action that merely waits for one.
    cycle_at: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one action naming no order",
            graph: &[("only", &[])],
            cycle_at: None,
        },
        Row {
            case: "three actions in parallel",
            graph: &[("a", &[]), ("b", &[]), ("c", &[])],
            cycle_at: None,
        },
        Row {
            case: "three actions chained",
            graph: &[("first", &[]), ("second", &["first"]), ("third", &["second"])],
            cycle_at: None,
        },
        Row {
            case: "a diamond: two branches joining",
            graph: &[
                ("start", &[]),
                ("left", &["start"]),
                ("right", &["start"]),
                ("join", &["left", "right"]),
            ],
            cycle_at: None,
        },
        Row {
            // The same earlier action named twice is still an acyclic order: a repeated
            // edge is not a cycle.
            case: "an action naming one earlier action twice",
            graph: &[("earlier", &[]), ("later", &["earlier", "earlier"])],
            cycle_at: None,
        },
        Row {
            case: "a chain of eight actions",
            graph: &[
                ("s0", &[]),
                ("s1", &["s0"]),
                ("s2", &["s1"]),
                ("s3", &["s2"]),
                ("s4", &["s3"]),
                ("s5", &["s4"]),
                ("s6", &["s5"]),
                ("s7", &["s6"]),
            ],
            cycle_at: None,
        },
        Row {
            case: "an action waiting for itself",
            graph: &[("loop", &["loop"])],
            cycle_at: Some("loop"),
        },
        Row {
            case: "two actions waiting for each other",
            graph: &[("a", &["b"]), ("b", &["a"])],
            cycle_at: Some("a"),
        },
        Row {
            case: "a three-action cycle",
            graph: &[("a", &["c"]), ("b", &["a"]), ("c", &["b"])],
            cycle_at: Some("a"),
        },
        Row {
            case: "a cycle reachable only from an action not on it",
            graph: &[("entry", &["x"]), ("x", &["y"]), ("y", &["x"])],
            cycle_at: Some("x"),
        },
        Row {
            case: "a cycle beside an acyclic component",
            graph: &[
                ("clean", &[]),
                ("after-clean", &["clean"]),
                ("spin", &["spun"]),
                ("spun", &["spin"]),
            ],
            cycle_at: Some("spin"),
        },
        Row {
            case: "a long chain closed back on its own head",
            graph: &[
                ("c0", &["c4"]),
                ("c1", &["c0"]),
                ("c2", &["c1"]),
                ("c3", &["c2"]),
                ("c4", &["c3"]),
            ],
            cycle_at: Some("c0"),
        },
    ]
}

/// The `actions` map `graph` describes: one `aws:fis:wait` per entry, carrying the
/// `startAfter` list the entry names.
fn actions(graph: Graph) -> Value {
    let mut map = Map::new();
    for (name, start_after) in graph {
        let mut action = json!({ "actionId": "aws:fis:wait", "parameters": { "duration": WAIT } });
        if !start_after.is_empty() {
            action["startAfter"] = json!(start_after);
        }
        map.insert((*name).to_string(), action);
    }
    Value::Object(map)
}

/// The create body for `row`: the members a create requires, with the row's action graph.
fn create_body(row: &Row) -> Value {
    json!({
        "clientToken": token(&format!("order-{}", row.case.replace(' ', "-"))),
        "description": format!("action order: {}", row.case),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": actions(row.graph)
    })
}

/// A template every update row starts from: one action, admissible, so the row's graph is
/// the only thing the update changes.
fn baseline_body(case: &str) -> Value {
    json!({
        "clientToken": token(&format!("order-baseline-{}", case.replace(' ', "-"))),
        "description": format!("action order baseline: {case}"),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": { "baseline": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT } } }
    })
}

/// `UpdateExperimentTemplate`: PATCH the template `id` names with the row's actions.
async fn update_template(router: &MultiServiceRouter, id: &str, body: Value) -> Answer {
    send(
        router,
        "PATCH",
        &format!("/experimentTemplates/{id}"),
        Some(body),
        FALLBACK_REGION,
    )
    .await
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

/// The `message` member of an answer, as a refused call carries it.
fn message(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert that `answer` is the answer `row` earns, and return the refusal as one string so
/// the two calls can be compared with each other.
fn assert_answer(row: &Row, call: &str, answer: &Answer) -> String {
    let case = row.case;
    let Some(cycle_at) = row.cycle_at else {
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from {call} for an order a run can carry out, \
             got {} with {:?}",
            answer.status, answer.body
        );
        return String::new();
    };
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 from {call} for an order no run can carry out, \
         got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected error code \"ValidationException\" from {call}, got {:?}",
        answer.body
    );
    let message = message(answer);
    let names = format!("actions.{cycle_at}.startAfter");
    assert!(
        message.contains(&names),
        "case {case:?}: expected the {call} message to name {names:?}, got {message:?}"
    );
    assert!(
        message.contains("cycle"),
        "case {case:?}: expected the {call} message to say the order is a cycle, got \
         {message:?}"
    );
    format!("{} {} {message}", answer.status, answer.error_code())
}

/// Assert that the stored `template` reports every action of `graph` with the `startAfter`
/// the request named, and nothing else.
fn assert_reports_graph(case: &str, graph: Graph, template: &Value) {
    let stored = template["actions"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected an actions map, got {template:?}"));
    assert_eq!(
        stored.len(),
        graph.len(),
        "case {case:?}: expected {} actions, got {stored:?}",
        graph.len()
    );
    for (name, start_after) in graph {
        let action = stored
            .get(*name)
            .unwrap_or_else(|| panic!("case {case:?}: no action {name:?} in {stored:?}"));
        let expected = if start_after.is_empty() {
            Value::Null
        } else {
            json!(start_after)
        };
        assert_eq!(
            action["startAfter"], expected,
            "case {case:?}: expected actions.{name}.startAfter to be {expected:?}, got {:?}",
            action["startAfter"]
        );
    }
}

/// Every row, through both calls that build a template.
#[tokio::test]
async fn an_action_order_a_run_cannot_carry_out_is_refused_by_both_calls() {
    for row in rows() {
        let case = row.case;
        let router = emulator();

        let created = create_template(&router, create_body(&row)).await;
        let from_create = assert_answer(&row, "CreateExperimentTemplate", &created);

        let baseline = create_template(&router, baseline_body(case)).await;
        assert_eq!(
            baseline.status, 200,
            "case {case:?}: expected HTTP 200 for the baseline template, got {} with {:?}",
            baseline.status, baseline.body
        );
        let before = baseline.member("experimentTemplate").clone();
        let id = before["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.id in {before:?}"))
            .to_string();

        let updated = update_template(&router, &id, json!({ "actions": actions(row.graph) })).await;
        let from_update = assert_answer(&row, "UpdateExperimentTemplate", &updated);
        assert_eq!(
            from_create, from_update,
            "case {case:?}: the two calls must judge the same action graph alike, got \
             {from_create:?} from the create and {from_update:?} from the update"
        );

        if row.cycle_at.is_some() {
            // A refused update leaves the stored template exactly as it was, and the
            // refused create stored nothing to read.
            let after = read_template(&router, case, &id).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must change nothing, got {after:?}"
            );
            continue;
        }

        // An admitted order is PERSISTED by both calls, with the `startAfter` entries the
        // request named.
        let from_create = created.member("experimentTemplate").clone();
        let created_id = from_create["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no experimentTemplate.id in {from_create:?}"))
            .to_string();
        let stored = read_template(&router, case, &created_id).await;
        assert_eq!(
            stored, from_create,
            "case {case:?}: GetExperimentTemplate must report what the create stored, got \
             {stored:?}"
        );
        assert_reports_graph(case, row.graph, &stored);
        assert_reports_graph(case, row.graph, &read_template(&router, case, &id).await);
    }
}

/// An order a run CAN carry out is started, and one it cannot never reaches
/// `StartExperiment` at all: the template it would be started from does not exist.
#[tokio::test]
async fn only_a_template_whose_order_can_be_carried_out_starts_an_experiment() {
    let router = emulator();
    let chain = Row {
        case: "three actions chained",
        graph: &[("first", &[]), ("second", &["first"]), ("third", &["second"])],
        cycle_at: None,
    };
    let created = create_template(&router, create_body(&chain)).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        chain.case, created.status, created.body
    );
    let id = created.member("experimentTemplate")["id"]
        .as_str()
        .expect("the created template reports an id")
        .to_string();

    let started = send(
        &router,
        "POST",
        "/experiments",
        Some(json!({
            "clientToken": token("order-start-chain"),
            "experimentTemplateId": id
        })),
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
        chain.case, started.status, started.body
    );

    let cyclic = Row {
        case: "two actions waiting for each other",
        graph: &[("a", &["b"]), ("b", &["a"])],
        cycle_at: Some("a"),
    };
    let refused = create_template(&router, create_body(&cyclic)).await;
    assert_answer(&cyclic, "CreateExperimentTemplate", &refused);
    assert!(
        refused.body["experimentTemplate"].is_null(),
        "case {:?}: a refused create reports no template to start from, got {:?}",
        cyclic.case,
        refused.body
    );
}
