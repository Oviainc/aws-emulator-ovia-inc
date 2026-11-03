// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateIpamInternetRegistryAssociation` and the
//! `DescribeIpamInternetRegistryAssociations` that reads it back, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * the create MINTS an association between the IPAM the request names and a Regional
//!   Internet Registry and STORES it: the Region it reports is the stored IPAM's own home
//!   Region rather than the Region the call was signed for, what the request decided (the
//!   registry, the organization handle, the description and the tags) is carried through, and
//!   everything the service owns is derived — the id, the ARN, the owning account and the
//!   non-terminal state the clock then owes the next one;
//! * the CLOCK, not a later call, settles a created association: it is answered
//!   `create-in-progress` and reaches `pending-enable`, with the state message moving with the
//!   state;
//! * an IPAM id that is not an IPAM id at all is refused with the IPAM's `Malformed` code, one
//!   the account holds no IPAM for with its `NotFound` code, a required value left empty or a
//!   value no documented constraint admits with EC2's code for that class — and NOTHING is
//!   stored;
//! * one `ClientToken` names the one association it minted: a replay answers that association
//!   and creates nothing, and a replay asking for anything else is refused;
//! * every member of the created association reads back through
//!   `DescribeIpamInternetRegistryAssociations`, which also narrows by the members the create
//!   stored.
//!
//! Each table row is a distinct input along what those rules generalise over: which Regional
//! Internet Registry of the modelled catalog, which optional member the request carries, how
//! many tags, which Region the IPAM lives in, and which member is at fault and in what way.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::ops::types::{IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES, RIR_VALUES};
use ec2::state::{
    IDEMPOTENT_PARAMETER_MISMATCH, INVALID_IPAM_ID_MALFORMED, INVALID_IPAM_ID_NOT_FOUND,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_CREATE_IN_PROGRESS_MESSAGE,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_PENDING_ENABLE_MESSAGE,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_CREATE_IN_PROGRESS,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_PENDING_ENABLE, MAX_CLIENT_TOKEN_CHARS,
    RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, filters, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The action under test.
const CREATE: &str = "CreateIpamInternetRegistryAssociation";

/// The action that reads the associations back.
const DESCRIBE: &str = "DescribeIpamInternetRegistryAssociations";

/// Longer than the span the clock owes a `create-in-progress` association.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One call of `action` signed for `region`, requiring success, and the body it answered.
async fn ok(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> String {
    let (status, body) = call(router, region, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// An IPAM a suite creates through `CreateIpam`: the id a create names it by and the Region it
/// was created in, which is that IPAM's home Region.
struct CreatedIpam {
    id: String,
    region: &'static str,
}

/// Create one IPAM in `region` through the operation that owns it.
async fn create_ipam(case: &str, router: &MultiServiceRouter, region: &'static str) -> CreatedIpam {
    let body = ok(case, router, region, "CreateIpam", &[]).await;
    CreatedIpam {
        id: element(&body, "ipamId")
            .unwrap_or_else(|| panic!("case {case:?}: no <ipamId> on the created IPAM in {body}")),
        region,
    }
}

/// The ids of every association the store holds — what proves a refusal wrote nothing.
fn stored_association_ids(engine: &engine::InMemoryEngine) -> BTreeSet<String> {
    let state = ec2::state::service_state(engine).expect("the EC2 store");
    let store = state.lock();
    store
        .ipam_internet_registry_associations
        .keys()
        .cloned()
        .collect()
}

/// One association as a response reports it: every member read off the wire, with the tags
/// flattened into comparable pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedAssociation {
    association_id: Option<String>,
    association_arn: Option<String>,
    ipam_id: Option<String>,
    ipam_region: Option<String>,
    owner_id: Option<String>,
    rir: Option<String>,
    organization_handle: Option<String>,
    description: Option<String>,
    state: Option<String>,
    state_message: Option<String>,
    child_request_xml: Option<String>,
    tags: Vec<(String, String)>,
}

/// The association element of a response as [`ReportedAssociation`].
fn read_association(node: roxmltree::Node<'_, '_>) -> ReportedAssociation {
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
        association_id: child_text(node, "ipamInternetRegistryAssociationId"),
        association_arn: child_text(node, "ipamInternetRegistryAssociationArn"),
        ipam_id: child_text(node, "ipamId"),
        ipam_region: child_text(node, "ipamRegion"),
        owner_id: child_text(node, "ownerId"),
        rir: child_text(node, "rir"),
        organization_handle: child_text(node, "organizationHandle"),
        description: child_text(node, "description"),
        state: child_text(node, "state"),
        state_message: child_text(node, "stateMessage"),
        child_request_xml: child_text(node, "childRequestXml"),
        tags,
    }
}

/// The one association a create response carries.
fn created_association(case: &str, body: &str) -> ReportedAssociation {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| {
            node.is_element() && child(*node, "ipamInternetRegistryAssociationId").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no association in {body}"));
    read_association(node)
}

/// Every association a describe response reports, in the order it reported them.
fn described_associations(case: &str, body: &str) -> Vec<ReportedAssociation> {
    let parsed = document(case, body);
    let Some(set) = parsed.descendants().find(|node| {
        node.is_element() && node.tag_name().name() == "ipamInternetRegistryAssociationSet"
    }) else {
        return Vec::new();
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(read_association)
        .collect()
}

/// The describe narrowed to one id, as the one association it reports.
async fn describe_one(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    association_id: &str,
) -> ReportedAssociation {
    let body = ok(
        case,
        router,
        region,
        DESCRIBE,
        &indexed("IpamInternetRegistryAssociationId", &[association_id]),
    )
    .await;
    let mut associations = described_associations(case, &body);
    assert_eq!(
        associations.len(),
        1,
        "case {case:?}: expected exactly the association {association_id} back, got {body}"
    );
    associations.remove(0)
}

/// The association ids a describe carrying `params` reports.
async fn described_ids(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> Vec<String> {
    let body = ok(case, router, region, DESCRIBE, params).await;
    described_associations(case, &body)
        .into_iter()
        .filter_map(|association| association.association_id)
        .collect()
}

/// The form fields for one `ipam-internet-registry-association` tag specification over `tags`.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION,
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
// The words this suite asserts on
// ---------------------------------------------------------------------------

/// The registries and states asserted below are the MODEL's own: each is a member of the
/// catalog the model publishes for its enum, so no row can pin a word the API does not have.
#[test]
fn the_asserted_words_are_the_models_own() {
    for (case, word, catalog) in [
        (
            "the state a create answers with",
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_CREATE_IN_PROGRESS,
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES,
        ),
        (
            "the state the clock then owes it",
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_PENDING_ENABLE,
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES,
        ),
    ] {
        assert!(
            catalog.contains(&word),
            "case {case:?}: expected {word:?} to be a member of the modelled catalog, \
             got {catalog:?}"
        );
    }
    for row in create_cases() {
        assert!(
            RIR_VALUES.contains(&row.rir),
            "case {case:?}: expected {rir:?} to be a member of the modelled Rir catalog, \
             got {RIR_VALUES:?}",
            case = row.case,
            rir = row.rir,
        );
    }
    // Every registry the model publishes is covered by a row, so the table spans the catalog
    // rather than one entry of it.
    let covered: BTreeSet<&str> = create_cases().iter().map(|row| row.rir).collect();
    assert_eq!(
        covered,
        RIR_VALUES.iter().copied().collect::<BTreeSet<&str>>(),
        "expected a row for every Regional Internet Registry the model publishes, got {covered:?}"
    );
}

// ---------------------------------------------------------------------------
// What the create stores
// ---------------------------------------------------------------------------

/// One row of the create table.
struct CreateCase {
    case: &'static str,
    /// The Regional Internet Registry the association is with, a member of `Rir`.
    rir: &'static str,
    /// The organization handle at that registry, as each registry spells one.
    organization_handle: &'static str,
    /// The description, when the request carries one.
    description: Option<&'static str>,
    /// The tags the request assigns to the association.
    tags: Vec<(&'static str, &'static str)>,
}

/// The rows the create table is written against: every registry the model publishes, with and
/// without a description, with no tags, one and three.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "arin, no optional member at all",
            rir: "arin",
            organization_handle: "EXAMPLE-ORG",
            description: None,
            tags: Vec::new(),
        },
        CreateCase {
            case: "ripe with a description and one tag",
            rir: "ripe",
            organization_handle: "ORG-EX1-RIPE",
            description: Some("delegated rpki for the edge prefixes"),
            tags: vec![("Name", "edge")],
        },
        CreateCase {
            case: "apnic with three tags",
            rir: "apnic",
            organization_handle: "A12345678",
            description: Some("apac"),
            tags: vec![("Name", "apac"), ("Team", "network"), ("Env", "prod")],
        },
        CreateCase {
            case: "lacnic with a description and no tags",
            rir: "lacnic",
            organization_handle: "LACNIC-EX",
            description: Some("latam"),
            tags: Vec::new(),
        },
        CreateCase {
            case: "nicbr with one tag and no description",
            rir: "nicbr",
            organization_handle: "NICBR-EX",
            description: None,
            tags: vec![("Name", "brazil")],
        },
    ]
}

/// The rule: `CreateIpamInternetRegistryAssociation` stores one association between the IPAM
/// the request names and the registry it names, carrying exactly what the request described,
/// with the IPAM's Region taken from the stored IPAM and every member the service owns
/// derived; and every one of those members reads back through
/// `DescribeIpamInternetRegistryAssociations`.
#[tokio::test]
async fn create_stores_the_association_the_request_describes() {
    let (router, engine) = emulator();
    let ipam = create_ipam("the IPAM the associations belong to", &router, "eu-west-2").await;
    // The calls below are signed for ANOTHER Region, so an association reporting the IPAM's own
    // Region cannot be an echo of the Region its create was signed for.
    let signing_region = "us-east-1";

    let account = engine::emulator_account_id().to_string();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    // The id each row's create minted, so the narrowing below can assert WHICH association a
    // filter selects rather than only how many.
    let mut minted_per_row: Vec<(&str, String)> = Vec::new();
    for (nth, row) in create_cases().iter().enumerate() {
        let case = row.case;
        let mut params = vec![
            param("IpamId", &ipam.id),
            param("Rir", row.rir),
            param("OrganizationHandle", row.organization_handle),
            param("ClientToken", &format!("token-{nth}")),
        ];
        if let Some(description) = row.description {
            params.push(param("Description", description));
        }
        params.extend(tag_params(&row.tags));

        let body = ok(case, &router, signing_region, CREATE, &params).await;
        let association = created_association(case, &body);
        let association_id = association
            .association_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the created association has no id: {body}"));
        assert!(
            is_long_id(IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX, &association_id),
            "case {case:?}: expected a minted \
             {IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX} id, got {association_id:?}"
        );
        assert!(
            minted.insert(association_id.clone()),
            "case {case:?}: expected an id no earlier create was given, \
             got {association_id:?}"
        );
        minted_per_row.push((case, association_id.clone()));

        // What the service owns, derived rather than echoed.
        assert_eq!(
            association.owner_id.as_deref(),
            Some(account.as_str()),
            "case {case:?}: the association is owned by this account, got {association:?}"
        );
        assert_eq!(
            association.association_arn.as_deref(),
            Some(
                engine::emulator_arn(
                    "ec2",
                    ipam.region,
                    &format!(
                        "{RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION}/{association_id}"
                    )
                )
                .as_str()
            ),
            "case {case:?}: the association reports its own ARN, in the IPAM's Region, \
             got {association:?}"
        );
        assert_eq!(
            association.state.as_deref(),
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_CREATE_IN_PROGRESS),
            "case {case:?}: the association is answered while it is still being created, \
             got {association:?}"
        );
        assert_eq!(
            association.state_message.as_deref(),
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_CREATE_IN_PROGRESS_MESSAGE),
            "case {case:?}: and its message describes that state, got {association:?}"
        );
        assert_eq!(
            association.child_request_xml, None,
            "case {case:?}: no child request document is published for an association whose \
             delegation has not been set up, got {association:?}"
        );

        // What the IPAM decided, taken from the stored IPAM rather than from the request.
        assert_eq!(
            association.ipam_id.as_deref(),
            Some(ipam.id.as_str()),
            "case {case:?}: the association names the IPAM it was created for, \
             got {association:?}"
        );
        assert_eq!(
            association.ipam_region.as_deref(),
            Some(ipam.region),
            "case {case:?}: the association reports that IPAM's home Region, not the Region \
             the create was signed for, got {association:?}"
        );

        // What the request decided, carried through.
        assert_eq!(
            association.rir.as_deref(),
            Some(row.rir),
            "case {case:?}: the association is with the registry asked for, \
             got {association:?}"
        );
        assert_eq!(
            association.organization_handle.as_deref(),
            Some(row.organization_handle),
            "case {case:?}: the association carries the handle asked for, got {association:?}"
        );
        assert_eq!(
            association.description.as_deref(),
            row.description,
            "case {case:?}: the association carries the description asked for, \
             got {association:?}"
        );
        let wanted_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            association.tags, wanted_tags,
            "case {case:?}: the association carries the tags asked for, got {association:?}"
        );

        // The describe reports the stored association, member for member.
        assert_eq!(
            describe_one(case, &router, signing_region, &association_id).await,
            association,
            "case {case:?}: the describe reports the association the create stored"
        );
    }

    // Every association created above is in the store, and the describe narrows by the members
    // the create stored rather than only by id.
    assert_eq!(
        stored_association_ids(&engine),
        minted,
        "the store holds exactly the associations the creates minted"
    );
    for (row, (case, association_id)) in create_cases().iter().zip(minted_per_row.iter()) {
        let case = *case;
        let ids = described_ids(
            case,
            &router,
            signing_region,
            &filters(&[("rir", &[row.rir])]),
        )
        .await;
        assert_eq!(
            ids,
            vec![association_id.clone()],
            "case {case:?}: the registry the create stored selects exactly that association, \
             got {ids:?}"
        );
        let handle_ids = described_ids(
            case,
            &router,
            signing_region,
            &filters(&[("organization-handle", &[row.organization_handle])]),
        )
        .await;
        assert_eq!(
            handle_ids,
            vec![association_id.clone()],
            "case {case:?}: the handle the create stored selects that same association, \
             got {handle_ids:?}"
        );
    }
}

