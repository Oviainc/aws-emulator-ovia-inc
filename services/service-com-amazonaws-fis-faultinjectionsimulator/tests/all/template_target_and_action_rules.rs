// SPDX-License-Identifier: CC0-1.0

//! The server-side rules an experiment template's TARGETS and ACTIONS are judged by, end to
//! end over HTTP.
//!
//! The rules under test, none of them expressible as a modelled constraint — the target
//! input models `resourceArns` and `resourceTags` as two independent optional members,
//! `selectionMode` as a free string, `actionId` as a free string and an action's `targets`
//! as a free `string -> string` map — so each one is refused by the service, persisting
//! nothing: with a 400 `ValidationException` where the rule is about the template's own
//! values, and with a 404 `ResourceNotFoundException` where it is about a REFERENCE that
//! resolves to nothing:
//!
//! * a target identifies its resources by EXACTLY ONE of `resourceArns` or `resourceTags`
//!   ("You must specify at least one Amazon Resource Name (ARN) or at least one resource
//!   tag. You cannot specify both ARNs and tags", `CreateExperimentTemplateTargetInput`);
//! * `selectionMode` is one of the three forms that member documents — `ALL`, `COUNT(n)`
//!   and `PERCENT(n)` — so a value in none of them is refused rather than resolved as the
//!   default;
//! * `actionId` names one of the actions FIS PUBLISHES, the same catalog `GetAction` and
//!   `ListActions` report from — an id that catalog holds no action for names a resource
//!   that does not exist, reported after the KEY of the `actions` entry that named it;
//! * an action supplies every parameter that catalog records as `required` for it;
//! * each entry of an action's `targets` names, under a target key that action supports, a
//!   target the SAME template declares, of the resource type the action takes there.
//!
//! What the rows span: for the targets, which of the two identifying members a target names
//! (one, the other, both, neither, each of them present but EMPTY) and which
//! `selectionMode` form it uses (each documented form, with arguments at and past what the
//! target identified, against malformed and differently cased values); for the actions,
//! every action the published catalog holds — not a sample — each admitted with its
//! required parameters and refused without each one of them in turn, ids that name no
//! published action, and target references that are right, unsupported, undeclared or of
//! the wrong resource type. The catalogued rows are driven from `state::ACTION_CATALOG`
//! itself, so they cover every action the service publishes, including the ones no issue
//! cited.
//!
//! The rules belong to the TEMPLATE rather than to the call that produced it, so the last
//! test drives a selection of the rows through `UpdateExperimentTemplate` and asserts it
//! answers exactly as the create did.

use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::state::ACTION_CATALOG;
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, send, token, Answer, FALLBACK_REGION,
};

/// A well-formed IAM role ARN: every row here is about targets and actions, not the role.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-target-and-action-rules";

/// The wire error code a refusal about the template's own VALUES carries.
const VALIDATION: &str = "ValidationException";

/// The wire error code a refusal about a REFERENCE that resolves to nothing carries: an
/// `actionId` naming no action the catalog publishes names a resource that does not exist,
/// which FIS answers with HTTP 404 rather than with a malformed-request class.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The name every row gives the one action under test.
const ACTION: &str = "underTest";

/// The `duration` value a row supplies for a parameter of that name: inside the 1 to 720
/// minutes FIS documents for the `aws:fis:wait` action's own duration, so an action that
/// requires a `duration` is admitted on its value as well as on its presence.
const DURATION: &str = "PT5M";

/// The value a row supplies for any other required parameter. `ExperimentTemplateActionParameter`
/// is modelled `@length(0..1024)`, `pattern ^[\S]+$`, which this satisfies; no rule here is
/// about what a parameter's value MEANS.
const PARAMETER_VALUE: &str = "placeholder";

/// Two EC2 instance ARNs, for the rows whose target identifies its resources by ARN.
fn instance_arns() -> Value {
    json!([
        "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
        "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef1"
    ])
}

/// A create body carrying `targets` and `actions`, with every other required member filled
/// in admissibly.
fn body(label: &str, targets: Value, actions: Value) -> Value {
    json!({
        "clientToken": token(&label.replace(' ', "-")),
        "description": format!("target and action rules: {label}"),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": targets,
        "actions": actions
    })
}

/// The one action a row that is not about actions carries: the built-in wait, with its
/// required `duration`.
fn wait() -> Value {
    json!({ ACTION: { "actionId": "aws:fis:wait", "parameters": { "duration": DURATION } } })
}

