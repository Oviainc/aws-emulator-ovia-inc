// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the BYOIP address-range slice: `ProvisionByoipCidr` and
//! `DescribeByoipCidrs`, driven over the wire the service actually speaks — `ec2Query`,
//! through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `ProvisionByoipCidr` BRINGS ONE RANGE FROM THE REQUEST and `DescribeByoipCidrs`
//!    REPORTS THE STORE: the block is stored in canonical form, every member the request
//!    sets round-trips, and the members the service owns (the address pool's id, the
//!    states) are derived rather than taken;
//! 2. a provisioned range is `pending-provision` and the CLOCK, not a caller, moves it to
//!    `provisioned` — or to `provisioned-not-publicly-advertisable` where the caller asked
//!    for an IPv6 range that is not publicly advertised;
//! 3. a `Cidr` outside what `ProvisionByoipCidrRequest$Cidr` publishes — not public
//!    address space, more specific than the family's most specific prefix, or not a CIDR
//!    block at all — is refused with EC2's own error code, and nothing is stored;
//! 4. a range that overlaps one already brought is refused, whichever way it overlaps;
//! 5. `DescribeByoipCidrs` pages the store in CIDR order behind an opaque `NextToken`, and
//!    refuses a token it did not mint;
//! 6. `DescribeByoipCidrs` is a REPORT: it changes nothing a later caller sees, and a
//!    `DryRun` provision stores nothing.
//!
//! Each table row is a distinct input along what the rule generalises over — the address
//! family, the prefix at the published bound and one past it, whether the range is public,
//! the shape of the overlap, the page size — so no row is the specification. The
//! non-public catalogs the refusal rows exercise come from the service's own reading of
//! the IANA special-purpose address registries (`crate::state::cidr_block_is_public`).

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    BYOIP_CIDR_STATE_PENDING_PROVISION, BYOIP_CIDR_STATE_PROVISIONED,
    BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE, LOCAL_ZONE_NETWORK_BORDER_GROUPS,
    TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code};

/// The Region every call here is signed for. It is the parent Region of two of the
/// documented Local Zone network border groups, which the border-group table needs.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn provision(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "ProvisionByoipCidr", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeByoipCidrs", params).await
}

/// The parameters that bring `cidr`, with whatever extra ones the case needs.
fn provision_params(
    cidr: &str,
    publicly_advertisable: Option<bool>,
    extra: Params,
) -> Params {
    let mut params = vec![p("Cidr", cidr)];
    if let Some(advertisable) = publicly_advertisable {
        params.push(p("PubliclyAdvertisable", advertisable.to_string()));
    }
    params.extend(extra);
    params
}

/// Bring `cidr` and answer the response.
async fn provision_cidr(
    router: &MultiServiceRouter,
    cidr: &str,
    publicly_advertisable: Option<bool>,
) -> (u16, String) {
    provision(
        router,
        &provision_params(cidr, publicly_advertisable, Vec::new()),
    )
    .await
}

/// One page of `DescribeByoipCidrs`: every range it reports as `(cidr, state)`, in the
/// order it reported them, plus the page's `NextToken`.
async fn described_page(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> (Vec<(String, String)>, Option<String>) {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeByoipCidrs, got {status} with body {body}"
    );
    let cidrs = elements(&body, "cidr");
    let states = elements(&body, "state");
    assert_eq!(
        cidrs.len(),
        states.len(),
        "case {case:?}: every reported range carries a state, got {body}"
    );
    (
        cidrs.into_iter().zip(states).collect(),
        element(&body, "nextToken"),
    )
}

/// The page size a request asks for when the case is not about paging: the largest the
/// model publishes (`DescribeByoipCidrsMaxResults`, `range 1..100`). `MaxResults` is
/// `@required`, so every call carries one.
const WHOLE_PAGE: &str = "100";

/// Every range the store holds, as `(cidr, state)` in the order `DescribeByoipCidrs`
/// reports them, read in one page.
async fn described(router: &MultiServiceRouter, case: &str) -> Vec<(String, String)> {
    described_page(router, case, &[p("MaxResults", WHOLE_PAGE)])
        .await
        .0
}

