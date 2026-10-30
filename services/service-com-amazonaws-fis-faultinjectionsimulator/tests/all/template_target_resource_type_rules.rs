// SPDX-License-Identifier: CC0-1.0

//! The published target-resource-type contract an experiment template's TARGETS are judged
//! by, end to end over HTTP.
//!
//! The rules under test, neither of them expressible as a modelled constraint —
//! `resourceType` is a free `TargetResourceTypeId` string and `parameters` a free
//! `string -> string` map — so each one is the service's own, refused with a 400
//! `ValidationException` that persists nothing:
//!
//! * `resourceType` names one of the target resource types the service PUBLISHES (the same
//!   table `ListTargetResourceTypes` and `GetTargetResourceType` report from), so an id that
//!   table holds no entry for — exactly the id `GetTargetResourceType` refuses with its own
//!   `ValidationException` — is refused rather than stored;
//! * a target supplies every parameter the published entry for its resource type marks
//!   `required` (`TargetResourceTypeParameter$required`, "indicates whether the parameter is
//!   required"), because a target without one names a resolution the published contract
//!   cannot identify.
//!
//! What the rows span: every resource type the service publishes — not a sample, since the
//! rows are driven from the catalog itself, which is how the types no issue cited are
//! covered — each admitted with its required parameters supplied and refused once per
//! required parameter with exactly that one left out, with no `parameters` member at all and
//! with an empty one; and, for the catalog miss, ids that depart from a published one in
//! every way an id can (an unpublished service, a published service with an unpublished
//! type, a published id differently cased, one with a segment missing or added, one in no
//! qualified shape at all).
//!
//! The two halves of the one table are checked against each other: for every row the
//! template rule admits a resource type for, `GetTargetResourceType` reports that type with
//! the same `required` flags, and for every id the template rule refuses,
//! `GetTargetResourceType` answers 400.
//!
//! The rules belong to the TEMPLATE and not to the call that produced it, so the last test
//! drives rows of each kind through `UpdateExperimentTemplate` and asserts it answers
//! exactly as `CreateExperimentTemplate` did.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::state::TARGET_RESOURCE_TYPE_CATALOG;
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, send, token, Answer, FALLBACK_REGION,
};

/// A well-formed IAM role ARN: every row here is about a target's resource type, not the
/// execution role.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-target-resource-type-rules";

/// The wire error code every refusal in this suite carries.
const VALIDATION: &str = "ValidationException";

/// The name every row gives the one target under test.
const TARGET: &str = "underTest";

/// The value a row supplies for a required target parameter. `ExperimentTemplateTargetParameterValue`
/// is modelled `@length(0..1024)`, `pattern ^[\S]+$`, which this satisfies; no rule here is
/// about what a parameter's value MEANS.
const PARAMETER_VALUE: &str = "placeholder";

/// The `duration` the one action every row carries supplies: inside the 1 to 720 minutes FIS
/// documents for `aws:fis:wait`.
const DURATION: &str = "PT5M";

/// An ARN for a resource of `resource_type`, built from the type's own
/// `aws:<service>:<kind>` id so each row names a resource of the type its target declares.
fn resource_arn(resource_type: &str) -> String {
    let mut segments = resource_type.split(':').skip(1);
    let service = segments.next().unwrap_or("fis");
    let kind = segments.next().unwrap_or("resource");
    format!("arn:aws:{service}:{FALLBACK_REGION}:000000000000:{kind}/under-test")
}

/// The one target a row declares: of `resource_type`, identified by one ARN, scoped to every
/// resource, and carrying `parameters` when the row supplies a map.
fn targets(resource_type: &str, parameters: Option<Value>) -> Value {
    let mut target = Map::new();
    target.insert("resourceType".to_string(), json!(resource_type));
    target.insert(
        "resourceArns".to_string(),
        json!([resource_arn(resource_type)]),
    );
    target.insert("selectionMode".to_string(), json!("ALL"));
    if let Some(parameters) = parameters {
        target.insert("parameters".to_string(), parameters);
    }
    json!({ TARGET: Value::Object(target) })
}

/// A create body carrying `targets`, with every other required member filled in admissibly.
/// The action is the built-in wait, which takes no target, so each row is judged on its
/// target alone.
fn body(label: &str, targets: Value) -> Value {
    json!({
        "clientToken": token(&label.replace([' ', ':', '(', ')'], "-")),
        "description": format!("target resource type rules: {label}"),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": targets,
        "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": DURATION } } }
    })
}

/// A map carrying `names`, each with the placeholder value.
fn parameter_map(names: &[&str]) -> Value {
    let mut parameters = Map::new();
    for name in names {
        parameters.insert((*name).to_string(), json!(PARAMETER_VALUE));
    }
    Value::Object(parameters)
}

