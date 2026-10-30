// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `Output$Preset` as a RESOURCE REFERENCE: a value the request
//! states but which carries no characters names no preset, so it is refused rather than read
//! as the member left out.
//!
//! The rule this suite is the rows of is the crate's one rule for a member that names a
//! resource (`tests/all/blank_resource_name.rs` states it for the top-level members, and
//! its hop-destination case for a reference nested in a list): the member targets a bare
//! string the model gives no pattern and no length
//! (`smithy_query(query="shape",shape="Output",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`
//! reports `Output$Preset` as `__stringMin0`, so the boundary validator passes the
//! zero-length value and a value made only of whitespace alike), and a reference carrying no
//! characters resolves to nothing — so stating it is `BadRequestException` (400) naming the
//! member's path, and nothing is stored. Stating the member and leaving it out are DIFFERENT
//! requests: only the second takes the "this output has no preset" reading, under which the
//! output's own `ContainerSettings` must name its container.
//!
//! The rows span what the rule generalises over: which blank form the value is made of
//! (every form in the shared table), whether the output ALSO carries container settings —
//! the case `Output$Preset` forbids when the reference carries characters ("You can specify
//! either Preset or Container settings, but not both") — and where in the graph the output
//! sits, so the member path the refusal names is asserted for a later output and a later
//! group too. The boundary rows are references that DO carry characters: those are resolved
//! byte for byte against the store, whitespace included, and never trimmed into another
//! name.
//!
//! Every row is driven through all three doors handed a settings graph — `CreateJob`,
//! `CreateJobTemplate` and `UpdateJobTemplate` — over the real restJson1 wire path through
//! the crate's one test harness, so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use super::blank_resource_name::{blanks, refusal_message};
use super::job_lifecycle::{Emulator, Reply, ROLE};
use super::output_preset_reference::{job_settings, output, stored_jobs, template_settings};

/// The job template every `UpdateJobTemplate` row is applied to.
const UPDATED_TEMPLATE: &str = "blank-preset-target";

/// A `ContainerType` value from the model's enum
/// (`smithy_query(query="shape",shape="ContainerType")`), so the rows that carry container
/// settings name a container the model publishes.
const CONTAINER: &str = "MP4";

/// The member path a refusal must name, built the way the service builds it: the output's
/// position in the graph, then the member the value arrived in.
fn preset_member(group: usize, output: usize) -> String {
    format!("OutputGroups[{group}].Outputs[{output}].Preset")
}

/// Assert `reply` is the modelled bad request whose message ends in the blank-member
/// sentence for `member`.
///
/// `ends_with` rather than equality because the two job-template doors state the settings
/// member the graph arrived in before the problem inside it, which the job door does not.
/// Pinning the whole tail still excludes every other refusal of the same output — the
/// "either Preset or Container settings" one and the "must specify either Preset or
/// ContainerSettings.Container" one both end differently.
fn assert_blank_preset_refusal(case: &str, member: &str, reply: &Reply) {
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
    let expected = refusal_message(member);
    assert!(
        message.ends_with(expected.as_str()),
        "case {case:?}: expected a refusal ending in {expected:?}, got message {message:?}"
    );
}

/// One row: the output groups a graph carries, and the member path its refusal must name.
struct Row {
    case: String,
    groups: Vec<Vec<Value>>,
    member: String,
}

/// The table: every blank form, with and without container settings of the output's own,
/// plus the two placements that move the output at fault inside the graph.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for blank in blanks() {
        for (label, container) in [
            ("alone", None),
            ("beside container settings", Some(CONTAINER)),
        ] {
            rows.push(Row {
                case: format!("a preset of {} {label}", blank.label),
                groups: vec![vec![output(Some(blank.value), container)]],
                member: preset_member(0, 0),
            });
        }
    }

    // The same value further into the graph: the refusal names the output it arrived at,
    // not the first one.
    let blank = " \t ";
    rows.push(Row {
        case: "a blank preset on the SECOND output of a group".to_string(),
        groups: vec![vec![
            output(None, Some(CONTAINER)),
            output(Some(blank), None),
        ]],
        member: preset_member(0, 1),
    });
    rows.push(Row {
        case: "a blank preset on the output of the SECOND group".to_string(),
        groups: vec![
            vec![output(None, Some(CONTAINER))],
            vec![output(Some(blank), Some(CONTAINER))],
        ],
        member: preset_member(1, 0),
    });
    rows
}

