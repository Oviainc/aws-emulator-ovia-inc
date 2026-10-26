// SPDX-License-Identifier: CC0-1.0

//! What the Traffic Mirror session operations report, delete and refuse — end to end over
//! the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"), through the crate's
//! shared [`super::ec2_query`] harness.
//!
//! The rules under test:
//!
//! 1. "Deletes the specified Traffic Mirror session" (`DeleteTrafficMirrorSession`): the
//!    session the request names is REMOVED from the store, whatever that session carries,
//!    and the call answers with the id of the session it deleted — so the delete is
//!    observable through the operation itself, a repeat of it naming nothing. Every other
//!    session the store holds is left exactly as it was.
//! 2. An id this account holds no Traffic Mirror session for is refused with HTTP 400
//!    `InvalidTrafficMirrorSessionId.NotFound` whatever the id looks like — the member
//!    publishes no pattern, so there is no second code for an id of the wrong shape — and
//!    the message names the id the request gave. A refusal deletes nothing.
//! 3. `DryRun` asks whether the call WOULD have succeeded: over a session the store holds it
//!    is answered `DryRunOperation` (HTTP 412) with nothing deleted, and over an id that
//!    names nothing it is answered with the refusal that id would have got instead.
//! 4. The session store is bounded by its own ceiling, and the bytes one session's
//!    caller-chosen text holds go back to the store's byte budget when the delete removes
//!    it — the charge the insert took, given back on the same terms.
//! 5. "Describes one or more Traffic Mirror sessions. By default, all Traffic Mirror
//!    sessions are described" (`DescribeTrafficMirrorSessions`): the report is the STORE —
//!    every member a stored session carries reaches the response, a member it does not
//!    carry is absent, and an account holding no session is reported as holding none
//!    rather than refused.
//! 6. The report NARROWS: by `TrafficMirrorSessionIds`, and by every filter name the
//!    operation answers — values ORed within one entry, entries ANDed with each other,
//!    each value matched with EC2's wildcards — while a name it does not document selects
//!    nothing, which is what the real service answers for one.
//! 7. The report PAGES: a page holds at most `MaxResults` sessions, `NextToken` resumes
//!    where the page ended, walking the pages visits every session exactly once, and a
//!    token this service never issued is refused.
//! 8. The describe is a READ: `DryRun` over it answers `DryRunOperation` (HTTP 412), an id
//!    naming no session of the account is refused with the same code the delete answers,
//!    and neither leaves the store changed — nor does a report that succeeded.
//!
//! `CreateTrafficMirrorSession` is still a scaffold stub with an issue of its own, so the
//! sessions here are seeded through the store's own write path
//! (`insert_traffic_mirror_session`), which is where that create will write them.
//!
//! Each table row is a distinct input along what the rule generalises over — which members
//! a session carries, how many sessions the store holds, which of them is deleted, and the
//! shape of an unknown id — so no row is the specification. The recorded unit this suite was
//! filed from is ONE row of the table in
//! [`an_unknown_session_id_is_refused_whatever_it_looks_like`].

use ec2::ops::types::{Tag, TrafficMirrorSession};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_TRAFFIC_MIRROR_SESSIONS,
    MAX_TRAFFIC_MIRROR_SESSIONS_PAGE_SIZE, TRAFFIC_MIRROR_SESSION_ID_PREFIX,
    traffic_mirror_session_payload_bytes,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, filters, indexed, member};
use super::wire::{child_text, document, is_long_id, set_items};

/// The Region every call here is signed for. A Traffic Mirror session is named by an id
/// that is unique across this store, and the modelled shape names no Region, so the Region
/// only selects the envelope.
const REGION: &str = "us-west-2";

/// The error code EC2 answers for a Traffic Mirror session id that names no session of the
/// account, as this crate's state module defines it.
const NOT_FOUND: &str = ec2::state::INVALID_TRAFFIC_MIRROR_SESSION_ID_NOT_FOUND;

