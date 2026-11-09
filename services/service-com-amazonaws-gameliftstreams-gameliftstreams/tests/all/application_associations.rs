// SPDX-License-Identifier: CC0-1.0

//! The two sides of an application-to-stream-group association, over the wire:
//! `DisassociateApplications` unlinking one, and `UpdateStreamGroup`'s
//! `DefaultApplicationIdentifier` promoting one that is already linked.
//!
//! Every assertion reads a RESPONSE BODY and every success is read back through
//! `GetStreamGroup` and `GetApplication`, so a handler that answered 200 without
//! writing to the store fails here.
//!
//! The rules under test, from this service's Smithy model
//! (`smithy_query(query="operation", operation="DisassociateApplications,UpdateStreamGroup")`
//! and the operations' own documentation):
//!
//! * `DisassociateApplications` (`POST /streamgroups/{Identifier}/disassociations`,
//!   `@idempotent`): "When you disassociate, or unlink, an application from a stream
//!   group, you can no longer stream this application by using that stream group's
//!   allocated compute resources … Amazon GameLift Streams will not initiate new
//!   streams in the stream group using the disassociated application … To disassociate
//!   an application, the stream group must be in ACTIVE status. If you disassociate the
//!   default application, Amazon GameLift Streams will automatically choose a new
//!   default application from the remaining associated applications."
//! * Its `Identifier` and every `ApplicationIdentifiers` entry are "an Amazon Resource
//!   Name (ARN) or ID that uniquely identifies" the resource, so both forms name the
//!   same record, and an entry that names a resource the store does not hold is the
//!   modelled `ResourceNotFoundException` (404).
//! * `UpdateStreamGroup`'s `DefaultApplicationIdentifier`: "Note that this parameter
//!   only sets the default application in a stream group. To associate a new
//!   application to an existing stream group, you must use AssociateApplications" and
//!   "The application that you specify must be in READY status" — so the member's only
//!   legal values are the applications the target group is already associated with, and
//!   any other value is the modelled `ValidationException` (400) rather than a
//!   not-found lookup.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::stream_workflow::{
    active_stream_group, call, emulator, ready_application, text, ACCOUNT, PAST_TRANSITION, REGION,
};

/// The wire error code of an error body (`__type`), or the empty string.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The error message of an error body. A modelled error renders the member's
/// `Message`; the request-validation envelope renders `message`.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One handler-side message split into the text it asserts and the per-request
/// correlation id this service appends to every one of them, which is a per-run value
/// and so is only ever checked for its form.
fn without_internal_id(message: &str, case: &str) -> String {
    let (head, suffix) = message
        .rsplit_once(" (Internal ID: ")
        .unwrap_or_else(|| panic!("case {case:?}: no internal id in {message:?}"));
    let id = suffix
        .strip_suffix(')')
        .unwrap_or_else(|| panic!("case {case:?}: unterminated internal id in {message:?}"));
    let groups: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected an 8-4-4-4-12 correlation id, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
        "case {case:?}: expected a hexadecimal correlation id, got {id:?}"
    );
    head.to_string()
}

/// An ARN of `resource_type` in this Region and account, whose resource path is `path`.
fn arn(resource_type: &str, path: &str) -> String {
    format!("arn:aws:gameliftstreams:{REGION}:{ACCOUNT}:{resource_type}/{path}")
}

/// An `Identifier` already in ARN form naming another Region and account: still a
/// legal identifier, and already its own resolved ARN.
const ELSEWHERE: &str = "arn:aws:gameliftstreams:eu-west-1:111122223333:application/a-elsewhere";

