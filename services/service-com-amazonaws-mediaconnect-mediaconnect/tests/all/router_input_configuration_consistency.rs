// SPDX-License-Identifier: CC0-1.0

//! What a router input's `Configuration` may SAY ABOUT ITSELF, end to end over HTTP:
//! CreateRouterInput and UpdateRouterInput judge the configuration's own content before
//! storing it, and GetRouterInput reads back what was stored.
//!
//! Two rules the model states about a router input configuration:
//!
//! * a STANDARD configuration carries a REQUIRED `ProtocolConfiguration` whose union variant
//!   IS the protocol, beside an optional `Protocol` enum
//!   (`smithy_query(query="shape",shape="StandardRouterInputConfiguration")`), so the stored
//!   configuration reports the protocol that configuration configures and a request naming a
//!   different one is refused rather than stored self-contradicting;
//! * a FAILOVER or MERGE configuration lists "exactly two protocol configurations for the
//!   failover input sources. Both must use the same protocol type"
//!   (`smithy_query(query="shape",shape="FailoverRouterInputConfiguration")`, member
//!   `ProtocolConfigurations`, and the same sentence on
//!   `MergeRouterInputConfiguration$ProtocolConfigurations`), and neither list shape carries
//!   a length constraint, so nothing at the boundary enforces either half.
//!
//! The tables span what the rules generalise over: every member of every protocol
//! configuration union the input side declares, each paired with every value of the
//! `RouterInputProtocol` enum (its own and each other one), every list length around and far
//! from two, and every mixed pair the two list unions admit — on the create AND on the
//! update, whose refusal must leave the stored configuration exactly as it was.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use serde_json::{json, Value};

use crate::flow_lifecycle::Emulator;
use crate::router_input_restart::{
    create_body, create_router_input, get_router_input, union, union_member,
};
use crate::router_outputs::minted_network_interface;
use crate::update_router_input::update_router_input;

/// One protocol a router input can ingest on: the union member that configures it, and the
/// `RouterInputProtocol` value that member IS.
struct Protocol {
    /// The union member name and its `@jsonName`, which [`union`] keys the payload by both of.
    member_name: &'static str,
    json_name: &'static str,
    /// The `RouterInputProtocol` value this member configures.
    protocol: &'static str,
}

/// Every member of `RouterInputProtocolConfiguration`
/// (`smithy_query(query="shape",shape="RouterInputProtocolConfiguration")`: Rist |
/// SrtListener | SrtCaller | Rtp) beside the `RouterInputProtocol` value it configures
/// (`smithy_query(query="shape",shape="RouterInputProtocol")`: RTP | RIST | SRT_CALLER |
/// SRT_LISTENER) — the protocol catalog both rules below are asserted over, including the
/// members the issue cited none of.
const PROTOCOLS: [Protocol; 4] = [
    Protocol {
        member_name: "Rtp",
        json_name: "rtp",
        protocol: "RTP",
    },
    Protocol {
        member_name: "Rist",
        json_name: "rist",
        protocol: "RIST",
    },
    Protocol {
        member_name: "SrtCaller",
        json_name: "srtCaller",
        protocol: "SRT_CALLER",
    },
    Protocol {
        member_name: "SrtListener",
        json_name: "srtListener",
        protocol: "SRT_LISTENER",
    },
];

/// The members of `MergeRouterInputProtocolConfiguration`
/// (`smithy_query(query="shape",shape="MergeRouterInputProtocolConfiguration")`: Rtp | Rist)
/// — the merge list admits two of the four, so a merge row is built from these.
const MERGE_PROTOCOLS: [&str; 2] = ["RTP", "RIST"];

