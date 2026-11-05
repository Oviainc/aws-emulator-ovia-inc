// SPDX-License-Identifier: CC0-1.0

//! `ListContactVersions`: the version history of ONE contact, end to end over HTTP.
//!
//! The RULE under test: `GET /contact/{contactId}/versions` reports the version history
//! the store holds for the contact that label names — every version record and nothing
//! else, oldest first (ascending `versionId`) — one page bounded by `maxResults` at a
//! time, resumable with a `nextToken` scoped to this listing and that contact; a scope
//! with nothing to report is an EMPTY PAGE rather than a refusal, so a `contactId` naming
//! no contact is answered HTTP 200 with `contactVersionsList: []` and no `nextToken`. Each
//! record is the stored version as the service's other
//! readers report it, so the newest record is the `version` member `DescribeContact`
//! answers, and the records a later change produces (an `UpdateContact` appending a
//! version, the service applying it, every older version being superseded) show up on
//! the next read.
//!
//! The suites assert that rule over a SET of inputs rather than one call:
//!
//! * a table of histories of growing length, built by chaining `UpdateContact` calls that
//!   differ along which members they name, each listed back and cross-checked against
//!   `DescribeContact`;
//! * one history read at every page size — none, the model's minimum `1`, two
//!   intermediate sizes and the model's maximum `100` (`smithy_query(query="shape",
//!   shape="PaginationMaxResults")`, range 1..100) — including the sizes that make the
//!   listing span several pages;
//! * a table of `nextToken`s this listing did not issue — forms other listings of this
//!   service issue, and a token issued for ANOTHER contact's history — each refused;
//! * a table of well-formed `Uuid`s naming no contact, each answered an empty page, against
//!   the single-contact reads of the same ids, which are the operations that DO refuse;
//! * the listing taken twice over an unchanged contact (a read changes nothing a later
//!   caller sees) and across a pending update (the clock-owed `UPDATING` -> `ACTIVE`
//!   move, which a read materialises but does not cause).
//!
//! The suite for `UpdateContact` itself — what an accepted update appends and what a
//! refused one does not — is `tests/all/contact_versions.rs`; this one is about the
//! listing. Every request goes through the real front door — the router,
//! deserialization, constraint validation, the handler and serialization — on the
//! harness `scheduling_loop.rs` stands up.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::contact_satellite_reference_integrity::onboard_satellite_arns;
use crate::ephemeris_catalog::program_track_ephemeris;
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, list_pages, send, Reply, ABSENT_UUID,
    AFTER_SETTLE,
};

/// The window every reservation in this suite books: long enough for the profile's
/// 180-second minimum viable duration.
const CONTACT_START: i64 = 1_950_000_000;
const CONTACT_END: i64 = CONTACT_START + 600;

/// `ReserveContact` against `satellite`, answering the `contactId` of a contact at version
/// [`state::FIRST_VERSION_ID`].
async fn reserve_contact(
    router: &MultiServiceRouter,
    profile_arn: &str,
    satellite: &str,
) -> String {
    reserve_contact_in_slot(router, profile_arn, satellite, 0).await
}

/// `ReserveContact` for the pass a day per `slot` after [`CONTACT_START`], so two
/// reservations of this suite hold windows of their own: an antenna serves one pass at a
/// time, so a second reservation of the SAME window at one ground station is refused.
async fn reserve_contact_in_slot(
    router: &MultiServiceRouter,
    profile_arn: &str,
    satellite: &str,
    slot: i64,
) -> String {
    let shift = slot * 86_400;
    let reserved = send(
        router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": "Ohio-1",
            "startTime": CONTACT_START + shift,
            "endTime": CONTACT_END + shift,
            "satelliteArn": satellite,
        })),
    )
    .await;
    assert_eq!(
        reserved.status, 200,
        "ReserveContact: expected 200, got {} {}",
        reserved.status, reserved.body
    );
    reserved.string("contactId")
}

