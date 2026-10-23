// SPDX-License-Identifier: CC0-1.0

//! The Lightsail instance firewall — open, close, replace and read an instance's
//! public ports — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: an instance's firewall is a PERSISTED set of inbound
//! rules, each identified by its port range and its protocol — so
//! `OpenInstancePublicPorts` upserts the ONE rule its `portInfo` names (replacing
//! the addresses allowed through a range that is already open and leaving every
//! other rule alone), `CloseInstancePublicPorts` removes the ONE rule its port
//! range and protocol name (and nothing when the firewall holds no such rule),
//! `PutInstancePublicPorts` replaces the WHOLE set with the rules the request
//! carries, and every later read of the instance — `GetInstancePortStates` and
//! `GetInstance`'s `networking.ports` — reports exactly the rules the store holds,
//! each `open`. A request the service cannot accept is refused with the modelled
//! `InvalidInputException` and leaves the firewall as it was; a name the account
//! does not hold is refused with the service's `DoesNotExist` envelope.

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    MAX_PORT_CIDRS, MAX_PORT_INFOS_PER_REQUEST, MAX_PORT_RULES,
};

// The harness, the wire error code and the minted-id check this test target already
// exposes: a sibling suite drives the service through the same front door rather than
// standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The zone every instance in this suite is created in; the firewall is not
/// regional, so one zone serves every case.
const ZONE: &str = "us-east-1a";

/// A Linux/Unix blueprint, whose default firewall opens SSH (22) and HTTP (80),
/// and the smallest plan: neither selects anything about a firewall rule.
const BLUEPRINT: &str = "amazon_linux_2023";
/// See [`BLUEPRINT`].
const BUNDLE: &str = "nano_3_0";

/// One rule of an instance's firewall, as a read reports it: the port range and
/// protocol that identify it, and the addresses allowed through it.
///
/// Ordered by exactly the key the store keeps its rules in — port range, then
/// protocol — so a sorted set of these is the order a read must report.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Rule {
    from_port: i64,
    to_port: i64,
    protocol: String,
    cidrs: Vec<String>,
    ipv6_cidrs: Vec<String>,
    cidr_list_aliases: Vec<String>,
}

impl Rule {
    /// A rule open to `cidrs` and `ipv6_cidrs` over `protocol`.
    fn new(
        from_port: i64,
        to_port: i64,
        protocol: &str,
        cidrs: &[&str],
        ipv6_cidrs: &[&str],
    ) -> Self {
        Self {
            from_port,
            to_port,
            protocol: protocol.to_string(),
            cidrs: cidrs.iter().map(|cidr| (*cidr).to_string()).collect(),
            ipv6_cidrs: ipv6_cidrs.iter().map(|cidr| (*cidr).to_string()).collect(),
            cidr_list_aliases: Vec::new(),
        }
    }

    /// The same rule, reached through the Lightsail console's RDP/SSH client
    /// alias instead of (or beside) named addresses.
    fn with_aliases(mut self, aliases: &[&str]) -> Self {
        self.cidr_list_aliases = aliases.iter().map(|alias| (*alias).to_string()).collect();
        self
    }

    /// The key that identifies this rule within one firewall.
    fn key(&self) -> (i64, i64, &str) {
        (self.from_port, self.to_port, self.protocol.as_str())
    }
}

/// The rule `value` describes, read out of an `InstancePortState` or an
/// `InstancePortInfo` — the two views share every member this rule is made of.
fn rule_of(value: &Value) -> Rule {
    Rule {
        from_port: value["fromPort"].as_i64().unwrap_or_default(),
        to_port: value["toPort"].as_i64().unwrap_or_default(),
        protocol: value["protocol"].as_str().unwrap_or_default().to_string(),
        cidrs: strings(&value["cidrs"]),
        ipv6_cidrs: strings(&value["ipv6Cidrs"]),
        cidr_list_aliases: strings(&value["cidrListAliases"]),
    }
}

/// The strings `value` holds, empty when the member is absent: an absent address
/// list and an empty one say the same thing about what a rule admits.
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Upsert `rule` into `rules`, the way a firewall holds one rule per port range
/// and protocol.
fn upsert(rules: &mut Vec<Rule>, rule: Rule) {
    match rules.iter_mut().find(|held| held.key() == rule.key()) {
        Some(held) => *held = rule,
        None => rules.push(rule),
    }
    rules.sort();
}

/// The rules Lightsail opens on a new Linux/Unix instance whose addresses are
/// `ip_address_type`'s: SSH and HTTP, each open to every address the instance
/// has.
fn default_rules(ip_address_type: &str) -> Vec<Rule> {
    let cidrs: &[&str] = if matches!(ip_address_type, "ipv4" | "dualstack") {
        &["0.0.0.0/0"]
    } else {
        &[]
    };
    let ipv6_cidrs: &[&str] = if matches!(ip_address_type, "ipv6" | "dualstack") {
        &["::/0"]
    } else {
        &[]
    };
    let mut rules = vec![
        Rule::new(22, 22, "tcp", cidrs, ipv6_cidrs),
        Rule::new(80, 80, "tcp", cidrs, ipv6_cidrs),
    ];
    rules.sort();
    rules
}

