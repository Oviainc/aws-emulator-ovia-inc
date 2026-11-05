// SPDX-License-Identifier: CC0-1.0

//! An instance's inbound firewall: the rules it is created with, what a request
//! makes of them, and the address families and CIDRs a rule may name.

use super::*;

/// The `operationType` the log entry of each firewall change carries, from the
/// modelled `OperationType` enum — kept with the firewall slice they belong to,
/// for the reason the `use` above gives.
pub const OPERATION_TYPE_OPEN_INSTANCE_PUBLIC_PORTS: &str = "OpenInstancePublicPorts";
/// See [`OPERATION_TYPE_OPEN_INSTANCE_PUBLIC_PORTS`].
pub const OPERATION_TYPE_CLOSE_INSTANCE_PUBLIC_PORTS: &str = "CloseInstancePublicPorts";
/// See [`OPERATION_TYPE_OPEN_INSTANCE_PUBLIC_PORTS`].
pub const OPERATION_TYPE_PUT_INSTANCE_PUBLIC_PORTS: &str = "PutInstancePublicPorts";

/// The most inbound firewall rules one instance's firewall may hold.
///
/// Emulator resource guard, NOT an AWS quota: every `OpenInstancePublicPorts`
/// and `PutInstancePublicPorts` call grows the rule set, so a call that would
/// take it past this is refused with the modelled `InvalidInputException`. Kept
/// here rather than with the store's other bounds, for the reason the `use`
/// above gives.
pub const MAX_PORT_RULES: usize = 200;

/// The most addresses one firewall rule's `cidrs`, `ipv6Cidrs` or
/// `cidrListAliases` list may carry.
///
/// Emulator resource guard: the modelled `StringList` declares no length, so
/// this is where the bound on a caller-sized address list lives.
pub const MAX_PORT_CIDRS: usize = 50;

/// The most `portInfos` entries one `PutInstancePublicPorts` request may carry.
///
/// Emulator resource guard: the modelled `PortInfoList` declares no length, and
/// this bound is what caps the WORK a request buys, before any entry of it is
/// looked at. It is deliberately above [`MAX_PORT_RULES`] — twice it — because a
/// request may name one rule more than once and such entries collapse, so the
/// count of entries and the count of rules are not the same bound.
pub const MAX_PORT_INFOS_PER_REQUEST: usize = 2 * MAX_PORT_RULES;

/// The management port a new instance's firewall opens: SSH for a Linux/Unix
/// blueprint, RDP for a Windows one.
pub(super) const SSH_PORT: i32 = 22;
/// See [`SSH_PORT`].
pub(super) const RDP_PORT: i32 = 3389;
/// The web port a new instance's firewall opens.
pub(super) const HTTP_PORT: i32 = 80;

/// The `PortState` every rule an instance's firewall holds reports, from the
/// modelled `InstancePortState$state` documentation: "The port state for
/// Lightsail instances is always `open`." A port that is not open is not a rule
/// of the firewall at all — `CloseInstancePublicPorts` removes it.
pub const PORT_STATE_OPEN: &str = "open";

/// The `accessDirection` every rule reports, from the modelled
/// `InstancePortInfo$accessDirection` documentation: "Lightsail currently
/// supports only `inbound` access direction."
pub const ACCESS_DIRECTION_INBOUND: &str = "inbound";

/// The `accessType` every rule these operations manage reports: they are the
/// instance's PUBLIC ports (`OpenInstancePublicPorts` /
/// `PutInstancePublicPorts`), and `Public` is the modelled `PortAccessType`
/// value for that.
pub const PORT_ACCESS_TYPE_PUBLIC: &str = "Public";

/// The protocol a rule carries when the request named none.
///
/// CHOSEN: `PortInfo$protocol` is optional and carries no modelled `@default`,
/// and `tcp` is the protocol this service's own default firewall rules use (see
/// [`default_port_rules`]), so it is what a rule that names no protocol means.
pub const DEFAULT_NETWORK_PROTOCOL: &str = "tcp";

