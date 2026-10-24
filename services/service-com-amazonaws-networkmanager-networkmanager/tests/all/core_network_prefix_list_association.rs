// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the core-network prefix-list association slice
//! (`CreateCoreNetworkPrefixListAssociation`, `DeleteCoreNetworkPrefixListAssociation`),
//! driven over HTTP.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes and the
//! label bindings — the delete carries BOTH its members as path labels — are exercised as a
//! client hits them. Nothing reaches into `crate::state`: an association is only ever
//! observed through the operations, a stored pair being exactly the one a delete removes
//! once and then no longer finds.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::{json, Value};

use service_com_amazonaws_networkmanager_networkmanager::state::{
    FIELD_PREFIX_LIST_ARN, LIMIT_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK,
    MAX_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK, PREFIX_LIST_ASSOCIATION_KIND,
};

use crate::core_network_lifecycle::create_core_network;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::resource_tagging::escape;

/// A core network of its own global network (one global network holds a single core
/// network), answering its minted id.
async fn core_network(emulator: &Emulator, label: &str) -> String {
    let global_network = emulator.create_global_network(label).await;
    let (status, body) =
        create_core_network(emulator, json!({ "GlobalNetworkId": global_network })).await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork({label}): expected 200, got {status} with {body}"
    );
    body["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateCoreNetwork({label}): no id in {body}"))
        .to_string()
}

/// The ARN of a managed prefix list whose id carries `digits` as its hex suffix, in the
/// skeleton every EC2 ARN this model types spells out (`VpcArn` carries
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$`, `smithy_query
/// shape=VpcArn`).
fn prefix_list_arn(digits: &str) -> String {
    format!("arn:aws:ec2:us-west-2:123456789012:prefix-list/pl-{digits}")
}

/// `CreateCoreNetworkPrefixListAssociation` with the request body `body`.
async fn associate_body(emulator: &Emulator, body: Value) -> (u16, Value) {
    emulator.post("/prefix-list", body).await
}

/// `CreateCoreNetworkPrefixListAssociation` for one core network / prefix list pair.
async fn associate(
    emulator: &Emulator,
    core_network_id: &str,
    arn: &str,
    alias: &str,
) -> (u16, Value) {
    associate_body(
        emulator,
        json!({
            "CoreNetworkId": core_network_id,
            "PrefixListArn": arn,
            "PrefixListAlias": alias,
        }),
    )
    .await
}

/// The `/prefix-list/{PrefixListArn}/core-network/{CoreNetworkId}` URI of one pair. Both
/// members are NON-greedy `@httpLabel`s (`smithy_query query="operation"
/// operation="DeleteCoreNetworkPrefixListAssociation"`), so the ARN's `:` and `/` travel
/// percent-encoded, which is what botocore sends.
fn disassociate_uri(core_network_id: &str, arn: &str) -> String {
    format!(
        "/prefix-list/{}/core-network/{core_network_id}",
        escape(arn)
    )
}

/// `DeleteCoreNetworkPrefixListAssociation` for one pair.
async fn disassociate(emulator: &Emulator, core_network_id: &str, arn: &str) -> (u16, Value) {
    emulator.delete(&disassociate_uri(core_network_id, arn)).await
}

/// Assert that `core_network_id` holds an association for `arn`: the delete removes it and
/// reports the pair, and a second delete of the same pair no longer finds it.
async fn assert_deletes_once(
    emulator: &Emulator,
    core_network_id: &str,
    arn: &str,
    case: &str,
) {
    let (status, body) = disassociate(emulator, core_network_id, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteCoreNetworkPrefixListAssociation on a stored pair: expected 200, got {status} with {body}"
    );
    assert_eq!(
        (
            body["CoreNetworkId"].as_str(),
            body["PrefixListArn"].as_str()
        ),
        (Some(core_network_id), Some(arn)),
        "case {case:?}: the delete must report the pair it removed, got {body}"
    );

    let (status, body) = disassociate(emulator, core_network_id, arn).await;
    assert_eq!(
        status, 404,
        "case {case:?}: DeleteCoreNetworkPrefixListAssociation on the removed pair: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {case:?}: wrong error code for the removed pair in {body}"
    );
    assert_eq!(
        (
            body["ResourceType"].as_str(),
            body["ResourceId"].as_str()
        ),
        (
            Some(PREFIX_LIST_ASSOCIATION_KIND),
            Some(format!("{core_network_id}/{arn}").as_str())
        ),
        "case {case:?}: the refusal must name the pair as the missing resource, got {body}"
    );
}

