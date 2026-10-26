// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateIpamScope` and the `DescribeIpamScopes` that reads it back,
//! driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * a create MINTS the resource in the container the request names and STORES it, taking
//!   the container's own facts (here: the IPAM's ARN and its home Region) from the stored
//!   container rather than from the request, and deriving everything else the service owns —
//!   the id, the ARN, the owning account, the pool count, the fact that a caller-created
//!   scope is not one of the IPAM's defaults, and the non-terminal state the clock then owes
//!   the terminal one;
//! * a container id the account holds no container for is refused with that resource's own
//!   `NotFound` code, and something that is not an id of that kind at all with its
//!   `Malformed` code, whatever else the request carries;
//! * one `ClientToken` names the one scope it minted: a replay answers that scope, and a
//!   replay asking for anything else is refused;
//! * every member of the created scope reads back through `DescribeIpamScopes`, which also
//!   narrows by id, by the documented filters and by the page size.
//!
//! Each table row is a distinct input along what those generalise over: which optional
//! member the request carries, how many tags it names, which member is at fault, and how
//! many scopes the store holds.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::state::{
    IPAM_SCOPE_ID_PREFIX, IPAM_SCOPE_TYPE_PRIVATE, IPAM_SCOPE_TYPE_PUBLIC,
    INVALID_IPAM_ID_MALFORMED, INVALID_IPAM_ID_NOT_FOUND, INVALID_IPAM_SCOPE_ID_NOT_FOUND,
    MAX_CLIENT_TOKEN_CHARS, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region.
const REGION: &str = "us-east-1";

/// The one external authority type the model offers
/// (`smithy_query(query="shape",shape="IpamScopeExternalAuthorityType")` is `infoblox`).
const EXTERNAL_AUTHORITY_TYPE: &str = "infoblox";

/// Longer than the span the clock owes a `create-in-progress` scope, so a read after this
/// much has certainly passed the due instant.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
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
/// scope of it reports, and the two default scopes it came up with.
struct CreatedIpam {
    /// The `IpamId` a `CreateIpamScope` request names.
    id: String,
    /// "The ARN of the IPAM" (`IpamScope$IpamArn`), which every scope of it reports.
    arn: String,
    /// The ids of the two scopes "an IPAM contains" (`IpamScope`) on creation.
    default_scopes: Vec<String>,
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
        default_scopes: vec![
            read("publicDefaultScopeId"),
            read("privateDefaultScopeId"),
        ],
    }
}

/// One IPAM scope as a response reports it: every member read off the wire, with the tags and
/// the external authority configuration flattened into comparable values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedScope {
    owner_id: Option<String>,
    scope_id: Option<String>,
    scope_arn: Option<String>,
    ipam_arn: Option<String>,
    ipam_region: Option<String>,
    scope_type: Option<String>,
    is_default: Option<String>,
    description: Option<String>,
    pool_count: Option<String>,
    state: Option<String>,
    tags: Vec<(String, String)>,
    external_authority: Option<(Option<String>, Option<String>)>,
}

/// The scope element of a response as [`ReportedScope`].
fn read_scope(node: roxmltree::Node<'_, '_>) -> ReportedScope {
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
    ReportedScope {
        owner_id: child_text(node, "ownerId"),
        scope_id: child_text(node, "ipamScopeId"),
        scope_arn: child_text(node, "ipamScopeArn"),
        ipam_arn: child_text(node, "ipamArn"),
        ipam_region: child_text(node, "ipamRegion"),
        scope_type: child_text(node, "ipamScopeType"),
        is_default: child_text(node, "isDefault"),
        description: child_text(node, "description"),
        pool_count: child_text(node, "poolCount"),
        state: child_text(node, "state"),
        tags,
        external_authority: child(node, "externalAuthorityConfiguration").map(|config| {
            (
                child_text(config, "type"),
                child_text(config, "externalResourceIdentifier"),
            )
        }),
    }
}

/// The one scope a `CreateIpamScope` response carries.
fn created_scope(case: &str, body: &str) -> ReportedScope {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamScopeId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM scope (no <ipamScopeId>) in {body}"));
    read_scope(node)
}

