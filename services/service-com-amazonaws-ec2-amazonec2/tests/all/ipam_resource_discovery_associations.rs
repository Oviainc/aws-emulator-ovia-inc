// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateIpamResourceDiscovery`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * the associate LINKS the resource discovery the request names to the IPAM it names and
//!   STORES that link, taking each linked resource's own facts from the STORED resource (the
//!   IPAM's ARN and home Region, the discovery owner's account) rather than from the request,
//!   and deriving everything else the service owns — the id, the ARN, the fact that a
//!   caller-made association is not the IPAM's default one, the discovery status, and the
//!   non-terminal state the clock then owes the terminal one;
//! * the IPAM's own count of the associations against it is recomputed from the associations
//!   the store holds, so it reads back through `DescribeIpams`;
//! * an id that is not an id of its resource's kind at all is refused with that resource's
//!   `Malformed` code, a well-formed id the account holds no resource for with its `NotFound`
//!   code, and the DISCOVERY is resolved before the IPAM;
//! * one IPAM-and-discovery pair holds at most one association, so a pair that already has
//!   one is refused and nothing further is stored;
//! * one `ClientToken` names the one association it made: a replay answers that association,
//!   and a replay asking for anything else is refused.
//!
//! Each table row is a distinct input along what those generalise over: how many tags the
//! request names, whether it carries a token, which member is at fault and in what way, and
//! which of the two linked resources the request gets wrong.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::state::{
    IDEMPOTENT_PARAMETER_MISMATCH, INVALID_IPAM_ID_MALFORMED, INVALID_IPAM_ID_NOT_FOUND,
    INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED, INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
    IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_PREFIX, MAX_CLIENT_TOKEN_CHARS,
    RESOURCE_ALREADY_ASSOCIATED, RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY_ASSOCIATION,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region.
const REGION: &str = "us-east-1";

/// The action under test.
const ACTION: &str = "AssociateIpamResourceDiscovery";

/// Longer than the span the clock owes an `associate-in-progress` association, so a read
/// after this much has certainly passed the due instant.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it: the router and
/// the engine its store lives in.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One call of `action` signed for the Region above.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call of `action` requiring success, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = invoke(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The IPAM a suite creates through `CreateIpam`: what a request names it by, the ARN every
/// association of it reports, and the default resource discovery and association it came up
/// with.
struct CreatedIpam {
    /// The `IpamId` an associate request names.
    id: String,
    /// "The IPAM ARN" (`IpamResourceDiscoveryAssociation$IpamArn`).
    arn: String,
    /// "The default resource discovery is the resource discovery automatically created when
    /// you create an IPAM" (`IpamResourceDiscovery$IsDefault`).
    default_discovery: String,
    /// The association `CreateIpam` made between the two above.
    default_association: String,
}

/// Create one IPAM through the operation that owns it, and answer what it is named by.
async fn create_ipam(case: &str, router: &MultiServiceRouter) -> CreatedIpam {
    let body = ok(case, router, "CreateIpam", &[]).await;
    let parsed = document(case, &body);
    let ipam = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM (no <ipamId>) in {body}"));
    let read = |name: &str| -> String {
        child_text(ipam, name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the IPAM in {body}"))
    };
    CreatedIpam {
        id: read("ipamId"),
        arn: read("ipamArn"),
        default_discovery: read("defaultResourceDiscoveryId"),
        default_association: read("defaultResourceDiscoveryAssociationId"),
    }
}

/// One resource discovery association as a response reports it: every member read off the
/// wire, with the tags flattened into comparable values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedAssociation {
    owner_id: Option<String>,
    association_id: Option<String>,
    association_arn: Option<String>,
    discovery_id: Option<String>,
    ipam_id: Option<String>,
    ipam_arn: Option<String>,
    ipam_region: Option<String>,
    is_default: Option<String>,
    discovery_status: Option<String>,
    state: Option<String>,
    tags: Vec<(String, String)>,
}