/// The `message` of a refused answer.
fn message(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// How many experiment templates the store holds. Every test here creates far fewer than
/// one page, so a single page is the whole store.
async fn template_count(router: &MultiServiceRouter) -> usize {
    let listed = send(
        router,
        "GET",
        "/experimentTemplates?maxResults=100",
        None,
        FALLBACK_REGION,
    )
    .await;
    listed
        .member("experimentTemplates")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// The template `id` names, as `GetExperimentTemplate` reports it.
async fn stored(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
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

/// Assert that `answer` refuses the call with a 400 `ValidationException` whose message
/// names every fragment in `names`.
fn assert_refused(case: &str, answer: &Answer, names: &[&str]) {
    assert_refused_as(case, answer, 400, VALIDATION, names);
}

/// Assert that `answer` refuses the call with HTTP `status` and error code `code`, and that
/// its message names every fragment in `names`.
fn assert_refused_as(case: &str, answer: &Answer, status: u16, code: &str, names: &[&str]) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected error code {code:?}, got {:?}",
        answer.body
    );
    let stated = message(answer);
    for fragment in names {
        assert!(
            stated.contains(fragment),
            "case {case:?}: expected the refusal to name {fragment:?}, got {stated:?}"
        );
    }
}

/// Assert that `answer` refuses the call because the entry named `action_name` carries an
/// `actionId` that resolves to no published action: a 404 `ResourceNotFoundException` worded
/// after the KEY of that entry, with the unresolvable id NOT echoed.
fn assert_unresolved_action(case: &str, answer: &Answer, action_name: &str) {
    assert_refused_as(case, answer, 404, NOT_FOUND, &[]);
    assert_eq!(
        message(answer),
        format!("Invalid actionId in action '{action_name}'."),
        "case {case:?}: expected the reference miss to name the action entry, got {:?}",
        answer.body
    );
}

/// Create `body` and, when the row is admitted, assert the store reports back what it
/// stored; when it is refused, assert the refusal and that the store grew by nothing.
/// Returns the stored template's id for an admitted row.
async fn judge(
    router: &MultiServiceRouter,
    case: &str,
    request: Value,
    refusal: Option<&[&str]>,
) -> Option<String> {
    let before = template_count(router).await;
    let answer = create_template(router, request).await;
    match refusal {
        Some(names) => {
            assert_refused(case, &answer, names);
            assert_eq!(
                template_count(router).await,
                before,
                "case {case:?}: a refused create must persist no template"
            );
            None
        }
        None => {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let id = answer.member("experimentTemplate")["id"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no experimentTemplate.id in {:?}", answer.body)
                })
                .to_string();
            assert_eq!(
                stored(router, case, &id).await,
                *answer.member("experimentTemplate"),
                "case {case:?}: GetExperimentTemplate must report what the create stored"
            );
            assert_eq!(
                template_count(router).await,
                before + 1,
                "case {case:?}: an admitted create persists exactly one template"
            );
            Some(id)
        }
    }
}

/// One target of the exclusivity table: the identifying members it names, and the refusal
/// it earns.
struct TargetRow {
    case: &'static str,
    /// The `resourceArns` the target names, absent for a row that names the member not at
    /// all.
    arns: Option<Value>,
    /// The `resourceTags` the target names, absent for a row that names the member not at
    /// all.
    tags: Option<Value>,
    /// `None` for a row the rule admits.
    refusal: Option<&'static [&'static str]>,
}

/// The target `row` describes, under the name the action table also uses.
fn target_of(row: &TargetRow) -> Value {
    let mut target = Map::new();
    target.insert("resourceType".to_string(), json!("aws:ec2:instance"));
    target.insert("selectionMode".to_string(), json!("ALL"));
    if let Some(arns) = &row.arns {
        target.insert("resourceArns".to_string(), arns.clone());
    }
    if let Some(tags) = &row.tags {
        target.insert("resourceTags".to_string(), tags.clone());
    }
    json!({ "hosts": Value::Object(target) })
}