/// The shapes of an id that names no Traffic Mirror session of the account: the long 17-hex
/// form the store mints, the all-zero suffix of that form (the shape the recorded unit this
/// suite was filed from used), the short 8-hex form of EC2's older ids, a non-hex suffix, a
/// suffix-less prefix, the id of another resource kind and text that is no id at all.
///
/// The ONE table of them, so the delete and the describe are held to the same rule: an id
/// this service never minted names no session whatever it looks like.
const UNKNOWN_SESSION_IDS: [&str; 8] = [
    "tms-0123456789abcdef0",
    "tms-00000000000000000",
    "tms-00000000",
    "tms-not-hex",
    "tms-",
    "tmf-0123456789abcdef0",
    "vpc-0123456789abcdef0",
    "not-an-id-at-all",
];

/// One session to seed: the members a create would have set on it. `None` everywhere is a
/// session carrying only what the service derives.
#[derive(Clone, Debug, Default)]
struct SessionRow {
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    packet_length: Option<i32>,
    session_number: Option<i32>,
    virtual_network_id: Option<i32>,
}

/// The modelled record one [`SessionRow`] stands for, as the create will build it: the
/// filter, the target and the network interface a session mirrors between are ids of other
/// resources, built from `n` so each seeded session names its own.
fn session(row: &SessionRow, n: usize) -> TrafficMirrorSession {
    TrafficMirrorSession {
        // Minted by the store at the insert, so an id is never chosen twice.
        traffic_mirror_session_id: None,
        traffic_mirror_target_id: Some(format!("tmt-{n:017x}")),
        traffic_mirror_filter_id: Some(format!("tmf-{n:017x}")),
        network_interface_id: Some(format!("eni-{n:017x}")),
        owner_id: Some(::engine::emulator_account_id().to_string()),
        packet_length: row.packet_length,
        session_number: row.session_number,
        virtual_network_id: row.virtual_network_id,
        description: row.description.map(str::to_string),
        tags: Some(
            row.tags
                .iter()
                .map(|(key, value)| Tag {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                })
                .collect(),
        ),
    }
}

/// Seed one session in the store and answer the id it was minted with.
fn seed(engine: &engine::InMemoryEngine, case: &str, row: &SessionRow, n: usize) -> String {
    let id = engine
        .state::<Ec2State>()
        .lock()
        .insert_traffic_mirror_session(session(row, n));
    let id = id.unwrap_or_else(|| {
        panic!("case {case:?}: the store must have room for the few sessions this suite seeds")
    });
    assert!(
        is_long_id(TRAFFIC_MIRROR_SESSION_ID_PREFIX, &id),
        "case {case:?}: expected a minted Traffic Mirror session id, got {id:?}"
    );
    id
}

/// One `DeleteTrafficMirrorSession` call naming `id`, with `extra` form fields beside it.
async fn delete(
    router: &MultiServiceRouter,
    id: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![("TrafficMirrorSessionId".to_string(), id.to_string())];
    for (key, value) in extra {
        params.push(((*key).to_string(), (*value).to_string()));
    }
    call(router, REGION, "DeleteTrafficMirrorSession", &params).await
}

/// Every Traffic Mirror session id the store holds, in the store's own order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .traffic_mirror_sessions
        .keys()
        .cloned()
        .collect()
}

/// The rows the delete table and the dry-run table are both driven over: a session carrying
/// nothing beyond what the service derives, and sessions carrying each of the members a
/// create can set, at the published bounds of the two bounded ones.
///
/// `PacketLength` is "the number of bytes in each packet to mirror", and `VirtualNetworkId`
/// "the virtual network ID associated with the Traffic Mirror session" — a VXLAN network
/// identifier, which is 24 bits wide, so `16777215` is its largest value
/// (`smithy_query(query="shape",shape="TrafficMirrorSession")`).
const SESSION_ROWS: &[(&str, SessionRow)] = &[
    (
        "a session carrying only the ids it mirrors between",
        SessionRow {
            description: None,
            tags: &[],
            packet_length: None,
            session_number: None,
            virtual_network_id: None,
        },
    ),
    (
        "a session with a description and one tag",
        SessionRow {
            description: Some("web tier mirror"),
            tags: &[("Team", "platform")],
            packet_length: Some(100),
            session_number: Some(1),
            virtual_network_id: None,
        },
    ),
    (
        "a session with a description holding punctuation and non-ASCII text",
        SessionRow {
            description: Some("mirror: ingress/egress — grün"),
            tags: &[("Team", "network"), ("Env", "staging")],
            packet_length: Some(1),
            session_number: Some(32766),
            virtual_network_id: Some(1),
        },
    ),
    (
        "a session at the widest virtual network id a VXLAN header carries",
        SessionRow {
            description: Some("d"),
            tags: &[("Only", "")],
            packet_length: Some(8947),
            session_number: Some(32766),
            virtual_network_id: Some(16_777_215),
        },
    ),
];

