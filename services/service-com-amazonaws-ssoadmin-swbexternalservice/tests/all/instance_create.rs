// SPDX-License-Identifier: CC0-1.0

//! `CreateInstance` end to end over the `awsJson1_1` wire.
//!
//! The rules under test, both from the operation's own documentation ("You can create
//! only one instance per account and across all Amazon Web Services Regions", and the
//! request "is rejected if … An instance already exists in the same account"):
//!
//! 1. an account holds AT MOST ONE instance, so a create made while it already holds one
//!    is refused with the modelled `ServiceQuotaExceededException` (HTTP 402) and the
//!    instance it holds is left untouched — whatever the request carries;
//! 2. a create on an account that holds none stores the instance and every later read
//!    answers from that record: it starts `CREATE_IN_PROGRESS` and is `ACTIVE` once the
//!    clock makes its activation due, it is an ACCOUNT instance (so it serves no
//!    permission sets), and it echoes the `Name` and `Tags` the request set;
//! 3. `ClientToken` is `@idempotencyToken`: the same token with the same parameters
//!    answers with the instance that token created, and with different parameters is the
//!    modelled `ConflictException` (409).
//!
//! The rows differ along what the rules generalise over: which optional members the
//! request carries, `Name` at its published upper bound (`length 0..255`), tag order, and
//! whether a token was seen before. Every call goes through
//! `MultiServiceRouter::dispatch`; the harness is the one `ssoadmin_permission_sets.rs`
//! owns.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    self, ComAmazonawsSsoadminSwbexternalserviceState as Store,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, error_code, is_instance_arn, router, router_over};

/// Read or write this service's store the way a handler does.
fn with_store<R>(engine: &Arc<engine::InMemoryEngine>, change: impl FnOnce(&mut Store) -> R) -> R {
    let service_state = state::service_state(&**engine).expect("the SSO Admin state store");
    let mut store = service_state.lock();
    change(&mut store)
}

/// A router over an account that holds NO instance — the account state `CreateInstance`
/// is documented for.
///
/// Nothing in this API empties an account (`DeleteInstance` is still a scaffold), so the
/// test clears the store directly; everything it then asserts goes over the wire.
fn empty_account_router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    with_store(&engine, |store| store.instances.clear());
    (router_over(Arc::clone(&engine)), engine)
}

/// Move the stored activation instant of every instance to `due_at`, so the status the
/// clock owes can be observed on both sides of it without the test sleeping. The
/// transition itself is still applied by the service's own `materialize_due`, exactly as
/// in production.
fn hold_activation(engine: &Arc<engine::InMemoryEngine>, due_at: SystemTime) {
    with_store(engine, |store| {
        for instance in store.instances.values_mut() {
            instance.active_due = Some(due_at);
        }
    });
}

/// `DescribeInstance` for `instance_arn`, as a 200 body.
async fn describe(router: &MultiServiceRouter, instance_arn: &str) -> Value {
    let (status, body) =
        call(router, "DescribeInstance", &json!({"InstanceArn": instance_arn})).await;
    assert_eq!(
        status, 200,
        "DescribeInstance expected 200 for {instance_arn}, got {status} {body}"
    );
    body
}

