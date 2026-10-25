// SPDX-License-Identifier: CC0-1.0

//! `UpdateExperimentDefinition` as a PATCH over the stored experiment definition.
//!
//! The rule under test: a member `UpdateExperimentDefinitionRequest` carries replaces the
//! stored one, a member it omits keeps the value the definition already holds, a supplied
//! `Control` or `Treatments` replaces the treatment or the whole list (re-minting the
//! service's keys from each treatment's place in the definition), the record's
//! `UpdatedAt` moves to the write instant while `CreatedAt` stays put, and everything the
//! request does not carry — `Id`, `Name`, `FlagKey`, the resolved application,
//! environment and configuration profile ids, and `Status` — is reported back out of the
//! stored record untouched. Both path identifiers resolve a resource by its ID **or** its
//! name, an identifier that resolves to nothing is HTTP 404 `ResourceNotFoundException`,
//! and a definition with an experiment run in flight (the one that reports `ACTIVE`) is
//! HTTP 409 `ConflictException` — every refusal writing nothing.
//!
//! Every case drives real HTTP requests through the router the emulator's front door
//! uses, and each update is read back through `GetExperimentDefinition`, so what is
//! asserted is what an AppConfig client sees on the wire rather than what one handler
//! happened to answer.
//!
//! The tables span what the rule generalises over rather than one recorded request:
//! WHICH subset of the writable members a patch carries, the VALUE each carries at the
//! published bounds (`Description` len 0..1024, `Rule` len 1..16384 and
//! `TreatmentInputList` len 1..5, from
//! `smithy_query(query="operation",operation="UpdateExperimentDefinition")`) and one past
//! them, how the treatment list changes length (shrinking, growing, at the modelled
//! maximum), whether the resources are addressed by id or by name, and which level of the
//! path is unresolvable.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_appconfig_amazonappconfig::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{create_application, json_call, router, router_on, Answer};
use crate::experiment_definitions::{
    assert_treatment_body, create_definition, flag, get_definition, scope, treatment, Scope,
};
use crate::resource_tags::{filler, pct};

/// The definition every row starts from: every optional member is already set, so a row
/// that omits one is distinguishable from a row that replaces it.
const BASELINE_HYPOTHESIS: &str = "the hypothesis as created";
/// See [`BASELINE_HYPOTHESIS`].
const BASELINE_AUDIENCE_RULE: &str = "attribute(\"tier\") == \"beta\"";
/// See [`BASELINE_HYPOTHESIS`].
const BASELINE_AUDIENCE_DESCRIPTION: &str = "the audience as created";
/// See [`BASELINE_HYPOTHESIS`].
const BASELINE_LAUNCH_CRITERIA: &str = "the launch criteria as created";
/// See [`BASELINE_HYPOTHESIS`].
const BASELINE_NAME: &str = "the-definition-as-created";

/// What a case expects the definition's writable members to hold afterwards. The
/// treatments are held in the `TreatmentInput` form a request writes them in; the key the
/// service mints is asserted separately, from each treatment's place in the list.
#[derive(Clone, Debug)]
struct Expected {
    hypothesis: Option<String>,
    audience_rule: String,
    audience_description: Option<String>,
    launch_criteria: Option<String>,
    treatments: Vec<Value>,
    control: Value,
}

impl Expected {
    fn with_hypothesis(&self, hypothesis: &str) -> Self {
        Self {
            hypothesis: Some(hypothesis.to_string()),
            ..self.clone()
        }
    }

    fn with_audience_rule(&self, audience_rule: &str) -> Self {
        Self {
            audience_rule: audience_rule.to_string(),
            ..self.clone()
        }
    }

    fn with_audience_description(&self, audience_description: &str) -> Self {
        Self {
            audience_description: Some(audience_description.to_string()),
            ..self.clone()
        }
    }

    fn with_launch_criteria(&self, launch_criteria: &str) -> Self {
        Self {
            launch_criteria: Some(launch_criteria.to_string()),
            ..self.clone()
        }
    }

