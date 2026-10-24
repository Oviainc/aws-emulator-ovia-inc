// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the settings graph a JOB TEMPLATE may be left holding, on
//! both doors into a stored template: `CreateJobTemplate` and `UpdateJobTemplate`.
//!
//! The rule every case below is one row of: the `JobTemplateSettings` a request asks the
//! service to save must be a graph jobs can be created from — it must describe at least one
//! complete output (at least one `OutputGroups` entry, each group holding at least one
//! `Outputs` entry, and each output stating the container it is written in, either its own
//! `ContainerSettings.Container` or the `Preset` that states it) and carry no more `Inputs`
//! entries than a job template may hold — or the request is refused with
//! `BadRequestException` (400) and the store is left exactly as it was. The same test is
//! taken on the job plane, on the effective graph a submitted job will be run from.
//!
//! The members and their documented rules come from the model:
//! `smithy_query(query="shape",shape="JobTemplateSettings,OutputGroup,Output,ContainerType",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`
//! — `JobTemplateSettings$OutputGroups` is "Contains one group of settings for each set of
//! outputs", `OutputGroup$Outputs` "holds groups of encoding settings, one group of settings
//! per output", `Output$Preset` is "Use Preset to specify a preset for your transcoding
//! settings. … You can specify either Preset or Container settings, but not both", and
//! `JobTemplateSettings$Inputs` states "There can only be one input in a job template. Using
//! the API, you can include multiple inputs when referencing a job template."
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness, so
//! what is asserted is the bytes a client would see, never an internal call. The rest of the
//! job-template write path is `tests/all/job_template_crud.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::MAX_JOB_TEMPLATE_INPUTS;

use super::job_lifecycle::{Emulator, Reply, ROLE};
use super::job_template_crud::settings as template_settings;

/// A `ContainerType` value from the model's enum
/// (`smithy_query(query="shape",shape="ContainerType")`: F4V, GIF, ISMV, M2TS, M3U8, CMFC,
/// MOV, MP4, MPD, MXF, OGG, WEBM, RAW, Y4M). `MXF` is not one the issue cited.
const CONTAINERS: [&str; 3] = ["MP4", "M3U8", "MXF"];

/// An `OutputGroupType` value from the model's enum
/// (`smithy_query(query="shape",shape="OutputGroupType")`).
const FILE_GROUP: &str = "FILE_GROUP_SETTINGS";

/// One output group writing to `destination`, holding one output in `container`.
fn group(destination: &str, container: &str) -> Value {
    json!({
        "name": "File Group",
        "outputs": [{
            "nameModifier": "_1",
            "containerSettings": { "container": container },
            "videoDescription": { "codecSettings": { "codec": "H_264" } }
        }],
        "outputGroupSettings": {
            "type": FILE_GROUP,
            "fileGroupSettings": { "destination": destination }
        }
    })
}

/// An output group whose `outputs` member is whatever a row states, so a row can leave it
/// absent, empty, or carrying outputs of its own.
fn group_with_outputs(outputs: Option<Value>) -> Value {
    let mut built = json!({
        "name": "File Group",
        "outputGroupSettings": {
            "type": FILE_GROUP,
            "fileGroupSettings": { "destination": "s3://example-bucket/out/" }
        }
    });
    if let (Some(object), Some(outputs)) = (built.as_object_mut(), outputs) {
        object.insert("outputs".to_string(), outputs);
    }
    built
}

/// The settings a template is created with before an update row, and the positive control
/// the issue names: one `FILE_GROUP_SETTINGS` group with one MP4 output.
fn valid_settings() -> Value {
    json!({
        "inputs": [{ "timecodeSource": "ZEROBASED" }],
        "outputGroups": [group("s3://example-bucket/out/", CONTAINERS[0])]
    })
}

/// The body of a template the fixture creates before each update row: one input, one output
/// group, and a description an assertion can see is unchanged.
fn fixture(name: &str) -> Value {
    json!({
        "name": name,
        "description": "the original",
        "settings": template_settings(1, 1),
    })
}

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

/// Assert the refusal's `message` names the settings member at fault, which is what a client
/// reads to find the problem.
fn assert_names(case: &str, reply: &Reply, names: &str) {
    let message = reply.string("/message");
    assert!(
        message.contains(names),
        "case {case:?}: the refusal must name the settings member at fault ({names}), \
         got message {message:?}"
    );
}

