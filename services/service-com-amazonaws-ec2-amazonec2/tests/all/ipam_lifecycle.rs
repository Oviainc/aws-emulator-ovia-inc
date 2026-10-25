// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateIpam` and the `DescribeIpams` that reads it back, driven
//! over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=CreateIpam`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! The rule under test is that `CreateIpam` CREATES THE IPAM THE REQUEST DESCRIBES, with
//! everything the service itself owns derived rather than echoed: the IPAM carries the
//! description, tier, metered account, private-GUA setting, operating Regions and tags the
//! request asked for (each absent member taking the default its doc gives), its home Region
//! is always one of its operating Regions, it reports the two default scopes, the default
//! resource discovery and that discovery's association AWS creates with it, it is answered
//! in the non-terminal `create-in-progress` state the clock then owes `create-complete`,
//! and every member of it reads back through `DescribeIpams`. Every table row is a distinct
//! input along what that generalises over — the presence of each optional member, the enum
//! value chosen, the number of operating Regions, the tag count — so the one case a
//! recording happens to contain is one row among many rather than the specification.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region.
const REGION: &str = "us-east-1";

/// Longer than the span the clock owes a `create-in-progress` IPAM
/// ([`ec2::state::TRANSITION_DELAY`]), so a read after this much has certainly passed the
/// due instant.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// One row of a refusal table: the case, the request's form fields, and the EC2 error code
/// the service answers.
type RefusalCase = (&'static str, Vec<Param>, &'static str);

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it: the router and
/// the engine its store lives in.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `CreateIpam` call carrying `params`, as its HTTP status and XML body.
async fn create_ipam(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateIpam", params).await
}

/// One `DescribeIpams` call carrying `params`, as its HTTP status and XML body.
async fn describe_ipams(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeIpams", params).await
}

/// `CreateIpam` requiring success, and the body it answered.
async fn create_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = create_ipam(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateIpam, got {status} with body {body}"
    );
    body
}

/// `DescribeIpams` requiring success, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe_ipams(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeIpams, got {status} with body {body}"
    );
    body
}

/// A scalar member of the one IPAM a response carries, or `None` when the IPAM does not
/// report it. Only DIRECT children of the `<ipam>` element are read, so a member of a
/// nested `operatingRegionSet` or `tagSet` item is never mistaken for one of the IPAM's.
fn ipam_member(case: &str, body: &str, name: &str) -> Option<String> {
    let document = document(case, body);
    let ipam = document
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM (no <ipamId>) in {body}"));
    child_text(ipam, name)
}