    fn with_treatments(&self, treatments: Vec<Value>) -> Self {
        Self {
            treatments,
            ..self.clone()
        }
    }

    fn with_control(&self, control: Value) -> Self {
        Self {
            control,
            ..self.clone()
        }
    }
}

/// The members no `UpdateExperimentDefinitionRequest` carries, as the create reported
/// them: every one of them must read back unchanged after any number of updates.
#[derive(Clone, Debug)]
struct Fixed {
    id: String,
    name: String,
    flag_key: String,
    application_id: String,
    environment_id: String,
    configuration_profile_id: String,
    status: String,
    created_at: Value,
}

/// `PATCH /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}`,
/// this operation's modelled URI.
async fn update_definition(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    patch: &Value,
) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!(
            "/applications/{}/experimentdefinitions/{}",
            pct(application),
            pct(definition)
        ),
        patch,
    )
    .await
}

/// The request body a baseline definition is created from, with every optional member
/// set and three treatments beside the control.
fn baseline_body(scope: &Scope) -> Value {
    json!({
        "Name": BASELINE_NAME,
        "ConfigurationProfileIdentifier": scope.profile,
        "EnvironmentIdentifier": scope.environment,
        "FlagKey": "checkout-flow",
        "AudienceRule": BASELINE_AUDIENCE_RULE,
        "Hypothesis": BASELINE_HYPOTHESIS,
        "AudienceDescription": BASELINE_AUDIENCE_DESCRIPTION,
        "LaunchCriteria": BASELINE_LAUNCH_CRITERIA,
        "Treatments": [
            treatment(25.5, Some("the first variation"), flag(true)),
            treatment(24.5, Some("the second variation"), flag(false)),
            treatment(0.0, None, flag(true)),
        ],
        "Control": treatment(50.0, Some("today's experience"), flag(false)),
    })
}

/// Create the baseline definition and answer what the update table starts from.
async fn create_baseline(
    router: &MultiServiceRouter,
    scope: &Scope,
) -> (Fixed, Expected, Value) {
    let request = baseline_body(scope);
    let created = create_definition(router, &scope.application, &request).await;
    assert_eq!(
        created.status,
        201,
        "the definition under test must be created, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let body = created.json();
    let member = |name: &str| {
        body.get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("CreateExperimentDefinition answered no {name}: {body}"))
            .to_string()
    };
    let fixed = Fixed {
        id: member("Id"),
        name: member("Name"),
        flag_key: member("FlagKey"),
        application_id: member("ApplicationId"),
        environment_id: member("EnvironmentId"),
        configuration_profile_id: member("ConfigurationProfileId"),
        status: member("Status"),
        created_at: body
            .get("CreatedAt")
            .cloned()
            .unwrap_or_else(|| panic!("CreateExperimentDefinition answered no CreatedAt: {body}")),
    };
    let expected = Expected {
        hypothesis: Some(BASELINE_HYPOTHESIS.to_string()),
        audience_rule: BASELINE_AUDIENCE_RULE.to_string(),
        audience_description: Some(BASELINE_AUDIENCE_DESCRIPTION.to_string()),
        launch_criteria: Some(BASELINE_LAUNCH_CRITERIA.to_string()),
        treatments: request
            .get("Treatments")
            .and_then(Value::as_array)
            .expect("the baseline request carries Treatments")
            .clone(),
        control: request
            .get("Control")
            .cloned()
            .expect("the baseline request carries a Control"),
    };
    (fixed, expected, body)
}

