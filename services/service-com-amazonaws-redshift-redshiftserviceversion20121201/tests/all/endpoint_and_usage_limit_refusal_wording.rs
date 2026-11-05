// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE, CODE and STATUS the Redshift-managed VPC endpoint, endpoint
//! authorization and usage-limit operations refuse with, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! Three RULES, each asserted over a table that spans what it generalises over:
//!
//! 1. The fault's status and error code are what the model fixes, while the
//!    MESSAGE belongs to the OPERATION: each one answers the sentence Amazon
//!    Redshift words for it, with the variable part — where it has one — taken
//!    from the request. So two operations asked about the SAME missing name can
//!    answer differently, and one whose sentence has no variable part answers
//!    the same sentence for every name.
//! 2. An endpoint authorization grants ANOTHER account access to this account's
//!    cluster, so `Account` naming the caller's own account is refused 400
//!    `InvalidParameterValue` with a fixed sentence — and refused BEFORE any
//!    name the request carries is resolved, so the refusal does not depend on
//!    whether the cluster exists.
//! 3. A name that resolves is still served, so none of the refusals above is an
//!    "everything is refused" artefact.
//!
//! The tables span the operation, the kind of name that is unresolvable (an
//! endpoint, a cluster, a usage limit), and the spelling, both documented length
//! bounds, the case and the "one character off a stored name" shape of that
//! name, plus a resource this account created and then removed.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};
use crate::endpoint_access::{
    assert_served, available_cluster, borrowed, create_endpoint, create_subnet_group,
    name_of_length, settle, OTHER_ACCOUNT,
};

/// One form parameter, owned so a row can build it from a borrowed name.
fn pair(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// POST one action with owned parameters.
async fn call(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    emulator.call(action, &borrowed(params)).await
}

/// Assert one refusal's status, error code and message.
fn assert_refusal(case: &str, answer: &Answer, status: u16, code: &str, message: &str) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (status, Some(code.to_string())),
        "case {case}: expected {status} {code}, got {} / {:?} with body {}",
        answer.status,
        answer.error_code(),
        answer.body
    );
    assert_eq!(
        answer.text("Message").as_deref(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?} with body {}",
        answer.text("Message"),
        answer.body
    );
}

// ---------------------------------------------------------------------------
// The sentences, spelled out in this suite rather than read off the crate under
// test, so a row cannot pass by restating whatever the handler happened to
// build.
// ---------------------------------------------------------------------------

/// The sentence an operation words its `EndpointNotFound` refusal with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointWording {
    /// `Endpoint <EndpointName> can't be found` — no trailing period.
    EndpointCannotBeFound,
    /// `The endpoint name <EndpointName> can't be found.`
    TheEndpointNameCannotBeFound,
}

/// The message `wording` words about `endpoint_name`.
fn endpoint_sentence(wording: EndpointWording, endpoint_name: &str) -> String {
    match wording {
        EndpointWording::EndpointCannotBeFound => format!("Endpoint {endpoint_name} can't be found"),
        EndpointWording::TheEndpointNameCannotBeFound => {
            format!("The endpoint name {endpoint_name} can't be found.")
        }
    }
}

/// What the endpoint AUTHORIZATION operations answer for a cluster identifier
/// that resolves to nothing: a FIXED sentence with no variable part.
const CLUSTER_IDENTIFIER_CANNOT_BE_FOUND: &str = "The specified cluster identifier can't be found.";

/// What `AuthorizeEndpointAccess` and `RevokeEndpointAccess` answer when
/// `Account` names the caller's own account.
const SELF_ACCOUNT: &str = "Specify an AWS account that is different from the current AWS account.";

/// The message the usage-limit operations word about `usage_limit_id`.
fn usage_limit_sentence(usage_limit_id: &str) -> String {
    format!("Usage limit {usage_limit_id} is not found")
}

// ---------------------------------------------------------------------------
// Rule 1, on an unresolvable endpoint name.
// ---------------------------------------------------------------------------

/// The form parameters one operation sends to ask about a single endpoint name.
type Request = fn(&str) -> Vec<(String, String)>;

fn endpoint_only(endpoint_name: &str) -> Vec<(String, String)> {
    vec![pair("EndpointName", endpoint_name)]
}

/// Every operation that resolves an `EndpointName`, with the sentence each one
/// answers.
fn endpoint_operations() -> Vec<(&'static str, Request, EndpointWording)> {
    vec![
        (
            "DescribeEndpointAccess",
            endpoint_only as Request,
            EndpointWording::EndpointCannotBeFound,
        ),
        (
            "DeleteEndpointAccess",
            endpoint_only as Request,
            EndpointWording::TheEndpointNameCannotBeFound,
        ),
        (
            "ModifyEndpointAccess",
            endpoint_only as Request,
            EndpointWording::TheEndpointNameCannotBeFound,
        ),
    ]
}

