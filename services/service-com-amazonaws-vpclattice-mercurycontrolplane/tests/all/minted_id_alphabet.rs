// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FORM a minted resource id's suffix wears — which alphabet it
//! is drawn from and which character it begins with — and for every value that embeds
//! such an id.
//!
//! Every case drives the emulator the way a client does — a restJson1 request through the
//! router that mounts the service — and reads what it asserts back out of a LATER request
//! (`GetService`, `GetListener`, `GetServiceNetworkServiceAssociation`, the listing, the
//! delete acknowledgement), so an id minted on the create path and an id echoed on a read
//! path are both under test.
//!
//! The rule under test, asserted over a TABLE spanning every kind this service mints an
//! id for and several mints of each: a minted id is its kind's prefix plus a 17-character
//! suffix of the form `0` followed by 16 characters of the LOWERCASE HEX alphabet, and
//! that suffix travels unchanged into every value that embeds it — the resource ARNs
//! (including the ones that nest under a parent), the `resourceId` of a conflict, the ids
//! a read or a listing echoes, and the label of the generated DNS name.
//!
//! The modelled patterns are wider than that (`^svc-[0-9a-z]{17}$`,
//! `^listener-[0-9a-z]{17}$`, `^sn-[0-9a-z]{17}$`, `^snsa-[0-9a-z]{17}$`,
//! `^rule-[0-9a-z]{17}$`, `^tg-[0-9a-z]{17}$` —
//! `smithy_query(query="shape",shape="ServiceId,ListenerId,ServiceNetworkId,TargetGroupId,RuleId")`),
//! so this form satisfies each of them while a base-36 suffix, or a hex suffix with
//! another leading character, is a value the real service never hands out.

use crate::listeners::{create_listener, listener_kind};
use crate::rules::{create_rule, fixed_response_rule, rule_kind};
use crate::target_groups::create_target_group;
use crate::vpclattice_service_slice::{expected_arn, Answer, Lattice, REGION};
use service_com_amazonaws_vpclattice_mercurycontrolplane::state;

/// How many characters a minted id carries after its prefix, as every modelled id pattern
/// of this service spells it (`[0-9a-z]{17}`).
const ID_SUFFIX_LEN: usize = 17;

/// How many mints of one kind each row of the table asks for: one id could be hex by luck,
/// a run of them cannot.
const MINTS_PER_KIND: usize = 4;

/// Every id prefix this service mints an id under (`state::PREFIX_*`).
const ID_PREFIXES: &[&str] = &[
    state::PREFIX_SERVICE_NETWORK,
    state::PREFIX_SERVICE,
    state::PREFIX_SERVICE_ASSOCIATION,
    state::PREFIX_LISTENER,
    state::PREFIX_RULE,
    state::PREFIX_TARGET_GROUP,
    state::PREFIX_RESOURCE_CONFIGURATION,
];

/// Whether `suffix` is the 17 characters a minted id wears: the fixed leading `0` and 16
/// lowercase-hex digits.
fn is_hex_suffix(suffix: &str) -> bool {
    suffix.len() == ID_SUFFIX_LEN
        && suffix.starts_with('0')
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Assert `id` is `prefix` plus that suffix.
fn assert_hex_id(case: &str, id: &str, prefix: &str) {
    let suffix = id
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("case {case}: expected an id beginning {prefix:?}, got {id:?}"));
    assert!(
        is_hex_suffix(suffix),
        "case {case}: expected {prefix} plus `0` and 16 characters from [0-9a-f], \
         got {id:?} (suffix {suffix:?})"
    );
}

/// Assert that every id EMBEDDED anywhere in a response — bare, inside an ARN, or as the
/// label of a DNS name — wears that suffix. This walks the whole body, so a member no row
/// names explicitly is covered too.
fn assert_embedded_ids_are_hex(case: &str, value: &serde_json::Value) {
    match value {
        serde_json::Value::String(text) => {
            for prefix in ID_PREFIXES {
                let mut rest = text.as_str();
                while let Some(at) = rest.find(prefix) {
                    let tail = &rest[at + prefix.len()..];
                    let suffix: String = tail
                        .chars()
                        .take_while(char::is_ascii_alphanumeric)
                        .collect();
                    assert!(
                        is_hex_suffix(&suffix),
                        "case {case}: {text:?} embeds a {prefix} id whose suffix is \
                         {suffix:?}, expected `0` and 16 characters from [0-9a-f]"
                    );
                    rest = tail;
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                assert_embedded_ids_are_hex(case, item);
            }
        }
        serde_json::Value::Object(members) => {
            for member in members.values() {
                assert_embedded_ids_are_hex(case, member);
            }
        }
        _ => {}
    }
}

/// The answer of a call that must have been served, with every id it embeds checked.
fn served(case: &str, expected_status: u16, answer: &Answer) {
    assert_eq!(
        answer.status, expected_status,
        "case {case}: expected {expected_status}, got {} / {}",
        answer.status, answer.body
    );
    assert_embedded_ids_are_hex(case, &answer.body);
}

/// The `dnsEntry.domainName` of a response.
fn domain_name_of(case: &str, answer: &Answer) -> String {
    answer.body["dnsEntry"]["domainName"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a dnsEntry.domainName, got {}",
                answer.body
            )
        })
        .to_string()
}

