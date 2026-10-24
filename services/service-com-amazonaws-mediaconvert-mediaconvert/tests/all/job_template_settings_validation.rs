// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the settings graph a JOB TEMPLATE may be left holding, on
//! both doors into a stored template: `CreateJobTemplate` and `UpdateJobTemplate`.
//!
//! The rule every case below is one row of: the `JobTemplateSettings` a request asks the
//! service to save must be a graph jobs can be created from — at least one `OutputGroups`
//! entry, and no more `Inputs` entries than a job template may hold — or the request is
//! refused with `BadRequestException` (400) and the store is left exactly as it was.
//!
//! The two members and their documented rules come from the model:
//! `smithy_query(query="shape",shape="JobTemplateSettings",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`
//! — `OutputGroups` "contains one group of settings for each set of outputs", and `Inputs`
//! states "There can only be one input in a job template. Using the API, you can include
//! multiple inputs when referencing a job template."
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness, so
//! what is asserted is the bytes a client would see, never an internal call.

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::MAX_JOB_TEMPLATE_INPUTS;

use super::job_lifecycle::{Emulator, Reply};
use super::job_template_crud::settings as template_settings;

/// How many entries a JSON array member holds, or `usize::MAX` when it is not an array —
/// a value an assertion prints rather than a panic.
fn count(view: &Value, pointer: &str) -> usize {
    view.pointer(pointer)
        .and_then(Value::as_array)
        .map_or(usize::MAX, Vec::len)
}

/// Assert a reply is the modelled refusal `status`/`error_type` names.
fn assert_refusal(case: &str, reply: &Reply, status: u16, error_type: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(error_type),
        "case {case:?}: expected {error_type}, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
}

/// A settings graph with no `OutputGroups` MEMBER at all, as against one whose member is
/// present and empty — the two forms "supplies no output group" takes on the wire.
fn settings_without_the_member() -> Value {
    json!({ "inputs": [{ "timecodeSource": "ZEROBASED" }] })
}

/// One row of the refusal table: a graph the service cannot save, and the settings member
/// the refusal must name as the one at fault.
struct Refused {
    case: &'static str,
    settings: Value,
    names: &'static str,
}

/// The graphs no job can be built from, spanning what the rule generalises over: which of
/// the two members is at fault, whether a list is absent or present-but-empty, how many
/// entries the other member carries, and an input count one past the published bound as
/// well as further past it.
fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "no outputGroups member at all",
            settings: settings_without_the_member(),
            names: "OutputGroups",
        },
        Refused {
            case: "an empty outputGroups list",
            settings: template_settings(1, 0),
            names: "OutputGroups",
        },
        Refused {
            case: "no outputGroups and no inputs either",
            settings: template_settings(0, 0),
            names: "OutputGroups",
        },
        Refused {
            // One past the bound `JobTemplateSettings$Inputs` publishes.
            case: "one input past the bound, with an output group",
            settings: template_settings(MAX_JOB_TEMPLATE_INPUTS + 1, 1),
            names: "Inputs",
        },
        Refused {
            case: "well past the bound, with several output groups",
            settings: template_settings(MAX_JOB_TEMPLATE_INPUTS + 2, 3),
            names: "Inputs",
        },
        Refused {
            case: "too many inputs and no output groups at once",
            settings: template_settings(MAX_JOB_TEMPLATE_INPUTS + 1, 0),
            names: "Inputs",
        },
    ]
}

/// The body of a template the fixture creates before each update row: one input, one
/// output group, and a description an assertion can see change.
fn fixture(name: &str) -> Value {
    json!({
        "name": name,
        "description": "the original",
        "settings": template_settings(1, 1),
    })
}

/// `CreateJobTemplate` refuses a settings graph no job can be built from, and stores
/// nothing: the name it named is still unreadable afterwards.
#[tokio::test]
async fn creating_a_template_from_an_unusable_settings_graph_is_a_bad_request() {
    let emulator = Emulator::new();

    for (index, row) in refused_rows().iter().enumerate() {
        let Refused {
            case,
            settings,
            names,
        } = row;
        let name = format!("rejected-{index}");

        let created = emulator
            .create_job_template(json!({ "name": name, "settings": settings }))
            .await;
        assert_refusal(case, &created, 400, "BadRequestException");
        let message = created.string("/message");
        assert!(
            message.contains(names),
            "case {case:?}: the refusal must name the settings member at fault ({names}), \
             got message {message:?}"
        );

        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: a refused create must store nothing, got {} with body {}",
            fetched.status, fetched.body
        );
    }
}