/// Create an `ACTIVE` stream group with NO default application, so its associations
/// are exactly what this suite's calls make them.
///
/// Not the harness's `active_stream_group`, which always names one: a group created
/// without a default application is what the `DefaultApplicationIdentifier` rule and
/// the empty-`ApplicationArns` case need.
async fn group_without_a_default_application(
    router: &MultiServiceRouter,
    description: &str,
    locations: &[&str],
) -> (String, String) {
    let configurations: Vec<Value> = locations
        .iter()
        .map(|location| json!({"LocationName": location, "AlwaysOnCapacity": 1}))
        .collect();
    let (status, created) = call(
        router,
        "POST",
        "/streamgroups",
        Some(json!({
            "Description": description,
            "StreamClass": "gen5n_high",
            "LocationConfigurations": configurations,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {description:?}: expected CreateStreamGroup 201, got {status} {created}"
    );
    assert_eq!(
        created["DefaultApplication"],
        Value::Null,
        "case {description:?}: a group created without one has no default application, \
         got {created}"
    );
    std::thread::sleep(PAST_TRANSITION);
    (
        text(&created, "/Id", description),
        text(&created, "/Arn", description),
    )
}

/// Associate `identifiers` with `group_id` and assert the call succeeded.
async fn associate(router: &MultiServiceRouter, group_id: &str, identifiers: &[String]) -> Value {
    let (status, got) = call(
        router,
        "POST",
        &format!("/streamgroups/{group_id}/associations"),
        Some(json!({"ApplicationIdentifiers": identifiers})),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateApplications({identifiers:?}): expected 200, got {status} {got}"
    );
    got
}

/// `GetApplication`'s view of one application.
async fn get_application(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/applications/{id}"), None).await;
    assert_eq!(
        status, 200,
        "GetApplication({id}): expected 200, got {status} {got}"
    );
    got
}

/// `GetStreamGroup`'s view of one stream group.
async fn get_stream_group(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/streamgroups/{id}"), None).await;
    assert_eq!(
        status, 200,
        "GetStreamGroup({id}): expected 200, got {status} {got}"
    );
    got
}

/// One disassociation: the case name, the identifiers the request carries, the
/// applications that stay associated afterwards and the default application the group
/// is left with.
type Row = (&'static str, Vec<String>, Vec<String>, Option<String>);

#[tokio::test]
async fn disassociating_applications_unlinks_both_sides_of_the_association() {
    let router = emulator();
    // Four applications to link, and a fifth that is never linked at all.
    let (first_id, first_arn) = ready_application(&router, "disassoc-first").await;
    let (second_id, second_arn) = ready_application(&router, "disassoc-second").await;
    let (third_id, third_arn) = ready_application(&router, "disassoc-third").await;
    let (fourth_id, fourth_arn) = ready_application(&router, "disassoc-fourth").await;
    let (never_id, never_arn) = ready_application(&router, "disassoc-never").await;
    let (group_id, group_arn) = group_without_a_default_application(
        &router,
        "disassoc-group",
        &[REGION, "us-east-1"],
    )
    .await;
    let path = format!("/streamgroups/{group_id}/disassociations");

    // "If a stream group does not already have a linked application, Amazon GameLift
    // Streams will automatically assign the first application provided in
    // ApplicationIdentifiers as the default", so the first application is the default
    // the rows below take away.
    associate(
        &router,
        &group_id,
        &[
            first_id.clone(),
            second_id.clone(),
            third_id.clone(),
            fourth_id.clone(),
        ],
    )
    .await;
    let group = get_stream_group(&router, &group_id).await;
    assert_eq!(
        group["AssociatedApplications"],
        json!([first_arn, second_arn, third_arn, fourth_arn]),
        "the fixture group starts out associated with all four applications, got {group}"
    );
    assert_eq!(
        group["DefaultApplication"]["Arn"],
        Value::String(first_arn.clone()),
        "the first application associated becomes the default, got {group}"
    );

    // The rows differ along what the rule generalises over: the FORM of each identifier
    // (a bare id, an ARN), the COUNT in one request (one, two, the same application
    // twice in two forms) and the association STATE of what is named (the default, a
    // plain association, one already unlinked, one never linked at all).
    let rows: Vec<Row> = vec![
        (
            "the default application, by bare id",
            vec![first_id.clone()],
            vec![second_arn.clone(), third_arn.clone(), fourth_arn.clone()],
            // "If you disassociate the default application, Amazon GameLift Streams
            // will automatically choose a new default application from the remaining
            // associated applications."
            Some(second_arn.clone()),
        ),
        (
            "the same application again, by ARN",
            vec![first_arn.clone()],
            vec![second_arn.clone(), third_arn.clone(), fourth_arn.clone()],
            Some(second_arn.clone()),
        ),
        (
            "two applications at once, one by ARN and one by bare id",
            vec![third_arn.clone(), fourth_id.clone()],
            vec![second_arn.clone()],
            Some(second_arn.clone()),
        ),
        (
            "the last association, named twice in both forms",
            vec![second_id.clone(), second_arn.clone()],
            vec![],
            None,
        ),
        (
            "an application that was never associated",
            vec![never_id.clone()],
            vec![],
            None,
        ),
    ];

    for (case, identifiers, remaining, default) in &rows {
        let (status, got) = call(
            &router,
            "POST",
            &path,
            Some(json!({"ApplicationIdentifiers": identifiers})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {got}"
        );
        assert_eq!(
            text(&got, "/Arn", case),
            group_arn,
            "case {case:?}: the response names the stream group, got {got}"
        );
        assert_eq!(
            got["ApplicationArns"],
            json!(remaining),
            "case {case:?}: expected the applications still associated, got {got}"
        );

        // The store, not the response: both sides of every link agree with it.
        let group = get_stream_group(&router, &group_id).await;
        assert_eq!(
            group["AssociatedApplications"],
            json!(remaining),
            "case {case:?}: GetStreamGroup must report the remaining associations, got {group}"
        );
        assert_eq!(
            group["DefaultApplication"]["Arn"],
            default.clone().map_or(Value::Null, Value::String),
            "case {case:?}: unexpected default application, got {group}"
        );
        for (id, application_arn) in [
            (&first_id, &first_arn),
            (&second_id, &second_arn),
            (&third_id, &third_arn),
            (&fourth_id, &fourth_arn),
            (&never_id, &never_arn),
        ] {
            let application = get_application(&router, id).await;
            let expected = if remaining.contains(application_arn) {
                json!([group_arn])
            } else {
                json!([])
            };
            assert_eq!(
                application["AssociatedStreamGroups"],
                expected,
                "case {case:?}: application {id} must report {expected}, got {application}"
            );
            // "Amazon GameLift Streams will not initiate new streams in the stream
            // group using the disassociated application": the content the group's
            // locations replicated for it goes with the link.
            let replications = application["ReplicationStatuses"]
                .as_array()
                .map_or(0, Vec::len);
            let wanted = if remaining.contains(application_arn) { 2 } else { 0 };
            assert_eq!(
                replications, wanted,
                "case {case:?}: application {id} must report {wanted} replication row(s), \
                 got {application}"
            );
        }
    }

    // The group itself is untouched by the unlinking: it still holds its locations and
    // its capacity, and it is still ACTIVE.
    let group = get_stream_group(&router, &group_id).await;
    assert_eq!(
        group["Status"], "ACTIVE",
        "the disassociations must not change the group's status, got {group}"
    );
    assert_eq!(
        group["LocationStates"]
            .as_array()
            .map_or(0, Vec::len),
        2,
        "\"The disassociate action does not affect the stream capacity of a stream \
         group\", got {group}"
    );
}

#[tokio::test]
async fn disassociating_refuses_what_the_documented_rules_forbid() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "refuse-disassoc-app").await;
    let (group_id, _) =
        active_stream_group(&router, "refuse-disassoc-group", &app_arn, &[REGION]).await;
    let path = format!("/streamgroups/{group_id}/disassociations");

    // An `ApplicationIdentifiers` entry that CAN name an application but names none the
    // store holds is the modelled 404, and the message names the ARN the entry resolves
    // to — a bare id against the request's Region and this account, an identifier
    // already in ARN form as the caller wrote it.
    let missing: Vec<(&str, String, String)> = vec![
        (
            "a bare application id that is not stored",
            "a-gone".to_string(),
            arn("application", "a-gone"),
        ),
        (
            "an application ARN that is not stored",
            arn("application", "a-also-gone"),
            arn("application", "a-also-gone"),
        ),
        (
            "an application ARN in another Region and account",
            ELSEWHERE.to_string(),
            ELSEWHERE.to_string(),
        ),
    ];
    for (case, identifier, resolved) in &missing {
        let (status, got) = call(
            &router,
            "POST",
            &path,
            Some(json!({"ApplicationIdentifiers": [identifier]})),
        )
        .await;
        assert_eq!(
            (status, error_code(&got)),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {got}"
        );
        assert_eq!(
            without_internal_id(&error_message(&got), case),
            format!("Resource '{resolved}' does not exist"),
            "case {case:?}: unexpected message, got {got}"
        );
    }

    // One unresolvable entry refuses the whole request, so the association it names
    // beside it is left in place.
    let (status, got) = call(
        &router,
        "POST",
        &path,
        Some(json!({"ApplicationIdentifiers": [app_id.clone(), "a-gone"]})),
    )
    .await;
    assert_eq!(
        (status, error_code(&got)),
        (404, "ResourceNotFoundException".to_string()),
        "case \"one unresolvable entry\": expected 404, got {status} {got}"
    );

    // The stream group named by the path label is resolved by the same rule.
    let (status, got) = call(
        &router,
        "POST",
        "/streamgroups/sg-gone/disassociations",
        Some(json!({"ApplicationIdentifiers": [app_id.clone()]})),
    )
    .await;
    assert_eq!(
        (status, error_code(&got)),
        (404, "ResourceNotFoundException".to_string()),
        "case \"a stream group that is not stored\": expected 404, got {status} {got}"
    );
    assert_eq!(
        without_internal_id(&error_message(&got), "a stream group that is not stored"),
        format!("Resource '{}' does not exist", arn("streamgroup", "sg-gone")),
        "case \"a stream group that is not stored\": unexpected message, got {got}"
    );

    // An identifier that names no application at all is a bad VALUE for the member,
    // which is a 400 and not a lookup.
    let (status, got) = call(
        &router,
        "POST",
        &path,
        Some(json!({"ApplicationIdentifiers": [arn("streamgroup", &group_id)]})),
    )
    .await;
    assert_eq!(
        (status, error_code(&got)),
        (400, "ValidationException".to_string()),
        "case \"a stream group ARN as the application\": expected 400, got {status} {got}"
    );

    // "To disassociate an application, the stream group must be in ACTIVE status." This
    // operation models no conflict, so the refusal is the client-input error it does
    // model.
    let (activating_id, _) = {
        let (status, created) = call(
            &router,
            "POST",
            "/streamgroups",
            Some(json!({
                "Description": "activating-disassoc-group",
                "StreamClass": "gen5n_high",
                "DefaultApplicationIdentifier": app_id.clone(),
                "LocationConfigurations": [{"LocationName": REGION, "AlwaysOnCapacity": 1}],
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "creating an ACTIVATING group: expected 201, got {status} {created}"
        );
        (
            text(&created, "/Id", "an ACTIVATING group"),
            text(&created, "/Arn", "an ACTIVATING group"),
        )
    };
    let (status, got) = call(
        &router,
        "POST",
        &format!("/streamgroups/{activating_id}/disassociations"),
        Some(json!({"ApplicationIdentifiers": [app_id.clone()]})),
    )
    .await;
    assert_eq!(
        (status, error_code(&got)),
        (400, "ValidationException".to_string()),
        "case \"an ACTIVATING stream group\": expected 400, got {status} {got}"
    );

    // None of those refusals wrote anything: both sides still report the association.
    let group = get_stream_group(&router, &group_id).await;
    assert_eq!(
        group["AssociatedApplications"],
        json!([app_arn]),
        "the refusals must leave the association in place, got {group}"
    );
    let application = get_application(&router, &app_id).await;
    assert_eq!(
        application["AssociatedStreamGroups"]
            .as_array()
            .map_or(0, Vec::len),
        2,
        "the application is still linked to both groups, got {application}"
    );

    // And once the clock has made the second group ACTIVE, the same call succeeds.
    std::thread::sleep(PAST_TRANSITION);
    let (status, got) = call(
        &router,
        "POST",
        &format!("/streamgroups/{activating_id}/disassociations"),
        Some(json!({"ApplicationIdentifiers": [app_id.clone()]})),
    )
    .await;
    assert_eq!(
        (status, &got["ApplicationArns"]),
        (200, &json!([])),
        "case \"the group is ACTIVE now\": expected 200 with no association left, got \
         {status} {got}"
    );
}

#[tokio::test]
async fn a_default_application_the_group_is_not_associated_with_is_a_validation_error() {
    let router = emulator();
    let (associated_id, associated_arn) = ready_application(&router, "default-associated").await;
    // A READY application no stream group is associated with.
    let (unassociated_id, unassociated_arn) =
        ready_application(&router, "default-unassociated").await;
    let (group_id, _) =
        group_without_a_default_application(&router, "default-group", &[REGION]).await;
    let path = format!("/streamgroups/{group_id}");

    // The rows differ along what the rule generalises over: whether an application of
    // that identifier exists at all, and the FORM of the identifier (a bare id, an ARN
    // in this Region and account, an ARN elsewhere). Every one of them resolves to an
    // application the group holds no association for, so every one of them is the same
    // refusal.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a bare id no application matches",
            "a-gone".to_string(),
            arn("application", "a-gone"),
        ),
        (
            "an application ARN no application matches",
            arn("application", "a-also-gone"),
            arn("application", "a-also-gone"),
        ),
        (
            "an application ARN in another Region and account",
            ELSEWHERE.to_string(),
            ELSEWHERE.to_string(),
        ),
        (
            "a stored application the group is not associated with, by bare id",
            unassociated_id.clone(),
            unassociated_arn.clone(),
        ),
        (
            "a stored application the group is not associated with, by ARN",
            unassociated_arn.clone(),
            unassociated_arn.clone(),
        ),
    ];
    for (case, identifier, resolved) in &rows {
        let (status, got) = call(
            &router,
            "PATCH",
            &path,
            Some(json!({"DefaultApplicationIdentifier": identifier})),
        )
        .await;
        assert_eq!(
            (status, error_code(&got)),
            (400, "ValidationException".to_string()),
            "case {case:?}: expected 400 ValidationException, got {status} {got}"
        );
        assert_eq!(
            without_internal_id(&error_message(&got), case),
            format!(
                "Application '{resolved}' cannot be set as default because it is not \
                 currently associated with the stream group"
            ),
            "case {case:?}: unexpected message, got {got}"
        );

        // The refused call leaves the group exactly as it was.
        let group = get_stream_group(&router, &group_id).await;
        assert_eq!(
            group["DefaultApplication"],
            Value::Null,
            "case {case:?}: a refused update must not set a default application, got {group}"
        );
        assert_eq!(
            group["Description"], "default-group",
            "case {case:?}: a refused update must not change the description, got {group}"
        );
    }

    // A plain `Description` update on the same group still succeeds after the refusals.
    let (status, updated) = call(
        &router,
        "PATCH",
        &path,
        Some(json!({"Description": "default-group-renamed"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a Description update after the refusals\": expected 200, got {status} {updated}"
    );
    let group = get_stream_group(&router, &group_id).await;
    assert_eq!(
        group["Description"], "default-group-renamed",
        "case \"a Description update after the refusals\": got {group}"
    );

    // The other side of the rule: an application the group IS associated with is
    // promoted, and once it is unlinked again the same identifier is refused once more.
    associate(&router, &group_id, std::slice::from_ref(&associated_id)).await;
    let (status, updated) = call(
        &router,
        "PATCH",
        &path,
        Some(json!({"DefaultApplicationIdentifier": associated_id})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an associated application\": expected 200, got {status} {updated}"
    );
    assert_eq!(
        updated["DefaultApplication"],
        json!({"Id": associated_id, "Arn": associated_arn}),
        "case \"an associated application\": expected the stored record, got {updated}"
    );
    let group = get_stream_group(&router, &group_id).await;
    assert_eq!(
        group["DefaultApplication"]["Arn"],
        Value::String(associated_arn.clone()),
        "case \"an associated application\": GetStreamGroup must report it, got {group}"
    );

    let (status, got) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/disassociations"),
        Some(json!({"ApplicationIdentifiers": [associated_id.clone()]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"unlink the default application\": expected 200, got {status} {got}"
    );
    let (status, got) = call(
        &router,
        "PATCH",
        &path,
        Some(json!({"DefaultApplicationIdentifier": associated_arn})),
    )
    .await;
    assert_eq!(
        (status, error_code(&got)),
        (400, "ValidationException".to_string()),
        "case \"the application after it was unlinked\": expected 400, got {status} {got}"
    );
}
