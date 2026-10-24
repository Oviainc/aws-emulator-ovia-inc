// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a customer DELETE of a Firewall Manager list may
//! remove: `DeleteAppsList` and `DeleteProtocolsList` against both namespaces of
//! their own kind of list.
//!
//! The rule asserted here, in one sentence: Firewall Manager keeps AWS-MANAGED
//! (default) lists and customer-created lists in separate namespaces, and an
//! operation whose request carries no namespace selector resolves a `ListId`
//! only among the CUSTOMER lists — so a delete removes a customer list and
//! answers the modelled `ResourceNotFoundException`, changing nothing, for an id
//! that names a managed list.
//!
//! The model is what splits the namespaces: `GetAppsListRequest` declares
//! `DefaultList` and `ListAppsListsRequest` declares `DefaultLists` (likewise
//! `GetProtocolsListRequest` / `ListProtocolsListsRequest`), while
//! `DeleteAppsListRequest` and `DeleteProtocolsListRequest` declare `ListId`
//! alone — so a delete addresses one namespace only, the customer's.
//!
//! The table in [`cases`] is the class the rule ranges over: the rows are the
//! product of the list KIND (applications, protocols) and the list's shape — its
//! member count (none, one, several), its `ListName` at both published bounds of
//! `com.amazonaws.fms#ResourceName` (1..128, from
//! `smithy_query(shape="AppsListData")`), and the tag set it carries (none, one,
//! several). Every assertion is made on every row, against a managed and a
//! customer list seeded alike.
//!
//! Managed lists are seeded through the store because no API creates one (they
//! are AWS's own), and applications lists are seeded through the store because
//! `PutAppsList` is still a 501 stub tracked by its own issue. A seeded list is
//! then observed only through the WIRE: its tags through `ListTagsForResource`,
//! and the list itself through `GetAppsList` / `GetProtocolsList` in each
//! namespace.

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::App;
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness and the store/seeding helpers live in one place for this test
// target, so no two suites can drift in how a request is signed, how an error
// envelope is read or how a resource is seeded.
use crate::administrator_gated_operations::{
    associate_admin, seed_apps_list, seed_protocols_list, with_store,
};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, REGION};
use crate::tagging_and_managing_admins::{list_tags, tag_list, tag_pairs};

/// The two kinds of Firewall Manager list that are split into a managed and a
/// customer namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    AppsList,
    ProtocolsList,
}

const KINDS: [Kind; 2] = [Kind::AppsList, Kind::ProtocolsList];

impl Kind {
    /// The delete operation a customer calls for this kind of list.
    fn delete_operation(self) -> &'static str {
        match self {
            Kind::AppsList => "DeleteAppsList",
            Kind::ProtocolsList => "DeleteProtocolsList",
        }
    }

    /// The reader a customer calls for this kind of list.
    fn get_operation(self) -> &'static str {
        match self {
            Kind::AppsList => "GetAppsList",
            Kind::ProtocolsList => "GetProtocolsList",
        }
    }

    /// The output member the list itself is reported under
    /// (`GetAppsListResponse$AppsList` /
    /// `GetProtocolsListResponse$ProtocolsList`) — which is also the member the
    /// list's own entries live under inside it.
    fn list_member(self) -> &'static str {
        match self {
            Kind::AppsList => "AppsList",
            Kind::ProtocolsList => "ProtocolsList",
        }
    }

    /// This kind's ARN for `list_id`, built by the service's own ARN helper so
    /// the test writes no ARN literal.
    fn arn_of(self, list_id: &str) -> String {
        match self {
            Kind::AppsList => state::apps_list_arn(REGION, list_id),
            Kind::ProtocolsList => state::protocols_list_arn(REGION, list_id),
        }
    }

    /// Seed one list of this kind in the namespace `default_list` selects, with
    /// `members` entries in it.
    fn seed(
        self,
        store: &mut state::ComAmazonawsFmsAwsfms20180101State,
        list_id: &str,
        list_name: &str,
        members: &[String],
        default_list: bool,
    ) {
        match self {
            Kind::AppsList => {
                seed_apps_list(
                    store,
                    REGION,
                    list_id,
                    list_name,
                    &apps(members),
                    default_list,
                );
            }
            Kind::ProtocolsList => {
                let protocols: Vec<&str> = members.iter().map(String::as_str).collect();
                seed_protocols_list(store, list_id, list_name, &protocols, default_list);
            }
        }
    }

    /// The wire value a reader must report for the list's own entry array: `App`
    /// structures for an applications list, protocol tokens for a protocols
    /// list.
    fn reported_members(self, members: &[String]) -> Value {
        match self {
            Kind::AppsList => Value::Array(
                apps(members)
                    .into_iter()
                    .map(|app| {
                        json!({
                            "AppName": app.app_name,
                            "Protocol": app.protocol,
                            "Port": app.port,
                        })
                    })
                    .collect(),
            ),
            Kind::ProtocolsList => json!(members),
        }
    }

    /// Whether this kind's store still holds a list under `list_id`, and the
    /// `ListName` it holds for it.
    fn stored_name(
        self,
        store: &state::ComAmazonawsFmsAwsfms20180101State,
        list_id: &str,
    ) -> Option<String> {
        match self {
            Kind::AppsList => store
                .apps_lists
                .get(list_id)
                .map(|record| record.list.list_name.clone()),
            Kind::ProtocolsList => store
                .protocols_lists
                .get(list_id)
                .map(|record| record.list.list_name.clone()),
        }
    }
}

