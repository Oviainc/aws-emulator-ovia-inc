// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteTransformer`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.<Operation>`) through the same router the emulator boots, so every
//! assertion is about what a real SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (the configuration style
//! and members the transformer carries, the status it rests in, how many capabilities
//! reference it, the shape of the id, the signing region).
//!
//! The wire harness (`b2bi_router`, `call`, `error_code`, `text`) and the configuration
//! builders are reused from the sibling suites rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::scaffolded::NOT_FOUND_STATUS;
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{
    b2bi_router, call, call_signed_for, capability_configuration, error_code,
    given_capability_buckets, given_profile, given_transformer, text, SIGNING_REGION,
};
use crate::transformer_lifecycle::{current_members, legacy_members};

/// Create a transformer from `request` and answer its `transformerId`.
async fn created_transformer(router: &MultiServiceRouter, region: &str, request: Value) -> String {
    let (status, created) = call_signed_for(router, region, "CreateTransformer", request).await;
    assert_eq!(
        status, 200,
        "the transformer under test must be created, got {status} {created}"
    );
    text(&created, "transformerId")
}

/// Create a capability whose `configuration.edi.transformerId` names `transformer_id`, and
/// answer its `capabilityId`.
///
/// `transactionSet` / `version` come from the modelled `X12TransactionSet` / `X12Version`
/// enums, so a row can vary the configuration around the reference it is really about.
async fn capability_using(
    router: &MultiServiceRouter,
    name: &str,
    transformer_id: &str,
    transaction_set: &str,
    version: &str,
) -> String {
    given_capability_buckets(router).await;
    let (status, body) = call(
        router,
        "CreateCapability",
        json!({
            "name": name,
            "type": "edi",
            "configuration": capability_configuration(transformer_id, transaction_set, version)
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCapability({name}) must succeed, got {status} {body}"
    );
    text(&body, "capabilityId")
}

/// RULE: `DeleteTransformer` removes the stored transformer, answers HTTP 200 with a body
/// carrying no member, and leaves nothing behind — `GetTransformer` and a repeated
/// `DeleteTransformer` both report the id absent — whatever the deleted record held.
///
/// The rows span both configuration styles, several member subsets, a transformer a caller
/// has already activated, one carrying tags, one with no configuration member at all, and
/// two signing regions.
#[tokio::test]
async fn a_deleted_transformer_is_gone_from_every_reader() {
    let router = b2bi_router();
    let current = current_members();
    let legacy = legacy_members();

    // (case, signing region, the create request, whether to activate before deleting)
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "current style, inbound conversion and mapping",
            SIGNING_REGION,
            json!({
                "name": "deleted-current",
                "inputConversion": current[0].1,
                "mapping": current[1].1,
            }),
            false,
        ),
        (
            "every current configuration member",
            "eu-central-1",
            json!({
                "name": "deleted-current-full",
                "inputConversion": current[0].1,
                "mapping": current[1].1,
                "outputConversion": current[2].1,
                "sampleDocuments": current[3].1,
            }),
            false,
        ),
        (
            "legacy style",
            SIGNING_REGION,
            json!({
                "name": "deleted-legacy",
                "ediType": legacy[0].1,
                "fileFormat": legacy[1].1,
                "mappingTemplate": legacy[2].1,
                "sampleDocument": legacy[3].1,
            }),
            false,
        ),
        (
            "activated before the delete",
            SIGNING_REGION,
            json!({ "name": "deleted-active", "inputConversion": current[0].1 }),
            true,
        ),
        (
            "tagged",
            "ap-southeast-2",
            json!({
                "name": "deleted-tagged",
                "inputConversion": current[0].1,
                "tags": [{ "Key": "env", "Value": "test" }],
            }),
            false,
        ),
        (
            "no configuration member at all",
            SIGNING_REGION,
            json!({ "name": "deleted-bare" }),
            false,
        ),
    ];

    for (case, region, create, activate) in &cases {
        let transformer_id = created_transformer(&router, region, create.clone()).await;
        if *activate {
            let (status, activated) = call_signed_for(
                &router,
                region,
                "UpdateTransformer",
                json!({ "transformerId": transformer_id, "status": "active" }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: activating the transformer must succeed, got {status} {activated}"
            );
        }

        let (status, deleted) = call_signed_for(
            &router,
            region,
            "DeleteTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteTransformer must succeed, got {status} {deleted}"
        );
        // `DeleteTransformer`'s output shape is `Unit`, so the body carries no member.
        assert!(
            deleted.as_object().is_none_or(|members| members.is_empty()),
            "case {case:?}: the delete response must carry no member, got {deleted}"
        );

        let (status, fetched) = call_signed_for(
            &router,
            region,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status,
            NOT_FOUND_STATUS,
            "case {case:?}: GetTransformer after the delete must be {NOT_FOUND_STATUS}, got \
             {status} {fetched}"
        );
        assert_eq!(
            error_code(&fetched),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {fetched}"
        );
        assert!(
            text(&fetched, "message").contains(&transformer_id),
            "case {case:?}: the refusal must name the deleted id, got {:?}",
            text(&fetched, "message")
        );

        let (status, repeated) = call_signed_for(
            &router,
            region,
            "DeleteTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status,
            NOT_FOUND_STATUS,
            "case {case:?}: a repeated delete must be {NOT_FOUND_STATUS}, got {status} {repeated}"
        );
        assert_eq!(
            error_code(&repeated),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {repeated}"
        );
    }
}

/// RULE: a transformer a capability's `configuration` still names cannot be deleted — the
/// modelled `ConflictException` (HTTP 409, member `message`) with the service's fixed
/// text, the transformer left readable — and the delete is served once no capability
/// references it any more.
///
/// The rows differ in how many capabilities hold the reference (so dropping one of two is
/// still a conflict) and in the X12 configuration around it.
#[tokio::test]
async fn a_transformer_a_capability_uses_cannot_be_deleted() {
    let router = b2bi_router();

    // (case, how many capabilities reference the transformer, transaction set, version)
    // The enum values come from the modelled `X12TransactionSet` / `X12Version`.
    let cases: &[(&str, usize, &str, &str)] = &[
        ("one capability", 1, "X12_850", "VERSION_4010"),
        ("two capabilities", 2, "X12_110", "VERSION_5010"),
        ("three capabilities", 3, "X12_204", "VERSION_4030"),
    ];

    for (case, references, transaction_set, version) in cases {
        let transformer_id = given_transformer(&router, &format!("used-by-{references}")).await;
        let mut capability_ids = Vec::new();
        for index in 0..*references {
            capability_ids.push(
                capability_using(
                    &router,
                    &format!("cap-{references}-{index}"),
                    &transformer_id,
                    transaction_set,
                    version,
                )
                .await,
            );
        }

        // The references are dropped one at a time, and the delete is re-attempted before
        // each drop: while ANY reference remains it is refused with the same answer.
        for (dropped, capability_id) in capability_ids.iter().enumerate() {
            let (status, body) = call(
                &router,
                "DeleteTransformer",
                json!({ "transformerId": transformer_id }),
            )
            .await;
            assert_eq!(
                status, 409,
                "case {case:?} with {} reference(s) left: the delete must be refused, got \
                 {status} {body}",
                references - dropped
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {body}"
            );
            // `ConflictException`'s member is `message` —
            // `smithy_query(query="shape",shape="ConflictException")`.
            assert_eq!(
                text(&body, "message"),
                "Transformer config is currently in use by a Capability",
                "case {case:?}: unexpected refusal text, got {body}"
            );

            // The refused delete removed nothing: the transformer is still readable.
            let (status, fetched) = call(
                &router,
                "GetTransformer",
                json!({ "transformerId": transformer_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the refused delete must leave the transformer readable, got \
                 {status} {fetched}"
            );
            assert_eq!(
                text(&fetched, "transformerId"),
                transformer_id,
                "case {case:?}: the reader must report the same transformer, got {fetched}"
            );

            let (status, body) = call(
                &router,
                "DeleteCapability",
                json!({ "capabilityId": capability_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteCapability must succeed, got {status} {body}"
            );
        }

        // Every referencing capability is gone, so the same delete is served.
        let (status, body) = call(
            &router,
            "DeleteTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: once no capability references it the delete must succeed, got \
             {status} {body}"
        );
        let (status, fetched) = call(
            &router,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status,
            NOT_FOUND_STATUS,
            "case {case:?}: the deleted transformer must be gone, got {status} {fetched}"
        );
    }
}

/// RULE: `DeleteTransformer` for an id this account's store holds no transformer for is
/// the modelled `ResourceNotFoundException` at this service's not-found status, naming
/// that id — never an empty success — whatever the id looks like, including an id another
/// b2bi resource kind really does hold.
#[tokio::test]
async fn deleting_an_unknown_transformer_is_refused_as_not_found() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "delete-transformer-unknown").await;
    let kept = given_transformer(&router, "kept").await;

    // (case, a transformerId satisfying the modelled len 1..64 / ^[a-zA-Z0-9_-]+$)
    let cases: Vec<(&str, String)> = vec![
        ("a never-minted transformer id", "t-0123456789abcdef0".to_string()),
        ("a one-character id", "t".to_string()),
        ("an id of the maximum modelled length", "t".repeat(64)),
        ("an id with every permitted character class", "T-9_z".to_string()),
        ("a profile id that really exists", profile_id),
    ];

    for (case, transformer_id) in &cases {
        let (status, body) = call(
            &router,
            "DeleteTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status,
            NOT_FOUND_STATUS,
            "case {case:?}: expected {NOT_FOUND_STATUS} for {transformer_id:?}, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            text(&body, "message"),
            // The one wording this service reports an absent transformer with
            // (`crate::state::transformer_not_found`), built from the request's own id.
            format!("Transformer not found with ID {transformer_id}"),
            "case {case:?}: the refusal must name the id that was not found, got {:?}",
            text(&body, "message")
        );
    }

    // None of those refusals touched the transformer this store really holds.
    let (status, fetched) = call(
        &router,
        "GetTransformer",
        json!({ "transformerId": kept.clone() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the stored transformer must still be readable, got {status} {fetched}"
    );
    assert_eq!(
        text(&fetched, "transformerId"),
        kept,
        "the reader must report the stored transformer, got {fetched}"
    );
}