/// Is `candidate` an address-pool id this service minted for a range of `cidr`'s family:
/// the family's `ResourceType` (`ipv4pool-ec2` / `ipv6pool-ec2`) followed by 17 hex
/// digits, the long EC2 resource-id form?
fn is_pool_id(candidate: &str, cidr: &str) -> bool {
    let prefix = if cidr.contains(':') {
        "ipv6pool-ec2-"
    } else {
        "ipv4pool-ec2-"
    };
    candidate.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 17 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One accepted row: `(case, the Cidr sent, the PubliclyAdvertisable sent, the canonical
/// block the answer must carry, the state the clock owes it)`.
type AcceptedRow = (
    &'static str,
    &'static str,
    Option<bool>,
    &'static str,
    &'static str,
);

/// The accepted catalog: both families, each at its published most specific prefix and
/// less specific than it, with and without the advertisability flag, and with host bits
/// set so the canonicalisation is visible.
const ACCEPTED: [AcceptedRow; 7] = [
    (
        "an IPv4 range at the most specific published prefix",
        "81.2.69.0/24",
        None,
        "81.2.69.0/24",
        BYOIP_CIDR_STATE_PROVISIONED,
    ),
    (
        "a less specific IPv4 range",
        "45.33.0.0/16",
        None,
        "45.33.0.0/16",
        BYOIP_CIDR_STATE_PROVISIONED,
    ),
    (
        "an IPv4 range given with host bits set",
        "81.3.69.17/24",
        None,
        "81.3.69.0/24",
        BYOIP_CIDR_STATE_PROVISIONED,
    ),
    (
        "a publicly advertisable IPv6 range at /48",
        "2600:1f14:abc::/48",
        Some(true),
        "2600:1f14:abc::/48",
        BYOIP_CIDR_STATE_PROVISIONED,
    ),
    (
        "an IPv6 range left at the default advertisability",
        "2a05:d018:1234::/48",
        None,
        "2a05:d018:1234::/48",
        BYOIP_CIDR_STATE_PROVISIONED,
    ),
    (
        "an IPv6 range that is not publicly advertisable, at /56",
        "2a05:d018:1234:5600::/56",
        Some(false),
        "2a05:d018:1234:5600::/56",
        BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
    ),
    (
        "an IPv6 range given with host bits set",
        "2606:4700:10::1/48",
        Some(true),
        "2606:4700:10::/48",
        BYOIP_CIDR_STATE_PROVISIONED,
    ),
];

/// RULE 1 and RULE 2 over that catalog: every accepted range round-trips through
/// `DescribeByoipCidrs` in canonical form with a minted pool id, starts
/// `pending-provision`, and is moved on by the clock alone.
#[tokio::test]
async fn a_provisioned_range_round_trips_and_the_clock_provisions_it() {
    for (case, requested, publicly_advertisable, canonical, provisioned_state) in ACCEPTED {
        // One emulator per row: the rows are independent inputs, not a sequence, and an
        // empty store is what makes "exactly this range is reported" an assertion about
        // the row.
        let router = emulator();
        let (status, body) = provision_cidr(&router, requested, publicly_advertisable).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ProvisionByoipCidr, got {status} with body {body}"
        );
        let reported = element(&body, "cidr").unwrap_or_default();
        assert_eq!(
            reported, canonical,
            "case {case:?}: expected the canonical block {canonical:?}, got {reported:?}"
        );
        let state = element(&body, "state").unwrap_or_default();
        assert_eq!(
            state, BYOIP_CIDR_STATE_PENDING_PROVISION,
            "case {case:?}: expected a provision to answer {BYOIP_CIDR_STATE_PENDING_PROVISION:?}, got {state:?}"
        );
        let pool_id = element(&body, "poolId").unwrap_or_default();
        assert!(
            is_pool_id(&pool_id, canonical),
            "case {case:?}: expected a minted address-pool id, got {pool_id:?}"
        );
        let border_group = element(&body, "networkBorderGroup").unwrap_or_default();
        assert_eq!(
            border_group, REGION,
            "case {case:?}: a range provisioned without a network border group is \
             advertised from its Region, got {border_group:?}"
        );
        assert!(
            element(&body, "statusMessage").is_none(),
            "case {case:?}: a range still pending has no pool to report yet, got {body}"
        );

        // RULE 1: the describe reports the store — the same block, the same pool, nothing
        // else.
        let (page, next_token) =
            described_page(&router, case, &[p("MaxResults", WHOLE_PAGE)]).await;
        assert_eq!(
            page,
            vec![(
                canonical.to_string(),
                BYOIP_CIDR_STATE_PENDING_PROVISION.to_string()
            )],
            "case {case:?}: expected the one provisioned range, got {page:?}"
        );
        assert!(
            next_token.is_none(),
            "case {case:?}: one range fits one page, got the token {next_token:?}"
        );
        let (_, described_body) = describe(&router, &[p("MaxResults", WHOLE_PAGE)]).await;
        assert_eq!(
            element(&described_body, "poolId").unwrap_or_default(),
            pool_id,
            "case {case:?}: the describe reports the pool the provision minted, got {described_body}"
        );

        // RULE 2: the clock, and nothing the caller did, moves the range on — and the
        // pool id becomes the status message: "Upon success, contains the ID of the
        // address pool" (`ByoipCidr$StatusMessage`).
        past_the_transition().await;
        let (page, _) = described_page(&router, case, &[p("MaxResults", WHOLE_PAGE)]).await;
        assert_eq!(
            page,
            vec![(canonical.to_string(), provisioned_state.to_string())],
            "case {case:?}: expected the clock to reach {provisioned_state:?}, got {page:?}"
        );
        let (_, body) = describe(&router, &[p("MaxResults", WHOLE_PAGE)]).await;
        assert_eq!(
            element(&body, "statusMessage").unwrap_or_default(),
            pool_id,
            "case {case:?}: a provisioned range reports its pool id, got {body}"
        );
    }
}