/// RULE: a target identifies its resources by EXACTLY ONE of `resourceArns` or
/// `resourceTags` — the model's own target input states it ("You must specify at least one
/// Amazon Resource Name (ARN) or at least one resource tag. You cannot specify both ARNs
/// and tags"), and because both members are modelled optional the rule is the service's,
/// which refuses the template rather than storing one FIS would not run.
#[tokio::test]
async fn a_target_identifies_its_resources_by_exactly_one_of_arns_or_tags() {
    let router = emulator();

    // The rows differ along WHICH of the two members the target names, and along whether a
    // member it names identifies anything: an empty list and an empty map name no resource
    // and no tag, which is what "at least one" is about.
    const BOTH: &[&str] = &["targets.hosts", "resourceArns", "resourceTags"];
    const NEITHER: &[&str] = &["targets.hosts", "at least one"];
    for row in [
        TargetRow {
            case: "resourceArns alone",
            arns: Some(instance_arns()),
            tags: None,
            refusal: None,
        },
        TargetRow {
            case: "one resourceArn alone",
            arns: Some(json!(["arn:aws:ec2:us-east-1:000000000000:instance/i-1"])),
            tags: None,
            refusal: None,
        },
        TargetRow {
            case: "resourceTags alone",
            arns: None,
            tags: Some(json!({ "Stage": "test" })),
            refusal: None,
        },
        TargetRow {
            case: "several resourceTags alone",
            arns: None,
            tags: Some(json!({ "Stage": "test", "Team": "chaos" })),
            refusal: None,
        },
        TargetRow {
            case: "an empty resourceArns beside resourceTags",
            arns: Some(json!([])),
            tags: Some(json!({ "Stage": "test" })),
            refusal: None,
        },
        TargetRow {
            case: "an empty resourceTags beside resourceArns",
            arns: Some(instance_arns()),
            tags: Some(json!({})),
            refusal: None,
        },
        TargetRow {
            case: "both members naming resources",
            arns: Some(instance_arns()),
            tags: Some(json!({ "Stage": "test" })),
            refusal: Some(BOTH),
        },
        TargetRow {
            case: "both members, one resource each",
            arns: Some(json!(["arn:aws:ec2:us-east-1:000000000000:instance/i-1"])),
            tags: Some(json!({ "Stage": "test" })),
            refusal: Some(BOTH),
        },
        TargetRow {
            case: "neither member at all",
            arns: None,
            tags: None,
            refusal: Some(NEITHER),
        },
        TargetRow {
            case: "both members present and both empty",
            arns: Some(json!([])),
            tags: Some(json!({})),
            refusal: Some(NEITHER),
        },
        TargetRow {
            case: "an empty resourceArns and no tags",
            arns: Some(json!([])),
            tags: None,
            refusal: Some(NEITHER),
        },
        TargetRow {
            case: "an empty resourceTags and no arns",
            arns: None,
            tags: Some(json!({})),
            refusal: Some(NEITHER),
        },
    ] {
        judge(
            &router,
            row.case,
            body(row.case, target_of(&row), wait()),
            row.refusal,
        )
        .await;
    }
}

/// RULE: `selectionMode` is one of the three forms its own model documentation publishes —
/// `ALL`, `COUNT(n)` and `PERCENT(n)` — so a value in none of them is refused when the
/// template is created, rather than stored and silently resolved as the documented default.
#[tokio::test]
async fn a_selection_mode_outside_the_three_documented_forms_is_refused() {
    let router = emulator();

    // SOURCE for the admitted forms: the `selectionMode` member's own documentation
    // (`smithy_query(query="doc", shape="CreateExperimentTemplateTargetInput",
    // member="selectionMode")`) — "ALL … COUNT(n) … PERCENT(n)". The rows differ along the
    // form, along the argument (none, zero, under what the target identified, past it, each
    // end of a percentage) and, for the refused ones, along HOW the value departs from a
    // form: another word entirely, the right word differently cased, a form with no
    // argument, an unclosed one, a non-numeric one and an empty one.
    for (case, mode, admitted) in [
        ("every resource", "ALL", true),
        ("a count under what the target identified", "COUNT(1)", true),
        ("a count of none", "COUNT(0)", true),
        ("a count past what the target identified", "COUNT(9)", true),
        ("a percentage of all of them", "PERCENT(100)", true),
        ("half of them", "PERCENT(50)", true),
        ("a percentage of none", "PERCENT(0)", true),
        ("another word entirely", "SOME", false),
        ("no scoping at all", "NONE", false),
        ("the default in lower case", "all", false),
        ("a count in mixed case", "Count(1)", false),
        ("a count with no argument", "COUNT", false),
        ("a count with an empty argument", "COUNT()", false),
        ("an unclosed count", "COUNT(1", false),
        ("a count whose argument is no number", "COUNT(x)", false),
        ("a negative count", "COUNT(-1)", false),
        ("a percentage with no argument", "PERCENT", false),
        ("an unclosed percentage", "PERCENT(50", false),
        ("the default with an argument", "ALL(1)", false),
    ] {
        let targets = json!({
            "hosts": {
                "resourceType": "aws:ec2:instance",
                "resourceArns": instance_arns(),
                "selectionMode": mode
            }
        });
        let refusal: &[&str] = &["targets.hosts.selectionMode", "COUNT(", "PERCENT("];
        let id = judge(
            &router,
            case,
            body(case, targets, wait()),
            (!admitted).then_some(refusal),
        )
        .await;
        if let Some(id) = id {
            // An admitted mode is stored as sent: the service refuses the forms it does not
            // know rather than rewriting them.
            assert_eq!(
                stored(&router, case, &id).await["targets"]["hosts"]["selectionMode"].as_str(),
                Some(mode),
                "case {case:?}: expected the stored selectionMode to be the one created"
            );
        }
    }
}