/// A string member of a JSON body.
fn text(body: &Value, member: &str) -> Option<String> {
    body.get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert that `answer` is a 200 describing `expected` under the members of `fixed`, none
/// of which an update can reach.
fn assert_definition(
    case: &str,
    source: &str,
    answer: &Answer,
    fixed: &Fixed,
    expected: &Expected,
) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?} ({source}): expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let body = answer.json();

    for (member, want) in [
        ("Hypothesis", expected.hypothesis.clone()),
        ("AudienceRule", Some(expected.audience_rule.clone())),
        ("AudienceDescription", expected.audience_description.clone()),
        ("LaunchCriteria", expected.launch_criteria.clone()),
    ] {
        assert_eq!(
            text(&body, member),
            want,
            "case {case:?} ({source}): expected {member} {want:?}, got {:?}",
            body.get(member)
        );
    }

    for (member, want) in [
        ("Id", &fixed.id),
        ("Name", &fixed.name),
        ("FlagKey", &fixed.flag_key),
        ("ApplicationId", &fixed.application_id),
        ("EnvironmentId", &fixed.environment_id),
        ("ConfigurationProfileId", &fixed.configuration_profile_id),
        ("Status", &fixed.status),
    ] {
        assert_eq!(
            text(&body, member).as_deref(),
            Some(want.as_str()),
            "case {case:?} ({source}): {member} is not a member of this request, expected \
             {want:?}, got {:?}",
            body.get(member)
        );
    }
    assert_eq!(
        body.get("CreatedAt"),
        Some(&fixed.created_at),
        "case {case:?} ({source}): expected CreatedAt {:?}, got {:?}",
        fixed.created_at,
        body.get("CreatedAt")
    );

    // The control keeps the key the service mints for it, and the compared treatments are
    // keyed from their place in the stored list — contiguously from `T1`, whatever the
    // list held before.
    let control = body
        .get("Control")
        .unwrap_or_else(|| panic!("case {case:?} ({source}): no Control in {body}"));
    assert_eq!(
        text(control, "Key").as_deref(),
        Some("C"),
        "case {case:?} ({source}): expected the control key C, got {:?}",
        control.get("Key")
    );
    assert_treatment_body(case, "Control", &expected.control, control);

    let treatments = body
        .get("Treatments")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?} ({source}): no Treatments in {body}"));
    assert_eq!(
        treatments.len(),
        expected.treatments.len(),
        "case {case:?} ({source}): expected {} treatments, got {}",
        expected.treatments.len(),
        treatments.len()
    );
    for (index, (want, got)) in expected.treatments.iter().zip(treatments).enumerate() {
        let key = format!("T{}", index + 1);
        assert_eq!(
            text(got, "Key").as_deref(),
            Some(key.as_str()),
            "case {case:?} ({source}): expected treatment {index} keyed {key:?}, got {:?}",
            got.get("Key")
        );
        assert_treatment_body(case, &key, want, got);
    }

    // An update is a write, so the record reports when it was last written.
    assert!(
        body.get("UpdatedAt").is_some(),
        "case {case:?} ({source}): expected an UpdatedAt, got {body}"
    );
}

/// Read the definition back through `GetExperimentDefinition` and assert the same
/// expectation: the update was persisted, not merely reported.
async fn assert_round_trip(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    fixed: &Fixed,
    expected: &Expected,
) {
    let fetched = get_definition(router, application, &fixed.id).await;
    assert_definition(
        case,
        "GetExperimentDefinition",
        &fetched,
        fixed,
        expected,
    );
}

/// An instant a response reports, as seconds since the epoch, whichever wire form the
/// `Iso8601DateTime` timestamp is written in. `pub(crate)` because more than one suite in
/// this target asserts against an experiment definition's write instants.
pub(crate) fn instant(case: &str, label: &str, value: Option<&Value>) -> f64 {
    match value {
        Some(Value::String(written)) => DateTime::from_str(written, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {label} {written:?} is not an ISO 8601 date-time: {error}")
            })
            .as_secs_f64(),
        Some(Value::Number(written)) => written.as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: {label} {written:?} is not a representable instant")
        }),
        other => panic!("case {case:?}: expected a {label} timestamp, got {other:?}"),
    }
}