#[tokio::test]
async fn every_endpoint_operation_words_its_endpoint_not_found_refusal_its_own_way() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "wording-endpoint-host").await;
    assert_served(
        "seed subnet group",
        &create_subnet_group(&emulator, "wording-endpoint-subnets", &["subnet-a", "subnet-b"])
            .await,
    );
    assert_served(
        "seed the present endpoint",
        &create_endpoint(
            &emulator,
            "wording-present-endpoint",
            "wording-endpoint-host",
            "wording-endpoint-subnets",
            &[],
        )
        .await,
    );
    // An endpoint this account created and then deleted: once the clock-owed
    // removal is due, the name resolves to nothing exactly like one that never
    // existed.
    assert_served(
        "seed the doomed endpoint",
        &create_endpoint(
            &emulator,
            "wording-doomed-endpoint",
            "wording-endpoint-host",
            "wording-endpoint-subnets",
            &[],
        )
        .await,
    );
    settle().await;
    assert_served(
        "delete the doomed endpoint",
        &call(
            &emulator,
            "DeleteEndpointAccess",
            &endpoint_only("wording-doomed-endpoint"),
        )
        .await,
    );
    settle().await;

    // `max_len` is the documented bound on `EndpointName`; see
    // `state::ENDPOINT_NAME_RULE`.
    let names: Vec<(&str, String)> = vec![
        (
            "a plain name nothing created",
            "no-such-endpoint".to_string(),
        ),
        (
            "the shortest name the documented rule admits",
            name_of_length(1),
        ),
        (
            "the longest name the documented rule admits",
            name_of_length(state::ENDPOINT_NAME_RULE.max_len),
        ),
        (
            "a name one character off a stored one",
            "wording-present-endpoin".to_string(),
        ),
        (
            // Uppercase is valid in an endpoint name and nothing folds it, so
            // another spelling of a stored name names no endpoint.
            "a stored name in another case",
            "Wording-Present-Endpoint".to_string(),
        ),
        (
            "an endpoint this account created and then deleted",
            "wording-doomed-endpoint".to_string(),
        ),
    ];

    for (name_case, endpoint_name) in &names {
        for (action, request, wording) in endpoint_operations() {
            let case = format!("{action} on {name_case}");
            let refused = call(&emulator, action, &request(endpoint_name)).await;
            assert_refusal(
                &case,
                &refused,
                404,
                "EndpointNotFound",
                &endpoint_sentence(wording, endpoint_name),
            );
        }
    }

    // The two sentences are DISTINGUISHABLE: the same missing name, asked about
    // through a read and through a write, comes back worded two different ways
    // rather than collapsed onto one template.
    let shared = "wording-collapse-probe";
    let read = call(&emulator, "DescribeEndpointAccess", &endpoint_only(shared)).await;
    let written = call(&emulator, "ModifyEndpointAccess", &endpoint_only(shared)).await;
    assert_ne!(
        read.text("Message"),
        written.text("Message"),
        "case the read and the write on one missing name: expected two different sentences, \
         got {:?} for both",
        read.text("Message")
    );

    // Rule 3: the stored endpoint is still served, so the refusals above are
    // the precondition and not a blanket refusal.
    let served = call(
        &emulator,
        "DescribeEndpointAccess",
        &endpoint_only("wording-present-endpoint"),
    )
    .await;
    assert_served("a stored endpoint is described", &served);
    let modified = call(
        &emulator,
        "ModifyEndpointAccess",
        &endpoint_only("wording-present-endpoint"),
    )
    .await;
    assert_served("a stored endpoint is modified", &modified);
}

// ---------------------------------------------------------------------------
// Rule 1, on an unresolvable cluster identifier, for the endpoint
// AUTHORIZATION operations.
// ---------------------------------------------------------------------------

/// Every operation on an endpoint AUTHORIZATION, with the members it carries
/// beside the cluster identifier it is asked about.
fn authorization_operations() -> Vec<(&'static str, Request)> {
    fn cluster_only(identifier: &str) -> Vec<(String, String)> {
        vec![pair("ClusterIdentifier", identifier)]
    }
    fn cluster_and_other_account(identifier: &str) -> Vec<(String, String)> {
        vec![
            pair("ClusterIdentifier", identifier),
            pair("Account", OTHER_ACCOUNT),
        ]
    }
    vec![
        ("DescribeEndpointAuthorization", cluster_only as Request),
        (
            "AuthorizeEndpointAccess",
            cluster_and_other_account as Request,
        ),
        (
            "RevokeEndpointAccess",
            cluster_and_other_account as Request,
        ),
    ]
}

