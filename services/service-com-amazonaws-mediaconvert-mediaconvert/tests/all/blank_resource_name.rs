// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the rule that a member naming a resource must carry at
//! least one character.
//!
//! Every such member in this service targets the model's bare `__string`
//! (`smithy_query(query="operation",operation="CreateQueue,CreatePreset,CreateJobTemplate,CreateJob",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`):
//! no pattern, no length and no normalisation, so the framework's boundary validation
//! passes the zero-length string and a value made only of whitespace alike. Neither
//! carries a resource identity — a key with no characters addresses nothing a later read
//! can reach, and a reference with none resolves to nothing — so every door that mints or
//! resolves a named resource answers such a value with `BadRequestException` (400) naming
//! the member, and leaves the store exactly as it was.
//!
//! The rule holds at every depth a queue reference arrives at: the hop-destination case
//! below drives it through `HopDestination$Queue`, which points at a queue resource exactly
//! as the top-level `Queue` does, on all three operations that accept a hop list. It holds
//! for a reference whatever the request does with the resource it resolves to, so the
//! share-and-tagging case covers `CreateResourceShare$JobId` and the `Arn` of the three
//! tagging doors as well. The doors that DELETE a named resource carry the same kind of
//! `{Name}` label, and `tests/all/blank_delete_name.rs` pins the rule there, reusing this
//! suite's blank forms and door table.
//!
//! The other side of the same rule is that a value which DOES carry characters is never
//! normalised: surrounding whitespace is part of the key, so a padded name creates its own
//! resource and reads back verbatim (`tests/all/queue_name_identity.rs` pins that for
//! queues; the second case below pins it for all three named resources at once).
//!
//! Each case drives the real restJson1 wire path through the crate's one test harness (the
//! router, the SigV4 scope and the reply decoding `job_lifecycle` exports), so what is
//! asserted is the bytes a client would see.

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::not_found_message;

use super::job_lifecycle::{
    projected_hop_destinations, settings as job_settings, Emulator, Reply, ROLE,
};
use super::job_template_crud::settings as template_settings;
use super::resource_tagging::encode_label;

/// A value a resource-naming member can be made ENTIRELY of, which is what the rule
/// generalises over: whether there are any characters at all, which whitespace character is
/// used, how many there are, and whether they are mixed.
pub(crate) struct Blank {
    pub(crate) label: &'static str,
    /// The member value exactly as the request body carries it.
    pub(crate) value: &'static str,
    /// `value` percent-encoded for a `{Name}` path label, so a case can try to READ the
    /// resource a refused create must not have stored, and the same encoding a QUERY
    /// member's value carries (`tests/all/blank_filter_value.rs` drives the rule's
    /// reporting half through those, and `tests/all/blank_delete_name.rs` drives it through
    /// the three delete doors' `{Name}`). Empty for the zero-length value, which is no path
    /// label at all.
    pub(crate) path: &'static str,
}

/// The blank forms every row below is driven through.
pub(crate) fn blanks() -> Vec<Blank> {
    vec![
        Blank {
            label: "no characters at all",
            value: "",
            path: "",
        },
        Blank {
            label: "one space",
            value: " ",
            path: "%20",
        },
        Blank {
            label: "three spaces",
            value: "   ",
            path: "%20%20%20",
        },
        Blank {
            label: "one tab",
            value: "\t",
            path: "%09",
        },
        Blank {
            label: "one newline",
            value: "\n",
            path: "%0A",
        },
        Blank {
            label: "one carriage return",
            value: "\r",
            path: "%0D",
        },
        // `str::trim` is defined over `char::is_whitespace`, so the vertical tab and the
        // form feed are whitespace exactly as a space is.
        Blank {
            label: "one vertical tab",
            value: "\u{000B}",
            path: "%0B",
        },
        Blank {
            label: "one form feed",
            value: "\u{000C}",
            path: "%0C",
        },
        Blank {
            label: "a mixture of spaces, a tab and a newline",
            value: " \t\n ",
            path: "%20%09%0A%20",
        },
    ]
}