/// RULE 1: whatever a stored session carries, the delete removes it from the store, answers
/// with its id, and leaves every other session alone — and a repeat of the same call reports
/// the id as naming nothing, so the deletion is observable through this operation alone.
///
/// The rows differ along what the rule generalises over: which members the session carries,
/// how many sessions the store holds beside it, and which of them the request names.
#[tokio::test]
async fn every_stored_session_is_deleted_and_reported_by_id() {
    for (case, row) in SESSION_ROWS {
        // Two neighbours are seeded around the session under test, and the row's own
        // position among them moves with the table, so "the first", "a middle one" and "the
        // last" session of the store are each deleted by some row.
        for position in 0..3 {
            let case = format!("{case}, deleted at position {position} of three");
            let (router, engine) = super::ec2_query::emulator();
            let seeded: Vec<String> = (0..3)
                .map(|n| {
                    let seeded_row = if n == position { row.clone() } else { SessionRow::default() };
                    seed(&engine, &case, &seeded_row, n)
                })
                .collect();
            let target = seeded[position].clone();

            let (status, body) = delete(&router, &target, &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                element(&body, "trafficMirrorSessionId").as_deref(),
                Some(target.as_str()),
                "case {case:?}: expected the id of the deleted session, got {body}"
            );

            let left: Vec<String> = seeded
                .iter()
                .filter(|id| **id != target)
                .cloned()
                .collect();
            assert_eq!(
                stored_ids(&engine),
                left,
                "case {case:?}: the delete must remove exactly the session it named, the \
                 store holds {:?}",
                stored_ids(&engine)
            );

            // The repeat: the id now names nothing, which is how a caller tells the
            // deletion it performed from one it did not.
            let (status, body) = delete(&router, &target, &[]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from the repeat, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                NOT_FOUND,
                "case {case:?}: expected {NOT_FOUND} from the repeat, got {body}"
            );
            assert_eq!(
                stored_ids(&engine),
                left,
                "case {case:?}: a refused repeat must delete nothing, the store holds {:?}",
                stored_ids(&engine)
            );
        }
    }
}

/// RULE 2: an id this account holds no session for is refused with
/// `InvalidTrafficMirrorSessionId.NotFound`, whatever shape the id has, and the message
/// names the id the request gave.
///
/// The rows differ along the SHAPE of the unknown id: the long 17-hex form the store mints,
/// the all-zero suffix of that form (the shape the recorded unit this suite was filed from
/// used), the short 8-hex form of EC2's older ids, a non-hex suffix, a suffix-less prefix
/// and the id of another resource kind. Each is tried against an EMPTY store and against one
/// holding a session, because "no such session" must not depend on the account holding none.
#[tokio::test]
async fn an_unknown_session_id_is_refused_whatever_it_looks_like() {
    let unknown = UNKNOWN_SESSION_IDS;

    for id in unknown {
        for seeded_sessions in [0usize, 1] {
            let case = format!("{id} with {seeded_sessions} session(s) stored");
            let (router, engine) = super::ec2_query::emulator();
            for n in 0..seeded_sessions {
                seed(&engine, &case, &SessionRow::default(), n);
            }
            let before = stored_ids(&engine);

            let (status, body) = delete(&router, id, &[]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for an id naming no session, got {status} \
                 with body {body}"
            );
            assert_eq!(
                error_code(&body),
                NOT_FOUND,
                "case {case:?}: expected {NOT_FOUND}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("{id} does not exist"),
                "case {case:?}: expected the message to name the id the request gave, got {body}"
            );
            assert_eq!(
                stored_ids(&engine),
                before,
                "case {case:?}: a refused delete must delete nothing, the store holds {:?}",
                stored_ids(&engine)
            );
        }
    }
}