/// How many job templates this account holds, read back through `ListJobTemplates` — the
/// only way a case checks the store, never by reaching into the state struct.
async fn stored_templates(emulator: &Emulator) -> usize {
    let listed = emulator.list_job_templates("").await;
    assert_eq!(
        listed.status, 200,
        "ListJobTemplates must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    listed
        .at("/jobTemplates")
        .as_array()
        .map_or(usize::MAX, Vec::len)
}

/// One row of the refusal table: a graph the service cannot save, and the settings member
/// the refusal must name as the one at fault.
struct Refused {
    case: &'static str,
    settings: Value,
    names: &'static str,
}

/// The graphs no job can be built from, which both write paths are driven through.
///
/// The table spans what the rule generalises over: WHICH member is at fault (the group list,
/// a group's output list, an output's container, the input count), whether a list is absent
/// or present-but-empty, how many entries the levels above it have, WHICH entry is the bad
/// one (the first, or a later one after complete ones), and an input count one past the
/// published bound as well as further past it.
fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "settings with no member at all",
            settings: json!({}),
            names: "OutputGroups",
        },
        Refused {
            case: "an empty output-group list",
            settings: json!({ "outputGroups": [] }),
            names: "OutputGroups",
        },
        Refused {
            case: "inputs but no outputGroups member",
            settings: json!({ "inputs": [{ "timecodeSource": "ZEROBASED" }] }),
            names: "OutputGroups",
        },
        Refused {
            case: "an input and an empty output-group list",
            settings: template_settings(1, 0),
            names: "OutputGroups",
        },
        Refused {
            case: "settings carrying only members that describe no output",
            settings: json!({ "adAvailOffset": 100, "timecodeConfig": { "source": "ZEROBASED" } }),
            names: "OutputGroups",
        },
        Refused {
            case: "one group that states no outputs member",
            settings: json!({ "outputGroups": [group_with_outputs(None)] }),
            names: "OutputGroups[0].Outputs",
        },
        Refused {
            case: "one group with an empty outputs list",
            settings: json!({ "outputGroups": [group_with_outputs(Some(json!([])))] }),
            names: "OutputGroups[0].Outputs",
        },
        Refused {
            case: "a second group with no outputs, after a complete one",
            settings: json!({
                "outputGroups": [
                    group("s3://example-bucket/out/", CONTAINERS[0]),
                    group_with_outputs(Some(json!([])))
                ]
            }),
            names: "OutputGroups[1].Outputs",
        },
        Refused {
            case: "an output that states neither a preset nor container settings",
            settings: json!({
                "outputGroups": [group_with_outputs(Some(json!([{ "nameModifier": "_1" }])))]
            }),
            names: "OutputGroups[0].Outputs[0]",
        },
        Refused {
            case: "an output whose container settings name no container",
            settings: json!({
                "outputGroups": [group_with_outputs(Some(json!([{
                    "nameModifier": "_1",
                    "containerSettings": { "mp4Settings": { "cslgAtom": "INCLUDE" } }
                }])))]
            }),
            names: "OutputGroups[0].Outputs[0]",
        },
        Refused {
            case: "an output whose preset reference is blank",
            settings: json!({
                "outputGroups": [group_with_outputs(Some(json!([{
                    "nameModifier": "_1",
                    "preset": "   "
                }])))]
            }),
            names: "OutputGroups[0].Outputs[0]",
        },
        Refused {
            case: "a second output with no container, after a complete one",
            settings: json!({
                "outputGroups": [group_with_outputs(Some(json!([
                    { "nameModifier": "_1", "containerSettings": { "container": CONTAINERS[1] } },
                    { "nameModifier": "_2" }
                ])))]
            }),
            names: "OutputGroups[0].Outputs[1]",
        },
        Refused {
            case: "a second group whose output has no container",
            settings: json!({
                "outputGroups": [
                    group("s3://example-bucket/out/", CONTAINERS[2]),
                    group_with_outputs(Some(json!([{ "nameModifier": "_2" }])))
                ]
            }),
            names: "OutputGroups[1].Outputs[0]",
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

/// `CreateJobTemplate` refuses a settings graph no job can be built from, and stores
/// nothing: the name it named is still unreadable afterwards, and no other template appears.
#[tokio::test]
async fn creating_a_template_from_an_unusable_settings_graph_is_a_bad_request() {
    let emulator = Emulator::new();
    let before = stored_templates(&emulator).await;
    let rows = refused_rows();

    for (index, row) in rows.iter().enumerate() {
        let Refused {
            case,
            settings,
            names,
        } = row;
        let name = format!("rejected-{index}");

        let created = emulator
            .create_job_template(json!({ "name": &name, "settings": settings }))
            .await;
        assert_refusal(case, &created, 400, "BadRequestException");
        assert_names(case, &created, names);

        // Nothing was stored: the name the refused create addressed is still free, so a
        // read of it is the modelled 404.
        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: a refused create must store nothing, got {} with body {}",
            fetched.status, fetched.body
        );
    }

    let after = stored_templates(&emulator).await;
    assert_eq!(
        after, before,
        "after {} refused creates the store must hold the templates it started with \
         ({before}), got {after}",
        rows.len()
    );
}

