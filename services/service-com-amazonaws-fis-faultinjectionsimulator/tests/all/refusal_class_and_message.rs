// SPDX-License-Identifier: CC0-1.0

//! WHICH error class FIS answers a refused request with, and in WHOSE words, end to end
//! over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler and serialization — the same path a client takes. The
//! rules under test are the ones that decide a refusal's class and its text, each asserted
//! over a table that spans the class of inputs it generalises over rather than one request:
//!
//! * a value FIS judges against a CATALOG it publishes is input: a `resourceType` no
//!   catalogued entry matches — including one no `TargetResourceTypeId` pattern admits — is
//!   a 400 `ValidationException` naming the type that was looked up, while an `actionId`
//!   that names no published action is a REFERENCE to a resource that does not exist and is
//!   answered 404 `ResourceNotFoundException`;
//! * a refusal carries FIS's own sentence rather than a report of the modelled constraint
//!   that was crossed: a `maxResults` outside its member's published `@range` names the
//!   member and BOTH bounds, and echoes no value;
//! * an operation addressing a CHILD of a resource resolves the PARENT first, so the miss
//!   a caller is told about is the one that is true, in the words the operation that
//!   reported it uses;
//! * a member the service requires of a REQUEST is reported before any value that request
//!   carries is resolved against a catalog or the store.
//!
//! The HTTP harness is the lifecycle suite's, as the sibling suites use it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, send, token, Answer, FALLBACK_REGION,
};

/// A well-formed IAM role ARN: no row here is about the execution role.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-refusal-wording";

/// A value inside `RoleArn`'s modelled bounds (`@length(20..2048)`, `^[\S]+$`) that is no
/// IAM role ARN, so only a server-side rule of the template can refuse it.
const NOT_A_ROLE_ARN: &str = "arn:aws:iam::000000000000:user/not-a-role";

/// The published bounds of every `maxResults` member of this service
/// (`@range(min: 1, max: 100)` on `ListActionsMaxResults`,
/// `ListTargetResourceTypesMaxResults`, `ListExperimentTemplatesMaxResults`,
/// `ListExperimentsMaxResults`, `ListTargetAccountConfigurationsMaxResults` and
/// `ListExperimentResolvedTargetsMaxResults`), spelled out here rather than imported from
/// the crate so a wrong bound fails this suite.
const MAX_RESULTS_FLOOR: i64 = 1;
const MAX_RESULTS_CEILING: i64 = 100;

/// The sentence FIS answers a `maxResults` outside those bounds with.
fn max_results_message() -> String {
    format!("maxResults must be between {MAX_RESULTS_FLOOR} and {MAX_RESULTS_CEILING}")
}

/// The `message` an answer carries.
fn message(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert that `answer` is a refusal with HTTP `status`, error code `code` and exactly the
/// message `expected`.
fn assert_refusal(case: &str, answer: &Answer, status: u16, code: &str, expected: &str) {
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
    assert_eq!(
        message(answer),
        expected,
        "case {case:?}: expected message {expected:?}, got {:?}",
        message(answer)
    );
}

/// A create body carrying `members` over the members every create needs, `targets`
/// INCLUDED — a row that is about the absent member removes it.
fn create_body(label: &str, members: Value) -> Value {
    let mut body = json!({
        "clientToken": token(&label.replace(' ', "-")),
        "description": format!("refusal class and message: {label}"),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } } }
    });
    let (Some(body_map), Some(members)) = (body.as_object_mut(), members.as_object()) else {
        panic!("case {label:?}: a row's members are a JSON object");
    };
    for (name, value) in members {
        body_map.insert(name.clone(), value.clone());
    }
    body
}

/// An experiment template, reported by its minted id.
async fn template(router: &MultiServiceRouter, label: &str) -> String {
    let answer = create_template(router, create_body(label, json!({}))).await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no template id in {:?}", answer.body))
        .to_string()
}

/// An experiment started from a fresh template, reported by its minted id.
async fn experiment(router: &MultiServiceRouter, label: &str) -> String {
    let template_id = template(router, &format!("{label}-template")).await;
    let answer = send(
        router,
        "POST",
        "/experiments",
        Some(json!({
            "clientToken": token(&format!("{label}-start")),
            "experimentTemplateId": template_id
        })),
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: expected HTTP 200 from StartExperiment, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experiment")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no experiment id in {:?}", answer.body))
        .to_string()
}

