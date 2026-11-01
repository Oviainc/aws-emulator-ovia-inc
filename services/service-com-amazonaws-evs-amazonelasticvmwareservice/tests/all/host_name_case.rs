// SPDX-License-Identifier: CC0-1.0

//! Which host a `hostName` NAMES, end to end over awsJson1_0.
//!
//! The rule under test: a `hostName` is a DNS hostname — "The DNS hostname of the host.
//! DNS hostnames for hosts must be unique across Amazon EVS environments and within VCF"
//! (`smithy_query(query="shape",shape="HostInfoForCreate")`) — and a DNS name is
//! case-insensitive, so two spellings that differ only in case are ONE hostname. The
//! modelled `HostName` pattern `^([a-zA-Z0-9\-]*)$`
//! (`smithy_query(query="shape",shape="HostName")`) admits upper and lower case alike, so
//! the boundary validator passes either spelling and the service is what decides the two
//! are one name. Every site that asks whether two host DNS hostnames are the same name
//! therefore answers yes for either spelling:
//!
//! * `CreateEnvironmentHost` refuses a `host.hostName` a live host already carries,
//!   whichever case either is spelled in — in the same environment or in another one,
//!   because the uniqueness the doc states is across environments;
//! * `CreateEnvironment` refuses a `hosts[i].hostName` for the same reason, and refuses an
//!   entry whose name another entry of the same request already took;
//! * no two members of one `vcfHostnames` object may carry one hostname ("Each hostname
//!   must be unique", `smithy_query(query="shape",shape="VcfHostnames")`), judged the same
//!   way; and
//! * `DeleteEnvironmentHost` addresses the host its `hostName` names — "The DNS hostname
//!   associated with the host to be deleted"
//!   (`smithy_query(query="shape",shape="DeleteEnvironmentHostRequest")`) — so it reaches
//!   that host whichever case the request spells its name in.
//!
//! What the rule generalises over, and so what the tables below span: the case
//! transformation between the stored spelling and the requested one (upper from lower,
//! lower from upper, mixed case, and the identical spelling), the form and length of the
//! name (a plain label, a label carrying digits and hyphens, a single character, a
//! 63-character label), WHERE the name the request collides with is held (the same
//! environment, another environment, another entry of the same request), and the contrast
//! rows that differ by more than case and so name another host.
//!
//! The spelling a request gave is what is STORED and reported, so every row reads the
//! records back through `ListEnvironmentHosts`.

use serde_json::{json, Value};

use crate::create_environment_validation::{assert_aggregated_refusal, environment_count};
use crate::environment_lifecycle::{
    call, create, create_settled, emulator, error_type, settle, Emulator,
};
use crate::vcf_hostname_uniqueness::hostnames_with;

/// One row of the host-name tables: the case, the spelling a host is stored under, the
/// spelling the call under test names, and whether the two are one DNS hostname.
struct Row {
    case: &'static str,
    stored: String,
    requested: String,
    same_host: bool,
}

/// `name` with its case flipped character by character — the first upper, the next lower,
/// and so on — so a row's requested spelling is neither the stored one nor a uniform
/// transformation of it. (Per CHARACTER, unlike the per-label helper the sibling
/// appliance-FQDN suite owns: a `HostName` is a single label and carries no dot.)
fn alternating_case(name: &str) -> String {
    name.char_indices()
        .map(|(index, character)| {
            if index % 2 == 0 {
                character.to_ascii_uppercase()
            } else {
                character.to_ascii_lowercase()
            }
        })
        .collect()
}

fn rows() -> Vec<Row> {
    // The longest a single DNS label can be, to span the length the rule is indifferent
    // to; the modelled `HostName` declares no length bound at all.
    let long_label = "esx".to_string() + &"a".repeat(60);
    vec![
        // One DNS hostname, spelled differently.
        Row {
            case: "the identical spelling",
            stored: "esx-same".to_string(),
            requested: "esx-same".to_string(),
            same_host: true,
        },
        Row {
            case: "a stored lower-case name asked for in upper case",
            stored: "esx-upper".to_string(),
            requested: "ESX-UPPER".to_string(),
            same_host: true,
        },
        Row {
            case: "a stored upper-case name asked for in lower case",
            stored: "ESX-LOWER".to_string(),
            requested: "esx-lower".to_string(),
            same_host: true,
        },
        Row {
            case: "a stored name asked for in mixed case",
            stored: "esx-mixed".to_string(),
            requested: alternating_case("esx-mixed"),
            same_host: true,
        },
        Row {
            case: "a name carrying digits and hyphens, asked for in upper case",
            stored: "esx-07-west-2".to_string(),
            requested: "ESX-07-WEST-2".to_string(),
            same_host: true,
        },
        Row {
            case: "a single-character name asked for in upper case",
            stored: "e".to_string(),
            requested: "E".to_string(),
            same_host: true,
        },
        Row {
            case: "a 63-character label asked for in upper case",
            stored: long_label.clone(),
            requested: long_label.to_ascii_uppercase(),
            same_host: true,
        },
        // ANOTHER host: the contrast rows, differing by more than case.
        Row {
            case: "a name differing in its last character",
            stored: "esx-other-1".to_string(),
            requested: "esx-other-2".to_string(),
            same_host: false,
        },
        Row {
            case: "a longer name sharing the stored name as its prefix",
            stored: "esx-prefix".to_string(),
            requested: "esx-prefix-b".to_string(),
            same_host: false,
        },
        Row {
            case: "another name in upper case",
            stored: "esx-distinct".to_string(),
            requested: "ESX-DISTINCT-B".to_string(),
            same_host: false,
        },
    ]
}

