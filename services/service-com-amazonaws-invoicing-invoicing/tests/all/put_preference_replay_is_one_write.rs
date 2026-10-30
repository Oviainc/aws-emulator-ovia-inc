// SPDX-License-Identifier: CC0-1.0

//! `PutProcurementPortalPreference` under a `ClientToken`, driven end to end over this
//! service's wire protocol with the shared harness of `refused_procurement_portal_writes`.
//!
//! The rule under test: a `ClientToken` identifies ONE request of an `@idempotent`
//! operation, so a repeat of that request against the same resource is a REPLAY — it
//! answers what the first call answered and the write lands once, leaving every member a
//! reader reports (the configuration, and the `Version`/`LastUpdateDate` pair every applied
//! write of the resource advances) exactly where the first call left them. A request under
//! a token this operation has not seen against that resource, or under no token at all, is
//! its own request and its own write.
//!
//! The rows span the token forms `BasicStringWithoutSpace` admits (`@length(min: 0, max:
//! 1024)`, `@pattern ^\S+$` —
//! `smithy_query(query="operation", operation="PutProcurementPortalPreference")`), how many
//! times the request is retried, and which optional configuration members it states.

use serde_json::{json, Value};

use crate::refused_procurement_portal_writes::{
    arn_of, call, emulator_with_engine, error_code, get_preference, last_update, listed_summary,
    minimal_create, ok, version, PORTAL_INSTANCE_ENDPOINT, PORTAL_NAMES,
};

/// One class of retried `Put`: the token it carries, how many times the client repeats the
/// byte-identical request, and the configuration members the request states beyond the ones
/// the operation marks required.
struct Row {
    label: &'static str,
    /// `None` is the row that carries no `ClientToken` at all, which has nothing to replay.
    token: Option<String>,
    /// How many times the SAME request is sent, the first call included.
    sends: i64,
    /// The optional members the request states, merged into the required ones.
    extra: Value,
}

/// The rows of the table: token shapes inside the member's bound, retry counts, and
/// configurations differing in which optional members they state.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "no token, repeated",
            token: None,
            sends: 3,
            extra: json!({}),
        },
        Row {
            label: "shortest token",
            token: Some("t".to_string()),
            sends: 2,
            extra: json!({}),
        },
        Row {
            label: "hyphenated token, states an endpoint",
            token: Some("a-client-token".to_string()),
            sends: 3,
            extra: json!({ "ProcurementPortalInstanceEndpoint": PORTAL_INSTANCE_ENDPOINT }),
        },
        Row {
            label: "punctuated token, states a secret and a selector",
            token: Some("9f2b/4c1d:+token".to_string()),
            sends: 5,
            extra: json!({
                "ProcurementPortalSharedSecret": "s3cr3t-value",
                "Selector": { "SellerOfRecords": ["AMCS_LLC"] },
            }),
        },
        Row {
            label: "longest token",
            token: Some("t".repeat(1024)),
            sends: 4,
            extra: json!({ "ProcurementPortalInstanceEndpoint": PORTAL_INSTANCE_ENDPOINT }),
        },
    ]
}

/// The request one row sends: every member the operation marks required, the row's optional
/// members, and the row's token when it carries one.
fn put_request(row: &Row, arn: &str, contact_name: &str) -> Value {
    let mut request = json!({
        "ProcurementPortalPreferenceArn": arn,
        "Contacts": [{ "Name": contact_name, "Email": "ops@example.com" }],
        "EinvoiceDeliveryEnabled": true,
        "PurchaseOrderRetrievalEnabled": true,
    });
    if let Some(members) = row.extra.as_object() {
        for (member, value) in members {
            request[member.as_str()] = value.clone();
        }
    }
    if let Some(token) = row.token.as_deref() {
        request["ClientToken"] = json!(token);
    }
    request
}