/// One row of the association table: a prefix list ARN form the service accepts and the
/// alias the create carries with it.
struct AssociationCase {
    name: &'static str,
    arn: String,
    alias: String,
}

/// The accepted forms, spanning what the rule generalises over: the partition, the Region
/// and account segments (both of which the modelled EC2 skeleton lets be empty), the width
/// of the prefix list id at each published bound, and the alias from empty to the longest
/// the modelled `ConstrainedString` admits (len 0..256).
fn accepted_cases() -> Vec<AssociationCase> {
    vec![
        AssociationCase {
            name: "an AWS-managed list, the shortest id width",
            arn: prefix_list_arn("63a5400a"),
            alias: "aws-managed".to_string(),
        },
        AssociationCase {
            name: "a customer-managed list, the longest id width",
            arn: prefix_list_arn("0123456789abcdef1"),
            alias: "customer-managed".to_string(),
        },
        AssociationCase {
            name: "another partition and Region",
            arn: "arn:aws-us-gov:ec2:us-gov-west-1:123456789012:prefix-list/pl-00112233"
                .to_string(),
            alias: "gov-cloud".to_string(),
        },
        AssociationCase {
            name: "an empty Region segment",
            arn: "arn:aws:ec2::123456789012:prefix-list/pl-aaaaaaaa".to_string(),
            alias: "no-region".to_string(),
        },
        AssociationCase {
            name: "an empty account segment",
            arn: "arn:aws:ec2:us-east-1::prefix-list/pl-bbbbbbbb".to_string(),
            alias: "no-account".to_string(),
        },
        AssociationCase {
            name: "the longest alias the model admits",
            arn: prefix_list_arn("cccccccc"),
            alias: "a".repeat(256),
        },
        AssociationCase {
            name: "an empty alias",
            arn: prefix_list_arn("dddddddd"),
            alias: String::new(),
        },
    ]
}

#[tokio::test]
async fn an_associated_prefix_list_is_stored_once_and_deleted_once_for_every_accepted_form() {
    let emulator = Emulator::new();
    let core = core_network(&emulator, "prefix-lists").await;

    for case in accepted_cases() {
        let AssociationCase { name, arn, alias } = case;

        let (status, created) = associate(&emulator, &core, &arn, &alias).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateCoreNetworkPrefixListAssociation: expected 200, got {status} with {created}"
        );
        assert_eq!(
            (
                created["CoreNetworkId"].as_str(),
                created["PrefixListArn"].as_str(),
                created["PrefixListAlias"].as_str()
            ),
            (Some(core.as_str()), Some(arn.as_str()), Some(alias.as_str())),
            "case {name:?}: the create must report the association it stored, got {created}"
        );

        // The pair IS the association's identity, so the same request repeated has nothing
        // left to create and answers the stored association rather than a second one —
        // which the single delete below is what proves.
        let (status, repeated) = associate(&emulator, &core, &arn, &alias).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a repeated create: expected 200, got {status} with {repeated}"
        );
        assert_eq!(
            repeated, created,
            "case {name:?}: a repeated create must answer the stored association, got {repeated}"
        );

        assert_deletes_once(&emulator, &core, &arn, name).await;
    }
}

/// One row of the refusal table: a string that is not a managed prefix list ARN.
struct MalformedCase {
    name: &'static str,
    arn: String,
}

/// The refused forms: a string that is no ARN, an ARN of another service or of another EC2
/// resource type, and an id whose width is one past each published bound (8 and 17 hex
/// digits, from the EC2 skeleton the model types) or outside its alphabet.
fn malformed_cases() -> Vec<MalformedCase> {
    vec![
        MalformedCase {
            name: "a bare prefix list id",
            arn: "pl-63a5400a".to_string(),
        },
        MalformedCase {
            name: "an ARN of this service",
            arn: format!(
                "arn:aws:networkmanager::{}:prefix-list/pl-63a5400a",
                engine::emulator_account_id()
            ),
        },
        MalformedCase {
            name: "another EC2 resource type",
            arn: "arn:aws:ec2:us-west-2:123456789012:vpc/vpc-0123456789abcdef1".to_string(),
        },
        MalformedCase {
            name: "an id with no pl- prefix",
            arn: "arn:aws:ec2:us-west-2:123456789012:prefix-list/63a5400a".to_string(),
        },
        MalformedCase {
            name: "one digit short of the shortest id",
            arn: prefix_list_arn("1234567"),
        },
        MalformedCase {
            name: "one digit past the longest id",
            arn: prefix_list_arn("0123456789abcdef12"),
        },
        MalformedCase {
            name: "uppercase hex in the id",
            arn: prefix_list_arn("63A5400A"),
        },
        MalformedCase {
            name: "an empty partition",
            arn: "arn::ec2:us-west-2:123456789012:prefix-list/pl-63a5400a".to_string(),
        },
    ]
}