/// Take `domain_name` apart against the template VPC Lattice hands out —
/// `<service name>-<the service id's suffix>.<account segment>.vpc-lattice-svcs.<region>.on.aws`
/// — and answer the account segment, which is 7 lowercase-hex characters.
fn account_segment_of(case: &str, name: &str, service_id: &str, domain_name: &str) -> String {
    let suffix = service_id
        .strip_prefix(state::PREFIX_SERVICE)
        .unwrap_or_else(|| panic!("case {case}: expected a svc- id, got {service_id:?}"));
    let head = format!("{name}-{suffix}.");
    let rest = domain_name.strip_prefix(&head).unwrap_or_else(|| {
        panic!("case {case}: expected a domain name beginning {head:?}, got {domain_name:?}")
    });
    let (segment, tail) = rest.split_once('.').unwrap_or_else(|| {
        panic!("case {case}: expected an account segment in {domain_name:?}")
    });
    assert_eq!(
        tail,
        format!("vpc-lattice-svcs.{REGION}.on.aws"),
        "case {case}: expected the lattice service domain of the request's region, \
         got {domain_name:?}"
    );
    // 7 hex characters: the segment AWS derives from the account, which every service of
    // one account shares.
    assert!(
        segment.len() == 7
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "case {case}: expected a 7-character hexadecimal account segment, got {segment:?} \
         in {domain_name:?}"
    );
    segment.to_string()
}

/// Every kind this service mints an id for, several mints each: the id wears AWS's suffix
/// form and the ARN the same call answers embeds exactly that id.
#[tokio::test]
async fn every_minted_id_and_arn_carries_a_lowercase_hex_suffix() {
    let lattice = Lattice::new();

    // A service network and a service to hang the nested kinds off, and a second service
    // to associate.
    let mut network_ids: Vec<String> = Vec::new();
    for mint in 0..MINTS_PER_KIND {
        let case = format!("service network mint {mint}");
        let created = lattice
            .create_service_network(serde_json::json!({ "name": format!("hex-net-{mint}") }))
            .await;
        served(&case, 201, &created);
        let id = created.str_field("id");
        assert_hex_id(&case, &id, state::PREFIX_SERVICE_NETWORK);
        assert_eq!(
            created.str_field("arn"),
            expected_arn(state::KIND_SERVICE_NETWORK, &id),
            "case {case}: the ARN must embed the minted id {id:?}, got {}",
            created.body
        );
        network_ids.push(id);
    }

    let mut service_ids: Vec<String> = Vec::new();
    for mint in 0..MINTS_PER_KIND {
        let case = format!("service mint {mint}");
        let created = lattice
            .create_service(serde_json::json!({ "name": format!("hex-host-{mint}") }))
            .await;
        served(&case, 201, &created);
        let id = created.str_field("id");
        assert_hex_id(&case, &id, state::PREFIX_SERVICE);
        assert_eq!(
            created.str_field("arn"),
            expected_arn(state::KIND_SERVICE, &id),
            "case {case}: the ARN must embed the minted id {id:?}, got {}",
            created.body
        );
        service_ids.push(id);
    }

    let host = &service_ids[0];
    let mut listener_ids: Vec<String> = Vec::new();
    for mint in 0..MINTS_PER_KIND {
        let case = format!("listener mint {mint}");
        let created = create_listener(
            &lattice,
            host,
            serde_json::json!({
                "name": format!("hex-front-{mint}"),
                "protocol": "HTTP",
                // A listener's port is unique within its service, so each mint takes its
                // own.
                "port": 30_000 + mint as i64,
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            }),
        )
        .await;
        served(&case, 201, &created);
        let id = created.str_field("id");
        assert_hex_id(&case, &id, state::PREFIX_LISTENER);
        assert_eq!(
            created.str_field("arn"),
            expected_arn(&listener_kind(host), &id),
            "case {case}: the listener ARN must nest under its service and embed {id:?}, \
             got {}",
            created.body
        );
        listener_ids.push(id);
    }

    let listener = &listener_ids[0];
    for mint in 0..MINTS_PER_KIND {
        let case = format!("rule mint {mint}");
        let created = create_rule(
            &lattice,
            host,
            listener,
            fixed_response_rule(&format!("hex-route-{mint}"), mint as i64 + 1),
        )
        .await;
        served(&case, 201, &created);
        let id = created.str_field("id");
        assert_hex_id(&case, &id, state::PREFIX_RULE);
        assert_eq!(
            created.str_field("arn"),
            expected_arn(&rule_kind(host, listener), &id),
            "case {case}: the rule ARN must nest under its listener and embed {id:?}, got {}",
            created.body
        );
    }

    for mint in 0..MINTS_PER_KIND {
        let case = format!("target group mint {mint}");
        let created = create_target_group(
            &lattice,
            serde_json::json!({ "name": format!("hex-pool-{mint}"), "type": "LAMBDA" }),
        )
        .await;
        served(&case, 201, &created);
        let id = created.str_field("id");
        assert_hex_id(&case, &id, state::PREFIX_TARGET_GROUP);
        assert_eq!(
            created.str_field("arn"),
            expected_arn(state::KIND_TARGET_GROUP, &id),
            "case {case}: the ARN must embed the minted id {id:?}, got {}",
            created.body
        );
    }

    // One association per (network, service) pair the table built: a pair may be
    // associated only once, so each mint takes a fresh pair.
    for mint in 0..MINTS_PER_KIND {
        let case = format!("association mint {mint}");
        let created = lattice
            .associate(&service_ids[mint], &network_ids[mint])
            .await;
        served(&case, 200, &created);
        let id = created.str_field("id");
        assert_hex_id(&case, &id, state::PREFIX_SERVICE_ASSOCIATION);
        assert_eq!(
            created.str_field("arn"),
            expected_arn(state::KIND_SERVICE_ASSOCIATION, &id),
            "case {case}: the ARN must embed the minted id {id:?}, got {}",
            created.body
        );
    }
}