/// Every IPv4 address, in the CIDR notation `PortInfo$cidrs` takes.
pub const ANY_IPV4_CIDR: &str = "0.0.0.0/0";
/// Every IPv6 address, in the CIDR notation `PortInfo$ipv6Cidrs` takes.
pub const ANY_IPV6_CIDR: &str = "::/0";

/// The `accessFrom` a rule open to every address reports, from the modelled
/// `InstancePortInfo$accessFrom` documentation, whose example is exactly this
/// text (`Anywhere (0.0.0.0/0)`).
pub const ACCESS_FROM_ANYWHERE_IPV4: &str = "Anywhere (0.0.0.0/0)";
/// The same for a rule open to every IPv6 address only.
pub const ACCESS_FROM_ANYWHERE_IPV6: &str = "Anywhere (::/0)";
/// The `accessFrom` a rule open to named addresses reports: the same member's
/// documentation — "or `Custom` if a specific IP address or range of IP
/// addresses is allowed".
pub const ACCESS_FROM_CUSTOM: &str = "Custom";

/// The only `cidrListAliases` entry Lightsail accepts, from the modelled
/// `PortInfo$cidrListAliases` documentation: "The only alias currently supported
/// is `lightsail-connect`, which allows IP addresses of the browser-based
/// RDP/SSH client in the Lightsail console to connect to your instance."
pub const CIDR_LIST_ALIAS_LIGHTSAIL_CONNECT: &str = "lightsail-connect";

/// One inbound rule of an instance's firewall: the port range and protocol it
/// opens, and the addresses allowed to reach the instance through it.
///
/// The port range and the protocol together IDENTIFY the rule — that is the key
/// `OpenInstancePublicPorts` upserts, `CloseInstancePublicPorts` removes and
/// `PutInstancePublicPorts` replaces the whole set of — and the addresses are
/// what the caller asked for, kept verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPortRule {
    pub from_port: i32,
    pub to_port: i32,
    pub protocol: String,
    pub cidrs: Vec<String>,
    pub ipv6_cidrs: Vec<String>,
    pub cidr_list_aliases: Vec<String>,
}

impl StoredPortRule {
    /// The key that identifies this rule within one firewall: its port range and
    /// its protocol.
    pub fn key(&self) -> (i32, i32, &str) {
        (self.from_port, self.to_port, self.protocol.as_str())
    }

    /// Whether this rule opens the same ports, over the same protocol, as
    /// `other` — i.e. whether the two are the same rule of one firewall.
    pub fn is_same_rule(&self, other: &Self) -> bool {
        self.key() == other.key()
    }

    /// The rule as the modelled `InstancePortState` a firewall read reports.
    pub fn port_state_view(&self) -> InstancePortState {
        InstancePortState {
            from_port: self.from_port,
            to_port: self.to_port,
            protocol: Some(self.protocol.clone()),
            state: Some(PORT_STATE_OPEN.to_string()),
            cidrs: list_or_absent(&self.cidrs),
            ipv6_cidrs: list_or_absent(&self.ipv6_cidrs),
            cidr_list_aliases: list_or_absent(&self.cidr_list_aliases),
        }
    }

    /// The rule as the modelled `InstancePortInfo` an instance's `networking`
    /// reports.
    ///
    /// `accessFrom` and `accessType` are DERIVED from the rule's own addresses,
    /// so one rule reads the same way through both views.
    pub fn port_info_view(&self) -> InstancePortInfo {
        InstancePortInfo {
            from_port: self.from_port,
            to_port: self.to_port,
            protocol: Some(self.protocol.clone()),
            access_from: Some(self.access_from().to_string()),
            access_type: Some(PORT_ACCESS_TYPE_PUBLIC.to_string()),
            // `commonName` is "The common name of the port information", which
            // this service has no source for — absent rather than invented.
            common_name: None,
            access_direction: Some(ACCESS_DIRECTION_INBOUND.to_string()),
            cidrs: list_or_absent(&self.cidrs),
            ipv6_cidrs: list_or_absent(&self.ipv6_cidrs),
            cidr_list_aliases: list_or_absent(&self.cidr_list_aliases),
        }
    }