/// RULE 3: `DryRun` asks whether the call would have succeeded — so a dry run over a stored
/// session is answered `DryRunOperation` with nothing deleted, while a dry run over an id
/// that names nothing is answered with the refusal that id would have got. A `DryRun` the
/// request sets to `false` is not a probe at all, so it deletes.
#[tokio::test]
async fn a_dry_run_deletes_nothing() {
    for (case, row) in SESSION_ROWS {
        let (router, engine) = super::ec2_query::emulator();
        let id = seed(&engine, case, row, 0);
        let before = stored_ids(&engine);

        let (status, body) = delete(&router, &id, &[("DryRun", "true")]).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 for a dry run, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            before,
            "case {case:?}: a dry run must delete nothing, the store holds {:?}",
            stored_ids(&engine)
        );

        // A dry run that would have been REFUSED is refused with that reason instead.
        let (status, body) = delete(&router, "tms-0123456789abcdef0", &[("DryRun", "true")]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a dry run over an unknown id is refused with that id's reason, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND} for a dry run over an unknown id, got {body}"
        );

        // `DryRun=false` is a real request.
        let (status, body) = delete(&router, &id, &[("DryRun", "false")]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 with DryRun=false, got {status} with body {body}"
        );
        assert!(
            stored_ids(&engine).is_empty(),
            "case {case:?}: the session must be gone, the store holds {:?}",
            stored_ids(&engine)
        );
    }
}

/// RULE 4: the bytes one session's caller-chosen text holds are charged to the store's byte
/// budget at the insert and given back when the delete removes the session, and the store is
/// bounded by its own ceiling — asserted AT the ceiling, one past it, and after a delete has
/// freed a slot.
#[test]
fn the_session_store_gives_its_byte_charge_back_and_is_bounded() {
    let (_router, engine) = super::ec2_query::emulator();
    let store = engine.state::<Ec2State>();

    // The byte charge round-trips, for a session of each shape in the table.
    for (case, row) in SESSION_ROWS {
        let mut store = store.lock();
        let before = store.stored_bytes();
        let record = session(row, 0);
        let charge = traffic_mirror_session_payload_bytes(&record);
        let id = store
            .insert_traffic_mirror_session(record)
            .unwrap_or_else(|| panic!("case {case:?}: the store must have room for one session"));
        assert_eq!(
            store.stored_bytes(),
            before + charge,
            "case {case:?}: the insert must charge exactly {charge} bytes, the store holds {}",
            store.stored_bytes()
        );
        let removed = store.remove_traffic_mirror_session(&id);
        assert!(
            removed.is_some(),
            "case {case:?}: the store must give back the session it stored"
        );
        assert_eq!(
            store.stored_bytes(),
            before,
            "case {case:?}: the delete must give the charge back, the store holds {}",
            store.stored_bytes()
        );
    }

    let mut store = store.lock();
    for n in 0..MAX_TRAFFIC_MIRROR_SESSIONS {
        let record = session(&SessionRow::default(), n);
        assert!(
            store.has_traffic_mirror_session_capacity(&record),
            "case \"session {n} of {MAX_TRAFFIC_MIRROR_SESSIONS}\": the store must still have \
             room, it holds {}",
            store.traffic_mirror_sessions.len()
        );
        assert!(
            store.insert_traffic_mirror_session(record).is_some(),
            "case \"session {n} of {MAX_TRAFFIC_MIRROR_SESSIONS}\": the store must accept a \
             session below its ceiling, it holds {}",
            store.traffic_mirror_sessions.len()
        );
    }

    let past_the_ceiling = session(&SessionRow::default(), MAX_TRAFFIC_MIRROR_SESSIONS);
    assert!(
        !store.has_traffic_mirror_session_capacity(&past_the_ceiling),
        "case \"at the ceiling\": the store must report no room, it holds {} of \
         {MAX_TRAFFIC_MIRROR_SESSIONS}",
        store.traffic_mirror_sessions.len()
    );
    assert!(
        store
            .insert_traffic_mirror_session(past_the_ceiling.clone())
            .is_none(),
        "case \"one past the ceiling\": the store must refuse the session, it holds {}",
        store.traffic_mirror_sessions.len()
    );

    // A delete frees a slot: the ceiling counts what the store HOLDS, not what it ever held.
    let first = store
        .traffic_mirror_sessions
        .keys()
        .next()
        .cloned()
        .expect("the store holds the sessions seeded above");
    assert!(
        store.remove_traffic_mirror_session(&first).is_some(),
        "case \"a delete at the ceiling\": the store must remove the session named"
    );
    assert!(
        store.insert_traffic_mirror_session(past_the_ceiling).is_some(),
        "case \"after a delete at the ceiling\": the freed slot must take a session, the store \
         holds {}",
        store.traffic_mirror_sessions.len()
    );
}

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `DescribeTrafficMirrorSessions` call carrying `params` as its form fields.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeTrafficMirrorSessions", params).await
}