/// Each patch writes the members it carries, leaves every other member of the record
/// alone, and what it leaves behind is what `GetExperimentDefinition` answers.
#[tokio::test]
async fn an_update_replaces_only_the_members_the_request_carries() {
    let router = router();
    let scope = scope(&router, "patch-table").await;
    let (fixed, baseline, _) = create_baseline(&router, &scope).await;

    // The published bounds of the writable members: `Description` len 0..1024 (the three
    // description-shaped members) and `Rule` len 1..16384 (`AudienceRule`).
    let longest_description = filler(1024, 3);
    let longest_rule = filler(16384, 5);

    // Rows differ along which members the patch carries, the value each carries (empty, at
    // a published bound, non-ASCII), and how the treatment list changes: shrunk to the
    // modelled minimum of one, grown to the modelled maximum of five, and replaced
    // alongside the control. Each row's expectation is relative to the row before it,
    // which is what makes "an omitted member keeps its stored value" observable.
    let mut expected = baseline.clone();
    let cases: Vec<(&str, Value, Expected)> = vec![
        (
            "hypothesis only",
            json!({ "Hypothesis": "the new flow converts better" }),
            expected.with_hypothesis("the new flow converts better"),
        ),
        (
            "audience rule only",
            json!({ "AudienceRule": "attribute(\"tier\") == \"gold\"" }),
            expected
                .with_hypothesis("the new flow converts better")
                .with_audience_rule("attribute(\"tier\") == \"gold\""),
        ),
        (
            "no writable member at all",
            json!({}),
            expected
                .with_hypothesis("the new flow converts better")
                .with_audience_rule("attribute(\"tier\") == \"gold\""),
        ),
        (
            "descriptions replaced by the empty string at their lower bound",
            json!({ "Hypothesis": "", "AudienceDescription": "", "LaunchCriteria": "" }),
            expected
                .with_audience_rule("attribute(\"tier\") == \"gold\"")
                .with_hypothesis("")
                .with_audience_description("")
                .with_launch_criteria(""),
        ),
        (
            "every description at its 1024-character upper bound",
            json!({
                "Hypothesis": longest_description,
                "AudienceDescription": longest_description,
                "LaunchCriteria": longest_description,
            }),
            expected
                .with_audience_rule("attribute(\"tier\") == \"gold\"")
                .with_hypothesis(&longest_description)
                .with_audience_description(&longest_description)
                .with_launch_criteria(&longest_description),
        ),
        (
            "an audience rule at its 1-character lower bound",
            json!({ "AudienceRule": "t" }),
            expected
                .with_audience_rule("t")
                .with_hypothesis(&longest_description)
                .with_audience_description(&longest_description)
                .with_launch_criteria(&longest_description),
        ),
        (
            "an audience rule at its 16384-character upper bound",
            json!({ "AudienceRule": longest_rule }),
            expected
                .with_audience_rule(&longest_rule)
                .with_hypothesis(&longest_description)
                .with_audience_description(&longest_description)
                .with_launch_criteria(&longest_description),
        ),
        (
            "non-ASCII text in every description and the rule",
            json!({
                "Hypothesis": "ユーザーの転換率が上がる",
                "AudienceDescription": "βeta — \"quoted\" & <escaped>",
                "LaunchCriteria": "🚀 ship it",
                "AudienceRule": "attribute(\"país\") == \"españa\"",
            }),
            expected
                .with_hypothesis("ユーザーの転換率が上がる")
                .with_audience_description("βeta — \"quoted\" & <escaped>")
                .with_launch_criteria("🚀 ship it")
                .with_audience_rule("attribute(\"país\") == \"españa\""),
        ),
        (
            "the treatment list shrunk to the modelled minimum of one",
            json!({ "Treatments": [treatment(100.0, Some("the only variation"), flag(true))] }),
            expected
                .with_hypothesis("ユーザーの転換率が上がる")
                .with_audience_description("βeta — \"quoted\" & <escaped>")
                .with_launch_criteria("🚀 ship it")
                .with_audience_rule("attribute(\"país\") == \"españa\"")
                .with_treatments(vec![treatment(
                    100.0,
                    Some("the only variation"),
                    flag(true),
                )]),
        ),
        (
            "the treatment list grown to the modelled maximum of five, and a new control",
            json!({
                "Treatments": (1..=5)
                    .map(|index| treatment(
                        f64::from(index) * 1.5,
                        Some(&format!("variation {index}")),
                        flag(index % 2 == 0),
                    ))
                    .collect::<Vec<Value>>(),
                "Control": treatment(12.25, None, flag(true)),
            }),
            expected
                .with_hypothesis("ユーザーの転換率が上がる")
                .with_audience_description("βeta — \"quoted\" & <escaped>")
                .with_launch_criteria("🚀 ship it")
                .with_audience_rule("attribute(\"país\") == \"españa\"")
                .with_treatments(
                    (1..=5)
                        .map(|index| {
                            treatment(
                                f64::from(index) * 1.5,
                                Some(&format!("variation {index}")),
                                flag(index % 2 == 0),
                            )
                        })
                        .collect(),
                )
                .with_control(treatment(12.25, None, flag(true))),
        ),
        (
            "a control whose description is dropped",
            json!({ "Control": treatment(0.0, None, flag(false)) }),
            expected
                .with_hypothesis("ユーザーの転換率が上がる")
                .with_audience_description("βeta — \"quoted\" & <escaped>")
                .with_launch_criteria("🚀 ship it")
                .with_audience_rule("attribute(\"país\") == \"españa\"")
                .with_treatments(
                    (1..=5)
                        .map(|index| {
                            treatment(
                                f64::from(index) * 1.5,
                                Some(&format!("variation {index}")),
                                flag(index % 2 == 0),
                            )
                        })
                        .collect(),
                )
                .with_control(treatment(0.0, None, flag(false))),
        ),
    ];

    for (case, patch, after) in cases {
        let updated = update_definition(&router, &scope.application, &fixed.id, &patch).await;
        assert_definition(
            case,
            "UpdateExperimentDefinition",
            &updated,
            &fixed,
            &after,
        );
        assert_round_trip(&router, case, &scope.application, &fixed, &after).await;
        expected = after;
    }

    // The last row is still what the record holds once the table is done, and the
    // baseline's values are long gone.
    assert_round_trip(
        &router,
        "the record after the whole table",
        &scope.application,
        &fixed,
        &expected,
    )
    .await;
}