/// One refused row: `(case, the Cidr sent, the PubliclyAdvertisable sent, the error
/// code)`.
type RefusedRow = (&'static str, &'static str, Option<bool>, &'static str);

/// The refusal catalog: one past each published prefix bound for each family and
/// advertisability, a member of each kind of non-public space, and values that are not
/// CIDR blocks at all.
const REFUSED: [RefusedRow; 19] = [
    (
        "an IPv4 range one bit more specific than the published /24",
        "81.2.69.0/25",
        None,
        "InvalidParameterValue",
    ),
    (
        "a single IPv4 address",
        "81.2.69.4/32",
        None,
        "InvalidParameterValue",
    ),
    (
        "a publicly advertisable IPv6 range one step more specific than /48",
        "2600:1f14:abc:100::/56",
        Some(true),
        "InvalidParameterValue",
    ),
    (
        "a not publicly advertisable IPv6 range more specific than /56",
        "2600:1f14:abc:100::/64",
        Some(false),
        "InvalidParameterValue",
    ),
    (
        "a private-use IPv4 range",
        "10.1.2.0/24",
        None,
        "InvalidParameterValue",
    ),
    (
        "another private-use IPv4 range",
        "192.168.1.0/24",
        None,
        "InvalidParameterValue",
    ),
    (
        "a shared-address-space IPv4 range",
        "100.64.0.0/24",
        None,
        "InvalidParameterValue",
    ),
    (
        "a loopback IPv4 range",
        "127.0.0.0/24",
        None,
        "InvalidParameterValue",
    ),
    (
        "a link-local IPv4 range",
        "169.254.1.0/24",
        None,
        "InvalidParameterValue",
    ),
    (
        "a documentation IPv4 range",
        "203.0.113.0/24",
        None,
        "InvalidParameterValue",
    ),
    (
        "an AS112 direct-delegation IPv4 range",
        "192.175.48.0/24",
        None,
        "InvalidParameterValue",
    ),
    (
        "an IPv4 range that only reaches into private-use space",
        "172.0.0.0/8",
        None,
        "InvalidParameterValue",
    ),
    (
        "a unique-local IPv6 range",
        "fd00:1234::/48",
        None,
        "InvalidParameterValue",
    ),
    (
        "a documentation IPv6 range",
        "2001:db8::/48",
        None,
        "InvalidParameterValue",
    ),
    (
        "an IPv6 range outside global unicast space",
        "fe80::/48",
        None,
        "InvalidParameterValue",
    ),
    (
        "a value that is not a CIDR block",
        "not-a-cidr",
        None,
        "InvalidParameterValue",
    ),
    (
        "an IPv4 address with no prefix length",
        "81.2.69.0",
        None,
        "InvalidParameterValue",
    ),
    (
        "an IPv4 prefix length past the address size",
        "81.2.69.0/33",
        None,
        "InvalidParameterValue",
    ),
    (
        "no cidr at all",
        "",
        None,
        "MissingParameter",
    ),
];

/// RULE 3 over that catalog: each refusal carries EC2's own code at HTTP 400 and leaves
/// the store empty.
#[tokio::test]
async fn a_cidr_outside_the_published_rules_is_refused_and_stores_nothing() {
    let router = emulator();
    for (case, requested, publicly_advertisable, code) in REFUSED {
        let (status, body) = provision_cidr(&router, requested, publicly_advertisable).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from ProvisionByoipCidr, got {status} with body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, code,
            "case {case:?}: expected the error code {code:?}, got {actual:?} in {body}"
        );
    }
    let page = described(&router, "after every refusal").await;
    assert!(
        page.is_empty(),
        "a refused ProvisionByoipCidr must store nothing, got {page:?}"
    );
}

