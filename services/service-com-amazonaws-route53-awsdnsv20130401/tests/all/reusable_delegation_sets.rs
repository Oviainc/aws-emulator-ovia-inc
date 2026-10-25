// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Route 53 reusable delegation set slice:
//! `CreateReusableDelegationSet`, the `GetReusableDelegationSet` read that has
//! to report what the create persisted, and the hosted-zone operations that
//! name a set.
//!
//! The rules under test, each asserted over a SET of inputs rather than the one
//! request that reported them:
//!
//! 1. `CreateReusableDelegationSet` mints a reusable delegation set from the
//!    service's state and answers with it: a server-minted `Id`, the
//!    `CallerReference` the request carried, and four name servers — four of
//!    its own, or the four an existing public hosted zone already delegates to
//!    when `HostedZoneId` names one. Every later read reports the same set, so
//!    the table spans the caller reference (shortest, longest, one carrying XML
//!    special characters), whether a hosted zone is named, and which form its
//!    id is named in.
//! 2. `CallerReference` is the idempotency token, so a reference the account
//!    already holds a set for is `DelegationSetAlreadyCreated` and persists
//!    nothing; a `HostedZoneId` naming no zone is `HostedZoneNotFound`; a
//!    private zone's delegation set cannot be made reusable; and a zone whose
//!    name servers a set already delegates to is
//!    `DelegationSetAlreadyReusable`.
//! 3. A set is REUSABLE: a hosted zone created with `DelegationSetId`
//!    delegates to that set's four name servers rather than to four of its own
//!    — in the delegation set every hosted-zone read reports and in the NS
//!    record at the zone's apex — and `ListHostedZones` filtered by the set
//!    reports exactly the zones created in it. An id naming no set is
//!    `NoSuchDelegationSet` wherever it is named.
//! 4. A read reports and writes nothing: reading a set any number of times
//!    answers the same set and leaves the account's delegation set count where
//!    it was.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the hosted-zone suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::{
    ACCOUNT_LIMIT_MAX_REUSABLE_DELEGATION_SETS, DELEGATION_SET_PREFIX,
};

use crate::hosted_zone_records::{call, escape_xml, router, Reply, ENDPOINT};
use crate::vpc_associations::{bare_zone_id, create_zone, get_zone, vpc_block, HOME_REGION};

/// `Nonce` — what `CallerReference` targets — is `len 1..128`
/// (smithy_query(query="shape",shape="Nonce")).
const CALLER_REFERENCE_MIN_LEN: usize = 1;
/// The other published bound of `Nonce`.
const CALLER_REFERENCE_MAX_LEN: usize = 128;

/// How many name servers a delegation set delegates to.
///
/// `CreateReusableDelegationSet`'s own documentation calls a delegation set "a
/// group of four name servers"; `DelegationSetNameServers` is `len 1..`, so the
/// count is the operation's rule rather than the model's.
const NAME_SERVER_COUNT: usize = 4;

/// A `CreateReusableDelegationSet` body. `hosted_zone_id` of `None` sends no
/// `HostedZoneId` element at all.
fn create_set_body(caller_reference: &str, hosted_zone_id: Option<&str>) -> String {
    let zone = hosted_zone_id.map_or_else(String::new, |id| {
        format!("<HostedZoneId>{}</HostedZoneId>", escape_xml(id))
    });
    format!(
        "<CreateReusableDelegationSetRequest \
         xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
         <CallerReference>{}</CallerReference>{zone}\
         </CreateReusableDelegationSetRequest>",
        escape_xml(caller_reference)
    )
}

/// Send one `CreateReusableDelegationSet` request.
///
/// `pub(crate)` so a sibling suite asserting the wording of this operation's
/// zone refusal drives the same request rather than growing a second copy of
/// its body.
pub(crate) async fn create_set(
    router: &MultiServiceRouter,
    caller_reference: &str,
    hosted_zone_id: Option<&str>,
) -> Reply {
    call(
        router,
        "POST",
        "/2013-04-01/delegationset",
        &create_set_body(caller_reference, hosted_zone_id),
    )
    .await
}

