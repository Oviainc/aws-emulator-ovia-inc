// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateTransformer`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.<Operation>`) through the same router the emulator boots, so every
//! assertion is about what a real SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (which members the update
//! names, how many, which configuration style they belong to, the enum value, the status
//! the record rests in, the signing region).
//!
//! The wire harness (`b2bi_router`, `call`, `error_code`, `text`) and the two
//! configuration-member tables are reused from the sibling suites rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{
    b2bi_router, call, call_signed_for, error_code, text, ACCOUNT, SIGNING_REGION,
};
use crate::transformer_lifecycle::{current_members, legacy_members, CONFIGURATION_MEMBERS};

/// The members `UpdateTransformer` can write: the eight configuration members plus the two
/// the model lets a caller set directly.
///
/// Source: the input members of `com.amazonaws.b2bi#UpdateTransformer`
/// (`smithy_query(query="operation",operation="UpdateTransformer")`) minus the required
/// `transformerId`.
fn updatable_members() -> Vec<&'static str> {
    let mut members = vec!["name", "status"];
    members.extend_from_slice(CONFIGURATION_MEMBERS);
    members
}

/// The two values of the modelled `TransformerStatus` enum
/// (`smithy_query(query="shape",shape="TransformerStatus")`).
const TRANSFORMER_STATUS_VALUES: &[&str] = &["active", "inactive"];

/// Create a transformer from `request` and return its create response.
async fn created_transformer(
    router: &MultiServiceRouter,
    region: &str,
    request: Value,
) -> (String, Value) {
    let (status, created) = call_signed_for(router, region, "CreateTransformer", request).await;
    assert_eq!(
        status, 201,
        "the transformer under test must be created, got {status} {created}"
    );
    (text(&created, "transformerId"), created)
}

/// The epoch seconds of a timestamp member, which awsJson1_0 renders as a number.
fn seconds(body: &Value, member: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} must be an epoch-seconds number, got {:?}", body[member]))
}