/// `UpdateJobTemplate` refuses the same graphs, and leaves the stored template exactly as it
/// was — every member, not only `Settings` — so a template created usable cannot be emptied.
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
        assert_names(case, &updated, names);

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
            .create_job_template(json!({ "name": &name, "settings": settings }))
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

/// One row of the container table: a graph whose outputs state their container in a
/// different form.
struct Container {
    case: &'static str,
    name: &'static str,
    settings: Value,
    /// The number of `OutputGroups` entries the stored template must report.
    groups: usize,
}

/// An output says what it is written as either in its own `ContainerSettings` or through the
/// `Preset` it names, and a graph is accepted whichever form it uses.
///
/// The rows span that axis and the ones around it: the container each output states, how
/// many outputs a group holds, how many groups the graph holds, and whether the container
/// comes from the output itself or from a preset this account holds.
#[tokio::test]
async fn a_graph_is_accepted_whatever_form_each_output_states_its_container_in() {
    let emulator = Emulator::new();
    // A real preset, so the row whose output takes its container from a preset names a
    // preset this account holds.
    let preset = emulator
        .send(
            "POST",
            "/2017-08-29/presets",
            Some(json!({
                "name": "container-from-here",
                "settings": { "containerSettings": { "container": CONTAINERS[0] } }
            })),
        )
        .await;
    assert_eq!(
        preset.status, 201,
        "the fixture preset must be created: got {} with body {}",
        preset.status, preset.body
    );

    let rows = vec![
        Container {
            case: "one file group with one MP4 output",
            name: "control",
            settings: valid_settings(),
            groups: 1,
        },
        Container {
            case: "an output whose container comes from the preset it names",
            name: "from-preset",
            settings: json!({
                "outputGroups": [group_with_outputs(Some(json!([{
                    "nameModifier": "_1",
                    "preset": "container-from-here"
                }])))]
            }),
            groups: 1,
        },
        Container {
            case: "two outputs in one group, in different containers",
            name: "two-outputs",
            settings: json!({
                "outputGroups": [group_with_outputs(Some(json!([
                    { "nameModifier": "_1", "containerSettings": { "container": CONTAINERS[1] } },
                    { "nameModifier": "_2", "containerSettings": { "container": CONTAINERS[2] } }
                ])))]
            }),
            groups: 1,
        },
        Container {
            case: "three groups, each with one output",
            name: "three-groups",
            settings: json!({
                "outputGroups": [
                    group("s3://example-bucket/out0/", CONTAINERS[0]),
                    group("s3://example-bucket/out1/", CONTAINERS[1]),
                    group("s3://example-bucket/out2/", CONTAINERS[2])
                ]
            }),
            groups: 3,
        },
    ];

    for row in &rows {
        let Container {
            case,
            name,
            settings,
            groups,
        } = row;
        let created = emulator
            .create_job_template(json!({ "name": name, "settings": settings }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );

        // Round trip: the graph the create accepted is the graph a reader sees.
        let fetched = emulator.get_job_template(name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetJobTemplate HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.template(),
            created.template(),
            "case {case:?}: GetJobTemplate must return what CreateJobTemplate persisted, got {}",
            fetched.body
        );
        let counted = count(fetched.template(), "/settings/outputGroups");
        assert_eq!(
            counted, *groups,
            "case {case:?}: expected {groups} stored output groups, got {counted} in {}",
            fetched.body
        );
        assert_eq!(
            fetched.at("/jobTemplate/settings/outputGroups"),
            settings.pointer("/outputGroups").unwrap_or(&Value::Null),
            "case {case:?}: the stored graph must be the one the request stated, got {}",
            fetched.body
        );
    }
}

/// `Settings` is "the new settings for the job template, if you are changing it": an update
/// that names none leaves the stored graph exactly as it was, so the test above is a test of
/// the graph the request carried and not of every update.
#[tokio::test]
async fn an_update_that_names_no_settings_leaves_the_stored_graph_alone() {
    let emulator = Emulator::new();
    let name = "untouched-settings";
    let created = emulator
        .create_job_template(json!({ "name": name, "settings": valid_settings() }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture template must be created: got {} with body {}",
        created.status, created.body
    );
    let stored = created.at("/jobTemplate/settings").clone();

    let described = emulator
        .update_job_template(name, json!({ "description": "still fine" }))
        .await;
    assert_eq!(
        described.status, 200,
        "an update that names no settings must be accepted, got {} with body {}",
        described.status, described.body
    );
    assert_eq!(
        described.at("/jobTemplate/settings"),
        &stored,
        "an update that names no settings must leave the stored graph alone, got {}",
        described.body
    );
    assert_eq!(
        described.string("/jobTemplate/description"),
        "still fine",
        "the member the update DID name must be applied, got {}",
        described.body
    );
}

/// The same rule on the job plane: the graph a job is run from takes the same test, whether
/// the job states it itself or inherits it from the template it names.
///
/// `CreateJob` already refused a graph with no output group; the rows here are the deeper
/// levels of the same rule, plus the inheritance case — the effective graph is the one that
/// is validated, so a job that supplies no outputs of its own is judged on the template's.
#[tokio::test]
async fn a_submitted_job_is_judged_on_the_graph_it_will_be_run_from() {
    let emulator = Emulator::new();
    let complete = emulator
        .create_job_template(json!({ "name": "complete-outputs", "settings": valid_settings() }))
        .await;
    assert_eq!(
        complete.status, 201,
        "the fixture template must be created: got {} with body {}",
        complete.status, complete.body
    );

    // (case, the job's own Settings, the JobTemplate it names, the status, the member the
    //  refusal must name)
    let rows: Vec<(&str, Value, Option<&str>, u16, &str)> = vec![
        (
            "a group with no outputs, stated by the job",
            json!({
                "inputs": [{ "fileInput": "s3://example-bucket/in/a.mp4" }],
                "outputGroups": [group_with_outputs(Some(json!([])))]
            }),
            None,
            400,
            "OutputGroups[0].Outputs",
        ),
        (
            "an output with no container, stated by the job",
            json!({
                "inputs": [{ "fileInput": "s3://example-bucket/in/a.mp4" }],
                "outputGroups": [group_with_outputs(Some(json!([{ "nameModifier": "_1" }])))]
            }),
            None,
            400,
            "OutputGroups[0].Outputs[0]",
        ),
        (
            "inputs of its own and outputs inherited from the template",
            json!({ "inputs": [{ "fileInput": "s3://example-bucket/in/b.mp4" }] }),
            Some("complete-outputs"),
            201,
            "",
        ),
        (
            "a complete graph of its own",
            json!({
                "inputs": [{ "fileInput": "s3://example-bucket/in/c.mp4" }],
                "outputGroups": [group("s3://example-bucket/out/", CONTAINERS[1])]
            }),
            None,
            201,
            "",
        ),
    ];

    for (case, settings, template, status, names) in rows {
        let mut body = json!({ "role": ROLE, "settings": settings });
        if let (Some(object), Some(template)) = (body.as_object_mut(), template) {
            object.insert("jobTemplate".to_string(), json!(template));
        }
        let reply = emulator.create_job(body).await;
        if status == 201 {
            assert_eq!(
                reply.status, 201,
                "case {case:?}: expected HTTP 201, got {} with body {}",
                reply.status, reply.body
            );
            continue;
        }
        assert_refusal(case, &reply, status, "BadRequestException");
        assert_names(case, &reply, names);
    }
}