/// RULE: a `resourceType` is judged against the catalog FIS publishes as INPUT, so a value
/// no catalogued entry matches — whatever makes it unmatchable, including a value
/// `TargetResourceTypeId`'s `^[\S]+$` does not admit — is a 400 `ValidationException`
/// naming the type that was looked up, and never a report of the pattern.
#[tokio::test]
async fn a_resource_type_no_catalogued_entry_matches_names_the_type_looked_up() {
    let router = emulator();

    // Each row is the path segment the request carries and the value that segment decodes
    // to (the message interpolates the DECODED value). They differ along why the value
    // matches nothing: a plain word, a published id differently cased or re-segmented, an
    // id of the sibling catalog, and values made only of whitespace or carrying it inside
    // — the ones the member's pattern refuses.
    for (case, segment, decoded) in [
        ("a-plain-word", "nonsense", "nonsense"),
        ("a-published-id-in-upper-case", "AWS:EC2:INSTANCE", "AWS:EC2:INSTANCE"),
        ("a-suffixed-published-id", "aws:ec2:instances", "aws:ec2:instances"),
        ("an-id-of-the-action-catalog", "aws:fis:wait", "aws:fis:wait"),
        ("one-space", "%20", " "),
        ("three-spaces", "%20%20%20", "   "),
        ("a-tab", "%09", "\t"),
        ("whitespace-inside-the-value", "aws:ec2:%20instance", "aws:ec2: instance"),
    ] {
        let answer = send(
            &router,
            "GET",
            &format!("/targetResourceTypes/{segment}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_refusal(
            case,
            &answer,
            400,
            "ValidationException",
            &format!("TargetResourceType with ID {decoded} not found."),
        );
    }
}

/// RULE: an `actionId` names a resource of the catalog, so an id no published action
/// carries is a MISSING RESOURCE — 404 `ResourceNotFoundException` — worded after the
/// member that named nothing, with the requested id not echoed.
#[tokio::test]
async fn an_action_id_no_published_action_carries_is_a_missing_resource() {
    let router = emulator();

    // The rows differ along how the id misses the catalog: an unpublished service, a
    // published service with an unpublished action, a published id differently cased or
    // re-segmented, and a value in no `aws:<service>:<action>` shape at all.
    for (case, id) in [
        ("an-unpublished-service", "aws:not-a-real-service:fake-action"),
        ("an-unpublished-action", "aws:ec2:defenestrate-instances"),
        ("a-published-id-in-upper-case", "AWS:FIS:WAIT"),
        ("a-published-id-with-a-segment-added", "aws:fis:wait:now"),
        ("no-qualified-id-at-all", "wait"),
        ("a-target-resource-type-id", "aws:ec2:instance"),
    ] {
        let answer = send(
            &router,
            "GET",
            &format!("/actions/{id}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_refusal(
            case,
            &answer,
            404,
            "ResourceNotFoundException",
            "Invalid actionId.",
        );
        assert!(
            !message(&answer).contains(id),
            "case {case:?}: the refusal must not echo the requested id, got {:?}",
            answer.body
        );
    }
}

/// RULE: a `maxResults` outside the `@range` its member declares is refused with FIS's own
/// sentence — the member and BOTH published bounds, with no value echoed — on every
/// paginated list that takes one, and a value AT either bound is served.
#[tokio::test]
async fn a_max_results_outside_its_published_range_states_both_bounds() {
    let router = emulator();
    let template_id = template(&router, "max-results-template").await;
    let experiment_id = experiment(&router, "max-results").await;

    // Every list of this service that models a `maxResults`, the two catalog listings
    // among them.
    let listings: [(&str, String); 5] = [
        ("ListActions", "/actions".to_string()),
        ("ListTargetResourceTypes", "/targetResourceTypes".to_string()),
        ("ListExperimentTemplates", "/experimentTemplates".to_string()),
        ("ListExperiments", "/experiments".to_string()),
        (
            "ListTargetAccountConfigurations",
            format!("/experimentTemplates/{template_id}/targetAccountConfigurations"),
        ),
    ];

    // The rows differ along how the value leaves the range: below the floor (at zero and
    // negative) and above the ceiling (one past it and far past it).
    let out_of_range = [
        MAX_RESULTS_FLOOR - 1,
        -1,
        MAX_RESULTS_CEILING + 1,
        1_000,
        i64::from(i32::MAX),
    ];

    for (operation, path) in &listings {
        for size in out_of_range {
            let case = format!("{operation} maxResults={size}");
            let answer = send(
                &router,
                "GET",
                &format!("{path}?maxResults={size}"),
                None,
                FALLBACK_REGION,
            )
            .await;
            assert_refusal(
                &case,
                &answer,
                400,
                "ValidationException",
                &max_results_message(),
            );
        }

        // The complement: a value at either published bound is served.
        for size in [MAX_RESULTS_FLOOR, MAX_RESULTS_CEILING] {
            let case = format!("{operation} maxResults={size}");
            let answer = send(
                &router,
                "GET",
                &format!("{path}?maxResults={size}"),
                None,
                FALLBACK_REGION,
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a value at a published bound is served, got {} with {:?}",
                answer.status, answer.body
            );
        }
    }

    // The resolved-target listing takes a `maxResults` too, and is reached through an
    // experiment, so its rows are judged against one that exists.
    let resolved = format!("/experiments/{experiment_id}/resolvedTargets");
    for size in out_of_range {
        let case = format!("ListExperimentResolvedTargets maxResults={size}");
        let answer = send(
            &router,
            "GET",
            &format!("{resolved}?maxResults={size}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_refusal(
            &case,
            &answer,
            400,
            "ValidationException",
            &max_results_message(),
        );
    }
}

/// One operation addressing a CHILD of an experiment: its name, the path it reads with
/// `{id}` standing for the experiment the request names, and the sentence FIS words that
/// operation's parent miss in.
struct ChildOperation {
    operation: &'static str,
    path: &'static str,
    parent_miss: &'static str,
}

/// RULE: an operation that reads or enumerates a CHILD of an experiment resolves the
/// PARENT first, so an `experimentId` that names no record is reported as the EXPERIMENT
/// missing — in the words that operation uses, which name no id — and a configuration is
/// reported missing only from an experiment that exists.
#[tokio::test]
async fn an_operation_on_a_child_of_an_experiment_reports_the_parent_first() {
    let router = emulator();
    let experiment_id = experiment(&router, "parent-first").await;

    // The account every child row names: a well-formed `TargetAccountId`
    // (`@length(12..48)`, `^[\S]+$`) no configuration of this experiment carries.
    let account = "111122223333";

    // Each operation addressing a child of an experiment, with the sentence FIS words its
    // parent miss in.
    let operations = [
        ChildOperation {
            operation: "ListExperimentResolvedTargets",
            path: "/experiments/{id}/resolvedTargets",
            parent_miss: "Experiment not found",
        },
        ChildOperation {
            operation: "ListExperimentTargetAccountConfigurations",
            path: "/experiments/{id}/targetAccountConfigurations",
            parent_miss: "Experiment not found.",
        },
        ChildOperation {
            operation: "GetExperimentTargetAccountConfiguration",
            path: "/experiments/{id}/targetAccountConfigurations/111122223333",
            parent_miss: "Experiment not found.",
        },
    ];

    // The unknown ids differ along their shape: one of the shape this service mints, one
    // of the template collection's shape, one that is no id at all, and one at
    // `ExperimentId`'s published 64-character ceiling.
    let at_the_ceiling = "z".repeat(64);
    let unknown = [
        "EXP000000000099",
        "EXT000000000001",
        "not-an-id",
        at_the_ceiling.as_str(),
    ];

    for child in operations {
        let operation = child.operation;
        for id in unknown {
            let case = format!("{operation} {id}");
            let answer = send(
                &router,
                "GET",
                &child.path.replace("{id}", id),
                None,
                FALLBACK_REGION,
            )
            .await;
            assert_refusal(
                &case,
                &answer,
                404,
                "ResourceNotFoundException",
                child.parent_miss,
            );
            assert!(
                !message(&answer).contains(id),
                "case {case:?}: the refusal must not echo the requested id, got {:?}",
                answer.body
            );
        }

        // The complement: the experiment that EXISTS is enumerated rather than refused.
        let case = format!("{operation} {experiment_id}");
        let answer = send(
            &router,
            "GET",
            &child.path.replace("{id}", &experiment_id),
            None,
            FALLBACK_REGION,
        )
        .await;
        let expected_status =
            if operation == "GetExperimentTargetAccountConfiguration" { 404 } else { 200 };
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for an experiment that exists, \
             got {} with {:?}",
            answer.status, answer.body
        );
    }

    // The child of an experiment that EXISTS is the child that is reported missing, in the
    // words FIS builds from the request for that miss.
    let answer = send(
        &router,
        "GET",
        &format!("/experiments/{experiment_id}/targetAccountConfigurations/{account}"),
        None,
        FALLBACK_REGION,
    )
    .await;
    assert_refusal(
        "a-configuration-missing-from-an-experiment-that-exists",
        &answer,
        404,
        "ResourceNotFoundException",
        &format!(
            "TargetAccountConfiguration for experiment {experiment_id} \
             and target account {account} not found"
        ),
    );
}

/// RULE: a member the service requires of a REQUEST — `targets` on a create, which the
/// model leaves optional — is reported as "<member> is required" before any value the
/// request carries is resolved against a catalog or the store; a request that NAMES the
/// member, even with no entry in it, earns whichever refusal its values do.
#[tokio::test]
async fn a_create_reports_the_member_it_requires_before_resolving_anything() {
    // The rows differ along what ELSE is wrong with the request: nothing, a reference to an
    // action the catalog does not publish, an execution role that is no IAM role, an action
    // order naming an action the template does not declare, and a template with no action
    // at all. Each row is judged twice: once with `targets` left out, where the absent
    // member is what is reported, and once with it named but empty, where the row's own
    // defect is.
    let rows: [(&str, Value, u16, &str, &str); 5] = [
        (
            "nothing else wrong",
            json!({}),
            200,
            "",
            "",
        ),
        (
            "an action the catalog does not publish",
            json!({ "actions": { "badAction": { "actionId": "aws:ec2:not-a-real-action" } } }),
            404,
            "ResourceNotFoundException",
            "Invalid actionId in action 'badAction'.",
        ),
        (
            "an execution role that is no iam role",
            json!({ "roleArn": NOT_A_ROLE_ARN }),
            400,
            "ValidationException",
            "roleArn",
        ),
        (
            "an order naming an action the template does not declare",
            json!({
                "actions": {
                    "second": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT2M" },
                        "startAfter": ["first"]
                    }
                }
            }),
            400,
            "ValidationException",
            "actions.second.startAfter",
        ),
        (
            "no action at all",
            json!({ "actions": {} }),
            400,
            "ValidationException",
            "actions",
        ),
    ];

    for (case, members, status, code, names) in rows {
        // One emulator per row, so no row's template can be mistaken for another's.
        let router = emulator();

        // `targets` left out: the absent member is what the caller is told about, whatever
        // else the request got wrong.
        let mut without = create_body(&format!("without-targets-{case}"), members.clone());
        without
            .as_object_mut()
            .expect("the create body is a JSON object")
            .remove("targets");
        let answer = create_template(&router, without).await;
        assert_refusal(
            case,
            &answer,
            400,
            "ValidationException",
            "targets is required",
        );

        // The member NAMED, with no entry in it: the row's own defect is what is reported,
        // and a row with no other defect is stored.
        let answer = create_template(&router, create_body(&format!("empty-targets-{case}"), members)).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} once the member is named, got {} with {:?}",
            answer.status, answer.body
        );
        if status == 200 {
            continue;
        }
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected error code {code:?}, got {:?}",
            answer.body
        );
        assert!(
            message(&answer).contains(names),
            "case {case:?}: expected the refusal to name {names:?}, got {:?}",
            answer.body
        );
    }
}