/// The minted suffix travels: a read, a listing, a delete acknowledgement and a conflict
/// all carry the same minted ids, and the generated DNS name splices the service id's
/// suffix into its first label.
#[tokio::test]
async fn the_minted_suffix_travels_into_reads_listings_deletes_and_conflicts() {
    let lattice = Lattice::new();
    let network = {
        let created = lattice
            .create_service_network(serde_json::json!({ "name": "travel-net" }))
            .await;
        served("the fixture service network", 201, &created);
        created.str_field("id")
    };

    // Rows differing in the service name the domain name is built from: the shortest the
    // `ServiceName` pattern admits, one with digits, one with inner hyphens, and one at
    // the length a probe uses.
    let names = [
        "abc",
        "travel7",
        "travel-host-two",
        "travel-probe-host-00000000",
    ];
    let mut segments: Vec<String> = Vec::new();
    let mut created_services: Vec<(String, String, String)> = Vec::new();
    for name in names {
        let case = format!("service {name:?}");
        let created = lattice
            .create_service(serde_json::json!({ "name": name }))
            .await;
        served(&case, 201, &created);
        let id = created.str_field("id");
        assert_hex_id(&case, &id, state::PREFIX_SERVICE);
        let domain_name = domain_name_of(&case, &created);
        segments.push(account_segment_of(&case, name, &id, &domain_name));

        // The read path echoes exactly what the create minted.
        let read = lattice.get_service(&id).await;
        served(&format!("{case} read back"), 200, &read);
        assert_eq!(
            read.str_field("arn"),
            created.str_field("arn"),
            "case {case}: GetService must echo the stored ARN, got {}",
            read.body
        );
        assert_eq!(
            domain_name_of(&format!("{case} read back"), &read),
            domain_name,
            "case {case}: GetService must echo the stored domain name, got {}",
            read.body
        );
        created_services.push((name.to_string(), id, domain_name));
    }
    // The account segment is one account's, so every service of this emulator shares it.
    for (index, segment) in segments.iter().enumerate() {
        assert_eq!(
            segment, &segments[0],
            "case service {:?}: expected the account segment {:?} every other service \
             carries, got {segment:?}",
            names[index], segments[0]
        );
    }

    // A listener on the first service, read back, then a duplicate name so the conflict
    // reports the stored listener's id.
    let (_, host, _) = &created_services[0];
    let listener = {
        let created = create_listener(
            &lattice,
            host,
            serde_json::json!({
                "name": "travel-front",
                "protocol": "HTTP",
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            }),
        )
        .await;
        served("the listener", 201, &created);
        let id = created.str_field("id");
        assert_hex_id("the listener", &id, state::PREFIX_LISTENER);
        assert_eq!(
            created.str_field("serviceArn"),
            expected_arn(state::KIND_SERVICE, host),
            "case the listener: expected the parent service's ARN, got {}",
            created.body
        );
        id
    };
    let read = lattice
        .call(
            "GET",
            &format!("/services/{host}/listeners/{listener}"),
            None,
        )
        .await;
    served("the listener read back", 200, &read);
    assert_eq!(
        read.str_field("arn"),
        expected_arn(&listener_kind(host), &listener),
        "case the listener read back: expected the nested listener ARN, got {}",
        read.body
    );
    assert_eq!(
        read.str_field("serviceArn"),
        expected_arn(state::KIND_SERVICE, host),
        "case the listener read back: expected the parent service's ARN, got {}",
        read.body
    );

    let conflict = create_listener(
        &lattice,
        host,
        serde_json::json!({
            "name": "travel-front",
            "protocol": "HTTP",
            "port": 8080,
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    assert_eq!(
        conflict.status, 409,
        "case the duplicate listener name: expected 409, got {} / {}",
        conflict.status, conflict.body
    );
    assert_embedded_ids_are_hex("the duplicate listener name", &conflict.body);
    assert_eq!(
        conflict.str_field("resourceId"),
        listener,
        "case the duplicate listener name: the conflict must name the stored listener, \
         got {}",
        conflict.body
    );

    // The association of the second service: create, read, list and delete all carry the
    // same minted ids, and the association reports the service's own domain name.
    let (_, associated, associated_domain) = &created_services[1];
    let created = lattice.associate(associated, &network).await;
    served("the association", 200, &created);
    let association = created.str_field("id");
    assert_hex_id("the association", &association, state::PREFIX_SERVICE_ASSOCIATION);
    assert_eq!(
        domain_name_of("the association", &created),
        *associated_domain,
        "case the association: expected the associated service's domain name, got {}",
        created.body
    );

    let read = lattice
        .call(
            "GET",
            &format!("/servicenetworkserviceassociations/{association}"),
            None,
        )
        .await;
    served("the association read back", 200, &read);
    for (member, expected) in [
        ("id", association.clone()),
        (
            "arn",
            expected_arn(state::KIND_SERVICE_ASSOCIATION, &association),
        ),
        ("serviceId", associated.clone()),
        ("serviceArn", expected_arn(state::KIND_SERVICE, associated)),
        ("serviceNetworkId", network.clone()),
        (
            "serviceNetworkArn",
            expected_arn(state::KIND_SERVICE_NETWORK, &network),
        ),
    ] {
        assert_eq!(
            read.str_field(member),
            expected,
            "case the association read back, member {member}: expected {expected:?}, got {}",
            read.body
        );
    }
    assert_eq!(
        domain_name_of("the association read back", &read),
        *associated_domain,
        "case the association read back: expected the service's domain name, got {}",
        read.body
    );

    let listed = lattice
        .call(
            "GET",
            &format!("/servicenetworkserviceassociations?serviceNetworkIdentifier={network}"),
            None,
        )
        .await;
    served("the association listing", 200, &listed);
    let items = listed.body["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        items.len(),
        1,
        "case the association listing: expected the one persisted association, got {}",
        listed.body
    );
    for (member, expected) in [
        ("id", association.clone()),
        (
            "arn",
            expected_arn(state::KIND_SERVICE_ASSOCIATION, &association),
        ),
        ("serviceId", associated.clone()),
        ("serviceArn", expected_arn(state::KIND_SERVICE, associated)),
        ("serviceNetworkId", network.clone()),
        (
            "serviceNetworkArn",
            expected_arn(state::KIND_SERVICE_NETWORK, &network),
        ),
    ] {
        assert_eq!(
            items[0][member].as_str(),
            Some(expected.as_str()),
            "case the association listing, member {member}: expected {expected:?}, got {}",
            items[0]
        );
    }
    assert_eq!(
        items[0]["dnsEntry"]["domainName"].as_str(),
        Some(associated_domain.as_str()),
        "case the association listing: expected the service's domain name, got {}",
        items[0]
    );

    let deleted = lattice
        .call(
            "DELETE",
            &format!("/servicenetworkserviceassociations/{association}"),
            None,
        )
        .await;
    served("the association delete", 200, &deleted);
    assert_eq!(
        deleted.str_field("id"),
        association,
        "case the association delete: expected the association's id, got {}",
        deleted.body
    );
    assert_eq!(
        deleted.str_field("arn"),
        expected_arn(state::KIND_SERVICE_ASSOCIATION, &association),
        "case the association delete: expected the association's ARN, got {}",
        deleted.body
    );

    // The delete acknowledgement of a service echoes its ARN with the same suffix.
    let (_, doomed, _) = &created_services[2];
    let deleted = lattice
        .call("DELETE", &format!("/services/{doomed}"), None)
        .await;
    served("the service delete", 200, &deleted);
    assert_eq!(
        deleted.str_field("arn"),
        expected_arn(state::KIND_SERVICE, doomed),
        "case the service delete: expected the deleted service's ARN, got {}",
        deleted.body
    );
}