#[tokio::test]
async fn the_endpoint_authorization_operations_refuse_an_unknown_cluster_with_a_fixed_sentence() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "wording-auth-host").await;
    // A cluster this account created and then deleted.
    let doomed = emulator.create_minimal("wording-auth-doomed").await;
    assert_served("seed the doomed cluster", &doomed);
    let deleted = call(
        &emulator,
        "DeleteCluster",
        &[
            pair("ClusterIdentifier", "wording-auth-doomed"),
            pair("SkipFinalClusterSnapshot", "true"),
        ],
    )
    .await;
    assert_served("delete the doomed cluster", &deleted);
    settle().await;

    // `max_len` is the documented bound on `ClusterIdentifier`; see
    // `state::CLUSTER_IDENTIFIER_RULE`.
    let longest: String = std::iter::once('c')
        .chain(std::iter::repeat_n(
            'x',
            state::CLUSTER_IDENTIFIER_RULE.max_len - 1,
        ))
        .collect();
    let identifiers: Vec<(&str, String)> = vec![
        (
            "a plain identifier nothing created",
            "no-such-cluster".to_string(),
        ),
        (
            "the shortest identifier the documented rule admits",
            "c".to_string(),
        ),
        ("the longest identifier the documented rule admits", longest),
        (
            "an identifier one character off a stored one",
            "wording-auth-hos".to_string(),
        ),
        (
            "an identifier in mixed case",
            "Wording-Auth-Absent".to_string(),
        ),
        (
            "a cluster this account created and then deleted",
            "wording-auth-doomed".to_string(),
        ),
    ];

    for (identifier_case, identifier) in &identifiers {
        for (action, request) in authorization_operations() {
            let case = format!("{action} on {identifier_case}");
            let refused = call(&emulator, action, &request(identifier)).await;
            assert_refusal(
                &case,
                &refused,
                404,
                "ClusterNotFound",
                CLUSTER_IDENTIFIER_CANNOT_BE_FOUND,
            );
            // The invariant that makes this sentence FIXED rather than
            // interpolated: it never echoes the identifier the request named.
            // Checked on the rows whose identifier is long enough not to occur
            // in English prose by accident (a one-letter name does).
            let message = refused.text("Message").unwrap_or_default();
            assert!(
                identifier.len() < 6 || !message.contains(identifier.as_str()),
                "case {case}: expected a sentence that names no identifier, got {message:?}"
            );
        }
    }

    // Rule 3: the stored cluster is served by the same operations.
    assert_served(
        "a stored cluster's authorizations are listed",
        &call(
            &emulator,
            "DescribeEndpointAuthorization",
            &[pair("ClusterIdentifier", "wording-auth-host")],
        )
        .await,
    );
    assert_served(
        "a stored cluster takes an authorization",
        &call(
            &emulator,
            "AuthorizeEndpointAccess",
            &[
                pair("ClusterIdentifier", "wording-auth-host"),
                pair("Account", OTHER_ACCOUNT),
            ],
        )
        .await,
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the self-authorization guard runs before the cluster is resolved.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_authorization_to_the_callers_own_account_is_refused_before_the_cluster_is_resolved() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "wording-self-host").await;
    let own_account = engine::emulator_account_id().to_string();

    // The guard holds whichever side of the cluster lookup the request would
    // land on: a cluster that resolves, one that does not, and one whose
    // identifier is past the documented bound (so the request is wrong in two
    // ways at once).
    let too_long: String = std::iter::once('c')
        .chain(std::iter::repeat_n(
            'x',
            state::CLUSTER_IDENTIFIER_RULE.max_len,
        ))
        .collect();
    let clusters: Vec<(&str, String)> = vec![
        ("a cluster this account holds", "wording-self-host".to_string()),
        ("a cluster nothing created", "no-such-cluster".to_string()),
        (
            "an identifier one character past the documented bound",
            too_long,
        ),
    ];

    for (cluster_case, identifier) in &clusters {
        for action in ["AuthorizeEndpointAccess", "RevokeEndpointAccess"] {
            let case = format!("{action} to the caller's own account on {cluster_case}");
            let refused = call(
                &emulator,
                action,
                &[
                    pair("ClusterIdentifier", identifier),
                    pair("Account", &own_account),
                ],
            )
            .await;
            assert_refusal(&case, &refused, 400, "InvalidParameterValue", SELF_ACCOUNT);
        }
    }

    // The guard is about the ACCOUNT, not a blanket refusal: the same requests
    // naming a third-party account are answered on the cluster — refused when it
    // resolves to nothing, served when it resolves.
    let other_and_missing = call(
        &emulator,
        "AuthorizeEndpointAccess",
        &[
            pair("ClusterIdentifier", "no-such-cluster"),
            pair("Account", OTHER_ACCOUNT),
        ],
    )
    .await;
    assert_refusal(
        "AuthorizeEndpointAccess to a third-party account on a cluster nothing created",
        &other_and_missing,
        404,
        "ClusterNotFound",
        CLUSTER_IDENTIFIER_CANNOT_BE_FOUND,
    );
    assert_served(
        "AuthorizeEndpointAccess to a third-party account on a cluster this account holds",
        &call(
            &emulator,
            "AuthorizeEndpointAccess",
            &[
                pair("ClusterIdentifier", "wording-self-host"),
                pair("Account", OTHER_ACCOUNT),
            ],
        )
        .await,
    );
}