/// The `message` of a refused answer.
fn message(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// How many experiment templates the store holds. Every test here creates far fewer than one
/// page, so a single page is the whole store.
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

/// Assert `answer` refuses the call with a 400 `ValidationException` naming every fragment in
/// `names`.
fn assert_refused(case: &str, answer: &Answer, names: &[&str]) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        VALIDATION,
        "case {case:?}: expected error code {VALIDATION:?}, got {:?}",
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

/// Create `request` and, when the row is refused, assert the refusal and that the store grew
/// by nothing; when it is admitted, assert the store reports back what it stored. Returns the
/// stored template's id for an admitted row.
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

/// What `GetTargetResourceType` reports for `resource_type`.
async fn get_resource_type(router: &MultiServiceRouter, resource_type: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/targetResourceTypes/{resource_type}"),
        None,
        FALLBACK_REGION,
    )
    .await
}

/// The `required` flag `GetTargetResourceType` reports for each parameter of
/// `resource_type`, or `None` when it reports no such type.
async fn published_parameters(
    router: &MultiServiceRouter,
    resource_type: &str,
) -> Option<BTreeMap<String, bool>> {
    let answer = get_resource_type(router, resource_type).await;
    if answer.status != 200 {
        return None;
    }
    let reported = answer.member("targetResourceType")["parameters"].clone();
    let members = reported.as_object()?;
    Some(
        members
            .iter()
            .map(|(name, parameter)| {
                (
                    name.clone(),
                    parameter["required"].as_bool().unwrap_or_default(),
                )
            })
            .collect(),
    )
}

/// RULE: a target supplies every parameter the published entry for its resource type marks
/// `required` — the same table `GetTargetResourceType` reports from decides it, so the rule
/// holds for every resource type the service publishes and not only for the ones an issue
/// named.
///
/// Driven from the catalog itself: each published type is admitted with its required
/// parameters supplied, and refused once per required parameter with exactly that one left
/// out, with no `parameters` member at all and with an empty one. Each type's published
/// report is checked to carry the very `required` flags these rows are driven from, so the
/// two halves of the one table cannot drift apart.
#[tokio::test]
async fn every_published_resource_type_needs_the_parameters_its_contract_requires() {
    for entry in TARGET_RESOURCE_TYPE_CATALOG {
        // One emulator per published type, so each type's rows are judged against a store
        // holding nothing but their own templates.
        let router = emulator();
        let required: Vec<&str> = entry
            .parameters
            .iter()
            .filter(|parameter| parameter.required)
            .map(|parameter| parameter.name)
            .collect();

        // The rows below are the published contract's, so the report has to agree with them.
        let published = published_parameters(&router, entry.id)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: GetTargetResourceType must report a published resource type",
                    entry.id
                )
            });
        let published_required: Vec<&str> = published
            .iter()
            .filter(|(_, required)| **required)
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            published_required, required,
            "case {:?}: expected GetTargetResourceType to report the required parameters the \
             template rule applies, got {published:?}",
            entry.id
        );

        let case = format!("{} with every required parameter", entry.id);
        let supplied = (!required.is_empty()).then(|| parameter_map(&required));
        let id = judge(
            &router,
            &case,
            body(&case, targets(entry.id, supplied.clone())),
            None,
        )
        .await
        .unwrap_or_else(|| panic!("case {case:?}: an admitted create reports an id"));
        let template = stored(&router, &case, &id).await;
        assert_eq!(
            template["targets"][TARGET]["resourceType"].as_str(),
            Some(entry.id),
            "case {case:?}: expected the stored resourceType to be the one created, got \
             {template:?}"
        );
        for name in &required {
            assert_eq!(
                template["targets"][TARGET]["parameters"][*name].as_str(),
                Some(PARAMETER_VALUE),
                "case {case:?}: expected the stored target to carry {name:?}, got {template:?}"
            );
        }

        // A type that requires nothing is admitted whether it names parameters or not; one
        // that requires some is refused for the FIRST required parameter it does not carry,
        // which is the one the published entry records first.
        let Some(first) = required.first() else {
            for (label, parameters) in [
                ("no parameters member at all", None),
                ("an empty parameters map", Some(json!({}))),
            ] {
                let case = format!("{} with {label}", entry.id);
                judge(
                    &router,
                    &case,
                    body(&case, targets(entry.id, parameters)),
                    None,
                )
                .await;
            }
            continue;
        };
        let missing: &[&str] = &["targets.underTest.parameters", first, entry.id];
        for (label, parameters) in [
            ("no parameters member at all", None),
            ("an empty parameters map", Some(json!({}))),
        ] {
            let case = format!("{} with {label}", entry.id);
            judge(
                &router,
                &case,
                body(&case, targets(entry.id, parameters)),
                Some(missing),
            )
            .await;
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
                body(&case, targets(entry.id, Some(parameter_map(&kept)))),
                Some(&["targets.underTest.parameters", omitted, entry.id]),
            )
            .await;
        }
    }
}

