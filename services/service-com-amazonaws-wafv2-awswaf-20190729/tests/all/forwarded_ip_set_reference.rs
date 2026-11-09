// SPDX-License-Identifier: CC0-1.0

//! Which address an `IPSetReferenceStatement` judges when it carries an
//! `IPSetForwardedIPConfig`.
//!
//! The rule: with a forwarded-IP configuration the statement judges the addresses the
//! configured `Position` names in the comma-separated list of the named header — `FIRST`
//! the list's first entry, `LAST` its last, `ANY` all of them ("if the header contains
//! more than 10 IP addresses, WAF inspects the last 10") — and not the address the
//! request came from. It matches when one of those addresses falls inside a CIDR range
//! of the referenced set, under the set's own `IPAddressVersion`; when the named
//! position holds no valid IP address — the header is absent, or the entry sitting there
//! is not an address — the `FallbackBehavior` is "the match status to assign to the web
//! request", so an address at ANOTHER position never stands in for it.
//!
//! The web ACLs and the IP sets are built over the wire through WAFv2's own control
//! plane, and the requests are driven through the data plane's own entry point
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]) on the SAME
//! shared engine — this service models no data-plane endpoint of its own, because the
//! requests a web ACL decides arrive at the protected resource's endpoint.
//!
//! The tables span the class rather than the one recorded request: the rows differ along
//! the position, where in the header the listed address sits, how many entries the
//! header carries, whether the entry at the position is an address at all, which header
//! is named, and which address family the set holds.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge::{self, InspectedRequest};

use crate::web_acl_lifecycle::{rule, visibility, Waf, TEST_REGION};

/// The `Scope` a web ACL in front of a regional resource lives in.
const SCOPE: &str = "REGIONAL";

/// The IPv4 ranges the referenced set holds: one single address and one wider prefix,
/// so a row can match a range rather than only an exact entry.
const IPV4_RANGES: [&str; 2] = ["198.51.100.7/32", "203.0.113.128/25"];

/// The IPv6 range the referenced set holds.
const IPV6_RANGES: [&str; 1] = ["2001:db8:1::/48"];

/// An address inside [`IPV4_RANGES`], and one outside every range of either set.
const LISTED: &str = "198.51.100.7";
const LISTED_IN_PREFIX: &str = "203.0.113.200";
const UNLISTED: &str = "203.0.113.9";

/// An address inside [`IPV6_RANGES`].
const LISTED_V6: &str = "2001:db8:1::25";

/// The one forwarded-IP configuration a case's web ACL carries, and the family of the
/// set its rule references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Config {
    /// `IPSet$IPAddressVersion` of the referenced set: `IPV4` or `IPV6`.
    family: &'static str,
    /// `IPSetForwardedIPConfig$HeaderName`.
    header: &'static str,
    /// `IPSetForwardedIPConfig$Position`.
    position: &'static str,
    /// `IPSetForwardedIPConfig$FallbackBehavior`.
    fallback: &'static str,
}

/// A stub protected resource: an API Gateway REST API stage, the one ARN form
/// `AssociateWebACLRequest$ResourceArn` documents for one. The stage belongs to another
/// service; what matters here is that the association is keyed by this ARN, so each
/// configuration gets its own.
fn stage_arn(index: usize) -> String {
    format!("arn:aws:apigateway:{TEST_REGION}::/restapis/a1b2c3d4e{index}/stages/prod")
}

/// Create the two referenced IP sets and answer `(IPv4 ARN, IPv6 ARN)`.
async fn ip_sets(waf: &Waf) -> (String, String) {
    let mut arns: Vec<String> = Vec::new();
    for (name, family, ranges) in [
        ("forwarded-v4", "IPV4", IPV4_RANGES.to_vec()),
        ("forwarded-v6", "IPV6", IPV6_RANGES.to_vec()),
    ] {
        let created = waf
            .ok(
                "CreateIPSet",
                json!({
                    "Name": name,
                    "Scope": SCOPE,
                    "IPAddressVersion": family,
                    "Addresses": ranges,
                }),
            )
            .await;
        arns.push(
            created["Summary"]["ARN"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateIPSet {name} must answer Summary.ARN"))
                .to_string(),
        );
    }
    let v6 = arns.pop().expect("the IPv6 set was created");
    let v4 = arns.pop().expect("the IPv4 set was created");
    (v4, v6)
}

/// The `IPSetReferenceStatement` `config` describes, referencing the set of its family.
fn statement(config: Config, v4_arn: &str, v6_arn: &str) -> Value {
    let arn = if config.family == "IPV6" { v6_arn } else { v4_arn };
    json!({
        "IPSetReferenceStatement": {
            "ARN": arn,
            "IPSetForwardedIPConfig": {
                "HeaderName": config.header,
                "FallbackBehavior": config.fallback,
                "Position": config.position,
            }
        }
    })
}

/// Create a web ACL whose single rule BLOCKS on `config` and put it in front of
/// [`stage_arn(index)`]; answers that stage ARN.
async fn in_front(waf: &Waf, config: Config, v4_arn: &str, v6_arn: &str, index: usize) -> String {
    let created = waf
        .ok(
            "CreateWebACL",
            json!({
                "Name": format!("forwarded-{index}"),
                "Scope": SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(&format!("forwarded{index}")),
                "Rules": [rule("forwarded", statement(config, v4_arn, v6_arn))],
            }),
        )
        .await;
    let arn = created["Summary"]["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWebACL {index} must answer Summary.ARN"))
        .to_string();
    let stage = stage_arn(index);
    waf.ok(
        "AssociateWebACL",
        json!({ "WebACLArn": arn, "ResourceArn": stage }),
    )
    .await;
    stage
}