/// Both path identifiers address a resource by its ID or by its name, so the same patch
/// reaches the same record however it is written.
#[tokio::test]
async fn an_update_is_addressed_by_id_or_by_name() {
    let router = router();
    let scope = scope(&router, "addressing").await;
    let (fixed, baseline, _) = create_baseline(&router, &scope).await;
    let application_name = "app-addressing".to_string();

    // Rows differ along which form each of the two identifiers takes.
    let addressings: Vec<(&str, String, String)> = vec![
        (
            "application id and definition id",
            scope.application.clone(),
            fixed.id.clone(),
        ),
        (
            "application id and definition name",
            scope.application.clone(),
            fixed.name.clone(),
        ),
        (
            "application name and definition id",
            application_name.clone(),
            fixed.id.clone(),
        ),
        (
            "application name and definition name",
            application_name,
            fixed.name.clone(),
        ),
    ];

    let mut expected = baseline;
    for (case, application, definition) in addressings {
        let hypothesis = format!("written through the {case}");
        let after = expected.with_hypothesis(&hypothesis);
        let updated = update_definition(
            &router,
            &application,
            &definition,
            &json!({ "Hypothesis": hypothesis }),
        )
        .await;
        assert_definition(
            case,
            "UpdateExperimentDefinition",
            &updated,
            &fixed,
            &after,
        );
        assert_round_trip(&router, case, &scope.application, &fixed, &after).await;
        expected = after;
    }
}

