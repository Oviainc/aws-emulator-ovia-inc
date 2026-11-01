// SPDX-License-Identifier: CC0-1.0

//! The FAMILIES of predefined security policies `DescribeSSLPolicies` publishes.
//!
//! A predefined policy is published as a SHAPE inside a FAMILY. The shape is the part of
//! the name that says what the policy offers — the lowest TLS version it negotiates
//! (`1-0`, `1-1`, `1-2`, `1-3`), `Ext0`/`Ext1`/`Ext2` for a widened cipher list, `Res`
//! for a narrowed one. The family is the constraint the whole generation carries: the
//! 2021-06 generation with none, the 2023-04 generation offering only what a
//! FIPS-validated module provides, the 2023-07 generation narrowed to the RFC 9151
//! profile's suites, the 2025-09 generation negotiating a post-quantum hybrid key
//! agreement.
//!
//! The rule under test is how a family's entry relates to the shape it mirrors:
//!
//! * the protocol versions come from the shape, so every `TLS13-1-N` policy negotiates
//!   TLS 1.3 and every version from 1.N up to 1.2;
//! * a FIPS entry offers its shape's suites less the ones whose primitives a validated
//!   module does not provide (the ChaCha20-Poly1305 suite is the only such suite in this
//!   catalogue), so no FIPS policy names it;
//! * a post-quantum entry offers EXACTLY the suites of the entry it mirrors, because the
//!   key agreement it changes is not a member of the reported shape
//!   (`smithy_query(query="shape",shape="SslPolicy")` carries `Name`, `SslProtocols`,
//!   `Ciphers` and `SupportedLoadBalancerTypes` and nothing else);
//! * an RFC 9151 entry offers only that profile's AES-256 suites, and its
//!   interoperability variants widen that list one step at a time, each a superset of the
//!   step before;
//! * and `Cipher$Priority` is the 1-based position of the cipher in the policy's own
//!   preference order, whatever the family.
//!
//! Every assertion below spans a TABLE of policies — one row per family member, including
//! the shapes no issue cited — and every request goes through the router exactly as a
//! client's would: awsQuery form body in, XML out. The cipher membership, the preference
//! order and the names themselves are the ones Elastic Load Balancing publishes for its
//! predefined policies ("Security policies" in the Application Load Balancers Guide and
//! in the Network Load Balancers Guide, which this operation's model documentation points
//! at).

use crate::catalog_reads::{describe, reported_policies, reported_policy_names};
use crate::provisioning_loop::{emulator, first_text, members, p};
use smithy_server_runtime::MultiServiceRouter;

/// The TLS 1.3 suite whose primitives a FIPS-validated module does not provide, so no
/// policy of a FIPS family offers it.
const NON_VALIDATED_SUITE: &str = "TLS_CHACHA20_POLY1305_SHA256";

/// Every predefined policy the service publishes, by family. Source: "Security policies"
/// in the Application Load Balancers Guide and in the Network Load Balancers Guide, the
/// two documents this operation's own model documentation points at.
const CATALOGUE: &[&str] = &[
    // The generation that predates TLS 1.3.
    "ELBSecurityPolicy-2016-08",
    "ELBSecurityPolicy-2015-05",
    "ELBSecurityPolicy-TLS-1-0-2015-04",
    "ELBSecurityPolicy-TLS-1-1-2017-01",
    "ELBSecurityPolicy-TLS-1-2-2017-01",
    "ELBSecurityPolicy-TLS-1-2-Ext-2018-06",
    "ELBSecurityPolicy-FS-2018-06",
    "ELBSecurityPolicy-FS-1-1-2019-08",
    "ELBSecurityPolicy-FS-1-2-2019-08",
    "ELBSecurityPolicy-FS-1-2-Res-2019-08",
    "ELBSecurityPolicy-FS-1-2-Res-2020-10",
    // The TLS 1.3 generation.
    "ELBSecurityPolicy-TLS13-1-0-2021-06",
    "ELBSecurityPolicy-TLS13-1-1-2021-06",
    "ELBSecurityPolicy-TLS13-1-2-2021-06",
    "ELBSecurityPolicy-TLS13-1-2-Ext1-2021-06",
    "ELBSecurityPolicy-TLS13-1-2-Ext2-2021-06",
    "ELBSecurityPolicy-TLS13-1-2-Res-2021-06",
    "ELBSecurityPolicy-TLS13-1-3-2021-06",
    // The FIPS generation.
    "ELBSecurityPolicy-TLS13-1-0-FIPS-2023-04",
    "ELBSecurityPolicy-TLS13-1-1-FIPS-2023-04",
    "ELBSecurityPolicy-TLS13-1-2-FIPS-2023-04",
    "ELBSecurityPolicy-TLS13-1-2-Ext0-FIPS-2023-04",
    "ELBSecurityPolicy-TLS13-1-2-Ext1-FIPS-2023-04",
    "ELBSecurityPolicy-TLS13-1-2-Ext2-FIPS-2023-04",
    "ELBSecurityPolicy-TLS13-1-2-Res-FIPS-2023-04",
    "ELBSecurityPolicy-TLS13-1-3-FIPS-2023-04",
    // The RFC 9151 generation.
    "ELBSecurityPolicy-TLS13-1-2-RFC9151-FIPS-2023-07",
    "ELBSecurityPolicy-TLS13-1-2-Ext0-RFC9151-FIPS-2023-07",
    "ELBSecurityPolicy-TLS13-1-2-RFC9151-INTEROP1-FIPS-2023-07",
    "ELBSecurityPolicy-TLS13-1-2-RFC9151-INTEROP2-FIPS-2023-07",
    "ELBSecurityPolicy-TLS13-1-2-RFC9151-INTEROP3-FIPS-2023-07",
    "ELBSecurityPolicy-TLS13-1-2-RFC9151-INTEROP4-FIPS-2023-07",
    "ELBSecurityPolicy-TLS13-1-3-RFC9151-FIPS-2023-07",
    // The post-quantum generation.
    "ELBSecurityPolicy-TLS13-1-0-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-0-FIPS-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-2-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-2-FIPS-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-2-Ext0-FIPS-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-2-Ext1-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-2-Ext1-FIPS-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-2-Ext2-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-2-Ext2-FIPS-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-2-Res-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-2-Res-FIPS-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-3-PQ-2025-09",
    "ELBSecurityPolicy-TLS13-1-3-FIPS-PQ-2025-09",
];

