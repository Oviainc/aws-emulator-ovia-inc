// SPDX-License-Identifier: CC0-1.0

//! What `DescribeProject` reports, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules this suite pins:
//!
//! * a read answers from the stored project — every member it reports is the one
//!   the accepted create left, under the ARN form the create minted and in the
//!   portal the create named — whatever form the project's name and description
//!   take across the lengths and the character range the model admits;
//! * the read is REPORTING: reading a project any number of times moves neither of
//!   its two instants nor any other member, so the next caller sees exactly what
//!   the first one did;
//! * an id that stores no project — including the id of a portal that does
//!   exist — is the modelled not-found refusal, worded the way this service words
//!   the absence of a resource, rather than an answer about a project nothing
//!   created;
//! * an id outside the modelled length of `projectId` is refused before anything
//!   is looked up.
//!
//! The sibling suites cover the WRITES over this slice — `create_project` what an
//! accepted create stores and the create's own bounds, `project_lifecycle` what an
//! update replaces — while this one is about the READ itself, in the
//! per-operation `describe_*_reads` form the crate already has for its other
//! readers. What it adds beside them: one table whose projects live in TWO
//! different portals, so the reported `portalId` is pinned against more than one
//! container; the not-found refusal probed with the id of a PORTAL that does
//! exist, so a lookup resolving against anything but the stored projects is
//! caught; the refusal of an id outside the one modelled length of `projectId`;
//! and repeated reads checked against the store's OWN instants rather than
//! against the wire alone.
//!
//! Every helper and bound it needs already exists and is reused rather than
//! copied: the HTTP harness (`call`, `error_code`, `is_modelled_id`,
//! `router_with_engine`) from the asset suite, the portal helper from the
//! access-policy suite, the project ARN oracle from the project-delete suite, and
//! the project route, the two modelled length maxima, the create body, the reader,
//! the description assertion and the project count from `project_lifecycle`.

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::access_policy_lifecycle::create_portal;
use crate::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine};
use crate::project_delete::expected_project_arn;
use crate::project_lifecycle::{
    assert_description, create_body, describe_project, stored_project_count,
    DESCRIPTION_MAX_LENGTH, NAME_MAX_LENGTH, PROJECTS,
};

/// The one modelled length of `projectId` (smithy_query query="operation"
/// operation="DescribeProject": `projectId` (ID, len 36..36)), which every read
/// addresses a project by.
const ID_LENGTH: usize = 36;

/// One row of the read table: the project to create, and which of the two portals
/// it is created in.
#[derive(Debug)]
struct ReadCase {
    label: &'static str,
    /// Which portal the project lives in: the rows alternate so a read is checked
    /// against projects of two different containers.
    portal: usize,
    name: String,
    description: Option<String>,
}

/// The rows: the forms `projectName` and `projectDescription` admit — both
/// published length bounds, non-ASCII text, and punctuation and spaces — each
/// paired with a different combination of the optional description and with one of
/// two portals, so the read is checked against a different stored project every
/// time rather than against a renamed copy of one.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            label: "the shortest name the model admits, and no description",
            portal: 0,
            name: "p".to_string(),
            description: None,
        },
        ReadCase {
            label: "both members at the longest form the model admits",
            portal: 1,
            name: "n".repeat(NAME_MAX_LENGTH),
            description: Some("d".repeat(DESCRIPTION_MAX_LENGTH)),
        },
        ReadCase {
            label: "a name and a description of non-ascii text",
            portal: 0,
            name: "proyecto-ñandú-日本語".to_string(),
            description: Some("descripción almacenada — 保存済み".to_string()),
        },
        ReadCase {
            label: "a name of spaces and punctuation, description at the shortest form",
            portal: 1,
            name: "Floor 1 / Line 2 (v2) — monitor".to_string(),
            description: Some("d".to_string()),
        },
        ReadCase {
            label: "a name of digits alone, in the first portal, with no description",
            portal: 0,
            name: "0123456789".to_string(),
            description: None,
        },
    ]
}

/// The two instants the store holds for the project `id`, as
/// `(creationDate, lastUpdateDate)`, or `None` when the store holds no such
/// project — the only way to see that a read moved neither of them, independently
/// of what the read itself reports.
fn stored_instants(
    engine: &engine::InMemoryEngine,
    id: &str,
) -> Option<(std::time::SystemTime, std::time::SystemTime)> {
    let state = service_state(engine).expect("the iotsitewise store");
    let instants = state
        .lock()
        .projects
        .get(id)
        .map(|project| (project.creation_date, project.last_update_date));
    instants
}