/// A `CreateEnvironmentHost` request for one host name.
fn host_request(environment_id: &str, host_name: &str) -> Value {
    json!({
        "environmentId": environment_id,
        "host": {
            "hostName": host_name,
            "keyName": "evs-key",
            "instanceType": "i4i.metal",
        },
    })
}

/// Every host name one environment holds, as `ListEnvironmentHosts` reports them (stable
/// `hostName` order).
async fn listed_names(router: &Emulator, case: &str, environment: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListEnvironmentHosts",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListEnvironmentHosts expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environmentHosts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentHosts in {body}"))
        .iter()
        .map(|host| {
            host["hostName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a listed host carries a name: {host}"))
                .to_string()
        })
        .collect()
}

/// Add one host to a deployed environment, failing the case if it is refused.
async fn add_host(router: &Emulator, case: &str, environment: &str, host_name: &str) {
    let (status, body) = call(
        router,
        "CreateEnvironmentHost",
        &host_request(environment, host_name),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: adding host {host_name:?} expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    assert_eq!(
        body["host"]["hostName"],
        json!(host_name),
        "case {case:?}: the host must be stored under the spelling the request gave, got {}",
        body["host"]["hostName"]
    );
}

#[tokio::test]
async fn a_host_name_one_environment_holds_is_taken_in_every_spelling_of_it() {
    let router = emulator();
    for row in rows() {
        let case = row.case;
        let environment = create_settled(&router, None).await;
        add_host(&router, case, &environment, &row.stored).await;

        let (status, body) = call(
            &router,
            "CreateEnvironmentHost",
            &host_request(&environment, &row.requested),
        )
        .await;

        if row.same_host {
            assert_aggregated_refusal(case, status, &body, &["host.hostName"]);
            let message = body["fieldList"][0]["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("already used"),
                "case {case:?}: expected the refusal to say the name is already used, got \
                 {message:?}"
            );
            assert_eq!(
                listed_names(&router, case, &environment).await,
                vec![row.stored.clone()],
                "case {case:?}: one DNS hostname must leave one host record"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: another DNS hostname must be accepted, got {status} ({}) {body}",
                error_type(&body)
            );
            let mut expected = vec![row.stored.clone(), row.requested.clone()];
            expected.sort();
            assert_eq!(
                listed_names(&router, case, &environment).await,
                expected,
                "case {case:?}: both hosts must be reported"
            );
        }
    }
}

#[tokio::test]
async fn a_host_name_another_environment_holds_is_taken_in_every_spelling_of_it() {
    // "DNS hostnames for hosts must be unique across Amazon EVS environments and within
    // VCF": the scope is every environment, so the holder of the name sits in a different
    // environment from the one the create addresses.
    let router = emulator();
    for row in rows() {
        let case = row.case;
        let holder = create_settled(&router, None).await;
        add_host(&router, case, &holder, &row.stored).await;
        let environment = create_settled(&router, None).await;

        let (status, body) = call(
            &router,
            "CreateEnvironmentHost",
            &host_request(&environment, &row.requested),
        )
        .await;

        if row.same_host {
            assert_aggregated_refusal(case, status, &body, &["host.hostName"]);
            assert!(
                listed_names(&router, case, &environment).await.is_empty(),
                "case {case:?}: a refused create must add no host, got {:?}",
                listed_names(&router, case, &environment).await
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: another DNS hostname must be accepted, got {status} ({}) {body}",
                error_type(&body)
            );
            assert_eq!(
                listed_names(&router, case, &environment).await,
                vec![row.requested.clone()],
                "case {case:?}: the second environment holds the host it added"
            );
        }
        // The holder is untouched either way.
        assert_eq!(
            listed_names(&router, case, &holder).await,
            vec![row.stored.clone()],
            "case {case:?}: the environment holding the name keeps its one host"
        );
    }
}

#[tokio::test]
async fn a_create_may_not_name_two_hosts_whose_names_are_one_dns_hostname() {
    // Each row: the case, the four `hosts` names the create carries, and the `fieldList`
    // members the refusal must name (empty for a row that is served).
    let rows: Vec<(&str, [String; 4], Vec<&str>)> = vec![
        (
            "four distinct names",
            [
                "mix-free-1".to_string(),
                "mix-free-2".to_string(),
                "mix-free-3".to_string(),
                "mix-free-4".to_string(),
            ],
            vec![],
        ),
        (
            "the third entry repeats the first in upper case",
            [
                "mix-dup-a".to_string(),
                "mix-free-5".to_string(),
                "MIX-DUP-A".to_string(),
                "mix-free-6".to_string(),
            ],
            vec!["hosts[2].hostName"],
        ),
        (
            "the last entry repeats the first in mixed case",
            [
                "mix-dup-b".to_string(),
                "mix-free-7".to_string(),
                "mix-free-8".to_string(),
                alternating_case("mix-dup-b"),
            ],
            vec!["hosts[3].hostName"],
        ),
        (
            "two disjoint pairs of entries differ only in case",
            [
                "mix-dup-c".to_string(),
                "mix-dup-d".to_string(),
                "MIX-DUP-C".to_string(),
                "Mix-Dup-D".to_string(),
            ],
            vec!["hosts[2].hostName", "hosts[3].hostName"],
        ),
        (
            "the first entry names a host another environment holds, in upper case",
            [
                "MIX-HELD".to_string(),
                "mix-free-9".to_string(),
                "mix-free-10".to_string(),
                "mix-free-11".to_string(),
            ],
            vec!["hosts[0].hostName"],
        ),
    ];

    for (case, names, expected_members) in rows {
        // A fresh emulator per row, so "nothing was created" is exact; the environment
        // holding the taken name is the only one it starts with.
        let router = emulator();
        let holder = create_settled(&router, Some("mix-holder")).await;
        add_host(&router, case, &holder, "mix-held").await;

        let mut request = router.create_request(Some("mix-env"));
        request["hosts"] = Value::Array(
            names
                .iter()
                .map(|host_name| {
                    json!({
                        "hostName": host_name,
                        "keyName": "evs-key",
                        "instanceType": "i4i.metal",
                    })
                })
                .collect(),
        );
        let (status, body) = call(&router, "CreateEnvironment", &request).await;

        if expected_members.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                error_type(&body)
            );
            let created = body["environment"]["environmentId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
                .to_string();
            settle(&router);
            let mut expected: Vec<String> = names.to_vec();
            expected.sort();
            assert_eq!(
                listed_names(&router, case, &created).await,
                expected,
                "case {case:?}: every entry must be materialised as a host"
            );
        } else {
            assert_aggregated_refusal(case, status, &body, &expected_members);
            assert_eq!(
                environment_count(&router).await,
                1,
                "case {case:?}: a refused create must persist no environment beside the \
                 one holding the taken name"
            );
            assert_eq!(
                listed_names(&router, case, &holder).await,
                vec!["mix-held".to_string()],
                "case {case:?}: a refused create must add no host to the holder"
            );
        }
    }
}