    /// Where this rule allows access from: `Anywhere (…)` when it admits a whole
    /// address family, `Custom` when it names addresses.
    ///
    /// A rule that admits a whole family ALONGSIDE narrower blocks still reads as
    /// `Anywhere`: the narrower entries add nothing the wide one does not already
    /// admit, so they do not make the access custom. A rule open to BOTH families
    /// reports the IPv4 form, the one this member's own documentation gives
    /// (`Anywhere (0.0.0.0/0)`); its IPv6 half is reported where it belongs, in
    /// `ipv6Cidrs`, so nothing about the rule is lost.
    pub(super) fn access_from(&self) -> &'static str {
        if self.cidrs.iter().any(|cidr| cidr == ANY_IPV4_CIDR) {
            ACCESS_FROM_ANYWHERE_IPV4
        } else if self.ipv6_cidrs.iter().any(|cidr| cidr == ANY_IPV6_CIDR) {
            ACCESS_FROM_ANYWHERE_IPV6
        } else {
            ACCESS_FROM_CUSTOM
        }
    }
}

/// `values` as a modelled list member, absent when it carries nothing: an empty
/// address list says the rule names no address of that family, which is not the
/// same claim as an empty list of them.
pub(super) fn list_or_absent(values: &[String]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| values.to_vec())
}

/// The inbound rules Lightsail opens on a newly created instance's firewall:
/// the platform's management port (SSH for Linux/Unix, RDP for Windows) and HTTP,
/// both reachable from anywhere. Taken from the Lightsail user guide's
/// "Firewalls and ports" description of a new instance's default rules.
///
/// The IPv6 side of each rule is present only when the instance has an IPv6
/// address at all, so the reported rules match the addresses the instance has.
pub fn default_port_rules(blueprint_id: &str, ip_address_type: &str) -> Vec<StoredPortRule> {
    let management = if blueprint_is_windows(blueprint_id) {
        RDP_PORT
    } else {
        SSH_PORT
    };
    let (cidrs, ipv6_cidrs) = anywhere_cidrs(ip_address_type);
    [management, HTTP_PORT]
        .into_iter()
        .map(|port| StoredPortRule {
            from_port: port,
            to_port: port,
            protocol: DEFAULT_NETWORK_PROTOCOL.to_string(),
            cidrs: cidrs.clone(),
            ipv6_cidrs: ipv6_cidrs.clone(),
            cidr_list_aliases: Vec::new(),
        })
        .collect()
}

/// The IPv4 and IPv6 "every address" blocks an instance with `ip_address_type`
/// can be reached on: each present only when that address type gives the
/// instance an address of the family, because a rule cannot admit an address
/// family the instance does not have.
pub(super) fn anywhere_cidrs(ip_address_type: &str) -> (Vec<String>, Vec<String>) {
    (
        if ip_address_type_has_ipv4(ip_address_type) {
            vec![ANY_IPV4_CIDR.to_string()]
        } else {
            Vec::new()
        },
        if ip_address_type_has_ipv6(ip_address_type) {
            vec![ANY_IPV6_CIDR.to_string()]
        } else {
            Vec::new()
        },
    )
}