/// Create the job template the update rows are applied to.
async fn fixture(emulator: &Emulator) {
    let created = emulator
        .create_job_template(json!({
            "name": UPDATED_TEMPLATE,
            "settings": template_settings(&[vec![output(None, Some(CONTAINER))]])
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture job template must be created: got {} with body {}",
        created.status, created.body
    );
}

/// A `Preset` the request states but which carries no characters is refused by every door
/// handed a settings graph, whatever the output's other members say, and nothing is stored.
#[tokio::test]
async fn a_stated_preset_carrying_no_characters_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    fixture(&emulator).await;
    let before_jobs = stored_jobs(&emulator).await;

    for (index, row) in rows().iter().enumerate() {
        let Row {
            case,
            groups,
            member,
        } = row;

        // What the addressed template carries BEFORE the update, so a refused update can be
        // shown to have changed nothing.
        let before = emulator.get_job_template(UPDATED_TEMPLATE).await;
        let before_template = before.template().clone();

        let created_name = format!("blank-preset-{index}");
        let job = emulator
            .create_job(json!({ "role": ROLE, "settings": job_settings(groups) }))
            .await;
        let template = emulator
            .create_job_template(json!({
                "name": &created_name,
                "settings": template_settings(groups)
            }))
            .await;
        let updated = emulator
            .update_job_template(
                UPDATED_TEMPLATE,
                json!({ "settings": template_settings(groups) }),
            )
            .await;

        for (operation, reply) in [
            ("CreateJob", &job),
            ("CreateJobTemplate", &template),
            ("UpdateJobTemplate", &updated),
        ] {
            assert_blank_preset_refusal(&format!("{operation}: {case}"), member, reply);
        }

        // The refusals left the store exactly as it was: no template under the name the
        // refused create named, and the addressed template untouched in every member.
        let missing = emulator.get_job_template(&created_name).await;
        assert_eq!(
            missing.status, 404,
            "case {case:?}: a refused create must store nothing, got {} with body {}",
            missing.status, missing.body
        );
        let after = emulator.get_job_template(UPDATED_TEMPLATE).await;
        assert_eq!(
            after.template(),
            &before_template,
            "case {case:?}: a refused update must change nothing, got {}",
            after.body
        );
    }

    let after_jobs = stored_jobs(&emulator).await;
    assert_eq!(
        after_jobs, before_jobs,
        "no refused submission may be stored: expected {before_jobs} jobs, got {after_jobs}"
    );
}

/// The preset the boundary rows name, whose own stored name is padded with whitespace: a
/// value that carries characters is a key byte for byte, so this name resolves as sent while
/// its trimmed spelling names no preset at all.
const PADDED_PRESET: &str = "  padded preset  ";

/// What a door answers for a reference that DOES carry characters.
enum Expect {
    /// The reference resolves: the request is accepted and the value is stored as sent.
    Accepted,
    /// The reference names no preset this account holds: the modelled lookup refusal
    /// naming the member's path and the value stated.
    NotFound,
    /// The output states its encoder settings twice, which `Output$Preset` forbids.
    BothStated,
}

/// The other side of the same rule: a `Preset` carrying at least one character is resolved
/// exactly as sent — never trimmed — so only an entirely blank value takes the blank
/// refusal, and a padded name is its own key.
#[tokio::test]
async fn a_preset_reference_that_carries_characters_is_resolved_as_sent() {
    let emulator = Emulator::new();
    let created = emulator
        .create_preset(json!({
            "name": PADDED_PRESET,
            "settings": { "containerSettings": { "container": CONTAINER } }
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture preset must be created: got {} with body {}",
        created.status, created.body
    );

    let rows = [
        (
            "the padded name the account holds",
            PADDED_PRESET.to_string(),
            None,
            Expect::Accepted,
        ),
        (
            "the same name with its padding stripped",
            PADDED_PRESET.trim().to_string(),
            None,
            Expect::NotFound,
        ),
        (
            "a padded spelling of a name nothing holds",
            " no-such-preset ".to_string(),
            None,
            Expect::NotFound,
        ),
        (
            "the padded name beside container settings",
            PADDED_PRESET.to_string(),
            Some(CONTAINER),
            Expect::BothStated,
        ),
    ];

    for (index, (case, preset, container, expect)) in rows.iter().enumerate() {
        let groups = vec![vec![output(Some(preset), *container)]];
        let name = format!("carries-characters-{index}");
        let reply = emulator
            .create_job_template(json!({ "name": &name, "settings": template_settings(&groups) }))
            .await;
        let member = preset_member(0, 0);
        match expect {
            Expect::Accepted => {
                assert_eq!(
                    reply.status, 201,
                    "case {case:?}: expected HTTP 201, got {} with body {}",
                    reply.status, reply.body
                );
                // Round trip: the reference the service resolved is the one a reader sees,
                // byte for byte.
                let fetched = emulator.get_job_template(&name).await;
                assert_eq!(
                    fetched.status, 200,
                    "case {case:?}: expected GetJobTemplate HTTP 200, got {} with body {}",
                    fetched.status, fetched.body
                );
                let stored = fetched.string("/jobTemplate/settings/outputGroups/0/outputs/0/preset");
                assert_eq!(
                    &stored, preset,
                    "case {case:?}: expected the stored preset {preset:?}, got {stored:?}"
                );
            }
            Expect::NotFound | Expect::BothStated => {
                assert_eq!(
                    reply.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with body {}",
                    reply.status, reply.body
                );
                let message = reply.string("/message");
                assert!(
                    !message.ends_with(refusal_message(&member).as_str()),
                    "case {case:?}: a reference carrying characters is not the blank \
                     refusal, got message {message:?}"
                );
                let expected_fragment = match expect {
                    Expect::NotFound => member.clone(),
                    _ => "but not both".to_string(),
                };
                assert!(
                    message.contains(expected_fragment.as_str()),
                    "case {case:?}: the refusal must name {expected_fragment:?}, got message \
                     {message:?}"
                );
                let missing = emulator.get_job_template(&name).await;
                assert_eq!(
                    missing.status, 404,
                    "case {case:?}: a refused create must store nothing, got {} with body {}",
                    missing.status, missing.body
                );
            }
        }
    }
}
