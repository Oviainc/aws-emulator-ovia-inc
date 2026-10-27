// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of what an OUTPUT inside a settings graph has to state about
//! its encoder settings, on every operation that is handed such a graph.
//!
//! The one rule every case below is a row of: an output states its encoder settings
//! EXACTLY once — either by naming a preset the account holds (the built-in catalogue or one
//! it created) or by carrying container settings of its own that name the container — so an
//! output that states both, states neither, or names a preset the account does not hold is
//! refused with `BadRequestException` (400) naming the output's position, and nothing is
//! stored or changed.
//!
//! The members and their documented purpose come from the model: `Output$Preset` is "Use
//! Preset to specify a preset for your transcoding settings. Provide the system or custom
//! preset name. You can specify either Preset or Container settings, but not both" and
//! `ContainerSettings$Container` is "Container for this output" — see
//! `smithy_query(query="shape",shape="Output",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`.
//!
//! The rows are driven through all three operations that take an output group list —
//! `CreateJob`, `CreateJobTemplate` and `UpdateJobTemplate` — so the rule is asserted
//! wherever it holds, not only on the operation the issue named. Every case drives the real
//! restJson1 wire path through the crate's one test harness, so what is asserted is the
//! bytes a client would see, never an internal call.

use serde_json::{json, Value};

use super::job_lifecycle::{Emulator, Reply, ROLE};

/// The source file every row's graph reads, so no row differs in its input.
const INPUT_URI: &str = "s3://example-bucket/in/a.mp4";

/// The preset the cases create for themselves, to have a `CUSTOM` one to name.
const CUSTOM_PRESET: &str = "my-output-preset";

/// A preset the cases create and then delete, so a row can name one that resolved once and
/// resolves no longer.
const DELETED_PRESET: &str = "preset-that-went-away";

/// The job template the `UpdateJobTemplate` rows are applied to.
const UPDATED_TEMPLATE: &str = "outputs-replaced-here";

/// One output, stating a preset, container settings, or neither.
fn output(preset: Option<&str>, container: Option<&str>) -> Value {
    let mut output = json!({ "nameModifier": "_a" });
    if let Some(members) = output.as_object_mut() {
        if let Some(preset) = preset {
            members.insert("preset".to_string(), json!(preset));
        }
        if let Some(container) = container {
            members.insert(
                "containerSettings".to_string(),
                json!({ "container": container }),
            );
        }
    }
    output
}

/// An output group per entry, each holding that entry's outputs, so a row can put the
/// output at fault anywhere in the graph.
fn output_groups(groups: &[Vec<Value>]) -> Value {
    Value::Array(
        groups
            .iter()
            .enumerate()
            .map(|(index, outputs)| {
                json!({
                    "name": format!("File Group {index}"),
                    "outputs": outputs,
                    "outputGroupSettings": {
                        "type": "FILE_GROUP_SETTINGS",
                        "fileGroupSettings": {
                            "destination": format!("s3://example-bucket/out{index}/")
                        }
                    }
                })
            })
            .collect(),
    )
}

/// A `JobSettings` whose outputs are the row's: one source file, and the row's groups.
fn job_settings(groups: &[Vec<Value>]) -> Value {
    json!({
        "inputs": [{ "fileInput": INPUT_URI }],
        "outputGroups": output_groups(groups)
    })
}

/// A `JobTemplateSettings` whose outputs are the row's. A template's `Inputs` are
/// `InputTemplate`s, which carry no file input — the file comes from the job.
fn template_settings(groups: &[Vec<Value>]) -> Value {
    json!({
        "inputs": [{ "timecodeSource": "ZEROBASED" }],
        "outputGroups": output_groups(groups)
    })
}

/// How many jobs the store holds, read back through `ListJobs`.
///
/// A refusal must leave the store as it was, and this is how every refusal row checks it:
/// through the API, never by reaching into the state struct.
async fn stored_jobs(emulator: &Emulator) -> usize {
    let listed = emulator.send("GET", "/2017-08-29/jobs", None).await;
    assert_eq!(
        listed.status, 200,
        "ListJobs must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    listed.at("/jobs").as_array().map_or(usize::MAX, Vec::len)
}

/// Assert a reply is the modelled refusal, and that its message names what was wrong.
fn assert_refusal(case: &str, reply: &Reply, expects: &[String]) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("BadRequestException"),
        "case {case:?}: expected BadRequestException, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
    let message = reply.string("/message");
    for fragment in expects {
        assert!(
            message.contains(fragment.as_str()),
            "case {case:?}: the refusal must name {fragment:?}, got message {message:?}"
        );
    }
}