/// The rule a `PortInfo` asks for, on an instance whose addresses are
/// `ip_address_type`'s — or a refusal naming the member that is wrong.
///
/// `member` is the input member the structure came from (`portInfo`,
/// `portInfos[2]`), so a refusal tells the caller which entry of the request to
/// fix.
///
/// What the model cannot express is checked here: the protocol must be one of
/// the modelled `NetworkProtocol` values, a port RANGE must run forwards, each
/// address must be in the notation its own member takes, each address must be of
/// a family `ip_address_type` gives the instance, and an alias must be one
/// Lightsail publishes. A rule that names no address at all is taken as open
/// to anywhere — Lightsail's own default rules are, and a rule admitting nothing
/// would open a port nothing can reach.
pub fn port_rule_from_request(
    member: &str,
    port_info: &PortInfo,
    ip_address_type: &str,
) -> Result<StoredPortRule, String> {
    let protocol = port_info
        .protocol
        .clone()
        .unwrap_or_else(|| DEFAULT_NETWORK_PROTOCOL.to_string());
    // `PortInfo$protocol` targets the modelled `NetworkProtocol` enum, so the
    // centralized walk has already refused any other value a REQUEST carried;
    // this keeps the rule total over whatever protocol reaches this function,
    // including the one substituted just above for a request that named none.
    if !NETWORK_PROTOCOL_VALUES.contains(&protocol.as_str()) {
        return Err(format!(
            "{member}.protocol must be one of [{}], got '{protocol}'",
            NETWORK_PROTOCOL_VALUES.join(", ")
        ));
    }
    // For `tcp`, `udp` and `all` the two members are the ends of a port range,
    // so the range has to run forwards. For `icmp` and `icmpv6` they are not a
    // range at all: `fromPort` is the ICMP type and `toPort` the ICMP code, as
    // both members' own documentation says, so no ordering holds between them.
    if !protocol_ports_are_icmp(&protocol) && port_info.from_port > port_info.to_port {
        return Err(format!(
            "{member}.fromPort {} is past {member}.toPort {}: a {protocol} rule opens a \
             range of ports, so fromPort must not be greater than toPort",
            port_info.from_port, port_info.to_port
        ));
    }
    // Each address member's own path, spelled once: both the notation check and
    // the family check below name the member the caller has to fix.
    let cidrs_member = format!("{member}.cidrs");
    let ipv6_cidrs_member = format!("{member}.ipv6Cidrs");
    let cidrs = checked_cidrs(
        &cidrs_member,
        port_info.cidrs.as_deref().unwrap_or_default(),
        is_ipv4_cidr,
        "an IPv4 address or an IPv4 range in CIDR notation",
    )?;
    let ipv6_cidrs = checked_cidrs(
        &ipv6_cidrs_member,
        port_info.ipv6_cidrs.as_deref().unwrap_or_default(),
        is_ipv6_cidr,
        "an IPv6 address or an IPv6 range in CIDR notation",
    )?;
    let cidr_list_aliases = checked_cidrs(
        &format!("{member}.cidrListAliases"),
        port_info.cidr_list_aliases.as_deref().unwrap_or_default(),
        |alias| alias == CIDR_LIST_ALIAS_LIGHTSAIL_CONNECT,
        "the only alias this service publishes, 'lightsail-connect'",
    )?;
    // Each address member is checked against the families the INSTANCE has, not
    // only against its own notation: a rule cannot admit an address family the
    // instance does not have, so an address of a family `ip_address_type` gives
    // the instance none of admits nothing, and is refused rather than stored.
    // That is the same rule the substitution below applies to a request naming
    // no address, and the one [`default_port_rules`] applies to a new instance's
    // firewall. `cidrListAliases` names no address family of its own: it is a
    // preconfigured range Lightsail publishes, so it is left to its own check
    // above.
    check_family_the_instance_has(&cidrs_member, &cidrs, ip_address_type, AddressFamily::Ipv4)?;
    check_family_the_instance_has(
        &ipv6_cidrs_member,
        &ipv6_cidrs,
        ip_address_type,
        AddressFamily::Ipv6,
    )?;
    let (cidrs, ipv6_cidrs) =
        if cidrs.is_empty() && ipv6_cidrs.is_empty() && cidr_list_aliases.is_empty() {
            anywhere_cidrs(ip_address_type)
        } else {
            (cidrs, ipv6_cidrs)
        };
    Ok(StoredPortRule {
        from_port: port_info.from_port,
        to_port: port_info.to_port,
        protocol,
        cidrs,
        ipv6_cidrs,
        cidr_list_aliases,
    })
}

/// Whether `protocol`'s `fromPort`/`toPort` are an ICMP type and code rather
/// than the ends of a port range, from those members' own documentation.
pub(super) fn protocol_ports_are_icmp(protocol: &str) -> bool {
    matches!(protocol, "icmp" | "icmpv6")
}

/// One of the two address families a firewall rule's addresses can be of: the
/// family `cidrs` carries, and the one `ipv6Cidrs` carries.
#[derive(Debug, Clone, Copy)]
pub(super) enum AddressFamily {
    Ipv4,
    Ipv6,
}

