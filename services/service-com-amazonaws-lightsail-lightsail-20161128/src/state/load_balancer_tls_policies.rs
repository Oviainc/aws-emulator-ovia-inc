// SPDX-License-Identifier: CC0-1.0

//! The published load balancer TLS security policy catalog: the protocol
//! versions and ciphers each policy admits.

use super::*;

/// How many TLS security policies one `GetLoadBalancerTlsPolicies` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that "a next
/// page token is not returned if there are no more results to display". Matches
/// [`LOAD_BALANCER_PAGE_SIZE`], so every listing of this slice pages the same
/// way, and it is at least the number of policies this service publishes, so the
/// catalog is answered whole in one page — as AWS answers it — while the paging
/// walk still holds for a catalog that outgrows it.
pub const LOAD_BALANCER_TLS_POLICY_PAGE_SIZE: usize = 5;

/// The protocol versions a published policy may negotiate, oldest first.
///
/// SOURCE: the wire tokens the recorded `GetLoadBalancerTlsPolicies` answer
/// carries for a policy's protocol versions — the `Protocol-` prefixed form of
/// each TLS version, which is the value a client echoes back when configuring a
/// load balancer, so `LoadBalancerTlsPolicy$protocols` ("The protocols used in a
/// given TLS security policy") is not interchangeable with the bare version
/// name. The capitalisation is the recorded one, which differs between the
/// tokens.
pub(super) const TLS_PROTOCOL_1_0: &str = "Protocol-TLSv1";
/// See [`TLS_PROTOCOL_1_0`].
pub(super) const TLS_PROTOCOL_1_1: &str = "Protocol-TLSV1.1";
/// See [`TLS_PROTOCOL_1_0`].
pub(super) const TLS_PROTOCOL_1_2: &str = "Protocol-TLSv1.2";

/// Every protocol version a policy that still accepts TLS 1.0 negotiates.
pub(super) const TLS_PROTOCOLS_1_0_AND_LATER: &[&str] =
    &[TLS_PROTOCOL_1_0, TLS_PROTOCOL_1_1, TLS_PROTOCOL_1_2];
/// The only protocol version a TLS 1.2 policy negotiates.
pub(super) const TLS_PROTOCOLS_1_2_ONLY: &[&str] = &[TLS_PROTOCOL_1_2];

/// The cipher suites the DEFAULT policy offers, in order of preference — the
/// suites of the load-balancing service's own 2016-08 security policy, which the
/// `TLS-2016-08` Lightsail policy is.
///
/// "The ciphers are listed in order of preference"
/// (`LoadBalancerTlsPolicy$ciphers`), so this order is part of the published
/// data: the ephemeral-key (`ECDHE`) suites come first, 128-bit before 256-bit
/// within each key exchange, and the static-RSA suites last.
pub(super) const TLS_DEFAULT_CIPHERS: &[&str] = &[
    "ECDHE-ECDSA-AES128-GCM-SHA256",
    "ECDHE-RSA-AES128-GCM-SHA256",
    "ECDHE-ECDSA-AES128-SHA256",
    "ECDHE-RSA-AES128-SHA256",
    "ECDHE-ECDSA-AES128-SHA",
    "ECDHE-RSA-AES128-SHA",
    "ECDHE-ECDSA-AES256-GCM-SHA384",
    "ECDHE-RSA-AES256-GCM-SHA384",
    "ECDHE-ECDSA-AES256-SHA384",
    "ECDHE-RSA-AES256-SHA384",
    "ECDHE-RSA-AES256-SHA",
    "ECDHE-ECDSA-AES256-SHA",
    "AES128-GCM-SHA256",
    "AES128-SHA256",
    "AES128-SHA",
    "AES256-GCM-SHA384",
    "AES256-SHA256",
    "AES256-SHA",
];