/// The `InstanceArn`s `ListInstances` reports.
async fn listed_arns(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {body}");
    body["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("no Instances in {body}"))
        .iter()
        .map(|instance| {
            instance["InstanceArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE 1 — with an instance already held, every shape of `CreateInstance` request is
/// refused 402, and the held instance does not move.
#[tokio::test]
async fn a_second_instance_is_refused_and_the_held_one_is_untouched() {
    let router = router();
    let (status, before) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {before}");
    let held = listed_arns(&router).await;
    assert_eq!(held.len(), 1, "expected the account's one instance, got {held:?}");

    // The rows differ along which optional members the request carries, `Name` at its
    // modelled upper bound (`length 0..255`), and a `ClientToken` no earlier call used.
    let rows: Vec<(&str, Value)> = vec![
        ("no members at all", json!({})),
        ("a Name", json!({"Name": "dup-attempt"})),
        (
            "a Name at its 255-character upper bound",
            json!({"Name": "n".repeat(255)}),
        ),
        (
            "Tags only",
            json!({"Tags": [{"Key": "Owner", "Value": "platform"}]}),
        ),
        (
            "an unseen ClientToken",
            json!({"ClientToken": "0f2b1c9a-token", "Name": "dup-attempt"}),
        ),
        (
            "every member",
            json!({
                "Name": "dup-attempt",
                "ClientToken": "another-token",
                "Tags": [{"Key": "Owner", "Value": "platform"}, {"Key": "Tier", "Value": "test"}],
            }),
        ),
    ];
    for (label, body) in rows {
        let (status, answer) = call(&router, "CreateInstance", &body).await;
        assert_eq!(
            status, 402,
            "case {label:?}: expected the modelled quota refusal, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ServiceQuotaExceededException",
            "case {label:?}: expected ServiceQuotaExceededException, got {answer}"
        );
    }

    let (status, after) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {after}");
    assert_eq!(
        before, after,
        "a refused create must leave the held instance exactly as it was, got {before} \
         then {after}"
    );
    let described = describe(&router, &held[0]).await;
    assert_eq!(
        described["Status"].as_str(),
        Some("ACTIVE"),
        "the held instance stays ACTIVE, got {:?}",
        described["Status"]
    );
}

/// RULE 2 — on an account that holds none, the create stores the instance and every
/// later read answers from that record.
#[tokio::test]
async fn the_created_instance_is_stored_and_read_back() {
    // (label, Name, Tags)
    let rows: Vec<(&str, Option<String>, Option<Value>)> = vec![
        ("no optional members", None, None),
        ("a Name", Some("MyInstance".to_string()), None),
        (
            "a Name at its 255-character upper bound and two tags",
            Some("n".repeat(255)),
            Some(json!([{"Key": "Owner", "Value": "platform"}, {"Key": "Tier", "Value": "test"}])),
        ),
        (
            "the full NameType charset",
            Some("name+=,.@-_1".to_string()),
            Some(json!([{"Key": "Owner", "Value": "platform"}])),
        ),
    ];

    for (label, name, tags) in rows {
        let (router, engine) = empty_account_router();
        assert!(
            listed_arns(&router).await.is_empty(),
            "case {label:?}: the account must start with no instance"
        );

        let mut request = json!({});
        if let Some(name) = &name {
            request["Name"] = json!(name);
        }
        if let Some(tags) = &tags {
            request["Tags"] = tags.clone();
        }
        let opened_at = SystemTime::now();
        let (status, created) = call(&router, "CreateInstance", &request).await;
        let closed_at = SystemTime::now();
        assert_eq!(
            status, 200,
            "case {label:?}: CreateInstance expected 200, got {status} {created}"
        );
        let instance_arn = created["InstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no InstanceArn in {created}"))
            .to_string();
        assert!(
            is_instance_arn(&instance_arn),
            "case {label:?}: expected a modelled InstanceArn, got {instance_arn:?}"
        );

        // It is the account's one instance, and it is the one the readers answer from.
        assert_eq!(
            listed_arns(&router).await,
            vec![instance_arn.clone()],
            "case {label:?}: ListInstances must report exactly the created instance"
        );

        // The create owes the instance its activation: a due INSTANT one span out, not a
        // status that was already terminal before the caller could observe it.
        let due_at = with_store(&engine, |store| {
            store
                .instance(&instance_arn)
                .unwrap_or_else(|| panic!("case {label:?}: the instance was not stored"))
                .active_due
        })
        .unwrap_or_else(|| panic!("case {label:?}: the create owed no activation instant"));
        assert!(
            state::INSTANCE_ACTIVATION_SPAN > Duration::ZERO,
            "case {label:?}: the activation span must never be zero"
        );
        assert!(
            due_at >= opened_at + state::INSTANCE_ACTIVATION_SPAN
                && due_at <= closed_at + state::INSTANCE_ACTIVATION_SPAN,
            "case {label:?}: the activation must fall due one span after the call, got \
             {due_at:?}"
        );

        // Before that instant the instance is in the transient status the model's
        // `InstanceStatus` enum declares, in the API and in every Region it holds.
        hold_activation(&engine, closed_at + Duration::from_secs(3_600));
        let pending = describe(&router, &instance_arn).await;
        assert_eq!(
            pending["Status"].as_str(),
            Some("CREATE_IN_PROGRESS"),
            "case {label:?}: a just-created instance is CREATE_IN_PROGRESS, got {:?}",
            pending["Status"]
        );
        assert_eq!(
            pending["Regions"][0]["Status"].as_str(),
            Some("CREATE_IN_PROGRESS"),
            "case {label:?}: its Region carries the same status, got {:?}",
            pending["Regions"]
        );
        // What the request set is echoed from the STORED record, and an absent `Name`
        // stays absent rather than being invented.
        assert_eq!(
            pending["Name"].as_str(),
            name.as_deref(),
            "case {label:?}: expected Name {name:?}, got {:?}",
            pending["Name"]
        );
        assert_eq!(
            pending["PermissionSetsEnabled"],
            Value::Bool(false),
            "case {label:?}: CreateInstance creates an ACCOUNT instance, which serves no \
             permission sets, got {:?}",
            pending["PermissionSetsEnabled"]
        );
        assert_eq!(
            pending["OwnerAccountId"].as_str(),
            Some(engine::EMULATOR_ACCOUNT_ID),
            "case {label:?}: expected the emulator's account id, got {:?}",
            pending["OwnerAccountId"]
        );
        assert!(
            pending["IdentityStoreId"]
                .as_str()
                .is_some_and(|id| id.starts_with("d-")),
            "case {label:?}: expected a minted IdentityStoreId, got {:?}",
            pending["IdentityStoreId"]
        );
        assert!(
            pending["CreatedDate"].is_number(),
            "case {label:?}: expected a server-set CreatedDate, got {:?}",
            pending["CreatedDate"]
        );
        // `Tags` are stored with the instance; `ListTagsForResource` is still a scaffold,
        // so the store is where this round trip can be read.
        let stored_tags: Vec<(String, String)> = with_store(&engine, |store| {
            store
                .instance(&instance_arn)
                .map(|instance| {
                    instance
                        .tags
                        .iter()
                        .map(|tag| (tag.key.clone(), tag.value.clone()))
                        .collect()
                })
                .unwrap_or_default()
        });
        let expected_tags: Vec<(String, String)> = tags
            .as_ref()
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .map(|tag| {
                        (
                            tag["Key"].as_str().unwrap_or_default().to_string(),
                            tag["Value"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            stored_tags, expected_tags,
            "case {label:?}: expected the request's tags to be stored, got {stored_tags:?}"
        );

        // Once the clock makes the activation due, the instance is ACTIVE — and stays
        // ACTIVE, with no further transition owed.
        hold_activation(&engine, closed_at - Duration::from_secs(1));
        for read in 0..2 {
            let active = describe(&router, &instance_arn).await;
            assert_eq!(
                active["Status"].as_str(),
                Some("ACTIVE"),
                "case {label:?}, read {read}: the activation was due, got {:?}",
                active["Status"]
            );
            assert_eq!(
                active["Regions"][0]["Status"].as_str(),
                Some("ACTIVE"),
                "case {label:?}, read {read}: its Region is ACTIVE too, got {:?}",
                active["Regions"]
            );
            assert_eq!(
                active["StatusReason"],
                Value::Null,
                "case {label:?}: an ACTIVE instance reports no StatusReason, got {:?}",
                active["StatusReason"]
            );
        }

        // And the account now holds one, so the next create is refused.
        let (status, second) = call(&router, "CreateInstance", &request).await;
        assert_eq!(
            status, 402,
            "case {label:?}: a second create expected 402, got {status} {second}"
        );
        assert_eq!(
            listed_arns(&router).await,
            vec![instance_arn],
            "case {label:?}: the refused create must not have added an instance"
        );
    }
}

/// RULE 3 — the `@idempotencyToken` `ClientToken`: the same token with the same
/// parameters answers with the instance it created, with different parameters conflicts,
/// and a token the account never saw is just another duplicate create.
#[tokio::test]
async fn the_client_token_makes_a_retry_idempotent() {
    let (router, _engine) = empty_account_router();
    let request = json!({
        "ClientToken": "a1b2c3d4-e5f6-4071-8899-aabbccddeeff",
        "Name": "Idempotent",
        "Tags": [{"Key": "Owner", "Value": "platform"}, {"Key": "Tier", "Value": "test"}],
    });

    let (status, created) = call(&router, "CreateInstance", &request).await;
    assert_eq!(status, 200, "CreateInstance expected 200, got {status} {created}");
    let instance_arn = created["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no InstanceArn in {created}"))
        .to_string();

    // The rows differ along what a retry can look like: the request verbatim, the same
    // request with its tags in another order (a tag set is unordered), the same token
    // with a different `Name`, the same token with different tags, and a token the
    // account never saw.
    let reordered_tags = json!({
        "ClientToken": "a1b2c3d4-e5f6-4071-8899-aabbccddeeff",
        "Name": "Idempotent",
        "Tags": [{"Key": "Tier", "Value": "test"}, {"Key": "Owner", "Value": "platform"}],
    });
    let other_name = json!({
        "ClientToken": "a1b2c3d4-e5f6-4071-8899-aabbccddeeff",
        "Name": "Different",
        "Tags": [{"Key": "Owner", "Value": "platform"}, {"Key": "Tier", "Value": "test"}],
    });
    let other_tags = json!({
        "ClientToken": "a1b2c3d4-e5f6-4071-8899-aabbccddeeff",
        "Name": "Idempotent",
        "Tags": [{"Key": "Owner", "Value": "platform"}],
    });
    let other_token = json!({
        "ClientToken": "ffffffff-eeee-4ddd-8ccc-bbbbaaaa9999",
        "Name": "Idempotent",
    });

    // (label, request, expected status)
    let rows: Vec<(&str, Value, u16)> = vec![
        ("the request verbatim", request.clone(), 200),
        ("the same tags in another order", reordered_tags, 200),
        ("the same token, another Name", other_name, 409),
        ("the same token, fewer tags", other_tags, 409),
        ("a token the account never saw", other_token, 402),
    ];
    for (label, body, expected) in rows {
        let (status, answer) = call(&router, "CreateInstance", &body).await;
        assert_eq!(
            status, expected,
            "case {label:?}: expected HTTP {expected}, got {status} {answer}"
        );
        match expected {
            200 => assert_eq!(
                answer["InstanceArn"].as_str(),
                Some(instance_arn.as_str()),
                "case {label:?}: a retry must answer with the instance the token created, \
                 got {answer}"
            ),
            409 => assert_eq!(
                error_code(&answer),
                "ConflictException",
                "case {label:?}: expected ConflictException, got {answer}"
            ),
            _ => assert_eq!(
                error_code(&answer),
                "ServiceQuotaExceededException",
                "case {label:?}: expected ServiceQuotaExceededException, got {answer}"
            ),
        }
        assert_eq!(
            listed_arns(&router).await,
            vec![instance_arn.clone()],
            "case {label:?}: the account must still hold exactly the one instance"
        );
    }
}