/// The ids no published resource type names, and how each one departs from a published one.
///
/// SOURCE for the published ids these are varied from: the service's own catalog, which
/// `ListTargetResourceTypes` and `GetTargetResourceType` report from. An id is
/// case-sensitive, so a differently cased published id names no type.
const UNPUBLISHED: &[(&str, &str)] = &[
    ("a service FIS publishes no type for", "aws:bogus:nonsense"),
    (
        "a published service and an unpublished type",
        "aws:ec2:placement-group",
    ),
    ("a published id in upper case", "AWS:EC2:INSTANCE"),
    ("a published id in mixed case", "aws:ec2:Instance"),
    ("a published id missing its service", "aws:instance"),
    ("a published id with a segment added", "aws:ec2:instance:now"),
    ("no qualified id at all", "instance"),
    ("an empty-ish id", "-"),
];

/// RULE: `resourceType` names one of the target resource types the service publishes, so an
/// id its published table holds no entry for is refused when the template is created instead
/// of being stored as a target no resolution could identify.
///
/// The same miss on the same table is what `GetTargetResourceType` refuses with
/// `ValidationException`, so each row asserts both halves.
#[tokio::test]
async fn a_resource_type_the_service_publishes_no_entry_for_is_refused() {
    let router = emulator();

    for (case, resource_type) in UNPUBLISHED {
        judge(
            &router,
            case,
            body(case, targets(resource_type, None)),
            Some(&["targets.underTest.resourceType", resource_type]),
        )
        .await;

        // A target of a type the published table holds nothing for carries parameters in
        // vain: there is no contract for them to satisfy, so the refusal is the same.
        let case = format!("{case} carrying parameters");
        judge(
            &router,
            &case,
            body(
                &case,
                targets(resource_type, Some(parameter_map(&["cluster", "service"]))),
            ),
            Some(&["targets.underTest.resourceType", resource_type]),
        )
        .await;

        let answer = get_resource_type(&router, resource_type).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from GetTargetResourceType over the same id, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}

/// RULE (the one both rules above belong to): these are rules about a TEMPLATE, not about the
/// call that produced it, so `UpdateExperimentTemplate` answers a target of the same shape
/// exactly as `CreateExperimentTemplate` did — the same status, the same error code and the
/// same message — and an update that brings a target within the contract is stored.
#[tokio::test]
async fn the_same_resource_type_rules_judge_an_updated_template() {
    // One row per way a target departs from the published contract, each in the shape an
    // update carries: the members it replaces.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a resource type no published entry names",
            json!({ "targets": targets("aws:bogus:nonsense", None) }),
        ),
        (
            "a published type with no parameters at all",
            json!({ "targets": targets("aws:ecs:task", None) }),
        ),
        (
            "a published type missing one required parameter",
            json!({ "targets": targets("aws:eks:pod", Some(parameter_map(&["clusterIdentifier", "namespace"]))) }),
        ),
    ];

    for (case, members) in rows {
        let router = emulator();

        // The create's answer over a template carrying those same members.
        let created = create_template(&router, body(case, members["targets"].clone())).await;
        assert_refused(case, &created, &[]);

        // A baseline template the update replaces the targets of.
        let baseline = create_template(
            &router,
            body(&format!("baseline {case}"), targets("aws:ec2:instance", None)),
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
        assert_refused(case, &updated, &[]);
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

        // The same update, with the target brought within the published contract, is stored.
        let admissible = json!({
            "targets": targets(
                "aws:ecs:task",
                Some(parameter_map(&["cluster", "service"])),
            )
        });
        let accepted = send(
            &router,
            "PATCH",
            &format!("/experimentTemplates/{id}"),
            Some(admissible.clone()),
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected the corrected update to be stored, got {} with {:?}",
            accepted.status, accepted.body
        );
        let after = stored(&router, case, &id).await;
        assert_eq!(
            after["targets"][TARGET]["resourceType"].as_str(),
            Some("aws:ecs:task"),
            "case {case:?}: expected the stored target to name the updated resourceType, got \
             {after:?}"
        );
        for name in ["cluster", "service"] {
            assert_eq!(
                after["targets"][TARGET]["parameters"][name].as_str(),
                Some(PARAMETER_VALUE),
                "case {case:?}: expected the stored target to carry {name:?}, got {after:?}"
            );
        }
    }
}