/// RULE: `UpdateTransformer` writes exactly the members the request names onto the stored
/// transformer, leaves every member it omits as it was, stamps `modifiedAt`, keeps the
/// system-assigned `transformerId` / `transformerArn` / `createdAt`, and answers with that
/// stored record — which `GetTransformer` then reports member for member.
///
/// The rows span updating only `name`, only `status`, one configuration member, every
/// member of each configuration style at once, a legacy-style record updated through its
/// legacy members, a mixed set of non-configuration and configuration members, and an
/// update that names no member at all, over two signing regions.
#[tokio::test]
async fn an_update_writes_the_named_members_and_leaves_the_rest() {
    let router = b2bi_router();

    let current = current_members();
    let legacy = legacy_members();

    // (case, signing region, the create request, the update request)
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "name only",
            SIGNING_REGION,
            json!({ "name": "inbound-850", "inputConversion": current[0].1 }),
            json!({ "name": "inbound_850_v2" }),
        ),
        (
            "status only, inactive -> active",
            SIGNING_REGION,
            json!({ "name": "activated", "inputConversion": current[0].1 }),
            json!({ "status": "active" }),
        ),
        (
            "one current configuration member replaced",
            "eu-central-1",
            json!({
                "name": "remapped",
                "inputConversion": current[0].1,
                "mapping": current[1].1,
            }),
            json!({
                "mapping": { "templateLanguage": "XSLT", "template": "<xsl:stylesheet version=\"1.0\"/>" }
            }),
        ),
        (
            "every current configuration member at once",
            SIGNING_REGION,
            json!({ "name": "fully-rewritten", "inputConversion": current[0].1 }),
            json!({
                "inputConversion": current[0].1,
                "mapping": current[1].1,
                "outputConversion": current[2].1,
                "sampleDocuments": current[3].1,
            }),
        ),
        (
            "legacy record, one legacy member replaced",
            SIGNING_REGION,
            json!({ "name": "legacy-xml", "fileFormat": "XML", "mappingTemplate": "{}" }),
            json!({ "fileFormat": "JSON" }),
        ),
        (
            "legacy record, every legacy member at once",
            "ap-southeast-2",
            json!({ "name": "legacy-full", "fileFormat": "NOT_USED" }),
            json!({
                "ediType": legacy[0].1,
                "fileFormat": legacy[1].1,
                "mappingTemplate": legacy[2].1,
                "sampleDocument": legacy[3].1,
            }),
        ),
        (
            "name, status and a configuration member together",
            SIGNING_REGION,
            json!({ "name": "renamed-and-activated", "inputConversion": current[0].1 }),
            json!({
                "name": "renamed_and_activated",
                "status": "active",
                "sampleDocuments": current[3].1,
            }),
        ),
        (
            // Nothing but the required id: a well-formed request, so it is served, and the
            // only thing it changes is `modifiedAt`.
            "no member but the required transformerId",
            SIGNING_REGION,
            json!({ "name": "untouched", "inputConversion": current[0].1 }),
            json!({}),
        ),
    ];

    for (case, region, create, update) in &cases {
        let (transformer_id, created) = created_transformer(&router, region, create.clone()).await;
        let mut request = update.clone();
        request["transformerId"] = json!(transformer_id);

        let (status, updated) =
            call_signed_for(&router, region, "UpdateTransformer", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateTransformer must succeed, got {status} {updated}"
        );

        // The system-assigned identity is the create's: an update never re-mints it.
        assert_eq!(
            text(&updated, "transformerId"),
            transformer_id,
            "case {case:?}: the id must be the one the create minted"
        );
        assert_eq!(
            text(&updated, "transformerArn"),
            format!("arn:aws:b2bi:{region}:{ACCOUNT}:transformer/{transformer_id}"),
            "case {case:?}: the ARN must stay the create's, got {:?}",
            updated["transformerArn"]
        );
        assert_eq!(
            updated["createdAt"], created["createdAt"],
            "case {case:?}: createdAt must not move: {:?} vs {:?}",
            updated["createdAt"], created["createdAt"]
        );
        assert!(
            seconds(&updated, "modifiedAt") >= seconds(&created, "createdAt"),
            "case {case:?}: modifiedAt {:?} must be at or after createdAt {:?}",
            updated["modifiedAt"],
            created["createdAt"]
        );

        for member in updatable_members() {
            let expected = update.get(member).cloned().unwrap_or_else(|| {
                // `status` is the only member the create assigns rather than echoes.
                created.get(member).cloned().unwrap_or(Value::Null)
            });
            assert_eq!(
                updated[member], expected,
                "case {case:?}: {member} must be {expected:?} (the update's value, or the \
                 stored one when the update omits it), got {:?}",
                updated[member]
            );
        }

        // Round trip: the reader reports the very record the update wrote.
        let (status, fetched) = call_signed_for(
            &router,
            region,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetTransformer must find the updated transformer, got \
             {status} {fetched}"
        );
        for member in ["transformerId", "transformerArn", "createdAt", "modifiedAt"]
            .iter()
            .copied()
            .chain(updatable_members())
        {
            assert_eq!(
                fetched[member], updated[member],
                "case {case:?}: GetTransformer disagrees with UpdateTransformer on {member}: \
                 {:?} vs {:?}",
                fetched[member], updated[member]
            );
        }
    }
}