/// A protocol configuration of `protocol` on `port`, with every member the model makes
/// required for that union member
/// (`smithy_query(query="shape",shape="RtpRouterInputConfiguration")` and the three
/// siblings: `Port` 3000..30000, `RecoveryLatencyMilliseconds` / `MinimumLatencyMilliseconds`
/// 10..10000, `SourceAddress` and `SourcePort` 1024..65535).
fn protocol_configuration(protocol: &str, port: i64) -> Value {
    let entry = PROTOCOLS
        .iter()
        .find(|candidate| candidate.protocol == protocol)
        .unwrap_or_else(|| panic!("{protocol:?} is a RouterInputProtocol value"));
    let payload = match protocol {
        "RTP" => json!({ "port": port, "forwardErrorCorrection": "ENABLED" }),
        "RIST" => json!({ "port": port, "recoveryLatencyMilliseconds": 200 }),
        "SRT_CALLER" => json!({
            "sourceAddress": "198.51.100.20",
            "sourcePort": port,
            "minimumLatencyMilliseconds": 120,
        }),
        _ => json!({ "port": port, "minimumLatencyMilliseconds": 120 }),
    };
    union(entry.member_name, entry.json_name, payload)
}

/// A STANDARD configuration ingesting on `interface_arn` with a `protocol` protocol
/// configuration, naming `named_protocol` as its `Protocol` only where the case names one —
/// the member is optional.
fn standard(
    interface_arn: &str,
    protocol: &str,
    port: i64,
    named_protocol: Option<&str>,
) -> Value {
    let mut payload = json!({
        "networkInterfaceArn": interface_arn,
        "protocolConfiguration": protocol_configuration(protocol, port),
    });
    if let Some(named) = named_protocol {
        payload["protocol"] = json!(named);
    }
    union("Standard", "standard", payload)
}

/// A FAILOVER configuration ingesting on `interface_arn` whose `ProtocolConfigurations` list
/// the `protocols` entries, each on its own port.
fn failover(interface_arn: &str, protocols: &[&str]) -> Value {
    union(
        "Failover",
        "failover",
        json!({
            "networkInterfaceArn": interface_arn,
            "sourcePriorityMode": "PRIMARY_SECONDARY",
            "primarySourceIndex": 0,
            "protocolConfigurations": listed(protocols),
        }),
    )
}

/// A MERGE configuration ingesting on `interface_arn`, the merge counterpart of
/// [`failover`].
fn merge(interface_arn: &str, protocols: &[&str]) -> Value {
    union(
        "Merge",
        "merge",
        json!({
            "networkInterfaceArn": interface_arn,
            "mergeRecoveryWindowMilliseconds": 500,
            "protocolConfigurations": listed(protocols),
        }),
    )
}

/// The `ProtocolConfigurations` list for `protocols`, one entry per protocol, each on its own
/// port so two entries of one protocol are still two distinct sources.
fn listed(protocols: &[&str]) -> Value {
    Value::Array(
        protocols
            .iter()
            .enumerate()
            .map(|(offset, protocol)| {
                let port = 5000 + i64::try_from(offset).unwrap_or_default() * 2;
                protocol_configuration(protocol, port)
            })
            .collect(),
    )
}

/// The `BadRequestException` message of a refused answer.
fn refusal_message(answered: &Value) -> String {
    answered["message"].as_str().unwrap_or_default().to_string()
}

/// Create one router input named after `case` and answer `(status, error code, body)`.
async fn create(emulator: &Emulator, name: &str, configuration: Value) -> (u16, Option<String>, Value) {
    create_router_input(emulator, create_body(name, configuration)).await
}