/// A `CidrAuthorizationContext` is "a signed document that proves that you are authorized
/// to bring the specified IP address range to Amazon using BYOIP" — proof against the
/// range's RPKI ROA, which no emulator can reach. So a request that carries one is
/// accepted on the same terms as one that does not, and NOTHING from the document reaches
/// the stored range or the report: it is a credential, not data about the range.
#[tokio::test]
async fn a_cidr_authorization_context_is_accepted_and_never_reported() {
    let router = emulator();
    let message = "1|aws|123456789012|81.2.69.0/24|20260101|SHA256|RSAPSS";
    let signature = "ZXhhbXBsZS1zaWduYXR1cmU=";
    let params = provision_params(
        "81.2.69.0/24",
        None,
        vec![
            p("CidrAuthorizationContext.Message", message),
            p("CidrAuthorizationContext.Signature", signature),
        ],
    );
    let (status, body) = provision(&router, &params).await;
    assert_eq!(
        status, 200,
        "a signed authorization document is accepted, got {status} with body {body}"
    );
    let (_, described_body) = describe(&router, &[p("MaxResults", WHOLE_PAGE)]).await;
    for secret in [message, signature] {
        assert!(
            !described_body.contains(secret) && !body.contains(secret),
            "the authorization document must not be reported back, got {body} and {described_body}"
        );
    }
    let page = described(&router, "with an authorization document").await;
    assert_eq!(
        page,
        vec![(
            "81.2.69.0/24".to_string(),
            BYOIP_CIDR_STATE_PENDING_PROVISION.to_string()
        )],
        "the range is brought exactly as it is without the document, got {page:?}"
    );
}