/// The cipher suites a RESTRICTED forward-secrecy policy offers: the
/// ephemeral-key (`ECDHE`) suites of [`TLS_DEFAULT_CIPHERS`], in the same order
/// of preference and without its static-RSA ones — a session key those cannot
/// leak with the server's private key is what forward secrecy means — kept to
/// the suites whose handshake and record MAC use SHA-256 or SHA-384, so the
/// SHA-1 suites are left out.
pub(super) const TLS_FORWARD_SECRECY_RESTRICTED_CIPHERS: &[&str] = &[
    "ECDHE-ECDSA-AES128-GCM-SHA256",
    "ECDHE-RSA-AES128-GCM-SHA256",
    "ECDHE-ECDSA-AES128-SHA256",
    "ECDHE-RSA-AES128-SHA256",
    "ECDHE-ECDSA-AES256-GCM-SHA384",
    "ECDHE-RSA-AES256-GCM-SHA384",
    "ECDHE-ECDSA-AES256-SHA384",
    "ECDHE-RSA-AES256-SHA384",
];

/// One TLS security policy a load balancer of this service can be given: the
/// protocol versions it negotiates and the cipher suites it offers, in order of
/// preference.
///
/// This is PUBLISHED data, not per-account state: every account sees the same
/// policies and no operation in this service creates, changes or withdraws one.
/// So it is a `const` table beside this service's other published catalogs
/// (instance bundles, blueprints, Regions), `GetLoadBalancerTlsPolicies` reports
/// it without reading or writing a stored resource, and
/// `CreateLoadBalancer$tlsPolicyName` names an entry of it — "Use the
/// GetLoadBalancerTlsPolicies action to get a list of TLS policy names that you
/// can specify".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadBalancerTlsPolicySpec {
    /// `LoadBalancerTlsPolicy$name` — "The name of the TLS security policy", and
    /// the value a load balancer's `tlsPolicyName` carries.
    pub name: &'static str,
    /// `LoadBalancerTlsPolicy$isDefault` — "A Boolean value that indicates
    /// whether the TLS security policy is the default", so exactly one entry of
    /// the catalog carries it.
    pub is_default: bool,
    /// `LoadBalancerTlsPolicy$protocols` — "The protocols used in a given TLS
    /// security policy", oldest first.
    pub protocols: &'static [&'static str],
    /// `LoadBalancerTlsPolicy$ciphers` — "The ciphers used by the TLS security
    /// policy", which "are listed in order of preference".
    pub ciphers: &'static [&'static str],
}

/// Every TLS security policy this service publishes, the default first and then
/// the forward-secrecy policy.
///
/// SOURCE: the Lightsail TLS security policies of the Developer Guide's
/// "Configuring TLS security policies on your Amazon Lightsail load balancers",
/// which the recorded `GetLoadBalancerTlsPolicies` answer reports — the default
/// `TLS-2016-08` policy and the restricted forward-secrecy (`FS`) policy, whose
/// name carries the protocol floor (`1-2`), the restricted cipher set (`Res`)
/// and the date of the policy. A policy the service does not publish has no row
/// here: a client offering one would configure a policy the service does not
/// have. Each row is the load-balancing service's own security policy of that
/// name, so its protocol versions and its ordered cipher suites are that
/// policy's.
pub const LOAD_BALANCER_TLS_POLICY_SPECS: &[LoadBalancerTlsPolicySpec] = &[
    LoadBalancerTlsPolicySpec {
        name: "TLS-2016-08",
        is_default: true,
        protocols: TLS_PROTOCOLS_1_0_AND_LATER,
        ciphers: TLS_DEFAULT_CIPHERS,
    },
    LoadBalancerTlsPolicySpec {
        name: "TLS-FS-1-2-Res-2019-08",
        is_default: false,
        protocols: TLS_PROTOCOLS_1_2_ONLY,
        ciphers: TLS_FORWARD_SECRECY_RESTRICTED_CIPHERS,
    },
];

/// The TLS version a protocol token names, without the wire prefix the token
/// carries: the token from its first digit onward (`Protocol-TLSv1.2` -> `1.2`).
pub(super) fn tls_protocol_version(protocol: &str) -> &str {
    protocol
        .find(|character: char| character.is_ascii_digit())
        .map_or(protocol, |first_digit| &protocol[first_digit..])
}

