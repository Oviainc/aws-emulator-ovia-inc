// SPDX-License-Identifier: CC0-1.0

//! `DescribeAccountLimits`, end to end over the awsQuery wire.
//!
//! The rule under test: `DescribeAccountLimits` reports one `AccountLimit` per limit
//! CloudFormation meters per account — the `LimitName` values the model documents — each
//! carrying the ceiling this emulator really holds the account to, one bounded page at a
//! time, resuming exactly where a cursor it issued left off and refusing one it did not.
//! A limit is a ceiling and not a usage count, so what it reports does not move as the
//! account fills up, and reading it changes nothing a later caller sees.
//!
//! The table spans the class the rule generalises over: every limit the model names, the
//! page a cursor at each offset into the collection resumes, the cursor forms that are
//! refused, and — for each of the three reported ceilings — the write that sits exactly ON
//! the reported limit beside the one just past it.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client calling `DescribeAccountLimits` sees.

use std::sync::Arc;
use std::time::SystemTime;

use crate::stack_lifecycle::{
    call, create_stack, member, router, router_with_engine, store, xml_blocks, xml_text,
    ACCOUNT_ID, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::{
    MAX_OUTPUTS_PER_TEMPLATE, MAX_RESOURCES_PER_TEMPLATE,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    account_limit_name, encode_next_token, PageOf, StoredStack, MAX_CONCURRENT_RESOURCES,
    MAX_STACKS,
};
use smithy_server_runtime::MultiServiceRouter;

/// One `AccountLimit` as the response carries it.
///
/// Read member by member, so each limit's name stays paired with its own value.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireLimit {
    name: Option<String>,
    value: Option<String>,
}

/// Every `AccountLimit` in a `DescribeAccountLimits` response, in wire order.
fn wire_limits(xml: &str) -> Vec<WireLimit> {
    let Some(block) = xml_blocks(xml, "AccountLimits").into_iter().next() else {
        return Vec::new();
    };
    xml_blocks(&block, "member")
        .iter()
        .map(|entry| WireLimit {
            name: xml_text(entry, "Name"),
            value: xml_text(entry, "Value"),
        })
        .collect()
}

