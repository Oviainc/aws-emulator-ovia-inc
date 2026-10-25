// SPDX-License-Identifier: CC0-1.0

//! `DeleteExperimentDefinition`: the two deletions the operation offers.
//!
//! The rule under test, in one sentence: a delete that asks for the archiving
//! `DeleteType` keeps the stored definition and moves its status to the archived one,
//! while any other delete removes the record outright — and either way the call answers
//! the modelled 204 with no body, resolves each path identifier by the resource's ID or
//! its name, and refuses with `ResourceNotFoundException` (HTTP 404) when either
//! identifier resolves to nothing, writing nothing when it refuses.
//!
//! So an archived definition is still readable, with every member but its status and its
//! write instant exactly as it was, and a removed one is readable nowhere: the reader,
//! the writer and a second removing delete all miss on it, and the name it held is free
//! for a new definition again.
//!
//! Every case drives real HTTP requests through the router the emulator's front door
//! uses, and every delete is read back through `GetExperimentDefinition`, so what is
//! asserted is what an AppConfig client sees on the wire rather than what one handler
//! happened to answer.
//!
//! The tables span what the rule generalises over rather than one recorded request: each
//! `DeleteType` the model's enum declares and the request that names none
//! (`smithy_query(query="shape",shape="DeleteType")` — `ARCHIVE` | `DESTROY`, the member
//! being optional with no modelled default), the status the record holds when the delete
//! arrives (each value of `ExperimentDefinitionStatus`), the shape of the definition being
//! deleted (the number of treatments across the modelled `TreatmentInputList` bounds 1..5
//! and whether the optional descriptions are present), whether each resource is addressed
//! by id or by name, which level of the path is unresolvable, and `DeleteType` values
//! outside the enum.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_appconfig_amazonappconfig::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    account, create_application, json_call, router_on, send, Answer,
};
use crate::experiment_definitions::{create_definition, flag, get_definition, scope, treatment, Scope};
use crate::resource_deletes::{assert_deleted, assert_not_found};
use crate::resource_tags::pct;
use crate::update_experiment_definition::instant;

/// The weights each row's treatments carry, so a row can take the first `n` of them and
/// span the modelled `TreatmentInputList` bounds (1..5) without computing anything.
const WEIGHTS: [f64; 5] = [50.0, 25.0, 12.5, 7.5, 5.0];

/// The label [`scope`] builds its resource names from, per suite, and the application name
/// it therefore gives the application — which a row addressing the application BY NAME
/// needs.
fn application_name(label: &str) -> String {
    format!("app-{label}")
}

