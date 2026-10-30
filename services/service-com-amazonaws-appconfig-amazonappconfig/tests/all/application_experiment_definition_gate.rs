// SPDX-License-Identifier: CC0-1.0

//! The child gate an application's delete applies to the EXPERIMENT DEFINITIONS under it.
//!
//! The rule under test, in one sentence: AppConfig will not take an application out of its
//! tree while any resource the application owns still exists, and an experiment definition
//! is one of those children, so while a stored definition is still there
//! `DeleteApplication` is refused with `BadRequestException` (HTTP 400) naming that kind of
//! child, writes nothing, and only answers its modelled 204 once every child — of every
//! kind — has been removed.
//!
//! Two consequences this suite pins beside the refusal itself:
//!
//! * the refusal is taken BEFORE anything is removed, so the application, its definitions
//!   and the run history under them are exactly as readable and as listable after it as
//!   before — which is what lets a client delete the definition and then the application,
//!   the teardown order AppConfig asks for, and have both answer 204;
//! * the gate is on child EXISTENCE and not on a child's own status: an ARCHIVED
//!   definition is still a stored, readable record, so it is still in the way.
//!
//! Every case drives real HTTP requests through the router the emulator's front door uses,
//! so what is asserted is what an AppConfig client sees on the wire.
//!
//! The table spans what the rule generalises over rather than one recorded application:
//! how many definitions are left under it (one, two, three), the status each of them holds
//! when the delete arrives (`IDLE`, `ARCHIVED`, and `IDLE` again after a run of it has been
//! started and stopped, so the record carries run history), and — in the second test —
//! which kinds of child are present together, which is what fixes the kind the refusal
//! names.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    account, create_environment, create_profile, json_call, router, send, Answer,
};
use crate::experiment_definitions::{body, flag, get_definition, scope, treatment, Scope};
use crate::resource_deletes::{assert_deleted, assert_gated};
use crate::resource_tags::pct;

/// `DELETE` one URI with no body, which is how every delete here is driven.
async fn delete(router: &MultiServiceRouter, uri: &str) -> Answer {
    send(router, "DELETE", uri, "application/json", Vec::new(), &[]).await
}

/// The `DeleteType` that REMOVES an experiment definition's record, as the model's
/// `DeleteType` enum spells it (`smithy_query(query="shape",shape="DeleteType")` —
/// `ARCHIVE` | `DESTROY`): a delete that asks for neither preserves the record, so a row
/// that needs the definition gone names this one.
const DESTROY: &str = "DESTROY";

/// The `Message` AppConfig refuses an application's delete with while `children` of that
/// kind are still under it.
fn gate_message(application: &str, children: &str) -> String {
    format!(
        "Cannot delete application {application}, because there are still {children} existing \
         under it."
    )
}

/// `DELETE /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}`,
/// with the `delete_type` query the model binds `DeleteType` to when one is named.
async fn delete_definition(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    delete_type: Option<&str>,
) -> Answer {
    let query = delete_type
        .map(|value| format!("?delete_type={}", pct(value)))
        .unwrap_or_default();
    delete(
        router,
        &format!(
            "/applications/{}/experimentdefinitions/{}{query}",
            pct(application),
            pct(definition)
        ),
    )
    .await
}