// ---------------------------------------------------------------------------
// Rule 1, on an unresolvable usage-limit id.
// ---------------------------------------------------------------------------

/// Every operation that resolves a `UsageLimitId`, with the members it carries
/// beside it.
fn usage_limit_operations() -> Vec<(&'static str, Request)> {
    fn limit_only(usage_limit_id: &str) -> Vec<(String, String)> {
        vec![pair("UsageLimitId", usage_limit_id)]
    }
    fn modify_limit(usage_limit_id: &str) -> Vec<(String, String)> {
        vec![
            pair("UsageLimitId", usage_limit_id),
            pair("Amount", "100"),
        ]
    }
    vec![
        ("ModifyUsageLimit", modify_limit as Request),
        ("DeleteUsageLimit", limit_only as Request),
    ]
}

/// Create one usage limit on a cluster and answer the id the service minted.
async fn create_limit(emulator: &Emulator, cluster_identifier: &str) -> String {
    // A feature is metered by ONE limit type, so the pairing comes from the
    // model's own catalogue rather than a literal at the call site.
    let feature = "concurrency-scaling";
    let limit_type = state::usage_limit_type_of_feature(feature)
        .expect("concurrency-scaling is one of the model's feature types");
    let created = call(
        emulator,
        "CreateUsageLimit",
        &[
            pair("ClusterIdentifier", cluster_identifier),
            pair("FeatureType", feature),
            pair("LimitType", limit_type),
            pair("Amount", "100"),
        ],
    )
    .await;
    assert_served("seed a usage limit", &created);
    created
        .text("UsageLimitId")
        .expect("a created usage limit reports the id the service minted")
}

#[tokio::test]
async fn the_usage_limit_operations_word_their_not_found_refusal_with_the_id_the_request_named() {
    let emulator = Emulator::new();
    available_cluster(&emulator, "wording-limit-host").await;
    let present = create_limit(&emulator, "wording-limit-host").await;
    let doomed = create_limit(&emulator, "wording-limit-host").await;
    assert_served(
        "delete the doomed usage limit",
        &call(
            &emulator,
            "DeleteUsageLimit",
            &[pair("UsageLimitId", &doomed)],
        )
        .await,
    );

    // The id is service-minted and opaque, so the rows span its SHAPE: a plain
    // word, one character off a minted id, another case of a minted id, a very
    // long value, and a minted id this account deleted.
    let one_off: String = {
        let mut shortened = present.clone();
        shortened.pop();
        shortened
    };
    let ids: Vec<(&str, String)> = vec![
        ("a plain value nothing minted", "no-such-limit".to_string()),
        (
            "a value one character off a minted id",
            one_off,
        ),
        (
            "a minted id in upper case",
            present.to_ascii_uppercase(),
        ),
        (
            "a value far longer than a minted id",
            "u".repeat(128),
        ),
        (
            "a usage limit this account created and then deleted",
            doomed.clone(),
        ),
    ];

    for (id_case, usage_limit_id) in &ids {
        for (action, request) in usage_limit_operations() {
            let case = format!("{action} on {id_case}");
            let refused = call(&emulator, action, &request(usage_limit_id)).await;
            assert_refusal(
                &case,
                &refused,
                404,
                "UsageLimitNotFound",
                &usage_limit_sentence(usage_limit_id),
            );
        }
    }

    // Rule 3: the minted id is still served, and the modify it takes is
    // observable on a later read.
    let modified = call(
        &emulator,
        "ModifyUsageLimit",
        &[pair("UsageLimitId", &present), pair("Amount", "250")],
    )
    .await;
    assert_served("a minted usage limit is modified", &modified);
    let described = call(
        &emulator,
        "DescribeUsageLimits",
        &[pair("UsageLimitId", &present)],
    )
    .await;
    assert_served("a minted usage limit is described", &described);
    assert_eq!(
        described.text("Amount").as_deref(),
        Some("250"),
        "case a minted usage limit is described: expected the modified amount 250, got {:?} \
         with body {}",
        described.text("Amount"),
        described.body
    );
}