/// `DELETE /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}`,
/// this operation's modelled URI, with the `delete_type` query the model binds
/// `DeleteType` to when the row names one.
async fn delete_definition(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    delete_type: Option<&str>,
) -> Answer {
    let query = delete_type
        .map(|value| format!("?delete_type={}", pct(value)))
        .unwrap_or_default();
    send(
        router,
        "DELETE",
        &format!(
            "/applications/{}/experimentdefinitions/{}{query}",
            pct(application),
            pct(definition)
        ),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// A create request body for a definition carrying `treatments` treatments beside its
/// control, and the optional descriptions when `descriptions` asks for them.
fn definition_body(scope: &Scope, name: &str, treatments: usize, descriptions: bool) -> Value {
    let listed: Vec<Value> = WEIGHTS[..treatments]
        .iter()
        .enumerate()
        .map(|(index, weight)| {
            treatment(
                *weight,
                Some(&format!("variation {}", index + 1)),
                flag(index % 2 == 0),
            )
        })
        .collect();
    let mut body = json!({
        "Name": name,
        "ConfigurationProfileIdentifier": scope.profile,
        "EnvironmentIdentifier": scope.environment,
        "FlagKey": "checkout-flow",
        "AudienceRule": "attribute(\"tier\") == \"beta\"",
        "Treatments": listed,
        "Control": treatment(0.0, None, flag(false)),
    });
    if descriptions {
        body["Hypothesis"] = Value::from("the new flow converts better");
        body["AudienceDescription"] = Value::from("beta testers only");
        body["LaunchCriteria"] = Value::from("a 5% lift over the control");
    }
    body
}

/// Create one definition under `scope` and answer its minted id.
async fn create(
    router: &MultiServiceRouter,
    case: &str,
    scope: &Scope,
    name: &str,
    treatments: usize,
    descriptions: bool,
) -> String {
    let created = create_definition(
        router,
        &scope.application,
        &definition_body(scope, name, treatments, descriptions),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: the definition under test must be created, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: CreateExperimentDefinition answered no Id"))
}

/// Move a stored definition's status, for the rows that need a definition the wire cannot
/// put in that status yet: the operation that starts an experiment run is still a scaffold
/// stub, so no request makes a definition `ACTIVE`. Everything else in this suite goes
/// over the wire.
fn set_status(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    application_id: &str,
    definition_id: &str,
    status: &str,
) {
    let state =
        state::service_state(&**engine).expect("the test engine must carry this service's state");
    let mut store = state.lock();
    let application = store
        .application_mut(application_id)
        .unwrap_or_else(|| panic!("case {case:?}: the application under test must be stored"));
    let definition = application
        .experiment_definitions
        .get_mut(definition_id)
        .unwrap_or_else(|| panic!("case {case:?}: the definition under test must be stored"));
    definition.status = status.to_string();
}

/// The definition `GetExperimentDefinition` reports, as a 200 body.
async fn read_definition(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    definition: &str,
) -> Value {
    let answer = get_definition(router, application, definition).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: GetExperimentDefinition must still report the definition, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// Every member of `before` except those in `moved` is reported in `after` unchanged, and
/// `after` carries no member `before` did not: an archiving delete PRESERVES the record.
fn assert_preserved(case: &str, before: &Value, after: &Value, moved: &[&str]) {
    let before_members = before
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object, got {before}"));
    let after_members = after
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object, got {after}"));
    for (member, want) in before_members {
        if moved.contains(&member.as_str()) {
            continue;
        }
        assert_eq!(
            after_members.get(member),
            Some(want),
            "case {case:?}: expected {member} to be preserved as {want:?}, got {:?}",
            after_members.get(member)
        );
    }
    for member in after_members.keys() {
        assert!(
            before_members.contains_key(member),
            "case {case:?}: {member:?} appeared on the archived record, which held {before}"
        );
    }
}

/// An archiving delete keeps the definition and moves it to the archived status: it stays
/// readable, with every other member exactly as it was.
#[tokio::test]
async fn an_archiving_delete_preserves_the_definition_and_reports_it_archived() {
    const LABEL: &str = "archive";
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let scope = scope(&router, LABEL).await;
    let application_name = application_name(LABEL);

    // Rows differ along the shape of the definition being archived (treatment count across
    // the modelled 1..5, descriptions present or absent), the status the record holds when
    // the delete arrives (every `ExperimentDefinitionStatus`, including one already
    // archived, which is what makes the call idempotent), and whether the request addresses
    // the application and the definition by id or by name.
    for (index, (case, treatments, descriptions, status_before, by_name)) in [
        (
            "one treatment, no descriptions, idle, addressed by id",
            1,
            false,
            state::EXPERIMENT_DEFINITION_IDLE,
            false,
        ),
        (
            "three treatments and every description, idle, addressed by name",
            3,
            true,
            state::EXPERIMENT_DEFINITION_IDLE,
            true,
        ),
        (
            "five treatments, the modelled maximum, idle, addressed by id",
            5,
            false,
            state::EXPERIMENT_DEFINITION_IDLE,
            false,
        ),
        (
            "a definition with an experiment run in flight, addressed by id",
            2,
            true,
            state::EXPERIMENT_DEFINITION_ACTIVE,
            false,
        ),
        (
            "a definition that is already archived, addressed by name",
            1,
            false,
            state::EXPERIMENT_DEFINITION_ARCHIVED,
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("archived-{index}");
        let id = create(&router, case, &scope, &name, treatments, descriptions).await;
        if status_before != state::EXPERIMENT_DEFINITION_IDLE {
            set_status(&engine, case, &scope.application, &id, status_before);
        }
        let (application, definition) = if by_name {
            (application_name.clone(), name.clone())
        } else {
            (scope.application.clone(), id.clone())
        };

        let before = read_definition(&router, case, &scope.application, &id).await;
        assert_eq!(
            before.get("Status").and_then(Value::as_str),
            Some(status_before),
            "case {case:?}: the row must start from {status_before:?}, got {:?}",
            before.get("Status")
        );

        let archived = delete_definition(
            &router,
            &application,
            &definition,
            Some(state::DELETE_TYPE_ARCHIVE),
        )
        .await;
        assert_deleted(case, "DeleteExperimentDefinition", &archived);

        let after = read_definition(&router, case, &scope.application, &id).await;
        assert_eq!(
            after.get("Status").and_then(Value::as_str),
            Some(state::EXPERIMENT_DEFINITION_ARCHIVED),
            "case {case:?}: expected the archived status, got {:?}",
            after.get("Status")
        );
        assert_preserved(case, &before, &after, &["Status", "UpdatedAt"]);
        // Archiving wrote the record, so it reports a write instant at or after the one it
        // was created at, and `CreatedAt` itself has not moved.
        assert!(
            instant(case, "UpdatedAt", after.get("UpdatedAt"))
                >= instant(case, "CreatedAt", after.get("CreatedAt")),
            "case {case:?}: expected UpdatedAt at or after CreatedAt, got {:?} and {:?}",
            after.get("UpdatedAt"),
            after.get("CreatedAt")
        );

        // The operation is idempotent: archiving an archived definition answers the same
        // 204 and leaves the same record behind.
        let again = delete_definition(
            &router,
            &application,
            &definition,
            Some(state::DELETE_TYPE_ARCHIVE),
        )
        .await;
        assert_deleted(case, "a repeated archiving DeleteExperimentDefinition", &again);
        let twice = read_definition(&router, case, &scope.application, &id).await;
        assert_eq!(
            twice.get("Status").and_then(Value::as_str),
            Some(state::EXPERIMENT_DEFINITION_ARCHIVED),
            "case {case:?}: expected the archived status after a repeated archive, got {:?}",
            twice.get("Status")
        );
        assert_preserved(case, &after, &twice, &["UpdatedAt"]);
    }
}

/// A delete that is not an archiving one removes the record: nothing reads it afterwards,
/// a second such delete misses, every other definition is untouched, and the name it held
/// is free again.
#[tokio::test]
async fn a_permanent_delete_removes_the_definition() {
    const LABEL: &str = "destroy";
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let scope = scope(&router, LABEL).await;
    let application_name = application_name(LABEL);

    // Rows differ along whether the request names the destroying `DeleteType` or names
    // none at all (the member is optional and carries no modelled default), the status the
    // record holds when the delete arrives, the shape of the definition, and whether the
    // request addresses the resources by id or by name.
    for (index, (case, delete_type, treatments, descriptions, status_before, by_name)) in [
        (
            "the destroying DeleteType, idle, addressed by id",
            Some(state::DELETE_TYPE_DESTROY),
            1,
            false,
            state::EXPERIMENT_DEFINITION_IDLE,
            false,
        ),
        (
            "no DeleteType at all, idle, addressed by id",
            None,
            2,
            true,
            state::EXPERIMENT_DEFINITION_IDLE,
            false,
        ),
        (
            "the destroying DeleteType while a run is in flight, addressed by name",
            Some(state::DELETE_TYPE_DESTROY),
            3,
            true,
            state::EXPERIMENT_DEFINITION_ACTIVE,
            true,
        ),
        (
            "no DeleteType at all on an already archived definition, addressed by name",
            None,
            1,
            false,
            state::EXPERIMENT_DEFINITION_ARCHIVED,
            true,
        ),
        (
            "the destroying DeleteType, five treatments, addressed by id",
            Some(state::DELETE_TYPE_DESTROY),
            5,
            false,
            state::EXPERIMENT_DEFINITION_IDLE,
            false,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("destroyed-{index}");
        let id = create(&router, case, &scope, &name, treatments, descriptions).await;
        // A sibling under the same application: a delete reaches ONE record.
        let sibling_name = format!("sibling-{index}");
        let sibling = create(&router, case, &scope, &sibling_name, 1, false).await;
        if status_before != state::EXPERIMENT_DEFINITION_IDLE {
            set_status(&engine, case, &scope.application, &id, status_before);
        }
        let (application, definition) = if by_name {
            (application_name.clone(), name.clone())
        } else {
            (scope.application.clone(), id.clone())
        };

        let removed = delete_definition(&router, &application, &definition, delete_type).await;
        assert_deleted(case, "DeleteExperimentDefinition", &removed);

        // Every caller that reads the map the delete took the record out of now misses on
        // it, by id and by name alike.
        for addressed in [id.clone(), name.clone()] {
            assert_not_found(
                case,
                "GetExperimentDefinition",
                &get_definition(&router, &scope.application, &addressed).await,
            );
            assert_not_found(
                case,
                "UpdateExperimentDefinition",
                &json_call(
                    &router,
                    "PATCH",
                    &format!(
                        "/applications/{}/experimentdefinitions/{}",
                        pct(&scope.application),
                        pct(&addressed)
                    ),
                    &json!({ "Hypothesis": "must not be stored" }),
                )
                .await,
            );
            assert_not_found(
                case,
                "a second DeleteExperimentDefinition",
                &delete_definition(&router, &scope.application, &addressed, delete_type).await,
            );
        }

        // Nothing else the store holds was disturbed.
        let kept = read_definition(&router, case, &scope.application, &sibling).await;
        assert_eq!(
            kept.get("Name").and_then(Value::as_str),
            Some(sibling_name.as_str()),
            "case {case:?}: expected the sibling definition {sibling_name:?} to survive, got {kept}"
        );

        // The removed definition held its name, and nothing holds it now, so a new
        // definition may take it.
        let remade = create_definition(
            &router,
            &scope.application,
            &definition_body(&scope, &name, 1, false),
        )
        .await;
        assert_eq!(
            remade.status,
            201,
            "case {case:?}: expected the name {name:?} to be free again, got {} / {}",
            remade.status,
            String::from_utf8_lossy(&remade.body)
        );
        assert_ne!(
            remade.string("Id").as_deref(),
            Some(id.as_str()),
            "case {case:?}: the re-created definition must be a new record, not {id:?}"
        );
    }
}

/// An identifier that resolves to no resource is HTTP 404 `ResourceNotFoundException`,
/// whichever deletion was asked for, and the refusal writes nothing.
#[tokio::test]
async fn a_delete_that_resolves_nothing_refuses_and_changes_nothing() {
    const LABEL: &str = "misses";
    let router = router_on(&Arc::new(engine::InMemoryEngine::new()));
    let scope = scope(&router, LABEL).await;
    let name = "the-definition-that-stays";
    let id = create(&router, "the definition under test", &scope, name, 2, true).await;
    // A second application, so a definition can be addressed under the wrong parent.
    let other_application = create_application(&router, &format!("{LABEL}-other")).await;
    let before = read_definition(&router, "before the table", &scope.application, &id).await;

    // Rows differ along WHICH level of the path is unresolvable and why: an application
    // nothing answers to, a definition nothing answers to, and a definition that exists
    // but under another application. Each is tried for every deletion the operation
    // offers, including the request that names none.
    for (case, application, definition) in [
        (
            "an application identifier nothing answers to",
            "nosuchapp".to_string(),
            id.clone(),
        ),
        (
            "a definition identifier nothing answers to",
            scope.application.clone(),
            "nosuchdef".to_string(),
        ),
        (
            "a definition name nothing answers to",
            scope.application.clone(),
            "no-such-definition".to_string(),
        ),
        (
            "a definition that exists under another application, addressed by id",
            other_application.clone(),
            id.clone(),
        ),
        (
            "a definition that exists under another application, addressed by name",
            other_application.clone(),
            name.to_string(),
        ),
    ] {
        for delete_type in [
            Some(state::DELETE_TYPE_ARCHIVE),
            Some(state::DELETE_TYPE_DESTROY),
            None,
        ] {
            let labelled = format!("{case} with DeleteType {delete_type:?}");
            let refused =
                delete_definition(&router, &application, &definition, delete_type).await;
            assert_not_found(&labelled, "DeleteExperimentDefinition", &refused);
            let after = read_definition(&router, &labelled, &scope.application, &id).await;
            assert_eq!(
                after, before,
                "case {labelled:?}: a refused delete must write nothing, expected {before}, got \
                 {after}"
            );
        }
    }
}

/// The refusal every experiment-definition lookup renders: this service states the kind
/// it looked up, the identifiers the request carried under the labels of the members they
/// came from, and the account the lookup was scoped to.
#[tokio::test]
async fn a_definition_that_resolves_to_nothing_is_refused_with_its_kinds_template() {
    const LABEL: &str = "templates";
    let router = router_on(&Arc::new(engine::InMemoryEngine::new()));
    let scope = scope(&router, LABEL).await;
    let account = account();

    // Rows differ along the length and form of the identifier that addresses nothing:
    // `ExperimentDefinitionIdentifier` targets `Identifier` (len 1..2048, no pattern), so
    // each of these reaches the lookup rather than being refused as malformed.
    for unknown in [
        "zzzzzzz",
        "nosuch1",
        "ab12",
        "a",
        "a-definition-nobody-ever-created",
    ] {
        let expected = format!(
            "Experiment Definition with Application Identifier {} and Experiment Definition \
             Identifier {unknown} could not be found for account {account}.",
            scope.application
        );
        // Every operation that looks a definition up states the same template.
        let path = format!(
            "/applications/{}/experimentdefinitions/{}",
            pct(&scope.application),
            pct(unknown)
        );
        let answers = [
            (
                "GetExperimentDefinition",
                json_call(&router, "GET", &path, &Value::Null).await,
            ),
            (
                "UpdateExperimentDefinition",
                json_call(&router, "PATCH", &path, &json!({ "Hypothesis": "x" })).await,
            ),
            (
                "DeleteExperimentDefinition",
                delete_definition(&router, &scope.application, unknown, None).await,
            ),
            (
                "an archiving DeleteExperimentDefinition",
                delete_definition(
                    &router,
                    &scope.application,
                    unknown,
                    Some(state::DELETE_TYPE_ARCHIVE),
                )
                .await,
            ),
        ];
        for (operation, answer) in answers {
            let case = format!("{operation} on definition {unknown:?}");
            assert_not_found(&case, operation, &answer);
            assert_eq!(
                answer.string("Message").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {:?}",
                answer.string("Message")
            );
        }
    }
}

/// A `DeleteType` outside the values the model's enum declares is refused with HTTP 400,
/// and the refusal writes nothing.
#[tokio::test]
async fn a_delete_type_outside_the_modelled_enum_is_refused_and_changes_nothing() {
    const LABEL: &str = "bad-type";
    let router = router_on(&Arc::new(engine::InMemoryEngine::new()));
    let scope = scope(&router, LABEL).await;
    let id = create(&router, "the definition under test", &scope, "kept", 2, true).await;
    let before = read_definition(&router, "before the table", &scope.application, &id).await;

    // The enum declares `ARCHIVE` and `DESTROY`
    // (`smithy_query(query="shape",shape="DeleteType")`); rows differ along how a value
    // misses them: the wrong case, a plausible synonym, and a value from neither end.
    for case in [
        "archive",
        "Archive",
        "destroy",
        "PERMANENT",
        "ARCHIVE,DESTROY",
    ] {
        let refused = delete_definition(&router, &scope.application, &id, Some(case)).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400 for a DeleteType outside the modelled enum, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        let after = read_definition(&router, case, &scope.application, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete must write nothing, expected {before}, got {after}"
        );
    }
}