/// RULE 1 for the members a request may set beside the block: the description, the pool's
/// tags and the network border group, each read back through `DescribeByoipCidrs`.
#[tokio::test]
async fn a_provision_round_trips_the_description_and_the_network_border_group() {
    // Every documented Local Zone network border group is a row: the ones of the signed
    // Region may be provisioned to, and one of another Region may not.
    for group in LOCAL_ZONE_NETWORK_BORDER_GROUPS {
        let case = format!("the network border group {group:?}");
        let router = emulator();
        let params = provision_params(
            "81.2.69.0/24",
            None,
            vec![
                p("Description", "edge pool"),
                p("NetworkBorderGroup", group),
                p("PoolTagSpecification.1.ResourceType", "ipv4pool-ec2"),
                p("PoolTagSpecification.1.Tag.1.Key", "Name"),
                p("PoolTagSpecification.1.Tag.1.Value", "edge"),
            ],
        );
        let (status, body) = provision(&router, &params).await;
        if group.starts_with(REGION) {
            assert_eq!(
                status, 200,
                "case {case:?}: a Local Zone group of the signed Region is allowed, got {status} with body {body}"
            );
            let (_, described_body) = describe(&router, &[p("MaxResults", WHOLE_PAGE)]).await;
            assert_eq!(
                element(&described_body, "networkBorderGroup").unwrap_or_default(),
                group,
                "case {case:?}: the network border group must round-trip, got {described_body}"
            );
            assert_eq!(
                element(&described_body, "description").unwrap_or_default(),
                "edge pool",
                "case {case:?}: the description must round-trip, got {described_body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a Local Zone group of another Region is refused, got {status} with body {body}"
            );
            let actual = error_code(&body);
            assert_eq!(
                actual, "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {actual:?} in {body}"
            );
            assert!(
                described(&router, &case).await.is_empty(),
                "case {case:?}: a refused provision must store nothing"
            );
        }
    }
}

/// One refused row of the member table: `(case, the parameters under test, the error
/// code)`.
type RefusedParamsRow = (&'static str, Params, &'static str);

/// RULE 3 for the members beside the block: a network border group that is not the
/// Region's, a Local Zone group for an IPv6 range, and a pool tag specification naming
/// anything but the pool this action creates.
#[tokio::test]
async fn a_member_outside_the_published_rules_is_refused_and_stores_nothing() {
    let local_zone = LOCAL_ZONE_NETWORK_BORDER_GROUPS
        .iter()
        .find(|group| group.starts_with(REGION))
        .copied()
        .expect("the signed Region has a documented Local Zone network border group");
    let rows: Vec<RefusedParamsRow> = vec![
        (
            "a network border group that is another Region",
            provision_params("81.2.69.0/24", None, vec![p("NetworkBorderGroup", "us-east-1")]),
            "InvalidParameterValue",
        ),
        (
            "a network border group that is no Region at all",
            provision_params("81.2.69.0/24", None, vec![p("NetworkBorderGroup", "nowhere")]),
            "InvalidParameterValue",
        ),
        (
            "a Local Zone network border group for an IPv6 range",
            provision_params(
                "2600:1f14:abc::/48",
                None,
                vec![p("NetworkBorderGroup", local_zone)],
            ),
            "InvalidParameterValue",
        ),
        (
            "a pool tag specification naming another resource type",
            provision_params(
                "81.2.69.0/24",
                None,
                vec![
                    p("PoolTagSpecification.1.ResourceType", "volume"),
                    p("PoolTagSpecification.1.Tag.1.Key", "Name"),
                    p("PoolTagSpecification.1.Tag.1.Value", "edge"),
                ],
            ),
            "InvalidParameterValue",
        ),
        (
            "a pool tag specification naming the other family's pool",
            provision_params(
                "81.2.69.0/24",
                None,
                vec![
                    p("PoolTagSpecification.1.ResourceType", "ipv6pool-ec2"),
                    p("PoolTagSpecification.1.Tag.1.Key", "Name"),
                    p("PoolTagSpecification.1.Tag.1.Value", "edge"),
                ],
            ),
            "InvalidParameterValue",
        ),
        (
            "a pool tag specification with no resource type",
            provision_params(
                "81.2.69.0/24",
                None,
                vec![
                    p("PoolTagSpecification.1.Tag.1.Key", "Name"),
                    p("PoolTagSpecification.1.Tag.1.Value", "edge"),
                ],
            ),
            "MissingParameter",
        ),
        (
            "a pool tag key in the reserved namespace",
            provision_params(
                "81.2.69.0/24",
                None,
                vec![
                    p("PoolTagSpecification.1.ResourceType", "ipv4pool-ec2"),
                    p("PoolTagSpecification.1.Tag.1.Key", "aws:created-by"),
                    p("PoolTagSpecification.1.Tag.1.Value", "me"),
                ],
            ),
            "InvalidParameterValue",
        ),
    ];

    let router = emulator();
    for (case, params, code) in rows {
        let (status, body) = provision(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from ProvisionByoipCidr, got {status} with body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, code,
            "case {case:?}: expected the error code {code:?}, got {actual:?} in {body}"
        );
    }
    let page = described(&router, "after every refusal").await;
    assert!(
        page.is_empty(),
        "a refused ProvisionByoipCidr must store nothing, got {page:?}"
    );
}

