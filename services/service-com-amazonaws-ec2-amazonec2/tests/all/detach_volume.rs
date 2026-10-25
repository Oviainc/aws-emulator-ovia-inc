// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DetachVolume`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! [`super::fpga_image_tasks_and_volume_io`] already covers which requests a detach admits
//! and refuses and that the clock frees the volume. The rules under test HERE are the three
//! the operation's doc states beyond that:
//!
//! 1. A REQUEST REFUSED FOR THE STATE OF THE RESOURCE IT NAMES answers `IncorrectState`
//!    whose message is the kind of resource, the id the request named and the state that
//!    resource is in, and nothing more — `Volume 'vol-…' is in the 'available' state.` The
//!    rows span the states a detach is refused in, the routes a volume reaches one by, and a
//!    second operation and resource kind the same code and wording come from
//!    (`CancelExportTask`);
//! 2. THE INSTANT A CLOCK-OWED MOVE FALLS DUE IS OWED BY THE CALL THAT STARTED IT: a detach
//!    of an attachment already on its way down reports it as it stands and does not push
//!    its completion further out, so a caller that polls with a detach cannot postpone it;
//! 3. AN ATTACHMENT TO AN AMAZON WEB SERVICES-MANAGED RESOURCE CANNOT BE DETACHED ("You
//!    can't detach or force detach volumes that are attached to Amazon Web Services-managed
//!    resources. Attempting to do this results in the UnsupportedOperationException
//!    exception", `DetachVolume`): refused with `UnsupportedOperation` whichever member
//!    marks the attachment as managed and whatever `Force` says, and nothing is written.
//!
//! Each table row is a distinct input along what the rule generalises over — which state the
//! resource is in, which operation and resource kind answer the refusal, which member marks
//! an attachment as managed, whether `Force` is set — so no row is the specification.

use std::time::{Instant, SystemTime};

use ec2::ops::types::AttachVolumeOutput;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, EXPORT_TASK_STATE_CANCELLED,
    EXPORT_TASK_STATE_COMPLETED, TRANSITION_DELAY, VOLUME_ATTACHMENT_STATE_ATTACHED,
    VOLUME_ATTACHMENT_STATE_DETACHING, VOLUME_STATE_AVAILABLE, VOLUME_STATE_CREATING,
    VOLUME_STATE_IN_USE, system_time_to_date_time,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};
use super::fpga_image_tasks_and_volume_io::{attach_in_store, create_volume, seed_export_task};
use super::volume_lifecycle::{REGION, p, past_the_transition};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A well-formed instance id (the long, 17 hex digit form EC2 mints today) for the
/// attachments these tests seed. `AttachVolume` is still a scaffold, so an attachment is
/// written into the store the way that operation will write it.
const INSTANCE_ID: &str = "i-0123456789abcdef0";

/// A second one, for the row that needs two attachments of different kinds.
const OTHER_INSTANCE_ID: &str = "i-0fedcba987654321f";

/// One `action` call signed for the Region the suite works in.
async fn op(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Give `volume_id` the attachment `attachment`, the way `AttachVolume` will. The
/// instance-and-device case is [`super::fpga_image_tasks_and_volume_io::attach_in_store`];
/// this one takes the whole attachment, for the managed-resource shapes rule 3 needs.
fn push_attachment(
    engine: &engine::InMemoryEngine,
    volume_id: &str,
    attachment: AttachVolumeOutput,
) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .volumes
        .get_mut(volume_id)
        .expect("the volume just created")
        .view
        .attachments
        .get_or_insert_with(Vec::new)
        .push(attachment);
}