/// `members` as the `App` structures an applications list holds: one
/// application per name, each on a port inside the published `IPPortNumber`
/// range 0..65535 (`smithy_query(shape="App")`).
fn apps(members: &[String]) -> Vec<App> {
    members
        .iter()
        .enumerate()
        .map(|(index, name)| App {
            app_name: name.clone(),
            protocol: "tcp".to_string(),
            port: 80 + index as i64,
        })
        .collect()
}

/// One row of the table: a list shape, seeded once in each namespace.
struct Case {
    /// Names the row in every assertion message.
    name: String,
    list_name: String,
    /// The list's own members: application names for an applications list,
    /// protocol tokens for a protocols list.
    members: Vec<String>,
    /// The `TagList` the row's lists carry, written through `TagResource`.
    tags: Value,
}

fn members(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// The list class the rule is asserted over.
///
/// `ListName` is `com.amazonaws.fms#ResourceName` (1..128 characters), and the
/// rows include both published bounds and one in between; the member arrays span
/// none, one and several entries. The protocol tokens are
/// `com.amazonaws.fms#Protocol` values (IP protocol keywords and the protocol
/// NUMBERS naming the same protocols).
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one member, no tags".to_string(),
            // The published ResourceName minimum: one character.
            list_name: "a".to_string(),
            members: members(&["TCP"]),
            tags: tag_list(&[]),
        },
        Case {
            name: "several members, one tag".to_string(),
            list_name: "list.with-many_members".to_string(),
            members: members(&["TCP", "UDP", "ICMP", "ESP"]),
            tags: tag_list(&[("team", "network")]),
        },
        Case {
            name: "numeric members, several tags".to_string(),
            // The published ResourceName maximum: 128 characters.
            list_name: "n".repeat(128),
            // 6 is TCP, 17 is UDP, 1 is ICMP.
            members: members(&["6", "17", "1"]),
            tags: tag_list(&[("env", "test"), ("owner", "")]),
        },
        Case {
            name: "no members at all, no tags".to_string(),
            list_name: "empty-list".to_string(),
            members: Vec::new(),
            tags: tag_list(&[]),
        },
    ]
}

/// A well-formed `ListId` — the model's `com.amazonaws.fms#ListId`: exactly 36
/// characters matching `^[a-z0-9A-Z-]{36}$` — derived from `seed`, so each row
/// names its own lists and the suite carries no id literal.
fn list_id(seed: &str) -> String {
    let mut id: String = seed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    id.truncate(36);
    while id.chars().count() < 36 {
        id.push('-');
    }
    id
}