/// One Traffic Mirror session as a response REPORTED it: every member read off the XML
/// tree, so a member the response omits is `None` and one it reports empty is `Some("")` —
/// the two cases an optional member has to be told apart by.
#[derive(Debug)]
struct Reported {
    traffic_mirror_session_id: Option<String>,
    traffic_mirror_target_id: Option<String>,
    traffic_mirror_filter_id: Option<String>,
    network_interface_id: Option<String>,
    owner_id: Option<String>,
    packet_length: Option<String>,
    session_number: Option<String>,
    virtual_network_id: Option<String>,
    description: Option<String>,
    tags: Vec<(String, String)>,
}

/// The sessions one `DescribeTrafficMirrorSessions` response reports, in the order it
/// reported them.
///
/// Read through the XML parser rather than by substring, so a session's own members stay
/// inside its `<item>` and an empty element is not mistaken for an absent one.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "trafficMirrorSessionSet")
        .into_iter()
        .map(|item| Reported {
            traffic_mirror_session_id: child_text(item, "trafficMirrorSessionId"),
            traffic_mirror_target_id: child_text(item, "trafficMirrorTargetId"),
            traffic_mirror_filter_id: child_text(item, "trafficMirrorFilterId"),
            network_interface_id: child_text(item, "networkInterfaceId"),
            owner_id: child_text(item, "ownerId"),
            packet_length: child_text(item, "packetLength"),
            session_number: child_text(item, "sessionNumber"),
            virtual_network_id: child_text(item, "virtualNetworkId"),
            description: child_text(item, "description"),
            tags: set_items(item, "tagSet")
                .into_iter()
                .map(|tag| {
                    (
                        child_text(tag, "key").unwrap_or_default(),
                        child_text(tag, "value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect()
}

/// The session ids `DescribeTrafficMirrorSessions` reports for `params`, in the order it
/// reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeTrafficMirrorSessions, got {status} \
         with body {body}"
    );
    reported(case, &body)
        .into_iter()
        .map(|session| session.traffic_mirror_session_id.unwrap_or_default())
        .collect()
}

/// RULE 5: the describe reports the STORE — every member a stored session carries reaches
/// the response and a member it does not carry is absent — and an account holding no
/// session is reported as holding none rather than refused.
///
/// Driven over the same rows the delete is: the rule generalises over which members a
/// session carries, so each row is a session of a different shape, and each is reported
/// beside a neighbour the request did not name.
#[tokio::test]
async fn every_stored_session_is_reported_by_the_describe() {
    // An account with no session at all: an empty report, not a refusal.
    let (router, _engine) = super::ec2_query::emulator();
    assert!(
        described_ids(&router, "an account with no sessions", &[])
            .await
            .is_empty(),
        "case \"an account with no sessions\": expected an empty report"
    );

    for (case, row) in SESSION_ROWS {
        let (router, engine) = super::ec2_query::emulator();
        let id = seed(&engine, case, row, 0);
        // A neighbour the request does not name, so "reports the session asked for" is not
        // satisfied by a store holding only one session.
        seed(&engine, case, &SessionRow::default(), 1);

        let (status, body) = describe(&router, &[p("TrafficMirrorSessionId.1", id.clone())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let sessions = reported(case, &body);
        assert_eq!(
            sessions.len(),
            1,
            "case {case:?}: expected exactly the session named, got {sessions:?}"
        );
        let got = &sessions[0];
        let expected = session(row, 0);
        for (name, actual, want) in [
            (
                "trafficMirrorSessionId",
                &got.traffic_mirror_session_id,
                Some(id.clone()),
            ),
            (
                "trafficMirrorTargetId",
                &got.traffic_mirror_target_id,
                expected.traffic_mirror_target_id.clone(),
            ),
            (
                "trafficMirrorFilterId",
                &got.traffic_mirror_filter_id,
                expected.traffic_mirror_filter_id.clone(),
            ),
            (
                "networkInterfaceId",
                &got.network_interface_id,
                expected.network_interface_id.clone(),
            ),
            ("ownerId", &got.owner_id, expected.owner_id.clone()),
            (
                "packetLength",
                &got.packet_length,
                expected.packet_length.map(|value| value.to_string()),
            ),
            (
                "sessionNumber",
                &got.session_number,
                expected.session_number.map(|value| value.to_string()),
            ),
            (
                "virtualNetworkId",
                &got.virtual_network_id,
                expected.virtual_network_id.map(|value| value.to_string()),
            ),
            (
                "description",
                &got.description,
                expected.description.clone(),
            ),
        ] {
            assert_eq!(
                *actual, want,
                "case {case:?}: expected <{name}> to report {want:?}, got {actual:?}"
            );
        }
        let want_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            got.tags, want_tags,
            "case {case:?}: expected the tags the session carries, got {:?}",
            got.tags
        );
    }
}

/// RULE 6: the report narrows by `TrafficMirrorSessionIds` and by every filter name the
/// operation answers — values ORed within one entry, entries ANDed with each other, each
/// value matched with EC2's wildcards — while a name it does not document selects nothing
/// rather than being refused, which is what the real service answers for one.
///
/// The rows differ along which filter name selects, how many values one entry carries, how
/// many entries are ANDed, and whether the selection is by id, by filter or by both — so no
/// row is the specification. The filter names come from
/// `DescribeTrafficMirrorSessionsRequest$Filters`, plus the `tag-key` / `tag:<key>` pair EC2
/// documents for every taggable resource; the three integer members are compared as the
/// decimal text the response reports them as.
#[tokio::test]
async fn the_describe_narrows_by_ids_and_by_every_filter_it_answers() {
    // Three sessions differing in every member a filter selects on, so each name below
    // selects a different subset.
    let rows: [SessionRow; 3] = [
        SessionRow {
            description: Some("web tier mirror"),
            tags: &[("Team", "platform")],
            packet_length: Some(100),
            session_number: Some(1),
            virtual_network_id: Some(7),
        },
        SessionRow {
            description: Some("data tier mirror"),
            tags: &[("Team", "data")],
            packet_length: Some(8947),
            session_number: Some(2),
            virtual_network_id: Some(16_777_215),
        },
        SessionRow {
            description: None,
            tags: &[],
            packet_length: None,
            session_number: Some(3),
            virtual_network_id: None,
        },
    ];

    let (router, engine) = super::ec2_query::emulator();
    let seeded: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(n, row)| seed(&engine, "the narrowing table", row, n))
        .collect();
    let mut all = seeded.clone();
    all.sort();
    let account = ::engine::emulator_account_id().to_string();

    // `(case, the describe parameters, the ids the report must carry, in order)`.
    type NarrowRow = (String, Vec<(String, String)>, Vec<String>);
    let table: Vec<NarrowRow> = vec![
        (
            "no narrowing at all".to_string(),
            Vec::new(),
            all.clone(),
        ),
        (
            "one id".to_string(),
            indexed("TrafficMirrorSessionId", &[seeded[2].as_str()]),
            vec![seeded[2].clone()],
        ),
        (
            "two ids".to_string(),
            indexed(
                "TrafficMirrorSessionId",
                &[seeded[1].as_str(), seeded[0].as_str()],
            ),
            {
                let mut want = vec![seeded[0].clone(), seeded[1].clone()];
                want.sort();
                want
            },
        ),
        (
            "description, one value".to_string(),
            filters(&[("description", &["web tier mirror"])]),
            vec![seeded[0].clone()],
        ),
        (
            "description, two values ORed".to_string(),
            filters(&[("description", &["data tier mirror", "web tier mirror"])]),
            {
                let mut want = vec![seeded[0].clone(), seeded[1].clone()];
                want.sort();
                want
            },
        ),
        (
            "description with a wildcard".to_string(),
            filters(&[("description", &["*tier mirror"])]),
            {
                let mut want = vec![seeded[0].clone(), seeded[1].clone()];
                want.sort();
                want
            },
        ),
        (
            "network-interface-id".to_string(),
            filters(&[("network-interface-id", &["eni-00000000000000001"])]),
            vec![seeded[1].clone()],
        ),
        (
            "owner-id, the account that owns them".to_string(),
            filters(&[("owner-id", &[account.as_str()])]),
            all.clone(),
        ),
        (
            "owner-id, another account".to_string(),
            filters(&[("owner-id", &["111122223333"])]),
            Vec::new(),
        ),
        (
            "packet-length".to_string(),
            filters(&[("packet-length", &["8947"])]),
            vec![seeded[1].clone()],
        ),
        (
            "packet-length, a session that carries none".to_string(),
            filters(&[("packet-length", &["0"])]),
            Vec::new(),
        ),
        (
            "session-number".to_string(),
            filters(&[("session-number", &["3"])]),
            vec![seeded[2].clone()],
        ),
        (
            "tag-key".to_string(),
            filters(&[("tag-key", &["Team"])]),
            {
                let mut want = vec![seeded[0].clone(), seeded[1].clone()];
                want.sort();
                want
            },
        ),
        (
            "tag:<key>".to_string(),
            filters(&[("tag:Team", &["platform"])]),
            vec![seeded[0].clone()],
        ),
        (
            "tag:<key>, two values ORed".to_string(),
            filters(&[("tag:Team", &["data", "platform"])]),
            {
                let mut want = vec![seeded[0].clone(), seeded[1].clone()];
                want.sort();
                want
            },
        ),
        (
            "traffic-mirror-filter-id".to_string(),
            filters(&[("traffic-mirror-filter-id", &["tmf-00000000000000002"])]),
            vec![seeded[2].clone()],
        ),
        (
            "traffic-mirror-session-id".to_string(),
            filters(&[("traffic-mirror-session-id", &[seeded[1].as_str()])]),
            vec![seeded[1].clone()],
        ),
        (
            "traffic-mirror-target-id".to_string(),
            filters(&[("traffic-mirror-target-id", &["tmt-00000000000000000"])]),
            vec![seeded[0].clone()],
        ),
        (
            "virtual-network-id".to_string(),
            filters(&[("virtual-network-id", &["16777215"])]),
            vec![seeded[1].clone()],
        ),
        (
            "two entries ANDed, both matching one session".to_string(),
            filters(&[("tag-key", &["Team"]), ("packet-length", &["100"])]),
            vec![seeded[0].clone()],
        ),
        (
            "two entries ANDed that no session satisfies together".to_string(),
            filters(&[
                ("description", &["web tier mirror"]),
                ("session-number", &["2"]),
            ]),
            Vec::new(),
        ),
        (
            "an id and a filter together".to_string(),
            [
                indexed(
                    "TrafficMirrorSessionId",
                    &[seeded[0].as_str(), seeded[1].as_str()],
                ),
                filters(&[("description", &["web tier mirror"])]),
            ]
            .concat(),
            vec![seeded[0].clone()],
        ),
    ];

    for (case, params, want) in &table {
        assert_eq!(
            described_ids(&router, case, params).await,
            *want,
            "case {case:?}: expected exactly those sessions"
        );
    }

    // A filter name this operation does not document is not refused: the recorded behaviour
    // of the real service answers a RESULT for one, and a name that selects on nothing
    // selects nothing. The rows are a name of another resource's filter, a misspelling, the
    // case-shifted spelling of a name it does answer ("Filter names are case-sensitive",
    // `Filter$Name`), a bare `tag:` naming no key, an empty name, and a truncation of a
    // documented name.
    for name in [
        "vpc-id",
        "descriptions",
        "Description",
        "tag:",
        "",
        "traffic-mirror-session",
    ] {
        let case = format!("the undocumented filter {name:?}");
        assert!(
            described_ids(&router, &case, &filters(&[(name, &["anything"])]))
                .await
                .is_empty(),
            "case {case:?}: a filter this operation does not document selects nothing"
        );
    }

    // ... while the documented names still select, so the empty answers above are the
    // narrowing and not a broken report.
    assert_eq!(
        described_ids(&router, "the documented filters still select", &[]).await,
        all,
        "case \"the documented filters still select\": the unnarrowed report must still \
         carry every session"
    );
}

/// RULE 7: the report pages — a page holds at most `MaxResults` sessions, `NextToken`
/// resumes where the page ended, walking the pages visits every session exactly once, and a
/// token this service never issued is refused.
#[tokio::test]
async fn the_describe_pages_over_every_session_exactly_once() {
    let (router, engine) = super::ec2_query::emulator();
    let mut seeded: Vec<String> = (0..7)
        .map(|n| seed(&engine, "the paging table", &SessionRow::default(), n))
        .collect();
    seeded.sort();

    // The modelled `MaxResults` range is 5..1000, so these are the smallest page the
    // operation can be asked for, one above it, the whole store exactly, and a page larger
    // than the store.
    for page_size in [5usize, 6, 7, 1000] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = &next_token {
                params.push(p("NextToken", token.clone()));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page: Vec<String> = reported(&case, &body)
                .into_iter()
                .map(|session| session.traffic_mirror_session_id.unwrap_or_default())
                .collect();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults sessions, got {}",
                page.len()
            );
            walked.extend(page);
            next_token = member(&body, "nextToken");
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, seeded,
            "case {case:?}: paging must walk every session exactly once, got {walked:?}"
        );
    }

    // No page size named: one page, up to the largest the operation can be asked for.
    assert!(
        seeded.len() <= MAX_TRAFFIC_MIRROR_SESSIONS_PAGE_SIZE,
        "case \"no MaxResults\": this suite seeds fewer sessions than one page holds, it \
         seeded {}",
        seeded.len()
    );
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"no MaxResults\": expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        member(&body, "nextToken"),
        None,
        "case \"no MaxResults\": a page that holds everything carries no token, got {body}"
    );

    // A token this service did not issue is refused rather than read as a cursor.
    for token in ["not-a-token", "00", "", "/////"] {
        let (status, body) = describe(&router, &[p("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400 for a foreign NextToken, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// RULE 8: the describe is a READ — `DryRun` over it answers `DryRunOperation`, an id naming
/// no session of the account is refused with the code the delete answers for one, and
/// nothing the describe does changes what a later caller sees.
#[tokio::test]
async fn the_describe_refuses_unknown_ids_and_changes_nothing() {
    for seeded_sessions in [0usize, 1, 3] {
        let (router, engine) = super::ec2_query::emulator();
        for n in 0..seeded_sessions {
            seed(&engine, "the read table", &SessionRow::default(), n);
        }
        let before = stored_ids(&engine);

        // An id naming nothing is an error, not an absent row — whatever the id looks like,
        // and whether or not the account holds sessions of its own.
        for id in UNKNOWN_SESSION_IDS {
            let case = format!("{id} with {seeded_sessions} session(s) stored");
            let (status, body) = describe(&router, &indexed("TrafficMirrorSessionId", &[id])).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for an id naming no session, got {status} \
                 with body {body}"
            );
            assert_eq!(
                error_code(&body),
                NOT_FOUND,
                "case {case:?}: expected {NOT_FOUND}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("{id} does not exist"),
                "case {case:?}: expected the message to name the id the request gave, got {body}"
            );
        }

        // An id that names nothing is refused even beside one that resolves.
        if let Some(known) = before.first() {
            let case = format!("a known id beside an unknown one, {seeded_sessions} stored");
            let (status, body) = describe(
                &router,
                &indexed(
                    "TrafficMirrorSessionId",
                    &[known.as_str(), "tms-0123456789abcdef0"],
                ),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                NOT_FOUND,
                "case {case:?}: expected {NOT_FOUND}, got {body}"
            );
        }

        // `DryRun` asks whether the call WOULD have succeeded.
        let case = format!("DryRun with {seeded_sessions} session(s) stored");
        let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 for a dry run, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );

        // A dry run that would have been REFUSED is refused with that reason instead.
        let (status, body) = describe(
            &router,
            &[
                indexed("TrafficMirrorSessionId", &["tms-0123456789abcdef0"]),
                vec![p("DryRun", "true")],
            ]
            .concat(),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a dry run over an unknown id is refused with that id's reason, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND} for a dry run over an unknown id, got {body}"
        );

        // Nothing the describe did changed the store, and a second report is the first.
        let first = described_ids(&router, &case, &[]).await;
        assert_eq!(
            stored_ids(&engine),
            before,
            "case {case:?}: a describe must change nothing, the store holds {:?}",
            stored_ids(&engine)
        );
        assert_eq!(
            described_ids(&router, &case, &[]).await,
            first,
            "case {case:?}: a repeat of the report must answer the same sessions"
        );
    }
}
