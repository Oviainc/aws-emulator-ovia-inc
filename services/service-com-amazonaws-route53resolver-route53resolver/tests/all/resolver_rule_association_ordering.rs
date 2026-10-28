// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that pin the ORDER `ListResolverRuleAssociations` reports,
//! and what concurrent associates of one rule/VPC pair leave behind.
//!
//! The rule: the list reports every association the account holds in one stable
//! total order — the order the store minted the records in — so the sequence a
//! caller walks does not depend on which calls raced to create the records, on
//! how the page is cut, or on how many times it is asked for; and because one
//! rule is associated with one VPC at most once, concurrent associates of a
//! single pair leave exactly one listed record, the rest being refused as the
//! resource that already exists.
//!
//! [`super::resolver_rule_association_listing`] pins the projection, the filter
//! catalog and the cursor scope; this suite pins the ordering and the race,
//! which the filter table cannot observe because it seeds its records one after
//! another.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`], and the fixtures are the ones in
//! [`super::resolver_rule_deletion`].

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, ok, string_member};
use super::resolver_rule_deletion::{forward_rule, outbound_endpoint};

/// The `Id`s one `ListResolverRuleAssociations` response carries, in the order
/// the response carries them.
fn listed_ids(body: &Value) -> Vec<String> {
    body.get("ResolverRuleAssociations")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| string_member(item, "Id")).collect())
        .unwrap_or_default()
}

/// Every association the store holds, read with no filter and no page bound.
async fn list_all(router: &MultiServiceRouter) -> Vec<String> {
    listed_ids(&ok(router, "ListResolverRuleAssociations", json!({})).await)
}

/// Walk the whole collection `page_size` records at a time, following the
/// cursor, and return the ids in the order the pages carried them.
async fn walk_pages(router: &MultiServiceRouter, case: &str, page_size: i64) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "MaxResults": page_size });
        if let Some(token) = token.as_ref() {
            request["NextToken"] = json!(token);
        }
        let page = ok(router, "ListResolverRuleAssociations", request).await;
        let ids = listed_ids(&page);
        assert!(
            ids.len() <= usize::try_from(page_size).unwrap_or_default(),
            "case {case:?}: a page holds at most MaxResults records, got {page}"
        );
        walked.extend(ids);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return walked;
        }
    }
}

/// A VPC id of the modelled form, numbered so each row of a table uses VPCs of
/// its own.
fn vpc(index: usize) -> String {
    format!("vpc-{:017}", 70_000 + index)
}

/// Associate `rule` with `vpc` and answer `(http status, parsed body)`.
async fn try_associate(
    router: &MultiServiceRouter,
    rule: &str,
    vpc: &str,
    name: Option<String>,
) -> (u16, Value) {
    let mut request = json!({ "ResolverRuleId": rule, "VPCId": vpc });
    if let Some(name) = name {
        request["Name"] = json!(name);
    }
    call(router, "AssociateResolverRule", request).await
}

/// One rule is associated with one VPC at most once, so however many callers
/// race on a single pair exactly ONE association is created: the others are
/// refused as the resource that already exists, and the list reports the one
/// record — the same one, in the same place, however often it is asked for.
///
/// The rows differ along how many callers race on the pair.
#[tokio::test]
async fn concurrent_associates_of_one_pair_leave_exactly_one_listed_record() {
    for racers in [2_usize, 3, 5, 8] {
        let case = format!("{racers} racers on one rule/VPC pair");
        let router = Arc::new(emulator());
        let endpoint = outbound_endpoint(&router, &format!("endpoint-race-{racers}")).await;
        let rule = forward_rule(
            &router,
            &format!("rule-race-{racers}"),
            &format!("race-{racers}.associations.example.com"),
            &endpoint,
        )
        .await;
        let contested = vpc(racers);

        let mut tasks = Vec::new();
        for racer in 0..racers {
            let router = Arc::clone(&router);
            let rule = rule.clone();
            let contested = contested.clone();
            tasks.push(tokio::spawn(async move {
                try_associate(
                    &router,
                    &rule,
                    &contested,
                    Some(format!("racer-{racer}-association")),
                )
                .await
            }));
        }

        let mut created: Vec<String> = Vec::new();
        let mut refused: Vec<Value> = Vec::new();
        for task in tasks {
            let (status, body) = task.await.expect("the racing task completed");
            if status == 200 {
                created.push(string_member(
                    body.get("ResolverRuleAssociation").unwrap_or(&Value::Null),
                    "Id",
                ));
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a losing racer answers HTTP 400, got {status}: {body}"
                );
                refused.push(body);
            }
        }

        assert_eq!(
            created.len(),
            1,
            "case {case:?}: exactly one racer may create the association, got {created:?}"
        );
        assert_eq!(
            refused.len(),
            racers - 1,
            "case {case:?}: every other racer must be refused, got {refused:?}"
        );
        for body in &refused {
            assert!(
                error_type(body).contains("ResourceExistsException"),
                "case {case:?}: a pair the store already holds is the resource that already \
                 exists, got {body}"
            );
            assert!(
                string_member(body, "Message").contains(&contested),
                "case {case:?}: the refusal must name the VPC of the pair it refused, got {body}"
            );
        }

        let listed = list_all(&router).await;
        assert_eq!(
            listed, created,
            "case {case:?}: the list must report the one association the race created, got \
             {listed:?}"
        );
        let again = list_all(&router).await;
        assert_eq!(
            again, listed,
            "case {case:?}: a second list of an unchanged store must agree, got {again:?}"
        );
    }
}