/// RULE: each entry of an action's `targets` names, under a target key that action SUPPORTS,
/// a target the SAME template declares, of the resource type the action takes under that key
/// — the three things `ActionTarget` (which `GetAction` reports for every published action)
/// states about an action's targets, and none of which the free `string -> string` map can
/// express.
#[tokio::test]
async fn an_action_refers_only_to_targets_its_template_declares_and_it_supports() {
    let router = emulator();

    // Two targets of different resource types, so a row can name the right one, the wrong
    // one, or one the template never declared.
    let declared = json!({
        "hosts": {
            "resourceType": "aws:ec2:instance",
            "resourceArns": instance_arns(),
            "selectionMode": "ALL"
        },
        "roles": {
            "resourceType": "aws:iam:role",
            "resourceArns": ["arn:aws:iam::000000000000:role/fis-targeted"],
            "selectionMode": "ALL"
        }
    });
    // The parameters `aws:fis:inject-api-internal-error` records as required, so these rows
    // are refused (or admitted) on their TARGETS alone.
    let fault_parameters = json!({
        "duration": DURATION,
        "service": "ec2",
        "operations": "DescribeAvailabilityZones"
    });

    // The rows differ along the action (one supporting an EC2 instance target, one
    // supporting an IAM role target, one supporting no target at all), along what its
    // `targets` map carries (nothing, an empty map, the right reference, an unsupported
    // key, an undeclared target, a target of the wrong type) and along which of the three
    // things fails.
    for (case, action_id, parameters, targets, refusal) in [
        (
            "no targets map at all",
            "aws:ec2:stop-instances",
            json!({}),
            None,
            None,
        ),
        (
            "an empty targets map",
            "aws:ec2:stop-instances",
            json!({}),
            Some(json!({})),
            None,
        ),
        (
            "the supported key naming a target of the right type",
            "aws:ec2:stop-instances",
            json!({}),
            Some(json!({ "Instances": "hosts" })),
            None,
        ),
        (
            "another action's supported key and target type",
            "aws:fis:inject-api-internal-error",
            fault_parameters.clone(),
            Some(json!({ "Roles": "roles" })),
            None,
        ),
        (
            "a key the action does not support",
            "aws:ec2:stop-instances",
            json!({}),
            Some(json!({ "Volumes": "hosts" })),
            Some(&["actions.underTest.targets.Volumes", "aws:ec2:stop-instances"] as &[&str]),
        ),
        (
            "a key on an action that supports no target",
            "aws:fis:wait",
            json!({ "duration": DURATION }),
            Some(json!({ "Instances": "hosts" })),
            Some(&["actions.underTest.targets.Instances", "aws:fis:wait"] as &[&str]),
        ),
        (
            "a target the template does not declare",
            "aws:ec2:stop-instances",
            json!({}),
            Some(json!({ "Instances": "no-such-target" })),
            Some(&["actions.underTest.targets.Instances", "no-such-target"] as &[&str]),
        ),
        (
            "a declared target of the wrong resource type",
            "aws:ec2:stop-instances",
            json!({}),
            Some(json!({ "Instances": "roles" })),
            Some(&["actions.underTest.targets.Instances", "aws:ec2:instance"] as &[&str]),
        ),
        (
            "another action's key naming a target of the wrong type",
            "aws:fis:inject-api-internal-error",
            fault_parameters.clone(),
            Some(json!({ "Roles": "hosts" })),
            Some(&["actions.underTest.targets.Roles", "aws:iam:role"] as &[&str]),
        ),
    ] {
        let mut action = json!({ "actionId": action_id, "parameters": parameters });
        if let Some(targets) = targets {
            action["targets"] = targets;
        }
        judge(
            &router,
            case,
            body(case, declared.clone(), json!({ ACTION: action })),
            refusal,
        )
        .await;
    }
}

