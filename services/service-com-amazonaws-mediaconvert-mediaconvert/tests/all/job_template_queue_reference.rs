// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the rule that a job template's top-level `Queue` is a
//! REFERENCE to a queue resource, resolved from the caller's bytes.
//!
//! `CreateJobTemplate$Queue` ("Optional. The queue that jobs created from this template are
//! assigned to. If you don't specify this, jobs will go to the default queue") and
//! `UpdateJobTemplate$Queue` ("The new queue for the job template, if you are changing it")
//! both target the model's bare `__string`
//! (`smithy_query(query="operation",operation="CreateJobTemplate,UpdateJobTemplate",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`):
//! no pattern, no length and no normalisation. A queue's `Name` is this store's KEY for it
//! and is stored byte for byte, so a queue reference resolves the string the caller sent —
//! either that name or the `queues/<name>` resource id of the queue's ARN — and a spelling
//! differing only by the whitespace around it names a DIFFERENT queue, which this account
//! holds none of. One string therefore resolves to the same queue at every door that takes
//! a queue reference, and the reference a template stores is the queue a job created from
//! it is sent to. A projection reports the queue it resolved as that queue's ARN, which
//! carries the stored name verbatim, so what a reader echoes shows which queue was reached.
//!
//! `tests/all/queue_name_identity.rs` pins that for a queue's own key and for
//! `CreateJob$Queue`, `tests/all/blank_resource_name.rs` for the nested
//! `HopDestination$Queue`, and `caller_sent_key_identity`'s last case for a template
//! reference given the NAME of a padded queue. What is left unpinned, and what this case
//! adds, is the rest of the class the rule spans on the template's own member: the ARN form
//! of the same reference, the other direction of the same mistake (a stored name carrying
//! no padding, referenced padded), the refusal's wording naming the member the reference
//! arrived in, and the comparison of both template doors against `CreateJob` on the very
//! same string. One case over one table, so the shared fixture work — a queue, a template
//! and a job per row — is done once.
//!
//! The case drives the real restJson1 wire path through the crate's one test harness (the
//! router, the SigV4 scope and the reply decoding `job_lifecycle` exports), so what is
//! asserted is the bytes a client would see.

use serde_json::{json, Value};

use super::job_lifecycle::{arn, settings as job_settings, Emulator, Reply, ROLE};
use super::job_template_crud::settings as template_settings;

/// One queue name in the store, paired with the spelling that is NOT it.
struct Case {
    label: &'static str,
    /// The `Name` the fixture queue is created with — the store's key, kept verbatim.
    stored: &'static str,
    /// A spelling differing from `stored` only by the whitespace around it, so it is the
    /// name of no queue this account holds.
    neighbour: &'static str,
}

/// The rows span the forms of surrounding whitespace a bare `__string` name can carry, in
/// BOTH directions — a padded stored name referenced by its trimmed spelling, and a plain
/// stored name referenced padded — which is what the rule generalises over, plus a name
/// whose whitespace is interior (so trimming it would change nothing) as the control.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a name with no surrounding whitespace, referenced padded",
            stored: "plain-queue",
            neighbour: "  plain-queue  ",
        },
        Case {
            label: "a name with a trailing space, referenced trimmed",
            stored: "trail-queue ",
            neighbour: "trail-queue",
        },
        Case {
            label: "a name with a leading space, referenced trimmed",
            stored: " lead-queue",
            neighbour: "lead-queue",
        },
        Case {
            label: "a name padded on both sides, referenced trimmed",
            stored: "  both-queue  ",
            neighbour: "both-queue",
        },
        Case {
            label: "a name with a trailing tab, referenced trimmed",
            stored: "tab-queue\t",
            neighbour: "tab-queue",
        },
        Case {
            label: "a name with a trailing newline, referenced trimmed",
            stored: "line-queue\n",
            neighbour: "line-queue",
        },
        Case {
            label: "a name whose whitespace is interior, referenced padded",
            stored: "two words queue",
            neighbour: " two words queue ",
        },
    ]
}