#[tokio::test]
async fn a_string_that_is_not_a_prefix_list_arn_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let core = core_network(&emulator, "malformed-arns").await;

    // A well-formed association made first, so the refusals below can be shown to have left
    // the store exactly as they found it.
    let stored = prefix_list_arn("0123456789abcde");
    let (status, body) = associate(&emulator, &core, &stored, "kept").await;
    assert_eq!(
        status, 200,
        "the control association: expected 200, got {status} with {body}"
    );

    for case in malformed_cases() {
        let MalformedCase { name, arn } = case;

        for (operation, (status, body)) in [
            (
                "CreateCoreNetworkPrefixListAssociation",
                associate(&emulator, &core, &arn, "refused").await,
            ),
            (
                "DeleteCoreNetworkPrefixListAssociation",
                disassociate(&emulator, &core, &arn).await,
            ),
        ] {
            assert_eq!(
                status, 400,
                "case {name:?}: {operation} on {arn:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {name:?}: {operation} on {arn:?}: wrong error code in {body}"
            );
            assert_eq!(
                body["Fields"][0]["Name"].as_str(),
                Some(FIELD_PREFIX_LIST_ARN),
                "case {name:?}: {operation} on {arn:?}: the refusal must name the ARN member, got {body}"
            );
        }
    }

    // An empty ARN is refused by the create too; no delete row for it, since an empty path
    // label addresses no route at all.
    let (status, body) = associate(&emulator, &core, "", "refused").await;
    assert_eq!(
        status, 400,
        "case \"an empty ARN\": expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"an empty ARN\": wrong error code in {body}"
    );

    assert_deletes_once(&emulator, &core, &stored, "the store after every refusal").await;
}

#[tokio::test]
async fn deleting_one_association_leaves_every_other_pair_associated() {
    let emulator = Emulator::new();
    let first = core_network(&emulator, "first").await;
    let second = core_network(&emulator, "second").await;

    // One prefix list associated with BOTH core networks, plus one more on each: a delete
    // addresses exactly one pair, so neither the other list of the same core network nor
    // the same list of another core network may go with it.
    let shared = prefix_list_arn("0000000000000aaaa");
    let only_first = prefix_list_arn("0000000000000bbbb");
    let only_second = prefix_list_arn("0000000000000cccc");
    for (core, arn) in [
        (&first, &shared),
        (&first, &only_first),
        (&second, &shared),
        (&second, &only_second),
    ] {
        let (status, body) = associate(&emulator, core, arn, "routing").await;
        assert_eq!(
            status, 200,
            "associating {arn} with {core}: expected 200, got {status} with {body}"
        );
    }

    assert_deletes_once(&emulator, &first, &shared, "the shared list of the first core network")
        .await;
    assert_deletes_once(&emulator, &first, &only_first, "the first core network's own list").await;
    assert_deletes_once(
        &emulator,
        &second,
        &shared,
        "the shared list of the second core network",
    )
    .await;

    // A core network id that names nothing stored is not found whatever prefix list the
    // request carries — a well-formed id, so the modelled pattern is satisfied.
    let unknown = "core-network-00000000000000bad";
    let (status, body) = disassociate(&emulator, unknown, &only_second).await;
    assert_eq!(
        status, 404,
        "a delete on an unknown core network: expected 404, got {status} with {body}"
    );
    assert_eq!(
        (
            error_code(&body),
            body["ResourceType"].as_str(),
            body["ResourceId"].as_str()
        ),
        (
            "ResourceNotFoundException",
            Some("core-network"),
            Some(unknown)
        ),
        "a delete on an unknown core network must name it as the missing resource, got {body}"
    );

    // An association is addressed only THROUGH its core network, so once the core network
    // is deleted the pair is unaddressable: the refusal names the core network, not the
    // association.
    let (status, body) = emulator.delete(&format!("/core-networks/{second}")).await;
    assert_eq!(
        status, 200,
        "DeleteCoreNetwork({second}): expected 200, got {status} with {body}"
    );
    let (status, body) = disassociate(&emulator, &second, &only_second).await;
    assert_eq!(
        status, 404,
        "a delete on a deleted core network: expected 404, got {status} with {body}"
    );
    assert_eq!(
        body["ResourceType"].as_str(),
        Some("core-network"),
        "a delete on a deleted core network must name the core network, got {body}"
    );
}