/// One policy as this operation reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    protocols: Vec<String>,
    /// `(Name, Priority)` of each cipher, in the reported order.
    ciphers: Vec<(String, String)>,
    load_balancer_types: Vec<String>,
}

impl Reported {
    /// Just the cipher names, in the reported order.
    fn cipher_names(&self) -> Vec<String> {
        self.ciphers.iter().map(|(name, _)| name.clone()).collect()
    }

    /// The cipher names, sorted — the SET of suites, independent of preference order.
    fn cipher_set(&self) -> Vec<String> {
        let mut names = self.cipher_names();
        names.sort();
        names
    }
}

/// Ask for ONE policy by name and read the document the service reports for it.
async fn policy(router: &MultiServiceRouter, name: &str) -> Reported {
    let reply = describe(
        router,
        "DescribeSSLPolicies",
        &[p("Names.member.1", name)],
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: expected 200 for a published policy, got {} body {}",
        reply.status, reply.body
    );
    let reported = reported_policies(&reply);
    assert_eq!(
        reported.len(),
        1,
        "case {name:?}: expected exactly that policy, got {reported:?}"
    );
    let member = &reported[0];
    Reported {
        protocols: members(member, "SslProtocols"),
        ciphers: members(member, "Ciphers")
            .iter()
            .map(|cipher| {
                (
                    first_text(cipher, "Name").unwrap_or_default(),
                    first_text(cipher, "Priority").unwrap_or_default(),
                )
            })
            .collect(),
        load_balancer_types: members(member, "SupportedLoadBalancerTypes"),
    }
}

/// The protocol versions a `TLS13-1-N` shape negotiates: TLS 1.3, plus every version from
/// 1.N up to 1.2. `None` for a name that is not of the TLS 1.3 generation.
fn protocols_of_shape(name: &str) -> Option<Vec<&'static str>> {
    let shape = name.strip_prefix("ELBSecurityPolicy-TLS13-")?;
    match shape.get(..3) {
        Some("1-0") => Some(vec!["TLSv1", "TLSv1.1", "TLSv1.2", "TLSv1.3"]),
        Some("1-1") => Some(vec!["TLSv1.1", "TLSv1.2", "TLSv1.3"]),
        Some("1-2") => Some(vec!["TLSv1.2", "TLSv1.3"]),
        Some("1-3") => Some(vec!["TLSv1.3"]),
        _ => None,
    }
}