/// A `JobSettings` for a job that states only its input, so every other member — the queue
/// included — comes from the template the job names.
fn job_from_template_settings() -> Value {
    json!({ "inputs": [{ "fileInput": "s3://example-bucket/in/a.mp4" }] })
}

/// How MediaConvert words a reference that resolves to nothing, spelled out here rather
/// than taken from the production helper so the sentence itself is asserted: the member the
/// reference arrived in is named, the value the caller sent is not.
const QUEUE_NOT_FOUND: &str = "The resource you specified in queue was not found.";

/// Assert `reply` is the modelled 404 for a queue reference that resolves to nothing.
fn assert_queue_not_found(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("NotFoundException"),
        "case {case:?}: expected NotFoundException, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.string("/message"),
        QUEUE_NOT_FOUND,
        "case {case:?}: expected the refusal to name the queue member, got {}",
        reply.body
    );
}

/// Create the fixture queue a row references and return its ARN.
async fn fixture_queue(emulator: &Emulator, label: &str, stored: &str) -> String {
    let created = emulator.create_queue(json!({ "name": stored })).await;
    assert_eq!(
        created.status, 201,
        "case {label:?}: the fixture queue must be created, got {} with body {}",
        created.status, created.body
    );
    assert_eq!(
        created.string("/queue/name"),
        stored,
        "case {label:?}: expected the queue stored under the name sent, got {}",
        created.body
    );
    created.string("/queue/arn")
}

