// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's NEGATIVE paths: the requests it refuses, and
//! the refusal each one carries on the wire.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour.
//!
//! Three rules, each asserted over a table that spans its class rather than over the one
//! request that exposed it:
//!
//! 1. `CreateGateway` registers a gateway only for an activation key this service
//!    ISSUED; any other key is refused with HTTP 400 `ValidationException` carrying
//!    `ErrorCode` `ActivationKeyNotFound`, and nothing is written — so the read side
//!    reports no gateway for a create that was refused.
//! 2. A constraint-violation message for a `@sensitive` member names the member and the
//!    constraint it failed and nothing of the value it held; a member that is not
//!    sensitive still reports its value.
//! 3. An operation that takes a `ResourceARN` resolves it as the resource type the ARN
//!    NAMES: a hypervisor ARN with no hypervisor behind it is refused with HTTP 400
//!    `ValidationException` / `ErrorCode` `HypervisorNotFound`, and every other
//!    resource type with the modelled 404 `ResourceNotFoundException`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::service_state;

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, create_body, issue_activation_keys,
    never_created_arn, Reply, REGION,
};

/// An ARN of this service naming `resource_type`/`id`, in the region the harness signs
/// for. Spans every resource type this service's ARNs can name, which is what the
/// tagging operations have to tell apart.
fn arn_of(resource_type: &str, id: &str) -> String {
    format!(
        "arn:aws:backup-gateway:{REGION}:{}:{resource_type}/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The refusal message, wherever the envelope carries it: a refusal the framework made
/// before the handler ran states it in `message`, a modelled error the handler returned
/// in the `Message` member.
fn message_of(reply: &Reply) -> String {
    reply
        .str_at("/message")
        .or_else(|| reply.str_at("/Message"))
        .unwrap_or_else(|| panic!("no message in {:?}", reply.body))
        .to_string()
}

/// Rule 1: an activation key is ISSUED to a gateway appliance, so `CreateGateway` can
/// only find one in this service's registry — a key the registry does not hold is
/// refused, whatever it looks like, and the refused call writes nothing.
#[tokio::test]
async fn an_activation_key_the_service_never_issued_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, the ActivationKey as the client sends it). Every row satisfies the modelled
    // `ActivationKey` constraints — len 1..50, pattern `^[0-9a-zA-Z\-]+$`
    // (`smithy_query(query="shape",shape="ActivationKey")`) — so each one reaches the
    // handler rather than the boundary validator, and none of them was ever issued. The
    // rows differ in length, case and hyphenation: the refusal keys on none of that.
    let rows: [(&str, String); 6] = [
        ("a single character", "k".to_string()),
        ("digits only", "0123456789".to_string()),
        (
            "five hyphenated groups",
            "ABCDE-FGHIJ-KLMNO-PQRST-UVWXY".to_string(),
        ),
        ("mixed case and hyphens", "aB3-dE6".to_string()),
        ("at the 50-character maximum", "K".repeat(50)),
        ("the shape another gateway service issues", "activation-key-0001".to_string()),
    ];

    for (case, key) in &rows {
        let refused = call(
            &router,
            "CreateGateway",
            &create_body("gw-unissued", key, &BTreeMap::new()),
        )
        .await;
        assert_eq!(
            (
                refused.status,
                refused.code.as_deref(),
                refused.str_at("/ErrorCode"),
            ),
            (400, Some("ValidationException"), Some("ActivationKeyNotFound")),
            "case {case:?}: an activation key this service never issued must be refused \
             with 400 ValidationException / ErrorCode ActivationKeyNotFound, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused
                .str_at("/Message")
                .is_some_and(|stated| !stated.is_empty()),
            "case {case:?}: the refusal must state a message, got {:?}",
            refused.body
        );
        assert!(
            refused.str_at("/GatewayArn").is_none(),
            "case {case:?}: a refused create must hand back no GatewayArn, got {:?}",
            refused.body
        );
    }

    // And nothing was written: the read side cannot report a gateway that a refused
    // create would have minted.
    let listed = call(&router, "ListGateways", &json!({})).await;
    assert_eq!(
        (listed.status, listed.gateway_arns()),
        (200, Vec::new()),
        "a refused CreateGateway must leave no gateway for ListGateways to report, got \
         {:?}",
        listed.body
    );
    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this service's state")
            .lock()
            .gateway_count(),
        0,
        "a refused CreateGateway must leave the store empty"
    );

    // The SAME request succeeds once the key is issued, and the gateway it mints reads
    // back: the refusal above is a fact about the registry, not about the key's spelling.
    let (case, key) = &rows[2];
    issue_activation_keys(&engine, &[key.as_str()]);
    let created = call(
        &router,
        "CreateGateway",
        &create_body("gw-issued", key, &BTreeMap::new()),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: once issued, the same activation key must create a gateway, got \
         {} {:?}",
        created.status, created.body
    );
    let arn = created.gateway_arn().to_string();
    let fetched = call(&router, "GetGateway", &json!({ "GatewayArn": arn })).await;
    assert_eq!(
        (
            fetched.status,
            fetched.str_at("/Gateway/GatewayDisplayName")
        ),
        (200, Some("gw-issued")),
        "case {case:?}: the created gateway must read back through GetGateway, got {} \
         {:?}",
        fetched.status,
        fetched.body
    );
    let listed = call(&router, "ListGateways", &json!({})).await;
    assert_eq!(
        (listed.status, listed.gateway_arns()),
        (200, vec![arn.clone()]),
        "case {case:?}: ListGateways must report exactly the gateway that was created, \
         got {:?}",
        listed.body
    );

    // An issued key stays issued: it is a registry entry, not a one-request fact.
    assert!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this service's state")
            .lock()
            .activation_key_issued(key),
        "case {case:?}: the issued key must still be in the registry after the create"
    );
}