/// One request a case drives, and what the web ACL in front of its stage must make of
/// it.
struct Case {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The configuration the deciding web ACL carries.
    config: Config,
    /// The forwarded headers the request carries, one entry per header instance.
    headers: Vec<(&'static str, String)>,
    /// The address the request came from, which a forwarded configuration never judges.
    client_ip: &'static str,
    /// Whether the rule's `Block` action must settle the request.
    blocked: bool,
}

/// Drive every case: one web ACL per distinct configuration, then one decision per row.
async fn assert_cases(cases: &[Case]) {
    let (waf, shared) = Waf::sharing_engine();
    let (v4_arn, v6_arn) = ip_sets(&waf).await;
    let mut fronts: Vec<(Config, String)> = Vec::new();
    for case in cases {
        if !fronts.iter().any(|(config, _)| *config == case.config) {
            let stage = in_front(&waf, case.config, &v4_arn, &v6_arn, fronts.len()).await;
            fronts.push((case.config, stage));
        }
    }

    for case in cases {
        let stage = fronts
            .iter()
            .find(|(config, _)| *config == case.config)
            .map(|(_, stage)| stage.clone())
            .expect("every case's configuration has a web ACL in front of a stage");
        let headers: Vec<(String, String)> = case
            .headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        let request = InspectedRequest {
            method: "GET".to_string(),
            uri_path: "/prod/orders".to_string(),
            query_string: String::new(),
            headers,
            body: Vec::new(),
            client_ip: case.client_ip.to_string(),
        };
        let decision = edge::decide(&*shared, &stage, &request).unwrap_or_else(|| {
            panic!(
                "case {:?}: the associated web ACL must decide the request",
                case.case
            )
        });
        let blocked = !decision.action.is_allow();
        assert_eq!(
            blocked, case.blocked,
            "case {:?}: expected blocked={}, got blocked={} from action {:?} with headers {:?}",
            case.case, case.blocked, blocked, decision.action, case.headers
        );
    }
}

/// A `NO_MATCH` configuration for `position` over the X-Forwarded-For header.
fn no_match_at(position: &'static str) -> Config {
    Config {
        family: "IPV4",
        header: "X-Forwarded-For",
        position,
        fallback: "NO_MATCH",
    }
}

/// One forwarded header instance carrying `value`.
fn xff(value: String) -> Vec<(&'static str, String)> {
    vec![("X-Forwarded-For", value)]
}

/// The rule over the POSITION: only the entry the position names is judged, the request
/// origin is not, and a header longer than the inspected bound is read from its end.
#[tokio::test]
async fn an_ip_set_reference_judges_only_the_address_at_the_configured_position() {
    // Eleven entries, the listed address FIRST: one more than the ten an `ANY` position
    // inspects, so the listed one sits outside them.
    let mut long_chain = vec![LISTED.to_string()];
    long_chain.extend((0..10).map(|octet| format!("192.0.2.{octet}")));
    // The same length with the listed address SECOND, which is inside the last ten.
    let mut long_chain_within = vec![UNLISTED.to_string(), LISTED.to_string()];
    long_chain_within.extend((0..9).map(|octet| format!("192.0.2.{octet}")));

    let cases = [
        Case {
            case: "FIRST: the listed address is the only entry",
            config: no_match_at("FIRST"),
            headers: xff(LISTED.to_string()),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "FIRST: the listed address heads a chain of proxies",
            config: no_match_at("FIRST"),
            headers: xff(format!("{LISTED}, {UNLISTED}")),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "FIRST: the listed address sits second in the chain",
            config: no_match_at("FIRST"),
            headers: xff(format!("{UNLISTED}, {LISTED}")),
            client_ip: UNLISTED,
            blocked: false,
        },
        Case {
            case: "FIRST: the first entry falls inside a wider range of the set",
            config: no_match_at("FIRST"),
            headers: xff(format!("{LISTED_IN_PREFIX}, {UNLISTED}")),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "FIRST: the request came FROM a listed address, the header names \
                   another",
            config: no_match_at("FIRST"),
            headers: xff(UNLISTED.to_string()),
            client_ip: LISTED,
            blocked: false,
        },
        Case {
            case: "FIRST: the entry at the position is not an address, a listed one \
                   follows it",
            config: no_match_at("FIRST"),
            headers: xff(format!("unknown, {LISTED}")),
            client_ip: UNLISTED,
            blocked: false,
        },
        Case {
            case: "FIRST: a trailing comma adds no entry",
            config: no_match_at("FIRST"),
            headers: xff(format!("{LISTED},")),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "LAST: the listed address closes the chain",
            config: no_match_at("LAST"),
            headers: xff(format!("{UNLISTED}, {LISTED}")),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "LAST: the listed address heads the chain",
            config: no_match_at("LAST"),
            headers: xff(format!("{LISTED}, {UNLISTED}")),
            client_ip: UNLISTED,
            blocked: false,
        },
        Case {
            case: "LAST: the final entry is not an address, a listed one precedes it",
            config: no_match_at("LAST"),
            headers: xff(format!("{LISTED}, unknown")),
            client_ip: UNLISTED,
            blocked: false,
        },
        Case {
            case: "ANY: the listed address sits in the middle of the chain",
            config: no_match_at("ANY"),
            headers: xff(format!("{UNLISTED}, {LISTED}, {UNLISTED}")),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "ANY: no entry of the chain is listed",
            config: no_match_at("ANY"),
            headers: xff(format!("{UNLISTED}, {UNLISTED}")),
            client_ip: LISTED,
            blocked: false,
        },
        Case {
            case: "ANY: the listed address sits one entry past the inspected ten",
            config: no_match_at("ANY"),
            headers: xff(long_chain.join(", ")),
            client_ip: UNLISTED,
            blocked: false,
        },
        Case {
            case: "ANY: the listed address sits inside the last inspected ten",
            config: no_match_at("ANY"),
            headers: xff(long_chain_within.join(", ")),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "ANY: a repeated header is one list, and the listed address sits in \
                   its second instance",
            config: no_match_at("ANY"),
            headers: vec![
                ("X-Forwarded-For", UNLISTED.to_string()),
                ("X-Forwarded-For", LISTED.to_string()),
            ],
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "a header other than X-Forwarded-For is the one inspected",
            config: Config {
                family: "IPV4",
                header: "True-Client-IP",
                position: "FIRST",
                fallback: "NO_MATCH",
            },
            headers: vec![("True-Client-IP", LISTED.to_string())],
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "the named header is absent and another forwarded header carries the \
                   listed address",
            config: Config {
                family: "IPV4",
                header: "True-Client-IP",
                position: "FIRST",
                fallback: "NO_MATCH",
            },
            headers: xff(LISTED.to_string()),
            client_ip: UNLISTED,
            blocked: false,
        },
        Case {
            case: "an IPv6 set judges an IPv6 address inside its prefix",
            config: Config {
                family: "IPV6",
                header: "X-Forwarded-For",
                position: "FIRST",
                fallback: "NO_MATCH",
            },
            headers: xff(format!("{LISTED_V6}, {UNLISTED}")),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "an IPv6 set holds no range an IPv4 address can fall in",
            config: Config {
                family: "IPV6",
                header: "X-Forwarded-For",
                position: "FIRST",
                fallback: "NO_MATCH",
            },
            headers: xff(LISTED.to_string()),
            client_ip: UNLISTED,
            blocked: false,
        },
    ];

    assert_cases(&cases).await;
}

/// The rule over the FALLBACK: it decides exactly the requests whose configured position
/// holds no valid IP address, and a valid address there is judged against the set
/// instead — whichever fallback is configured.
#[tokio::test]
async fn the_fallback_behavior_decides_a_position_holding_no_valid_address() {
    /// A `MATCH` configuration for `position` over the X-Forwarded-For header.
    fn match_at(position: &'static str) -> Config {
        Config {
            family: "IPV4",
            header: "X-Forwarded-For",
            position,
            fallback: "MATCH",
        }
    }

    let cases = [
        Case {
            case: "MATCH, FIRST: the request carries no such header",
            config: match_at("FIRST"),
            headers: Vec::new(),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "MATCH, FIRST: the entry at the position is not an address",
            config: match_at("FIRST"),
            headers: xff(format!("unknown, {UNLISTED}")),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "MATCH, FIRST: the position holds a valid address outside the set",
            config: match_at("FIRST"),
            headers: xff(UNLISTED.to_string()),
            client_ip: UNLISTED,
            blocked: false,
        },
        Case {
            case: "MATCH, FIRST: the position holds a listed address",
            config: match_at("FIRST"),
            headers: xff(LISTED.to_string()),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "MATCH, LAST: the final entry is not an address",
            config: match_at("LAST"),
            headers: xff(format!("{UNLISTED}, unknown")),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "MATCH, ANY: no entry of the chain is an address",
            config: match_at("ANY"),
            headers: xff("unknown, not-an-address".to_string()),
            client_ip: UNLISTED,
            blocked: true,
        },
        Case {
            case: "MATCH, ANY: one entry is an address, outside the set",
            config: match_at("ANY"),
            headers: xff(format!("unknown, {UNLISTED}")),
            client_ip: UNLISTED,
            blocked: false,
        },
        Case {
            case: "NO_MATCH, FIRST: the request carries no such header",
            config: no_match_at("FIRST"),
            headers: Vec::new(),
            client_ip: LISTED,
            blocked: false,
        },
        Case {
            case: "NO_MATCH, ANY: no entry of the chain is an address",
            config: no_match_at("ANY"),
            headers: xff("unknown, not-an-address".to_string()),
            client_ip: LISTED,
            blocked: false,
        },
    ];

    assert_cases(&cases).await;
}

/// `GetWebACL` answers the stored `IPSetForwardedIPConfig` exactly as it was written —
/// every position, every fallback, any header name — and answers no such member for a
/// reference statement that carried none.
#[tokio::test]
async fn get_web_acl_round_trips_the_forwarded_ip_configuration_as_written() {
    let waf = Waf::new();
    let (v4_arn, _) = ip_sets(&waf).await;

    let configs = [
        json!({
            "HeaderName": "X-Forwarded-For",
            "FallbackBehavior": "NO_MATCH",
            "Position": "FIRST",
        }),
        json!({
            "HeaderName": "True-Client-IP",
            "FallbackBehavior": "MATCH",
            "Position": "LAST",
        }),
        json!({
            "HeaderName": "CF-Connecting-IP",
            "FallbackBehavior": "NO_MATCH",
            "Position": "ANY",
        }),
    ];

    for (index, config) in configs.iter().enumerate() {
        let name = format!("round-trip-{index}");
        let reference = json!({
            "IPSetReferenceStatement": { "ARN": &v4_arn, "IPSetForwardedIPConfig": config }
        });
        let created = waf
            .ok(
                "CreateWebACL",
                json!({
                    "Name": &name,
                    "Scope": SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&format!("roundtrip{index}")),
                    "Rules": [rule("forwarded", reference.clone())],
                }),
            )
            .await;
        let id = created["Summary"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {config}: CreateWebACL must answer Summary.Id"))
            .to_string();
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &name, "Scope": SCOPE, "Id": id }),
            )
            .await;
        let stored =
            &got["WebACL"]["Rules"][0]["Statement"]["IPSetReferenceStatement"]
                ["IPSetForwardedIPConfig"];
        assert_eq!(
            stored, config,
            "case {config}: GetWebACL must answer the stored IPSetForwardedIPConfig, got {got}"
        );
    }

    // A reference statement that carried no forwarded configuration round-trips without
    // one: the member is optional, and nothing invents it.
    let created = waf
        .ok(
            "CreateWebACL",
            json!({
                "Name": "round-trip-plain",
                "Scope": SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("roundtripplain"),
                "Rules": [rule(
                    "plain",
                    json!({ "IPSetReferenceStatement": { "ARN": &v4_arn } }),
                )],
            }),
        )
        .await;
    let id = created["Summary"]["Id"]
        .as_str()
        .expect("CreateWebACL must answer Summary.Id")
        .to_string();
    let got = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "round-trip-plain", "Scope": SCOPE, "Id": id }),
        )
        .await;
    let statement = &got["WebACL"]["Rules"][0]["Statement"]["IPSetReferenceStatement"];
    assert_eq!(
        statement["IPSetForwardedIPConfig"],
        Value::Null,
        "case a reference statement with no forwarded configuration: GetWebACL must \
         answer none, got {statement}"
    );
}