/// Create the instance `name` with `ip_address_type` (the modelled default when
/// none is named), answering nothing: every case reads the firewall back.
async fn create_instance(emulator: &Emulator, name: &str, ip_address_type: Option<&str>) {
    let mut body = json!({
        "instanceNames": [name],
        "availabilityZone": ZONE,
        "blueprintId": BLUEPRINT,
        "bundleId": BUNDLE,
    });
    if let Some(ip_address_type) = ip_address_type {
        body["ipAddressType"] = json!(ip_address_type);
    }
    emulator.ok("CreateInstances", body).await;
}

/// The `portStates` `GetInstancePortStates` reports for `name`.
async fn port_states(emulator: &Emulator, name: &str) -> Vec<Value> {
    let body = emulator
        .ok("GetInstancePortStates", json!({"instanceName": name}))
        .await;
    body["portStates"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name}: expected a portStates array, got {body:?}"))
        .clone()
}

/// The firewall of `name`, as `GetInstancePortStates` reports it.
async fn firewall(emulator: &Emulator, name: &str) -> Vec<Rule> {
    port_states(emulator, name).await.iter().map(rule_of).collect()
}

/// The `networking.ports` `GetInstance` reports for `name`.
async fn networking_ports(emulator: &Emulator, name: &str) -> Vec<Value> {
    let instance = emulator
        .ok("GetInstance", json!({"instanceName": name}))
        .await["instance"]
        .clone();
    instance["networking"]["ports"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name}: expected a ports array, got {instance:?}"))
        .clone()
}

/// Assert that `name`'s firewall is exactly `want`, through BOTH reads, and that
/// each read reports its rules in the store's one stable order.
async fn assert_firewall(emulator: &Emulator, case: &str, name: &str, want: &[Rule]) {
    let states = port_states(emulator, name).await;
    let reported: Vec<Rule> = states.iter().map(rule_of).collect();
    assert_eq!(
        reported, want,
        "case {case}: expected GetInstancePortStates to report {want:?}, got {reported:?}"
    );
    for state in &states {
        assert_eq!(
            state["state"].as_str(),
            Some("open"),
            "case {case}: every rule a firewall holds is open, got {state:?}"
        );
    }

    let ports = networking_ports(emulator, name).await;
    let through_instance: Vec<Rule> = ports.iter().map(rule_of).collect();
    assert_eq!(
        through_instance, want,
        "case {case}: expected GetInstance's networking.ports to report the same rules \
         {want:?}, got {through_instance:?}"
    );
    for port in &ports {
        // The modelled `InstancePortInfo` members a rule's own addresses decide.
        let rule = rule_of(port);
        let anywhere_ipv4 = rule.cidrs.iter().any(|cidr| cidr == "0.0.0.0/0");
        let anywhere_ipv6 = rule.ipv6_cidrs.iter().any(|cidr| cidr == "::/0");
        let want_access_from = if anywhere_ipv4 {
            "Anywhere (0.0.0.0/0)"
        } else if anywhere_ipv6 {
            "Anywhere (::/0)"
        } else {
            "Custom"
        };
        assert_eq!(
            port["accessFrom"].as_str(),
            Some(want_access_from),
            "case {case}: expected accessFrom {want_access_from} for {rule:?}, got {port:?}"
        );
        assert_eq!(
            port["accessType"].as_str(),
            Some("Public"),
            "case {case}: the public ports are Public access, got {port:?}"
        );
        assert_eq!(
            port["accessDirection"].as_str(),
            Some("inbound"),
            "case {case}: Lightsail supports only inbound rules, got {port:?}"
        );
    }
}

/// Assert everything the rule says about the `operation` a firewall call answers
/// with: it names the instance it changed, it is the modelled operation type, and
/// the change is already done.
fn assert_operation(case: &str, operation: &Value, operation_type: &str, name: &str) {
    assert_eq!(
        operation["operationType"].as_str(),
        Some(operation_type),
        "case {case}: expected the {operation_type} operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(name),
        "case {case}: expected the operation to name {name}, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("Instance"),
        "case {case}: a firewall belongs to an instance, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case {case}: the firewall change is done when the call answers, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: a finished operation is terminal, got {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {operation:?}"
    );
    assert!(
        operation["createdAt"].is_number(),
        "case {case}: expected a creation timestamp, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["availabilityZone"].as_str(),
        Some(ZONE),
        "case {case}: expected the instance's own location, got {operation:?}"
    );
}

// ---------------------------------------------------------------------------
// The open table
// ---------------------------------------------------------------------------

/// One `OpenInstancePublicPorts` request and the rule the firewall must then
/// hold. The rows differ along everything the rule generalises over: the
/// protocol (every modelled `NetworkProtocol` value), a single port against a
/// range, how the addresses are written (none at all, a bare address, a /32, a
/// block, several, both families), the console alias, and the instance's own
/// address types.
struct OpenRow {
    case: &'static str,
    /// What the instance is created with, or `None` to send none (the modelled
    /// `dualstack` default).
    ip_address_type: Option<&'static str>,
    /// The effective address type, which decides the families a rule that names
    /// no address at all is opened to.
    effective_ip_address_type: &'static str,
    /// The `portInfo` the request carries, verbatim.
    port_info: Value,
    /// The rule the firewall must hold afterwards.
    want: Rule,
    /// The LITERAL `accessFrom` the instance's `networking.ports` must report for
    /// that rule, pinned per row rather than recomputed from the answer.
    want_access_from: &'static str,
}