/// One door that MINTS a named resource: the collection it posts to, what a valid request
/// carries beside `Name`, and how the resource is reported.
pub(crate) struct Door {
    /// The operation, for the case label.
    pub(crate) label: &'static str,
    /// The collection URI: `POST` creates, `GET` lists, `GET <uri>/{Name}` reads one.
    pub(crate) collection: &'static str,
    /// The required members a request to this door carries BESIDE `Name`, so the only thing
    /// wrong with a refused row is the name itself.
    pub(crate) extra: Value,
    /// The member a single-resource reply reports the resource in.
    pub(crate) view: &'static str,
    /// The member a listing reports its records in.
    pub(crate) listing: &'static str,
}

/// The three named resources this service mints, each with the smallest request that is
/// valid apart from its `Name`.
pub(crate) fn doors() -> Vec<Door> {
    vec![
        Door {
            label: "CreateQueue",
            collection: "/2017-08-29/queues",
            extra: json!({}),
            view: "queue",
            listing: "queues",
        },
        Door {
            label: "CreatePreset",
            collection: "/2017-08-29/presets",
            // A preset's settings must name the container the output is written in.
            extra: json!({ "settings": { "containerSettings": { "container": "MP4" } } }),
            view: "preset",
            listing: "presets",
        },
        Door {
            label: "CreateJobTemplate",
            collection: "/2017-08-29/jobTemplates",
            extra: json!({ "settings": template_settings(1, 1) }),
            view: "jobTemplate",
            listing: "jobTemplates",
        },
    ]
}

/// A create request for `door` naming `name`.
pub(crate) fn create_request(door: &Door, name: &str) -> Value {
    let mut body = door.extra.clone();
    body.as_object_mut()
        .expect("the fixture request is a JSON object")
        .insert("name".to_string(), json!(name));
    body
}

/// How this service words the refusal of a member that carries no characters — shared
/// with the suites that drive the other halves of the same rule, so none of them can
/// assert a different sentence.
pub(crate) fn refusal_message(member: &str) -> String {
    format!("{member} must not be empty.")
}

/// Assert `reply` is the modelled bad request naming `member`.
pub(crate) fn assert_blank_refusal(case: &str, member: &str, reply: &Reply) {
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
    assert_eq!(
        reply.string("/message"),
        refusal_message(member),
        "case {case:?}: expected the refusal to name {member}, got body {}",
        reply.body
    );
}

/// The names a collection's listing reports.
pub(crate) async fn listed_names(emulator: &Emulator, door: &Door) -> Vec<String> {
    let listed = emulator.send("GET", door.collection, None).await;
    assert_eq!(
        listed.status, 200,
        "case {:?}: listing must answer HTTP 200, got {} with body {}",
        door.label, listed.status, listed.body
    );
    listed
        .at(&format!("/{}", door.listing))
        .as_array()
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record
                        .pointer("/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// No door that mints a named resource stores one whose name carries no characters: each
/// blank form is the modelled bad request, nothing is readable under it afterwards, and the
/// listing reports no such record.
#[tokio::test]
async fn a_name_carrying_no_characters_is_refused_by_every_named_create() {
    let emulator = Emulator::new();

    for door in doors() {
        for blank in blanks() {
            let case = format!("{} with a name of {}", door.label, blank.label);
            let refused = emulator
                .send(
                    "POST",
                    door.collection,
                    Some(create_request(&door, blank.value)),
                )
                .await;
            assert_blank_refusal(&case, "Name", &refused);

            if !blank.path.is_empty() {
                let fetched = emulator
                    .send(
                        "GET",
                        &format!("{}/{}", door.collection, blank.path),
                        None,
                    )
                    .await;
                assert_eq!(
                    fetched.status, 404,
                    "case {case:?}: a refused create must persist nothing, got {} with \
                     body {}",
                    fetched.status, fetched.body
                );
            }
        }

        let blank_names: Vec<String> = listed_names(&emulator, &door)
            .await
            .into_iter()
            .filter(|name| name.trim().is_empty())
            .collect();
        assert!(
            blank_names.is_empty(),
            "case {:?}: the listing must report no record whose name carries no \
             characters, got {blank_names:?}",
            door.label
        );
    }
}

/// The boundary of the same rule: a name that carries characters is stored and addressed
/// byte for byte, surrounding whitespace included — so only a name made ENTIRELY of
/// whitespace is refused, and nothing is trimmed on the way in.
#[tokio::test]
async fn a_name_that_carries_characters_keeps_its_whitespace() {
    let emulator = Emulator::new();

    // Each row carries at least one character, padded with the forms the refused table
    // above is made only of.
    let kept: [(&str, &str, &str); 5] = [
        ("no padding", "plain", "plain"),
        ("a trailing space", "trail ", "trail%20"),
        ("a leading space", " lead", "%20lead"),
        ("spaces on both sides", "  both  ", "%20%20both%20%20"),
        ("a trailing tab and newline", "padded\t\n", "padded%09%0A"),
    ];

    for door in doors() {
        for (label, name, path) in kept {
            let case = format!("{} with {label}", door.label);
            let created = emulator
                .send("POST", door.collection, Some(create_request(&door, name)))
                .await;
            assert_eq!(
                created.status, 201,
                "case {case:?}: expected HTTP 201, got {} with body {}",
                created.status, created.body
            );
            assert_eq!(
                created.string(&format!("/{}/name", door.view)),
                name,
                "case {case:?}: expected the name echoed verbatim, got body {}",
                created.body
            );

            let fetched = emulator
                .send("GET", &format!("{}/{}", door.collection, path), None)
                .await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: expected the created resource to read back under its own \
                 name, got {} with body {}",
                fetched.status, fetched.body
            );
            assert_eq!(
                fetched.at(&format!("/{}", door.view)),
                created.at(&format!("/{}", door.view)),
                "case {case:?}: expected the stored resource, got body {}",
                fetched.body
            );
        }
    }
}

/// Assert `reply` is the lookup refusal this service words for a hop destination whose
/// `Queue` carries characters but names no stored queue.
fn assert_hop_not_found(case: &str, reply: &Reply) {
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
        not_found_message("hopDestinations"),
        "case {case:?}: expected the member-naming lookup refusal, got body {}",
        reply.body
    );
}