/// One row: a graph's output groups, and what the service must answer for them.
struct Row {
    case: String,
    /// The outputs of each output group the graph carries.
    groups: Vec<Vec<Value>>,
    /// Fragments the refusal's message must contain. EMPTY means the row is accepted.
    expects: Vec<String>,
}

impl Row {
    fn accepted(case: &str, groups: Vec<Vec<Value>>) -> Self {
        Self {
            case: case.to_string(),
            groups,
            expects: Vec::new(),
        }
    }

    fn refused(case: &str, groups: Vec<Vec<Value>>, expects: &[&str]) -> Self {
        Self {
            case: case.to_string(),
            groups,
            expects: expects.iter().map(|text| (*text).to_string()).collect(),
        }
    }

    fn is_accepted(&self) -> bool {
        self.expects.is_empty()
    }
}

/// The table, spanning what the rule generalises over: which half of the pair the output
/// states (preset, container settings, both, neither), the KIND of preset it names (one the
/// account created, one the service published), the form of the name (a name nothing holds,
/// one that was deleted, one differing only in case, one made of whitespace), and WHERE in
/// the graph the output at fault sits (first output, second output of a group, output of a
/// second group).
///
/// `custom` is the preset the fixtures created and `system` one the store published, both
/// read back from the service rather than hard-coded.
fn rows(custom: &str, system: &str) -> Vec<Row> {
    vec![
        Row::accepted(
            "container settings naming the container",
            vec![vec![output(None, Some("MP4"))]],
        ),
        Row::accepted(
            "a preset the account created",
            vec![vec![output(Some(custom), None)]],
        ),
        Row::accepted(
            "a built-in preset, by the name the store published",
            vec![vec![output(Some(system), None)]],
        ),
        Row::accepted(
            "two groups, one naming each kind of preset",
            vec![
                vec![output(Some(custom), None)],
                vec![output(Some(system), None)],
            ],
        ),
        Row::accepted(
            "two outputs in one group, a preset and container settings",
            vec![vec![
                output(Some(custom), None),
                output(None, Some("M3U8")),
            ]],
        ),
        Row::refused(
            "a preset name nothing holds",
            vec![vec![output(Some("no-such-preset-xyz"), None)]],
            &["OutputGroups[0].Outputs[0].Preset", "no-such-preset-xyz"],
        ),
        Row::refused(
            "a preset that was deleted",
            vec![vec![output(Some(DELETED_PRESET), None)]],
            &["OutputGroups[0].Outputs[0].Preset", DELETED_PRESET],
        ),
        Row::refused(
            "a preset name differing from a stored one only in case",
            vec![vec![output(Some(&custom.to_uppercase()), None)]],
            &["OutputGroups[0].Outputs[0].Preset"],
        ),
        Row::refused(
            "the SECOND output of a group names a preset nothing holds",
            vec![vec![
                output(None, Some("MP4")),
                output(Some("no-such-preset-xyz"), None),
            ]],
            &["OutputGroups[0].Outputs[1].Preset"],
        ),
        Row::refused(
            "the output of the SECOND group names a preset nothing holds",
            vec![
                vec![output(None, Some("MP4"))],
                vec![output(Some("no-such-preset-xyz"), None)],
            ],
            &["OutputGroups[1].Outputs[0].Preset"],
        ),
        Row::refused(
            "both a preset and container settings",
            vec![vec![output(Some(custom), Some("MP4"))]],
            &["OutputGroups[0].Outputs[0]", "but not both"],
        ),
        Row::refused(
            "both, where the preset resolves to nothing either",
            vec![vec![output(Some("no-such-preset-xyz"), Some("MP4"))]],
            &["OutputGroups[0].Outputs[0]"],
        ),
        Row::refused(
            "neither a preset nor container settings",
            vec![vec![output(None, None)]],
            &["OutputGroups[0].Outputs[0]"],
        ),
        Row::refused(
            "container settings that name no container",
            vec![vec![json!({
                "nameModifier": "_a",
                "containerSettings": { "mp4Settings": { "cslgAtom": "INCLUDE" } }
            })]],
            &["OutputGroups[0].Outputs[0]"],
        ),
        Row::refused(
            "a preset name made only of whitespace",
            vec![vec![output(Some("   "), None)]],
            &["OutputGroups[0].Outputs[0]"],
        ),
    ]
}