/// An attachment of `volume_id` to an Amazon Web Services-managed resource: the instance
/// members are null ("If the volume is attached to an Amazon Web Services-managed resource,
/// this parameter returns null" on `VolumeAttachment$InstanceId` and `$Device`) and the
/// managed resource is named by whichever of the two markers this case sets.
fn managed_attachment(
    volume_id: &str,
    associated_resource: Option<&str>,
    instance_owning_service: Option<&str>,
) -> AttachVolumeOutput {
    AttachVolumeOutput {
        volume_id: Some(volume_id.to_string()),
        instance_id: None,
        device: None,
        state: Some(VOLUME_ATTACHMENT_STATE_ATTACHED.to_string()),
        attach_time: Some(system_time_to_date_time(SystemTime::now())),
        delete_on_termination: Some(false),
        associated_resource: associated_resource.map(str::to_string),
        instance_owning_service: instance_owning_service.map(str::to_string),
        ebs_card_index: Some(0),
    }
}

/// The instants the store owes this volume's pending detachments, one per attachment being
/// taken down. Read straight off the record, so rule 2 is asserted on the owed instant
/// itself rather than on how long a test slept.
fn owed_detachments(engine: &engine::InMemoryEngine, volume_id: &str) -> Vec<Instant> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .volumes
        .get(volume_id)
        .expect("the volume just created")
        .detaching_at
        .values()
        .copied()
        .collect()
}

/// Hold `volume_id` inside its `creating` -> `available` span by pushing the instant the
/// clock owes that move out, so a row about a volume still being created reads the same
/// however long the table around it takes rather than racing the clock.
fn hold_in_creating(engine: &engine::InMemoryEngine, volume_id: &str) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .volumes
        .get_mut(volume_id)
        .expect("the volume just created")
        .available_at = Instant::now() + TRANSITION_DELAY * 600;
}

/// Rule 1: a refusal for the state the named resource is in answers `IncorrectState` with
/// the kind of resource, the id and that state, and nothing more. The rows span the volume
/// states a detach is refused in — including the one a completed detach leaves behind — and
/// a second operation and resource kind the same code comes from.
#[tokio::test]
async fn an_incorrect_state_refusal_names_the_resource_and_the_state_it_is_in() {
    let (router, engine) = emulator();

    // One past its creation span, attached to nothing: `available`.
    let available = create_volume(&router).await;
    // One that WAS attached and whose detach the clock has since finished: `available` too,
    // reached by a different route.
    let detached = create_volume(&router).await;
    attach_in_store(&engine, &detached, INSTANCE_ID, "/dev/sdf");
    let (status, body) = op(&router, "DetachVolume", &[p("VolumeId", &detached)]).await;
    assert_eq!(
        status, 200,
        "seeding the detached case: expected HTTP 200, got {status} with body {body}"
    );
    past_the_transition().await;

    let completed = seed_export_task(&engine, EXPORT_TASK_STATE_COMPLETED);
    let cancelled = seed_export_task(&engine, EXPORT_TASK_STATE_CANCELLED);

    // Held inside its `creating` -> `available` span, so the row below asserts the state it
    // is really in whatever the rest of the table costs: a volume still being created
    // cannot hold an attachment, so a detach is refused with that state.
    let creating = create_volume(&router).await;
    hold_in_creating(&engine, &creating);

    let rows: Vec<(&str, &str, Params, String)> = vec![
        (
            "a volume the clock has not finished creating",
            "DetachVolume",
            vec![p("VolumeId", &creating)],
            format!("Volume '{creating}' is in the '{VOLUME_STATE_CREATING}' state."),
        ),
        (
            "a volume that was never attached",
            "DetachVolume",
            vec![p("VolumeId", &available)],
            format!("Volume '{available}' is in the '{VOLUME_STATE_AVAILABLE}' state."),
        ),
        (
            "a volume whose detach the clock already finished",
            "DetachVolume",
            vec![p("VolumeId", &detached)],
            format!("Volume '{detached}' is in the '{VOLUME_STATE_AVAILABLE}' state."),
        ),
        (
            "an export task that has completed",
            "CancelExportTask",
            vec![p("ExportTaskId", &completed)],
            format!("Export task '{completed}' is in the '{EXPORT_TASK_STATE_COMPLETED}' state."),
        ),
        (
            "an export task that was already cancelled",
            "CancelExportTask",
            vec![p("ExportTaskId", &cancelled)],
            format!("Export task '{cancelled}' is in the '{EXPORT_TASK_STATE_CANCELLED}' state."),
        ),
    ];

    for (case, action, params, expected_message) in rows {
        let (status, body) = op(&router, action, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: expected IncorrectState, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            error_message(&body)
        );
    }
}

