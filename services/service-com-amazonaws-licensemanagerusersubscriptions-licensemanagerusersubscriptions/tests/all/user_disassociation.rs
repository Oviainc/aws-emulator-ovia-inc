// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DISASSOCIATION lifecycle of a user association, driven over
//! the real wire path: every call goes through the router's `dispatch`, so the request is
//! deserialized, constraint-checked, handled and serialized exactly as a client's would
//! be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded request:
//! a call that answers with a resource in a NON-TERMINAL status leaves that resource in
//! state, reported in that status, until the work the status announces falls due — so
//! `DisassociateUser`, which answers `DISASSOCIATING` (`InstanceUserSummary$Status`, "The
//! status of a user associated with an EC2 instance", is a free-form string, and the
//! service's own vocabulary spells the teardown with the progressive `DISASSOCIATING`),
//! keeps the association observable in `DISASSOCIATING`, and keeps the instance it names
//! in this service's instance view, instead of the state vanishing between two reads.
//!
//! What this suite pins:
//!
//! * the disassociate answer and every later read report the same stored association in
//!   `DISASSOCIATING`, with the instant it ended unchanged (`@idempotent`: a repeat call
//!   restarts nothing), and the identifiers stop resolving only once the teardown the
//!   clock owes has completed;
//! * the status the association was in when the disassociation was accepted
//!   (`ASSOCIATING` or `ASSOCIATED`) does not change that lifecycle;
//! * the instance stays in this service's view while a user is still being disassociated
//!   from it, and leaves it when the last teardown completes;
//! * the `(IdentityProvider, InstanceId, Username)` triple stays OCCUPIED while the
//!   teardown runs, so re-associating that user with that instance is the modelled
//!   `ConflictException` — and mints a new association only once the teardown is done;
//! * an association being torn down is no longer a resource the tagging surface resolves.
//!
//! `ListUserAssociations` is still a scaffolded 501 (see
//! `issues/.../license-manager-user-subscriptions-listuserassociations-stub.md`), so the
//! read-back of one association here is the `@idempotent` `DisassociateUser` itself, and
//! `ListInstances` is the list the instance view is read from.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::{
    scaffolded::{BAD_REQUEST_EXCEPTION_CODE, INVALID_INPUT_RESOURCE_ARN_MESSAGE},
    ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState,
};

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider-registration helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, call, harness, provider_union, register, Harness, Reply, PRODUCTS,
};
use super::resource_resolution_refusals::{
    assert_fails_closed, assert_invalid_resource_arn, associate, disassociate, held_instance,
    list_tags, registration, tag,
};

/// How long a read waits before it must see a transition the clock owed.
///
/// A clock-owed transition in this emulator falls due inside 250-750 ms of the call that
/// started it, so a read this far later must observe it whatever span the service chose
/// inside that window. The same wait the license-server-endpoint deletion suite uses.
const PAST_THE_OWED_TRANSITION: Duration = Duration::from_millis(900);

/// The tags an association's `AssociateUser` supplies when the row carries any.
fn row_tags() -> Value {
    json!({ "team": "licensing" })
}

/// One association to disassociate. The rows differ along what the rule generalises over:
/// the instance, the directory-id form, the user name, whether a `Domain` narrows the
/// association, whether it carries tags, and which of the two addressings
/// `DisassociateUser` admits the disassociation is asked for by.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// An index into the account's seeded EC2 fleet; two rows share one instance, so the
    /// instance view is exercised with more than one association on it.
    instance: usize,
    /// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; both spellings appear below.
    directory_id: &'static str,
    username: &'static str,
    domain: Option<&'static str>,
    tagged: bool,
    /// The disassociation is addressed by `InstanceUserArn` when true, and by the
    /// `(IdentityProvider, InstanceId, Username)` triple when false — the model admits
    /// both, so the lifecycle must not depend on which one a caller uses.
    by_arn: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "aws-managed directory, no domain, untagged, addressed by ARN",
        instance: 0,
        directory_id: "d-0123456789",
        username: "first.user",
        domain: None,
        tagged: false,
        by_arn: true,
    },
    Row {
        case: "the same directory on another instance, with a domain, tagged, by triple",
        instance: 1,
        directory_id: "d-0123456789",
        username: "second.user",
        domain: Some("corp.example"),
        tagged: true,
        by_arn: false,
    },
    Row {
        case: "a shared directory, a third instance, with a domain, by ARN",
        instance: 2,
        directory_id: "sd-abcdef0123",
        username: "third.user",
        domain: Some("example.internal"),
        tagged: false,
        by_arn: true,
    },
    Row {
        case: "a second user on an instance that already has one, tagged, by triple",
        instance: 0,
        directory_id: "sd-abcdef0123",
        username: "fourth.user",
        domain: None,
        tagged: true,
        by_arn: false,
    },
];