/// Rule 2: a constraint-violation message for a `@sensitive` member must not reproduce
/// the value that was rejected.
///
/// The sensitive members of this model are the `Password` and `Username` shapes
/// (`smithy_query(query="shape",shape="Password,Username")` -> `sensitive: true`),
/// reached by `ImportHypervisorConfiguration`, `TestHypervisorConfiguration` and
/// `UpdateHypervisor`; the rows below span all three operations, both members and both
/// constraint classes (pattern and length), and the last three rows are the control: a
/// member that is NOT sensitive still reports the value it held.
#[tokio::test]
async fn a_constraint_violation_on_a_sensitive_member_never_echoes_the_value() {
    let engine = build_engine();
    let router = build_router(&engine);
    let gateway_arn = create(&router, "gw-sensitive").await;
    let hypervisor_arn = arn_of("hypervisor", "hv-1111111111111111");
    // `Host` is len 3..128, pattern `^.+$`; `Name` is len 1..100, pattern
    // `^[a-zA-Z0-9-]*$` (`smithy_query(query="shape",shape="Host,Name")`).
    let host = "198.51.100.10";
    let name = "hv-one";
    let long = "p".repeat(101);

    // (case, operation, body, member path, the value the message must or must not
    // carry, whether that member is @sensitive, the requirement the clause must state)
    let rows: [(&str, &str, Value, &str, &str, bool, &str); 8] = [
        (
            "ImportHypervisorConfiguration password outside the pattern",
            "ImportHypervisorConfiguration",
            json!({ "Name": name, "Host": host, "Password": "bad\tpassword" }),
            "password",
            "bad\tpassword",
            true,
            "Member must satisfy regular expression pattern: [ -~]+",
        ),
        (
            "ImportHypervisorConfiguration password past its length maximum",
            "ImportHypervisorConfiguration",
            json!({ "Name": name, "Host": host, "Password": long }),
            "password",
            &long,
            true,
            "Member must have length less than or equal to 100",
        ),
        (
            "ImportHypervisorConfiguration username outside the pattern",
            "ImportHypervisorConfiguration",
            json!({ "Name": name, "Host": host, "Username": "bad\tusername" }),
            "username",
            "bad\tusername",
            true,
            // The modelled `Username` pattern, unanchored
            // (`smithy_query(query="shape",shape="Username")`).
            "Member must satisfy regular expression pattern: [ -\\.0-\\[\\]-~]*[!-\\.0-\\[\\]-~][ -\\.0-\\[\\]-~]*",
        ),
        (
            "TestHypervisorConfiguration password outside the pattern",
            "TestHypervisorConfiguration",
            json!({ "GatewayArn": gateway_arn, "Host": host, "Password": "bad\tpassword" }),
            "password",
            "bad\tpassword",
            true,
            "Member must satisfy regular expression pattern: [ -~]+",
        ),
        (
            "UpdateHypervisor password past its length maximum",
            "UpdateHypervisor",
            json!({ "HypervisorArn": hypervisor_arn, "Password": long }),
            "password",
            &long,
            true,
            "Member must have length less than or equal to 100",
        ),
        (
            "UpdateHypervisor username outside the pattern",
            "UpdateHypervisor",
            json!({ "HypervisorArn": hypervisor_arn, "Username": "bad\tusername" }),
            "username",
            "bad\tusername",
            true,
            "Member must satisfy regular expression pattern: [ -\\.0-\\[\\]-~]*[!-\\.0-\\[\\]-~][ -\\.0-\\[\\]-~]*",
        ),
        (
            "ImportHypervisorConfiguration name outside the pattern",
            "ImportHypervisorConfiguration",
            json!({ "Name": "bad_name", "Host": host }),
            "name",
            "bad_name",
            false,
            "Member must satisfy regular expression pattern: [a-zA-Z0-9-]*",
        ),
        (
            "CreateGateway display name outside the pattern",
            "CreateGateway",
            json!({
                "ActivationKey": "KEY1",
                "GatewayDisplayName": "my_gateway",
                "GatewayType": "BACKUP_VM",
            }),
            "gatewayDisplayName",
            "my_gateway",
            false,
            "Member must satisfy regular expression pattern: [a-zA-Z0-9-]*",
        ),
    ];

    for (case, operation, body, member, value, sensitive, requirement) in &rows {
        let refused = call(&router, operation, body).await;
        assert_eq!(
            (refused.status, refused.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a value outside a modelled constraint must be refused with \
             400 ValidationException, got {} {:?}",
            refused.status,
            refused.body
        );
        let stated = message_of(&refused);
        assert!(
            stated.starts_with("1 validation error detected: "),
            "case {case:?}: one violated constraint must state one validation error, got \
             {stated:?}"
        );
        assert!(
            stated.contains(&format!("at '{member}' failed to satisfy constraint: ")),
            "case {case:?}: the clause must name the member it refused, got {stated:?}"
        );
        assert!(
            stated.ends_with(requirement),
            "case {case:?}: the clause must state the constraint that failed \
             ({requirement:?}), got {stated:?}"
        );
        if *sensitive {
            assert!(
                !stated.contains(value),
                "case {case:?}: the message must not reproduce a @sensitive member's \
                 value, got {stated:?}"
            );
            assert!(
                stated.contains(&format!("Value at '{member}'")),
                "case {case:?}: a @sensitive member is reported by name alone, got \
                 {stated:?}"
            );
        } else {
            assert!(
                stated.contains(&format!("Value '{value}' at '{member}'")),
                "case {case:?}: a member that is not @sensitive still reports the value \
                 it held, got {stated:?}"
            );
        }
    }
}

/// Rule 3: an operation that takes a `ResourceARN` answers for the resource type the ARN
/// names — a missing hypervisor is the modelled `ResourceNotFoundException` answered at
/// HTTP 400 and carrying `ErrorCode` `HypervisorNotFound`, every other type the same
/// shape at the 404 its `@httpError` declares and with no `ErrorCode`.
#[tokio::test]
async fn a_missing_resource_is_refused_as_the_type_its_arn_names() {
    let engine = build_engine();
    let router = build_router(&engine);
    // A real gateway exists, so a refusal below means "this ARN", not "the store is
    // empty".
    let live = create(&router, "gw-live").await;

    // (case, the ARN, expected status, expected error shape, expected ErrorCode member)
    let rows: [(&str, String, u16, &str, Option<&str>); 5] = [
        (
            "a hypervisor that was never imported",
            arn_of("hypervisor", "hv-1111111111111111"),
            400,
            "ResourceNotFoundException",
            Some("HypervisorNotFound"),
        ),
        (
            "another hypervisor id",
            arn_of("hypervisor", "hv-2222222222nonexistent"),
            400,
            "ResourceNotFoundException",
            Some("HypervisorNotFound"),
        ),
        (
            "a gateway that was never created",
            never_created_arn(),
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "a virtual machine",
            arn_of("vm", "vm-1111111111111111"),
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "a resource type this service does not name",
            arn_of("appliance", "ap-1111111111111111"),
            404,
            "ResourceNotFoundException",
            None,
        ),
    ];

    for (case, arn, status, shape, error_code) in &rows {
        // Both operations that resolve a `ResourceARN` of any type answer alike.
        let requests: [(&str, Value); 2] = [
            (
                "TagResource",
                json!({ "ResourceARN": arn, "Tags": [{ "Key": "env", "Value": "test" }] }),
            ),
            ("ListTagsForResource", json!({ "ResourceArn": arn })),
        ];
        for (operation, body) in &requests {
            let refused = call(&router, operation, body).await;
            assert_eq!(
                (
                    refused.status,
                    refused.code.as_deref(),
                    refused.str_at("/ErrorCode"),
                ),
                (*status, Some(*shape), *error_code),
                "case {case:?}: {operation} on an ARN with no record must answer \
                 {status} {shape} (ErrorCode {error_code:?}), got {} {:?}",
                refused.status,
                refused.body
            );
            assert!(
                !message_of(&refused).is_empty(),
                "case {case:?}: {operation} must state why it refused, got {:?}",
                refused.body
            );
        }
    }

    // A hypervisor that is missing is stated as such, and not as the generic
    // "resource not found" sentence.
    let refused = call(
        &router,
        "TagResource",
        &json!({
            "ResourceARN": arn_of("hypervisor", "hv-1111111111111111"),
            "Tags": [{ "Key": "env", "Value": "test" }],
        }),
    )
    .await;
    assert_eq!(
        message_of(&refused),
        "The hypervisor is not found.",
        "a missing hypervisor must be stated as a missing hypervisor, got {:?}",
        refused.body
    );

    // And the resource that DOES exist still tags and reads back, so the refusals above
    // are about the ARN and not about the operation.
    let tagged = call(
        &router,
        "TagResource",
        &json!({ "ResourceARN": live, "Tags": [{ "Key": "env", "Value": "test" }] }),
    )
    .await;
    assert_eq!(
        tagged.status, 200,
        "tagging the gateway that exists must answer 200, got {} {:?}",
        tagged.status, tagged.body
    );
    let listed = call(&router, "ListTagsForResource", &json!({ "ResourceArn": live })).await;
    assert_eq!(
        (listed.status, listed.tags()),
        (
            200,
            BTreeMap::from([("env".to_string(), "test".to_string())])
        ),
        "the tag written through TagResource must read back, got {} {:?}",
        listed.status,
        listed.body
    );
}