async fn describe_account_limits(
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> (u16, String) {
    call(router, "DescribeAccountLimits", members).await
}

/// Every limit this account is held to, the ceiling the crate enforces for it, and the
/// order CloudFormation reports them in.
///
/// The names are the three the model documents on `AccountLimit$Name` ("Values:
/// `ConcurrentResourcesLimit` | `StackLimit` | `StackOutputsLimit`"), and each value is the
/// constant the code that meters that limit reads — so a row here fails the moment a
/// reported limit stops being the enforced one. The ORDER is the service's own: the per-stack
/// quotas first and the per-account resource ceiling last, which is what a client reading a
/// limit positionally reads.
fn expected_limits() -> Vec<(&'static str, usize)> {
    vec![
        (account_limit_name::STACK, MAX_STACKS),
        (account_limit_name::STACK_OUTPUTS, MAX_OUTPUTS_PER_TEMPLATE),
        (
            account_limit_name::CONCURRENT_RESOURCES,
            MAX_CONCURRENT_RESOURCES,
        ),
    ]
}

/// The value a response reports for one limit, as the count it is.
fn reported_value(xml: &str, name: &str) -> usize {
    let value = wire_limits(xml)
        .into_iter()
        .find(|limit| limit.name.as_deref() == Some(name))
        .and_then(|limit| limit.value)
        .unwrap_or_else(|| panic!("case {name:?}: expected the limit to be reported, got {xml}"));
    value
        .parse()
        .unwrap_or_else(|_| panic!("case {name:?}: expected a count, got {value:?}"))
}

/// A template declaring `count` topics, for a case that needs a known number of resources.
fn topics_template(count: usize) -> String {
    let resources: serde_json::Map<String, serde_json::Value> = (0..count)
        .map(|index| {
            (
                format!("Topic{index:04}"),
                serde_json::json!({"Type": "AWS::SNS::Topic"}),
            )
        })
        .collect();
    serde_json::json!({"Resources": resources}).to_string()
}

/// Seed the store with stacks holding `total` resources between them.
///
/// Written straight into the store, as `stack_lifecycle.rs` seeds a bound's worth of
/// stacks: filling an account-wide resource ceiling through the wire would mean really
/// creating thousands of resources in the services that own them. Each seeded stack
/// deploys its own template, so the records counted are the ones a deployed stack holds.
fn seed_resources(engine: &Arc<engine::InMemoryEngine>, total: usize) {
    let now = SystemTime::now();
    let state = store(engine);
    let mut held = state.lock();
    let mut seeded = 0;
    let mut index = 0;
    while seeded < total {
        let count = (total - seeded).min(MAX_RESOURCES_PER_TEMPLATE);
        let name = format!("seeded-{index:03}");
        let mut stack = StoredStack::new(
            format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/{name}/{index:03}"),
            name,
            Some(topics_template(count)),
            now,
        );
        // The seeded template declares no transform, so its processed stage is the body
        // itself and the deployment cannot be refused for want of one; the result is
        // asserted through the record count below either way.
        let deployed = stack.deploy_template(&std::collections::BTreeMap::new());
        assert!(
            deployed.is_ok(),
            "case \"seeding {count} resources\": the seeded template must deploy, got \
             {deployed:?}"
        );
        assert_eq!(
            stack.current_resource_count(),
            count,
            "case \"seeding {count} resources\": expected the stack to hold them, got {}",
            stack.current_resource_count()
        );
        held.stacks.push(stack);
        seeded += count;
        index += 1;
    }
}

#[tokio::test]
async fn describe_account_limits_reports_every_limit_this_account_is_held_to() {
    let (router, engine) = router_with_engine();

    let (status_code, body) = describe_account_limits(&router, &[]).await;
    assert_eq!(
        status_code, 200,
        "case \"an empty request\": expected 200, got {status_code} {body}"
    );
    let reported = wire_limits(&body);
    let expected = expected_limits();
    assert_eq!(
        reported.len(),
        expected.len(),
        "case \"an empty request\": expected {} limits, got {reported:?}",
        expected.len()
    );
    for (index, (name, value)) in expected.iter().enumerate() {
        assert_eq!(
            reported[index].name.as_deref(),
            Some(*name),
            "case {name:?}: expected it reported in the service's own order, got {reported:?}"
        );
        assert_eq!(
            reported[index].value,
            Some(value.to_string()),
            "case {name:?}: expected the enforced ceiling {value}, got {:?}",
            reported[index].value
        );
    }
    // Three limits fit one page, so there is no further page to point at.
    assert_eq!(
        xml_text(&body, "NextToken"),
        None,
        "case \"an empty request\": expected no cursor, got {body}"
    );

    // A limit is a ceiling, not a usage count: it does not move as the account fills up,
    // and the read leaves the store exactly as it found it.
    let (status_code, body) = create_stack(&router, "limits-unmoved", "{}", &[]).await;
    assert_eq!(
        status_code, 200,
        "case \"the account holds a stack\": expected the create to succeed, got {status_code} {body}"
    );
    let stacks_before = store(&engine).lock().stacks.len();
    let (_, body) = describe_account_limits(&router, &[]).await;
    assert_eq!(
        wire_limits(&body),
        reported,
        "case \"the account holds a stack\": expected the same limits, got {body}"
    );
    assert_eq!(
        store(&engine).lock().stacks.len(),
        stacks_before,
        "case \"the account holds a stack\": expected the read to store nothing"
    );
}

#[tokio::test]
async fn describe_account_limits_resumes_from_its_own_cursor_and_refuses_any_other() {
    let router = router();
    let (_, body) = describe_account_limits(&router, &[]).await;
    let all = wire_limits(&body);

    // Every offset into the collection, the empty page just past its last entry included.
    for offset in 0..=all.len() {
        let token = encode_next_token(PageOf::AccountLimits, offset);
        let (status_code, body) =
            describe_account_limits(&router, &[member("NextToken", &token)]).await;
        assert_eq!(
            status_code, 200,
            "case \"a cursor at offset {offset}\": expected 200, got {status_code} {body}"
        );
        assert_eq!(
            wire_limits(&body),
            all[offset..].to_vec(),
            "case \"a cursor at offset {offset}\": expected the rest of the limits, got {body}"
        );
    }

    for (case, token) in [
        (
            "a cursor past the end",
            encode_next_token(PageOf::AccountLimits, all.len() + 1),
        ),
        (
            "a cursor issued for another collection",
            encode_next_token(PageOf::Stacks, 0),
        ),
        ("not a cursor at all", "not-a-cursor".to_string()),
    ] {
        let (status_code, body) =
            describe_account_limits(&router, &[member("NextToken", &token)]).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the request refused, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
        // The refusal is built from the request: it names the token it rejected.
        assert_eq!(
            xml_text(&body, "Message"),
            Some(format!("{token} is not a valid NextToken")),
            "case {case:?}: expected the refusal to name the rejected token, got {body}"
        );
    }
}

#[tokio::test]
async fn the_reported_stack_limit_is_the_one_a_create_is_held_to() {
    let (router, engine) = router_with_engine();
    let (_, body) = describe_account_limits(&router, &[]).await;
    let limit = reported_value(&body, account_limit_name::STACK);

    // The account exactly AT the limit it was told about.
    {
        let now = SystemTime::now();
        let state = store(&engine);
        let mut held = state.lock();
        for index in 0..limit {
            let name = format!("at-the-limit-{index:05}");
            held.stacks.push(StoredStack::new(
                format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/{name}/{index:05}"),
                name,
                Some("{}".to_string()),
                now,
            ));
        }
    }

    let (status_code, body) = create_stack(&router, "one-stack-too-many", "{}", &[]).await;
    assert_eq!(
        status_code, 400,
        "case \"one stack past the reported StackLimit\": expected the create refused, got \
         {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("LimitExceededException"),
        "case \"one stack past the reported StackLimit\": expected LimitExceededException, got \
         {body}"
    );
    assert_eq!(
        store(&engine).lock().stacks.len(),
        limit,
        "case \"one stack past the reported StackLimit\": expected the refused create to store \
         nothing"
    );
}

#[tokio::test]
async fn the_reported_stack_outputs_limit_is_the_one_a_template_is_held_to() {
    let router = router();
    let (_, body) = describe_account_limits(&router, &[]).await;
    let limit = reported_value(&body, account_limit_name::STACK_OUTPUTS);

    // The reported ceiling, and one output past it.
    for (case, count, accepted) in [
        ("outputs at the reported StackOutputsLimit", limit, true),
        ("one output past the reported StackOutputsLimit", limit + 1, false),
    ] {
        let outputs: serde_json::Map<String, serde_json::Value> = (0..count)
            .map(|index| {
                (
                    format!("Out{index:04}"),
                    serde_json::json!({"Value": {"Ref": "AWS::StackName"}}),
                )
            })
            .collect();
        let template = serde_json::json!({"Outputs": outputs}).to_string();
        let (status_code, body) =
            create_stack(&router, &format!("outputs-{count}"), &template, &[]).await;
        if accepted {
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the create to succeed, got {status_code} {body}"
            );
            continue;
        }
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the create refused, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }
}

