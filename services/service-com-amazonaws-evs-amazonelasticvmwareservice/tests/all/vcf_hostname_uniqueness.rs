// SPDX-License-Identifier: CC0-1.0

//! The uniqueness rule over the members of `vcfHostnames`, over the wire.
//!
//! The rule under test: when Amazon EVS installs VCF, no two members of one
//! `vcfHostnames` object may carry the same DNS hostname — "Each hostname must be
//! unique" (`VcfHostnames` doc, `smithy_query(query="shape", shape="VcfHostnames")`) —
//! so the FIRST member to carry a hostname keeps it and every member that repeats it is
//! refused with a `fieldList` entry naming both members, while an object whose
//! hostnames are all distinct is accepted and reported back unchanged.
//!
//! The rows below vary WHICH pair collides (each of the nine members, in both model
//! orders), HOW MANY collisions one request carries (one pair, two disjoint pairs, a
//! triple, all nine), the FORM of hostnames that are merely similar (a shared prefix,
//! a one-character difference — distinct, so accepted), and the mode (`SELF_DEPLOYED`
//! installs no appliance, so it claims no hostname). The pair the issue report cites is
//! one row of that table.

use serde_json::{json, Value};

use crate::create_environment_validation::{assert_aggregated_refusal, environment_count};
use crate::environment_lifecycle::{call, emulator, error_type};

/// The nine `vcfHostnames` members, in the order `VcfHostnames` models them
/// (`smithy_query(query="shape", shape="VcfHostnames")`: vCenter, nsx, nsxManager1..3,
/// nsxEdge1..2, sddcManager, cloudBuilder — all nine `@required`).
const VCF_HOSTNAME_MEMBERS: [&str; 9] = [
    "vCenter",
    "nsx",
    "nsxManager1",
    "nsxManager2",
    "nsxManager3",
    "nsxEdge1",
    "nsxEdge2",
    "sddcManager",
    "cloudBuilder",
];

/// A `vcfHostnames` object whose nine members carry nine distinct hostnames, except
/// that every member named in `sharing` carries the hostname given beside it.
///
/// The names stay inside the modelled `HostName` pattern (`^([a-zA-Z0-9\-]*)$`).
pub(crate) fn hostnames_with(sharing: &[(&str, &str)]) -> Value {
    let mut object = serde_json::Map::new();
    for member in VCF_HOSTNAME_MEMBERS {
        object.insert(member.to_string(), json!(format!("evs-{member}")));
    }
    for (member, hostname) in sharing {
        assert!(
            VCF_HOSTNAME_MEMBERS.contains(member),
            "the fixture names a vcfHostnames member: {member:?}"
        );
        object.insert((*member).to_string(), json!(*hostname));
    }
    Value::Object(object)
}

/// One row of the table below: the case, the `vcfHostnames` object the request
/// carries, what else it overrides on an otherwise valid request, and the members the
/// refusal must name — each paired with the member whose hostname it repeats, or with
/// `""` for an expected failure of another rule. An empty list is a row that must be
/// accepted.
type Row = (
    &'static str,
    Value,
    Value,
    Vec<(&'static str, &'static str)>,
);

/// The member path a `fieldList` entry carries for a `vcfHostnames` member.
fn path(member: &str) -> String {
    format!("vcfHostnames.{member}")
}