/// The TLS versions of a policy, as one English list: the items separated by
/// commas with `and` before the last, and the serial comma once there are three
/// or more.
pub(super) fn tls_version_list(versions: &[&str]) -> String {
    match versions {
        [] => String::new(),
        [only] => (*only).to_string(),
        [first, last] => format!("{first} and {last}"),
        [leading @ .., last] => format!("{}, and {last}", leading.join(", ")),
    }
}

/// The description a policy reports.
///
/// DERIVED from the policy's own protocol versions rather than carried per
/// entry, so a policy added to [`LOAD_BALANCER_TLS_POLICY_SPECS`] is described
/// the same way: the sentence states which TLS versions the policy enables, the
/// one fact the recorded descriptions carry — `Enables TLS versions 1, 1.1, and
/// 1.2` for the policy that admits all three, and `Enables only TLS version 1.2`
/// for one that admits a single version.
pub fn load_balancer_tls_policy_description(protocols: &[&str]) -> String {
    let versions: Vec<&str> = protocols
        .iter()
        .map(|protocol| tls_protocol_version(protocol))
        .collect();
    match versions.as_slice() {
        [] => "Enables no TLS version".to_string(),
        [only] => format!("Enables only TLS version {only}"),
        several => format!("Enables TLS versions {}", tls_version_list(several)),
    }
}

impl LoadBalancerTlsPolicySpec {
    /// This policy as the modelled `LoadBalancerTlsPolicy`.
    ///
    /// `description` is DERIVED by [`load_balancer_tls_policy_description`] from
    /// this entry's protocols rather than carried a second time.
    pub fn view(&self) -> LoadBalancerTlsPolicy {
        LoadBalancerTlsPolicy {
            name: Some(self.name.to_string()),
            is_default: Some(self.is_default),
            description: Some(load_balancer_tls_policy_description(self.protocols)),
            protocols: Some(self.protocols.iter().map(|p| (*p).to_string()).collect()),
            ciphers: Some(self.ciphers.iter().map(|c| (*c).to_string()).collect()),
        }
    }
}

/// The policies a `GetLoadBalancerTlsPolicies` call reports out of `specs`, in
/// `specs`' own order, so two reads answer identically.
///
/// Taking the catalog as an argument is what lets the reporting rule be
/// exercised against catalogs the published one is only one of.
pub fn load_balancer_tls_policies_from(
    specs: &[LoadBalancerTlsPolicySpec],
) -> Vec<LoadBalancerTlsPolicy> {
    specs.iter().map(LoadBalancerTlsPolicySpec::view).collect()
}

/// The policies a `GetLoadBalancerTlsPolicies` call reports:
/// [`load_balancer_tls_policies_from`] over this service's published catalog,
/// [`LOAD_BALANCER_TLS_POLICY_SPECS`].
pub fn load_balancer_tls_policies() -> Vec<LoadBalancerTlsPolicy> {
    load_balancer_tls_policies_from(LOAD_BALANCER_TLS_POLICY_SPECS)
}

/// One page of `reported`, by the same token walk every published catalog of this
/// service uses ([`catalog_page`]): a page token is the policy name a previous
/// page reported as `nextPageToken`.
pub fn load_balancer_tls_policy_page(
    reported: &[LoadBalancerTlsPolicy],
    page_size: usize,
    page_token: Option<&str>,
) -> Option<(Vec<LoadBalancerTlsPolicy>, Option<String>)> {
    catalog_page(reported, page_size, page_token, |policy| {
        policy.name.as_deref().map(std::borrow::Cow::Borrowed)
    })
}

/// The policy names `reported` holds, for a refusal that has to tell the caller
/// what a page token may name.
pub fn reported_load_balancer_tls_policy_names(reported: &[LoadBalancerTlsPolicy]) -> String {
    reported
        .iter()
        .filter_map(|policy| policy.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The published TLS security policy `name` names, or `None` when this service
/// publishes no such policy.
pub fn load_balancer_tls_policy_spec(name: &str) -> Option<LoadBalancerTlsPolicySpec> {
    LOAD_BALANCER_TLS_POLICY_SPECS
        .iter()
        .find(|spec| spec.name == name)
        .copied()
}