/// `UpdatedAt` is the instant the record was last written and `CreatedAt` the instant it
/// was created: an update moves the first and leaves the second where it was.
#[tokio::test]
async fn an_update_moves_updated_at_and_leaves_created_at_alone() {
    let router = router();
    let scope = scope(&router, "timestamps").await;
    let (fixed, baseline, created) = create_baseline(&router, &scope).await;

    let created_at = instant("as created", "CreatedAt", created.get("CreatedAt"));
    let mut previous = instant("as created", "UpdatedAt", created.get("UpdatedAt"));

    // Rows differ along which member the patch carries, including one that carries none:
    // the record is written either way.
    for (case, patch) in [
        ("a patched hypothesis", json!({ "Hypothesis": "revised" })),
        (
            "a patched treatment list",
            json!({ "Treatments": [treatment(100.0, None, flag(true))] }),
        ),
        ("no writable member at all", json!({})),
    ] {
        // The two write instants must be far enough apart to be distinguishable in the
        // reported timestamp, which is what this short pause buys.
        std::thread::sleep(Duration::from_millis(10));
        let updated = update_definition(&router, &scope.application, &fixed.id, &patch).await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );
        let body = updated.json();
        let updated_at = instant(case, "UpdatedAt", body.get("UpdatedAt"));
        assert!(
            updated_at > previous,
            "case {case:?}: expected UpdatedAt past {previous}, got {updated_at}"
        );
        assert!(
            updated_at >= created_at,
            "case {case:?}: expected UpdatedAt at or after CreatedAt {created_at}, got \
             {updated_at}"
        );
        assert_eq!(
            instant(case, "CreatedAt", body.get("CreatedAt")),
            created_at,
            "case {case:?}: expected CreatedAt to stay {created_at}, got {:?}",
            body.get("CreatedAt")
        );

        // The reader reports the same instants the update answered.
        let fetched = get_definition(&router, &scope.application, &fixed.id).await;
        let read_back = fetched.json();
        assert_eq!(
            read_back.get("UpdatedAt"),
            body.get("UpdatedAt"),
            "case {case:?}: expected GetExperimentDefinition to report UpdatedAt {:?}, got {:?}",
            body.get("UpdatedAt"),
            read_back.get("UpdatedAt")
        );
        previous = updated_at;
    }

    // Nothing above reached a member of the record an update cannot write.
    assert_round_trip(
        &router,
        "the fixed members after the timestamp table",
        &scope.application,
        &fixed,
        &baseline
            .with_hypothesis("revised")
            .with_treatments(vec![treatment(100.0, None, flag(true))]),
    )
    .await;
}

/// An identifier that resolves to no resource is HTTP 404 `ResourceNotFoundException`,
/// and the refusal writes nothing: neither the addressed record (there is none) nor the
/// definition that does exist moves.
#[tokio::test]
async fn an_update_is_not_found_for_an_identifier_that_resolves_to_nothing() {
    let router = router();
    let scope = scope(&router, "misses").await;
    let (fixed, baseline, _) = create_baseline(&router, &scope).await;
    // A second application, so a definition can be addressed under the wrong parent.
    let other_application = create_application(&router, "app-misses-other").await;

    // Rows differ along WHICH level of the path is unresolvable and WHY: an application
    // nothing answers to, a definition nothing answers to, a definition that exists but
    // under another application, and an id that belongs to another resource kind.
    for (case, application, definition) in [
        (
            "an application identifier nothing answers to",
            "nosuchapp".to_string(),
            fixed.id.clone(),
        ),
        (
            "a definition identifier nothing answers to",
            scope.application.clone(),
            "zzzz99".to_string(),
        ),
        (
            "a definition name nothing answers to",
            scope.application.clone(),
            "no-such-definition".to_string(),
        ),
        (
            "a definition that exists under another application",
            other_application.clone(),
            fixed.id.clone(),
        ),
        (
            "an environment id used as a definition identifier",
            scope.application.clone(),
            scope.environment.clone(),
        ),
    ] {
        let answer = update_definition(
            &router,
            &application,
            &definition,
            &json!({ "Hypothesis": "must not be stored" }),
        )
        .await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
        // The definition that does exist still holds exactly what the create stored.
        assert_round_trip(&router, case, &scope.application, &fixed, &baseline).await;
    }
}