/// Write `tags` onto the resource `arn` names through the wire, the way a
/// customer tags any Firewall Manager resource.
async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: &Value, case: &str) {
    if tags.as_array().is_some_and(Vec::is_empty) {
        return;
    }
    let (status, body) = call(
        router,
        "TagResource",
        &json!({ "ResourceArn": arn, "TagList": tags }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected TagResource to answer 200, got {status} with {body}"
    );
}

/// The managed/customer pair of ids a row uses for one kind of list.
fn ids(kind: Kind, index: usize) -> (String, String) {
    let tag = match kind {
        Kind::AppsList => "apps",
        Kind::ProtocolsList => "protocols",
    };
    (
        list_id(&format!("managed-{tag}-{index}")),
        list_id(&format!("customer-{tag}-{index}")),
    )
}

/// The rule: a delete resolves its `ListId` in the CUSTOMER namespace only, so a
/// managed (default) list is refused with `ResourceNotFoundException` and
/// survives the call — tags and all — while the customer list of the same shape
/// is removed.
#[tokio::test]
async fn a_customer_delete_refuses_a_managed_list_and_leaves_it_stored() {
    for kind in KINDS {
        for (index, case) in cases().into_iter().enumerate() {
            let (router, engine) = emulator_with_engine();
            associate_admin(&router).await;

            let (managed_id, customer_id) = ids(kind, index);
            let managed_arn = kind.arn_of(&managed_id);
            let customer_arn = kind.arn_of(&customer_id);
            let case_name = format!("{kind:?} / {}", case.name);

            with_store(&engine, |store| {
                kind.seed(store, &managed_id, &case.list_name, &case.members, true);
                kind.seed(store, &customer_id, &case.list_name, &case.members, false);
            });
            tag_resource(&router, &managed_arn, &case.tags, &case_name).await;
            tag_resource(&router, &customer_arn, &case.tags, &case_name).await;
            // The pairs the row wrote, as the sorted set the tag assertions
            // compare against.
            let expected_tags = tag_pairs(&case.tags);

            // Deleting the managed list is refused, and twice over: a refusal
            // that had written something would answer differently the second
            // time.
            for attempt in 1..=2 {
                let (status, body) = call(
                    &router,
                    kind.delete_operation(),
                    &json!({ "ListId": managed_id }),
                )
                .await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {case_name:?} attempt {attempt}: {} must refuse a Firewall Manager \
                     managed list, got {status} with {body}",
                    kind.delete_operation()
                );
            }

            // And it is still there, with the name it was stored under.
            let stored = with_store(&engine, |store| kind.stored_name(store, &managed_id));
            assert_eq!(
                stored.as_deref(),
                Some(case.list_name.as_str()),
                "case {case_name:?}: a refused delete must leave the managed list stored, got \
                 {stored:?}"
            );
            // Its ARN still resolves on the wire, carrying the tags it held: the
            // refused delete dropped neither the list nor its tag-index entry.
            let (status, code, tags) = list_tags(&router, &managed_arn).await;
            assert_eq!(
                (status, code.as_str(), tags),
                (200, "", expected_tags.clone()),
                "case {case_name:?}: the managed list must still carry its tags"
            );

            // The CUSTOMER list of the same shape is in the namespace the delete
            // addresses, so it goes — and a second delete then finds nothing.
            let (status, body) = call(
                &router,
                kind.delete_operation(),
                &json!({ "ListId": customer_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case_name:?}: expected {} to delete the customer list, got {status} with \
                 {body}",
                kind.delete_operation()
            );
            let stored = with_store(&engine, |store| kind.stored_name(store, &customer_id));
            assert_eq!(
                stored, None,
                "case {case_name:?}: a deleted customer list must be gone from state, got \
                 {stored:?}"
            );
            let (status, code, _) = list_tags(&router, &customer_arn).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case_name:?}: a deleted list's ARN must resolve to nothing"
            );
            let (status, body) = call(
                &router,
                kind.delete_operation(),
                &json!({ "ListId": customer_id }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case_name:?}: a deleted list must not be deletable twice, got {status} \
                 with {body}"
            );

            // Deleting the customer list left the managed one of the SAME shape
            // alone.
            let stored = with_store(&engine, |store| kind.stored_name(store, &managed_id));
            assert_eq!(
                stored.as_deref(),
                Some(case.list_name.as_str()),
                "case {case_name:?}: deleting a customer list must not touch the managed list, \
                 got {stored:?}"
            );
        }
    }
}

/// The namespace split is what a reader reports too: a managed list is read back
/// only with `DefaultList: true`, and it still reads back — byte for byte —
/// after a delete attempt, which is the end-to-end round trip for a list that
/// survived.
#[tokio::test]
async fn a_managed_list_still_reads_back_after_a_refused_delete() {
    for kind in KINDS {
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;

        for (index, case) in cases().into_iter().enumerate() {
            let (managed_id, customer_id) = ids(kind, index);
            let case_name = format!("{kind:?} / {}", case.name);
            with_store(&engine, |store| {
                kind.seed(store, &managed_id, &case.list_name, &case.members, true);
                kind.seed(store, &customer_id, &case.list_name, &case.members, false);
            });

            // The managed list is in the managed namespace and in no other.
            let (status, before) = call(
                &router,
                kind.get_operation(),
                &json!({ "ListId": managed_id, "DefaultList": true }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case_name:?}: expected the managed list to be readable, got {status} with \
                 {before}"
            );
            assert_eq!(
                before[kind.list_member()]["ListName"].as_str(),
                Some(case.list_name.as_str()),
                "case {case_name:?}: the managed list must report its stored name, got {before}"
            );
            let (status, body) = call(
                &router,
                kind.get_operation(),
                &json!({ "ListId": managed_id, "DefaultList": false }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case_name:?}: a managed list is not a customer list, got {status} with \
                 {body}"
            );

            let (status, body) = call(
                &router,
                kind.delete_operation(),
                &json!({ "ListId": managed_id }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case_name:?}: {} must refuse a managed list, got {status} with {body}",
                kind.delete_operation()
            );

            // The refused delete changed nothing a reader can see.
            let (status, after) = call(
                &router,
                kind.get_operation(),
                &json!({ "ListId": managed_id, "DefaultList": true }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case_name:?}: the managed list must still be readable, got {status} with \
                 {after}"
            );
            assert_eq!(
                after, before,
                "case {case_name:?}: a refused delete must not change what the reader reports, \
                 got {after}"
            );
            assert_eq!(
                after[kind.list_member()][kind.list_member()],
                kind.reported_members(&case.members),
                "case {case_name:?}: the managed list must keep its own entries, got {after}"
            );

            // While the customer list of the same shape deletes and is then gone
            // from its own namespace.
            let (status, body) = call(
                &router,
                kind.delete_operation(),
                &json!({ "ListId": customer_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case_name:?}: expected the customer list to delete, got {status} with \
                 {body}"
            );
            let (status, body) = call(
                &router,
                kind.get_operation(),
                &json!({ "ListId": customer_id, "DefaultList": false }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case_name:?}: a deleted customer list must be gone, got {status} with \
                 {body}"
            );
        }
    }
}