/// One row of the `vcfHostnames` table: the case, the (member, hostname) pairs the
/// object shares, and the member the refusal must name together with the member whose
/// hostname it repeats — `None` for a row whose nine hostnames are distinct.
type HostnamesRow = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Option<(&'static str, &'static str)>,
);

#[tokio::test]
async fn two_vcf_hostnames_differing_only_in_case_are_one_hostname() {
    let rows: Vec<HostnamesRow> = vec![
        (
            "sddcManager repeats the vCenter hostname in upper case",
            vec![("vCenter", "evs-1"), ("sddcManager", "EVS-1")],
            Some(("sddcManager", "vCenter")),
        ),
        (
            "nsxManager2 repeats nsxManager1 in mixed case",
            vec![("nsxManager1", "evs-mgr"), ("nsxManager2", "EvS-mGr")],
            Some(("nsxManager2", "nsxManager1")),
        ),
        (
            "cloudBuilder repeats an upper-case vCenter hostname in lower case",
            vec![("vCenter", "EVS-CB"), ("cloudBuilder", "evs-cb")],
            Some(("cloudBuilder", "vCenter")),
        ),
        (
            "hostnames differing by more than case are distinct, so they are accepted",
            vec![("nsxEdge1", "evs-a"), ("nsxEdge2", "EVS-B")],
            None,
        ),
    ];

    for (case, sharing, expected) in rows {
        let router = emulator();
        let hostnames = hostnames_with(&sharing);
        let mut request = router.create_request(Some("hostname-case-env"));
        request["vcfHostnames"] = hostnames.clone();

        let (status, body) = call(&router, "CreateEnvironment", &request).await;

        match expected {
            Some((member, claimed_by)) => {
                let name = format!("vcfHostnames.{member}");
                assert_aggregated_refusal(case, status, &body, &[name.as_str()]);
                let message = body["fieldList"][0]["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(&format!("vcfHostnames.{claimed_by}")),
                    "case {case:?}: expected the entry for {name} to name the member that \
                     claimed the hostname, got {message:?}"
                );
                assert_eq!(
                    environment_count(&router).await,
                    0,
                    "case {case:?}: a refused create must persist no environment"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                    error_type(&body)
                );
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
                    "case {case:?}: the stored hostnames must be the ones the request gave, got {}",
                    got["environment"]["vcfHostnames"]
                );
            }
        }
    }
}