impl AddressFamily {
    /// Whether an instance with `ip_address_type` has an address of this family,
    /// asked of the ONE predicate per family this service already keeps.
    pub(super) fn given_by(self, ip_address_type: &str) -> bool {
        match self {
            Self::Ipv4 => ip_address_type_has_ipv4(ip_address_type),
            Self::Ipv6 => ip_address_type_has_ipv6(ip_address_type),
        }
    }

    /// The family's name, as a refusal names it.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Ipv4 => "IPv4",
            Self::Ipv6 => "IPv6",
        }
    }
}

/// `Ok(())` when `entries` names no address at all or an instance with
/// `ip_address_type` has an address of `family`, and otherwise a refusal naming
/// `member`, that family and the address type that gives the instance none of it.
///
/// An instance is reachable only on the families its `ip_address_type` gives it,
/// so a rule naming an address of any other family would admit nothing — see
/// [`anywhere_cidrs`], which leaves out the same family for the same reason. The
/// family decides BOTH halves of the check, so the predicate asked and the name
/// reported cannot disagree.
pub(super) fn check_family_the_instance_has(
    member: &str,
    entries: &[String],
    ip_address_type: &str,
    family: AddressFamily,
) -> Result<(), String> {
    if entries.is_empty() || family.given_by(ip_address_type) {
        return Ok(());
    }
    let family = family.label();
    Err(format!(
        "{member} names {family} addresses, but the instance's ipAddressType \
         '{ip_address_type}' gives it no {family} address: a rule cannot admit an address \
         family the instance does not have"
    ))
}

/// `entries` when every one of them satisfies `accepted` and the list is within
/// [`MAX_PORT_CIDRS`], or a refusal naming `member`, the entry that is wrong and
/// the `notation` that member takes.
pub(super) fn checked_cidrs(
    member: &str,
    entries: &[String],
    accepted: impl Fn(&str) -> bool,
    notation: &str,
) -> Result<Vec<String>, String> {
    if entries.len() > MAX_PORT_CIDRS {
        return Err(format!(
            "{member} carries {} entries, more than the {MAX_PORT_CIDRS} this service \
             accepts",
            entries.len()
        ));
    }
    for entry in entries {
        if !accepted(entry) {
            return Err(format!("{member} entry '{entry}' is not {notation}"));
        }
    }
    Ok(entries.to_vec())
}

/// Whether `value` is an IPv4 address or a block of them in CIDR notation.
///
/// A bare address is accepted because `PortInfo$cidrs` says it is: "To allow the
/// IP address `192.0.2.44`, specify `192.0.2.44` or `192.0.2.44/32`" — so it is
/// kept as the caller wrote it rather than rewritten.
pub(super) fn is_ipv4_cidr(value: &str) -> bool {
    is_cidr::<std::net::Ipv4Addr>(value, 32)
}

/// Whether `value` is an IPv6 address or a block of them in CIDR notation. See
/// [`is_ipv4_cidr`]; `PortInfo$ipv6Cidrs` takes the same notation over the IPv6
/// address family.
pub(super) fn is_ipv6_cidr(value: &str) -> bool {
    is_cidr::<std::net::Ipv6Addr>(value, 128)
}

/// Whether `value` is an address of `Addr`, optionally followed by a prefix
/// length of at most `address_bits`.
///
/// The ADDRESS is parsed by the standard library's own parser for that family,
/// which is the parser for this notation: only what `Addr` admits is accepted,
/// and what remains of CIDR notation is the one decimal prefix length this
/// checks. No dependency is added for it, which keeps this change out of
/// `Cargo.lock` — the file every concurrent change in this repository also
/// writes.
pub(super) fn is_cidr<Addr: std::str::FromStr>(value: &str, address_bits: u32) -> bool {
    let (address, prefix) = match value.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix)),
        None => (value, None),
    };
    if address.parse::<Addr>().is_err() {
        return false;
    }
    match prefix {
        None => true,
        Some(prefix) => {
            !prefix.is_empty()
                && prefix.chars().all(|digit| digit.is_ascii_digit())
                && prefix.parse::<u32>().is_ok_and(|bits| bits <= address_bits)
        }
    }
}