/// RULE: a repeat of one `@idempotent` request — the same `ClientToken` against the same
/// resource — is replayed, so it answers the first call's answer and the resource takes ONE
/// write; a call under a token not seen against that resource, or under none, is its own
/// write.
#[tokio::test]
async fn a_put_replayed_under_one_client_token_is_one_write() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in rows().iter().enumerate() {
        let label = row.label;
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &minimal_create(
                    PORTAL_NAMES[index % PORTAL_NAMES.len()],
                    &format!("BUYER-REPLAY-{index}"),
                    &format!("SUPPLIER-REPLAY-{index}"),
                ),
            )
            .await,
        );
        // The create left the preference at its first version.
        let created = get_preference(&router, &arn).await;
        assert_eq!(
            version(&created),
            1,
            "case {label}: expected the create to leave Version 1, got {created}"
        );

        let request = put_request(row, &arn, "Replayed Contact");
        let first = ok(&router, "PutProcurementPortalPreference", &request).await;
        assert_eq!(
            arn_of(&first),
            arn,
            "case {label}: the first put answers with the preference it wrote, got {first}"
        );
        let after_first = get_preference(&router, &arn).await;
        assert_eq!(
            version(&after_first),
            2,
            "case {label}: expected the first put to leave Version 2, got {after_first}"
        );

        for send in 2..=row.sends {
            let answered = ok(&router, "PutProcurementPortalPreference", &request).await;
            assert_eq!(
                arn_of(&answered),
                arn,
                "case {label} send {send}: the answer names the same preference, got {answered}"
            );
            let reported = get_preference(&router, &arn).await;
            match row.token {
                // The repeat is the SAME request, so the resource still reports exactly
                // what the first call left — the configuration AND the pair every applied
                // write advances.
                Some(_) => assert_eq!(
                    reported, after_first,
                    "case {label} send {send}: a replay must leave the preference as the \
                     first call left it ({after_first}), got {reported}"
                ),
                // Nothing identifies this call as a repeat, so it is another write.
                None => {
                    let expected = 1 + send;
                    assert_eq!(
                        version(&reported),
                        expected,
                        "case {label} send {send}: expected Version {expected}, got {reported}"
                    );
                }
            }
            // Whichever it was, the configuration the request states is what the resource
            // holds.
            assert_eq!(
                reported.get("Contacts"),
                after_first.get("Contacts"),
                "case {label} send {send}: expected the stated Contacts {:?}, got {:?}",
                after_first.get("Contacts"),
                reported.get("Contacts")
            );
            // Both readers of this plane report the one stored resource.
            let summary = listed_summary(&router, &arn).await;
            assert_eq!(
                version(&summary),
                version(&reported),
                "case {label} send {send}: the listing reports the version the single-resource \
                 reader does ({}), got {summary}",
                version(&reported)
            );
        }

        let before_fresh = get_preference(&router, &arn).await;
        // A token this operation has not seen against this preference is a DIFFERENT
        // request, so it is applied.
        let mut fresh = request.clone();
        fresh["ClientToken"] = json!(format!("fresh-{index}"));
        fresh["Contacts"] = json!([{ "Name": "Fresh Contact", "Email": "fresh@example.com" }]);
        ok(&router, "PutProcurementPortalPreference", &fresh).await;
        let after_fresh = get_preference(&router, &arn).await;
        assert_eq!(
            version(&after_fresh),
            version(&before_fresh) + 1,
            "case {label}: a put under an unseen token is one more write, expected Version {}, \
             got {after_fresh}",
            version(&before_fresh) + 1
        );
        assert_eq!(
            after_fresh.get("Contacts"),
            fresh.get("Contacts"),
            "case {label}: the unseen token's configuration is applied, got {after_fresh}"
        );
        assert!(
            last_update(&after_fresh) >= last_update(&before_fresh),
            "case {label}: expected LastUpdateDate at or after the previous write {}, got {}",
            last_update(&before_fresh),
            last_update(&after_fresh)
        );
    }
}

/// RULE: a replay record belongs to the (operation, resource, token) triple, so one token
/// replays only the call it was first used for: against another resource it is a request
/// that resource has not seen, and once the resource is gone there is nothing to answer for
/// — the modelled not-found is reported instead.
#[tokio::test]
async fn a_put_replay_is_keyed_on_the_preference_the_token_was_used_against() {
    let (router, _engine) = emulator_with_engine();
    let shared_token = "one-token-two-preferences";

    let mut arns = Vec::new();
    for (index, portal) in PORTAL_NAMES.iter().enumerate() {
        arns.push(arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &minimal_create(
                    portal,
                    &format!("BUYER-SHARED-{index}"),
                    &format!("SUPPLIER-SHARED-{index}"),
                ),
            )
            .await,
        ));
    }

    // The same token against each preference in turn: each is a request its own preference
    // has not seen, so each is applied once.
    for (index, arn) in arns.iter().enumerate() {
        let request = json!({
            "ProcurementPortalPreferenceArn": arn,
            "Contacts": [{ "Name": format!("Shared {index}"), "Email": "ops@example.com" }],
            "EinvoiceDeliveryEnabled": true,
            "PurchaseOrderRetrievalEnabled": false,
            "ClientToken": shared_token,
        });
        ok(&router, "PutProcurementPortalPreference", &request).await;
        let reported = get_preference(&router, arn).await;
        assert_eq!(
            version(&reported),
            2,
            "case preference {index}: one token used against another preference is that \
             preference's own first update, expected Version 2, got {reported}"
        );
        assert_eq!(
            reported.get("PurchaseOrderRetrievalEnabled"),
            Some(&json!(false)),
            "case preference {index}: the stated configuration is applied, got {reported}"
        );
        // And repeating it against this preference is now the replay.
        ok(&router, "PutProcurementPortalPreference", &request).await;
        let replayed = get_preference(&router, arn).await;
        assert_eq!(
            replayed, reported,
            "case preference {index}: the repeat under the same token writes nothing more, \
             got {replayed}"
        );
    }

    // A preference that is GONE has no configuration to answer for, whatever token the
    // request carries: the replay record is not a resource.
    let removed = arns
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("expected at least one preference"));
    ok(
        &router,
        "DeleteProcurementPortalPreference",
        &json!({"ProcurementPortalPreferenceArn": removed}),
    )
    .await;
    for token in [Some(shared_token), Some("never-seen"), None] {
        let mut request = json!({
            "ProcurementPortalPreferenceArn": removed,
            "Contacts": [{ "Name": "Gone", "Email": "ops@example.com" }],
            "EinvoiceDeliveryEnabled": true,
            "PurchaseOrderRetrievalEnabled": true,
        });
        if let Some(token) = token {
            request["ClientToken"] = json!(token);
        }
        let (status, body) = call(&router, "PutProcurementPortalPreference", &request).await;
        assert_eq!(
            status, 404,
            "case token {token:?}: a put against a preference that is gone is the modelled \
             404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case token {token:?}: expected the modelled not-found error, got {body}"
        );
    }
}