/// The listed order is one stable total order over the stored associations: it
/// holds every record exactly once, two lists of an unchanged store agree, a
/// page walk at any `MaxResults` yields that same sequence, a record created
/// later joins at the END without disturbing the records already there, and a
/// record taken out leaves the others in the order they were in.
///
/// The rows differ along how many associations the store holds, and each row
/// creates them CONCURRENTLY, so the order the list reports is not the order
/// the test issued the calls in — which is what makes the assertion about the
/// order being the store's own rather than the caller's.
#[tokio::test]
async fn the_listed_order_is_one_stable_total_order_over_the_stored_associations() {
    for count in [1_usize, 2, 5, 9] {
        let case = format!("{count} associations created concurrently");
        let router = Arc::new(emulator());
        let endpoint = outbound_endpoint(&router, &format!("endpoint-order-{count}")).await;
        let rule = forward_rule(
            &router,
            &format!("rule-order-{count}"),
            &format!("order-{count}.associations.example.com"),
            &endpoint,
        )
        .await;

        let mut tasks = Vec::new();
        for index in 0..count {
            let router = Arc::clone(&router);
            let rule = rule.clone();
            let with = vpc(index);
            tasks.push(tokio::spawn(async move {
                try_associate(&router, &rule, &with, None).await
            }));
        }
        let mut created: BTreeSet<String> = BTreeSet::new();
        for task in tasks {
            let (status, body) = task.await.expect("the associating task completed");
            assert_eq!(
                status, 200,
                "case {case:?}: each distinct pair associates, got {status}: {body}"
            );
            created.insert(string_member(
                body.get("ResolverRuleAssociation").unwrap_or(&Value::Null),
                "Id",
            ));
        }
        assert_eq!(
            created.len(),
            count,
            "case {case:?}: each association must carry an id of its own, got {created:?}"
        );

        let order = list_all(&router).await;
        assert_eq!(
            order.iter().cloned().collect::<BTreeSet<String>>(),
            created,
            "case {case:?}: the list must hold every created association exactly once, got \
             {order:?}"
        );
        assert_eq!(
            order.len(),
            count,
            "case {case:?}: the list must hold no record twice, got {order:?}"
        );
        assert_eq!(
            list_all(&router).await,
            order,
            "case {case:?}: two lists of an unchanged store must report one order"
        );

        // Every way of cutting the collection reports that same order: a page
        // of one, a page the collection does not fill, each bound in between,
        // and the documented maximum.
        let mut page_sizes: BTreeSet<i64> = BTreeSet::from([1, 100]);
        for size in 1..=count {
            page_sizes.insert(i64::try_from(size).unwrap_or(1));
        }
        for page_size in page_sizes {
            let walked = walk_pages(&router, &case, page_size).await;
            assert_eq!(
                walked, order,
                "case {case:?}: a walk at MaxResults {page_size} must report the one order, got \
                 {walked:?}"
            );
        }

        // A record created after the page was walked joins at the end; the
        // records already there keep their places.
        let (status, appended) = try_associate(&router, &rule, &vpc(count + 100), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: one more distinct pair associates, got {status}: {appended}"
        );
        let appended = string_member(
            appended.get("ResolverRuleAssociation").unwrap_or(&Value::Null),
            "Id",
        );
        let mut expected = order.clone();
        expected.push(appended.clone());
        let grown = list_all(&router).await;
        assert_eq!(
            grown, expected,
            "case {case:?}: a later association must join the end of the order, got {grown:?}"
        );

        // And a record taken out of the middle leaves the rest in the order
        // they were already in.
        let removed = grown[grown.len() / 2].clone();
        let fetched = ok(
            &router,
            "GetResolverRuleAssociation",
            json!({ "ResolverRuleAssociationId": removed.clone() }),
        )
        .await;
        let fetched = fetched
            .get("ResolverRuleAssociation")
            .unwrap_or(&Value::Null);
        ok(
            &router,
            "DisassociateResolverRule",
            json!({
                "ResolverRuleId": string_member(fetched, "ResolverRuleId"),
                "VPCId": string_member(fetched, "VPCId"),
            }),
        )
        .await;
        let shrunk = list_all(&router).await;
        assert_eq!(
            shrunk,
            grown
                .iter()
                .filter(|id| **id != removed)
                .cloned()
                .collect::<Vec<String>>(),
            "case {case:?}: removing {removed:?} must leave the others in their order, got \
             {shrunk:?}"
        );
    }
}