/// Every scope a `DescribeIpamScopes` response reports, in the order it reported them.
fn described_scopes(case: &str, body: &str) -> Vec<ReportedScope> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "ipamScopeSet")
    else {
        return Vec::new();
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(read_scope)
        .collect()
}

/// The ids every `DescribeIpamScopes` body in `body` reports.
fn described_ids(case: &str, body: &str) -> Vec<String> {
    described_scopes(case, body)
        .into_iter()
        .filter_map(|scope| scope.scope_id)
        .collect()
}

/// `DescribeIpamScopes` narrowed to one id, as the one scope it reports.
async fn describe_one(case: &str, router: &MultiServiceRouter, scope_id: &str) -> ReportedScope {
    let body = ok(
        case,
        router,
        "DescribeIpamScopes",
        &[param("IpamScopeId.1", scope_id)],
    )
    .await;
    let mut scopes = described_scopes(case, &body);
    assert_eq!(
        scopes.len(),
        1,
        "case {case:?}: expected exactly the scope {scope_id} back, got {body}"
    );
    scopes.remove(0)
}

/// The form fields for one `ipam-scope` tag specification over `tags`.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param("TagSpecification.1.ResourceType", "ipam-scope")];
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

/// The form fields for an `ExternalAuthorityConfiguration`.
fn authority_params(authority_type: &str, identifier: &str) -> Vec<Param> {
    vec![
        param("ExternalAuthorityConfiguration.Type", authority_type),
        param(
            "ExternalAuthorityConfiguration.ExternalResourceIdentifier",
            identifier,
        ),
    ]
}

// ---------------------------------------------------------------------------
// What the create stores
// ---------------------------------------------------------------------------

/// One row of the create table: the case, the optional members the request carries, the
/// description and tags it therefore asks for, and the external authority it links.
struct CreateCase {
    case: &'static str,
    description: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
    authority: Option<(&'static str, &'static str)>,
}