/// Both job-template doors resolve `Queue` from the bytes the caller sent, in either
/// reference form: the stored queue is reached under the string that names it, the template
/// reports that queue, a job created from the template is sent to it, and the neighbouring
/// spelling — the name of no queue — is the modelled 404 at the template doors exactly as it
/// is at `CreateJob`.
#[tokio::test]
async fn a_job_template_queue_reference_resolves_the_callers_bytes() {
    let emulator = Emulator::new();

    for (index, case) in cases().into_iter().enumerate() {
        let Case {
            label,
            stored,
            neighbour,
        } = case;
        let queue_arn = fixture_queue(&emulator, label, stored).await;
        assert_eq!(
            queue_arn,
            arn(&format!("queues/{stored}")),
            "case {label:?}: expected the queue ARN to carry the name verbatim"
        );

        // The job door, on the very string the template doors are given below, so the two
        // are compared on one value rather than on two spellings.
        let job = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": stored,
                "settings": job_settings("s3://example-bucket/in/a.mp4", "_out"),
            }))
            .await;
        assert_eq!(
            job.status, 201,
            "case {label:?}: expected CreateJob to accept the reference, got {} with body {}",
            job.status, job.body
        );
        assert_eq!(
            job.string("/job/queue"),
            queue_arn,
            "case {label:?}: expected the job on the queue the reference named, got {}",
            job.body
        );

        // The create door takes the reference in either form. The ARN row is the one
        // `caller_sent_key_identity` does not drive.
        for (suffix, form, reference) in [
            ("name", "the queue's name", stored.to_string()),
            ("arn", "the queue's ARN", queue_arn.clone()),
        ] {
            let template = format!("created-{index}-{suffix}");
            let created = emulator
                .create_job_template(json!({
                    "name": template,
                    "queue": reference,
                    "settings": template_settings(1, 1),
                }))
                .await;
            assert_eq!(
                created.status, 201,
                "case {label:?} by {form}: expected HTTP 201, got {} with body {}",
                created.status, created.body
            );
            assert_eq!(
                created.string("/jobTemplate/queue"),
                queue_arn,
                "case {label:?} by {form}: expected the template to report the queue the \
                 reference resolved to, got {}",
                created.body
            );

            let fetched = emulator.get_job_template(&template).await;
            assert_eq!(
                fetched.status, 200,
                "case {label:?} by {form}: expected the template to read back, got {} with \
                 body {}",
                fetched.status, fetched.body
            );
            assert_eq!(
                fetched.string("/jobTemplate/queue"),
                queue_arn,
                "case {label:?} by {form}: expected the stored reference on a later read, \
                 got {}",
                fetched.body
            );

            // And the stored reference is the queue a job built from this template lands
            // on, which is what the member is for.
            let inherited = emulator
                .create_job(json!({
                    "role": ROLE,
                    "jobTemplate": template,
                    "settings": job_from_template_settings(),
                }))
                .await;
            assert_eq!(
                inherited.status, 201,
                "case {label:?} by {form}: expected a job from the template to be accepted, \
                 got {} with body {}",
                inherited.status, inherited.body
            );
            assert_eq!(
                inherited.string("/job/queue"),
                queue_arn,
                "case {label:?} by {form}: expected the job on the queue the template named, \
                 got {}",
                inherited.body
            );
        }

        // The neighbouring spelling is the name of no queue, so it is refused at the create
        // door exactly as it is at the job door, and persists nothing.
        let missing = format!("missing-{index}");
        assert_queue_not_found(
            &format!("{label}: CreateJobTemplate naming {neighbour:?}"),
            &emulator
                .create_job_template(json!({
                    "name": missing,
                    "queue": neighbour,
                    "settings": template_settings(1, 1),
                }))
                .await,
        );
        let fetched = emulator.get_job_template(&missing).await;
        assert_eq!(
            fetched.status, 404,
            "case {label:?}: a refused create must persist nothing, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_queue_not_found(
            &format!("{label}: CreateJob naming {neighbour:?}"),
            &emulator
                .create_job(json!({
                    "role": ROLE,
                    "queue": neighbour,
                    "settings": job_settings("s3://example-bucket/in/a.mp4", "_out"),
                }))
                .await,
        );

        // The update door resolves the same reference the same way. This template starts
        // out naming no queue, so the update is the only thing that can put one on it.
        let template = format!("updated-{index}");
        let created = emulator
            .create_job_template(json!({
                "name": template,
                "settings": template_settings(1, 1),
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the fixture template must be created, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.at("/jobTemplate/queue"),
            &Value::Null,
            "case {label:?}: the fixture template must name no queue, got {}",
            created.body
        );
        assert_queue_not_found(
            &format!("{label}: UpdateJobTemplate naming {neighbour:?}"),
            &emulator
                .update_job_template(&template, json!({ "queue": neighbour }))
                .await,
        );
        let unchanged = emulator.get_job_template(&template).await;
        assert_eq!(
            unchanged.at("/jobTemplate/queue"),
            &Value::Null,
            "case {label:?}: a refused update must change nothing, got {}",
            unchanged.body
        );

        for (form, reference) in [
            ("the queue's name", stored.to_string()),
            ("the queue's ARN", queue_arn.clone()),
        ] {
            let updated = emulator
                .update_job_template(&template, json!({ "queue": reference }))
                .await;
            assert_eq!(
                updated.status, 200,
                "case {label:?} by {form}: expected HTTP 200 from UpdateJobTemplate, got {} \
                 with body {}",
                updated.status, updated.body
            );
            assert_eq!(
                updated.string("/jobTemplate/queue"),
                queue_arn,
                "case {label:?} by {form}: expected the update to store the queue the \
                 reference resolved to, got {}",
                updated.body
            );
            let fetched = emulator.get_job_template(&template).await;
            assert_eq!(
                fetched.string("/jobTemplate/queue"),
                queue_arn,
                "case {label:?} by {form}: expected the update visible to a later read, got \
                 {}",
                fetched.body
            );
        }

        // And a job created from the updated template lands on that queue.
        let inherited = emulator
            .create_job(json!({
                "role": ROLE,
                "jobTemplate": template,
                "settings": job_from_template_settings(),
            }))
            .await;
        assert_eq!(
            inherited.status, 201,
            "case {label:?}: expected a job from the updated template to be accepted, got {} \
             with body {}",
            inherited.status, inherited.body
        );
        assert_eq!(
            inherited.string("/job/queue"),
            queue_arn,
            "case {label:?}: expected the job on the queue the update stored, got {}",
            inherited.body
        );
    }
}