/// Every pairing of a STANDARD configuration's `ProtocolConfiguration` variant with the
/// `Protocol` member: the member absent, the member naming the protocol that configuration
/// configures, and the member naming each of the other three.
///
/// The stored configuration must report the protocol the configuration CONFIGURES in all the
/// accepted rows — including the one that named none — and the contradicting rows must be
/// refused with `BadRequestException` (400), naming both protocols.
#[tokio::test]
async fn a_standard_configuration_reports_the_protocol_its_protocol_configuration_configures() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-standard-protocol").await;

    for (offset, configured) in PROTOCOLS.iter().enumerate() {
        let port = 6000 + i64::try_from(offset).unwrap_or_default() * 10;

        for named in [None, Some(configured.protocol)] {
            let case = format!(
                "{} protocol configuration with Protocol {named:?}",
                configured.protocol
            );
            let name = format!("ri-std-{}-{}", configured.json_name, named.unwrap_or("none"));
            let (status, code, answered) = create(
                &emulator,
                &name,
                standard(&interface_arn, configured.protocol, port, named),
            )
            .await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: CreateRouterInput expected 201, got {status} {code:?}: {answered}"
            );

            let arn = answered["routerInput"]["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (status, code, reported) = get_router_input(&emulator, &arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: GetRouterInput expected 200, got {status} {code:?}: {reported}"
            );

            // The stored configuration reports the protocol its protocol configuration
            // configures, whether or not the request named one, and the read answers the same
            // configuration the create stored.
            for (source, configuration) in [
                ("the create", &answered["routerInput"]["configuration"]),
                ("the read", &reported["routerInput"]["configuration"]),
            ] {
                let standard = union_member(configuration, "Standard", "standard");
                assert_eq!(
                    standard["protocol"],
                    json!(configured.protocol),
                    "case {case:?}: {source} reports Protocol {:?}, got {:?}",
                    configured.protocol,
                    standard["protocol"]
                );
                assert!(
                    !union_member(
                        &standard["protocolConfiguration"],
                        configured.member_name,
                        configured.json_name,
                    )
                    .is_null(),
                    "case {case:?}: {source} keeps the {} protocol configuration, got {:?}",
                    configured.member_name,
                    standard["protocolConfiguration"]
                );
            }
        }

        for contradicting in PROTOCOLS
            .iter()
            .filter(|candidate| candidate.protocol != configured.protocol)
        {
            let case = format!(
                "{} protocol configuration with Protocol {}",
                configured.protocol, contradicting.protocol
            );
            let name = format!(
                "ri-std-{}-vs-{}",
                configured.json_name, contradicting.json_name
            );
            let (status, code, answered) = create(
                &emulator,
                &name,
                standard(
                    &interface_arn,
                    configured.protocol,
                    port,
                    Some(contradicting.protocol),
                ),
            )
            .await;
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: CreateRouterInput expected 400 BadRequestException, got \
                 {status} {code:?}: {answered}"
            );
            let message = refusal_message(&answered);
            assert!(
                message.contains(configured.protocol) && message.contains(contradicting.protocol),
                "case {case:?}: the refusal names the configured and the named protocol, got \
                 {message:?}"
            );
        }
    }
}

/// One FAILOVER / MERGE list, and whether the service may store it.
struct PairCase {
    case: String,
    configuration: Value,
    /// The `RouterInputStreamDetails` union member an accepted input of this kind reports.
    details_member: (&'static str, &'static str),
    /// Whether the list is the pair of one protocol type the member documents.
    accepted: bool,
    /// A fragment the refusal must state, for a refused row.
    expected_detail: String,
}

/// Every list length around and far from two, and every mixed pair the two list unions
/// admit, for each of the two configurations that ingest a PAIR of sources.
fn pair_cases(interface_arn: &str) -> Vec<PairCase> {
    let failover_details = ("Failover", "failover");
    let merge_details = ("Merge", "merge");
    let mut cases = Vec::new();

    // Every length but two names no pair of sources. The lengths span the empty list, one
    // short of the pair, one past it and far past it.
    for count in [0usize, 1, 3, 5] {
        let protocols = vec!["RTP"; count];
        cases.push(PairCase {
            case: format!("a FAILOVER list of {count} protocol configurations"),
            configuration: failover(interface_arn, &protocols),
            details_member: failover_details,
            accepted: false,
            expected_detail: format!("but it lists {count}"),
        });
        cases.push(PairCase {
            case: format!("a MERGE list of {count} protocol configurations"),
            configuration: merge(interface_arn, &protocols),
            details_member: merge_details,
            accepted: false,
            expected_detail: format!("but it lists {count}"),
        });
    }

    // A pair of the SAME protocol type is the list the member documents — one row per member
    // each list union admits, so every protocol either kind of input can ingest is covered.
    for protocol in PROTOCOLS.iter().map(|entry| entry.protocol) {
        cases.push(PairCase {
            case: format!("a FAILOVER pair of {protocol} sources"),
            configuration: failover(interface_arn, &[protocol, protocol]),
            details_member: failover_details,
            accepted: true,
            expected_detail: String::new(),
        });
    }
    for protocol in MERGE_PROTOCOLS {
        cases.push(PairCase {
            case: format!("a MERGE pair of {protocol} sources"),
            configuration: merge(interface_arn, &[protocol, protocol]),
            details_member: merge_details,
            accepted: true,
            expected_detail: String::new(),
        });
    }

    // A pair of DIFFERENT protocol types uses no single protocol type: every unordered mixed
    // pair the failover list admits, and the one the merge list admits.
    for (offset, first) in PROTOCOLS.iter().enumerate() {
        for second in PROTOCOLS.iter().skip(offset + 1) {
            cases.push(PairCase {
                case: format!(
                    "a FAILOVER pair of a {} and a {} source",
                    first.protocol, second.protocol
                ),
                configuration: failover(interface_arn, &[first.protocol, second.protocol]),
                details_member: failover_details,
                accepted: false,
                expected_detail: "same protocol type".to_string(),
            });
        }
    }
    cases.push(PairCase {
        case: "a MERGE pair of an RTP and a RIST source".to_string(),
        configuration: merge(interface_arn, &["RTP", "RIST"]),
        details_member: merge_details,
        accepted: false,
        expected_detail: "same protocol type".to_string(),
    });

    cases
}

