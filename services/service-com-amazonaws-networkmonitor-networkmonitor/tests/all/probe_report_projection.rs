// SPDX-License-Identifier: CC0-1.0

//! Every probe read path reports ONE stored record the same way.
//!
//! `CreateProbe`, `GetProbe`, `UpdateProbe` and the `probes[]` entry
//! `GetMonitor` lists are four separate response shapes modelling the same
//! members, and each of them answers from the stored probe record. The rule:
//! the members an operation reports for a probe, and their values, come from
//! that record alone, so the four shapes report a given stored probe
//! identically — the only member that may differ between two of these answers
//! is one the record itself changed in between (`state`, which the clock moves
//! from `PENDING` to `ACTIVE`).
//!
//! The table spans the class the rule covers: the destination's address family
//! (IPv4 and IPv6, so `addressFamily` is derived either way), the protocol
//! (`ICMP` with no port, `TCP` with one), whether the optional members are
//! named at all, the published bounds of `destinationPort` and `packetSize`,
//! and how many tags the probe carries (none, one, three). The recorded case —
//! `ICMP` to an IPv4 destination with a single tag — is one row.

use std::collections::BTreeSet;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{
    call, create_monitor, create_probe, get_monitor, is_modelled_probe_id, probes_of, source_arn_of,
    string_at, PAST_ACTIVATION,
};

/// One row of the table: a probe to create, and what the record it stores
/// reports back.
struct Row {
    /// What this row varies, named in every assertion of it.
    case: &'static str,
    /// `destination` the request names.
    destination: &'static str,
    /// `protocol` the request names.
    protocol: &'static str,
    /// The optional `probe` members this row adds, as JSON body text
    /// (`,"destinationPort":1`).
    extra: &'static str,
    /// The `tags` the request assigns the probe, as JSON body text (`{}` for a
    /// probe carrying none).
    tags: &'static str,
    /// The `addressFamily` the stored record reports, derived from
    /// `destination`.
    address_family: &'static str,
}

/// The rows. `destinationPort` is bounded by the model's `Port`
/// (`smithy_query(query="shape",shape="Port")` -> `@range(min: 0, max: 65536)`)
/// with the documented floor of `1` for `ProbeInput$destinationPort`, and
/// `packetSize` by `PacketSize` (`@range(min: 56, max: 8500)`), so both bounds
/// appear here.
const ROWS: &[Row] = &[
    Row {
        case: "the recorded case: ICMP to an IPv4 destination, one tag, no optional members",
        destination: "10.0.0.1",
        protocol: "ICMP",
        extra: "",
        tags: r#"{"team":"net"}"#,
        address_family: "IPV4",
    },
    Row {
        case: "TCP to an IPv4 destination at the lowest port and smallest packet, untagged",
        destination: "192.168.3.4",
        protocol: "TCP",
        extra: r#","destinationPort":1,"packetSize":56"#,
        tags: "{}",
        address_family: "IPV4",
    },
    Row {
        case: "TCP to an IPv6 destination at the highest port and largest packet, three tags",
        destination: "2001:db8::1",
        protocol: "TCP",
        extra: r#","destinationPort":65536,"packetSize":8500"#,
        tags: r#"{"a":"1","b":"2","c":"3"}"#,
        address_family: "IPV6",
    },
    Row {
        case: "ICMP to a compressed IPv6 destination, packet size only",
        destination: "::1",
        protocol: "ICMP",
        extra: r#","packetSize":1500"#,
        tags: "{}",
        address_family: "IPV6",
    },
];

async fn get_probe(router: &MultiServiceRouter, name: &str, probe_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/monitors/{name}/probes/{probe_id}"),
        None,
    )
    .await
}

/// An `UpdateProbe` that names no member, so it changes nothing and answers the
/// record exactly as it stands.
async fn update_probe_noop(
    router: &MultiServiceRouter,
    name: &str,
    probe_id: &str,
) -> (u16, Value) {
    call(
        router,
        "PATCH",
        &format!("/monitors/{name}/probes/{probe_id}"),
        Some("{}"),
    )
    .await
}

