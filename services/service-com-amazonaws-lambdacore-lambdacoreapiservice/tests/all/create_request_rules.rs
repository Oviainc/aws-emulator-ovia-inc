// SPDX-License-Identifier: CC0-1.0

//! What `CreateNetworkConnector` REFUSES, and the sentence it refuses with.
//!
//! Three rules, each asserted over a table that varies along what it generalises over
//! rather than over the one payload a recording happened to carry:
//!
//! 1. the connector type the request's `Configuration` selects decides whether
//!    `OperatorRole` is required — the role is "the IAM role that Lambda assumes to manage
//!    elastic network interfaces in your VPC", so a connector whose type provisions those
//!    interfaces cannot be created without one, and the request is refused with HTTP 400 /
//!    `InvalidParameterValueException` before anything is stored;
//! 2. a `ClientToken` stands for the request it was first presented with — "if you retry a
//!    request that completed successfully using the same client token, the operation returns
//!    the existing connector without creating a duplicate" — so the same token carrying a
//!    DIFFERENT parameter set is not a retry and is refused with HTTP 409 /
//!    `ResourceConflictException`, whichever member changed, while an identical retry still
//!    replays;
//! 3. a `Name` is "a unique name for the network connector within your account and Region",
//!    so a second create under a name the account holds is refused with HTTP 409 /
//!    `ResourceConflictException` naming it.
//!
//! Every refusal is also asserted to have changed NOTHING: the account still holds exactly
//! the connectors it held, with the configuration and identity it held them under.
//!
//! The harness (router, request helpers, the clock wait) is the lifecycle suite's.

use serde_json::json;

use super::network_connector_lifecycle::{
    at, create, emulator, get, list, minimal_configuration, minimal_create, text, update,
    vpc_egress, wait_for_the_clock, Answer, OPERATOR_ROLE,
};

/// The only value `NetworkConnectorType` declares
/// (`smithy_query(query="shape",shape="NetworkConnectorType")`), and the type a
/// `VpcEgressConfiguration` selects.
const VPC_EGRESS: &str = "VPC_EGRESS";

/// A second operator role, for a row that changes only that member.
const OTHER_OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/LambdaNetworkConnectorStandby";

/// Assert `answer` is the modelled error `code` at `status`, carrying exactly `message`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str, message: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        answer.status, answer.body
    );
    let seen_code = text(&answer.body, "/__type");
    assert!(
        seen_code.ends_with(code),
        "case {case:?}: expected the error code {code}, got {seen_code:?}"
    );
    let seen_message = text(&answer.body, "/message");
    assert_eq!(
        seen_message, message,
        "case {case:?}: expected the message {message:?}, got {seen_message:?}"
    );
}

/// The names `ListNetworkConnectors` reports, in the order it reports them.
fn listed_names(answer: &Answer) -> Vec<String> {
    answer.body["NetworkConnectors"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListNetworkConnectors must return a NetworkConnectors list: {:?}",
                answer.body
            )
        })
        .iter()
        .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE 1: `OperatorRole` is required for a connector whose type provisions network