/// The rule: the CLOCK — not a later call — moves a created association out of the transient
/// state the create answered with, so a caller reading straight after the create still sees
/// `create-in-progress`, and the state message moves with the state.
#[tokio::test]
async fn the_clock_settles_a_created_association() {
    let (router, _engine) = emulator();
    let region = "us-east-1";
    let ipam = create_ipam("the IPAM", &router, region).await;

    let case = "an association read straight after its create";
    let body = ok(
        case,
        &router,
        region,
        CREATE,
        &[
            param("IpamId", &ipam.id),
            param("Rir", "arin"),
            param("OrganizationHandle", "EXAMPLE-ORG"),
        ],
    )
    .await;
    let association_id = created_association(case, &body)
        .association_id
        .expect("the created association has an id");
    let straight_after = describe_one(case, &router, region, &association_id).await;
    assert_eq!(
        (
            straight_after.state.as_deref(),
            straight_after.state_message.as_deref()
        ),
        (
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_CREATE_IN_PROGRESS),
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_CREATE_IN_PROGRESS_MESSAGE)
        ),
        "case {case:?}: the association is still being created, got {straight_after:?}"
    );

    tokio::time::sleep(past_the_transition()).await;
    let case = "an association read after the span the clock owes it";
    let settled = describe_one(case, &router, region, &association_id).await;
    assert_eq!(
        (settled.state.as_deref(), settled.state_message.as_deref()),
        (
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_PENDING_ENABLE),
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_PENDING_ENABLE_MESSAGE)
        ),
        "case {case:?}: the clock has settled the association, got {settled:?}"
    );
}