#[tokio::test]
async fn the_reported_concurrent_resources_limit_is_the_one_a_create_is_held_to() {
    let (router, engine) = router_with_engine();
    let (_, body) = describe_account_limits(&router, &[]).await;
    let limit = reported_value(&body, account_limit_name::CONCURRENT_RESOURCES);

    // One resource short of the ceiling the account was told about, so the create that
    // brings the last one in sits exactly ON it.
    seed_resources(&engine, limit - 1);
    let (status_code, body) =
        create_stack(&router, "the-last-resource", &topics_template(1), &[]).await;
    assert_eq!(
        status_code, 200,
        "case \"the resource at the reported ConcurrentResourcesLimit\": expected the create to \
         succeed, got {status_code} {body}"
    );
    assert_eq!(
        store(&engine).lock().current_resource_count(),
        limit,
        "case \"the resource at the reported ConcurrentResourcesLimit\": expected the account at \
         its ceiling"
    );

    let stacks_before = store(&engine).lock().stacks.len();
    let (status_code, body) =
        create_stack(&router, "one-resource-too-many", &topics_template(1), &[]).await;
    assert_eq!(
        status_code, 400,
        "case \"one resource past the reported ConcurrentResourcesLimit\": expected the create \
         refused, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("LimitExceededException"),
        "case \"one resource past the reported ConcurrentResourcesLimit\": expected \
         LimitExceededException, got {body}"
    );
    assert_eq!(
        store(&engine).lock().stacks.len(),
        stacks_before,
        "case \"one resource past the reported ConcurrentResourcesLimit\": expected the refused \
         create to store nothing"
    );
}