/// The rule: `CreateIpamScope` stores one further scope in the IPAM the request names,
/// carrying exactly what the request described, with the IPAM's own facts taken from the
/// stored IPAM and every member the service owns derived — and every one of those members
/// reads back through `DescribeIpamScopes`.
#[tokio::test]
async fn create_ipam_scope_stores_the_scope_the_request_describes() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM the scopes are created in", &router).await;

    let cases = [
        CreateCase {
            case: "no optional member at all",
            description: None,
            tags: Vec::new(),
            authority: None,
        },
        CreateCase {
            case: "a description",
            description: Some("the finance network"),
            tags: Vec::new(),
            authority: None,
        },
        CreateCase {
            case: "a description and one tag",
            description: Some("one tag"),
            tags: vec![("Name", "scope-alpha")],
            authority: None,
        },
        CreateCase {
            case: "three tags and no description",
            description: None,
            tags: vec![("Name", "scope-beta"), ("Team", "network"), ("Env", "prod")],
            authority: None,
        },
        CreateCase {
            case: "an external authority configuration",
            description: None,
            tags: Vec::new(),
            authority: Some((
                EXTERNAL_AUTHORITY_TYPE,
                "v1.identity.account.realm-a.entity-1",
            )),
        },
        CreateCase {
            case: "every optional member at once",
            description: Some("everything"),
            tags: vec![("Name", "scope-gamma"), ("Env", "test")],
            authority: Some((
                EXTERNAL_AUTHORITY_TYPE,
                "v2.identity.account.realm-b.entity-2",
            )),
        },
    ];

    let account = engine::emulator_account_id().to_string();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (nth, row) in cases.iter().enumerate() {
        let case = row.case;
        let mut params = vec![
            param("IpamId", &ipam.id),
            param("ClientToken", &format!("token-{nth}")),
        ];
        if let Some(description) = row.description {
            params.push(param("Description", description));
        }
        params.extend(tag_params(&row.tags));
        if let Some((authority_type, identifier)) = row.authority {
            params.extend(authority_params(authority_type, identifier));
        }

        let body = ok(case, &router, "CreateIpamScope", &params).await;
        let scope = created_scope(case, &body);
        let scope_id = scope
            .scope_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the created scope has no id: {body}"));
        assert!(
            is_long_id(IPAM_SCOPE_ID_PREFIX, &scope_id),
            "case {case:?}: expected a minted {IPAM_SCOPE_ID_PREFIX} id, got {scope_id:?}"
        );
        assert!(
            minted.insert(scope_id.clone()),
            "case {case:?}: expected a scope id no earlier create was given, got {scope_id:?}"
        );

        // What the service owns, derived rather than echoed.
        assert_eq!(
            scope.owner_id.as_deref(),
            Some(account.as_str()),
            "case {case:?}: the scope is owned by this account, got {scope:?}"
        );
        assert_eq!(
            scope.scope_arn.as_deref(),
            Some(
                engine::emulator_arn("ec2", "", &format!("ipam-scope/{scope_id}")).as_str()
            ),
            "case {case:?}: the scope reports its own ARN, got {scope:?}"
        );
        assert_eq!(
            scope.ipam_arn.as_deref(),
            Some(ipam.arn.as_str()),
            "case {case:?}: the scope names the IPAM it was created in, got {scope:?}"
        );
        assert_eq!(
            scope.ipam_region.as_deref(),
            Some(REGION),
            "case {case:?}: the scope is in the IPAM's home Region, got {scope:?}"
        );
        assert_eq!(
            scope.scope_type.as_deref(),
            Some(IPAM_SCOPE_TYPE_PRIVATE),
            "case {case:?}: a created scope holds private address space, got {scope:?}"
        );
        assert_eq!(
            scope.is_default.as_deref(),
            Some("false"),
            "case {case:?}: a created scope is not one of the IPAM's defaults, got {scope:?}"
        );
        assert_eq!(
            scope.pool_count.as_deref(),
            Some("0"),
            "case {case:?}: nothing has created a pool in the new scope, got {scope:?}"
        );
        assert_eq!(
            scope.state.as_deref(),
            Some("create-in-progress"),
            "case {case:?}: the scope is answered while it is still being created, \
             got {scope:?}"
        );

        // What the request decided, carried through.
        assert_eq!(
            scope.description.as_deref(),
            row.description,
            "case {case:?}: the scope carries the description the request asked for, \
             got {scope:?}"
        );
        let wanted_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            scope.tags, wanted_tags,
            "case {case:?}: the scope carries the tags the request asked for, got {scope:?}"
        );
        let wanted_authority = row.authority.map(|(authority_type, identifier)| {
            (
                Some(authority_type.to_string()),
                Some(identifier.to_string()),
            )
        });
        assert_eq!(
            scope.external_authority, wanted_authority,
            "case {case:?}: the scope links the external authority the request named, \
             got {scope:?}"
        );

        // The round trip: every member reads back through the describe.
        let read_back = describe_one(case, &router, &scope_id).await;
        assert_eq!(
            read_back, scope,
            "case {case:?}: the describe reports the scope the create answered"
        );
    }

    // The two default scopes are untouched, and the IPAM now counts every scope in it:
    // "the number of scopes in the IPAM" (`Ipam$ScopeCount`).
    let all = ok("the whole store", &router, "DescribeIpamScopes", &[]).await;
    let found: BTreeSet<String> = described_ids("the whole store", &all).into_iter().collect();
    let mut expected = minted.clone();
    expected.extend(ipam.default_scopes.iter().cloned());
    assert_eq!(
        found, expected,
        "the describe reports the two default scopes and every created one, got {all}"
    );
    let described_ipam = ok(
        "the IPAM's scope count",
        &router,
        "DescribeIpams",
        &[param("IpamId.1", &ipam.id)],
    )
    .await;
    let parsed = document("the IPAM's scope count", &described_ipam);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
        .expect("the described IPAM");
    assert_eq!(
        child_text(node, "scopeCount"),
        Some(expected.len().to_string()),
        "the IPAM counts the scopes the store holds in it, got {described_ipam}"
    );
}