// ---------------------------------------------------------------------------
// What the create refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    params: Vec<Param>,
    status: u16,
    code: &'static str,
}

/// The rule: a request naming something that is not an id of its resource's kind is refused
/// with that resource's `Malformed` code, a well-formed id the account holds no resource for
/// with its `NotFound` code, a required value left empty with EC2's code for a missing
/// parameter, and a value no documented constraint admits with EC2's code for that class — and
/// NOTHING is stored in any of those cases.
#[tokio::test]
async fn the_create_refuses_what_it_cannot_store() {
    let (router, engine) = emulator();
    let region = "us-east-1";
    let ipam = create_ipam("the IPAM", &router, region).await;
    let stored_before = stored_association_ids(&engine);

    let valid = |replacements: &[Param]| -> Vec<Param> {
        let mut params = vec![
            param("IpamId", &ipam.id),
            param("Rir", "arin"),
            param("OrganizationHandle", "EXAMPLE-ORG"),
        ];
        for (name, value) in replacements {
            params.retain(|(existing, _)| existing != name);
            params.push((name.clone(), value.clone()));
        }
        params
    };

    let cases = vec![
        RefusalCase {
            case: "an IPAM id that is not one at all",
            params: valid(&[param("IpamId", "not-an-ipam")]),
            status: 400,
            code: INVALID_IPAM_ID_MALFORMED,
        },
        RefusalCase {
            case: "an IPAM id under another resource's prefix",
            params: valid(&[param("IpamId", "ipam-scope-0123456789abcdef0")]),
            status: 400,
            code: INVALID_IPAM_ID_MALFORMED,
        },
        RefusalCase {
            case: "a well-formed IPAM id the account holds no IPAM for",
            params: valid(&[param("IpamId", "ipam-0ffffffffffffffff")]),
            status: 400,
            code: INVALID_IPAM_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a registry outside the modelled enum",
            params: valid(&[param("Rir", "afrinic")]),
            status: 400,
            code: "ValidationError",
        },
        RefusalCase {
            case: "an organization handle left empty",
            params: valid(&[param("OrganizationHandle", "")]),
            status: 400,
            code: "MissingParameter",
        },
        RefusalCase {
            case: "a tag specification naming another resource type",
            params: {
                let mut params = valid(&[]);
                params.push(param("TagSpecification.1.ResourceType", "ipam-pool"));
                params.push(param("TagSpecification.1.Tag.1.Key", "Name"));
                params.push(param("TagSpecification.1.Tag.1.Value", "wrong-type"));
                params
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a client token longer than this service keeps",
            params: valid(&[param(
                "ClientToken",
                &"t".repeat(MAX_CLIENT_TOKEN_CHARS + 1),
            )]),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a permission check that would otherwise have succeeded",
            params: valid(&[param("DryRun", "true")]),
            status: 412,
            code: "DryRunOperation",
        },
        RefusalCase {
            case: "a permission check whose request is malformed anyway",
            params: valid(&[param("DryRun", "true"), param("IpamId", "not-an-ipam")]),
            status: 400,
            code: INVALID_IPAM_ID_MALFORMED,
        },
    ];

    for row in &cases {
        let case = row.case;
        let (status, body) = call(&router, region, CREATE, &row.params).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {}, got {body}",
            row.code
        );
        assert_eq!(
            stored_association_ids(&engine),
            stored_before,
            "case {case:?}: a refused create stores nothing"
        );
    }
}