/// A FAILOVER or MERGE input ingests a PAIR of sources of one protocol type: a list of any
/// other length, or of two different protocol types, is refused with `BadRequestException`
/// (400) — and an accepted one reports one indexed stream details entry per LISTED source.
#[tokio::test]
async fn a_failover_or_merge_input_lists_exactly_two_sources_of_one_protocol_type() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-source-pair").await;

    for (offset, case) in pair_cases(&interface_arn).into_iter().enumerate() {
        let PairCase {
            case,
            configuration,
            details_member,
            accepted,
            expected_detail,
        } = case;
        let name = format!("ri-pair-{offset}");
        let (status, code, answered) = create(&emulator, &name, configuration).await;

        if !accepted {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: CreateRouterInput expected 400 BadRequestException, got \
                 {status} {code:?}: {answered}"
            );
            let message = refusal_message(&answered);
            assert!(
                message.contains(&expected_detail),
                "case {case:?}: the refusal states {expected_detail:?}, got {message:?}"
            );
            continue;
        }

        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: CreateRouterInput expected 201, got {status} {code:?}: {answered}"
        );
        let arn = answered["routerInput"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, code, reported) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: GetRouterInput expected 200, got {status} {code:?}: {reported}"
        );

        // "The index number (0 or 1) assigned to this source in the failover configuration"
        // (`FailoverRouterInputIndexedStreamDetails$SourceIndex`): one entry per source the
        // stored configuration lists, which an accepted list has two of.
        let details = union_member(
            &reported["routerInput"]["streamDetails"],
            details_member.0,
            details_member.1,
        );
        for (member, json_name, index) in [
            ("SourceIndexZeroStreamDetails", "sourceIndexZeroStreamDetails", 0),
            ("SourceIndexOneStreamDetails", "sourceIndexOneStreamDetails", 1),
        ] {
            let entry = union_member(details, member, json_name);
            assert_eq!(
                entry["sourceIndex"],
                json!(index),
                "case {case:?}: {json_name} reports SourceIndex {index}, got {:?}",
                entry["sourceIndex"]
            );
        }
    }
}

/// One replacement configuration an update carries, and what it must do.
struct UpdateCase {
    case: &'static str,
    /// The configuration the input is created with.
    created: Value,
    /// The configuration the update asks for.
    replacement: Value,
    /// Whether the replacement is one the service may store.
    accepted: bool,
    /// The `Protocol` the stored STANDARD configuration must report once the call is in, for
    /// a standard row.
    stored_protocol: Option<&'static str>,
}