impl Row {
    /// The `AssociateUser` body for this row.
    fn associate_request(&self) -> Value {
        let mut body = json!({
            "Username": self.username,
            "InstanceId": held_instance(self.instance),
            "IdentityProvider": provider_union(self.directory_id),
        });
        if let Some(domain) = self.domain {
            body["Domain"] = json!(domain);
        }
        if self.tagged {
            body["Tags"] = row_tags();
        }
        body
    }

    /// The `DisassociateUser` body for this row, in the addressing the row names.
    fn disassociate_request(&self, arn: &str) -> Value {
        let mut body = if self.by_arn {
            json!({ "InstanceUserArn": arn })
        } else {
            json!({
                "Username": self.username,
                "InstanceId": held_instance(self.instance),
                "IdentityProvider": provider_union(self.directory_id),
            })
        };
        if let Some(domain) = self.domain {
            body["Domain"] = json!(domain);
        }
        body
    }

    /// The `DisassociateUser` body in the OTHER addressing, so the refusal after the
    /// teardown is pinned for both forms.
    fn other_disassociate_request(&self, arn: &str) -> Value {
        let mut flipped = *self;
        flipped.by_arn = !self.by_arn;
        flipped.disassociate_request(arn)
    }
}

/// A harness whose account has a registration for every directory [`ROWS`] names, so a
/// user can be associated through each of them.
async fn setup() -> Harness {
    let h = harness();
    for (directory_id, product) in [("d-0123456789", PRODUCTS[0]), ("sd-abcdef0123", PRODUCTS[4])] {
        register(
            &h,
            &registration(directory_id, product, &["subnet-0123456789abcdef0"], None),
        )
        .await
        .ok(directory_id);
    }
    h
}

/// Associate every row and answer the `InstanceUserArn` each one minted.
async fn associate_rows(h: &Harness) -> Vec<(Row, String)> {
    let mut minted = Vec::new();
    for row in ROWS {
        let reply = associate(h, &row.associate_request()).await;
        let summary = reply.ok(row.case)["InstanceUserSummary"].clone();
        minted.push((*row, as_str(&summary, "InstanceUserArn", row.case).to_string()));
    }
    minted
}

/// The instance ids `ListInstances` reports, in store order.
async fn listed_instances(h: &Harness, case: &str) -> Vec<String> {
    let reply = call(h, "/instance/ListInstances", &json!({})).await;
    reply.ok(case)["InstanceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no InstanceSummaries in {:?}", reply.body))
        .iter()
        .map(|summary| as_str(summary, "InstanceId", case).to_string())
        .collect()
}

/// The `InstanceUserSummary` of a reply the call answered 200 for.
fn summary(reply: &Reply, case: &str) -> Value {
    reply.ok(case)["InstanceUserSummary"].clone()
}