/// RULE: `actionId` names one of the actions FIS PUBLISHES — the catalog `GetAction` and
/// `ListActions` report from — so an id that catalog holds no action for is refused instead
/// of being stored and later started. It is a REFERENCE to a catalogued resource, so a
/// reference that resolves to nothing is reported as a missing resource (404
/// `ResourceNotFoundException`) worded after the KEY of the offending `actions` entry, and
/// not as a malformed value.
#[tokio::test]
async fn an_action_id_no_published_action_names_is_refused() {
    let router = emulator();

    // The rows differ along HOW the id misses the catalog: a service FIS has no actions
    // for, a published service with an operation it does not publish, a published id
    // differently cased, a published id with a segment missing or added, and a value in no
    // `aws:<service>:<action>` shape at all. An action id is case-sensitive.
    for (case, action_id) in [
        ("a service FIS publishes nothing for", "aws:bogus:nonsense"),
        (
            "a published service and an unpublished action",
            "aws:ec2:defenestrate-instances",
        ),
        ("a published id in upper case", "AWS:FIS:WAIT"),
        ("a published id in mixed case", "aws:fis:Wait"),
        ("a published id missing its service", "aws:wait"),
        ("a published id with a segment added", "aws:fis:wait:now"),
        ("no qualified id at all", "wait"),
        ("an empty-ish id", "-"),
    ] {
        let before = template_count(&router).await;
        let answer = create_template(
            &router,
            body(
                case,
                json!({}),
                json!({ ACTION: { "actionId": action_id, "parameters": { "duration": DURATION } } }),
            ),
        )
        .await;
        assert_unresolved_action(case, &answer, ACTION);
        assert!(
            !message(&answer).contains(action_id),
            "case {case:?}: the refusal must not echo the unresolvable id, got {:?}",
            answer.body
        );
        assert_eq!(
            template_count(&router).await,
            before,
            "case {case:?}: a refused create must persist no template"
        );
    }
}

/// The parameters a catalogued action needs, with every required one supplied.
fn required_parameters(required: &[&str]) -> Value {
    let mut parameters = Map::new();
    for name in required {
        let value = if *name == "duration" {
            DURATION
        } else {
            PARAMETER_VALUE
        };
        parameters.insert((*name).to_string(), json!(value));
    }
    Value::Object(parameters)
}

/// RULE: an action supplies every parameter the published catalog records as `required` for
/// it — `ActionParameter$required`, which `GetAction` reports for every action FIS
/// publishes, so the rule is the same for all of them and not only for the built-in wait.
///
/// Driven from the catalog itself, so this covers every action the service publishes: each
/// one is admitted with its required parameters supplied, and refused once per required
/// parameter with exactly that one left out.
#[tokio::test]
async fn every_published_action_needs_the_parameters_its_contract_requires() {
    for entry in ACTION_CATALOG {
        // One emulator per catalogued action, so each action's rows are judged against a
        // store holding nothing but their own template.
        let router = emulator();
        let required: Vec<&str> = entry
            .parameters
            .iter()
            .filter(|parameter| parameter.required)
            .map(|parameter| parameter.name)
            .collect();

        let case = format!("{} with every required parameter", entry.id);
        let id = judge(
            &router,
            &case,
            body(
                &case,
                json!({}),
                json!({ ACTION: {
                    "actionId": entry.id,
                    "parameters": required_parameters(&required)
                } }),
            ),
            None,
        )
        .await
        .unwrap_or_else(|| panic!("case {case:?}: an admitted create reports an id"));
        let template = stored(&router, &case, &id).await;
        assert_eq!(
            template["actions"][ACTION]["actionId"].as_str(),
            Some(entry.id),
            "case {case:?}: expected the stored actionId to be the one created, got {template:?}"
        );
        for name in &required {
            assert!(
                template["actions"][ACTION]["parameters"][*name].is_string(),
                "case {case:?}: expected the stored action to carry {name:?}, got {template:?}"
            );
        }

        for omitted in &required {
            let case = format!("{} without {omitted}", entry.id);
            let kept: Vec<&str> = required
                .iter()
                .copied()
                .filter(|name| name != omitted)
                .collect();
            judge(
                &router,
                &case,
                body(
                    &case,
                    json!({}),
                    json!({ ACTION: {
                        "actionId": entry.id,
                        "parameters": required_parameters(&kept)
                    } }),
                ),
                Some(&[
                    "actions.underTest.parameters",
                    omitted,
                    entry.id,
                ]),
            )
            .await;
        }
    }
}