#[tokio::test]
async fn a_delete_addresses_its_host_in_every_spelling_of_its_dns_hostname() {
    let router = emulator();
    for row in rows() {
        let case = row.case;
        let environment = create_settled(&router, None).await;
        add_host(&router, case, &environment, &row.stored).await;
        settle(&router);

        let (status, body) = call(
            &router,
            "DeleteEnvironmentHost",
            &json!({ "environmentId": &environment, "hostName": &row.requested }),
        )
        .await;

        if row.same_host {
            assert_eq!(
                status, 200,
                "case {case:?}: a delete naming the stored host expected HTTP 200, got \
                 {status} ({}) {body}",
                error_type(&body)
            );
            assert_eq!(
                body["host"]["hostName"],
                json!(row.stored),
                "case {case:?}: the delete must report the stored spelling, got {}",
                body["host"]["hostName"]
            );
            assert_eq!(
                body["host"]["hostState"],
                json!("DELETING"),
                "case {case:?}: the named host must be going away, got {}",
                body["host"]["hostState"]
            );
        } else {
            // A name that differs by more than case names no host of this environment.
            assert_eq!(
                status, 400,
                "case {case:?}: a delete naming another host expected HTTP 400, got \
                 {status} ({}) {body}",
                error_type(&body)
            );
            assert_eq!(
                error_type(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected the host to be the missing resource, got {body}"
            );
            assert_eq!(
                body["resourceType"],
                json!("host"),
                "case {case:?}: expected the missing resource to be a host, got {body}"
            );
            // Nothing was written for it: the stored host is still CREATED.
            let (status, listed) = call(
                &router,
                "ListEnvironmentHosts",
                &json!({ "environmentId": &environment }),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: list expected 200, got {listed}");
            assert_eq!(
                listed["environmentHosts"][0]["hostState"],
                json!("CREATED"),
                "case {case:?}: a refused delete must leave the host untouched, got {listed}"
            );
        }
    }
}

#[tokio::test]
async fn a_freed_dns_hostname_retaken_in_another_case_leaves_one_host_record() {
    // A host that has reached DELETED is gone from VCF, so its DNS hostname is free — and
    // the host that takes it takes the NAME, not one spelling of it: the record kept for
    // reporting is replaced, so the environment holds one host per DNS hostname.
    let router = emulator();
    let environment = create(&router, Some("freed-name-env")).await;
    settle(&router);
    add_host(&router, "the first host", &environment, "esx-freed").await;
    settle(&router);

    let (status, body) = call(
        &router,
        "DeleteEnvironmentHost",
        &json!({ "environmentId": &environment, "hostName": "ESX-FREED" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    // While the host is still going away it OCCUPIES the environment, so its name is
    // taken in every spelling of it.
    let (status, body) = call(
        &router,
        "CreateEnvironmentHost",
        &host_request(&environment, "Esx-Freed"),
    )
    .await;
    assert_aggregated_refusal("while DELETING", status, &body, &["host.hostName"]);

    settle(&router);
    add_host(&router, "the replacement", &environment, "ESX-FREED").await;
    assert_eq!(
        listed_names(&router, "the replacement", &environment).await,
        vec!["ESX-FREED".to_string()],
        "one DNS hostname must leave one host record, under the spelling the create gave"
    );
}