/// The replacement configurations: the same two rules, on the operation that REPLACES a
/// stored configuration.
fn update_cases(interface_arn: &str) -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a STANDARD configuration whose Protocol contradicts its protocol \
                   configuration",
            created: standard(interface_arn, "RTP", 7000, Some("RTP")),
            replacement: standard(interface_arn, "RTP", 7002, Some("SRT_LISTENER")),
            accepted: false,
            stored_protocol: Some("RTP"),
        },
        UpdateCase {
            case: "a STANDARD configuration naming no Protocol at all",
            created: standard(interface_arn, "RTP", 7004, Some("RTP")),
            replacement: standard(interface_arn, "RIST", 7006, None),
            accepted: true,
            stored_protocol: Some("RIST"),
        },
        UpdateCase {
            case: "a STANDARD configuration naming the protocol it configures",
            created: standard(interface_arn, "RTP", 7008, None),
            replacement: standard(interface_arn, "SRT_CALLER", 7010, Some("SRT_CALLER")),
            accepted: true,
            stored_protocol: Some("SRT_CALLER"),
        },
        UpdateCase {
            case: "a FAILOVER list of one protocol configuration",
            created: failover(interface_arn, &["RTP", "RTP"]),
            replacement: failover(interface_arn, &["RTP"]),
            accepted: false,
            stored_protocol: None,
        },
        UpdateCase {
            case: "a FAILOVER pair of two different protocol types",
            created: failover(interface_arn, &["RTP", "RTP"]),
            replacement: failover(interface_arn, &["RTP", "SRT_CALLER"]),
            accepted: false,
            stored_protocol: None,
        },
        UpdateCase {
            case: "a FAILOVER pair of one protocol type",
            created: failover(interface_arn, &["RTP", "RTP"]),
            replacement: failover(interface_arn, &["SRT_LISTENER", "SRT_LISTENER"]),
            accepted: true,
            stored_protocol: None,
        },
        UpdateCase {
            case: "a MERGE pair of two different protocol types",
            created: merge(interface_arn, &["RTP", "RTP"]),
            replacement: merge(interface_arn, &["RTP", "RIST"]),
            accepted: false,
            stored_protocol: None,
        },
        UpdateCase {
            case: "a MERGE pair of one protocol type",
            created: merge(interface_arn, &["RTP", "RTP"]),
            replacement: merge(interface_arn, &["RIST", "RIST"]),
            accepted: true,
            stored_protocol: None,
        },
    ]
}

/// UpdateRouterInput stores a replacement configuration through the same rules the create
/// stores one through: a refused replacement leaves the stored configuration exactly as it
/// was, and an accepted one is what the read answers.
#[tokio::test]
async fn an_update_judges_a_replacement_configuration_by_the_same_rules() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-update-consistency").await;

    for (offset, update_case) in update_cases(&interface_arn).into_iter().enumerate() {
        let UpdateCase {
            case,
            created,
            replacement,
            accepted,
            stored_protocol,
        } = update_case;
        let name = format!("ri-update-consistency-{offset}");
        let (status, code, answered) = create(&emulator, &name, created).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: CreateRouterInput expected 201, got {status} {code:?}: {answered}"
        );
        let arn = answered["routerInput"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let stored_before = answered["routerInput"]["configuration"].clone();

        let (status, code, updated) =
            update_router_input(&emulator, &arn, json!({ "configuration": replacement })).await;
        let (status_after, code_after, reported) = get_router_input(&emulator, &arn).await;
        assert_eq!(
            (status_after, code_after.as_deref()),
            (200, None),
            "case {case:?}: GetRouterInput expected 200, got {status_after} {code_after:?}: \
             {reported}"
        );
        let stored_after = reported["routerInput"]["configuration"].clone();

        if !accepted {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: UpdateRouterInput expected 400 BadRequestException, got \
                 {status} {code:?}: {updated}"
            );
            assert_eq!(
                stored_after, stored_before,
                "case {case:?}: a refused update leaves the stored configuration as it was \
                 ({stored_before}), got {stored_after}"
            );
            continue;
        }

        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: UpdateRouterInput expected 202, got {status} {code:?}: {updated}"
        );
        assert_eq!(
            stored_after, updated["routerInput"]["configuration"],
            "case {case:?}: the read answers the configuration the update stored ({:?}), got \
             {stored_after}",
            updated["routerInput"]["configuration"]
        );
        if let Some(protocol) = stored_protocol {
            let standard = union_member(&stored_after, "Standard", "standard");
            assert_eq!(
                standard["protocol"],
                json!(protocol),
                "case {case:?}: the stored STANDARD configuration reports Protocol \
                 {protocol:?}, got {:?}",
                standard["protocol"]
            );
        }
    }
}