/// The one association an `AssociateIpamResourceDiscovery` response carries.
fn reported_association(case: &str, body: &str) -> ReportedAssociation {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| {
            node.is_element() && child(*node, "ipamResourceDiscoveryAssociationId").is_some()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: no association (no <ipamResourceDiscoveryAssociationId>) in {body}")
        });
    let tags = child(node, "tagSet")
        .map(|set| {
            set.children()
                .filter(|item| item.is_element())
                .map(|item| {
                    (
                        child_text(item, "key").unwrap_or_default(),
                        child_text(item, "value").unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    ReportedAssociation {
        owner_id: child_text(node, "ownerId"),
        association_id: child_text(node, "ipamResourceDiscoveryAssociationId"),
        association_arn: child_text(node, "ipamResourceDiscoveryAssociationArn"),
        discovery_id: child_text(node, "ipamResourceDiscoveryId"),
        ipam_id: child_text(node, "ipamId"),
        ipam_arn: child_text(node, "ipamArn"),
        ipam_region: child_text(node, "ipamRegion"),
        is_default: child_text(node, "isDefault"),
        discovery_status: child_text(node, "resourceDiscoveryStatus"),
        state: child_text(node, "state"),
        tags,
    }
}

/// "The IPAM's resource discovery association count"
/// (`Ipam$ResourceDiscoveryAssociationCount`), as `DescribeIpams` reports it for one IPAM.
async fn association_count(case: &str, router: &MultiServiceRouter, ipam_id: &str) -> String {
    let body = ok(
        case,
        router,
        "DescribeIpams",
        &[param("IpamId.1", ipam_id)],
    )
    .await;
    let parsed = document(case, &body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM (no <ipamId>) in {body}"));
    child_text(node, "resourceDiscoveryAssociationCount").unwrap_or_else(|| {
        panic!("case {case:?}: no <resourceDiscoveryAssociationCount> on the IPAM in {body}")
    })
}

/// The ids of every association the store holds, which is what proves a refusal wrote
/// nothing: the describe that reports them is a scaffold of its own, so the store is read
/// directly — the same way `ipam_lifecycle` checks the records a `CreateIpam` writes.
fn stored_association_ids(engine: &engine::InMemoryEngine) -> BTreeSet<String> {
    let state = ec2::state::service_state(engine).expect("the EC2 store");
    let store = state.lock();
    store
        .ipam_resource_discovery_associations
        .keys()
        .cloned()
        .collect()
}

/// The state one stored association is in, read from the store for the reason above.
fn stored_association_state(engine: &engine::InMemoryEngine, association_id: &str) -> String {
    let state = ec2::state::service_state(engine).expect("the EC2 store");
    let store = state.lock();
    store
        .ipam_resource_discovery_associations
        .get(association_id)
        .and_then(|association| association.state.clone())
        .unwrap_or_else(|| panic!("association {association_id} is not stored"))
}

/// The form fields for one `ipam-resource-discovery-association` tag specification.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY_ASSOCIATION,
    )];
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

// ---------------------------------------------------------------------------
// What the associate stores
// ---------------------------------------------------------------------------

/// One row of the associate table: the case, whether the request carries a token, and the
/// tags it names.
struct AssociateCase {
    case: &'static str,
    with_token: bool,
    tags: Vec<(&'static str, &'static str)>,
}

/// The rule: `AssociateIpamResourceDiscovery` stores one association between the two
/// resources the request names, carrying exactly what the request described, with each
/// linked resource's own facts taken from the stored resource and every member the service
/// owns derived — and the IPAM's association count reads the associations back.
#[tokio::test]
async fn associate_ipam_resource_discovery_links_the_discovery_to_the_ipam() {
    let (router, engine) = emulator();
    // The discovery every row associates: one IPAM's own default resource discovery, which
    // `CreateIpam` created and associated with THAT IPAM, so associating it with a further
    // IPAM is what each row below does.
    let source = create_ipam("the IPAM whose discovery is associated", &router).await;

    let cases = [
        AssociateCase {
            case: "no optional member at all",
            with_token: false,
            tags: Vec::new(),
        },
        AssociateCase {
            case: "a client token",
            with_token: true,
            tags: Vec::new(),
        },
        AssociateCase {
            case: "one tag",
            with_token: false,
            tags: vec![("Name", "discovery-alpha")],
        },
        AssociateCase {
            case: "a token and three tags",
            with_token: true,
            tags: vec![("Name", "discovery-beta"), ("Team", "network"), ("Env", "prod")],
        },
    ];

    let account = engine::emulator_account_id().to_string();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    // Every association the store should hold at the end: each IPAM's own default one, plus
    // the one each row makes.
    let mut expected: BTreeSet<String> =
        BTreeSet::from([source.default_association.clone()]);
    for (nth, row) in cases.iter().enumerate() {
        let case = row.case;
        // One pair holds at most one association, so each row needs a target IPAM of its own.
        let target = create_ipam(case, &router).await;
        expected.insert(target.default_association.clone());
        let mut params = vec![
            param("IpamId", &target.id),
            param("IpamResourceDiscoveryId", &source.default_discovery),
        ];
        if row.with_token {
            params.push(param("ClientToken", &format!("token-{nth}")));
        }
        params.extend(tag_params(&row.tags));

        let body = ok(case, &router, ACTION, &params).await;
        let association = reported_association(case, &body);
        let association_id = association
            .association_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the association has no id: {body}"));
        assert!(
            is_long_id(
                IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_PREFIX,
                &association_id
            ),
            "case {case:?}: expected a minted \
             {IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_PREFIX} id, got {association_id:?}"
        );
        assert!(
            minted.insert(association_id.clone()),
            "case {case:?}: expected an id no earlier call was given, got {association_id:?}"
        );
        expected.insert(association_id.clone());

        // What the service owns, derived rather than echoed.
        assert_eq!(
            association.association_arn.as_deref(),
            Some(
                engine::emulator_arn(
                    "ec2",
                    "",
                    &format!(
                        "{RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY_ASSOCIATION}/{association_id}"
                    ),
                )
                .as_str()
            ),
            "case {case:?}: the association reports its own ARN, got {association:?}"
        );
        assert_eq!(
            association.owner_id.as_deref(),
            Some(account.as_str()),
            "case {case:?}: the discovery is owned by this account, got {association:?}"
        );
        assert_eq!(
            association.is_default.as_deref(),
            Some("false"),
            "case {case:?}: a caller-made association is not the IPAM's default one, \
             got {association:?}"
        );
        assert_eq!(
            association.discovery_status.as_deref(),
            Some("active"),
            "case {case:?}: the results of a stored discovery are readable, \
             got {association:?}"
        );
        assert_eq!(
            association.state.as_deref(),
            Some("associate-in-progress"),
            "case {case:?}: the association is answered while it is still being made, \
             got {association:?}"
        );

        // What the two linked resources decide, taken from the STORED resources.
        assert_eq!(
            association.ipam_id.as_deref(),
            Some(target.id.as_str()),
            "case {case:?}: the association names the IPAM the request named, \
             got {association:?}"
        );
        assert_eq!(
            association.ipam_arn.as_deref(),
            Some(target.arn.as_str()),
            "case {case:?}: the association reports the stored IPAM's ARN, \
             got {association:?}"
        );
        assert_eq!(
            association.ipam_region.as_deref(),
            Some(REGION),
            "case {case:?}: the association reports the IPAM's home Region, \
             got {association:?}"
        );
        assert_eq!(
            association.discovery_id.as_deref(),
            Some(source.default_discovery.as_str()),
            "case {case:?}: the association names the discovery the request named, \
             got {association:?}"
        );

        // What the request decided, carried through.
        let wanted_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            association.tags, wanted_tags,
            "case {case:?}: the association carries the tags the request asked for, \
             got {association:?}"
        );

        // The round trip: the IPAM now counts the association beside its own default one.
        assert_eq!(
            association_count(case, &router, &target.id).await,
            "2",
            "case {case:?}: the target IPAM counts its default association and this one"
        );
        // The SOURCE IPAM is untouched: the discovery was associated elsewhere, which adds
        // no association against the IPAM that owns it.
        assert_eq!(
            association_count(case, &router, &source.id).await,
            "1",
            "case {case:?}: the IPAM that owns the discovery still holds only its default \
             association"
        );
    }

    // Every association the store holds, and nothing besides: one default per IPAM created
    // above, plus the one each row made.
    assert_eq!(
        stored_association_ids(&engine),
        expected,
        "the store holds each IPAM's default association and every one the rows made"
    );
    assert_eq!(
        minted.len(),
        cases.len(),
        "each row made an association of its own, got {minted:?}"
    );
}

/// The rule: an association `AssociateIpamResourceDiscovery` answers is in the non-terminal
/// `associate-in-progress` state, and the clock owes it `associate-complete` whether or not
/// anybody calls again — a caller acting straight after the call still sees the transient
/// state, and nothing brings the move forward.
#[tokio::test]
async fn an_association_completes_on_its_own() {
    let case = "the clock completes the association";
    let (router, engine) = emulator();
    let source = create_ipam(case, &router).await;
    let target = create_ipam(case, &router).await;

    let body = ok(
        case,
        &router,
        ACTION,
        &[
            param("IpamId", &target.id),
            param("IpamResourceDiscoveryId", &source.default_discovery),
        ],
    )
    .await;
    let association_id = reported_association(case, &body)
        .association_id
        .expect("the association has an id");

    assert_eq!(
        stored_association_state(&engine, &association_id),
        "associate-in-progress",
        "case {case:?}: a caller acting straight after the call sees the transient state"
    );

    tokio::time::sleep(past_the_transition()).await;
    // A call that touches the store is what materialises what the clock already owed.
    let _ = association_count(case, &router, &target.id).await;
    assert_eq!(
        stored_association_state(&engine, &association_id),
        "associate-complete",
        "case {case:?}: the association completes once the transition is due"
    );
}

// ---------------------------------------------------------------------------
// What the associate refuses
// ---------------------------------------------------------------------------

/// The rule: a request the documented contract does not admit is refused with the code that
/// names what was wrong — each resource's `Malformed` code for a value that is not an id of
/// its kind at all, its `NotFound` code for a well-formed id the account holds no resource
/// for, `Resource.AlreadyAssociated` for a pair that already holds an association, and
/// `DryRunOperation` for a permission check, whose answer does not depend on whether the
/// resources the request names exist — and NOTHING further is stored.
#[tokio::test]
async fn associate_ipam_resource_discovery_refuses_what_the_contract_does_not_admit() {
    let (router, engine) = emulator();
    let source = create_ipam("the IPAM whose discovery the refusals aim at", &router).await;
    let target = create_ipam("the IPAM the refusals aim at", &router).await;
    let unknown_ipam = "ipam-00000000000000000";
    let unknown_discovery = "ipam-res-disco-00000000000000000";
    let long_token = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);
    let good_discovery = source.default_discovery.clone();

    // (case, params, expected status, expected code)
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "a value that is no resource discovery id at all",
            vec![
                param("IpamId", &target.id),
                param("IpamResourceDiscoveryId", "not-a-discovery"),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        ),
        (
            "a resource discovery id with a non-hex suffix",
            vec![
                param("IpamId", &target.id),
                param("IpamResourceDiscoveryId", "ipam-res-disco-zzzzzzzz"),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        ),
        (
            "a resource discovery id of the wrong length",
            vec![
                param("IpamId", &target.id),
                param("IpamResourceDiscoveryId", "ipam-res-disco-0123456"),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        ),
        (
            // The two resources share an id prefix, and an association is not a discovery.
            "an association id where a resource discovery id belongs",
            vec![
                param("IpamId", &target.id),
                param("IpamResourceDiscoveryId", &source.default_association),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        ),
        (
            "a well-formed resource discovery id the account holds none for",
            vec![
                param("IpamId", &target.id),
                param("IpamResourceDiscoveryId", unknown_discovery),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        ),
        (
            "a short-form resource discovery id the account holds none for",
            vec![
                param("IpamId", &target.id),
                param("IpamResourceDiscoveryId", "ipam-res-disco-0000000f"),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        ),
        (
            // The discovery is resolved FIRST, so an unknown discovery beside an unknown
            // IPAM is answered as the discovery's own `NotFound`.
            "neither resource exists",
            vec![
                param("IpamId", unknown_ipam),
                param("IpamResourceDiscoveryId", unknown_discovery),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        ),
        (
            "a value that is no IPAM id at all",
            vec![
                param("IpamId", "not-an-ipam"),
                param("IpamResourceDiscoveryId", &good_discovery),
            ],
            400,
            INVALID_IPAM_ID_MALFORMED,
        ),
        (
            "an IPAM id of the wrong length",
            vec![
                param("IpamId", "ipam-0123456"),
                param("IpamResourceDiscoveryId", &good_discovery),
            ],
            400,
            INVALID_IPAM_ID_MALFORMED,
        ),
        (
            "a well-formed IPAM id the account holds no IPAM for",
            vec![
                param("IpamId", unknown_ipam),
                param("IpamResourceDiscoveryId", &good_discovery),
            ],
            400,
            INVALID_IPAM_ID_NOT_FOUND,
        ),
        (
            // `CreateIpam` already associated the discovery with the IPAM that owns it.
            "a pair that already holds an association",
            vec![
                param("IpamId", &source.id),
                param("IpamResourceDiscoveryId", &good_discovery),
            ],
            400,
            RESOURCE_ALREADY_ASSOCIATED,
        ),
        (
            "a tag specification for another resource type",
            vec![
                param("IpamId", &target.id),
                param("IpamResourceDiscoveryId", &good_discovery),
                param("TagSpecification.1.ResourceType", "ipam-scope"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "wrong"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag specification naming no resource type",
            vec![
                param("IpamId", &target.id),
                param("IpamResourceDiscoveryId", &good_discovery),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "no type"),
            ],
            400,
            "MissingParameter",
        ),
        (
            "a client token past the length this service keeps",
            vec![
                param("IpamId", &target.id),
                param("IpamResourceDiscoveryId", &good_discovery),
                param("ClientToken", &long_token),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a dry run that would have succeeded",
            vec![
                param("IpamId", &target.id),
                param("IpamResourceDiscoveryId", &good_discovery),
                param("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
        (
            // "Checks whether you have the required permissions … without actually making
            // the request", so the answer does not depend on the resources it names.
            "a dry run naming resources that do not exist",
            vec![
                param("IpamId", unknown_ipam),
                param("IpamResourceDiscoveryId", unknown_discovery),
                param("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
    ];

    let before = stored_association_ids(&engine);
    for (case, params, status, code) in &cases {
        let (answered, body) = invoke(&router, ACTION, params).await;
        assert_eq!(
            answered, *status,
            "case {case:?}: expected HTTP {status}, got {answered} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {case:?}: expected the code {code}, got {body}"
        );
    }

    // The refusals name the id the REQUEST carried, in the sentence each resource's own
    // refusal is worded with.
    let (_status, body) = invoke(
        &router,
        ACTION,
        &[
            param("IpamId", &target.id),
            param("IpamResourceDiscoveryId", unknown_discovery),
        ],
    )
    .await;
    assert_eq!(
        error_message(&body),
        format!("The ipam-resource-discovery ID '{unknown_discovery}' does not exist"),
        "the refusal names the discovery the request asked for, got {body}"
    );
    let (_status, body) = invoke(
        &router,
        ACTION,
        &[
            param("IpamId", unknown_ipam),
            param("IpamResourceDiscoveryId", &good_discovery),
        ],
    )
    .await;
    assert_eq!(
        error_message(&body),
        format!("The IPAM ({unknown_ipam}) does not exist."),
        "the refusal names the IPAM the request asked for, got {body}"
    );

    // None of the refusals above stored anything.
    assert_eq!(
        stored_association_ids(&engine),
        before,
        "a refused associate stores nothing"
    );
    assert_eq!(
        association_count("nothing was stored", &router, &target.id).await,
        "1",
        "the target IPAM still holds only its own default association"
    );
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// The rule: "a client token" (`AssociateIpamResourceDiscoveryRequest$ClientToken`) — one
/// token names the one association it made, so a repeat of the request reports that
/// association and writes nothing, and a replay asking for anything else is refused rather
/// than answered with an association the caller did not ask for.
#[tokio::test]
async fn one_client_token_names_the_one_association_it_made() {
    let case = "the token replay";
    let (router, engine) = emulator();
    let source = create_ipam(case, &router).await;
    let target = create_ipam(case, &router).await;
    let other_target = create_ipam(case, &router).await;
    let other_source = create_ipam(case, &router).await;

    let token = "one-token";
    let request = |ipam_id: &str, discovery_id: &str, tags: &[(&str, &str)]| -> Vec<Param> {
        let mut params = vec![
            param("IpamId", ipam_id),
            param("IpamResourceDiscoveryId", discovery_id),
            param("ClientToken", token),
        ];
        params.extend(tag_params(tags));
        params
    };

    let first = ok(
        case,
        &router,
        ACTION,
        &request(&target.id, &source.default_discovery, &[("Name", "once")]),
    )
    .await;
    let made = reported_association(case, &first);
    let held = stored_association_ids(&engine);

    // The same request again: the same association, and nothing further stored.
    let replay = ok(
        case,
        &router,
        ACTION,
        &request(&target.id, &source.default_discovery, &[("Name", "once")]),
    )
    .await;
    assert_eq!(
        reported_association(case, &replay),
        made,
        "case {case:?}: a replay of the same request answers the association it made"
    );
    assert_eq!(
        stored_association_ids(&engine),
        held,
        "case {case:?}: a replay writes nothing further"
    );

    // Each row: the same token with ONE member of the request changed.
    let mismatches: Vec<(&str, Vec<Param>)> = vec![
        (
            "another IPAM",
            request(
                &other_target.id,
                &source.default_discovery,
                &[("Name", "once")],
            ),
        ),
        (
            "another resource discovery",
            request(
                &target.id,
                &other_source.default_discovery,
                &[("Name", "once")],
            ),
        ),
        (
            "another tag value",
            request(
                &target.id,
                &source.default_discovery,
                &[("Name", "something else")],
            ),
        ),
        (
            "no tag at all",
            request(&target.id, &source.default_discovery, &[]),
        ),
    ];
    for (changed, params) in &mismatches {
        let (status, body) = invoke(&router, ACTION, params).await;
        assert_eq!(
            status, 400,
            "case {changed:?}: expected HTTP 400 for a replay asking for something else, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            IDEMPOTENT_PARAMETER_MISMATCH,
            "case {changed:?}: expected the mismatch code, got {body}"
        );
    }
    assert_eq!(
        stored_association_ids(&engine),
        held,
        "a refused replay writes nothing"
    );
}