/// The `Id` of every definition `ListExperimentDefinitions` serves under one application,
/// sorted so a row compares sets rather than the order a listing happens to use. The
/// listing is account-wide with the application named in the `application_identifier`
/// query (`smithy_query(query="operation",operation="ListExperimentDefinitions")`).
async fn listed_definitions(router: &MultiServiceRouter, application: &str) -> Vec<String> {
    let answer = json_call(
        router,
        "GET",
        &format!(
            "/experimentdefinitions?application_identifier={}",
            pct(application)
        ),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "ListExperimentDefinitions must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let mut ids: Vec<String> = answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item.get("Id").and_then(Value::as_str).map(str::to_string))
        .collect();
    ids.sort();
    ids
}

/// Create one definition under `scope` and answer its minted id.
async fn given_definition(
    router: &MultiServiceRouter,
    case: &str,
    scope: &Scope,
    name: &str,
) -> String {
    let created = crate::experiment_definitions::create_definition(
        router,
        &scope.application,
        &body(
            scope,
            name,
            vec![treatment(50.0, None, flag(true))],
            treatment(50.0, None, flag(false)),
        ),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateExperimentDefinition must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: CreateExperimentDefinition answered no Id"))
}

/// Take the environment and the configuration profile out of `scope`'s application, so the
/// only children left under it are its experiment definitions.
async fn remove_the_other_children(router: &MultiServiceRouter, case: &str, scope: &Scope) {
    assert_deleted(
        case,
        "DeleteConfigurationProfile",
        &delete(
            router,
            &format!(
                "/applications/{}/configurationprofiles/{}",
                scope.application, scope.profile
            ),
        )
        .await,
    );
    assert_deleted(
        case,
        "DeleteEnvironment",
        &delete(
            router,
            &format!(
                "/applications/{}/environments/{}",
                scope.application, scope.environment
            ),
        )
        .await,
    );
}

/// One row: the definitions left under the application whose delete it drives.
struct Row {
    case: &'static str,
    /// How many definitions are created under the application.
    definitions: usize,
    /// The indexes of the definitions ARCHIVEd before the delete is driven, so the row
    /// leaves records whose status is the archived one.
    archived: &'static [usize],
    /// Whether a run of the first definition is started and then stopped, so that record
    /// carries run history and reports the idle status again.
    run_history: bool,
}

/// An application delete is refused while any experiment definition is still stored under
/// it, the refusal changes nothing, and the same call succeeds once the last definition has
/// been deleted.
#[tokio::test]
async fn an_application_delete_is_refused_while_experiment_definitions_exist() {
    let router = router();
    let account = account();

    let rows = [
        Row {
            case: "one idle definition",
            definitions: 1,
            archived: &[],
            run_history: false,
        },
        Row {
            case: "three idle definitions",
            definitions: 3,
            archived: &[],
            run_history: false,
        },
        Row {
            case: "the only definition is archived",
            definitions: 1,
            archived: &[0],
            run_history: false,
        },
        Row {
            case: "one archived definition beside an idle one",
            definitions: 2,
            archived: &[1],
            run_history: false,
        },
        Row {
            case: "a definition carrying the history of a finished run",
            definitions: 1,
            archived: &[],
            run_history: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let scope = scope(&router, &format!("gate-{index}")).await;
        let application = scope.application.clone();

        let mut definitions = Vec::new();
        for ordinal in 0..row.definitions {
            definitions
                .push(given_definition(&router, case, &scope, &format!("def-{ordinal}")).await);
        }

        if row.run_history {
            let started = crate::start_experiment_run::start_run(
                &router,
                &application,
                &definitions[0],
                &json!({ "ExposurePercentage": 10.0 }),
            )
            .await;
            assert_eq!(
                started.status,
                201,
                "case {case:?}: StartExperimentRun must answer 201, got {} / {}",
                started.status,
                String::from_utf8_lossy(&started.body)
            );
            let run = started.json()["Run"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: StartExperimentRun answered no Run"));
            let stopped = crate::stop_experiment_run::stop_run(
                &router,
                &application,
                &definitions[0],
                run,
                &json!({ "Result": { "ExecutiveSummary": "the run has finished" } }),
            )
            .await;
            assert_eq!(
                stopped.status,
                200,
                "case {case:?}: StopExperimentRun must answer 200, got {} / {}",
                stopped.status,
                String::from_utf8_lossy(&stopped.body)
            );
        }

        for archived in row.archived {
            assert_deleted(
                case,
                "DeleteExperimentDefinition(ARCHIVE)",
                &delete_definition(&router, &application, &definitions[*archived], Some("ARCHIVE"))
                    .await,
            );
        }

        // Only the experiment definitions are left under the application, so the kind the
        // gate names is theirs.
        remove_the_other_children(&router, case, &scope).await;

        let uri = format!("/applications/{application}");
        let before = json_call(&router, "GET", &uri, &Value::Null).await.json();
        let listed_before = listed_definitions(&router, &application).await;
        let documents_before: Vec<Value> = {
            let mut documents = Vec::new();
            for definition in &definitions {
                documents.push(
                    get_definition(&router, &application, definition)
                        .await
                        .json(),
                );
            }
            documents
        };

        // Every definition still stored refuses the delete, however many are left: the
        // refusal is driven once per definition, each time naming the same kind of child,
        // and one of them is deleted between the rounds.
        for (removed, definition) in definitions.iter().enumerate() {
            let refused = delete(&router, &uri).await;
            assert_gated(
                case,
                "DeleteApplication",
                &refused,
                &gate_message(&application, "experiment definitions"),
            );

            if removed == 0 {
                // The refusal had no effect: the application reads back exactly as it did,
                // every definition is still listed under it, and every definition document
                // — status and run history included — is unchanged.
                let after = json_call(&router, "GET", &uri, &Value::Null).await;
                assert_eq!(
                    after.status,
                    200,
                    "case {case:?}: a refused delete leaves the application readable, got {} / {}",
                    after.status,
                    String::from_utf8_lossy(&after.body)
                );
                assert_eq!(
                    after.json(),
                    before,
                    "case {case:?}: a refused delete must leave the application unchanged, got {}",
                    after.json()
                );
                assert_eq!(
                    listed_definitions(&router, &application).await,
                    listed_before,
                    "case {case:?}: a refused delete must leave every definition listed"
                );
                for (ordinal, definition) in definitions.iter().enumerate() {
                    let document = get_definition(&router, &application, definition).await;
                    assert_eq!(
                        document.status,
                        200,
                        "case {case:?}: definition {ordinal} must still be readable, got {} / {}",
                        document.status,
                        String::from_utf8_lossy(&document.body)
                    );
                    assert_eq!(
                        document.json(),
                        documents_before[ordinal],
                        "case {case:?}: definition {ordinal} must be unchanged, got {}",
                        document.json()
                    );
                }
            }

            assert_deleted(
                case,
                "DeleteExperimentDefinition",
                &delete_definition(&router, &application, definition, Some(DESTROY)).await,
            );
        }

        // With the last definition gone the application is a leaf, and the identical call
        // succeeds: the gate is an ORDERING contract, not a resource that cannot be deleted.
        assert_deleted(
            case,
            "DeleteApplication on the leaf application",
            &delete(&router, &uri).await,
        );
        let missed = json_call(&router, "GET", &uri, &Value::Null).await;
        assert_eq!(
            missed.status,
            404,
            "case {case:?}: the deleted application must be gone, got {} / {}",
            missed.status,
            String::from_utf8_lossy(&missed.body)
        );
        assert_eq!(
            missed.string("Message"),
            Some(format!(
                "Application with Id {application} could not be found for account {account}."
            )),
            "case {case:?}: GetApplication must answer the application template, got {}",
            missed.json()
        );
    }
}

/// With children of several kinds under one application the refusal names the kind that is
/// checked first, and each kind in turn once the one before it is gone — so a client
/// tearing an application down is told, one kind at a time, what is still in the way, and
/// the delete only succeeds on the leaf.
#[tokio::test]
async fn the_refusal_names_each_kind_of_child_in_turn() {
    let router = router();
    let case = "one child of every kind";
    let scope = scope(&router, "every-kind").await;
    let application = scope.application.clone();
    let definition = given_definition(&router, case, &scope, "every-kind-def").await;

    // A second environment and a second profile, so the kind that is reported is not an
    // artefact of there being exactly one child of it.
    let spare_environment = create_environment(&router, &application, "every-kind-env-2").await;
    let spare_profile = create_profile(&router, &application, "every-kind-profile-2").await;

    let uri = format!("/applications/{application}");
    assert_gated(
        case,
        "DeleteApplication with children of every kind",
        &delete(&router, &uri).await,
        &gate_message(&application, "environments"),
    );

    for environment in [scope.environment.clone(), spare_environment] {
        assert_deleted(
            case,
            "DeleteEnvironment",
            &delete(
                &router,
                &format!("/applications/{application}/environments/{environment}"),
            )
            .await,
        );
    }
    assert_gated(
        case,
        "DeleteApplication with profiles and a definition left",
        &delete(&router, &uri).await,
        &gate_message(&application, "configuration profiles"),
    );

    for profile in [scope.profile.clone(), spare_profile] {
        assert_deleted(
            case,
            "DeleteConfigurationProfile",
            &delete(
                &router,
                &format!("/applications/{application}/configurationprofiles/{profile}"),
            )
            .await,
        );
    }
    assert_gated(
        case,
        "DeleteApplication with only the definition left",
        &delete(&router, &uri).await,
        &gate_message(&application, "experiment definitions"),
    );

    assert_deleted(
        case,
        "DeleteExperimentDefinition",
        &delete_definition(&router, &application, &definition, Some(DESTROY)).await,
    );
    assert_deleted(
        case,
        "DeleteApplication on the leaf application",
        &delete(&router, &uri).await,
    );
}