#[tokio::test]
async fn a_read_reports_the_stored_project_whatever_form_its_members_take() {
    let (router, _engine) = router_with_engine();
    let portals = [
        create_portal(&router, "the-first-read-portal").await,
        create_portal(&router, "the-second-read-portal").await,
    ];

    for case in read_cases() {
        let portal_id = &portals[case.portal];
        let (status, created) = call(
            &router,
            "POST",
            PROJECTS,
            Some(create_body(
                portal_id,
                &case.name,
                case.description.as_deref(),
            )),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: CreateProject expected 201, got {status} {created}",
            case.label
        );
        let id = created["projectId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&id),
            "case {:?}: projectId must be a modelled UUID, got {created}",
            case.label
        );

        let (status, read) = describe_project(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeProject expected 200, got {status} {read}",
            case.label
        );

        assert_eq!(
            read["projectId"].as_str().unwrap_or(""),
            id,
            "case {:?}: the read must name the project it was asked for, got {read}",
            case.label
        );
        assert_eq!(
            read["projectArn"].as_str().unwrap_or(""),
            expected_project_arn(&id),
            "case {:?}: the ARN must carry the project's own id, got {read}",
            case.label
        );
        assert_eq!(
            read["projectArn"], created["projectArn"],
            "case {:?}: the read must report the identity the create minted, got {read}",
            case.label
        );
        assert_eq!(
            read["portalId"].as_str().unwrap_or(""),
            portal_id.as_str(),
            "case {:?}: a project is reported in the portal the create named, got {read}",
            case.label
        );
        assert_eq!(
            read["projectName"].as_str().unwrap_or(""),
            case.name,
            "case {:?}: the read must report the name the create stored, got {read}",
            case.label
        );
        assert_description(case.label, &read, case.description.as_deref());

        let creation_date = read["projectCreationDate"].as_f64().unwrap_or(0.0);
        let last_update_date = read["projectLastUpdateDate"].as_f64().unwrap_or(0.0);
        assert!(
            creation_date > 0.0,
            "case {:?}: projectCreationDate must be a real instant, got {read}",
            case.label
        );
        assert_eq!(
            creation_date, last_update_date,
            "case {:?}: a project no update has touched reports one instant for both, got {read}",
            case.label
        );
    }
}

#[tokio::test]
async fn reading_a_project_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-polled-project-portal").await;

    let (status, created) = call(
        &router,
        "POST",
        PROJECTS,
        Some(create_body(
            &portal_id,
            "the-polled-project",
            Some("read over and over"),
        )),
    )
    .await;
    assert_eq!(status, 201, "CreateProject expected 201, got {status} {created}");
    let id = created["projectId"].as_str().unwrap_or("").to_string();

    let instants_before =
        stored_instants(&engine, &id).unwrap_or_else(|| panic!("project {id} was not persisted"));

    let (status, first) = describe_project(&router, &id).await;
    assert_eq!(status, 200, "the first read got {status} {first}");

    for poll in 0..5 {
        let (status, read) = describe_project(&router, &id).await;
        assert_eq!(status, 200, "poll {poll}: got {status} {read}");
        assert_eq!(
            read, first,
            "poll {poll}: a read must report the project the read before it did, got {read}"
        );
    }

    let instants_after = stored_instants(&engine, &id)
        .unwrap_or_else(|| panic!("project {id} vanished on being read"));
    assert_eq!(
        instants_after, instants_before,
        "the stored instants must not move because the project was read, got {instants_after:?}"
    );

    let stored = stored_project_count(&engine);
    assert_eq!(
        stored, 1,
        "a read must neither add nor drop a project, the store holds {stored}"
    );
}

#[tokio::test]
async fn an_id_that_stores_no_project_is_the_modelled_not_found_refusal() {
    let (router, _engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-absent-project-portal").await;
    let (status, created) = call(
        &router,
        "POST",
        PROJECTS,
        Some(create_body(&portal_id, "the-only-stored-project", None)),
    )
    .await;
    assert_eq!(status, 201, "CreateProject expected 201, got {status} {created}");
    let stored_id = created["projectId"].as_str().unwrap_or("").to_string();

    // Well-formed ids that address no stored project: one no create ever minted,
    // one that differs from the stored id in a single nibble, and the id of the
    // PORTAL, which is a resource that does exist but is not a project — so a read
    // that resolved against anything but the projects it holds would answer for it.
    let mut neighbour: Vec<char> = stored_id.chars().collect();
    let at = neighbour
        .iter()
        .position(|ch| *ch != '-' && *ch != 'f')
        .expect("a character the probe can change in the stored id");
    neighbour[at] = 'f';
    let neighbour: String = neighbour.into_iter().collect();

    for case in [
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        neighbour.as_str(),
        portal_id.as_str(),
    ] {
        assert!(
            is_modelled_id(case),
            "case {case:?}: the probe id must itself be modelled"
        );
        assert_ne!(
            case, stored_id,
            "case {case:?}: the probe must not be the project that exists"
        );
        let (status, body) = describe_project(&router, case).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a read of an absent project must be a modelled 404, got {status} \
             {body}"
        );
        assert_eq!(
            body["message"].as_str().unwrap_or(""),
            format!("Project with ID {case} not found"),
            "case {case:?}: the refusal must name the id the request supplied, got {body}"
        );
    }

    // The refusals left the one stored project exactly as it was.
    let (status, read) = describe_project(&router, &stored_id).await;
    assert_eq!(status, 200, "the stored project after the refusals: got {status} {read}");
    assert_eq!(
        read["projectName"].as_str().unwrap_or(""),
        "the-only-stored-project",
        "a refused read must leave the stored project alone, got {read}"
    );
}

#[tokio::test]
async fn an_id_outside_the_modelled_length_is_refused_rather_than_answered() {
    let (router, _engine) = router_with_engine();

    // The rows walk the one published length of `projectId`: one character below it
    // and one past it, so no accepted create can ever have stored either and no
    // read may answer for them.
    //
    // Only the STATUS is asserted: both rows are refused by the boundary validator
    // before the handler runs, and the error CODE that walk renders is the subject
    // of its own issue — the same reason the sibling `project_lifecycle` suite
    // records for the bounds it walks.
    let cases: Vec<(&str, String)> = vec![
        ("one character below the modelled length", "a".repeat(ID_LENGTH - 1)),
        ("one character past the modelled length", "a".repeat(ID_LENGTH + 1)),
    ];

    for (case, id) in cases {
        let (status, body) = describe_project(&router, &id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a read outside the modelled length expected 400, got {status} {body}"
        );
    }
}