/// RULE 4: "The address range cannot overlap with another address range that you've
/// brought" — whichever way it overlaps, and only within one address family.
#[tokio::test]
async fn a_range_overlapping_one_already_brought_is_refused() {
    let router = emulator();
    for (cidr, advertisable) in [
        ("45.33.0.0/16", None),
        ("2600:1f14:abc::/48", Some(true)),
    ] {
        let (status, body) = provision_cidr(&router, cidr, advertisable).await;
        assert_eq!(
            status, 200,
            "the first provision of {cidr:?} must succeed, got {status} with body {body}"
        );
    }

    // Each row names the same addresses a different way: the exact repeat, a block inside
    // one already brought, and a block that contains one.
    let overlapping: Vec<(&str, &str, Option<bool>)> = vec![
        ("the exact IPv4 range again", "45.33.0.0/16", None),
        ("an IPv4 block inside one already brought", "45.33.7.0/24", None),
        ("an IPv4 block containing one already brought", "45.32.0.0/15", None),
        ("the exact IPv6 range again", "2600:1f14:abc::/48", Some(true)),
        (
            "an IPv6 block inside one already brought",
            "2600:1f14:abc:700::/56",
            Some(false),
        ),
    ];
    for (case, cidr, advertisable) in overlapping {
        let (status, body) = provision_cidr(&router, cidr, advertisable).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from ProvisionByoipCidr, got {status} with body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {actual:?} in {body}"
        );
    }

    // A disjoint block of either family is not an overlap: an IPv4 range can never meet
    // an IPv6 one.
    for (case, cidr, advertisable) in [
        ("a disjoint IPv4 range", "81.2.69.0/24", None),
        ("a disjoint IPv6 range", "2a05:d018:1234::/48", Some(true)),
    ] {
        let (status, body) = provision_cidr(&router, cidr, advertisable).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ProvisionByoipCidr, got {status} with body {body}"
        );
    }

    let reported: Vec<String> = described(&router, "after the overlaps")
        .await
        .into_iter()
        .map(|(cidr, _)| cidr)
        .collect();
    let mut expected = vec![
        "45.33.0.0/16".to_string(),
        "2600:1f14:abc::/48".to_string(),
        "81.2.69.0/24".to_string(),
        "2a05:d018:1234::/48".to_string(),
    ];
    // The store reports its ranges in CIDR order, which is the order the page cursor names
    // a position in.
    expected.sort();
    assert_eq!(
        reported, expected,
        "expected exactly the four accepted ranges in CIDR order, got {reported:?}"
    );
}