/// The same rule on the members that REFERENCE a named resource rather than mint one: a
/// reference carrying no characters resolves to nothing, so it is the modelled bad request
/// naming the member, never a stored record pointing at nothing.
#[tokio::test]
async fn a_resource_reference_carrying_no_characters_is_refused() {
    let emulator = Emulator::new();

    // A template a blank `Queue` can be sent to an update of.
    let fixture = emulator
        .create_job_template(json!({
            "name": "referencing-template",
            "settings": template_settings(1, 1),
        }))
        .await;
    assert_eq!(
        fixture.status, 201,
        "the fixture template must be created: got {} with body {}",
        fixture.status, fixture.body
    );

    for (index, blank) in blanks().into_iter().enumerate() {
        let job_settings = job_settings("s3://example-bucket/in/a.mp4", "_out");

        // `CreateJob$Role` is required, and a role name with no characters is no role.
        assert_blank_refusal(
            &format!("CreateJob with a role of {}", blank.label),
            "Role",
            &emulator
                .create_job(json!({ "role": blank.value, "settings": job_settings.clone() }))
                .await,
        );

        // `CreateJob$Queue` and `CreateJob$JobTemplate` are optional, so a value the caller
        // SENT is one they meant: a blank one is refused rather than resolved or dropped.
        assert_blank_refusal(
            &format!("CreateJob with a queue of {}", blank.label),
            "Queue",
            &emulator
                .create_job(json!({
                    "role": ROLE,
                    "queue": blank.value,
                    "settings": job_settings.clone(),
                }))
                .await,
        );
        assert_blank_refusal(
            &format!("CreateJob with a job template of {}", blank.label),
            "JobTemplate",
            &emulator
                .create_job(json!({
                    "role": ROLE,
                    "jobTemplate": blank.value,
                    "settings": job_settings.clone(),
                }))
                .await,
        );

        // And on both doors into a stored job template, whose `Queue` is the same kind of
        // reference.
        let name = format!("blank-queue-{index}");
        assert_blank_refusal(
            &format!("CreateJobTemplate with a queue of {}", blank.label),
            "Queue",
            &emulator
                .create_job_template(json!({
                    "name": name,
                    "queue": blank.value,
                    "settings": template_settings(1, 1),
                }))
                .await,
        );
        let fetched = emulator.get_job_template(&name).await;
        assert_eq!(
            fetched.status, 404,
            "case CreateJobTemplate with a queue of {}: a refused create must persist \
             nothing, got {} with body {}",
            blank.label, fetched.status, fetched.body
        );
        assert_blank_refusal(
            &format!("UpdateJobTemplate with a queue of {}", blank.label),
            "Queue",
            &emulator
                .update_job_template("referencing-template", json!({ "queue": blank.value }))
                .await,
        );
    }

    // The fixture is untouched by every refusal above: it still names no queue.
    let after = emulator.get_job_template("referencing-template").await;
    assert_eq!(
        after.status, 200,
        "the fixture template must still be readable, got {} with body {}",
        after.status, after.body
    );
    assert_eq!(
        after.at("/jobTemplate/queue"),
        &Value::Null,
        "a refused update must change nothing, got body {}",
        after.body
    );
}