/// Rule 2: the `detached` move is due at the instant the detach that STARTED it set, so a
/// repeat reports the attachment without postponing it — and once that instant passes the
/// volume holds no attachment and is `available`, which is what `DescribeVolumes` reports.
#[tokio::test]
async fn a_repeated_detach_does_not_postpone_the_move_it_already_owes() {
    let (router, engine) = emulator();
    let volume_id = create_volume(&router).await;
    past_the_transition().await;
    attach_in_store(&engine, &volume_id, INSTANCE_ID, "/dev/sdf");

    let (status, body) = op(&router, "DetachVolume", &[p("VolumeId", &volume_id)]).await;
    assert_eq!(
        status, 200,
        "the first detach: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "status").as_deref(),
        Some(VOLUME_ATTACHMENT_STATE_DETACHING),
        "the first detach reports the attachment as {VOLUME_ATTACHMENT_STATE_DETACHING}, \
         got {body}"
    );
    let first = owed_detachments(&engine, &volume_id);
    assert_eq!(
        first.len(),
        1,
        "the first detach owes exactly one move, got {first:?}"
    );

    // Far enough in to tell a kept deadline from a reset one, and well inside the span.
    tokio::time::sleep(TRANSITION_DELAY / 5).await;

    let (status, body) = op(&router, "DetachVolume", &[p("VolumeId", &volume_id)]).await;
    assert_eq!(
        status, 200,
        "the repeated detach: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "status").as_deref(),
        Some(VOLUME_ATTACHMENT_STATE_DETACHING),
        "the repeated detach reports the attachment it found, got {body}"
    );
    let second = owed_detachments(&engine, &volume_id);
    assert_eq!(
        second, first,
        "the repeated detach must leave the owed instant where the first call put it: \
         expected {first:?}, got {second:?}"
    );

    past_the_transition().await;

    let (_, described) = op(&router, "DescribeVolumes", &indexed("VolumeId", &[&volume_id])).await;
    assert_eq!(
        elements(&described, "status"),
        vec![VOLUME_STATE_AVAILABLE.to_string()],
        "once the owed instant passes the volume holds no attachment and is \
         {VOLUME_STATE_AVAILABLE}, got {described}"
    );
    // The attachment is gone, so there is nothing left to detach — and the next writer has
    // materialised the finished move rather than leaving an instant owed on an attachment
    // the volume no longer holds.
    let (status, body) = op(&router, "DetachVolume", &[p("VolumeId", &volume_id)]).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "IncorrectState"),
        "detaching a freed volume: expected IncorrectState, got {status} with body {body}"
    );
    assert!(
        owed_detachments(&engine, &volume_id).is_empty(),
        "the finished move is not still owed, got {:?}",
        owed_detachments(&engine, &volume_id)
    );
}