/// Read one reusable delegation set back.
async fn get_set(router: &MultiServiceRouter, id: &str) -> Reply {
    call(router, "GET", &format!("/2013-04-01/delegationset/{id}"), "").await
}

/// How many reusable delegation sets the account holds, as `GetAccountLimit`
/// reports it — the count that has to agree with the sets in the store.
async fn set_count(router: &MultiServiceRouter) -> String {
    let reply = call(
        router,
        "GET",
        &format!("/2013-04-01/accountlimit/{ACCOUNT_LIMIT_MAX_REUSABLE_DELEGATION_SETS}"),
        "",
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from GetAccountLimit, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .first("Count")
        .unwrap_or_else(|| panic!("the reply must carry Count; body {}", reply.body))
        .to_string()
}

/// One `DelegationSet` as the wire reports it.
#[derive(Debug, PartialEq, Eq)]
struct SetView {
    id: String,
    caller_reference: Option<String>,
    name_servers: Vec<String>,
}

/// The `DelegationSet` a reply carries.
fn set_of(reply: &Reply, case: &str) -> SetView {
    SetView {
        id: reply
            .first("DelegationSet/Id")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the reply must carry DelegationSet/Id; body {}",
                    reply.body
                )
            })
            .to_string(),
        caller_reference: reply
            .first("DelegationSet/CallerReference")
            .map(str::to_string),
        name_servers: reply
            .all("DelegationSet/NameServers/NameServer")
            .into_iter()
            .map(str::to_string)
            .collect(),
    }
}

/// The name servers a reply's `DelegationSet` lists, in document order.
fn name_servers_of(reply: &Reply, case: &str) -> Vec<String> {
    let servers: Vec<String> = reply
        .all("DelegationSet/NameServers/NameServer")
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        servers.len(),
        NAME_SERVER_COUNT,
        "case {case:?}: expected {NAME_SERVER_COUNT} name servers, got {servers:?}; body {}",
        reply.body
    );
    servers
}

/// The bare (`N…`) id behind a `DelegationSet/Id`, which the API reports with
/// the `/delegationset/` prefix.
fn bare_set_id(id: &str) -> &str {
    id.trim_start_matches(DELEGATION_SET_PREFIX)
}