#[tokio::test]
async fn every_family_of_the_catalogue_is_published_with_its_shapes_protocols() {
    let (router, _engine) = emulator();
    let whole = reported_policy_names(&describe(&router, "DescribeSSLPolicies", &[]).await);

    // The catalogue is exactly the published set: nothing missing, nothing invented.
    let mut reported = whole.clone();
    reported.sort();
    let mut expected: Vec<String> = CATALOGUE.iter().map(|name| name.to_string()).collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "case whole catalogue: expected every published policy exactly once, got {whole:?}"
    );

    for name in CATALOGUE {
        let reported = policy(&router, name).await;

        // The protocol versions follow from the shape, for every family that carries it.
        if let Some(expected) = protocols_of_shape(name) {
            assert_eq!(
                reported.protocols, expected,
                "case {name:?}: expected {expected:?}, got {:?}",
                reported.protocols
            );
        }

        // Priority is the 1-based position in the policy's own preference order, so the
        // priorities of a policy are exactly 1..=n with no gap and no repeat.
        let positions: Vec<String> = (1..=reported.ciphers.len())
            .map(|at| at.to_string())
            .collect();
        let priorities: Vec<String> = reported
            .ciphers
            .iter()
            .map(|(_, priority)| priority.clone())
            .collect();
        assert_eq!(
            priorities, positions,
            "case {name:?}: expected priorities 1..={}, got {priorities:?}",
            reported.ciphers.len()
        );

        // A suite is offered once: a caller reading the list by name would otherwise see
        // one of its two priorities.
        let mut unique = reported.cipher_set();
        unique.dedup();
        assert_eq!(
            unique.len(),
            reported.ciphers.len(),
            "case {name:?}: expected each suite once, got {:?}",
            reported.cipher_names()
        );

        // Every predefined policy can be set on an HTTPS listener of an Application Load
        // Balancer and on a TLS listener of a Network Load Balancer.
        assert_eq!(
            reported.load_balancer_types,
            vec!["application".to_string(), "network".to_string()],
            "case {name:?}: expected both TLS listener types, got {:?}",
            reported.load_balancer_types
        );

        // A FIPS family offers nothing a validated module cannot provide.
        if name.contains("-FIPS") {
            assert!(
                !reported.cipher_names().iter().any(|c| c == NON_VALIDATED_SUITE),
                "case {name:?}: a FIPS policy offers no {NON_VALIDATED_SUITE}, got {:?}",
                reported.cipher_names()
            );
        }
    }
}

#[tokio::test]
async fn a_fips_policy_offers_its_shapes_suites_less_the_non_validated_one() {
    let (router, _engine) = emulator();

    // One row per shape the FIPS family shares with the generation before it. `Ext0` has
    // no row: that shape is published only from the FIPS family on, so there is no
    // earlier entry to relate it to — the suite set test above covers it instead.
    let rows: &[(&str, &str)] = &[
        (
            "ELBSecurityPolicy-TLS13-1-0-FIPS-2023-04",
            "ELBSecurityPolicy-TLS13-1-0-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-1-FIPS-2023-04",
            "ELBSecurityPolicy-TLS13-1-1-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-FIPS-2023-04",
            "ELBSecurityPolicy-TLS13-1-2-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-Ext1-FIPS-2023-04",
            "ELBSecurityPolicy-TLS13-1-2-Ext1-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-Ext2-FIPS-2023-04",
            "ELBSecurityPolicy-TLS13-1-2-Ext2-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-Res-FIPS-2023-04",
            "ELBSecurityPolicy-TLS13-1-2-Res-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-3-FIPS-2023-04",
            "ELBSecurityPolicy-TLS13-1-3-2021-06",
        ),
    ];

    for (fips, shape) in rows {
        let case = format!("{fips} against {shape}");
        let validated = policy(&router, fips).await;
        let unconstrained = policy(&router, shape).await;

        let mut expected: Vec<String> = unconstrained
            .cipher_set()
            .into_iter()
            .filter(|suite| suite != NON_VALIDATED_SUITE)
            .collect();
        expected.sort();
        assert_eq!(
            validated.cipher_set(),
            expected,
            "case {case:?}: expected the shape's suites less {NON_VALIDATED_SUITE}, got {:?}",
            validated.cipher_names()
        );

        // The shape decides the protocol versions, so the constraint leaves them alone.
        assert_eq!(
            validated.protocols, unconstrained.protocols,
            "case {case:?}: expected {:?}, got {:?}",
            unconstrained.protocols, validated.protocols
        );
    }
}

#[tokio::test]
async fn a_post_quantum_policy_reports_exactly_the_policy_it_mirrors() {
    let (router, _engine) = emulator();

    // The post-quantum generation changes the key agreement, which this operation does
    // not report, so each of its entries must report the document of the entry whose
    // shape and constraint it mirrors — suites, preference order and protocols alike.
    let rows: &[(&str, &str)] = &[
        (
            "ELBSecurityPolicy-TLS13-1-0-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-0-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-2-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-Ext1-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-2-Ext1-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-Ext2-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-2-Ext2-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-Res-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-2-Res-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-3-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-3-2021-06",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-0-FIPS-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-0-FIPS-2023-04",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-FIPS-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-2-FIPS-2023-04",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-Ext0-FIPS-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-2-Ext0-FIPS-2023-04",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-Ext1-FIPS-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-2-Ext1-FIPS-2023-04",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-Ext2-FIPS-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-2-Ext2-FIPS-2023-04",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-2-Res-FIPS-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-2-Res-FIPS-2023-04",
        ),
        (
            "ELBSecurityPolicy-TLS13-1-3-FIPS-PQ-2025-09",
            "ELBSecurityPolicy-TLS13-1-3-FIPS-2023-04",
        ),
    ];

    for (post_quantum, mirrored) in rows {
        let case = format!("{post_quantum} against {mirrored}");
        let hybrid = policy(&router, post_quantum).await;
        let classic = policy(&router, mirrored).await;
        assert_eq!(
            hybrid, classic,
            "case {case:?}: expected the mirrored document, got {hybrid:?}"
        );
    }
}