/// The name of a preset the SERVICE publishes, read through `ListPresets` so no case here
/// hard-codes the built-in catalogue.
async fn a_published_preset(emulator: &Emulator) -> String {
    let listed = emulator.list_presets("listBy=SYSTEM").await;
    assert_eq!(
        listed.status, 200,
        "ListPresets must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    let name = listed.string("/presets/0/name");
    assert!(
        !name.is_empty(),
        "the account must hold at least one built-in preset, got body {}",
        listed.body
    );
    name
}

/// Create the presets and the job template every row is driven against, and report the
/// name of a built-in preset.
async fn fixtures(emulator: &Emulator) -> String {
    for name in [CUSTOM_PRESET, DELETED_PRESET] {
        let created = emulator
            .create_preset(json!({
                "name": name,
                "settings": { "containerSettings": { "container": "MP4" } }
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "the fixture preset {name:?} must be created: got {} with body {}",
            created.status, created.body
        );
    }
    // One preset resolved when it was created and resolves no longer, which is what a
    // template outliving its preset leaves behind.
    let deleted = emulator.delete_preset(DELETED_PRESET).await;
    assert_eq!(
        deleted.status, 202,
        "the fixture preset {DELETED_PRESET:?} must be deleted: got {} with body {}",
        deleted.status, deleted.body
    );

    let template = emulator
        .create_job_template(json!({
            "name": UPDATED_TEMPLATE,
            "settings": template_settings(&[vec![output(None, Some("MP4"))]])
        }))
        .await;
    assert_eq!(
        template.status, 201,
        "the fixture job template must be created: got {} with body {}",
        template.status, template.body
    );

    a_published_preset(emulator).await
}

/// Assert the stored graph reports, output by output, what the row's graph stated: a
/// reference the service accepted is the one a reader sees.
fn assert_outputs_round_trip(case: &str, groups: &[Vec<Value>], stored: &Value, label: &str) {
    for (group, outputs) in groups.iter().enumerate() {
        for (index, sent) in outputs.iter().enumerate() {
            let pointer = format!("/{group}/outputs/{index}");
            let read = stored.pointer(&pointer).unwrap_or(&Value::Null);
            for member in ["preset", "containerSettings"] {
                let expected = sent.get(member).unwrap_or(&Value::Null);
                let actual = read.get(member).unwrap_or(&Value::Null);
                assert_eq!(
                    actual, expected,
                    "case {case:?} via {label}: expected {member} {expected} at {pointer}, \
                     got {actual}"
                );
            }
        }
    }
}

/// An output states its encoder settings exactly once, and the preset it names must be one
/// the account holds — on `CreateJob`, `CreateJobTemplate` and `UpdateJobTemplate` alike.
#[tokio::test]
async fn an_output_states_its_encoder_settings_once_and_its_preset_must_resolve() {
    let emulator = Emulator::new();
    let system = fixtures(&emulator).await;

    let rows = rows(CUSTOM_PRESET, &system);
    let mut accepted = 0;
    for (index, row) in rows.iter().enumerate() {
        let Row {
            case,
            groups,
            expects,
        } = row;

        // What the addressed template carries BEFORE the update, so a refused update can be
        // shown to have changed nothing whatever an earlier row left there.
        let before = emulator.get_job_template(UPDATED_TEMPLATE).await;
        let before_groups = before.at("/jobTemplate/settings/outputGroups").clone();

        let job = emulator
            .create_job(json!({ "role": ROLE, "settings": job_settings(groups) }))
            .await;
        let template = emulator
            .create_job_template(json!({
                "name": format!("preset-rule-{index}"),
                "settings": template_settings(groups)
            }))
            .await;
        let updated = emulator
            .update_job_template(
                UPDATED_TEMPLATE,
                json!({ "settings": template_settings(groups) }),
            )
            .await;

        if row.is_accepted() {
            accepted += 1;
            assert_eq!(
                job.status, 201,
                "case {case:?} via CreateJob: expected HTTP 201, got {} with body {}",
                job.status, job.body
            );
            assert_eq!(
                template.status, 201,
                "case {case:?} via CreateJobTemplate: expected HTTP 201, got {} with body {}",
                template.status, template.body
            );
            assert_eq!(
                updated.status, 200,
                "case {case:?} via UpdateJobTemplate: expected HTTP 200, got {} with body {}",
                updated.status, updated.body
            );

            // Round trip: every accepted graph reads back through the reader of its kind,
            // reporting the references the service resolved.
            let fetched = emulator.get_job(&job.string("/job/id")).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: expected GetJob HTTP 200, got {} with body {}",
                fetched.status, fetched.body
            );
            assert_outputs_round_trip(
                case,
                groups,
                fetched.at("/job/settings/outputGroups"),
                "GetJob",
            );
            let stored_template = emulator.get_job_template(UPDATED_TEMPLATE).await;
            assert_outputs_round_trip(
                case,
                groups,
                stored_template.at("/jobTemplate/settings/outputGroups"),
                "GetJobTemplate",
            );
        } else {
            for (operation, reply) in [
                ("CreateJob", &job),
                ("CreateJobTemplate", &template),
                ("UpdateJobTemplate", &updated),
            ] {
                assert_refusal(&format!("{operation}: {case}"), reply, expects);
            }
            // A refused update leaves the addressed template exactly as it was.
            let after = emulator.get_job_template(UPDATED_TEMPLATE).await;
            let after_groups = after.at("/jobTemplate/settings/outputGroups");
            assert_eq!(
                after_groups, &before_groups,
                "case {case:?}: a refused UpdateJobTemplate must change nothing, got {}",
                after.body
            );
            // And a refused create stores no template under the name it named.
            let missing = emulator
                .get_job_template(&format!("preset-rule-{index}"))
                .await;
            assert_eq!(
                missing.status, 404,
                "case {case:?}: a refused CreateJobTemplate must store nothing, got {} with \
                 body {}",
                missing.status, missing.body
            );
        }
    }

    let stored = stored_jobs(&emulator).await;
    assert_eq!(
        stored, accepted,
        "only the accepted submissions may be stored: expected {accepted}, got {stored}"
    );
}

/// A job that takes its outputs from a job template is validated against the EFFECTIVE
/// graph: the preset the inherited output names is resolved when the job is submitted, so a
/// template that outlives its preset refuses the submission instead of storing a job whose
/// output names a preset no reader can resolve.
#[tokio::test]
async fn an_inherited_output_resolves_its_preset_at_submit_time() {
    let emulator = Emulator::new();
    let preset = "inherited-output-preset";
    let created = emulator
        .create_preset(json!({
            "name": preset,
            "settings": { "containerSettings": { "container": "MP4" } }
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture preset must be created: got {} with body {}",
        created.status, created.body
    );
    let template = emulator
        .create_job_template(json!({
            "name": "outputs-from-a-preset",
            "settings": template_settings(&[vec![output(Some(preset), None)]])
        }))
        .await;
    assert_eq!(
        template.status, 201,
        "the fixture job template must be created: got {} with body {}",
        template.status, template.body
    );

    // While the preset resolves, the job inherits the output that names it.
    let job = emulator
        .create_job(json!({
            "role": ROLE,
            "jobTemplate": "outputs-from-a-preset",
            "settings": { "inputs": [{ "fileInput": INPUT_URI }] }
        }))
        .await;
    assert_eq!(
        job.status, 201,
        "case \"preset still held\": expected HTTP 201, got {} with body {}",
        job.status, job.body
    );
    let fetched = emulator.get_job(&job.string("/job/id")).await;
    assert_eq!(
        fetched.string("/job/settings/outputGroups/0/outputs/0/preset"),
        preset,
        "case \"preset still held\": the stored job must name the inherited preset, got {}",
        fetched.body
    );

    let deleted = emulator.delete_preset(preset).await;
    assert_eq!(
        deleted.status, 202,
        "the fixture preset must be deleted: got {} with body {}",
        deleted.status, deleted.body
    );

    // The same submission, against the same template, once the preset is gone.
    let refused = emulator
        .create_job(json!({
            "role": ROLE,
            "jobTemplate": "outputs-from-a-preset",
            "settings": { "inputs": [{ "fileInput": INPUT_URI }] }
        }))
        .await;
    assert_refusal(
        "preset deleted after the template was written",
        &refused,
        &[
            "OutputGroups[0].Outputs[0].Preset".to_string(),
            preset.to_string(),
        ],
    );

    let stored = stored_jobs(&emulator).await;
    assert_eq!(
        stored, 1,
        "the refused submission must store no second job, got {stored}"
    );
}