/// RULE 5: every page size pages the same CIDR-ordered store, each page is at most that
/// size, and the last page carries no token.
#[tokio::test]
async fn describe_pages_the_store_in_cidr_order_behind_an_opaque_token() {
    let router = emulator();
    let brought = [
        ("129.21.0.0/16", None),
        ("45.33.0.0/16", None),
        ("81.2.69.0/24", None),
        ("2600:1f14:abc::/48", Some(true)),
        ("2a05:d018:1234::/48", Some(false)),
    ];
    for (cidr, advertisable) in brought {
        let (status, body) = provision_cidr(&router, cidr, advertisable).await;
        assert_eq!(
            status, 200,
            "the provision of {cidr:?} must succeed, got {status} with body {body}"
        );
    }
    let mut expected: Vec<String> = brought
        .iter()
        .map(|(cidr, _)| (*cidr).to_string())
        .collect();
    expected.sort();

    // A page size per row: one at a time, a size that does not divide the total, the exact
    // total, one past it, and the model's published ceiling.
    for page_size in [1, 2, 3, 5, 6, 100] {
        let case = format!("MaxResults={page_size}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params: Params = vec![p("MaxResults", page_size.to_string())];
            if let Some(cursor) = token.as_deref() {
                params.push(p("NextToken", cursor));
            }
            let (page, next) = described_page(&router, &case, &params).await;
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} ranges, got {}",
                page.len()
            );
            collected.extend(page.into_iter().map(|(cidr, _)| cidr));
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging must terminate, it has made {pages} pages"
            );
            match next {
                None => break,
                Some(next) => token = Some(next),
            }
        }
        assert_eq!(
            collected, expected,
            "case {case:?}: paging must report every range once, in CIDR order"
        );
    }

    // `MaxResults` is `@required`, so a request that leaves it out is refused before the
    // operation runs rather than answered with a page size of the emulator's choosing.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 400,
        "a describe with no MaxResults is refused, got {status} with body {body}"
    );

    // A token this service did not mint is refused rather than read as a cursor. The
    // forged ones are built the way the sibling suites build theirs (`describe_vpcs`,
    // `describe_subnets`): hex, through the same `hex` crate the service encodes with.
    for (case, token) in [
        ("a token that is not even hex", "not-a-token".to_string()),
        ("a hex token with no marker", ::hex::encode("45.33.0.0/16")),
        (
            "a hex token whose checksum does not match",
            ::hex::encode("ec2-1:0000000000000000:45.33.0.0/16"),
        ),
    ] {
        let (status, body) = describe(
            &router,
            &[p("MaxResults", WHOLE_PAGE), p("NextToken", token.clone())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DescribeByoipCidrs, got {status} with body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {actual:?} in {body}"
        );
    }
}

/// RULE 6: the describe is a report — two reads answer the same thing — and a `DryRun`
/// provision stores nothing while a `DryRun` describe reads nothing.
#[tokio::test]
async fn describe_reports_without_changing_anything_and_a_dry_run_writes_nothing() {
    let router = emulator();

    // An account that has brought nothing has nothing to report, and that is a 200.
    let (page, token) = described_page(&router, "an empty store", &[p("MaxResults", "5")]).await;
    assert!(
        page.is_empty() && token.is_none(),
        "an account that has brought no range reports none, got {page:?} and {token:?}"
    );

    let (status, body) = provision_cidr(&router, "81.2.69.0/24", None).await;
    assert_eq!(
        status, 200,
        "the first provision must succeed, got {status} with body {body}"
    );

    // Two reads of the same store answer the same thing: a read moves nothing on.
    let first = described(&router, "the first read").await;
    let second = described(&router, "the second read").await;
    assert_eq!(
        first, second,
        "two reads of one store must agree, got {first:?} then {second:?}"
    );

    // "If you have the required permissions, the error response is DryRunOperation"
    // (`ProvisionByoipCidrRequest$DryRun`), and the range is NOT brought.
    let (status, body) = provision(
        &router,
        &provision_params("45.33.0.0/16", None, vec![p("DryRun", "true")]),
    )
    .await;
    assert_eq!(
        status, 412,
        "a dry run that would have succeeded answers HTTP 412, got {status} with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, "DryRunOperation",
        "expected DryRunOperation, got {actual:?} in {body}"
    );
    let after = described(&router, "after the dry run").await;
    assert_eq!(
        after, first,
        "a dry-run provision must store nothing, got {after:?}"
    );

    // The describe's own dry run reports the outcome and reads nothing.
    let (status, body) = describe(
        &router,
        &[p("MaxResults", WHOLE_PAGE), p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "a dry-run describe answers HTTP 412, got {status} with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, "DryRunOperation",
        "expected DryRunOperation, got {actual:?} in {body}"
    );
}