#[tokio::test]
async fn the_rfc9151_family_narrows_to_the_profiles_suites_and_widens_by_steps() {
    let (router, _engine) = emulator();

    // The profile admits AES-256-GCM-SHA384 only, so the unextended entries of this
    // family name no AES-128 suite at all.
    for name in [
        "ELBSecurityPolicy-TLS13-1-2-RFC9151-FIPS-2023-07",
        "ELBSecurityPolicy-TLS13-1-3-RFC9151-FIPS-2023-07",
        "ELBSecurityPolicy-TLS13-1-2-Ext0-RFC9151-FIPS-2023-07",
    ] {
        let reported = policy(&router, name).await;
        for suite in reported.cipher_names() {
            assert!(
                suite.contains("AES256") || suite.contains("AES_256"),
                "case {name:?}: expected only the profile's AES-256 suites, got {suite:?}"
            );
        }
        assert_eq!(
            reported.cipher_names().first().map(String::as_str),
            Some("TLS_AES_256_GCM_SHA384"),
            "case {name:?}: expected the profile's TLS 1.3 suite first, got {:?}",
            reported.cipher_names()
        );
    }

    // The interoperability variants widen that list one step at a time: each step offers
    // every suite of the step before it, and strictly more of them.
    let ladder = [
        "ELBSecurityPolicy-TLS13-1-2-RFC9151-FIPS-2023-07",
        "ELBSecurityPolicy-TLS13-1-2-RFC9151-INTEROP1-FIPS-2023-07",
        "ELBSecurityPolicy-TLS13-1-2-RFC9151-INTEROP2-FIPS-2023-07",
        "ELBSecurityPolicy-TLS13-1-2-RFC9151-INTEROP3-FIPS-2023-07",
        "ELBSecurityPolicy-TLS13-1-2-RFC9151-INTEROP4-FIPS-2023-07",
    ];
    let mut previous: Option<(&str, Reported)> = None;
    for name in ladder {
        let step = policy(&router, name).await;

        // Every list of this family leads with the profile's own TLS 1.3 suite.
        assert_eq!(
            step.cipher_names().first().map(String::as_str),
            Some("TLS_AES_256_GCM_SHA384"),
            "case {name:?}: expected the profile's TLS 1.3 suite first, got {:?}",
            step.cipher_names()
        );

        if let Some((narrower_name, narrower)) = previous {
            let case = format!("{name} against {narrower_name}");
            for suite in narrower.cipher_names() {
                assert!(
                    step.cipher_names().contains(&suite),
                    "case {case:?}: expected the narrower step's {suite:?} to be offered too, got {:?}",
                    step.cipher_names()
                );
            }
            assert!(
                step.ciphers.len() > narrower.ciphers.len(),
                "case {case:?}: expected a wider step, got {} suites against {}",
                step.ciphers.len(),
                narrower.ciphers.len()
            );
        }
        previous = Some((name, step));
    }

    // Within a key size the family prefers the AEAD suite, and it prefers AES-256 to
    // AES-128 throughout — the reverse of the generations that lead with AES-128.
    let widest = policy(
        &router,
        "ELBSecurityPolicy-TLS13-1-2-RFC9151-INTEROP4-FIPS-2023-07",
    )
    .await;
    let names = widest.cipher_names();
    let first_128 = names
        .iter()
        .position(|suite| suite.contains("AES128") || suite.contains("AES_128"));
    let last_256 = names
        .iter()
        .rposition(|suite| suite.contains("AES256") || suite.contains("AES_256"));
    assert!(
        first_128.is_some() && last_256.is_some(),
        "case the widest interoperability step: expected both key sizes, got {names:?}"
    );
    let ephemeral_256 = names
        .iter()
        .rposition(|suite| suite.starts_with("ECDHE") && suite.contains("AES256"));
    let static_rsa_256 = names.iter().position(|suite| suite == "AES256-GCM-SHA384");
    assert!(
        ephemeral_256 < static_rsa_256,
        "case the widest interoperability step: expected the ephemeral AES-256 suites \
         ahead of the static-RSA one, got {names:?}"
    );
}