/// The rule: one `ClientToken` names the one association it minted — a replay of the same
/// request answers that association and creates nothing, and a replay carrying any different
/// parameter is refused with `IdempotentParameterMismatch`.
#[tokio::test]
async fn one_client_token_names_one_association() {
    let (router, engine) = emulator();
    let region = "us-east-1";
    let ipam = create_ipam("the IPAM", &router, region).await;
    let other_ipam = create_ipam("another IPAM", &router, region).await;

    let first = |replacements: &[Param]| -> Vec<Param> {
        let mut params = vec![
            param("IpamId", &ipam.id),
            param("Rir", "ripe"),
            param("OrganizationHandle", "ORG-EX1-RIPE"),
            param("Description", "the first call"),
            param("ClientToken", "one-token"),
        ];
        for (name, value) in replacements {
            params.retain(|(existing, _)| existing != name);
            params.push((name.clone(), value.clone()));
        }
        params
    };

    let case = "the first call";
    let body = ok(case, &router, region, CREATE, &first(&[])).await;
    let minted = created_association(case, &body);
    let stored_after_first = stored_association_ids(&engine);

    let case = "the same request replayed";
    let replay = ok(case, &router, region, CREATE, &first(&[])).await;
    assert_eq!(
        created_association(case, &replay),
        minted,
        "case {case:?}: the replay answers the association the token minted"
    );
    assert_eq!(
        stored_association_ids(&engine),
        stored_after_first,
        "case {case:?}: the replay created nothing"
    );

    let mismatches: Vec<(&str, Vec<Param>)> = vec![
        (
            "replayed against another IPAM",
            first(&[param("IpamId", &other_ipam.id)]),
        ),
        (
            "replayed for another registry",
            first(&[param("Rir", "arin")]),
        ),
        (
            "replayed with another organization handle",
            first(&[param("OrganizationHandle", "ORG-EX2-RIPE")]),
        ),
        (
            "replayed with another description",
            first(&[param("Description", "something else")]),
        ),
        ("replayed with a tag", {
            let mut params = first(&[]);
            params.extend(tag_params(&[("Name", "late")]));
            params
        }),
    ];
    for (case, params) in &mismatches {
        let (status, body) = call(&router, region, CREATE, params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected {IDEMPOTENT_PARAMETER_MISMATCH}, got {body}"
        );
        assert_eq!(
            stored_association_ids(&engine),
            stored_after_first,
            "case {case:?}: a refused replay stores nothing"
        );
    }
}