/// RULE: a disassociation is ASYNCHRONOUS — the association stays reported in the
/// non-terminal `DISASSOCIATING` status (and its instance stays in the service's view)
/// until the teardown the clock owes completes, and only then do its identifiers stop
/// resolving. The status the association was in when the disassociation was accepted does
/// not change that lifecycle.
#[tokio::test]
async fn a_disassociation_is_observable_in_disassociating_until_the_teardown_completes() {
    for (phase, settle_first) in [
        ("disassociated while still ASSOCIATING", false),
        ("disassociated once ASSOCIATED", true),
    ] {
        let h = setup().await;
        let minted = associate_rows(&h).await;
        if settle_first {
            // The precondition for this phase: the `ASSOCIATED` transition the clock owed
            // has been materialised before the disassociation is asked for.
            tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        }

        for (row, arn) in &minted {
            let case = format!("{} ({phase})", row.case);
            let accepted = disassociate(&h, &row.disassociate_request(arn)).await;
            let answered = summary(&accepted, &case);
            assert_eq!(
                as_str(&answered, "InstanceUserArn", &case),
                arn,
                "case {case:?}: the disassociation must answer the stored association, got \
                 {answered:?}"
            );
            assert_eq!(
                as_str(&answered, "Status", &case),
                "DISASSOCIATING",
                "case {case:?}: expected the non-terminal teardown status, got {answered:?}"
            );
            let ended = as_str(&answered, "DisassociationDate", &case);
            assert!(
                ended.ends_with('Z') && ended.len() == 20,
                "case {case:?}: expected an ISO-8601 UTC instant, got {ended:?}"
            );
            assert!(
                answered.get("AssociationDate").is_some(),
                "case {case:?}: the instant the association started is still reported, got \
                 {answered:?}"
            );

            // A client reading straight after the call still sees the transient status, by
            // either addressing form, with nothing re-stamped (`@idempotent`).
            for (form, request) in [
                ("the same addressing", row.disassociate_request(arn)),
                ("the other addressing", row.other_disassociate_request(arn)),
            ] {
                let case = format!("{case}, read back through {form}");
                let again = disassociate(&h, &request).await;
                assert_eq!(
                    summary(&again, &case),
                    answered,
                    "case {case:?}: a read of an association being torn down must report it \
                     unchanged, got {:?}",
                    again.body
                );
            }

            // The instance is still configured for user-based subscriptions, so it is still
            // in this service's view.
            let listed = listed_instances(&h, &case).await;
            assert!(
                listed.contains(&held_instance(row.instance)),
                "case {case:?}: the instance must stay in view while a user is still being \
                 disassociated from it, got {listed:?}"
            );
        }

        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

        // The teardown the status announced has completed. A READ is enough to see it: the
        // instances have left the view with their last association, whether or not anything
        // was called in between.
        let listed = listed_instances(&h, phase).await;
        assert!(
            listed.is_empty(),
            "case {phase:?}: an instance leaves the view with its last association once the \
             teardown completes, got {listed:?}"
        );

        // And the association is gone, so both addressings fail closed exactly as an
        // identifier that never existed does.
        for (row, arn) in &minted {
            let case = format!("{} ({phase}, past the owed teardown)", row.case);
            assert_fails_closed(
                &format!("{case}, by the row's addressing"),
                arn,
                &disassociate(&h, &row.disassociate_request(arn)).await,
            );
            assert_fails_closed(
                &format!("{case}, by the other addressing"),
                arn,
                &disassociate(&h, &row.other_disassociate_request(arn)).await,
            );
        }

        // Every view the store keeps of an association agrees: a completed teardown
        // unindexed it everywhere, so nothing is left pointing at a record that is gone.
        {
            let store = h
                .engine
                .state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
            let guard = store.lock();
            assert!(
                guard.instance_users.is_empty()
                    && guard.instance_user_order.is_empty()
                    && guard.instance_user_index.is_empty()
                    && guard.instance_users_by_instance.is_empty()
                    && guard.instance_order.is_empty(),
                "case {phase:?}: a completed teardown must leave no index pointing at the \
                 association, got instance_users={:?}, order={:?}, index={:?}, \
                 by_instance={:?}, instances={:?}",
                guard.instance_users.keys().collect::<Vec<_>>(),
                guard.instance_user_order,
                guard.instance_user_index,
                guard.instance_users_by_instance,
                guard.instance_order
            );
        }
    }
}