/// The doors whose reference member arrives in the REQUEST BODY or as a path label but
/// does not mint or address a named resource of its own: the job a share names and the
/// resource a tagging call works on. Both are references exactly as `CreateJob$Queue` is,
/// so the same rule holds — a value carrying no characters is the modelled bad request
/// naming the member, never a lookup of a value nothing could hold.
///
/// The table spans every blank form, both reference-carrying members of
/// `CreateResourceShare` crossed (so the refusal of one does not depend on the other), and
/// all three tagging doors; the boundary rows at the end are references that DO carry
/// characters, which the same doors serve.
#[tokio::test]
async fn a_reference_carrying_no_characters_is_refused_by_the_share_and_tagging_doors() {
    let emulator = Emulator::new();

    // One stored job and one stored queue, so every refusal below is about the reference
    // and not about an empty store.
    let created = emulator
        .create_job(json!({
            "role": ROLE,
            "settings": job_settings("s3://example-bucket/in/a.mp4", "_out"),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture job must be created: got {} with body {}",
        created.status, created.body
    );
    let job_id = created.string("/job/id");
    let queue = emulator.create_queue(json!({ "name": "tagged-queue" })).await;
    assert_eq!(
        queue.status, 201,
        "the fixture queue must be created: got {} with body {}",
        queue.status, queue.body
    );
    let queue_arn = queue.string("/queue/arn");

    for blank in blanks() {
        // `CreateResourceShare$JobId` is "Specify MediaConvert Job ID or ARN to share": a
        // value with no characters is neither, so it names no job. The sibling
        // `SupportCaseId` is well-formed here, and blank in the second row, so neither
        // refusal depends on the other member's value.
        for (row, support_case_id) in [
            ("a well-formed support case", "case-42"),
            ("a blank support case too", blank.value),
        ] {
            let refused = share_request(&emulator, blank.value, support_case_id).await;
            assert_blank_refusal(
                &format!(
                    "CreateResourceShare with a job id of {} and {row}",
                    blank.label
                ),
                "JobId",
                &refused,
            );
        }

        // Nothing was recorded against the job the refused request could not name.
        let read = emulator.get_job(&job_id).await;
        assert_eq!(
            read.at("/job/lastShareDetails"),
            &Value::Null,
            "case CreateResourceShare with a job id of {}: a refused share must record \
             nothing, got body {}",
            blank.label,
            read.body
        );

        // `TagResource$Arn` arrives in the body; `ListTagsForResource` and
        // `UntagResource` carry it as a path label, which only a value with at least one
        // character is addressable as at all.
        assert_blank_refusal(
            &format!("TagResource with an arn of {}", blank.label),
            "Arn",
            &emulator
                .send(
                    "POST",
                    TAGS_URI,
                    Some(json!({ "arn": blank.value, "tags": { "env": "prod" } })),
                )
                .await,
        );
        if !blank.path.is_empty() {
            let addressed = format!("{TAGS_URI}/{}", blank.path);
            assert_blank_refusal(
                &format!("ListTagsForResource with an arn of {}", blank.label),
                "Arn",
                &emulator.send("GET", &addressed, None).await,
            );
            assert_blank_refusal(
                &format!("UntagResource with an arn of {}", blank.label),
                "Arn",
                &emulator
                    .send("PUT", &addressed, Some(json!({ "tagKeys": ["env"] })))
                    .await,
            );
        }
    }

    // The boundary of the same rule on these doors: a reference that carries characters is
    // served. The share is recorded against the job the request named, and the tagging
    // doors reach the queue the ARN names.
    let accepted = share_request(&emulator, &job_id, "case-42").await;
    assert_eq!(
        accepted.status, 202,
        "case \"a job id that carries characters\": expected HTTP 202, got {} with body {}",
        accepted.status, accepted.body
    );
    let read = emulator.get_job(&job_id).await;
    assert!(
        read.string("/job/lastShareDetails").contains("case-42"),
        "case \"a job id that carries characters\": the share must be recorded against the \
         job, got body {}",
        read.body
    );
    let tagged = emulator
        .send(
            "POST",
            TAGS_URI,
            Some(json!({ "arn": queue_arn, "tags": { "env": "prod" } })),
        )
        .await;
    assert_eq!(
        tagged.status, 200,
        "case \"an arn that carries characters\": expected HTTP 200, got {} with body {}",
        tagged.status, tagged.body
    );
    let listed = emulator
        .send("GET", &format!("{TAGS_URI}/{}", encode_label(&queue_arn)), None)
        .await;
    assert_eq!(
        listed.at("/resourceTags/tags/env"),
        &json!("prod"),
        "case \"an arn that carries characters\": the stored tag must read back, got body {}",
        listed.body
    );
}

/// The tagging collection every tagging door hangs off.
const TAGS_URI: &str = "/2017-08-29/tags";

/// `CreateResourceShare` for `job_reference` and `support_case_id`.
async fn share_request(emulator: &Emulator, job_reference: &str, support_case_id: &str) -> Reply {
    emulator
        .send(
            "POST",
            "/2017-08-29/resourceShares",
            Some(json!({ "jobId": job_reference, "supportCaseId": support_case_id })),
        )
        .await
}

/// The queue the jobs in the hop-destination case are submitted to. A hop destination
/// "cannot be the original queue to which the job is submitted", so the source queue is one
/// the fixture creates rather than the account's default.
const HOP_SOURCE_QUEUE: &str = "blank-hop-source";

/// A stored queue whose own name is padded with whitespace: the other side of the rule
/// keeps it, so a hop naming it resolves byte for byte while its trimmed spelling names no
/// queue at all.
const PADDED_HOP_QUEUE: &str = "  padded hop  ";

/// What the doors that accept a hop list answer for one row of it.
enum Expect {
    /// The list sets up a hop: the request is accepted and the list is stored.
    Accepted,
    /// A `Queue` carrying no characters: the modelled bad request naming the member.
    BlankRefusal,
    /// A `Queue` that carries characters but names no stored queue: the lookup refusal.
    NotFound,
}

/// `HopDestination$Queue` is a queue reference exactly as the top-level `Queue` is, so the
/// same rule holds there: an entry stating a queue that carries no characters is the
/// modelled bad request naming the member, never a lookup of a name nothing could hold.
///
/// The table spans what the rule generalises over: every blank form, the entry's position
/// in the list (the only one, and a later one after a resolvable entry), and — as the
/// boundary — an entry that names a stored queue whose name is itself padded, that queue's
/// trimmed spelling (which names nothing, so it is the LOOKUP refusal), and an entry that
/// names no queue at all (the member left out, which moves a job to the default queue).
/// Every row is driven through all three operations that carry `HopDestinations`, so the
/// rule is asserted wherever it holds.
#[tokio::test]
async fn a_hop_destination_queue_carrying_no_characters_is_refused() {
    let emulator = Emulator::new();
    for name in [HOP_SOURCE_QUEUE, PADDED_HOP_QUEUE] {
        let created = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            created.status, 201,
            "the fixture queue {name:?} must be created: got {} with body {}",
            created.status, created.body
        );
    }
    let fixture = emulator
        .create_job_template(json!({
            "name": "hopping-template",
            "settings": template_settings(1, 1),
        }))
        .await;
    assert_eq!(
        fixture.status, 201,
        "the fixture template must be created: got {} with body {}",
        fixture.status, fixture.body
    );

    let mut rows: Vec<(String, Value, Expect)> = Vec::new();
    for blank in blanks() {
        rows.push((
            format!("the only entry states a queue of {}", blank.label),
            json!([{ "queue": blank.value, "waitMinutes": 10 }]),
            Expect::BlankRefusal,
        ));
        rows.push((
            format!(
                "a second entry states a queue of {}, after a resolvable one",
                blank.label
            ),
            json!([
                { "queue": PADDED_HOP_QUEUE, "waitMinutes": 10 },
                { "queue": blank.value, "waitMinutes": 20 },
            ]),
            Expect::BlankRefusal,
        ));
    }
    rows.push((
        "an entry naming the stored queue whose name is padded".to_string(),
        json!([{ "queue": PADDED_HOP_QUEUE, "waitMinutes": 10 }]),
        Expect::Accepted,
    ));
    rows.push((
        "an entry naming that queue's trimmed spelling".to_string(),
        json!([{ "queue": PADDED_HOP_QUEUE.trim(), "waitMinutes": 10 }]),
        Expect::NotFound,
    ));
    rows.push((
        "an entry naming no queue at all".to_string(),
        json!([{ "waitMinutes": 10 }]),
        Expect::Accepted,
    ));

    for (index, (case, hops, expect)) in rows.into_iter().enumerate() {
        // What the addressed template carries BEFORE the update, so a refused update can be
        // shown to have changed nothing whatever an earlier row left there.
        let before = emulator.get_job_template("hopping-template").await;
        let before_hops = before.at("/jobTemplate/hopDestinations").clone();

        let job = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": HOP_SOURCE_QUEUE,
                "hopDestinations": hops.clone(),
                "settings": job_settings("s3://example-bucket/in/a.mp4", "_a"),
            }))
            .await;
        let created_name = format!("hopping-{index}");
        let template = emulator
            .create_job_template(json!({
                "name": created_name,
                "hopDestinations": hops.clone(),
                "settings": template_settings(1, 1),
            }))
            .await;
        let updated = emulator
            .update_job_template("hopping-template", json!({ "hopDestinations": hops.clone() }))
            .await;

        match expect {
            Expect::Accepted => {
                for (operation, reply, accepted) in [
                    ("CreateJob", &job, 201),
                    ("CreateJobTemplate", &template, 201),
                    ("UpdateJobTemplate", &updated, 200),
                ] {
                    assert_eq!(
                        reply.status, accepted,
                        "case {operation}: {case:?}: expected HTTP {accepted}, got {} with \
                         body {}",
                        reply.status, reply.body
                    );
                }
                assert_eq!(
                    job.at("/job/hopDestinations"),
                    &projected_hop_destinations(&hops),
                    "case {case:?}: the created job must carry the hop list it was given, \
                     got body {}",
                    job.body
                );
                let after = emulator.get_job_template("hopping-template").await;
                assert_eq!(
                    after.at("/jobTemplate/hopDestinations"),
                    &projected_hop_destinations(&hops),
                    "case {case:?}: an accepted update must store the hop list it was given, \
                     got body {}",
                    after.body
                );
                continue;
            }
            Expect::BlankRefusal => {
                for (operation, reply) in [
                    ("CreateJob", &job),
                    ("CreateJobTemplate", &template),
                    ("UpdateJobTemplate", &updated),
                ] {
                    assert_blank_refusal(
                        &format!("{operation}: {case}"),
                        "HopDestinations.Queue",
                        reply,
                    );
                }
            }
            Expect::NotFound => {
                for (operation, reply) in [
                    ("CreateJob", &job),
                    ("CreateJobTemplate", &template),
                    ("UpdateJobTemplate", &updated),
                ] {
                    assert_hop_not_found(&format!("{operation}: {case}"), reply);
                }
            }
        }

        // Every refusal above left the store exactly as it was: no template under the name
        // the refused create named, and the addressed template's hop list unchanged.
        let fetched = emulator.get_job_template(&created_name).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: a refused create must persist nothing, got {} with body {}",
            fetched.status, fetched.body
        );
        let after = emulator.get_job_template("hopping-template").await;
        assert_eq!(
            after.at("/jobTemplate/hopDestinations"),
            &before_hops,
            "case {case:?}: a refused update must change nothing, got body {}",
            after.body
        );
    }
}