/// The rule: a scope `CreateIpamScope` answers is in the non-terminal `create-in-progress`
/// state, and the clock owes it `create-complete` whether or not anybody calls again — a
/// caller reading straight back still sees the transient state.
#[tokio::test]
async fn a_created_scope_completes_on_its_own() {
    let case = "the clock completes the scope";
    let (router, _engine) = emulator();
    let ipam = create_ipam(case, &router).await;
    let body = ok(
        case,
        &router,
        "CreateIpamScope",
        &[param("IpamId", &ipam.id)],
    )
    .await;
    let scope_id = created_scope(case, &body)
        .scope_id
        .expect("the created scope has an id");

    // Straight back, before the due instant: still being created.
    assert_eq!(
        describe_one(case, &router, &scope_id).await.state.as_deref(),
        Some("create-in-progress"),
        "case {case:?}: a caller reading straight back sees the transient state"
    );

    tokio::time::sleep(past_the_transition()).await;
    assert_eq!(
        describe_one(case, &router, &scope_id).await.state.as_deref(),
        Some("create-complete"),
        "case {case:?}: the scope completes once the transition is due"
    );
}

// ---------------------------------------------------------------------------
// What the create refuses
// ---------------------------------------------------------------------------

/// The rule: a request the documented contract does not admit is refused with the code that
/// names what was wrong — the IPAM's `Malformed` code for a value that is not an IPAM id at
/// all, its `NotFound` code for a well-formed id the account holds no IPAM for, and
/// `DryRunOperation` for a permission check, whose answer does not depend on whether the
/// resources the request names exist — and NOTHING is stored.
#[tokio::test]
async fn create_ipam_scope_refuses_what_the_contract_does_not_admit() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM the refusals are aimed at", &router).await;
    let unknown_ipam = "ipam-00000000000000000";
    let long_token = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);

    // (case, params, expected status, expected code)
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "a value that is no IPAM id at all",
            vec![param("IpamId", "not-an-ipam")],
            400,
            INVALID_IPAM_ID_MALFORMED,
        ),
        (
            "an IPAM id with a non-hex suffix",
            vec![param("IpamId", "ipam-zzzzzzzz")],
            400,
            INVALID_IPAM_ID_MALFORMED,
        ),
        (
            "an IPAM id of the wrong length",
            vec![param("IpamId", "ipam-0123456")],
            400,
            INVALID_IPAM_ID_MALFORMED,
        ),
        (
            "an IPAM scope id where an IPAM id belongs",
            vec![param("IpamId", "ipam-scope-00000000000000000")],
            400,
            INVALID_IPAM_ID_MALFORMED,
        ),
        (
            "a well-formed IPAM id the account holds no IPAM for",
            vec![param("IpamId", unknown_ipam)],
            400,
            INVALID_IPAM_ID_NOT_FOUND,
        ),
        (
            "a short-form IPAM id the account holds no IPAM for",
            vec![param("IpamId", "ipam-0000000f")],
            400,
            INVALID_IPAM_ID_NOT_FOUND,
        ),
        (
            "a tag specification for another resource type",
            vec![
                param("IpamId", &ipam.id),
                param("TagSpecification.1.ResourceType", "ipam-pool"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "wrong"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag specification naming no resource type",
            vec![
                param("IpamId", &ipam.id),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "no type"),
            ],
            400,
            "MissingParameter",
        ),
        (
            "an external authority with a type and no identifier",
            vec![
                param("IpamId", &ipam.id),
                param("ExternalAuthorityConfiguration.Type", EXTERNAL_AUTHORITY_TYPE),
            ],
            400,
            "MissingParameter",
        ),
        (
            "an external authority with an identifier and no type",
            vec![
                param("IpamId", &ipam.id),
                param(
                    "ExternalAuthorityConfiguration.ExternalResourceIdentifier",
                    "v1.identity.account.realm-a.entity-1",
                ),
            ],
            400,
            "MissingParameter",
        ),
        (
            "a client token past the length this service keeps",
            vec![param("IpamId", &ipam.id), param("ClientToken", &long_token)],
            400,
            "InvalidParameterValue",
        ),
        (
            "a dry run that would have succeeded",
            vec![param("IpamId", &ipam.id), param("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
        (
            // "Checks whether you have the required permissions … without actually making
            // the request", so the answer does not depend on the resources it names.
            "a dry run naming an IPAM that does not exist",
            vec![param("IpamId", unknown_ipam), param("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, status, code) in &cases {
        let (answered, body) = invoke(&router, "CreateIpamScope", params).await;
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

    // The refusal for an IPAM that does not exist names the id the REQUEST carried.
    let (_status, body) = invoke(
        &router,
        "CreateIpamScope",
        &[param("IpamId", unknown_ipam)],
    )
    .await;
    assert_eq!(
        error_message(&body),
        format!("The IPAM ({unknown_ipam}) does not exist."),
        "the refusal names the IPAM the request asked for, got {body}"
    );

    // None of the refusals above stored anything: the IPAM still holds exactly the two
    // default scopes it came up with.
    let all = ok("nothing was stored", &router, "DescribeIpamScopes", &[]).await;
    assert_eq!(
        described_ids("nothing was stored", &all)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        ipam.default_scopes.iter().cloned().collect::<BTreeSet<_>>(),
        "a refused create stores nothing, got {all}"
    );
}

/// The rule: "a unique, case-sensitive identifier that you provide to ensure the idempotency
/// of the request" (`CreateIpamScopeRequest$ClientToken`) — one token names the one scope it
/// minted, so a repeat of the request reports that scope and creates nothing, a different
/// token creates a second one, and a replay asking for anything else is refused.
#[tokio::test]
async fn a_client_token_makes_create_ipam_scope_idempotent() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM the tokens create scopes in", &router).await;

    // (case, token, description)
    let cases = [
        ("a uuid-shaped token", "6f3c9a1e-0b1d-4b7f-9f14-2a7e2a0f9c11", None),
        ("a short token", "t1", Some("the short one")),
        ("a token at the length this service keeps", &"z".repeat(MAX_CLIENT_TOKEN_CHARS), Some("the long one")),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, token, description) in cases {
        let mut params = vec![param("IpamId", &ipam.id), param("ClientToken", token)];
        if let Some(description) = description {
            params.push(param("Description", description));
        }
        let first = ok(case, &router, "CreateIpamScope", &params).await;
        let first_id = created_scope(case, &first)
            .scope_id
            .unwrap_or_else(|| panic!("case {case:?}: the created scope has no id: {first}"));

        let replay = ok(case, &router, "CreateIpamScope", &params).await;
        assert_eq!(
            created_scope(case, &replay).scope_id.as_deref(),
            Some(first_id.as_str()),
            "case {case:?}: a replayed token answers the scope it minted, got {replay}"
        );

        // A replay asking for something else is refused rather than answered with a scope
        // the caller did not ask for.
        let different = vec![
            param("IpamId", &ipam.id),
            param("ClientToken", token),
            param("Description", "something else entirely"),
        ];
        let (status, body) = invoke(&router, "CreateIpamScope", &different).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token replayed with other parameters is refused, \
             got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: a token replayed with other parameters is refused as a \
             mismatch, got {body}"
        );

        minted.push(first_id);
    }

    // Three tokens, three scopes — and the replays created none.
    let all = ok("one scope per token", &router, "DescribeIpamScopes", &[]).await;
    let found: BTreeSet<String> = described_ids("one scope per token", &all)
        .into_iter()
        .collect();
    let mut expected: BTreeSet<String> = minted.iter().cloned().collect();
    assert_eq!(
        minted.len(),
        expected.len(),
        "each token minted a scope of its own, got {minted:?}"
    );
    expected.extend(ipam.default_scopes.iter().cloned());
    assert_eq!(
        found, expected,
        "a replayed token creates nothing, got {all}"
    );
}

// ---------------------------------------------------------------------------
// What the describe reports
// ---------------------------------------------------------------------------

/// The rule: `DescribeIpamScopes` reports the scopes the store holds, narrowed by the ids the
/// request names and by every filter it carries (joined with AND), and refuses a filter name
/// it does not resolve and an id that names no scope.
#[tokio::test]
async fn describe_ipam_scopes_narrows_by_ids_and_filters() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM the scopes belong to", &router).await;

    let tagged = {
        let case = "the tagged scope";
        let mut params = vec![
            param("IpamId", &ipam.id),
            param("Description", "the tagged scope"),
        ];
        params.extend(tag_params(&[("Name", "tagged"), ("Env", "prod")]));
        let body = ok(case, &router, "CreateIpamScope", &params).await;
        created_scope(case, &body).scope_id.expect("an id")
    };
    let linked = {
        let case = "the externally linked scope";
        let mut params = vec![param("IpamId", &ipam.id)];
        params.extend(authority_params(
            EXTERNAL_AUTHORITY_TYPE,
            "v3.identity.account.realm-c.entity-3",
        ));
        let body = ok(case, &router, "CreateIpamScope", &params).await;
        created_scope(case, &body).scope_id.expect("an id")
    };
    let public_default = ipam.default_scopes[0].clone();
    let private_default = ipam.default_scopes[1].clone();

    // (case, filter name, filter values, the ids the filter selects)
    let cases: Vec<(&str, &str, Vec<&str>, Vec<String>)> = vec![
        (
            "by the scope type the defaults and the creates share",
            "ipam-scope-type",
            vec![IPAM_SCOPE_TYPE_PRIVATE],
            vec![private_default.clone(), tagged.clone(), linked.clone()],
        ),
        (
            "by the public scope type, which only a default scope has",
            "ipam-scope-type",
            vec![IPAM_SCOPE_TYPE_PUBLIC],
            vec![public_default.clone()],
        ),
        (
            "by the default flag",
            "is-default",
            vec!["true"],
            vec![public_default.clone(), private_default.clone()],
        ),
        (
            "by the default flag being off",
            "is-default",
            vec!["false"],
            vec![tagged.clone(), linked.clone()],
        ),
        (
            "by a description wildcard",
            "description",
            vec!["the tagged*"],
            vec![tagged.clone()],
        ),
        (
            "by a tag key",
            "tag-key",
            vec!["Env"],
            vec![tagged.clone()],
        ),
        (
            "by a tag value",
            "tag:Name",
            vec!["tagged"],
            vec![tagged.clone()],
        ),
        (
            // An entry of the filter list the created scopes exercise through their external
            // authority rather than through any member the issue cited.
            "by the external authority type",
            "external-authority-type",
            vec![EXTERNAL_AUTHORITY_TYPE],
            vec![linked.clone()],
        ),
        (
            "by the external authority's resource identifier",
            "external-authority-external-resource-identifier",
            vec!["v3.identity.account.realm-c.entity-3"],
            vec![linked.clone()],
        ),
        (
            "by the pool count every scope still has",
            "pool-count",
            vec!["0"],
            vec![
                public_default.clone(),
                private_default.clone(),
                tagged.clone(),
                linked.clone(),
            ],
        ),
        (
            "by the IPAM every scope belongs to",
            "ipam-arn",
            vec![ipam.arn.as_str()],
            vec![
                public_default.clone(),
                private_default.clone(),
                tagged.clone(),
                linked.clone(),
            ],
        ),
        (
            "by a state no scope is in",
            "state",
            vec!["delete-complete"],
            Vec::new(),
        ),
        (
            "by an owner no scope has",
            "owner-id",
            vec!["111122223333"],
            Vec::new(),
        ),
    ];

    for (case, name, values, expected) in &cases {
        let mut params = vec![param("Filter.1.Name", name)];
        for (index, value) in values.iter().enumerate() {
            params.push(param(&format!("Filter.1.Value.{}", index + 1), value));
        }
        let body = ok(case, &router, "DescribeIpamScopes", &params).await;
        let found: BTreeSet<String> = described_ids(case, &body).into_iter().collect();
        let wanted: BTreeSet<String> = expected.iter().cloned().collect();
        assert_eq!(
            found, wanted,
            "case {case:?}: expected the filter to select {wanted:?}, got {body}"
        );
    }

    // Two filters hold together with AND.
    let case = "two filters";
    let body = ok(
        case,
        &router,
        "DescribeIpamScopes",
        &[
            param("Filter.1.Name", "is-default"),
            param("Filter.1.Value.1", "false"),
            param("Filter.2.Name", "tag-key"),
            param("Filter.2.Value.1", "Name"),
        ],
    )
    .await;
    assert_eq!(
        described_ids(case, &body),
        vec![tagged.clone()],
        "case {case:?}: two filters are joined with AND, got {body}"
    );

    // An id that names no scope, and a filter name the operation does not resolve.
    let (status, body) = invoke(
        &router,
        "DescribeIpamScopes",
        &[param("IpamScopeId.1", "ipam-scope-00000000000000000")],
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown scope id is refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_IPAM_SCOPE_ID_NOT_FOUND,
        "an unknown scope id is refused as not found, got {body}"
    );
    let (status, body) = invoke(
        &router,
        "DescribeIpamScopes",
        &[
            param("Filter.1.Name", "scope-colour"),
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
}

/// The rule: `DescribeIpamScopes` answers at most `MaxResults` scopes and a `NextToken`
/// naming where the next page starts, that token continues from the end of the page before
/// it, and "this value is null when there are no more results to return"
/// (`DescribeIpamScopesResult$NextToken`). A token the service did not mint is refused.
#[tokio::test]
async fn describe_ipam_scopes_pages_in_a_stable_order() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM the pages come from", &router).await;
    // The model's `IpamMaxResults` has a minimum of 5, so paging needs more than five scopes
    // for a second page to exist at all: the IPAM's two defaults plus five created ones.
    let mut expected: BTreeSet<String> = ipam.default_scopes.iter().cloned().collect();
    for nth in 0..5 {
        let case = format!("scope {nth}");
        let body = ok(
            &case,
            &router,
            "CreateIpamScope",
            &[
                param("IpamId", &ipam.id),
                param("ClientToken", &format!("page-{nth}")),
            ],
        )
        .await;
        expected.insert(
            created_scope(&case, &body)
                .scope_id
                .unwrap_or_else(|| panic!("case {case:?}: the created scope has no id")),
        );
    }

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
            let body = ok(&case, &router, "DescribeIpamScopes", &params).await;
            let page = described_ids(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size} scopes, got {body}"
            );
            seen.extend(page);
            token = super::ec2_query::element(&body, "nextToken");
            pages += 1;
            assert!(
                pages <= 5,
                "case {case:?}: the paging must terminate, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen.len(),
            "case {case:?}: no scope is reported twice across the pages, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            expected,
            "case {case:?}: the pages together report every stored scope"
        );
        assert_eq!(
            seen.clone(),
            {
                let mut ordered = seen.clone();
                ordered.sort();
                ordered
            },
            "case {case:?}: the pages are reported in a stable id order"
        );
    }

    // A token this service did not mint names no position in that order.
    let (status, body) = invoke(
        &router,
        "DescribeIpamScopes",
        &[param("NextToken", "not-a-token-this-service-minted")],
    )
    .await;
    assert_eq!(
        status, 400,
        "a token the service did not mint is refused, got {status} with body {body}"
    );

    // Reading the pages changed nothing: the whole store still reports the same scopes, and
    // each one still carries the members it was created with.
    let all = ok("the whole store", &router, "DescribeIpamScopes", &[]).await;
    assert_eq!(
        described_ids("the whole store", &all)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        expected,
        "the reads report the scopes that were created and nothing else, got {all}"
    );
    let parsed = document("the whole store", &all);
    let set = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "ipamScopeSet")
        .expect("the reported scope set");
    for item in set_items(set.parent().expect("the result element"), "ipamScopeSet") {
        let scope = read_scope(item);
        assert_eq!(
            scope.ipam_arn.as_deref(),
            Some(ipam.arn.as_str()),
            "every reported scope belongs to the one IPAM, got {scope:?}"
        );
    }
}