/// RULE: the `(IdentityProvider, InstanceId, Username)` triple stays occupied while the
/// teardown runs, so associating that user with that instance again is the modelled
/// `ConflictException` rather than a fresh association — and mints a new association, in
/// `ASSOCIATING` under a new ARN, once the teardown has completed.
#[tokio::test]
async fn re_associating_a_user_still_being_disassociated_conflicts() {
    let h = setup().await;
    let minted = associate_rows(&h).await;

    for (row, arn) in &minted {
        disassociate(&h, &row.disassociate_request(arn))
            .await
            .ok(row.case);

        let conflict = associate(&h, &row.associate_request()).await;
        // `ConflictException` carries `@error("server")` and no `@httpError` in this
        // service's model, so the framework serializes it at HTTP 500.
        assert_eq!(
            conflict.status, 500,
            "case {:?}: ConflictException is modelled at HTTP 500, got {conflict:?}",
            row.case
        );
        assert_eq!(
            conflict.error_type.as_deref(),
            Some("ConflictException"),
            "case {:?}: expected ConflictException, got {conflict:?}",
            row.case
        );
        // The refusal changed nothing: the association is still the one being torn down.
        let unchanged = disassociate(&h, &row.disassociate_request(arn)).await;
        assert_eq!(
            as_str(&summary(&unchanged, row.case), "InstanceUserArn", row.case),
            arn,
            "case {:?}: a refused re-association must leave the stored association alone, \
             got {:?}",
            row.case,
            unchanged.body
        );
    }

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    for (row, arn) in &minted {
        let fresh = associate(&h, &row.associate_request()).await;
        let answered = summary(&fresh, row.case);
        let fresh_arn = as_str(&answered, "InstanceUserArn", row.case);
        assert_ne!(
            fresh_arn, arn,
            "case {:?}: an association after a completed teardown is a NEW one, got \
             {answered:?}",
            row.case
        );
        assert_eq!(
            as_str(&answered, "Status", row.case),
            "ASSOCIATING",
            "case {:?}: the new association starts its own lifecycle, got {answered:?}",
            row.case
        );
        assert!(
            answered.get("DisassociationDate").is_none(),
            "case {:?}: a live association has no disassociation date, got {answered:?}",
            row.case
        );
    }
}

/// RULE: an association whose disassociation was accepted is no longer a resource the
/// tagging surface resolves — it is reported so a client can watch the teardown finish,
/// not so it can be changed. The tagging surface refuses an ARN it does not resolve with
/// `BadRequestException` / "Invalid input resource arn".
#[tokio::test]
async fn an_association_being_disassociated_is_no_longer_taggable() {
    let h = setup().await;
    let minted = associate_rows(&h).await;

    for (row, arn) in &minted {
        // The precondition: a live association resolves, reporting whatever tags its
        // `AssociateUser` stored (an untagged row reports an empty map).
        let before = list_tags(&h, arn).await;
        let tags = before.ok(row.case)["Tags"].clone();
        assert_eq!(
            tags.as_object().is_some_and(|tags| !tags.is_empty()),
            row.tagged,
            "case {:?}: a live association reports the tags its association stored, got {:?}",
            row.case,
            before.body
        );

        disassociate(&h, &row.disassociate_request(arn))
            .await
            .ok(row.case);

        for (surface, reply) in [
            ("ListTagsForResource", list_tags(&h, arn).await),
            ("TagResource", tag(&h, arn, &row_tags()).await),
        ] {
            assert_invalid_resource_arn(
                &format!("{}/{surface} (while disassociating)", row.case),
                &reply,
            );
        }
        // Which is a refusal about the RESOURCE, not about the request: the sentence and
        // the code are the tagging surface's fixed ones.
        let refused = list_tags(&h, arn).await;
        assert_eq!(
            refused.error_type.as_deref(),
            Some(BAD_REQUEST_EXCEPTION_CODE),
            "case {:?}: expected {BAD_REQUEST_EXCEPTION_CODE}, got {refused:?}",
            row.case
        );
        assert_eq!(
            refused.message(),
            Some(INVALID_INPUT_RESOURCE_ARN_MESSAGE),
            "case {:?}: expected the tagging surface's fixed sentence, got {refused:?}",
            row.case
        );

        // The association is still REPORTED, which is the whole point of the non-terminal
        // status.
        let still_there = disassociate(&h, &row.disassociate_request(arn)).await;
        assert_eq!(
            as_str(&summary(&still_there, row.case), "Status", row.case),
            "DISASSOCIATING",
            "case {:?}: the association is still reported while it is torn down, got {:?}",
            row.case,
            still_there.body
        );
    }
}