/// The members a response reports, so two answers can be compared on the SET of
/// members and not only on the values of the ones both carry.
fn members_of(body: &Value) -> BTreeSet<String> {
    body.as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

/// The value a response reports for `member`, with an absent member read as
/// `Null`.
fn member_of(body: &Value, member: &str) -> Value {
    body.get(member).cloned().unwrap_or(Value::Null)
}

/// The `probes[]` entry `GetMonitor` reports for `probe_id`.
fn listed_probe(body: &Value, probe_id: &str) -> Value {
    probes_of(body)
        .into_iter()
        .find(|probe| string_at(probe, "probeId") == probe_id)
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn every_probe_read_path_reports_the_same_stored_record() {
    let (router, fixtures) = probes_of_every_row().await;
    // The probes were all created before this wait, so by now the clock owes
    // each of them its `PENDING` -> `ACTIVE` transition and every read below
    // sees a settled record that no longer moves under it.
    std::thread::sleep(PAST_ACTIVATION);

    for (index, row) in ROWS.iter().enumerate() {
        let (name, probe_id, created) = &fixtures[index];
        let (get_status, got) = get_probe(&router, name, probe_id).await;
        assert_eq!(
            get_status, 200,
            "case {:?}: GetProbe expected 200, got {get_status} {got:?}",
            row.case
        );
        let (update_status, updated) = update_probe_noop(&router, name, probe_id).await;
        assert_eq!(
            update_status, 200,
            "case {:?}: UpdateProbe expected 200, got {update_status} {updated:?}",
            row.case
        );
        let (monitor_status, monitor) = get_monitor(&router, name).await;
        assert_eq!(
            monitor_status, 200,
            "case {:?}: GetMonitor expected 200, got {monitor_status} {monitor:?}",
            row.case
        );
        let listed = listed_probe(&monitor, probe_id);

        // Every answer reports the SAME members: a member one read path carries
        // and another drops would be a probe reported differently depending on
        // which operation was asked.
        let expected_members = members_of(created);
        for (operation, body) in [
            ("GetProbe", &got),
            ("UpdateProbe", &updated),
            ("GetMonitor probes[]", &listed),
        ] {
            let actual = members_of(body);
            assert_eq!(
                actual, expected_members,
                "case {:?}: {operation} must report the members CreateProbe did, \
                 expected {expected_members:?}, got {actual:?}",
                row.case
            );
        }

        // `state` is the one member the RECORD moved between the create and
        // these reads, so it is asserted on its own below; every other member is
        // the same stored value in all four answers.
        for member in expected_members.iter().filter(|member| *member != "state") {
            let expected = member_of(created, member);
            for (operation, body) in [
                ("GetProbe", &got),
                ("UpdateProbe", &updated),
                ("GetMonitor probes[]", &listed),
            ] {
                let actual = member_of(body, member);
                assert_eq!(
                    actual, expected,
                    "case {:?}: {operation} must report the {member} CreateProbe stored, \
                     expected {expected:?}, got {actual:?}",
                    row.case
                );
            }
        }

        // The values themselves are the stored record's, so the shared
        // projection is pinned to the request that created the probe and not
        // merely to itself.
        let reported_id = string_at(created, "probeId");
        assert!(
            is_modelled_probe_id(&reported_id),
            "case {:?}: probeId must satisfy the modelled ProbeId pattern, got {reported_id:?}",
            row.case
        );
        assert_eq!(
            string_at(created, "destination"),
            row.destination,
            "case {:?}: destination must be the one stored, got {:?}",
            row.case,
            string_at(created, "destination")
        );
        assert_eq!(
            string_at(created, "protocol"),
            row.protocol,
            "case {:?}: protocol must be the one stored, got {:?}",
            row.case,
            string_at(created, "protocol")
        );
        assert_eq!(
            string_at(created, "addressFamily"),
            row.address_family,
            "case {:?}: addressFamily is derived from the destination, got {:?}",
            row.case,
            string_at(created, "addressFamily")
        );
        assert_eq!(
            string_at(created, "sourceArn"),
            source_arn_of(index),
            "case {:?}: sourceArn must be the one stored, got {:?}",
            row.case,
            string_at(created, "sourceArn")
        );
        let reported_arn = string_at(created, "probeArn");
        assert!(
            reported_arn.ends_with(&format!("probe/{reported_id}")),
            "case {:?}: probeArn must name the probe's own id, got {reported_arn:?}",
            row.case
        );
        let expected_tags: Value =
            serde_json::from_str(row.tags).expect("a row's tags are well-formed JSON");
        assert_eq!(
            member_of(created, "tags"),
            expected_tags,
            "case {:?}: tags must be the pairs stored, got {:?}",
            row.case,
            member_of(created, "tags")
        );
        assert_eq!(
            member_of(created, "createdAt"),
            member_of(created, "modifiedAt"),
            "case {:?}: a freshly created probe was never modified after it was created, \
             got createdAt {:?} and modifiedAt {:?}",
            row.case,
            member_of(created, "createdAt"),
            member_of(created, "modifiedAt")
        );

        // `state`: the create answers the record as it stood (provisioning),
        // and each later read answers the transition the clock has since made
        // due — one record, reported by all three read paths alike.
        assert_eq!(
            string_at(created, "state"),
            "PENDING",
            "case {:?}: CreateProbe reports the provisioning state, got {:?}",
            row.case,
            string_at(created, "state")
        );
        for (operation, body) in [
            ("GetProbe", &got),
            ("UpdateProbe", &updated),
            ("GetMonitor probes[]", &listed),
        ] {
            let actual = string_at(body, "state");
            assert_eq!(
                actual, "ACTIVE",
                "case {:?}: {operation} reports the state the clock already made due, \
                 expected ACTIVE, got {actual:?}",
                row.case
            );
        }
    }
}

/// What one row's probe was stored as: its monitor's name, its minted id and
/// the `CreateProbe` answer.
type Fixture = (String, String, Value);

/// Create one monitor per row, each holding that row's probe in a subnet of its
/// own, and answer the router beside what each create reported — built once, so
/// the whole table shares a single wait for the activation the clock owes.
async fn probes_of_every_row() -> (MultiServiceRouter, Vec<Fixture>) {
    let router = super::monitor_and_probe::router();
    let mut fixtures = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let name = format!("probe-projection-{index}");
        let (status, monitor) =
            create_monitor(&router, &format!(r#"{{"monitorName":"{name}"}}"#)).await;
        assert_eq!(
            status, 200,
            "case {:?}: setup CreateMonitor expected 200, got {status} {monitor:?}",
            row.case
        );
        let body = format!(
            r#"{{"probe":{{"sourceArn":"{}","destination":"{}","protocol":"{}"{}}},"tags":{}}}"#,
            source_arn_of(index),
            row.destination,
            row.protocol,
            row.extra,
            row.tags
        );
        let (status, created) = create_probe(&router, &name, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateProbe expected 200, got {status} {created:?}",
            row.case
        );
        let probe_id = string_at(&created, "probeId");
        fixtures.push((name, probe_id, created));
    }
    (router, fixtures)
}