/// The table.
///
/// The ICMP rows are the examples the modelled `PortInfo$fromPort` /
/// `PortInfo$toPort` documentation gives — "specify `8` as the `fromPort` (ICMP
/// type), and `-1` as the `toPort` (ICMP code), to enable ICMP Ping", and
/// `128`/`0` for ICMPv6 — and the alias row is the one alias
/// `PortInfo$cidrListAliases` publishes.
fn open_rows() -> Vec<OpenRow> {
    vec![
        OpenRow {
            case: "one tcp port, no address named, dualstack instance",
            ip_address_type: None,
            effective_ip_address_type: "dualstack",
            port_info: json!({"fromPort": 8080, "toPort": 8080, "protocol": "tcp"}),
            want: Rule::new(8080, 8080, "tcp", &["0.0.0.0/0"], &["::/0"]),
            want_access_from: "Anywhere (0.0.0.0/0)",
        },
        OpenRow {
            case: "one tcp port, no address named, ipv4-only instance",
            ip_address_type: Some("ipv4"),
            effective_ip_address_type: "ipv4",
            port_info: json!({"fromPort": 8080, "toPort": 8080, "protocol": "tcp"}),
            want: Rule::new(8080, 8080, "tcp", &["0.0.0.0/0"], &[]),
            want_access_from: "Anywhere (0.0.0.0/0)",
        },
        OpenRow {
            case: "one tcp port, no address named, ipv6-only instance",
            ip_address_type: Some("ipv6"),
            effective_ip_address_type: "ipv6",
            port_info: json!({"fromPort": 8080, "toPort": 8080, "protocol": "tcp"}),
            want: Rule::new(8080, 8080, "tcp", &[], &["::/0"]),
            want_access_from: "Anywhere (::/0)",
        },
        OpenRow {
            case: "no protocol named at all",
            ip_address_type: None,
            effective_ip_address_type: "dualstack",
            port_info: json!({"fromPort": 9090, "toPort": 9090}),
            want: Rule::new(9090, 9090, "tcp", &["0.0.0.0/0"], &["::/0"]),
            want_access_from: "Anywhere (0.0.0.0/0)",
        },
        OpenRow {
            case: "a udp port range, one IPv4 block",
            ip_address_type: None,
            effective_ip_address_type: "dualstack",
            port_info: json!({
                "fromPort": 5000,
                "toPort": 5100,
                "protocol": "udp",
                "cidrs": ["192.0.2.0/24"],
            }),
            want: Rule::new(5000, 5100, "udp", &["192.0.2.0/24"], &[]),
            want_access_from: "Custom",
        },
        OpenRow {
            case: "a bare IPv4 address, which the model says means /32",
            ip_address_type: None,
            effective_ip_address_type: "dualstack",
            port_info: json!({
                "fromPort": 443,
                "toPort": 443,
                "protocol": "tcp",
                "cidrs": ["192.0.2.44"],
            }),
            want: Rule::new(443, 443, "tcp", &["192.0.2.44"], &[]),
            want_access_from: "Custom",
        },
        OpenRow {
            case: "several addresses, both families",
            ip_address_type: None,
            effective_ip_address_type: "dualstack",
            port_info: json!({
                "fromPort": 3000,
                "toPort": 3000,
                "protocol": "tcp",
                "cidrs": ["192.0.2.44/32", "198.51.100.0/24"],
                "ipv6Cidrs": ["2001:DB8::/32"],
            }),
            want: Rule::new(
                3000,
                3000,
                "tcp",
                &["192.0.2.44/32", "198.51.100.0/24"],
                &["2001:DB8::/32"],
            ),
            want_access_from: "Custom",
        },
        OpenRow {
            case: "every port, every transport protocol",
            ip_address_type: None,
            effective_ip_address_type: "dualstack",
            port_info: json!({"fromPort": 0, "toPort": 65535, "protocol": "all"}),
            want: Rule::new(0, 65535, "all", &["0.0.0.0/0"], &["::/0"]),
            want_access_from: "Anywhere (0.0.0.0/0)",
        },
        OpenRow {
            case: "ICMP ping: type 8, code -1, so fromPort is past toPort",
            ip_address_type: None,
            effective_ip_address_type: "dualstack",
            port_info: json!({
                "fromPort": 8,
                "toPort": -1,
                "protocol": "icmp",
                "cidrs": ["0.0.0.0/0"],
            }),
            want: Rule::new(8, -1, "icmp", &["0.0.0.0/0"], &[]),
            want_access_from: "Anywhere (0.0.0.0/0)",
        },
        OpenRow {
            case: "ICMPv6: type 128, code 0",
            ip_address_type: None,
            effective_ip_address_type: "dualstack",
            port_info: json!({
                "fromPort": 128,
                "toPort": 0,
                "protocol": "icmpv6",
                "ipv6Cidrs": ["::/0"],
            }),
            want: Rule::new(128, 0, "icmpv6", &[], &["::/0"]),
            want_access_from: "Anywhere (::/0)",
        },
        OpenRow {
            case: "the console's RDP/SSH client alias, over a port already open",
            ip_address_type: None,
            effective_ip_address_type: "dualstack",
            port_info: json!({
                "fromPort": 22,
                "toPort": 22,
                "protocol": "tcp",
                "cidrListAliases": ["lightsail-connect"],
            }),
            want: Rule::new(22, 22, "tcp", &[], &[]).with_aliases(&["lightsail-connect"]),
            want_access_from: "Custom",
        },
    ]
}