/// Rule 3: an attachment to an Amazon Web Services-managed resource is refused with
/// `UnsupportedOperation` — whichever member marks it as managed, and whether or not the
/// request forces the detachment — and the attachment is left exactly as it was.
#[tokio::test]
async fn a_volume_attached_to_a_managed_resource_cannot_be_detached() {
    let (router, engine) = emulator();
    // An ARN of the managed resource, built the way this emulator builds every ARN.
    let file_system_arn = engine::emulator_arn("fsx", REGION, "file-system/fs-0123456789abcdef0");
    // A service principal, the form `VolumeAttachment$InstanceOwningService` carries.
    let owning_service = "fsx.amazonaws.com";

    // Which member marks the attachment as managed: the model says each one is returned
    // only for a volume attached to a managed resource, so either one alone is the marker.
    let markers: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("the resource ARN alone", Some(file_system_arn.as_str()), None),
        ("the owning service alone", None, Some(owning_service)),
        (
            "both markers",
            Some(file_system_arn.as_str()),
            Some(owning_service),
        ),
    ];
    // The forms a detach of it can take. `Force` "forces detachment if the previous
    // detachment attempt did not occur cleanly" and the refusal covers a forced detach too.
    let forms: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("no other member", vec![]),
        ("Force set", vec![("Force", "true")]),
        ("Force cleared", vec![("Force", "false")]),
    ];

    for (marker, associated_resource, instance_owning_service) in markers {
        let volume_id = create_volume(&router).await;
        past_the_transition().await;
        push_attachment(
            &engine,
            &volume_id,
            managed_attachment(&volume_id, associated_resource, instance_owning_service),
        );

        for (form, extra) in &forms {
            let case = format!("{marker} with {form}");
            let mut params = vec![p("VolumeId", &volume_id)];
            params.extend(extra.iter().map(|(key, value)| p(key, *value)));
            let (status, body) = op(&router, "DetachVolume", &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnsupportedOperation",
                "case {case:?}: expected UnsupportedOperation, got {body}"
            );
            assert!(
                owed_detachments(&engine, &volume_id).is_empty(),
                "case {case:?}: a refused detach owes no move, got {:?}",
                owed_detachments(&engine, &volume_id)
            );
        }

        // The attachment is still the volume's, so the volume is still in use.
        let (_, described) =
            op(&router, "DescribeVolumes", &indexed("VolumeId", &[&volume_id])).await;
        assert!(
            elements(&described, "status").contains(&VOLUME_STATE_IN_USE.to_string()),
            "case {marker:?}: the refused volume still holds its attachment and is \
             {VOLUME_STATE_IN_USE}, got {described}"
        );
        assert!(
            elements(&described, "status").contains(&VOLUME_ATTACHMENT_STATE_ATTACHED.to_string()),
            "case {marker:?}: the attachment is left {VOLUME_ATTACHMENT_STATE_ATTACHED}, \
             got {described}"
        );
    }
}

/// Rule 3 does not swallow the ordinary case: a volume one managed resource and one instance
/// both hold is still detachable from the instance, and that detach leaves the managed
/// attachment alone.
#[tokio::test]
async fn the_managed_refusal_does_not_reach_an_instances_own_attachment() {
    let (router, engine) = emulator();
    let volume_id = create_volume(&router).await;
    past_the_transition().await;
    push_attachment(
        &engine,
        &volume_id,
        managed_attachment(&volume_id, None, Some("fsx.amazonaws.com")),
    );
    attach_in_store(&engine, &volume_id, OTHER_INSTANCE_ID, "/dev/sdf");

    // A request that names neither an instance nor a device selects BOTH attachments, so it
    // is told to narrow the selection: the managed refusal is a fact of one attachment, and
    // the request has not named one yet.
    let (status, body) = op(&router, "DetachVolume", &[p("VolumeId", &volume_id)]).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterCombination"),
        "a detach selecting the managed and the instance attachment at once: expected \
         InvalidParameterCombination, got {status} with body {body}"
    );

    let (status, body) = op(
        &router,
        "DetachVolume",
        &[
            p("VolumeId", &volume_id),
            p("InstanceId", OTHER_INSTANCE_ID),
            p("Device", "/dev/sdf"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "detaching the instance's own attachment: expected HTTP 200, got {status} with \
         body {body}"
    );
    assert_eq!(
        element(&body, "instanceId").as_deref(),
        Some(OTHER_INSTANCE_ID),
        "the detach reports the instance it was asked about, got {body}"
    );

    past_the_transition().await;

    let (_, described) = op(&router, "DescribeVolumes", &indexed("VolumeId", &[&volume_id])).await;
    assert!(
        elements(&described, "status").contains(&VOLUME_STATE_IN_USE.to_string()),
        "the managed attachment is still held, so the volume is {VOLUME_STATE_IN_USE}, \
         got {described}"
    );
    assert!(
        elements(&described, "instanceId").is_empty(),
        "no attachment to an instance is left, got {described}"
    );
}