#[tokio::test]
async fn one_pair_and_one_client_token_hold_a_single_association() {
    let emulator = Emulator::new();
    let core = core_network(&emulator, "idempotency").await;

    // The alias belongs to the association and no operation of this service updates one,
    // so the same pair asked for under a different alias is a conflict naming the stored
    // association rather than a rewrite of its alias.
    let arn = prefix_list_arn("0000000000000dddd");
    let (status, body) = associate(&emulator, &core, &arn, "alpha").await;
    assert_eq!(
        status, 200,
        "the first create: expected 200, got {status} with {body}"
    );
    let (status, conflict) = associate(&emulator, &core, &arn, "beta").await;
    assert_eq!(
        status, 409,
        "the same pair under another alias: expected 409, got {status} with {conflict}"
    );
    assert_eq!(
        (
            error_code(&conflict),
            conflict["ResourceType"].as_str(),
            conflict["ResourceId"].as_str()
        ),
        (
            "ConflictException",
            Some(PREFIX_LIST_ASSOCIATION_KIND),
            Some(format!("{core}/{arn}").as_str())
        ),
        "the conflict must name the stored association, got {conflict}"
    );

    // Each row: what the retry changes about the request the token first carried, and the
    // status it must answer — nothing changed is the stored association, anything else a
    // conflict.
    let token = "token-for-one-association";
    let other = prefix_list_arn("0000000000000eeee");
    let (status, created) = associate_body(
        &emulator,
        json!({
            "CoreNetworkId": &core,
            "PrefixListArn": &other,
            "PrefixListAlias": "gamma",
            "ClientToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first create under a client token: expected 200, got {status} with {created}"
    );
    let unused = prefix_list_arn("0000000000000ffff");
    for (name, alias, retried_arn, expected) in [
        ("the same request", "gamma", &other, 200),
        ("another alias", "delta", &other, 409),
        ("another prefix list", "gamma", &unused, 409),
    ] {
        let (status, body) = associate_body(
            &emulator,
            json!({
                "CoreNetworkId": &core,
                "PrefixListArn": retried_arn,
                "PrefixListAlias": alias,
                "ClientToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {name:?}: the token retried: expected {expected}, got {status} with {body}"
        );
        if expected == 200 {
            assert_eq!(
                body, created,
                "case {name:?}: the token must answer the association it first stored, got {body}"
            );
        }
    }

    // Exactly one association per pair, and the refused retries stored nothing: each stored
    // pair deletes once, and the prefix list only a conflict ever named is not associated.
    assert_deletes_once(&emulator, &core, &arn, "the pair created without a token").await;
    assert_deletes_once(&emulator, &core, &other, "the pair created under a token").await;
    let (status, body) = disassociate(&emulator, &core, &unused).await;
    assert_eq!(
        status, 404,
        "a conflicted retry must store nothing: expected 404, got {status} with {body}"
    );
}

#[tokio::test]
async fn associating_past_the_per_core_network_quota_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let full = core_network(&emulator, "full").await;
    let sibling = core_network(&emulator, "sibling").await;

    // The cap comes from the service's own constant, so this test cannot drift from it.
    for index in 0..MAX_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK {
        let arn = prefix_list_arn(&format!("{:017x}", index + 1));
        let (status, body) = associate(&emulator, &full, &arn, "routing").await;
        assert_eq!(
            status, 200,
            "association {index} of the cap: expected 200, got {status} with {body}"
        );
    }

    let past = prefix_list_arn("0000000000000f1f1");
    let (status, refused) = associate(&emulator, &full, &past, "one too many").await;
    assert_eq!(
        status, 402,
        "one association past the cap: expected 402, got {status} with {refused}"
    );
    assert_eq!(
        (
            error_code(&refused),
            refused["LimitCode"].as_str(),
            refused["ResourceId"].as_str()
        ),
        (
            "ServiceQuotaExceededException",
            Some(LIMIT_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK),
            Some(full.as_str())
        ),
        "the quota refusal must name its limit and the core network, got {refused}"
    );
    let (status, body) = disassociate(&emulator, &full, &past).await;
    assert_eq!(
        status, 404,
        "a refused create must have stored nothing: expected 404, got {status} with {body}"
    );

    // The quota is per core network, so a sibling of a full one still has room.
    let (status, body) = associate(&emulator, &sibling, &past, "routing").await;
    assert_eq!(
        status, 200,
        "the sibling core network: expected 200, got {status} with {body}"
    );
    assert_deletes_once(&emulator, &sibling, &past, "the sibling core network").await;
}