/// RULE: an `actionId` the catalog publishes no action for is a reference to a missing
/// resource wherever a template carries it, reported 404 `ResourceNotFoundException` after
/// the KEY of the offending entry in the request's `actions` map — the key, not the value,
/// and the same on the create and on the update.
#[tokio::test]
async fn an_unresolved_action_reference_names_the_entry_that_carried_it() {
    // The rows differ along the KEY of the offending entry and along the unpublished id it
    // carries, so neither the message's variable part nor the value it omits is fixed.
    for (key, action_id) in [
        ("badAction", "aws:ec2:not-a-real-action"),
        ("primary", "aws:does-not-exist:no-such-action"),
        ("step-2", "AWS:FIS:WAIT"),
        ("z", "wait"),
    ] {
        let router = emulator();
        let case = format!("{key} -> {action_id}");
        let expected = format!("Invalid actionId in action '{key}'.");
        let actions = json!({ key: { "actionId": action_id, "parameters": { "duration": "PT5M" } } });

        let created = create_template(
            &router,
            create_body(&format!("create-{key}"), json!({ "actions": actions })),
        )
        .await;
        assert_refusal(
            &case,
            &created,
            404,
            "ResourceNotFoundException",
            &expected,
        );
        assert!(
            !message(&created).contains(action_id),
            "case {case:?}: the refusal must not echo the unresolvable id, got {:?}",
            created.body
        );

        // The rule belongs to the TEMPLATE, so an update that would carry the same entry
        // earns the same refusal, and leaves the stored template as it was.
        let id = template(&router, &format!("baseline-{key}")).await;
        let updated = send(
            &router,
            "PATCH",
            &format!("/experimentTemplates/{id}"),
            Some(json!({ "actions": actions })),
            FALLBACK_REGION,
        )
        .await;
        assert_refusal(
            &case,
            &updated,
            404,
            "ResourceNotFoundException",
            &expected,
        );

        let stored = send(
            &router,
            "GET",
            &format!("/experimentTemplates/{id}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            stored.member("experimentTemplate")["actions"]["wait"]["actionId"].as_str(),
            Some("aws:fis:wait"),
            "case {case:?}: a refused update must leave the stored actions alone, got {:?}",
            stored.body
        );
    }
}