/// interfaces in the caller's VPC, and the refusal names that type.
///
/// The rows differ along every member of the configuration the rule does NOT depend on —
/// how many subnets, whether security groups are named at all, which network protocol,
/// which compute resource type, and whether the request also carries tags or a client
/// token — so none of them can be satisfied by a case keyed to one payload's shape.
#[tokio::test]
async fn a_connector_type_that_provisions_interfaces_requires_an_operator_role() {
    let expected = format!("NetworkConnectorOperatorRole is required for {VPC_EGRESS} connector type");
    let cases = vec![
        (
            "one subnet, nothing else",
            json!({ "Name": "role-minimal", "Configuration": minimal_configuration() }),
        ),
        (
            "several subnets and security groups, IPv4, MicroVm",
            json!({
                "Name": "role-many",
                "Configuration": vpc_egress(
                    &[
                        "subnet-0123456789abcdef0",
                        "subnet-0fedcba9876543210",
                        "subnet-00112233445566778",
                    ],
                    &["sg-0123456789abcdef0", "sg-0fedcba9876543210"],
                    Some("IPv4"),
                    Some("MicroVm"),
                ),
            }),
        ),
        (
            "dual-stack networking for a web function",
            json!({
                "Name": "role-dualstack",
                "Configuration": vpc_egress(
                    &["subnet-0123456789abcdef0"],
                    &["sg-0123456789abcdef0"],
                    Some("DualStack"),
                    Some("WebFunction"),
                ),
            }),
        ),
        (
            "tags as well, which say nothing about the role",
            json!({
                "Name": "role-tagged",
                "Configuration": minimal_configuration(),
                "Tags": { "env": "test", "team": "networking" },
            }),
        ),
        (
            "a client token as well, so the refusal is not an idempotency answer",
            json!({
                "Name": "role-tokened",
                "Configuration": minimal_configuration(),
                "ClientToken": "token-without-a-role",
            }),
        ),
    ];

    let router = emulator();
    for (case, request) in &cases {
        let refused = create(&router, request.clone()).await;
        assert_refused(
            case,
            &refused,
            400,
            "InvalidParameterValueException",
            &expected,
        );

        // The refusal creates nothing: neither a read nor a list knows the name.
        let name = text(request, "/Name");
        let fetched = get(&router, &name).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: a refused create stores no connector, but \
             GetNetworkConnector answered {} with {:?}",
            fetched.status, fetched.body
        );
    }
    let listed = list(&router, "").await;
    assert_eq!(
        listed_names(&listed),
        Vec::<String>::new(),
        "no refused create may leave a connector behind, got {:?}",
        listed.body
    );

    // The same payloads WITH the role are accepted, so the rule refuses the omission and
    // nothing else.
    for (case, request) in &cases {
        let mut accepted_request = request.clone();
        accepted_request["OperatorRole"] = json!(OPERATOR_ROLE);
        let accepted = create(&router, accepted_request).await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: the same request carrying an OperatorRole must be accepted, \
             got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            at(&accepted.body, "/State"),
            json!("PENDING"),
            "case {case:?}: \"the network connector starts in PENDING state\", got {:?}",
            at(&accepted.body, "/State")
        );
        let name = text(request, "/Name");
        let fetched = get(&router, &name).await;
        assert_eq!(
            text(&fetched.body, "/OperatorRole"),
            OPERATOR_ROLE,
            "case {case:?}: GetNetworkConnector must report the role the create stored, \
             got {:?}",
            at(&fetched.body, "/OperatorRole")
        );
    }
}