/// RULE: `OpenInstancePublicPorts` persists the ONE rule its `portInfo` names,
/// leaving every other rule of the firewall alone, and both reads then report
/// exactly the rules the store holds.
#[tokio::test]
async fn an_opened_rule_is_what_every_later_read_reports() {
    let emulator = Emulator::new();

    for (index, row) in open_rows().into_iter().enumerate() {
        let case = row.case;
        let name = format!("firewall-open-{index}");
        create_instance(&emulator, &name, row.ip_address_type).await;

        // Before the call, the firewall is the platform's own defaults.
        let mut want = default_rules(row.effective_ip_address_type);
        assert_firewall(&emulator, case, &name, &want).await;

        let answered = emulator
            .ok(
                "OpenInstancePublicPorts",
                json!({"instanceName": name, "portInfo": row.port_info}),
            )
            .await;
        assert_operation(
            case,
            &answered["operation"],
            "OpenInstancePublicPorts",
            &name,
        );

        upsert(&mut want, row.want.clone());
        assert_firewall(&emulator, case, &name, &want).await;

        // The LITERAL `accessFrom` this row's rule must report, pinned by the
        // table rather than recomputed from the answer.
        let reported = networking_ports(&emulator, &name).await;
        let opened = reported
            .iter()
            .find(|port| rule_of(port) == row.want)
            .unwrap_or_else(|| {
                panic!("case {case}: expected {:?} among {reported:?}", row.want)
            });
        assert_eq!(
            opened["accessFrom"].as_str(),
            Some(row.want_access_from),
            "case {case}: expected accessFrom {}, got {opened:?}",
            row.want_access_from
        );
    }
}

/// RULE: a rule is identified by its port range and protocol, so opening a range
/// that is already open REPLACES the addresses allowed through it rather than
/// holding the same ports twice — and a rule that differs in any part of that key
/// is a rule of its own.
#[tokio::test]
async fn opening_the_same_range_replaces_its_addresses() {
    let emulator = Emulator::new();
    let name = "firewall-upsert";
    create_instance(&emulator, name, None).await;
    let mut want = default_rules("dualstack");

    // Each step: what is opened, and the rule the firewall must then hold.
    let steps: Vec<(&str, Value, Rule)> = vec![
        (
            "first open, one block",
            json!({"fromPort": 8080, "toPort": 8080, "protocol": "tcp", "cidrs": ["192.0.2.0/24"]}),
            Rule::new(8080, 8080, "tcp", &["192.0.2.0/24"], &[]),
        ),
        (
            "same ports and protocol, another block",
            json!({"fromPort": 8080, "toPort": 8080, "protocol": "tcp", "cidrs": ["198.51.100.7/32"]}),
            Rule::new(8080, 8080, "tcp", &["198.51.100.7/32"], &[]),
        ),
        (
            "same ports, another protocol: a rule of its own",
            json!({"fromPort": 8080, "toPort": 8080, "protocol": "udp", "cidrs": ["192.0.2.0/24"]}),
            Rule::new(8080, 8080, "udp", &["192.0.2.0/24"], &[]),
        ),
        (
            "same protocol, a wider range: a rule of its own",
            json!({"fromPort": 8080, "toPort": 8090, "protocol": "tcp", "cidrs": ["192.0.2.0/24"]}),
            Rule::new(8080, 8090, "tcp", &["192.0.2.0/24"], &[]),
        ),
        (
            "an address family added to a rule already open",
            json!({
                "fromPort": 8080,
                "toPort": 8080,
                "protocol": "tcp",
                "cidrs": ["198.51.100.7/32"],
                "ipv6Cidrs": ["2001:DB8::/32"],
            }),
            Rule::new(8080, 8080, "tcp", &["198.51.100.7/32"], &["2001:DB8::/32"]),
        ),
    ];

    for (case, port_info, rule) in steps {
        emulator
            .ok(
                "OpenInstancePublicPorts",
                json!({"instanceName": name, "portInfo": port_info}),
            )
            .await;
        upsert(&mut want, rule);
        assert_firewall(&emulator, case, name, &want).await;
    }
}