#[tokio::test]
async fn no_two_vcf_hostnames_of_one_request_may_be_the_same() {
    let rows: Vec<Row> = vec![
        (
            "nine distinct hostnames are accepted",
            hostnames_with(&[]),
            json!({}),
            vec![],
        ),
        (
            "vCenter and sddcManager carry the same hostname",
            hostnames_with(&[("vCenter", "evs-1"), ("sddcManager", "evs-1")]),
            json!({}),
            vec![("sddcManager", "vCenter")],
        ),
        (
            "two adjacent NSX Manager members carry the same hostname",
            hostnames_with(&[("nsxManager1", "mgr"), ("nsxManager2", "mgr")]),
            json!({}),
            vec![("nsxManager2", "nsxManager1")],
        ),
        (
            "the second NSX Edge node repeats the NSX virtual IP hostname",
            hostnames_with(&[("nsx", "edge"), ("nsxEdge2", "edge")]),
            json!({}),
            vec![("nsxEdge2", "nsx")],
        ),
        (
            "the last member repeats the first",
            hostnames_with(&[("vCenter", "shared"), ("cloudBuilder", "shared")]),
            json!({}),
            vec![("cloudBuilder", "vCenter")],
        ),
        (
            "two disjoint pairs collide in one request",
            hostnames_with(&[
                ("vCenter", "first"),
                ("nsx", "first"),
                ("nsxEdge1", "second"),
                ("cloudBuilder", "second"),
            ]),
            json!({}),
            vec![("nsx", "vCenter"), ("cloudBuilder", "nsxEdge1")],
        ),
        (
            "three members share one hostname, so the two that repeat it are refused",
            hostnames_with(&[
                ("nsxManager1", "mgr"),
                ("nsxManager2", "mgr"),
                ("nsxManager3", "mgr"),
            ]),
            json!({}),
            vec![("nsxManager2", "nsxManager1"), ("nsxManager3", "nsxManager1")],
        ),
        (
            "all nine members carry one hostname",
            hostnames_with(
                &VCF_HOSTNAME_MEMBERS
                    .iter()
                    .map(|member| (*member, "only-one"))
                    .collect::<Vec<_>>(),
            ),
            json!({}),
            // The first member keeps the name; the other eight repeat it.
            VCF_HOSTNAME_MEMBERS[1..]
                .iter()
                .map(|member| (*member, "vCenter"))
                .collect(),
        ),
        (
            "a collision is aggregated with a failure of another precondition",
            hostnames_with(&[("nsxEdge1", "dup"), ("nsxEdge2", "dup")]),
            json!({ "termsAccepted": false }),
            vec![("nsxEdge2", "nsxEdge1"), ("termsAccepted", "")],
        ),
        (
            "hostnames that share a prefix are distinct, so they are accepted",
            hostnames_with(&[("vCenter", "evs"), ("sddcManager", "evs-1")]),
            json!({}),
            vec![],
        ),
        (
            "hostnames differing in one character are distinct, so they are accepted",
            hostnames_with(&[("nsxEdge1", "evs-a"), ("nsxEdge2", "evs-b")]),
            json!({}),
            vec![],
        ),
        (
            // "When you specify SELF_DEPLOYED for vcfVersion, Amazon EVS provisions only
            // the VLAN subnets; no hosts are added and no VCF installation is performed.
            // The licenseInfo, hosts, vcfHostnames, siteId, and connectivityInfo
            // parameters are not supported in this mode" (this operation's doc): that
            // mode installs no appliance, so it claims no hostname and the member is
            // accepted as supplied — the same scope the sibling `hosts[].hostName`
            // uniqueness rule is judged at.
            "SELF_DEPLOYED installs no appliance, so a repeated hostname claims nothing",
            hostnames_with(&[("vCenter", "same"), ("sddcManager", "same")]),
            json!({ "vcfVersion": "SELF_DEPLOYED" }),
            vec![],
        ),
        (
            "the other VCF version Amazon EVS installs refuses the collision too",
            hostnames_with(&[("vCenter", "same"), ("nsxManager3", "same")]),
            json!({ "vcfVersion": "VCF-5.2.2" }),
            vec![("nsxManager3", "vCenter")],
        ),
    ];

    for (case, hostnames, overrides, expected) in rows {
        // A fresh emulator per row, so "nothing was created" is exact.
        let router = emulator();
        let mut request = router.create_request(Some("hostnames-env"));
        request["vcfHostnames"] = hostnames.clone();
        for (member, value) in overrides.as_object().expect("the overrides are an object") {
            request[member] = value.clone();
        }

        let (status, body) = call(&router, "CreateEnvironment", &request).await;

        if expected.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                error_type(&body)
            );
            // Round trip: the hostnames the request gave are what the stored
            // environment reports when it is read back.
            let id = body["environment"]["environmentId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
                .to_string();
            let (status, got) =
                call(&router, "GetEnvironment", &json!({ "environmentId": id })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: read back expected HTTP 200, got {status} ({}) {got}",
                error_type(&got)
            );
            assert_eq!(
                got["environment"]["vcfHostnames"], hostnames,
                "case {case:?}: expected the stored vcfHostnames {hostnames}, got {}",
                got["environment"]["vcfHostnames"]
            );
            continue;
        }

        let expected_paths: Vec<String> = expected
            .iter()
            .map(|(member, collides_with)| {
                if collides_with.is_empty() {
                    (*member).to_string()
                } else {
                    path(member)
                }
            })
            .collect();
        let expected_members: Vec<&str> = expected_paths.iter().map(String::as_str).collect();
        assert_aggregated_refusal(case, status, &body, &expected_members);

        // Each uniqueness entry names the member whose hostname its member repeats, so
        // the caller learns which member claimed the name.
        let entries = body["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no fieldList in {body}"));
        for (member, collides_with) in &expected {
            if collides_with.is_empty() {
                continue;
            }
            let name = path(member);
            let entry = entries
                .iter()
                .find(|entry| entry["name"] == json!(name))
                .unwrap_or_else(|| panic!("case {case:?}: no fieldList entry for {name} in {body}"));
            let message = entry["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&path(collides_with)),
                "case {case:?}: expected the entry for {name} to name {}, got {message:?}",
                path(collides_with)
            );
        }

        assert_eq!(
            environment_count(&router).await,
            0,
            "case {case:?}: a refused create must not persist an environment"
        );
    }
}