/// RULE 2: a `ClientToken` replays only the request it was first presented with; the same
/// token carrying any other parameter set is refused, and changes nothing.
///
/// The rows differ in exactly ONE member each, spanning every member a create carries
/// besides the token itself — the name, each member of the configuration, the operator role
/// and the tags.
#[tokio::test]
async fn a_client_token_carrying_other_parameters_is_refused() {
    let expected = "A request with the same client token but different parameters was already submitted";
    let token = "retry-token";
    let name = "token-bound";
    let configuration = vpc_egress(
        &["subnet-0123456789abcdef0"],
        &["sg-0123456789abcdef0"],
        Some("IPv4"),
        Some("MicroVm"),
    );
    let first_request = json!({
        "Name": name,
        "Configuration": configuration,
        "OperatorRole": OPERATOR_ROLE,
        "Tags": { "env": "test" },
        "ClientToken": token,
    });

    let router = emulator();
    let first = create(&router, first_request.clone()).await;
    assert_eq!(
        first.status, 202,
        "the first create must succeed, got {} with {:?}",
        first.status, first.body
    );

    // Each row changes one member of the request the token already stands for.
    let mut changed_name = first_request.clone();
    changed_name["Name"] = json!("token-bound-other");
    let mut other_subnet = first_request.clone();
    other_subnet["Configuration"] = vpc_egress(
        &["subnet-0fedcba9876543210"],
        &["sg-0123456789abcdef0"],
        Some("IPv4"),
        Some("MicroVm"),
    );
    let mut extra_subnet = first_request.clone();
    extra_subnet["Configuration"] = vpc_egress(
        &["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
        &["sg-0123456789abcdef0"],
        Some("IPv4"),
        Some("MicroVm"),
    );
    let mut other_security_group = first_request.clone();
    other_security_group["Configuration"] = vpc_egress(
        &["subnet-0123456789abcdef0"],
        &["sg-0fedcba9876543210"],
        Some("IPv4"),
        Some("MicroVm"),
    );
    let mut other_protocol = first_request.clone();
    other_protocol["Configuration"] = vpc_egress(
        &["subnet-0123456789abcdef0"],
        &["sg-0123456789abcdef0"],
        Some("DualStack"),
        Some("MicroVm"),
    );
    let mut other_compute = first_request.clone();
    other_compute["Configuration"] = vpc_egress(
        &["subnet-0123456789abcdef0"],
        &["sg-0123456789abcdef0"],
        Some("IPv4"),
        Some("WebFunction"),
    );
    let mut other_role = first_request.clone();
    other_role["OperatorRole"] = json!(OTHER_OPERATOR_ROLE);
    let mut other_tags = first_request.clone();
    other_tags["Tags"] = json!({ "env": "prod" });

    let cases = vec![
        ("a different name", changed_name),
        ("a different subnet", other_subnet),
        ("one more subnet", extra_subnet),
        ("a different security group", other_security_group),
        ("a different network protocol", other_protocol),
        ("a different compute resource type", other_compute),
        ("a different operator role", other_role),
        ("different tags", other_tags),
    ];

    for (case, request) in &cases {
        let refused = create(&router, request.clone()).await;
        assert_refused(case, &refused, 409, "ResourceConflictException", expected);

        // The refused attempt applied nothing: the connector the token created still
        // serves the parameters it was created with, and it is still the only one.
        let fetched = get(&router, name).await;
        assert_eq!(
            at(&fetched.body, "/Configuration"),
            at(&first.body, "/Configuration"),
            "case {case:?}: a refused create may not change the stored configuration, \
             got {:?}",
            at(&fetched.body, "/Configuration")
        );
        assert_eq!(
            text(&fetched.body, "/OperatorRole"),
            OPERATOR_ROLE,
            "case {case:?}: a refused create may not change the stored operator role, \
             got {:?}",
            at(&fetched.body, "/OperatorRole")
        );
        let listed = list(&router, "").await;
        assert_eq!(
            listed_names(&listed),
            vec![name.to_string()],
            "case {case:?}: a refused create may not add a connector, got {:?}",
            listed.body
        );
    }

    // And the retry the token exists for still answers with the connector it created.
    let replay = create(&router, first_request).await;
    assert_eq!(
        replay.status, 202,
        "an identical retry must succeed, got {} with {:?}",
        replay.status, replay.body
    );
    for member in ["/Arn", "/Id", "/Name", "/Version", "/VersionArn"] {
        assert_eq!(
            at(&replay.body, member),
            at(&first.body, member),
            "an identical retry must answer with the first connector's {member}"
        );
    }
}

/// RULE 2 again, through the other operation that binds a `ClientToken`: the rule is the
/// token's, not one handler's.
#[tokio::test]
async fn an_update_token_carrying_other_parameters_is_refused() {
    let expected = "A request with the same client token but different parameters was already submitted";
    let router = emulator();
    let name = "token-updated";
    let created = create(&router, minimal_create(name)).await;
    assert_eq!(
        created.status, 202,
        "the create must succeed, got {} with {:?}",
        created.status, created.body
    );
    // An update needs an ACTIVE connector, which the clock owes the new one.
    wait_for_the_clock();

    let request = json!({
        "Configuration": vpc_egress(&["subnet-0fedcba9876543210"], &[], None, None),
        "ClientToken": "update-token",
    });
    let updated = update(&router, name, request.clone()).await;
    assert_eq!(
        at(&updated.body, "/Version"),
        json!(2),
        "a configuration change appends a version, got {:?}",
        at(&updated.body, "/Version")
    );

    let mut other_configuration = request.clone();
    other_configuration["Configuration"] =
        vpc_egress(&["subnet-00112233445566778"], &[], None, None);
    let mut with_a_role = request.clone();
    with_a_role["OperatorRole"] = json!(OTHER_OPERATOR_ROLE);
    for (case, body) in [
        ("a different configuration", other_configuration),
        ("an operator role the first request did not carry", with_a_role),
    ] {
        let refused = update(&router, name, body).await;
        assert_refused(case, &refused, 409, "ResourceConflictException", expected);
        let fetched = get(&router, name).await;
        assert_eq!(
            at(&fetched.body, "/LatestVersion"),
            json!(2),
            "case {case:?}: a refused update may not append a version, got {:?}",
            at(&fetched.body, "/LatestVersion")
        );
    }

    let replay = update(&router, name, request).await;
    assert_eq!(
        at(&replay.body, "/Version"),
        json!(2),
        "an identical retry replays the version the token produced, got {:?}",
        at(&replay.body, "/Version")
    );
}

/// RULE 3: a name the account already holds is refused naming it, and the account keeps the
/// connector it had.
///
/// The rows span the name alphabet `NetworkConnectorName` admits (plain, dashes and
/// underscores, mixed case and digits, the longest run that leaves room for a suffix) and
/// both second attempts a client makes: the identical payload, and one that differs
/// everywhere but the name.
#[tokio::test]
async fn a_name_already_in_use_is_refused_naming_it() {
    // Values the name branch of `NetworkConnectorName`'s @pattern admits, `[a-zA-Z0-9_-]`
    // of 1..64 characters (`smithy_query(query="shape",shape="NetworkConnectorName")`);
    // the longest row leaves room for the `-b` suffix the second half of the case appends.
    let longest_run = "n".repeat(62);
    let cases = vec![
        ("a plain name", "collide".to_string(), true),
        ("dashes and underscores", "lnc_egress-01".to_string(), true),
        ("mixed case and digits", "Lnc0Egress9".to_string(), false),
        ("the longest run the name branch admits", longest_run, false),
    ];

    let router = emulator();
    for (case, name, identical_retry) in &cases {
        let first = create(&router, minimal_create(name)).await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first create must succeed, got {} with {:?}",
            first.status, first.body
        );
        let id = text(&first.body, "/Id");

        // The second attempt: either the same payload, or one whose every other member
        // differs — the collision is on the name alone.
        let second_request = if *identical_retry {
            minimal_create(name)
        } else {
            json!({
                "Name": name,
                "Configuration": vpc_egress(
                    &["subnet-0fedcba9876543210", "subnet-00112233445566778"],
                    &["sg-0123456789abcdef0"],
                    Some("DualStack"),
                    Some("WebFunction"),
                ),
                "OperatorRole": OTHER_OPERATOR_ROLE,
                "Tags": { "env": "test" },
            })
        };
        let refused = create(&router, second_request).await;
        assert_refused(
            case,
            &refused,
            409,
            "ResourceConflictException",
            &format!("A network connector with name '{name}' already exists in this account"),
        );

        // The refusal left the first connector, with its own identity, in place.
        let fetched = get(&router, name).await;
        assert_eq!(
            text(&fetched.body, "/Id"),
            id,
            "case {case:?}: the account still holds the connector it minted first, got {:?}",
            at(&fetched.body, "/Id")
        );
        assert_eq!(
            at(&fetched.body, "/Configuration"),
            at(&first.body, "/Configuration"),
            "case {case:?}: a refused create may not change the stored configuration, got {:?}",
            at(&fetched.body, "/Configuration")
        );

        // The same payload under a free name is accepted, with an identity of its own.
        let other_name = format!("{name}-b");
        let accepted = create(&router, minimal_create(&other_name)).await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: {other_name:?} is free, so the create must succeed, got {} \
             with {:?}",
            accepted.status, accepted.body
        );
        assert_ne!(
            text(&accepted.body, "/Id"),
            id,
            "case {case:?}: a second connector gets its own id, got {:?}",
            at(&accepted.body, "/Id")
        );
    }

    // Exactly one connector per name, and no more.
    let listed = list(&router, "").await;
    let mut expected_names: Vec<String> = cases
        .iter()
        .flat_map(|(_, name, _)| [name.clone(), format!("{name}-b")])
        .collect();
    expected_names.sort();
    assert_eq!(
        listed_names(&listed),
        expected_names,
        "every accepted create, and only those, is listed once, got {:?}",
        listed.body
    );
}