/// Whether `id` has the `/delegationset/` + `N` + 14 uppercase-alphanumerics
/// form Route 53 reports a delegation set id in (`/delegationset/N1PA6795SAMPLE`),
/// which also fits the 32 characters `ResourceId` allows.
fn is_delegation_set_id(id: &str) -> bool {
    let Some(bare) = id.strip_prefix(DELEGATION_SET_PREFIX) else {
        return false;
    };
    bare.starts_with('N')
        && bare.len() == 15
        && bare
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

/// Whether `server` is a host name a delegation set can delegate to: at least
/// three non-empty labels of letters, digits and hyphens, and no trailing dot
/// (a name server is reported bare in a delegation set and fully qualified only
/// inside DNS record data).
fn is_name_server(server: &str) -> bool {
    let labels: Vec<&str> = server.split('.').collect();
    labels.len() >= 3
        && labels.iter().all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

/// Assert a set's four name servers are four distinct, well-formed host names.
fn assert_name_servers(case: &str, servers: &[String]) {
    assert_eq!(
        servers.len(),
        NAME_SERVER_COUNT,
        "case {case:?}: expected {NAME_SERVER_COUNT} name servers, got {servers:?}"
    );
    for server in servers {
        assert!(
            is_name_server(server),
            "case {case:?}: {server:?} is not a name server a delegation set can delegate to"
        );
    }
    let mut distinct = servers.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        servers.len(),
        "case {case:?}: a delegation set's name servers must be distinct, got {servers:?}"
    );
}

/// A `DelegationSetId` element, for the `config` slot of [`create_zone`].
fn delegation_set_element(id: &str) -> String {
    format!("<DelegationSetId>{}</DelegationSetId>", escape_xml(id))
}

/// The hosted zones a listing reports, as bare (`Z…`) ids in document order.
async fn listed_zone_ids(router: &MultiServiceRouter, query: &str) -> Vec<String> {
    let reply = call(router, "GET", &format!("/2013-04-01/hostedzone{query}"), "").await;
    assert_eq!(
        reply.status, 200,
        "ListHostedZones{query} must answer 200, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .all("HostedZone/Id")
        .into_iter()
        .map(|id| id.trim_start_matches("/hostedzone/").to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: create, then read back
// ---------------------------------------------------------------------------

/// One row of the create table: the request sent, and the hosted zone (if any)
/// whose name servers the new set has to delegate to.
struct CreateRow {
    case: &'static str,
    caller_reference: String,
    /// `Some((zone name, send the id with its `/hostedzone/` prefix))` when the
    /// row marks an existing zone's delegation set reusable.
    zone: Option<(&'static str, bool)>,
}

#[tokio::test]
async fn a_created_delegation_set_reads_back_as_it_was_created() {
    let rows = [
        CreateRow {
            case: "a set of its own four name servers",
            caller_reference: "plain-set".to_string(),
            zone: None,
        },
        CreateRow {
            case: "a caller reference at the 1-character minimum",
            caller_reference: "x".repeat(CALLER_REFERENCE_MIN_LEN),
            zone: None,
        },
        CreateRow {
            case: "a caller reference at the 128-character maximum",
            caller_reference: "c".repeat(CALLER_REFERENCE_MAX_LEN),
            zone: None,
        },
        CreateRow {
            case: "a caller reference carrying XML special characters",
            caller_reference: "a&b<c>d\"e'f".to_string(),
            zone: None,
        },
        CreateRow {
            case: "an existing zone's delegation set, named by its bare id",
            caller_reference: "from-zone-bare".to_string(),
            zone: Some(("bare.example.com", false)),
        },
        CreateRow {
            case: "an existing zone's delegation set, named by its prefixed id",
            caller_reference: "from-zone-prefixed".to_string(),
            zone: Some(("prefixed.example.net", true)),
        },
    ];

    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for row in &rows {
        // A row that marks a zone's delegation set reusable needs that zone
        // first, and the four name servers it already delegates to.
        let zone = match row.zone {
            None => None,
            Some((name, prefixed)) => {
                let created = create_zone(&router, name, row.case, "").await;
                assert_eq!(
                    created.status, 201,
                    "case {:?}: expected 201 from CreateHostedZone, got {} with body {}",
                    row.case, created.status, created.body
                );
                let bare = bare_zone_id(&created);
                let sent = if prefixed {
                    format!("/hostedzone/{bare}")
                } else {
                    bare.clone()
                };
                Some((bare, sent, name_servers_of(&created, row.case)))
            }
        };

        let created = create_set(
            &router,
            &row.caller_reference,
            zone.as_ref().map(|(_, sent, _)| sent.as_str()),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateReusableDelegationSet, got {} with body {}",
            row.case, created.status, created.body
        );
        let answered = set_of(&created, row.case);
        assert!(
            is_delegation_set_id(&answered.id),
            "case {:?}: expected a /delegationset/-prefixed minted id, got {:?}",
            row.case,
            answered.id
        );
        assert!(
            !minted.contains(&answered.id),
            "case {:?}: every created set must get its own id, but {:?} was minted twice",
            row.case,
            answered.id
        );
        assert_eq!(
            answered.caller_reference.as_deref(),
            Some(row.caller_reference.as_str()),
            "case {:?}: the set must report the CallerReference it was created with, got {:?}",
            row.case,
            answered.caller_reference
        );
        assert_name_servers(row.case, &answered.name_servers);

        // The Location names the resource the read is bound to, at the endpoint
        // the request was sent to.
        let expected_location = format!(
            "https://{ENDPOINT}/2013-04-01/delegationset/{}",
            bare_set_id(&answered.id)
        );
        assert_eq!(
            created.location.as_deref(),
            Some(expected_location.as_str()),
            "case {:?}: expected the Location {expected_location:?}, got {:?}",
            row.case,
            created.location
        );

        // A set made from a zone delegates to exactly that zone's name servers,
        // and the zone now reports the set it belongs to.
        if let Some((bare_zone, _, zone_servers)) = &zone {
            assert_eq!(
                answered.name_servers, *zone_servers,
                "case {:?}: the set must delegate to the zone's own name servers, got {:?} \
                 for {zone_servers:?}",
                row.case, answered.name_servers
            );
            let read_zone = get_zone(&router, bare_zone).await;
            let zone_set = set_of(&read_zone, row.case);
            assert_eq!(
                zone_set, answered,
                "case {:?}: GetHostedZone must report the set the zone's delegation became, \
                 got {zone_set:?} after {answered:?}; body {}",
                row.case, read_zone.body
            );
        }

        // Round trip: the read reports the very same set, whichever form of the
        // id it is asked by — the bare id, and the `/delegationset/`-prefixed
        // `Id` the response reported (percent-encoded, as a client sends a path
        // label carrying slashes).
        for (form, id) in [
            ("the bare id", bare_set_id(&answered.id).to_string()),
            (
                "the prefixed id",
                format!("%2Fdelegationset%2F{}", bare_set_id(&answered.id)),
            ),
        ] {
            let read = get_set(&router, &id).await;
            assert_eq!(
                read.status, 200,
                "case {:?} via {form}: expected 200 from GetReusableDelegationSet, got {} \
                 with body {}",
                row.case, read.status, read.body
            );
            let got = set_of(&read, row.case);
            assert_eq!(
                got, answered,
                "case {:?} via {form}: the read must report the set the create persisted, \
                 got {got:?} after {answered:?}; body {}",
                row.case, read.body
            );
        }

        minted.push(answered.id);
    }

    // The account holds exactly the sets that were created.
    let count = set_count(&router).await;
    assert_eq!(
        count,
        minted.len().to_string(),
        "expected the account to hold {} reusable delegation sets, got Count {count:?}",
        minted.len()
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the refusals, none of which persist anything
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_caller_reference_the_account_already_holds_is_refused() {
    let router = router();
    let first = create_set(&router, "taken", None).await;
    assert_eq!(
        first.status, 201,
        "expected 201 from CreateReusableDelegationSet, got {} with body {}",
        first.status, first.body
    );
    let held = set_of(&first, "the set that holds the reference");

    // The refusal is about the REFERENCE, so it stands whatever else differs:
    // this repeat also names a hosted zone, which the first request did not.
    let zone = create_zone(&router, "repeat.example.com", "repeat-zone", "").await;
    assert_eq!(
        zone.status, 201,
        "expected 201 from CreateHostedZone, got {} with body {}",
        zone.status, zone.body
    );
    let zone_id = bare_zone_id(&zone);
    let again = create_set(&router, "taken", Some(&zone_id)).await;
    assert_eq!(
        again.status, 400,
        "expected HTTP 400 for a caller reference the account already holds, got {} with body {}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "DelegationSetAlreadyCreated",
        "expected DelegationSetAlreadyCreated, got body {}",
        again.body
    );

    // The set already holding the reference is untouched, no second set came
    // into being, and the zone named by the refused request kept its own
    // delegation.
    let read = get_set(&router, bare_set_id(&held.id)).await;
    let got = set_of(&read, "after the refusal");
    assert_eq!(
        got, held,
        "a refused create must leave the set holding the reference exactly as it was, \
         got {got:?} after {held:?}; body {}",
        read.body
    );
    let count = set_count(&router).await;
    assert_eq!(
        count, "1",
        "a refused create must persist nothing, got Count {count:?}"
    );
    let read_zone = get_zone(&router, &zone_id).await;
    assert_eq!(
        read_zone.first("DelegationSet/Id"),
        None,
        "a refused create must leave the zone's delegation unnamed; body {}",
        read_zone.body
    );
}

#[tokio::test]
async fn a_hosted_zone_whose_delegation_cannot_be_made_reusable_is_refused() {
    let router = router();
    // One zone whose delegation set is already reusable, and one private zone:
    // the two zones a `HostedZoneId` can name that no new set can be made from.
    let public = create_zone(&router, "already.example.com", "already-reusable", "").await;
    assert_eq!(
        public.status, 201,
        "expected 201 from CreateHostedZone, got {} with body {}",
        public.status, public.body
    );
    let public_id = bare_zone_id(&public);
    let marked = create_set(&router, "first-time", Some(&public_id)).await;
    assert_eq!(
        marked.status, 201,
        "expected 201 from CreateReusableDelegationSet, got {} with body {}",
        marked.status, marked.body
    );

    let private = create_zone(
        &router,
        "private.example.com",
        "private-zone",
        &vpc_block(Some("vpc-1a2b3c4d"), Some(HOME_REGION)),
    )
    .await;
    assert_eq!(
        private.status, 201,
        "expected 201 from CreateHostedZone, got {} with body {}",
        private.status, private.body
    );
    let private_id = bare_zone_id(&private);

    let rows = [
        (
            "a bare zone id no zone carries",
            "Z0000000000000000".to_string(),
            "HostedZoneNotFound",
        ),
        (
            "a prefixed zone id no zone carries",
            "/hostedzone/Z0000000000000000".to_string(),
            "HostedZoneNotFound",
        ),
        (
            "a zone whose delegation set is already reusable",
            public_id.clone(),
            "DelegationSetAlreadyReusable",
        ),
        (
            "a private hosted zone",
            private_id.clone(),
            "InvalidArgument",
        ),
    ];

    for (index, (case, hosted_zone_id, code)) in rows.iter().enumerate() {
        let reply = create_set(&router, &format!("refused-{index}"), Some(hosted_zone_id)).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 from CreateReusableDelegationSet, got {} \
             with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case:?}: expected {code}, got body {}",
            reply.body
        );
        // Only the one set created above is held: no refusal persisted a thing.
        let count = set_count(&router).await;
        assert_eq!(
            count, "1",
            "case {case:?}: a refused create must persist nothing, got Count {count:?}"
        );
    }
}

#[tokio::test]
async fn an_id_no_set_exists_for_is_refused() {
    let router = router();
    let created = create_set(&router, "the-only-set", None).await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateReusableDelegationSet, got {} with body {}",
        created.status, created.body
    );
    let held = set_of(&created, "the only set");

    // An id the store does not hold is the same refusal whichever form it is
    // written in, and a hosted-zone id is not a delegation set id.
    let rows = [
        ("a bare id no set carries", "N0000000000000000".to_string()),
        (
            "a prefixed id no set carries",
            "%2Fdelegationset%2FN0000000000000000".to_string(),
        ),
        ("a hosted zone id", "Z0000000000000000".to_string()),
    ];
    for (case, id) in &rows {
        let reply = get_set(&router, id).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 from GetReusableDelegationSet, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NoSuchDelegationSet",
            "case {case:?}: expected NoSuchDelegationSet, got body {}",
            reply.body
        );
    }

    // The set the store does hold still reads.
    let read = get_set(&router, bare_set_id(&held.id)).await;
    let got = set_of(&read, "the set the store holds");
    assert_eq!(
        got, held,
        "the stored set must still read back, got {got:?}; body {}",
        read.body
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a set is reusable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_zone_created_in_a_delegation_set_delegates_to_it() {
    let router = router();
    let created = create_set(&router, "reused-by-zones", None).await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateReusableDelegationSet, got {} with body {}",
        created.status, created.body
    );
    let set = set_of(&created, "the set the zones are created in");
    let bare = bare_set_id(&set.id).to_string();

    // The rows differ along the form the set id is named in and the zone's own
    // name, not merely the name: every zone in the set delegates to the SAME
    // four name servers, which is what makes the set reusable.
    let rows = [
        ("the bare set id", "first.example.com", bare.clone()),
        ("the prefixed set id", "second.example.org", set.id.clone()),
        ("the bare set id again", "third.example.net", bare.clone()),
    ];

    let mut in_set: Vec<String> = Vec::new();
    for (case, name, sent) in &rows {
        let zone = create_zone(&router, name, case, &delegation_set_element(sent)).await;
        assert_eq!(
            zone.status, 201,
            "case {case:?}: expected 201 from CreateHostedZone, got {} with body {}",
            zone.status, zone.body
        );
        let answered = set_of(&zone, case);
        assert_eq!(
            answered, set,
            "case {case:?}: a zone created in a set must report that set, got {answered:?} \
             for {set:?}; body {}",
            zone.body
        );
        let zone_id = bare_zone_id(&zone);

        // The same set is reported by the read, and the NS record at the zone's
        // apex carries the set's four name servers fully qualified. The listing
        // starts at that record and takes just it.
        let read = get_zone(&router, &zone_id).await;
        let read_set = set_of(&read, case);
        assert_eq!(
            read_set, set,
            "case {case:?}: GetHostedZone must report the set the zone was created in, \
             got {read_set:?}; body {}",
            read.body
        );
        let records = call(
            &router,
            "GET",
            &format!("/2013-04-01/hostedzone/{zone_id}/rrset?name={name}&type=NS&maxitems=1"),
            "",
        )
        .await;
        assert_eq!(
            records.status, 200,
            "case {case:?}: expected 200 from ListResourceRecordSets, got {} with body {}",
            records.status, records.body
        );
        let apex: Vec<String> = records
            .all("ResourceRecord/Value")
            .into_iter()
            .map(str::to_string)
            .collect();
        let expected: Vec<String> = set
            .name_servers
            .iter()
            .map(|server| format!("{server}."))
            .collect();
        assert_eq!(
            apex, expected,
            "case {case:?}: the apex NS record must carry the set's name servers, got {apex:?} \
             for {expected:?}; body {}",
            records.body
        );

        in_set.push(zone_id);
    }

    // A zone created in NO set keeps four name servers of its own, which are
    // not the set's.
    let outside = create_zone(&router, "outside.example.com", "no-set", "").await;
    assert_eq!(
        outside.status, 201,
        "expected 201 from CreateHostedZone, got {} with body {}",
        outside.status, outside.body
    );
    assert_eq!(
        outside.first("DelegationSet/Id"),
        None,
        "a zone created in no set must report no delegation set id; body {}",
        outside.body
    );
    let outside_servers = name_servers_of(&outside, "a zone in no set");
    assert_ne!(
        outside_servers, set.name_servers,
        "a zone created in no set must get name servers of its own, got the set's {:?}",
        set.name_servers
    );

    // The listing filtered by the set is exactly the zones created in it,
    // whichever form the filter names it in.
    let mut expected_ids = in_set.clone();
    expected_ids.sort_unstable();
    for (case, filter) in [("bare", bare.as_str()), ("prefixed", set.id.as_str())] {
        let listed = listed_zone_ids(&router, &format!("?delegationsetid={filter}")).await;
        assert_eq!(
            listed, expected_ids,
            "case {case:?}: the listing filtered by the set must be the zones created in it, \
             got {listed:?} for {expected_ids:?}"
        );
    }

    // An id naming no set is the refusal, not an empty page.
    let reply = call(
        &router,
        "GET",
        "/2013-04-01/hostedzone?delegationsetid=N0000000000000000",
        "",
    )
    .await;
    assert_eq!(
        reply.status, 400,
        "expected HTTP 400 from ListHostedZones for an id no set carries, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "NoSuchDelegationSet",
        "expected NoSuchDelegationSet, got body {}",
        reply.body
    );

    // And a private zone cannot be created in a reusable delegation set at all.
    let private = create_zone(
        &router,
        "private.example.com",
        "private-in-set",
        &format!(
            "{}{}",
            vpc_block(Some("vpc-1a2b3c4d"), Some(HOME_REGION)),
            delegation_set_element(&bare)
        ),
    )
    .await;
    assert_eq!(
        private.status, 400,
        "expected HTTP 400 for a private zone named with a delegation set, got {} with body {}",
        private.status, private.body
    );
    assert_eq!(
        private.error_code(),
        "InvalidInput",
        "expected InvalidInput, got body {}",
        private.body
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a read reports and writes nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reading_a_delegation_set_leaves_the_account_exactly_as_it_was() {
    let router = router();
    let created = create_set(&router, "read-repeatedly", None).await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateReusableDelegationSet, got {} with body {}",
        created.status, created.body
    );
    let held = set_of(&created, "the created set");

    for attempt in 1..=3 {
        let read = get_set(&router, bare_set_id(&held.id)).await;
        let got = set_of(&read, "a repeated read");
        assert_eq!(
            got, held,
            "read {attempt}: every read must report the same set, got {got:?}; body {}",
            read.body
        );
        let count = set_count(&router).await;
        assert_eq!(
            count, "1",
            "read {attempt}: reading a set must not move the account's count, got Count {count:?}"
        );
    }
}