/// RULE (the one every rule above belongs to): these are rules about a TEMPLATE, not about
/// the call that produced it, so `UpdateExperimentTemplate` answers a template of the same
/// shape exactly as `CreateExperimentTemplate` did — the same status, the same error code
/// and the same message.
#[tokio::test]
async fn the_same_rules_judge_an_updated_template() {
    // One row per rule, each in the shape an update carries: the members it replaces,
    // beside the class that rule's refusal belongs to (HTTP status and wire error code).
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a target naming both resourceArns and resourceTags",
            json!({
                "targets": {
                    "hosts": {
                        "resourceType": "aws:ec2:instance",
                        "resourceArns": instance_arns(),
                        "resourceTags": { "Stage": "test" },
                        "selectionMode": "ALL"
                    }
                }
            }),
            400,
            VALIDATION,
        ),
        (
            "a target naming neither",
            json!({
                "targets": {
                    "hosts": { "resourceType": "aws:ec2:instance", "selectionMode": "ALL" }
                }
            }),
            400,
            VALIDATION,
        ),
        (
            "a selection mode in no documented form",
            json!({
                "targets": {
                    "hosts": {
                        "resourceType": "aws:ec2:instance",
                        "resourceArns": instance_arns(),
                        "selectionMode": "SOME"
                    }
                }
            }),
            400,
            VALIDATION,
        ),
        (
            "an action id no published action names",
            json!({ "actions": { ACTION: { "actionId": "aws:bogus:nonsense" } } }),
            404,
            NOT_FOUND,
        ),
        (
            "an action missing a required parameter",
            json!({ "actions": { ACTION: { "actionId": "aws:ebs:pause-volume-io" } } }),
            400,
            VALIDATION,
        ),
        (
            "an action naming a target the template does not declare",
            json!({
                "targets": {},
                "actions": {
                    ACTION: {
                        "actionId": "aws:ec2:stop-instances",
                        "targets": { "Instances": "no-such-target" }
                    }
                }
            }),
            400,
            VALIDATION,
        ),
    ];

    for (case, members, status, code) in rows {
        let router = emulator();

        // The create's answer, over a template built from those same members.
        let mut created_body = body(case, json!({}), wait());
        let Some(members_map) = members.as_object() else {
            panic!("case {case:?}: a row's members are a JSON object");
        };
        for (name, value) in members_map {
            created_body[name] = value.clone();
        }
        let created = create_template(&router, created_body).await;
        assert_refused_as(case, &created, status, code, &[]);

        // A baseline template the update replaces those members on.
        let baseline = create_template(
            &router,
            body(&format!("baseline {case}"), json!({}), wait()),
        )
        .await;
        assert_eq!(
            baseline.status, 200,
            "case {case:?}: expected the baseline template to be created, got {} with {:?}",
            baseline.status, baseline.body
        );
        let before = baseline.member("experimentTemplate").clone();
        let id = before["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no id on the baseline {before:?}"))
            .to_string();

        let updated = send(
            &router,
            "PATCH",
            &format!("/experimentTemplates/{id}"),
            Some(members.clone()),
            FALLBACK_REGION,
        )
        .await;
        assert_refused_as(case, &updated, status, code, &[]);
        assert_eq!(
            message(&updated),
            message(&created),
            "case {case:?}: the two calls must refuse the same template with the same message"
        );

        // A refused update leaves the stored template exactly as it was.
        assert_eq!(
            stored(&router, case, &id).await,
            before,
            "case {case:?}: a refused update must change nothing"
        );
    }
}