/// A value outside a modelled constraint is refused with HTTP 400 and writes nothing.
#[tokio::test]
async fn an_update_past_a_modelled_bound_is_refused_without_writing() {
    let router = router();
    let scope = scope(&router, "bounds").await;
    let (fixed, baseline, _) = create_baseline(&router, &scope).await;

    // One past each published bound: `Description` max 1024, `Rule` min 1 and max 16384,
    // `TreatmentInputList` min 1 and max 5.
    for (case, patch) in [
        (
            "a hypothesis one character past its upper bound",
            json!({ "Hypothesis": filler(1025, 1) }),
        ),
        (
            "a launch criteria one character past its upper bound",
            json!({ "LaunchCriteria": filler(1025, 2) }),
        ),
        (
            "an audience rule below its 1-character lower bound",
            json!({ "AudienceRule": "" }),
        ),
        (
            "an audience rule one character past its upper bound",
            json!({ "AudienceRule": filler(16385, 4) }),
        ),
        (
            "an empty treatment list, below the modelled minimum of one",
            json!({ "Treatments": [] }),
        ),
        (
            "six treatments, one past the modelled maximum of five",
            json!({
                "Treatments": (1..=6)
                    .map(|index| treatment(f64::from(index), None, flag(true)))
                    .collect::<Vec<Value>>(),
            }),
        ),
    ] {
        let refused = update_definition(&router, &scope.application, &fixed.id, &patch).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_round_trip(&router, case, &scope.application, &fixed, &baseline).await;
    }
}

/// A definition cannot be updated while an experiment run is active: the update is HTTP
/// 409 `ConflictException` and writes nothing, and the same patch is accepted once the
/// definition is no longer `ACTIVE`.
///
/// The status is moved in the state the handlers share because the operation that starts a
/// run is still a scaffold stub, so there is no request that can make a definition
/// `ACTIVE` yet; everything else here goes over the wire.
#[tokio::test]
async fn an_update_conflicts_while_the_definition_is_active() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let scope = scope(&router, "conflict").await;
    let (fixed, baseline, _) = create_baseline(&router, &scope).await;

    let set_status = |status: &str| {
        let state = state::service_state(&*engine)
            .expect("the test engine must carry this service's state");
        let mut store = state.lock();
        let application = store
            .applications
            .get_mut(&fixed.application_id)
            .expect("the application under test must be stored");
        let definition = application
            .experiment_definitions
            .get_mut(&fixed.id)
            .expect("the definition under test must be stored");
        definition.status = status.to_string();
    };

    // Rows differ along which members the refused patch carries: none of them is written
    // while a run is in flight.
    set_status(state::EXPERIMENT_DEFINITION_ACTIVE);
    let active = Fixed {
        status: state::EXPERIMENT_DEFINITION_ACTIVE.to_string(),
        ..fixed.clone()
    };
    for (case, patch) in [
        (
            "a hypothesis while a run is active",
            json!({ "Hypothesis": "must not be stored" }),
        ),
        (
            "a treatment list while a run is active",
            json!({ "Treatments": [treatment(100.0, None, flag(false))] }),
        ),
        (
            "every writable member while a run is active",
            json!({
                "Hypothesis": "nor this",
                "AudienceRule": "false",
                "AudienceDescription": "nor this",
                "LaunchCriteria": "nor this",
                "Control": treatment(1.0, None, flag(true)),
                "Treatments": [treatment(99.0, None, flag(true))],
            }),
        ),
        ("no writable member at all", json!({})),
    ] {
        let refused = update_definition(&router, &scope.application, &fixed.id, &patch).await;
        assert_eq!(
            refused.status,
            409,
            "case {case:?}: expected 409, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?}",
            refused.error_code()
        );
        assert_round_trip(&router, case, &scope.application, &active, &baseline).await;
    }

    // With no run in flight the very same patch is accepted, so what was refused was the
    // definition's state and nothing else about the request.
    set_status(state::EXPERIMENT_DEFINITION_IDLE);
    let case = "the same patch once no run is active";
    let after = baseline.with_hypothesis("stored once the run is over");
    let updated = update_definition(
        &router,
        &scope.application,
        &fixed.id,
        &json!({ "Hypothesis": "stored once the run is over" }),
    )
    .await;
    assert_definition(
        case,
        "UpdateExperimentDefinition",
        &updated,
        &fixed,
        &after,
    );
    assert_round_trip(&router, case, &scope.application, &fixed, &after).await;
}