/// `UpdateContact` (`POST /contact/{contactId}/versions`), asserting it was accepted and
/// answering the `versionId` it appended.
async fn append_version(router: &MultiServiceRouter, contact_id: &str, body: Value) -> i64 {
    let updated = send(
        router,
        "POST",
        &format!("/contact/{contact_id}/versions"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "UpdateContact {body}: expected 200, got {} {}",
        updated.status, updated.body
    );
    updated.field("versionId").as_i64().unwrap_or_else(|| {
        panic!(
            "UpdateContact {body}: must answer a versionId, got {}",
            updated.body
        )
    })
}

/// The ephemeris of each form a `ProgramTrackSettings` member may name, stored on the
/// router under test.
///
/// A program track references an ephemeris BY ID and a stored reference has to resolve, so
/// the overrides this suite chains onto a contact name ephemerides this account holds.
struct ProgramTracks {
    az_el: String,
    oem: String,
    tle: String,
}

impl ProgramTracks {
    /// One ephemeris per member, each of the form that member requires, through
    /// `CreateEphemeris`.
    async fn create(router: &MultiServiceRouter) -> Self {
        Self {
            az_el: program_track_ephemeris(router, "azEl").await,
            oem: program_track_ephemeris(router, "oem").await,
            tle: program_track_ephemeris(router, "tle").await,
        }
    }
}

/// The bodies this suite chains onto one contact, differing along which members an update
/// names and which `ProgramTrackSettings` member its overrides select
/// (`smithy_query(query="shape", shape="ProgramTrackSettings")`). The satellites are
/// on-boarded ones, since a contact is flown against a satellite of this account.
fn update_bodies(tracks: &ProgramTracks, satellites: &[String]) -> Vec<Value> {
    vec![
        json!({"satelliteArn": satellites[1]}),
        json!({"trackingOverrides": {"programTrackSettings": {
            "azEl": {"ephemerisId": tracks.az_el},
        }}}),
        json!({
            "satelliteArn": satellites[2],
            "trackingOverrides": {"programTrackSettings": {
                "oem": {"ephemerisId": tracks.oem},
            }},
        }),
        json!({"trackingOverrides": {"programTrackSettings": {
            "tle": {"ephemerisId": tracks.tle},
        }}}),
        json!({"satelliteArn": satellites[3]}),
        json!({"trackingOverrides": {}}),
    ]
}

/// `ListContactVersions` for one contact, with the query string `query` (empty for none),
/// asserting it answered.
async fn list_versions(
    router: &MultiServiceRouter,
    contact_id: &str,
    query: &str,
    case: &str,
) -> Reply {
    let route = if query.is_empty() {
        format!("/contact/{contact_id}/versions")
    } else {
        format!("/contact/{contact_id}/versions?{query}")
    };
    let reply = send(router, "GET", &route, None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListContactVersions 200, got {} {}",
        reply.status, reply.body
    );
    reply
}

/// The `contactVersionsList` of an answer, as a list.
fn versions_of(reply: &Reply, case: &str) -> Vec<Value> {
    reply
        .field("contactVersionsList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: contactVersionsList must be a list, got {}",
                reply.body
            )
        })
        .clone()
}

/// The `versionId`s of a list of version records, in the order they came back.
fn version_ids(versions: &[Value], case: &str) -> Vec<i64> {
    versions
        .iter()
        .map(|version| {
            version
                .get("versionId")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| panic!("case {case}: every record carries a versionId, got {version}"))
        })
        .collect()
}