/// A scalar member of the one IPAM a response carries, or a panic naming the case.
fn required_member(case: &str, body: &str, name: &str) -> String {
    ipam_member(case, body, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the IPAM in {body}"))
}

/// The `regionName`s of the one IPAM a response carries, sorted: "operating Regions are
/// Amazon Web Services Regions where the IPAM is allowed to manage IP address CIDRs"
/// (`Ipam$OperatingRegions`) — a SET of Regions, so the order they are reported in is not
/// part of the rule, but how MANY times each appears is: sorting rather than collecting into
/// a set is what lets a duplicate fail the comparison.
fn operating_regions(case: &str, body: &str) -> Vec<String> {
    let document = document(case, body);
    let Some(ipam) = document
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
    else {
        return Vec::new();
    };
    let Some(set) = child(ipam, "operatingRegionSet") else {
        return Vec::new();
    };
    let mut regions: Vec<String> = set
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .filter_map(|item| child_text(item, "regionName"))
        .collect();
    regions.sort();
    regions
}

/// The tags of the one IPAM a response carries, in the order it reported them.
fn ipam_tags(case: &str, body: &str) -> Vec<(String, String)> {
    let document = document(case, body);
    let Some(ipam) = document
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
    else {
        return Vec::new();
    };
    let Some(set) = child(ipam, "tagSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// The form fields for one `ipam` tag specification over `tags`.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param("TagSpecification.1.ResourceType", "ipam")];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// The form fields for an `OperatingRegions` list of `regions`.
fn operating_region_params(regions: &[&str]) -> Vec<Param> {
    regions
        .iter()
        .enumerate()
        .map(|(index, region)| {
            param(
                &format!("OperatingRegion.{}.RegionName", index + 1),
                region,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// What the create stores
// ---------------------------------------------------------------------------

/// The rule: the IPAM `CreateIpam` stores carries exactly what the request described, each
/// member the request omitted taking the default its own documentation gives (`free` tier,
/// `ipam-owner` metered account, private GUA disabled, no description, no tags), its
/// operating Regions are the SET of the Regions named plus its own home Region, and every
/// member of it reads back unchanged through `DescribeIpams`.
///
/// The rows differ along what that generalises over: whether each optional member is given,
/// which value of each two-member enum is chosen, how many operating Regions are named
/// (none, one, three, and one already in the set) and how many tags are assigned (none, one,
/// three).
#[tokio::test]
async fn create_ipam_stores_what_the_request_describes_and_round_trips_through_describe_ipams() {
    let (router, _engine) = emulator();
    // (case, extra form fields, expected description, tier, metered account, private GUA,
    //  operating Regions beyond the home Region, tags)
    #[allow(clippy::type_complexity)]
    let cases: [(
        &str,
        Vec<Param>,
        Option<&str>,
        &str,
        &str,
        &str,
        Vec<&str>,
        Vec<(&str, &str)>,
    ); 8] = [
        (
            "nothing but the action: every default",
            Vec::new(),
            None,
            "free",
            "ipam-owner",
            "false",
            Vec::new(),
            Vec::new(),
        ),
        (
            "a description",
            vec![param("Description", "corp address space")],
            Some("corp address space"),
            "free",
            "ipam-owner",
            "false",
            Vec::new(),
            Vec::new(),
        ),
        (
            "the advanced tier",
            vec![param("Tier", "advanced")],
            None,
            "advanced",
            "ipam-owner",
            "false",
            Vec::new(),
            Vec::new(),
        ),
        (
            "metered to the resource owner, GUA on",
            vec![
                param("MeteredAccount", "resource-owner"),
                param("EnablePrivateGua", "true"),
            ],
            None,
            "free",
            "resource-owner",
            "true",
            Vec::new(),
            Vec::new(),
        ),
        (
            "one further operating Region, one tag",
            {
                let mut params = operating_region_params(&["eu-west-1"]);
                params.extend(tag_params(&[("Name", "shared")]));
                params
            },
            None,
            "free",
            "ipam-owner",
            "false",
            vec!["eu-west-1"],
            vec![("Name", "shared")],
        ),
        (
            "three further operating Regions, three tags, every member set",
            {
                let mut params = vec![
                    param("Description", "global"),
                    param("Tier", "advanced"),
                    param("MeteredAccount", "resource-owner"),
                    param("EnablePrivateGua", "true"),
                ];
                params.extend(operating_region_params(&[
                    "eu-west-1",
                    "ap-northeast-2",
                    "ap-south-1",
                ]));
                params.extend(tag_params(&[
                    ("Name", "global"),
                    ("Team", "network"),
                    ("Stage", "prod"),
                ]));
                params
            },
            Some("global"),
            "advanced",
            "resource-owner",
            "true",
            vec!["eu-west-1", "ap-northeast-2", "ap-south-1"],
            vec![("Name", "global"), ("Team", "network"), ("Stage", "prod")],
        ),
        (
            // The home Region is already in the set, so naming it adds nothing.
            "the home Region named explicitly",
            operating_region_params(&[REGION]),
            None,
            "free",
            "ipam-owner",
            "false",
            Vec::new(),
            Vec::new(),
        ),
        (
            "one Region named twice",
            operating_region_params(&["eu-west-1", "eu-west-1"]),
            None,
            "free",
            "ipam-owner",
            "false",
            vec!["eu-west-1"],
            Vec::new(),
        ),
    ];

    let account = engine::emulator_account_id();
    let mut ids = BTreeSet::new();
    let mut scope_ids = BTreeSet::new();
    for (case, params, description, tier, metered_account, private_gua, regions, tags) in cases {
        let created = create_ok(case, &router, &params).await;
        let ipam_id = required_member(case, &created, "ipamId");

        // Read the whole IPAM back through the describe, and assert every member on BOTH
        // bodies: the create's answer and the stored record must be the same IPAM.
        let read = describe_ok(case, &router, &[param("IpamId.1", &ipam_id)]).await;
        for (source, body) in [("CreateIpam", &created), ("DescribeIpams", &read)] {
            let where_at = format!("{case} ({source})");
            let at = where_at.as_str();

            assert!(
                is_long_id("ipam", &required_member(at, body, "ipamId")),
                "case {at:?}: the IPAM id is an EC2 long id, got {body}"
            );
            assert_eq!(
                required_member(at, body, "ipamRegion"),
                REGION,
                "case {at:?}: the home Region is the Region the call was signed for, \
                 got {body}"
            );
            assert_eq!(
                required_member(at, body, "ownerId"),
                account,
                "case {at:?}: the owner is this emulator's account, got {body}"
            );
            assert_eq!(
                required_member(at, body, "ipamArn"),
                format!("arn:aws:ec2::{account}:ipam/{ipam_id}"),
                "case {at:?}: the ARN names the IPAM with no Region segment, got {body}"
            );
            assert_eq!(
                required_member(at, body, "tier"),
                tier,
                "case {at:?}: expected tier {tier:?}, got {body}"
            );
            assert_eq!(
                required_member(at, body, "meteredAccount"),
                metered_account,
                "case {at:?}: expected metered account {metered_account:?}, got {body}"
            );
            assert_eq!(
                required_member(at, body, "enablePrivateGua"),
                private_gua,
                "case {at:?}: expected enablePrivateGua {private_gua:?}, got {body}"
            );
            assert_eq!(
                ipam_member(at, body, "description").as_deref(),
                description,
                "case {at:?}: expected description {description:?}, got {body}"
            );
            assert_eq!(
                ipam_tags(at, body),
                tags.iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect::<Vec<_>>(),
                "case {at:?}: expected the tags of the ipam tag specification, got {body}"
            );

            // "The operating Regions for the IPAM": the set of the Regions the request
            // named and the home Region, each exactly once however often it was named.
            let mut expected_regions: Vec<String> = regions
                .iter()
                .map(|region| (*region).to_string())
                .chain(std::iter::once(REGION.to_string()))
                .collect();
            expected_regions.sort();
            assert_eq!(
                operating_regions(at, body),
                expected_regions,
                "case {at:?}: expected the operating Regions {expected_regions:?}, got {body}"
            );

            // The resources created WITH the IPAM: two default scopes, one default resource
            // discovery and the association of that discovery with this IPAM.
            let public_scope = required_member(at, body, "publicDefaultScopeId");
            let private_scope = required_member(at, body, "privateDefaultScopeId");
            assert!(
                is_long_id("ipam-scope", &public_scope)
                    && is_long_id("ipam-scope", &private_scope),
                "case {at:?}: both default scope ids are IPAM scope ids, got {body}"
            );
            assert_ne!(
                public_scope, private_scope,
                "case {at:?}: the public and private default scopes are two scopes, got {body}"
            );
            assert_eq!(
                required_member(at, body, "scopeCount"),
                "2",
                "case {at:?}: a fresh IPAM has its two default scopes, got {body}"
            );
            assert!(
                is_long_id(
                    "ipam-res-disco",
                    &required_member(at, body, "defaultResourceDiscoveryId")
                ),
                "case {at:?}: the default resource discovery id is a discovery id, got {body}"
            );
            assert!(
                is_long_id(
                    "ipam-res-disco-assoc",
                    &required_member(at, body, "defaultResourceDiscoveryAssociationId")
                ),
                "case {at:?}: the default association id is an association id, got {body}"
            );
            assert_eq!(
                required_member(at, body, "resourceDiscoveryAssociationCount"),
                "1",
                "case {at:?}: a fresh IPAM has exactly its own discovery associated, \
                 got {body}"
            );
            assert_eq!(
                ipam_member(at, body, "stateMessage"),
                None,
                "case {at:?}: nothing went wrong, so there is no state message, got {body}"
            );
            scope_ids.insert(public_scope);
            scope_ids.insert(private_scope);
        }

        assert!(
            ids.insert(ipam_id.clone()),
            "case {case:?}: every create mints a fresh IPAM id, but {ipam_id} was seen twice"
        );
    }

    assert_eq!(
        scope_ids.len(),
        2 * ids.len(),
        "every IPAM has two default scopes of its own, got {scope_ids:?} for {ids:?}"
    );

    // Everything created above is reported by an unnarrowed describe, and nothing else is:
    // a fresh account holds no IPAM until one is created.
    let all = describe_ok("the whole store", &router, &[]).await;
    assert_eq!(
        elements(&all, "ipamId").into_iter().collect::<BTreeSet<_>>(),
        ids,
        "the describe reports exactly the IPAMs that were created, got {all}"
    );
}

/// The rule: an IPAM is answered in the non-terminal `create-in-progress` state and reaches
/// `create-complete` once the clock has made that move due — a caller acting straight after
/// the create still sees `create-in-progress`, and a read never brings the move forward.
/// The scopes, the resource discovery and the association created with it move on the same
/// instant, which is checked in the store because their own describes are not implemented
/// yet.
#[tokio::test]
async fn an_ipam_and_everything_created_with_it_complete_only_once_the_clock_says_so() {
    let (router, engine) = emulator();
    let case = "a plain create";
    let created = create_ok(case, &router, &[]).await;
    let ipam_id = required_member(case, &created, "ipamId");

    assert_eq!(
        required_member(case, &created, "state"),
        "create-in-progress",
        "case {case:?}: a caller acting straight after CreateIpam still sees \
         create-in-progress, got {created}"
    );
    // Read again straight away: the state is a property of the clock, not of being read.
    let read = describe_ok(case, &router, &[param("IpamId.1", &ipam_id)]).await;
    assert_eq!(
        required_member(case, &read, "state"),
        "create-in-progress",
        "case {case:?}: a read before the transition is due must not bring it forward, \
         got {read}"
    );

    // The records created with the IPAM, in the store the handlers share.
    let state = ec2::state::service_state(&*engine).expect("the EC2 store");
    let (scopes, discovery, association) = {
        let store = state.lock();
        let ipam = store
            .ipams
            .get(&ipam_id)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the IPAM is not stored"));
        (
            [
                ipam.public_default_scope_id.clone().unwrap_or_default(),
                ipam.private_default_scope_id.clone().unwrap_or_default(),
            ],
            ipam.default_resource_discovery_id.clone().unwrap_or_default(),
            ipam.default_resource_discovery_association_id
                .clone()
                .unwrap_or_default(),
        )
    };
    {
        let store = state.lock();
        for scope_id in &scopes {
            let scope = store
                .ipam_scopes
                .get(scope_id)
                .unwrap_or_else(|| panic!("case {case:?}: scope {scope_id} is not stored"));
            assert_eq!(
                scope.state.as_deref(),
                Some("create-in-progress"),
                "case {case:?}: scope {scope_id} comes up with the IPAM, got {scope:?}"
            );
            assert_eq!(
                scope.is_default,
                Some(true),
                "case {case:?}: scope {scope_id} is a default scope, got {scope:?}"
            );
            assert_eq!(
                scope.ipam_region.as_deref(),
                Some(REGION),
                "case {case:?}: scope {scope_id} is in the IPAM's home Region, got {scope:?}"
            );
        }
        let stored_discovery = store
            .ipam_resource_discoveries
            .get(&discovery)
            .unwrap_or_else(|| panic!("case {case:?}: discovery {discovery} is not stored"));
        assert_eq!(
            stored_discovery.state.as_deref(),
            Some("create-in-progress"),
            "case {case:?}: the default resource discovery comes up with the IPAM, \
             got {stored_discovery:?}"
        );
        let stored_association = store
            .ipam_resource_discovery_associations
            .get(&association)
            .unwrap_or_else(|| panic!("case {case:?}: association {association} is not stored"));
        assert_eq!(
            stored_association.state.as_deref(),
            Some("associate-in-progress"),
            "case {case:?}: the default association is being made, got {stored_association:?}"
        );
        assert_eq!(
            stored_association.ipam_id.as_deref(),
            Some(ipam_id.as_str()),
            "case {case:?}: the association names this IPAM, got {stored_association:?}"
        );
        assert_eq!(
            stored_association.resource_discovery_status.as_deref(),
            Some("active"),
            "case {case:?}: the IPAM's own discovery is readable, got {stored_association:?}"
        );
    }

    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let read = describe_ok(case, &router, &[param("IpamId.1", &ipam_id)]).await;
    assert_eq!(
        required_member(case, &read, "state"),
        "create-complete",
        "case {case:?}: an IPAM completes on its own once the transition is due, got {read}"
    );
    let store = state.lock();
    for scope_id in &scopes {
        assert_eq!(
            store
                .ipam_scopes
                .get(scope_id)
                .and_then(|scope| scope.state.clone())
                .as_deref(),
            Some("create-complete"),
            "case {case:?}: scope {scope_id} completes with the IPAM"
        );
    }
    assert_eq!(
        store
            .ipam_resource_discoveries
            .get(&discovery)
            .and_then(|discovery| discovery.state.clone())
            .as_deref(),
        Some("create-complete"),
        "case {case:?}: the default resource discovery completes with the IPAM"
    );
    assert_eq!(
        store
            .ipam_resource_discovery_associations
            .get(&association)
            .and_then(|association| association.state.clone())
            .as_deref(),
        Some("associate-complete"),
        "case {case:?}: the default association completes with the IPAM"
    );
}

/// The rule: "a unique, case-sensitive identifier that you provide to ensure the idempotency
/// of the request" (`CreateIpamRequest$ClientToken`) — one token names the one IPAM it
/// minted, so a repeat of the request reports that IPAM and creates nothing, while a
/// different token creates a second one.
#[tokio::test]
async fn a_client_token_makes_create_ipam_idempotent() {
    let (router, _engine) = emulator();
    // (case, token)
    let cases = [
        ("a uuid-shaped token", "6f3c9a1e-0b1d-4b7f-9f14-2a7e2a0f9c11"),
        ("a short token", "t1"),
        (
            "a token of the longest accepted length",
            &"k".repeat(ec2::state::MAX_CLIENT_TOKEN_CHARS),
        ),
    ];

    let mut minted = BTreeSet::new();
    for (case, token) in cases {
        let first = create_ok(case, &router, &[param("ClientToken", token)]).await;
        let second = create_ok(case, &router, &[param("ClientToken", token)]).await;
        let first_id = required_member(case, &first, "ipamId");
        let second_id = required_member(case, &second, "ipamId");
        assert_eq!(
            first_id, second_id,
            "case {case:?}: a replay reports the IPAM the token minted, got {second}"
        );
        assert!(
            minted.insert(first_id),
            "case {case:?}: a different token mints a different IPAM"
        );
    }

    let all = describe_ok("after the replays", &router, &[]).await;
    assert_eq!(
        elements(&all, "ipamId").into_iter().collect::<BTreeSet<_>>(),
        minted,
        "one IPAM per distinct token and no more, got {all}"
    );

    // A token is idempotent for the REQUEST it was first given with. Each row replays one
    // token with exactly one member changed — a member supplied, a member dropped, an enum
    // member swapped, a Region added, a tag value changed — and every one of them asks for
    // an IPAM other than the one the token minted.
    let token = "mismatch-token";
    let first = create_ok(
        "the first request of the mismatch token",
        &router,
        &{
            let mut params = vec![
                param("ClientToken", token),
                param("Description", "first request"),
                param("Tier", "advanced"),
            ];
            params.extend(operating_region_params(&["eu-west-1"]));
            params.extend(tag_params(&[("Name", "first")]));
            params
        },
    )
    .await;
    let token_ipam = required_member("the first request of the mismatch token", &first, "ipamId");
    let replay = |changed: Vec<Param>| {
        let mut params = vec![param("ClientToken", token)];
        params.extend(changed);
        params
    };
    let mismatches: Vec<(&str, Vec<Param>)> = vec![
        (
            "a different description",
            replay({
                let mut params = vec![param("Description", "second request"), param("Tier", "advanced")];
                params.extend(operating_region_params(&["eu-west-1"]));
                params.extend(tag_params(&[("Name", "first")]));
                params
            }),
        ),
        (
            "the description dropped",
            replay({
                let mut params = vec![param("Tier", "advanced")];
                params.extend(operating_region_params(&["eu-west-1"]));
                params.extend(tag_params(&[("Name", "first")]));
                params
            }),
        ),
        (
            "the other member of the tier enum",
            replay({
                let mut params = vec![param("Description", "first request"), param("Tier", "free")];
                params.extend(operating_region_params(&["eu-west-1"]));
                params.extend(tag_params(&[("Name", "first")]));
                params
            }),
        ),
        (
            "one more operating Region",
            replay({
                let mut params = vec![param("Description", "first request"), param("Tier", "advanced")];
                params.extend(operating_region_params(&["eu-west-1", "us-west-2"]));
                params.extend(tag_params(&[("Name", "first")]));
                params
            }),
        ),
        (
            "a different tag value",
            replay({
                let mut params = vec![param("Description", "first request"), param("Tier", "advanced")];
                params.extend(operating_region_params(&["eu-west-1"]));
                params.extend(tag_params(&[("Name", "second")]));
                params
            }),
        ),
    ];
    for (case, params) in mismatches {
        let (status, body) = create_ipam(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token replayed with a different parameter is refused, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected the idempotency mismatch code, got {body}"
        );
    }

    // The home Region is the Region the request was signed for, so the same body sent to
    // another Region asks for an IPAM homed there, not for this one.
    let (status, body) = super::ec2_query::call(
        &router,
        "us-west-2",
        "CreateIpam",
        &{
            let mut params = vec![
                param("ClientToken", token),
                param("Description", "first request"),
                param("Tier", "advanced"),
            ];
            params.extend(operating_region_params(&["eu-west-1"]));
            params.extend(tag_params(&[("Name", "first")]));
            params
        },
    )
    .await;
    assert_eq!(
        status, 400,
        "a token replayed from another Region is refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::IDEMPOTENT_PARAMETER_MISMATCH,
        "a token replayed from another Region is refused as a mismatch, got {body}"
    );

    // The same request replayed still answers with the IPAM the token minted, and no
    // mismatch above created a second one.
    let again = create_ok(
        "the first request again",
        &router,
        &{
            let mut params = vec![
                param("ClientToken", token),
                param("Description", "first request"),
                param("Tier", "advanced"),
            ];
            params.extend(operating_region_params(&["eu-west-1"]));
            params.extend(tag_params(&[("Name", "first")]));
            params
        },
    )
    .await;
    assert_eq!(
        required_member("the first request again", &again, "ipamId"),
        token_ipam,
        "a replay of the same request reports the IPAM the token minted, got {again}"
    );
    minted.insert(token_ipam);
    let all = describe_ok("after the mismatches", &router, &[]).await;
    assert_eq!(
        elements(&all, "ipamId").into_iter().collect::<BTreeSet<_>>(),
        minted,
        "a refused replay stores nothing, got {all}"
    );
}

// ---------------------------------------------------------------------------
// What the create refuses
// ---------------------------------------------------------------------------

/// The rule: a request the service cannot satisfy is refused with the EC2 error code for
/// what is wrong with it, and nothing is stored — a Region name that is not one is
/// `InvalidParameterValue`, an operating Region with no name is `MissingParameter`, a value
/// outside a modelled enum is the boundary validator's own `ValidationError`, and a `DryRun`
/// that would have succeeded is `DryRunOperation`.
#[tokio::test]
async fn create_ipam_refuses_a_request_it_cannot_satisfy_and_stores_nothing() {
    let (router, _engine) = emulator();
    let cases: Vec<RefusalCase> = vec![
        (
            "a tier the IpamTier enum does not define",
            vec![param("Tier", "premium")],
            "ValidationError",
        ),
        (
            "a tier in the wrong case",
            vec![param("Tier", "Free")],
            "ValidationError",
        ),
        (
            "a metered account the enum does not define",
            vec![param("MeteredAccount", "payer")],
            "ValidationError",
        ),
        (
            "an operating Region that is not a Region name",
            operating_region_params(&["US East (N. Virginia)"]),
            "InvalidParameterValue",
        ),
        (
            // Well formed as a Region code, but no such Region: an operating Region a
            // REQUEST names is held to the catalog of Regions AWS publishes, not to the
            // grammar.
            "an operating Region AWS does not publish",
            operating_region_params(&["eu-west-9"]),
            "InvalidParameterValue",
        ),
        (
            // A Region of another partition than the one this emulator's ARNs name.
            "an operating Region of another partition",
            operating_region_params(&["us-gov-west-1"]),
            "InvalidParameterValue",
        ),
        (
            "an operating Region with no name",
            vec![param("OperatingRegion.1.RegionName", "")],
            "MissingParameter",
        ),
        (
            "a tag specification for another resource type",
            tag_params(&[("Name", "nope")])
                .into_iter()
                .map(|(name, value)| {
                    if name == "TagSpecification.1.ResourceType" {
                        param(&name, "vpc")
                    } else {
                        (name, value)
                    }
                })
                .collect(),
            "InvalidParameterValue",
        ),
        (
            "a client token one character past the accepted length",
            vec![param(
                "ClientToken",
                &"k".repeat(ec2::state::MAX_CLIENT_TOKEN_CHARS + 1),
            )],
            "InvalidParameterValue",
        ),
        (
            "a dry run that would have succeeded",
            vec![param("DryRun", "true")],
            "DryRunOperation",
        ),
    ];

    for (case, params, expected) in cases {
        let (status, body) = create_ipam(&router, &params).await;
        assert_ne!(
            status, 200,
            "case {case:?}: the request must be refused, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected the error code {expected:?}, got {body}"
        );
    }

    let all = describe_ok("after the refusals", &router, &[]).await;
    assert!(
        elements(&all, "ipamId").is_empty(),
        "a refused create stores nothing, got {all}"
    );
}

// ---------------------------------------------------------------------------
// What the describe selects and pages over
// ---------------------------------------------------------------------------

/// The rule: `DescribeIpams` reports the whole store by default, narrowed by `IpamIds` and
/// by every filter it resolves — one filter per attribute of the IPAM, spelled in
/// kebab-case, values joined with OR and filters with AND — and an id or a filter name it
/// cannot resolve is refused rather than answered as "no results".
///
/// The filter rows span the catalog: an id, a derived ARN, a string the request chose, each
/// of the two enums, a count, a boolean, an operating Region (a member of a list) and the
/// tag pair, including one entry the issue that prompted this suite never mentioned.
#[tokio::test]
async fn describe_ipams_selects_by_id_and_by_every_filter_it_resolves() {
    let (router, _engine) = emulator();
    // Three IPAMs that differ in every filterable attribute a request can set.
    let alpha = create_ok(
        "alpha",
        &router,
        &{
            let mut params = vec![
                param("Description", "alpha space"),
                param("Tier", "advanced"),
                param("MeteredAccount", "resource-owner"),
                param("EnablePrivateGua", "true"),
            ];
            params.extend(operating_region_params(&["eu-west-1"]));
            params.extend(tag_params(&[("Name", "alpha"), ("Team", "network")]));
            params
        },
    )
    .await;
    let beta = create_ok(
        "beta",
        &router,
        &{
            let mut params = vec![param("Description", "beta space")];
            params.extend(operating_region_params(&["ap-northeast-2"]));
            params.extend(tag_params(&[("Name", "beta")]));
            params
        },
    )
    .await;
    let gamma = create_ok("gamma", &router, &[]).await;

    let alpha_id = required_member("alpha", &alpha, "ipamId");
    let beta_id = required_member("beta", &beta, "ipamId");
    let gamma_id = required_member("gamma", &gamma, "ipamId");
    let alpha_arn = required_member("alpha", &alpha, "ipamArn");
    let alpha_public_scope = required_member("alpha", &alpha, "publicDefaultScopeId");
    let every_id = [&alpha_id, &beta_id, &gamma_id];

    // (case, filter name, filter values, the IPAMs that match)
    let cases: Vec<(&str, &str, Vec<&str>, Vec<&String>)> = vec![
        ("by id", "ipam-id", vec![&alpha_id], vec![&alpha_id]),
        (
            "by id, two values joined with OR",
            "ipam-id",
            vec![&alpha_id, &gamma_id],
            vec![&alpha_id, &gamma_id],
        ),
        ("by the derived ARN", "ipam-arn", vec![&alpha_arn], vec![&alpha_id]),
        (
            "by the public default scope id",
            "public-default-scope-id",
            vec![&alpha_public_scope],
            vec![&alpha_id],
        ),
        (
            "by description",
            "description",
            vec!["beta space"],
            vec![&beta_id],
        ),
        (
            "by description with a wildcard",
            "description",
            vec!["*space"],
            vec![&alpha_id, &beta_id],
        ),
        ("by tier", "tier", vec!["advanced"], vec![&alpha_id]),
        (
            "by the default tier",
            "tier",
            vec!["free"],
            vec![&beta_id, &gamma_id],
        ),
        (
            "by metered account",
            "metered-account",
            vec!["resource-owner"],
            vec![&alpha_id],
        ),
        (
            "by the private GUA flag",
            "enable-private-gua",
            vec!["true"],
            vec![&alpha_id],
        ),
        (
            "by scope count, which every IPAM shares",
            "scope-count",
            vec!["2"],
            vec![&alpha_id, &beta_id, &gamma_id],
        ),
        (
            "by owner id, which every IPAM shares",
            "owner-id",
            vec![engine::emulator_account_id()],
            vec![&alpha_id, &beta_id, &gamma_id],
        ),
        (
            "by home Region, which every IPAM shares",
            "ipam-region",
            vec![REGION],
            vec![&alpha_id, &beta_id, &gamma_id],
        ),
        (
            "by an operating Region only one IPAM has",
            "operating-region",
            vec!["ap-northeast-2"],
            vec![&beta_id],
        ),
        (
            "by the home Region as an operating Region",
            "operating-region",
            vec![REGION],
            vec![&alpha_id, &beta_id, &gamma_id],
        ),
        (
            "by a tag value",
            "tag:Name",
            vec!["alpha"],
            vec![&alpha_id],
        ),
        (
            "by a tag key",
            "tag-key",
            vec!["Team"],
            vec![&alpha_id],
        ),
        (
            // `tag-value` compares against the values of EVERY tag, whatever key holds
            // them, where `tag:<key>` compares against one key's.
            "by a tag value under any key",
            "tag-value",
            vec!["network"],
            vec![&alpha_id],
        ),
        (
            "by a tag value two IPAMs share under the same key",
            "tag-value",
            vec!["alpha", "beta"],
            vec![&alpha_id, &beta_id],
        ),
        (
            "by a tag value no IPAM carries",
            "tag:Name",
            vec!["delta"],
            Vec::new(),
        ),
        (
            "by a tag value no IPAM carries under any key",
            "tag-value",
            vec!["delta"],
            Vec::new(),
        ),
        (
            "by a state no IPAM is in yet",
            "state",
            vec!["delete-complete"],
            Vec::new(),
        ),
        (
            // Nothing has gone wrong with any IPAM, so none carries a state message and
            // the filter selects nothing whatever value it names.
            "by a state message no IPAM carries",
            "state-message",
            vec!["*"],
            Vec::new(),
        ),
    ];

    for (case, name, values, expected) in cases {
        let mut params = vec![param("Filter.1.Name", name)];
        for (index, value) in values.iter().enumerate() {
            params.push(param(&format!("Filter.1.Value.{}", index + 1), value));
        }
        let body = describe_ok(case, &router, &params).await;
        let found: BTreeSet<String> = elements(&body, "ipamId").into_iter().collect();
        let wanted: BTreeSet<String> = expected.iter().map(|id| (*id).clone()).collect();
        assert_eq!(
            found, wanted,
            "case {case:?}: expected the filter to select {wanted:?}, got {body}"
        );
    }

    // Two filters hold together with AND.
    let body = describe_ok(
        "two filters",
        &router,
        &[
            param("Filter.1.Name", "tier"),
            param("Filter.1.Value.1", "free"),
            param("Filter.2.Name", "tag-key"),
            param("Filter.2.Value.1", "Name"),
        ],
    )
    .await;
    assert_eq!(
        elements(&body, "ipamId"),
        vec![beta_id.clone()],
        "two filters are joined with AND, got {body}"
    );

    // An id that names no IPAM, and a filter name the operation does not resolve.
    let (status, body) = describe_ipams(&router, &[param("IpamId.1", "ipam-00000000000000000")]).await;
    assert_eq!(
        status, 400,
        "an unknown IPAM id is refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidIpamId.NotFound",
        "an unknown IPAM id is refused as not found, got {body}"
    );
    let (status, body) = describe_ipams(
        &router,
        &[
            param("Filter.1.Name", "ipam-colour"),
            param("Filter.1.Value.1", "blue"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "an unresolvable filter name is refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "an unresolvable filter name is refused as an invalid value, got {body}"
    );

    // Nothing above changed the store.
    let all = describe_ok("the whole store", &router, &[]).await;
    assert_eq!(
        elements(&all, "ipamId").into_iter().collect::<BTreeSet<_>>(),
        every_id.iter().map(|id| (*id).clone()).collect::<BTreeSet<_>>(),
        "the describes report the IPAMs that were created and nothing else, got {all}"
    );
}

/// The rule: `DescribeIpams` answers at most `MaxResults` IPAMs and a `NextToken` naming
/// where the next page starts, that token continues from the end of the page before it, and
/// "this value is null when there are no more results to return"
/// (`DescribeIpamsResult$NextToken`). A token the service did not mint is refused.
#[tokio::test]
async fn describe_ipams_pages_in_a_stable_order() {
    let (router, _engine) = emulator();
    // The model's `IpamMaxResults` has a minimum of 5, so paging needs more than five
    // IPAMs for a second page to exist at all.
    let mut created = BTreeSet::new();
    for nth in 0..7 {
        let case = format!("IPAM {nth}");
        let body = create_ok(&case, &router, &[param("ClientToken", &format!("t{nth}"))]).await;
        created.insert(required_member(&case, &body, "ipamId"));
    }

    // (case, page size)
    for page_size in [5, 6, 7] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(param("NextToken", token));
            }
            let body = describe_ok(&case, &router, &params).await;
            let page = elements(&body, "ipamId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} IPAMs, got {body}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= 4,
                "case {case:?}: the paging walk must terminate, got {seen:?}"
            );
            match element(&body, "nextToken") {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {case:?}: the pages come in IpamId order, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            created,
            "case {case:?}: paging reports every IPAM exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            created.len(),
            "case {case:?}: no IPAM is reported twice, got {seen:?}"
        );
    }

    let (status, body) = describe_ipams(&router, &[param("NextToken", "not-a-token")]).await;
    assert_eq!(
        status, 400,
        "a token the service did not mint is refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "a bogus token is refused as an invalid pagination token, got {body}"
    );
}

/// The rule: a request is refused BEFORE any work proportional to what it carries when it
/// names more ids, more filters, more values per filter or a longer value than this service
/// will match — the same per-factor caps every other paginated describe in this crate
/// applies, which is what bounds the product of candidates, filters, values and value length
/// and so the time one call holds the shared store.
///
/// The rows are the four factors, each one past its published cap; the row inside the cap
/// shows the cap is not refusing a request it should serve.
#[tokio::test]
async fn describe_ipams_refuses_more_filtering_than_it_will_do() {
    let (router, _engine) = emulator();
    let created = create_ok("one IPAM", &router, &[]).await;
    let ipam_id = required_member("one IPAM", &created, "ipamId");

    let many_filters = |count: usize| -> Vec<Param> {
        (0..count)
            .flat_map(|index| {
                vec![
                    param(&format!("Filter.{}.Name", index + 1), "ipam-id"),
                    param(&format!("Filter.{}.Value.1", index + 1), &ipam_id),
                ]
            })
            .collect()
    };
    let many_values = |count: usize| -> Vec<Param> {
        let mut params = vec![param("Filter.1.Name", "ipam-id")];
        // Synthesised ids for every position but the last, which carries the id the store
        // really minted — so a list AT the cap exercises the cap and still selects the
        // IPAM, rather than depending on where the store's id counter happens to stand.
        params.extend((0..count.saturating_sub(1)).map(|index| {
            param(
                &format!("Filter.1.Value.{}", index + 1),
                &format!("ipam-{:017x}", index + 1),
            )
        }));
        params.push(param(&format!("Filter.1.Value.{count}"), &ipam_id));
        params
    };
    let many_ids = |count: usize| -> Vec<Param> {
        (0..count)
            .map(|index| param(&format!("IpamId.{}", index + 1), &ipam_id))
            .collect()
    };

    let cases: Vec<RefusalCase> = vec![
        (
            "one filter past the cap",
            many_filters(ec2::state::MAX_FILTERS + 1),
            "InvalidParameterValue",
        ),
        (
            "one value past the cap",
            many_values(ec2::state::MAX_FILTER_VALUES + 1),
            "InvalidParameterValue",
        ),
        (
            "a filter value one byte past the cap",
            vec![
                param("Filter.1.Name", "description"),
                param(
                    "Filter.1.Value.1",
                    &"d".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1),
                ),
            ],
            "InvalidParameterValue",
        ),
        (
            "one id past the cap",
            many_ids(ec2::state::MAX_RESOURCE_IDS + 1),
            "InvalidParameterValue",
        ),
    ];
    for (case, params, expected) in cases {
        let (status, body) = describe_ipams(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the request must be refused, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected the error code {expected:?}, got {body}"
        );
    }

    // Each factor AT its cap is served, so the caps refuse only what is past them.
    for (case, params) in [
        ("filters at the cap", many_filters(ec2::state::MAX_FILTERS)),
        (
            "values at the cap",
            many_values(ec2::state::MAX_FILTER_VALUES),
        ),
        ("ids at the cap", many_ids(ec2::state::MAX_RESOURCE_IDS)),
    ] {
        let body = describe_ok(case, &router, &params).await;
        assert_eq!(
            elements(&body, "ipamId"),
            vec![ipam_id.clone()],
            "case {case:?}: a request at the cap is served, got {body}"
        );
    }
}