/// `UpdateJobTemplate` refuses the same graphs, and leaves the stored template exactly as
/// it was — every member, not only `Settings`.
#[tokio::test]
async fn updating_a_template_to_an_unusable_settings_graph_is_a_bad_request() {
    let emulator = Emulator::new();

    for (index, row) in refused_rows().iter().enumerate() {
        let Refused {
            case,
            settings,
            names,
        } = row;
        let name = format!("kept-{index}");

        let created = emulator.create_job_template(fixture(&name)).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the fixture template must be created, got {} with body {}",
            created.status, created.body
        );

        // The update carries the bad graph AND a member that is fine on its own, so the
        // assertion below shows the whole request was refused rather than half applied.
        let updated = emulator
            .update_job_template(
                &name,
                json!({ "description": "the replacement", "settings": settings }),
            )
            .await;
        assert_refusal(case, &updated, 400, "BadRequestException");
        let message = updated.string("/message");
        assert!(
            message.contains(names),
            "case {case:?}: the refusal must name the settings member at fault ({names}), \
             got message {message:?}"
        );

        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the template must still be readable, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.template(),
            created.template(),
            "case {case:?}: a refused update must change nothing, got {} after {}",
            fetched.template(),
            created.template()
        );
    }
}

/// One row of the accepted table: a graph jobs CAN be built from, and the entry counts the
/// stored template must report.
struct Accepted {
    case: String,
    settings: Value,
    inputs: usize,
    groups: usize,
}

/// The positive control of the rule, on both doors: a graph at or inside the published
/// bounds is stored and reads back through `GetJobTemplate` with the entries it carried.
///
/// The rows span the same axes as the refusals: every input count a template may hold
/// (including none, which a job supplies itself) against one, two and three output groups.
fn accepted_rows() -> Vec<Accepted> {
    let mut rows = Vec::new();
    for inputs in 0..=MAX_JOB_TEMPLATE_INPUTS {
        for groups in 1..=3 {
            rows.push(Accepted {
                case: format!("{inputs} inputs and {groups} output groups"),
                settings: template_settings(inputs, groups),
                inputs,
                groups,
            });
        }
    }
    rows
}

/// A usable graph is accepted by `CreateJobTemplate`, accepted by `UpdateJobTemplate`, and
/// read back by `GetJobTemplate` with the entries it carried.
#[tokio::test]
async fn a_usable_settings_graph_is_stored_and_reads_back() {
    let emulator = Emulator::new();

    for (index, row) in accepted_rows().iter().enumerate() {
        let Accepted {
            case,
            settings,
            inputs,
            groups,
        } = row;
        let name = format!("accepted-{index}");

        let created = emulator
            .create_job_template(json!({ "name": name, "settings": settings }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from the create, got {} with body {}",
            created.status, created.body
        );

        // The same graph through the update door, onto a template created with one input
        // and one output group, so the update is a real replacement.
        let target = format!("replaced-{index}");
        let fixture_reply = emulator.create_job_template(fixture(&target)).await;
        assert_eq!(
            fixture_reply.status, 201,
            "case {case:?}: the fixture template must be created, got {} with body {}",
            fixture_reply.status, fixture_reply.body
        );
        let updated = emulator
            .update_job_template(&target, json!({ "settings": settings }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {} with body {}",
            updated.status, updated.body
        );

        for name in [&name, &target] {
            let fetched = emulator.get_job_template(name).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: expected {name} to read back, got {} with body {}",
                fetched.status, fetched.body
            );
            let view = fetched.template();
            let got_inputs = count(view, "/settings/inputs");
            assert_eq!(
                got_inputs, *inputs,
                "case {case:?}: expected {inputs} inputs stored under {name}, got \
                 {got_inputs} in {view}"
            );
            let got_groups = count(view, "/settings/outputGroups");
            assert_eq!(
                got_groups, *groups,
                "case {case:?}: expected {groups} output groups stored under {name}, got \
                 {got_groups} in {view}"
            );
        }
    }
}