/// One `CloseInstancePublicPorts` step of the walk below: the `portInfo` it
/// carries, and the rule key the firewall must have lost afterwards.
struct CloseStep {
    case: &'static str,
    port_info: Value,
    /// `None` where the request names no rule the firewall holds, which must
    /// leave every rule where it is.
    closed: Option<(i64, i64, &'static str)>,
}

/// RULE: `CloseInstancePublicPorts` removes the ONE rule its port range and
/// protocol name — whatever addresses the request carries — and nothing at all
/// when the firewall holds no such rule.
#[tokio::test]
async fn closing_removes_only_the_rule_its_range_and_protocol_name() {
    let emulator = Emulator::new();
    let name = "firewall-close";
    create_instance(&emulator, name, None).await;

    // A firewall with rules that differ in each part of the key, so a close can
    // only be right by matching all of it.
    for port_info in [
        json!({"fromPort": 8080, "toPort": 8080, "protocol": "tcp", "cidrs": ["192.0.2.0/24"]}),
        json!({"fromPort": 8080, "toPort": 8080, "protocol": "udp"}),
        json!({"fromPort": 9000, "toPort": 9100, "protocol": "tcp"}),
    ] {
        emulator
            .ok(
                "OpenInstancePublicPorts",
                json!({"instanceName": name, "portInfo": port_info}),
            )
            .await;
    }
    let mut want = default_rules("dualstack");
    upsert(&mut want, Rule::new(8080, 8080, "tcp", &["192.0.2.0/24"], &[]));
    upsert(
        &mut want,
        Rule::new(8080, 8080, "udp", &["0.0.0.0/0"], &["::/0"]),
    );
    upsert(
        &mut want,
        Rule::new(9000, 9100, "tcp", &["0.0.0.0/0"], &["::/0"]),
    );
    assert_firewall(&emulator, "the firewall to close rules of", name, &want).await;

    // Each step: what is closed, and the key the firewall must have lost (none
    // where the request names no rule the firewall holds).
    let steps: Vec<CloseStep> = vec![
        CloseStep {
            case: "a port that is not open at all",
            port_info: json!({"fromPort": 7000, "toPort": 7000, "protocol": "tcp"}),
            closed: None,
        },
        CloseStep {
            case: "an open range, under another protocol",
            port_info: json!({"fromPort": 9000, "toPort": 9100, "protocol": "udp"}),
            closed: None,
        },
        CloseStep {
            case: "part of an open range",
            port_info: json!({"fromPort": 9000, "toPort": 9050, "protocol": "tcp"}),
            closed: None,
        },
        CloseStep {
            case: "an open rule, named with addresses it was not opened to",
            port_info: json!({
                "fromPort": 8080,
                "toPort": 8080,
                "protocol": "tcp",
                "cidrs": ["0.0.0.0/0"],
            }),
            closed: Some((8080, 8080, "tcp")),
        },
        CloseStep {
            case: "the same ports under the other protocol",
            port_info: json!({"fromPort": 8080, "toPort": 8080, "protocol": "udp"}),
            closed: Some((8080, 8080, "udp")),
        },
        CloseStep {
            case: "a default rule the instance was created with",
            port_info: json!({"fromPort": 22, "toPort": 22, "protocol": "tcp"}),
            closed: Some((22, 22, "tcp")),
        },
        CloseStep {
            case: "the whole open range",
            port_info: json!({"fromPort": 9000, "toPort": 9100, "protocol": "tcp"}),
            closed: Some((9000, 9100, "tcp")),
        },
        CloseStep {
            case: "the same rule a second time",
            port_info: json!({"fromPort": 9000, "toPort": 9100, "protocol": "tcp"}),
            closed: None,
        },
    ];

    for step in steps {
        let case = step.case;
        let answered = emulator
            .ok(
                "CloseInstancePublicPorts",
                json!({"instanceName": name, "portInfo": step.port_info}),
            )
            .await;
        assert_operation(
            case,
            &answered["operation"],
            "CloseInstancePublicPorts",
            name,
        );
        if let Some(key) = step.closed {
            want.retain(|rule| rule.key() != key);
        }
        assert_firewall(&emulator, case, name, &want).await;
    }
}

/// RULE: `PutInstancePublicPorts` replaces the WHOLE set — "This action also
/// closes all currently open ports that are not included in the request" — so
/// what the firewall holds afterwards is exactly the rules the request carried,
/// one per port range and protocol.
#[tokio::test]
async fn put_replaces_the_whole_firewall() {
    let emulator = Emulator::new();
    let name = "firewall-put";
    create_instance(&emulator, name, None).await;

    // Each step: the `portInfos` the request carries, and the firewall it leaves.
    let steps: Vec<(&str, Value, Vec<Rule>)> = vec![
        (
            "two rules, closing both defaults",
            json!([
                {"fromPort": 443, "toPort": 443, "protocol": "tcp"},
                {"fromPort": 1194, "toPort": 1194, "protocol": "udp", "cidrs": ["192.0.2.0/24"]},
            ]),
            vec![
                Rule::new(443, 443, "tcp", &["0.0.0.0/0"], &["::/0"]),
                Rule::new(1194, 1194, "udp", &["192.0.2.0/24"], &[]),
            ],
        ),
        (
            "one rule, closing the other",
            json!([{"fromPort": 443, "toPort": 443, "protocol": "tcp", "cidrs": ["198.51.100.0/24"]}]),
            vec![Rule::new(443, 443, "tcp", &["198.51.100.0/24"], &[])],
        ),
        (
            "two entries naming one rule: the last of them is held",
            json!([
                {"fromPort": 80, "toPort": 80, "protocol": "tcp", "cidrs": ["192.0.2.0/24"]},
                {"fromPort": 80, "toPort": 80, "protocol": "tcp", "cidrs": ["198.51.100.7/32"]},
            ]),
            vec![Rule::new(80, 80, "tcp", &["198.51.100.7/32"], &[])],
        ),
        (
            "no rule at all: every port closed",
            json!([]),
            Vec::new(),
        ),
        (
            "a firewall built again from nothing",
            json!([
                {"fromPort": 22, "toPort": 22, "protocol": "tcp"},
                {"fromPort": 8, "toPort": -1, "protocol": "icmp"},
            ]),
            vec![
                Rule::new(8, -1, "icmp", &["0.0.0.0/0"], &["::/0"]),
                Rule::new(22, 22, "tcp", &["0.0.0.0/0"], &["::/0"]),
            ],
        ),
    ];

    for (case, port_infos, mut want) in steps {
        let answered = emulator
            .ok(
                "PutInstancePublicPorts",
                json!({"instanceName": name, "portInfos": port_infos}),
            )
            .await;
        assert_operation(case, &answered["operation"], "PutInstancePublicPorts", name);
        want.sort();
        assert_firewall(&emulator, case, name, &want).await;
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One `portInfo` the model admits but the service does not, and the member its
/// refusal must name. The rows cover every check the model cannot express: a
/// range that runs backwards, both address notations, the prefix bounds of each
/// address family and the alias catalog.
struct RefusalRow {
    case: &'static str,
    port_info: Value,
    /// A fragment of the member name the refusal has to name, so the caller can
    /// fix the request from the message alone.
    names: &'static str,
}

/// The table.
fn refusal_rows() -> Vec<RefusalRow> {
    vec![
        RefusalRow {
            case: "a tcp range that runs backwards",
            port_info: json!({"fromPort": 9000, "toPort": 8000, "protocol": "tcp"}),
            names: "fromPort",
        },
        RefusalRow {
            case: "a udp range that runs backwards",
            port_info: json!({"fromPort": 1, "toPort": 0, "protocol": "udp"}),
            names: "fromPort",
        },
        RefusalRow {
            case: "cidrs carrying something that is not an address",
            port_info: json!({"fromPort": 80, "toPort": 80, "protocol": "tcp", "cidrs": ["everyone"]}),
            names: "cidrs",
        },
        RefusalRow {
            case: "cidrs carrying an IPv6 block",
            port_info: json!({
                "fromPort": 80,
                "toPort": 80,
                "protocol": "tcp",
                "cidrs": ["2001:DB8::/32"],
            }),
            names: "cidrs",
        },
        RefusalRow {
            case: "an IPv4 prefix past the 32 bits of an IPv4 address",
            port_info: json!({
                "fromPort": 80,
                "toPort": 80,
                "protocol": "tcp",
                "cidrs": ["192.0.2.0/33"],
            }),
            names: "cidrs",
        },
        RefusalRow {
            case: "ipv6Cidrs carrying an IPv4 block",
            port_info: json!({
                "fromPort": 80,
                "toPort": 80,
                "protocol": "tcp",
                "ipv6Cidrs": ["192.0.2.0/24"],
            }),
            names: "ipv6Cidrs",
        },
        RefusalRow {
            case: "an IPv6 prefix past the 128 bits of an IPv6 address",
            port_info: json!({
                "fromPort": 80,
                "toPort": 80,
                "protocol": "tcp",
                "ipv6Cidrs": ["2001:DB8::/129"],
            }),
            names: "ipv6Cidrs",
        },
        RefusalRow {
            case: "an alias this service does not publish",
            port_info: json!({
                "fromPort": 22,
                "toPort": 22,
                "protocol": "tcp",
                "cidrListAliases": ["lightsail-console"],
            }),
            names: "cidrListAliases",
        },
    ]
}

/// RULE: a request the service cannot accept is refused with the modelled
/// `InvalidInputException` — whose message names the member that is wrong — and
/// the firewall is left exactly as it was, whichever of the three operations
/// carried it.
#[tokio::test]
async fn a_rule_the_service_cannot_accept_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let name = "firewall-refusal";
    create_instance(&emulator, name, None).await;
    let want = default_rules("dualstack");

    for row in refusal_rows() {
        for (operation, body) in [
            (
                "OpenInstancePublicPorts",
                json!({"instanceName": name, "portInfo": row.port_info}),
            ),
            (
                "CloseInstancePublicPorts",
                json!({"instanceName": name, "portInfo": row.port_info}),
            ),
            (
                // A whole-set replacement carrying one unacceptable entry must
                // leave the set alone, so the acceptable entry beside it is not
                // applied either.
                "PutInstancePublicPorts",
                json!({
                    "instanceName": name,
                    "portInfos": [
                        {"fromPort": 3000, "toPort": 3000, "protocol": "tcp"},
                        row.port_info,
                    ],
                }),
            ),
        ] {
            let case = format!("{} via {operation}", row.case);
            let (status, answered) = emulator.call(operation, body).await;
            assert_eq!(
                status, 400,
                "case {case}: expected the modelled InvalidInputException status, got \
                 {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "InvalidInputException",
                "case {case}: expected InvalidInputException, got {answered:?}"
            );
            let message = answered["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(row.names),
                "case {case}: expected the message to name {}, got {answered:?}",
                row.names
            );
            assert_firewall(&emulator, &case, name, &want).await;
        }
    }
}

/// RULE: a value outside a MODELLED constraint — a port outside `Port`'s `@range`
/// of -1 to 65535, a protocol outside the `NetworkProtocol` enum — is refused by
/// the modelled-constraint walk before any handler runs, and nothing is stored.
#[tokio::test]
async fn a_value_outside_the_modelled_constraints_is_refused_by_the_walk() {
    let emulator = Emulator::new();
    let name = "firewall-range";
    create_instance(&emulator, name, None).await;
    let want = default_rules("dualstack");

    for (case, port_info) in [
        (
            "fromPort past the modelled maximum",
            json!({"fromPort": 65536, "toPort": 65536, "protocol": "tcp"}),
        ),
        (
            "toPort past the modelled maximum",
            json!({"fromPort": 80, "toPort": 70000, "protocol": "tcp"}),
        ),
        (
            "fromPort under the modelled minimum",
            json!({"fromPort": -2, "toPort": 80, "protocol": "tcp"}),
        ),
        (
            "a protocol outside the modelled NetworkProtocol enum",
            json!({"fromPort": 132, "toPort": 132, "protocol": "sctp"}),
        ),
    ] {
        let (status, answered) = emulator
            .call(
                "OpenInstancePublicPorts",
                json!({"instanceName": name, "portInfo": port_info}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case}: expected the modelled-constraint walk's refusal, got {answered:?}"
        );
        assert_firewall(&emulator, case, name, &want).await;
    }
}

/// RULE: a well-formed name the account does not hold has no firewall at all, so
/// every one of these operations answers the service's `DoesNotExist` envelope —
/// HTTP 400, wire error `NotFoundException`, body `code` `DoesNotExist` — rather
/// than a zeroed 200.
#[tokio::test]
async fn an_instance_the_account_does_not_hold_has_no_firewall() {
    let emulator = Emulator::new();
    // One instance exists, so the refusal is about the name and not about an
    // empty store.
    create_instance(&emulator, "firewall-present", None).await;

    let port_info = json!({"fromPort": 8080, "toPort": 8080, "protocol": "tcp"});
    for (operation, body) in [
        (
            "GetInstancePortStates",
            json!({"instanceName": "firewall-absent"}),
        ),
        (
            "OpenInstancePublicPorts",
            json!({"instanceName": "firewall-absent", "portInfo": port_info}),
        ),
        (
            "CloseInstancePublicPorts",
            json!({"instanceName": "firewall-absent", "portInfo": port_info}),
        ),
        (
            "PutInstancePublicPorts",
            json!({"instanceName": "firewall-absent", "portInfos": [port_info]}),
        ),
    ] {
        let (status, answered) = emulator.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {operation}: Lightsail answers a missing resource with 400, got \
             {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "NotFoundException",
            "case {operation}: expected NotFoundException, got {answered:?}"
        );
        assert_eq!(
            answered["code"].as_str(),
            Some("DoesNotExist"),
            "case {operation}: expected the DoesNotExist code, got {answered:?}"
        );
    }

    // The instance that does exist still reports its own firewall.
    assert_firewall(
        &emulator,
        "the instance that does exist",
        "firewall-present",
        &default_rules("dualstack"),
    )
    .await;
}

/// RULE: a firewall belongs to ONE instance, and reading it changes nothing — so
/// a change to one instance's ports is invisible to every other, and two reads
/// in a row report the same rules.
#[tokio::test]
async fn a_firewall_belongs_to_one_instance_and_a_read_changes_nothing() {
    let emulator = Emulator::new();
    create_instance(&emulator, "firewall-one", None).await;
    create_instance(&emulator, "firewall-two", None).await;

    emulator
        .ok(
            "OpenInstancePublicPorts",
            json!({
                "instanceName": "firewall-one",
                "portInfo": {"fromPort": 8080, "toPort": 8080, "protocol": "tcp"},
            }),
        )
        .await;
    emulator
        .ok(
            "PutInstancePublicPorts",
            json!({"instanceName": "firewall-two", "portInfos": []}),
        )
        .await;

    let mut want_one = default_rules("dualstack");
    upsert(
        &mut want_one,
        Rule::new(8080, 8080, "tcp", &["0.0.0.0/0"], &["::/0"]),
    );
    assert_firewall(&emulator, "the instance that was opened", "firewall-one", &want_one).await;
    assert_firewall(
        &emulator,
        "the instance that was emptied",
        "firewall-two",
        &[],
    )
    .await;

    // The reads above changed nothing: the same calls answer the same rules.
    let first = firewall(&emulator, "firewall-one").await;
    let second = firewall(&emulator, "firewall-one").await;
    assert_eq!(
        first, second,
        "case reading twice: a read reports the firewall, never changes it, got {first:?} \
         then {second:?}"
    );
    assert_eq!(
        first, want_one,
        "case reading twice: expected {want_one:?}, got {first:?}"
    );
}

/// RULE: the emulator's own growth guards — the most rules one firewall holds and
/// the most addresses one rule's list carries — admit everything up to the bound
/// and refuse what is past it with the modelled `InvalidInputException`, leaving
/// the firewall as it was.
///
/// The bounds themselves are this crate's `MAX_PORT_RULES` and `MAX_PORT_CIDRS`,
/// read from the store rather than copied, so the cases follow the constants.
#[tokio::test]
async fn the_stores_own_bounds_admit_the_bound_and_refuse_what_is_past_it() {
    let emulator = Emulator::new();
    let name = "firewall-bounds";
    create_instance(&emulator, name, None).await;

    // Exactly the bound: one rule per port, all of them held.
    let full: Vec<Value> = (0..MAX_PORT_RULES)
        .map(|index| json!({"fromPort": 10_000 + index, "toPort": 10_000 + index, "protocol": "tcp"}))
        .collect();
    emulator
        .ok(
            "PutInstancePublicPorts",
            json!({"instanceName": name, "portInfos": full}),
        )
        .await;
    let mut want: Vec<Rule> = (0..MAX_PORT_RULES)
        .map(|index| {
            let port = i64::try_from(10_000 + index).expect("a port fits in an i64");
            Rule::new(port, port, "tcp", &["0.0.0.0/0"], &["::/0"])
        })
        .collect();
    want.sort();
    assert_firewall(&emulator, "a firewall filled to the bound", name, &want).await;

    // One rule past the bound: an open of a rule the full firewall does not
    // already hold is refused, and the full set is untouched.
    let (status, answered) = emulator
        .call(
            "OpenInstancePublicPorts",
            json!({
                "instanceName": name,
                "portInfo": {"fromPort": 20_000, "toPort": 20_000, "protocol": "tcp"},
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case opening past the bound: expected a 400 refusal, got {status} {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "InvalidInputException",
        "case opening past the bound: expected InvalidInputException, got {answered:?}"
    );
    assert_firewall(&emulator, "a firewall that refused an open", name, &want).await;

    // A rule the full firewall DOES hold is still an upsert, not growth.
    emulator
        .ok(
            "OpenInstancePublicPorts",
            json!({
                "instanceName": name,
                "portInfo": {
                    "fromPort": 10_000,
                    "toPort": 10_000,
                    "protocol": "tcp",
                    "cidrs": ["192.0.2.0/24"],
                },
            }),
        )
        .await;
    upsert(
        &mut want,
        Rule::new(10_000, 10_000, "tcp", &["192.0.2.0/24"], &[]),
    );
    assert_firewall(&emulator, "an upsert on a full firewall", name, &want).await;

    // A replacement whose entries DEDUPE to within the bound is accepted: the
    // bound is on what the firewall holds, not on how many entries the request
    // wrote it with.
    let duplicated: Vec<Value> = (0..MAX_PORT_RULES)
        .flat_map(|index| {
            let port = 50_000 + index;
            [
                json!({"fromPort": port, "toPort": port, "protocol": "tcp", "cidrs": ["192.0.2.0/24"]}),
                json!({"fromPort": port, "toPort": port, "protocol": "tcp"}),
            ]
        })
        .collect();
    assert_eq!(
        duplicated.len(),
        2 * MAX_PORT_RULES,
        "case a deduping replacement: the request must carry twice the bound, got {}",
        duplicated.len()
    );
    emulator
        .ok(
            "PutInstancePublicPorts",
            json!({"instanceName": name, "portInfos": duplicated}),
        )
        .await;
    let mut rebuilt: Vec<Rule> = (0..MAX_PORT_RULES)
        .map(|index| {
            let port = i64::try_from(50_000 + index).expect("a port fits in an i64");
            Rule::new(port, port, "tcp", &["0.0.0.0/0"], &["::/0"])
        })
        .collect();
    rebuilt.sort();
    want = rebuilt;
    assert_firewall(&emulator, "a deduping replacement", name, &want).await;

    // A whole-set replacement past the bound is refused, and so is a rule whose
    // address list is past the per-list bound; neither changes anything.
    let past_rules: Vec<Value> = (0..=MAX_PORT_RULES)
        .map(|index| json!({"fromPort": 30_000 + index, "toPort": 30_000 + index, "protocol": "tcp"}))
        .collect();
    // One entry past what ONE REQUEST may carry, which is the bound on the work
    // a request buys rather than on what the firewall holds: every entry names
    // the same rule, so the set it would leave is a single rule.
    let past_entries: Vec<Value> = (0..=MAX_PORT_INFOS_PER_REQUEST)
        .map(|_| json!({"fromPort": 31_000, "toPort": 31_000, "protocol": "tcp"}))
        .collect();
    let past_cidrs: Vec<String> = (0..=MAX_PORT_CIDRS)
        .map(|index| format!("192.0.{}.{}/32", index / 256, index % 256))
        .collect();
    for (case, operation, body, names) in [
        (
            "a request carrying more entries than one request may",
            "PutInstancePublicPorts",
            json!({"instanceName": name, "portInfos": past_entries}),
            "portInfos",
        ),
        (
            "a replacement carrying more rules than the bound",
            "PutInstancePublicPorts",
            json!({"instanceName": name, "portInfos": past_rules}),
            "portInfos",
        ),
        (
            "a rule carrying more addresses than the bound",
            "OpenInstancePublicPorts",
            json!({
                "instanceName": name,
                "portInfo": {
                    "fromPort": 40_000,
                    "toPort": 40_000,
                    "protocol": "tcp",
                    "cidrs": past_cidrs,
                },
            }),
            "cidrs",
        ),
    ] {
        let (status, answered) = emulator.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {answered:?}"
        );
        let message = answered["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(names),
            "case {case}: expected the message to name {names}, got {answered:?}"
        );
        assert_firewall(&emulator, case, name, &want).await;
    }
}
