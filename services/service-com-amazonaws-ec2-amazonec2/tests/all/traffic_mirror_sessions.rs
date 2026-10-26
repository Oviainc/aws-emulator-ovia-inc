// SPDX-License-Identifier: CC0-1.0

//! What `DeleteTrafficMirrorSession` deletes and what it refuses — end to end over the wire
//! the service actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST
//! whose body carries `Action=DeleteTrafficMirrorSession`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"), through the crate's shared [`super::ec2_query`] harness.
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
    TRAFFIC_MIRROR_SESSION_ID_PREFIX, traffic_mirror_session_payload_bytes,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};
use super::wire::is_long_id;

/// The Region every call here is signed for. A Traffic Mirror session is named by an id
/// that is unique across this store, and the modelled shape names no Region, so the Region
/// only selects the envelope.
const REGION: &str = "us-west-2";

/// The error code EC2 answers for a Traffic Mirror session id that names no session of the
/// account, as this crate's state module defines it.
const NOT_FOUND: &str = ec2::state::INVALID_TRAFFIC_MIRROR_SESSION_ID_NOT_FOUND;

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
    let unknown = [
        "tms-0123456789abcdef0",
        "tms-00000000000000000",
        "tms-00000000",
        "tms-not-hex",
        "tms-",
        "tmf-0123456789abcdef0",
        "vpc-0123456789abcdef0",
        "not-an-id-at-all",
    ];

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