#[tokio::test]
async fn every_version_the_store_holds_is_listed_oldest_first() {
    let (router, engine) = harness_with_engine();
    // A contact is flown against a satellite of THIS account, so the satellites the rows
    // below name are on-boarded first (`crate::contact_satellite_reference_integrity`).
    let satellites = onboard_satellite_arns(&engine, 4);
    let profile_arn =
        create_mission_profile(&router, "version listing", 180, Some(60), Some(30)).await;
    let contact_id = reserve_contact(&router, &profile_arn, &satellites[0]).await;
    let tracks = ProgramTracks::create(&router).await;

    // Rows of growing history: the reservation's version alone, then one more version per
    // update chained onto the same contact. Each row reads the whole history back, so the
    // listing is asserted at every length from one up.
    let mut expected_ids: Vec<i64> = vec![i64::from(state::FIRST_VERSION_ID)];
    let mut case = "reserved only".to_string();
    for (index, body) in [None]
        .into_iter()
        .chain(update_bodies(&tracks, &satellites).into_iter().map(Some))
        .enumerate()
    {
        if let Some(body) = body {
            let appended = append_version(&router, &contact_id, body.clone()).await;
            expected_ids.push(appended);
            case = format!("after update {index} ({body})");
        }
        // Past the settle span, so the version the update appended has taken force and
        // every version before it has been superseded.
        std::thread::sleep(AFTER_SETTLE);

        let listed = list_versions(&router, &contact_id, "", &case).await;
        let versions = versions_of(&listed, &case);
        assert_eq!(
            version_ids(&versions, &case),
            expected_ids,
            "case {case}: the listing must report every version oldest first, got {}",
            listed.body
        );
        assert_eq!(
            listed.field("nextToken"),
            &Value::Null,
            "case {case}: a history that fits one page closes the listing, got {}",
            listed.field("nextToken")
        );

        let newest = versions.last().unwrap_or_else(|| {
            panic!("case {case}: a contact always has a version, got {}", listed.body)
        });
        assert_eq!(
            newest.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case}: the newest version has taken force, got {newest}"
        );
        assert_eq!(
            newest.get("superseded").unwrap_or(&Value::Null),
            &Value::Null,
            "case {case}: nothing has superseded the newest version, got {newest}"
        );
        for superseded in &versions[..versions.len() - 1] {
            assert_eq!(
                superseded.get("status").and_then(Value::as_str),
                Some("SUPERSEDED"),
                "case {case}: a version a newer one replaced is SUPERSEDED, got {superseded}"
            );
            assert!(
                superseded.get("superseded").and_then(Value::as_f64).is_some(),
                "case {case}: a superseded version carries the instant it was replaced, \
                 got {superseded}"
            );
        }
        for version in &versions {
            for member in ["failureCodes", "failureMessage"] {
                assert_eq!(
                    version.get(member).unwrap_or(&Value::Null),
                    &Value::Null,
                    "case {case}: {member} must be absent on a version nothing failed, got \
                     {version}"
                );
            }
        }

        // The newest record is the one `DescribeContact` reports as the contact's
        // version: two readers of one stored history cannot disagree.
        let described = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected DescribeContact 200, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.field("version"),
            newest,
            "case {case}: DescribeContact reports the newest version, got {} and {newest}",
            described.field("version")
        );
    }
}

#[tokio::test]
async fn the_history_pages_the_same_way_at_every_page_size() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard_satellite_arns(&engine, 4);
    let profile_arn = create_mission_profile(&router, "version paging", 180, None, None).await;
    let contact_id = reserve_contact(&router, &profile_arn, &satellites[0]).await;
    let tracks = ProgramTracks::create(&router).await;
    let bodies = update_bodies(&tracks, &satellites);
    for body in bodies.clone() {
        append_version(&router, &contact_id, body).await;
    }
    std::thread::sleep(AFTER_SETTLE);

    let path = format!("/contact/{contact_id}/versions");
    let whole: Vec<Value> = list_pages(&router, &path, "contactVersionsList", None).await;
    let expected = version_ids(&whole, "maxResults=None");
    assert_eq!(
        expected.len(),
        bodies.len() + 1,
        "case maxResults=None: the reservation's version plus one per update, got {expected:?}"
    );
    assert_eq!(
        expected,
        (1..=expected.len() as i64).collect::<Vec<i64>>(),
        "case maxResults=None: versions are numbered from the first without a gap, got \
         {expected:?}"
    );

    // Page sizes spanning the modelled range: the minimum (which makes one page per
    // version), two sizes that split the history, and the maximum (which holds it whole).
    for size in [1_i64, 2, 3, 100] {
        let paged = list_pages(&router, &path, "contactVersionsList", Some(size)).await;
        let ids = version_ids(&paged, &format!("maxResults={size}"));
        assert_eq!(
            ids, expected,
            "case maxResults={size}: walking every page must yield the whole history in \
             ascending versionId order, got {ids:?}"
        );

        // The FIRST page of a history longer than the page size holds exactly that many
        // records and hands back a token to resume with.
        let first = list_versions(
            &router,
            &contact_id,
            &format!("maxResults={size}"),
            &format!("maxResults={size} first page"),
        )
        .await;
        let page = versions_of(&first, &format!("maxResults={size} first page"));
        let fits = (size as usize) >= expected.len();
        assert_eq!(
            page.len(),
            if fits { expected.len() } else { size as usize },
            "case maxResults={size}: a page holds at most {size} records, got {}",
            first.body
        );
        assert_eq!(
            first.field("nextToken") == &Value::Null,
            fits,
            "case maxResults={size}: a listing hands back a token exactly when records \
             remain, got {}",
            first.body
        );
    }
}