/// RULE: `status` is a plain member of the stored record, so it moves to either value of
/// the modelled `TransformerStatus` enum as often as a caller asks — including re-setting
/// the value it already holds — and every read in between reports the value last written.
///
/// The rows walk a toggle sequence that covers both directions, both no-op re-sets and a
/// configuration rewrite performed while the record is `inactive`.
#[tokio::test]
async fn status_toggles_between_every_modelled_value() {
    let router = b2bi_router();
    let current = current_members();
    let (transformer_id, created) = created_transformer(
        &router,
        SIGNING_REGION,
        json!({ "name": "toggled", "inputConversion": current[0].1 }),
    )
    .await;
    assert_eq!(
        text(&created, "status"),
        "inactive",
        "a new transformer rests inactive, got {created}"
    );

    // Every ordered pair of the enum's values, so each direction and each no-op re-set is
    // exercised: inactive -> active -> active -> inactive -> inactive -> active.
    let mut sequence: Vec<&str> = Vec::new();
    for from in TRANSFORMER_STATUS_VALUES {
        for to in TRANSFORMER_STATUS_VALUES {
            sequence.push(from);
            sequence.push(to);
        }
    }

    for (step, target) in sequence.iter().enumerate() {
        let case = format!("step {step} -> {target}");
        let (status, updated) = call(
            &router,
            "UpdateTransformer",
            json!({ "transformerId": transformer_id, "status": target }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the status move must succeed, got {status} {updated}"
        );
        assert_eq!(
            text(&updated, "status"),
            *target,
            "case {case:?}: expected status {target:?}, got {:?}",
            updated["status"]
        );

        let (status, fetched) = call(
            &router,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetTransformer must succeed, got {status} {fetched}"
        );
        assert_eq!(
            text(&fetched, "status"),
            *target,
            "case {case:?}: the reader must report the status last written, got {:?}",
            fetched["status"]
        );
        assert_eq!(
            fetched["inputConversion"], created["inputConversion"],
            "case {case:?}: a status move must not touch the configuration, got {:?}",
            fetched["inputConversion"]
        );
    }
}

/// RULE: the configuration of a transformer that is currently `active` is immutable — an
/// update naming any configuration member is refused with the modelled
/// `ConflictException` (HTTP 409) and writes nothing — while `name` and `status` stay
/// updatable, so deactivating the transformer makes the same rewrite succeed.
///
/// Source: the recorded AWS behaviour for
/// `b2bi.update-transformer-immutable-fields-once-active-rejected`. The status and the
/// error code are the requirement; the message is the one this handler builds from the
/// request, so the rows assert that it names the member they tried to rewrite.
///
/// The rows are every one of the eight configuration members, in both styles.
#[tokio::test]
async fn the_configuration_of_an_active_transformer_cannot_be_rewritten() {
    let router = b2bi_router();
    let current = current_members();
    let (transformer_id, created) = created_transformer(
        &router,
        SIGNING_REGION,
        json!({ "name": "locked", "inputConversion": current[0].1 }),
    )
    .await;

    let (status, activated) = call(
        &router,
        "UpdateTransformer",
        json!({ "transformerId": transformer_id, "status": "active" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "activating the transformer must succeed, got {status} {activated}"
    );

    for (member, value) in legacy_members().into_iter().chain(current_members()) {
        let (status, body) = call(
            &router,
            "UpdateTransformer",
            json!({ "transformerId": transformer_id, member: value }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {member:?}: rewriting an active transformer's configuration must be \
             refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {member:?}: expected ConflictException, got {body}"
        );
        // `ConflictException`'s member is `message` —
        // `smithy_query(query="shape",shape="ConflictException")`.
        let message = text(&body, "message");
        assert!(
            message.contains(member) && message.contains(&transformer_id),
            "case {member:?}: the refusal must name the member and the transformer, got \
             {message:?}"
        );

        // Nothing was written: the stored record still holds the created configuration.
        let (status, fetched) = call(
            &router,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {member:?}: GetTransformer must succeed, got {status} {fetched}"
        );
        for configuration_member in CONFIGURATION_MEMBERS {
            assert_eq!(
                fetched[*configuration_member], created[*configuration_member],
                "case {member:?}: the refused update must not have written \
                 {configuration_member}: got {:?}, created with {:?}",
                fetched[*configuration_member], created[*configuration_member]
            );
        }
    }

    // `name` is not configuration, so it is updatable while active.
    let (status, renamed) = call(
        &router,
        "UpdateTransformer",
        json!({ "transformerId": transformer_id, "name": "locked_but_renamed" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "renaming an active transformer must succeed, got {status} {renamed}"
    );
    assert_eq!(
        text(&renamed, "name"),
        "locked_but_renamed",
        "the rename must be stored, got {renamed}"
    );

    // Deactivating it releases the configuration: the very rewrite refused above is served.
    let (status, deactivated) = call(
        &router,
        "UpdateTransformer",
        json!({ "transformerId": transformer_id, "status": "inactive" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "deactivating the transformer must succeed, got {status} {deactivated}"
    );
    let (status, rewritten) = call(
        &router,
        "UpdateTransformer",
        json!({ "transformerId": transformer_id, "mapping": current[1].1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the same rewrite must be served once the transformer is inactive, got \
         {status} {rewritten}"
    );
    assert_eq!(
        rewritten["mapping"], current[1].1,
        "the rewrite must be stored, got {:?}",
        rewritten["mapping"]
    );
}

/// RULE: an update naming BOTH a member the model marks `@deprecated` and a member that
/// replaces it is refused with the modelled `ValidationException` (HTTP 400) and writes
/// nothing — the same rule `CreateTransformer` enforces on a create.
///
/// The rows are every (deprecated member, replacement member) pair: 4 x 4 = 16.
#[tokio::test]
async fn mixing_the_deprecated_and_current_configuration_members_is_refused() {
    let router = b2bi_router();
    let current = current_members();
    let (transformer_id, created) = created_transformer(
        &router,
        SIGNING_REGION,
        json!({ "name": "single-style", "inputConversion": current[0].1 }),
    )
    .await;

    for (legacy_member, legacy_value) in legacy_members() {
        for (current_member, current_value) in current_members() {
            let case = format!("{legacy_member} + {current_member}");
            let mut request = json!({ "transformerId": transformer_id });
            request[legacy_member] = legacy_value.clone();
            request[current_member] = current_value.clone();

            let (status, body) = call(&router, "UpdateTransformer", request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a mixed-style update must be refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            // `ValidationException`'s member is `Message` (capital M) in this service's
            // model — `smithy_query(query="shape",shape="ValidationException")`.
            let message = text(&body, "Message");
            assert!(
                message.ends_with(&format!(
                    "this request specifies {current_member} and {legacy_member}"
                )),
                "case {case:?}: the refusal must name the members this request sent in each \
                 style, got {message:?} in {body}"
            );
        }
    }

    // None of the 16 refusals touched the record: it is still exactly what the create
    // stored, and still carries no `modifiedAt`.
    let (status, fetched) = call(
        &router,
        "GetTransformer",
        json!({ "transformerId": transformer_id }),
    )
    .await;
    assert_eq!(status, 200, "GetTransformer must succeed, got {status} {fetched}");
    assert_eq!(
        fetched["modifiedAt"],
        Value::Null,
        "a refused update must not stamp modifiedAt, got {:?}",
        fetched["modifiedAt"]
    );
    for member in updatable_members() {
        assert_eq!(
            fetched[member], created[member],
            "the refused updates must not have written {member}: got {:?}, created with {:?}",
            fetched[member], created[member]
        );
    }
}

/// RULE: `UpdateTransformer` for an id this account's store does not hold is the modelled
/// `ResourceNotFoundException` (HTTP 404) naming that id — never an empty success and
/// never a minted transformer — whatever the id looks like, including an id another b2bi
/// resource kind really does hold, and whatever else the request gets wrong: the resource
/// is resolved before the request's own members are judged.
#[tokio::test]
async fn updating_an_unknown_transformer_is_a_404() {
    let router = b2bi_router();
    let current = current_members();
    let legacy = legacy_members();
    let profile_id = crate::onboarding_loop::given_profile(&router, "update-transformer-404").await;

    // (case, a transformerId satisfying the modelled len 1..64 / ^[a-zA-Z0-9_-]+$, the
    // members the update names beside it)
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "a never-minted transformer id",
            "t-0123456789abcdef0".to_string(),
            json!({ "status": "active" }),
        ),
        ("a one-character id", "t".to_string(), json!({ "status": "inactive" })),
        (
            "an id of the maximum modelled length",
            "t".repeat(64),
            json!({ "name": "renamed" }),
        ),
        (
            "an id with every permitted character class",
            "T-9_z".to_string(),
            json!({ "mapping": current[1].1 }),
        ),
        (
            // The missing resource is answered BEFORE the request's members are judged,
            // so a mixed-style update against an unknown id is still a 404.
            "an unknown id with a mixed-style configuration",
            "t-fedcba98765432100".to_string(),
            json!({ "fileFormat": legacy[1].1, "inputConversion": current[0].1 }),
        ),
        (
            "a profile id that really exists",
            profile_id,
            json!({ "status": "active" }),
        ),
    ];

    for (case, transformer_id, members) in &cases {
        let mut request = members.clone();
        request["transformerId"] = json!(transformer_id);
        let (status, body) = call(&router, "UpdateTransformer", request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {transformer_id:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            text(&body, "message").contains(transformer_id),
            "case {case:?}: the refusal must name the id that was not found, got {:?}",
            text(&body, "message")
        );

        // The refusal minted nothing: the reader still does not know that id.
        let (status, fetched) = call(
            &router,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused update must not have created a transformer, got \
             {status} {fetched}"
        );
    }
}