#[tokio::test]
async fn a_token_this_listing_did_not_issue_for_this_contact_is_refused() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard_satellite_arns(&engine, 4);
    let profile_arn = create_mission_profile(&router, "version tokens", 180, None, None).await;
    let first_contact = reserve_contact(&router, &profile_arn, &satellites[0]).await;
    let second_contact = reserve_contact_in_slot(&router, &profile_arn, &satellites[0], 1).await;
    let tracks = ProgramTracks::create(&router).await;
    for body in update_bodies(&tracks, &satellites) {
        append_version(&router, &first_contact, body.clone()).await;
        append_version(&router, &second_contact, body).await;
    }
    std::thread::sleep(AFTER_SETTLE);

    // A real token, issued by the first contact's own listing: resuming the SECOND
    // contact's history with it is refused, because an offset only means anything against
    // the sequence it was taken from.
    let first_page = list_versions(&router, &first_contact, "maxResults=1", "token source").await;
    let foreign_token = first_page
        .field("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case token source: a history longer than one page hands back a token, got {}",
                first_page.body
            )
        })
        .to_string();

    // The other rows are token-shaped values this listing never issued: the forms other
    // listings of this service issue, and one from nowhere. Every row satisfies
    // `PaginationToken` (`^[A-Za-z0-9-/+_.=]+$`, length 3..1000), so each reaches the
    // handler rather than the request validator.
    let mut tokens = vec![
        foreign_token,
        "AAA".to_string(),
        "contacts.0".to_string(),
        "contact-versions.0".to_string(),
        "contact-versions.0000000000000000.2".to_string(),
        "configs.12".to_string(),
        "mission-profiles.0".to_string(),
    ];
    // A token the SECOND contact's listing did issue is accepted there, so the rows above
    // are refused for their scope and not because every token is.
    let own_token = list_versions(&router, &second_contact, "maxResults=2", "own token")
        .await
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .expect("a history longer than two records hands back a token");
    let resumed = list_versions(
        &router,
        &second_contact,
        &format!("maxResults=2&nextToken={own_token}"),
        "own token resumed",
    )
    .await;
    assert!(
        !versions_of(&resumed, "own token resumed").is_empty(),
        "case own token resumed: this contact's own token resumes its history, got {}",
        resumed.body
    );
    tokens.push(own_token);
    let last = tokens.len() - 1;

    for (index, token) in tokens.iter().enumerate() {
        // The last row is the second contact's own token, asserted against the FIRST
        // contact: a token is scoped to the contact whose history it was taken from.
        let addressed = if index == last {
            &first_contact
        } else {
            &second_contact
        };
        let reply = send(
            &router,
            "GET",
            &format!("/contact/{addressed}/versions?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 431,
            "case token={token}: a token this listing did not issue for this contact is an \
             invalid parameter, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case token={token}: wrong error shape, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_contact_that_does_not_exist_is_an_empty_page() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard_satellite_arns(&engine, 1);
    let profile_arn = create_mission_profile(&router, "version not found", 180, None, None).await;
    // One contact exists, so an empty page below is about the id that was asked for rather
    // than about an empty store.
    let existing = reserve_contact(&router, &profile_arn, &satellites[0]).await;

    // Well-formed `Uuid`s naming nothing: a run of zeros, a run of `f`s and a mixed one,
    // so the answer cannot be passing on one id's shape.
    for absent in [
        ABSENT_UUID,
        "ffffffff-ffff-4fff-bfff-ffffffffffff",
        "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
    ] {
        assert_ne!(
            absent, existing,
            "case {absent}: this row must name no contact"
        );
        // The listing of a scope that holds nothing: 200 with the items member present and
        // empty, and no token, so an SDK paginator yields zero items instead of raising.
        let listed = list_versions(&router, absent, "", absent).await;
        assert_eq!(
            versions_of(&listed, absent),
            Vec::<Value>::new(),
            "case {absent}: a contact with no version to report is an empty page, got {}",
            listed.body
        );
        assert_eq!(
            listed.field("nextToken"),
            &Value::Null,
            "case {absent}: an empty page closes the listing, got {}",
            listed.field("nextToken")
        );
        // Every page size answers the same way, the member never going missing.
        for size in [1_i64, 100] {
            let case = format!("{absent} maxResults={size}");
            let page = list_versions(&router, absent, &format!("maxResults={size}"), &case).await;
            assert_eq!(
                versions_of(&page, &case),
                Vec::<Value>::new(),
                "case {case}: a bounded page of an empty history is still empty, got {}",
                page.body
            );
        }

        // The reads addressed at ONE contact are the operations that refuse: a listing
        // reports a set, a read must answer a resource.
        for (what, route) in [
            ("DescribeContact", format!("/contact/{absent}")),
            (
                "DescribeContactVersion",
                format!("/contact/{absent}/versions/{}", state::FIRST_VERSION_ID),
            ),
        ] {
            let refused = send(&router, "GET", &route, None).await;
            assert_eq!(
                refused.status, 434,
                "case {absent}/{what}: a single-contact read must refuse a contact that does \
                 not exist, got {} {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ResourceNotFoundException",
                "case {absent}/{what}: wrong error shape, got {}",
                refused.body
            );
        }
    }

    // The id that DOES name a contact answers its history instead, so the empty page is
    // about the scope holding nothing and not about the listing reporting nothing.
    let listed = list_versions(&router, &existing, "", "existing").await;
    assert_eq!(
        version_ids(&versions_of(&listed, "existing"), "existing"),
        vec![i64::from(state::FIRST_VERSION_ID)],
        "case existing: the reserved contact's history is its first version, got {}",
        listed.body
    );
}

#[tokio::test]
async fn listing_versions_changes_nothing_a_later_caller_sees() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard_satellite_arns(&engine, 2);
    let profile_arn = create_mission_profile(&router, "version reads", 180, None, None).await;
    let contact_id = reserve_contact(&router, &profile_arn, &satellites[0]).await;
    std::thread::sleep(AFTER_SETTLE);

    let first = versions_of(
        &list_versions(&router, &contact_id, "", "first read").await,
        "first read",
    );
    let second = versions_of(
        &list_versions(&router, &contact_id, "", "second read").await,
        "second read",
    );
    assert_eq!(
        first, second,
        "case repeat read: a read must change nothing a later caller sees, got {first:?} then \
         {second:?}"
    );

    // An update appends a version the service owes a move to ACTIVE: the listing taken
    // straight after sees the new record in one of the two statuses that move spans, and
    // the listing taken past the span sees it in force with the version before it
    // superseded. The reads materialise that move; they do not cause it.
    append_version(
        &router,
        &contact_id,
        json!({"satelliteArn": satellites[1]}),
    )
    .await;
    let pending = versions_of(
        &list_versions(&router, &contact_id, "", "pending update").await,
        "pending update",
    );
    assert_eq!(
        version_ids(&pending, "pending update"),
        vec![1, 2],
        "case pending update: the appended version is in the history at once, got {pending:?}"
    );
    let pending_status = pending[1]
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        pending_status == "UPDATING" || pending_status == "ACTIVE",
        "case pending update: a version is applied or still being applied, got \
         {pending_status:?}"
    );

    std::thread::sleep(AFTER_SETTLE);

    let settled = versions_of(
        &list_versions(&router, &contact_id, "", "settled update").await,
        "settled update",
    );
    assert_eq!(
        version_ids(&settled, "settled update"),
        vec![1, 2],
        "case settled update: applying a version appends nothing further, got {settled:?}"
    );
    assert_eq!(
        settled[1].get("status").and_then(Value::as_str),
        Some("ACTIVE"),
        "case settled update: the appended version has taken force, got {}",
        settled[1]
    );
    assert_eq!(
        settled[0].get("status").and_then(Value::as_str),
        Some("SUPERSEDED"),
        "case settled update: the version before it has been superseded, got {}",
        settled[0]
    );
    assert_eq!(
        settled[0].get("created"),
        first[0].get("created"),
        "case settled update: a version's created instant must not move, got {} and {}",
        settled[0],
        first[0]
    );
}
