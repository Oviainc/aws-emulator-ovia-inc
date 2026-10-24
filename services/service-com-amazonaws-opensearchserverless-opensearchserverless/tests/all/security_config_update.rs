// SPDX-License-Identifier: CC0-1.0

//! `UpdateSecurityConfig` end to end over the wire: the write it performs on a stored
//! security configuration, the members it leaves alone, and the preconditions it refuses.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: an update replaces exactly the members the request names — a whole
//! options structure for the types whose options the request restates, and only the named
//! attributes for the IAM Identity Center patch shape — mints a fresh `configVersion`,
//! stamps `lastModifiedDate`, leaves the configuration's identity and `createdDate` alone,
//! and is refused unless the request's `configVersion` is the stored one. The tables below
//! span what that rule generalises over: every `SecurityConfigType`, each options member
//! and each of its attributes, a request that names only a description, and the published
//! bounds of the members a request may carry.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, emulator, is_policy_version, text, CLIENT_ERROR};
use crate::security_configs::{create_config, list_all, saml_options, INSTANCE_ARN, SAML_METADATA};

/// A second IdP metadata document, so an update that restates `samlOptions` is observably
/// a different document from the created one. `samlMetadata` is printable text
/// (`@pattern("[\u{0009}\u{000A}\u{000D}\u{0020}-\u{007E}\u{00A1}-\u{00FF}]+")`).
const OTHER_SAML_METADATA: &str =
    r#"<EntityDescriptor entityID="https://other.example.com/metadata"></EntityDescriptor>"#;

/// A `PolicyVersion` of the modelled shape (`@length(min: 20, max: 36)`,
/// `^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$`) that this emulator
/// never minted, so it is a well-formed version naming no write.
const UNMINTED_VERSION: &str = "0123456789abcdef01234567";

/// Update the configuration `id` at `version` with the members `patch` names, answering the
/// call's status, error code and parsed body.
async fn update(
    router: &MultiServiceRouter,
    id: &str,
    version: &str,
    patch: &Value,
) -> (u16, String, Value) {
    let mut request = patch.clone();
    request["id"] = json!(id);
    request["configVersion"] = json!(version);
    call(router, "UpdateSecurityConfig", request).await
}

/// The `securityConfigDetail` `GetSecurityConfig` reports for `id`, asserting the read was
/// accepted.
async fn read_config(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, code, answer) = call(router, "GetSecurityConfig", json!({ "id": id })).await;
    assert_eq!(
        status, 200,
        "case GetSecurityConfig({id}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "securityConfigDetail").clone()
}

// ---------------------------------------------------------------------------
// The write: what an update replaces, and what it leaves at the stored value.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_keeps_the_rest() {
    let router = emulator();

    // Each row: the case, the create request, the members the update names, and the
    // members the stored record must then report differently from the created one.
    // `SecurityConfigType` = saml | iamidentitycenter | iamfederation (smithy_query shape
    // values), and every options member and attribute of each is exercised.
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            // Only a description: the identity-provider options stay exactly as created,
            // which is the partial-write half of the rule.
            "saml, description only",
            json!({
                "type": "saml",
                "name": "desc-only-saml",
                "description": "as created",
                "samlOptions": { "metadata": SAML_METADATA, "sessionTimeout": 120 },
            }),
            json!({ "description": "after the update" }),
            json!({ "description": "after the update" }),
        ),
        (
            // `samlOptions` is the WHOLE `SamlConfigOptions` structure on both the create
            // and the update, so the options the update states are the options the
            // configuration has: the created `userAttribute` is gone, not merged.
            "saml, samlOptions restated in full",
            json!({
                "type": "saml",
                "name": "full-restate-saml",
                "description": "kept",
                "samlOptions": {
                    "metadata": SAML_METADATA,
                    "userAttribute": "Email",
                    "sessionTimeout": 120,
                },
            }),
            json!({
                "samlOptions": {
                    "metadata": OTHER_SAML_METADATA,
                    "groupAttribute": "Group",
                    "sessionTimeout": 720,
                },
            }),
            json!({
                "samlOptions": {
                    "metadata": OTHER_SAML_METADATA,
                    "groupAttribute": "Group",
                    "sessionTimeout": 720,
                },
            }),
        ),
        (
            // `SamlConfigOptions$sessionTimeout`: "The session timeout, in minutes.
            // Default is 60 minutes." An update that states options without it stores the
            // documented default, exactly as a create does.
            "saml, samlOptions without sessionTimeout",
            json!({
                "type": "saml",
                "name": "default-timeout-saml",
                "samlOptions": { "metadata": SAML_METADATA, "sessionTimeout": 720 },
            }),
            json!({ "samlOptions": { "metadata": OTHER_SAML_METADATA } }),
            json!({ "samlOptions": { "metadata": OTHER_SAML_METADATA, "sessionTimeout": 60 } }),
        ),
        (
            // `sessionTimeout`'s published minimum (`@range(min: 5, max: 720)`).
            "saml, sessionTimeout at the published minimum",
            json!({
                "type": "saml",
                "name": "min-timeout-saml",
                "samlOptions": { "metadata": SAML_METADATA },
            }),
            json!({ "samlOptions": { "metadata": SAML_METADATA, "sessionTimeout": 5 } }),
            json!({ "samlOptions": { "metadata": SAML_METADATA, "sessionTimeout": 5 } }),
        ),
        (
            // `UpdateIamIdentityCenterConfigOptions` carries ONLY the two attributes, so it
            // patches them: `userAttribute` alone leaves the created `groupAttribute` and
            // the instance the configuration is integrated with in place.
            "iamidentitycenter, userAttribute alone",
            json!({
                "type": "iamidentitycenter",
                "name": "patch-user-idc",
                "iamIdentityCenterOptions": {
                    "instanceArn": INSTANCE_ARN,
                    "userAttribute": "UserId",
                    "groupAttribute": "GroupName",
                },
            }),
            json!({ "iamIdentityCenterOptionsUpdates": { "userAttribute": "Email" } }),
            json!({
                "iamIdentityCenterOptions": {
                    "instanceArn": INSTANCE_ARN,
                    "userAttribute": "Email",
                    "groupAttribute": "GroupName",
                },
            }),
        ),
        (
            // Both attributes at once, over the other values of
            // `IamIdentityCenterUserAttribute` (Email | UserId | UserName) and
            // `IamIdentityCenterGroupAttribute` (GroupId | GroupName).
            "iamidentitycenter, both attributes",
            json!({
                "type": "iamidentitycenter",
                "name": "patch-both-idc",
                "description": "kept",
                "iamIdentityCenterOptions": { "instanceArn": INSTANCE_ARN },
            }),
            json!({
                "iamIdentityCenterOptionsUpdates": {
                    "userAttribute": "UserName",
                    "groupAttribute": "GroupName",
                },
            }),
            json!({
                "iamIdentityCenterOptions": {
                    "instanceArn": INSTANCE_ARN,
                    "userAttribute": "UserName",
                    "groupAttribute": "GroupName",
                },
            }),
        ),
        (
            // An empty patch names no attribute, so it changes none of them — only the
            // server-derived members move.
            "iamidentitycenter, empty patch",
            json!({
                "type": "iamidentitycenter",
                "name": "empty-patch-idc",
                "iamIdentityCenterOptions": { "instanceArn": INSTANCE_ARN },
            }),
            json!({ "iamIdentityCenterOptionsUpdates": {} }),
            json!({}),
        ),
        (
            // `iamFederationOptions` is the whole structure on both calls, so restating
            // only `groupAttribute` drops the created `userAttribute`.
            "iamfederation, options restated in full",
            json!({
                "type": "iamfederation",
                "name": "restate-federation",
                "iamFederationOptions": { "userAttribute": "Email", "groupAttribute": "Team" },
            }),
            json!({ "iamFederationOptions": { "groupAttribute": "Department" } }),
            json!({ "iamFederationOptions": { "groupAttribute": "Department" } }),
        ),
        (
            "iamfederation, description and options together",
            json!({
                "type": "iamfederation",
                "name": "both-federation",
                "description": "as created",
                "iamFederationOptions": { "userAttribute": "Email" },
            }),
            json!({
                "description": "after the update",
                "iamFederationOptions": { "userAttribute": "Login", "groupAttribute": "Team" },
            }),
            json!({
                "description": "after the update",
                "iamFederationOptions": { "userAttribute": "Login", "groupAttribute": "Team" },
            }),
        ),
    ];

    for (case, create, patch, overrides) in &cases {
        let created = create_config(&router, create.clone()).await;
        let id = text(&created, "id");
        let created_version = text(&created, "configVersion");

        let (status, code, answer) = update(&router, &id, &created_version, patch).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {code} with {answer}"
        );
        let detail = at(&answer, "securityConfigDetail").clone();

        // Every write mints a fresh `configVersion` of the modelled shape, so a caller can
        // tell one write from the next.
        let version = text(&detail, "configVersion");
        assert!(
            is_policy_version(&version),
            "case {case}: configVersion must match PolicyVersion's length and pattern, got {version:?}"
        );
        assert_ne!(
            version, created_version,
            "case {case}: an update mints a fresh configVersion, got {detail}"
        );

        // The timestamps: `createdDate` is the create's and `lastModifiedDate` is this
        // write's, so it is at or after the creation instant.
        let created_date = at(&created, "createdDate").as_i64().unwrap_or_default();
        assert_eq!(
            at(&detail, "createdDate").as_i64(),
            Some(created_date),
            "case {case}: an update does not move createdDate, got {detail}"
        );
        let modified = at(&detail, "lastModifiedDate").as_i64().unwrap_or_default();
        assert!(
            modified >= created_date,
            "case {case}: lastModifiedDate must be the write instant, got {modified} against createdDate {created_date}"
        );

        // Everything else is the created record with the named members replaced — which
        // pins both halves of the rule in one assertion.
        let mut expected = created.clone();
        for (member, value) in overrides.as_object().expect("the overrides are an object") {
            expected[member.as_str()] = value.clone();
        }
        expected["configVersion"] = json!(version);
        expected["lastModifiedDate"] = json!(modified);
        assert_eq!(
            detail, expected,
            "case {case}: the update must replace only the members it named, got {detail}"
        );

        // The read-back: the update reported exactly what it stored.
        assert_eq!(
            read_config(&router, &id).await,
            detail,
            "case {case}: GetSecurityConfig must answer the updated record"
        );
        let kind = text(&detail, "type");
        let summary = list_all(&router, &kind)
            .await
            .into_iter()
            .find(|summary| text(summary, "id") == id)
            .unwrap_or_else(|| panic!("case {case}: {id} must still be listed"));
        for member in ["configVersion", "description", "createdDate", "lastModifiedDate"] {
            assert_eq!(
                at(&summary, member),
                at(&detail, member),
                "case {case}: the listing's summary.{member} must be the updated record's, got {summary}"
            );
        }
    }

    // Repeated updates each move the version on, and the version an update answers is the
    // one the NEXT update must carry — the chain a caller follows.
    let created = create_config(
        &router,
        json!({ "type": "saml", "name": "chained-saml", "samlOptions": saml_options() }),
    )
    .await;
    let id = text(&created, "id");
    let mut version = text(&created, "configVersion");
    let mut seen = vec![version.clone()];
    for round in 0..3 {
        let patch = json!({ "description": format!("round {round}") });
        let (status, code, answer) = update(&router, &id, &version, &patch).await;
        assert_eq!(
            status, 200,
            "case chained update (round {round}): expected 200, got {status} {code} with {answer}"
        );
        let detail = at(&answer, "securityConfigDetail");
        assert_eq!(
            text(detail, "description"),
            format!("round {round}"),
            "case chained update (round {round}): the description must be the one just written, got {detail}"
        );
        version = text(detail, "configVersion");
        assert!(
            !seen.contains(&version),
            "case chained update (round {round}): every write mints an unseen configVersion, got {version:?} after {seen:?}"
        );
        seen.push(version.clone());
    }
}

// ---------------------------------------------------------------------------
// The preconditions: the stale version, the missing record, the options of another
// type, and the bounds the request's own members carry.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_is_refused_unless_its_version_and_options_fit_the_stored_config() {
    let router = emulator();

    // One configuration of each type, so each refusal below is exercised against the whole
    // value list of `SecurityConfigType` rather than one recorded type.
    let saml = create_config(
        &router,
        json!({ "type": "saml", "name": "locked-saml", "samlOptions": saml_options() }),
    )
    .await;
    let idc = create_config(
        &router,
        json!({
            "type": "iamidentitycenter",
            "name": "locked-idc",
            "iamIdentityCenterOptions": { "instanceArn": INSTANCE_ARN },
        }),
    )
    .await;
    let federation = create_config(
        &router,
        json!({
            "type": "iamfederation",
            "name": "locked-federation",
            "iamFederationOptions": { "userAttribute": "Email" },
        }),
    )
    .await;

    // The optimistic lock: the first update succeeds, and the version it consumed is stale
    // afterwards, so replaying it is the conflict and the stored record is untouched.
    let id = text(&saml, "id");
    let stale = text(&saml, "configVersion");
    let (status, code, answer) =
        update(&router, &id, &stale, &json!({ "description": "the winner" })).await;
    assert_eq!(
        status, 200,
        "case first update: expected 200, got {status} {code} with {answer}"
    );
    let after_first = at(&answer, "securityConfigDetail").clone();

    for (case, version) in [
        ("the version the first update consumed", stale.as_str()),
        ("a version this service never minted", UNMINTED_VERSION),
    ] {
        let (status, code, answer) =
            update(&router, &id, version, &json!({ "description": "the loser" })).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ConflictException"),
            "case update with {case}: expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
        );
        assert_eq!(
            read_config(&router, &id).await,
            after_first,
            "case update with {case}: a refused update writes nothing"
        );
    }

    // An id that names no configuration is the modelled 404 — including the id of a
    // configuration that has been deleted.
    let doomed = create_config(
        &router,
        json!({ "type": "saml", "name": "doomed-saml", "samlOptions": saml_options() }),
    )
    .await;
    let doomed_id = text(&doomed, "id");
    let doomed_version = text(&doomed, "configVersion");
    let (status, code, answer) =
        call(&router, "DeleteSecurityConfig", json!({ "id": doomed_id.clone() })).await;
    assert_eq!(
        status, 200,
        "case DeleteSecurityConfig(doomed-saml): expected 200, got {status} {code} with {answer}"
    );
    for (case, id, version) in [
        (
            "an id of no configuration",
            format!("saml/{}/never-created", engine::emulator_account_id()),
            UNMINTED_VERSION.to_string(),
        ),
        (
            "the id of a deleted configuration",
            doomed_id.clone(),
            doomed_version.clone(),
        ),
    ] {
        let (status, code, answer) =
            update(&router, &id, &version, &json!({ "description": "nowhere" })).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case update {case}: expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
        );
    }

    // The options of another identity-provider type describe a provider the addressed
    // configuration does not have, so each pairing of a stored type with another type's
    // options member is refused — and the stored record is left alone.
    let current = read_config(&router, &id).await;
    let other_options: Vec<(&str, &Value, Value)> = vec![
        (
            "a saml config given iamFederationOptions",
            &saml,
            json!({ "iamFederationOptions": { "userAttribute": "Email" } }),
        ),
        (
            "a saml config given iamIdentityCenterOptionsUpdates",
            &saml,
            json!({ "iamIdentityCenterOptionsUpdates": { "userAttribute": "Email" } }),
        ),
        (
            "an iamidentitycenter config given samlOptions",
            &idc,
            json!({ "samlOptions": { "metadata": SAML_METADATA } }),
        ),
        (
            "an iamidentitycenter config given iamFederationOptions",
            &idc,
            json!({ "iamFederationOptions": { "userAttribute": "Email" } }),
        ),
        (
            "an iamfederation config given samlOptions",
            &federation,
            json!({ "samlOptions": { "metadata": SAML_METADATA } }),
        ),
        (
            "an iamfederation config given iamIdentityCenterOptionsUpdates",
            &federation,
            json!({ "iamIdentityCenterOptionsUpdates": { "groupAttribute": "GroupId" } }),
        ),
    ];
    for (case, target, patch) in other_options {
        let target_id = text(target, "id");
        let stored = read_config(&router, &target_id).await;
        let version = text(&stored, "configVersion");
        let (status, code, answer) = update(&router, &target_id, &version, &patch).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case update, {case}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
        assert_eq!(
            read_config(&router, &target_id).await,
            stored,
            "case update, {case}: a refused update writes nothing"
        );
    }

    // What the request's own members are bounded by: the enums the boundary validator does
    // not narrow, the published bounds of `sessionTimeout` and `configVersion`, the
    // required members, and `samlOptions`' own required `metadata`.
    let saml_version = text(&current, "configVersion");
    let refusals: Vec<(&str, Value)> = vec![
        (
            "userAttribute outside IamIdentityCenterUserAttribute",
            json!({
                "id": text(&idc, "id"),
                "configVersion": text(&idc, "configVersion"),
                "iamIdentityCenterOptionsUpdates": { "userAttribute": "Login" },
            }),
        ),
        (
            "groupAttribute outside IamIdentityCenterGroupAttribute",
            json!({
                "id": text(&idc, "id"),
                "configVersion": text(&idc, "configVersion"),
                "iamIdentityCenterOptionsUpdates": { "groupAttribute": "Team" },
            }),
        ),
        (
            "samlOptions without metadata",
            json!({
                "id": id.clone(),
                "configVersion": saml_version.clone(),
                "samlOptions": { "userAttribute": "Email" },
            }),
        ),
        (
            "sessionTimeout one short of the 5-minute minimum",
            json!({
                "id": id.clone(),
                "configVersion": saml_version.clone(),
                "samlOptions": { "metadata": SAML_METADATA, "sessionTimeout": 4 },
            }),
        ),
        (
            "sessionTimeout one past the 720-minute maximum",
            json!({
                "id": id.clone(),
                "configVersion": saml_version.clone(),
                "samlOptions": { "metadata": SAML_METADATA, "sessionTimeout": 721 },
            }),
        ),
        (
            "configVersion one short of PolicyVersion's 20-character minimum",
            json!({ "id": id.clone(), "configVersion": "a".repeat(19) }),
        ),
        (
            "configVersion one past PolicyVersion's 36-character maximum",
            json!({ "id": id.clone(), "configVersion": "a".repeat(37) }),
        ),
        (
            "configVersion outside PolicyVersion's pattern",
            json!({ "id": id.clone(), "configVersion": "not a policy version!!!!" }),
        ),
        ("configVersion absent", json!({ "id": id.clone() })),
        (
            "id absent",
            json!({ "configVersion": saml_version.clone() }),
        ),
        (
            "id past SecurityConfigId's 100-character maximum",
            json!({ "id": "a".repeat(101), "configVersion": saml_version.clone() }),
        ),
        (
            "description past ConfigDescription's 1000-character maximum",
            json!({
                "id": id.clone(),
                "configVersion": saml_version.clone(),
                "description": "d".repeat(1001),
            }),
        ),
    ];
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "UpdateSecurityConfig", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case UpdateSecurityConfig({case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }

    // None of the refusals wrote: every configuration is still at the version it was.
    for (case, stored) in [
        ("locked-saml", &current),
        ("locked-idc", &idc),
        ("locked-federation", &federation),
    ] {
        let stored_id = text(stored, "id");
        assert_eq!(
            read_config(&router, &stored_id).await,
            *stored,
            "case {case}: a refused update leaves the stored configuration alone"
        );
    }
}

// ---------------------------------------------------------------------------
// `clientToken`: `UpdateSecurityConfig` is `@idempotent` with an `@idempotencyToken`
// member, so a retry of one write is one write.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_answers_the_write_it_already_performed() {
    let router = emulator();

    let created = create_config(
        &router,
        json!({ "type": "saml", "name": "token-update-saml", "samlOptions": saml_options() }),
    )
    .await;
    let id = text(&created, "id");
    let version = text(&created, "configVersion");

    let patch = json!({ "description": "written once", "clientToken": "update-token" });
    let (status, code, answer) = update(&router, &id, &version, &patch).await;
    assert_eq!(
        status, 200,
        "case first update with a clientToken: expected 200, got {status} {code} with {answer}"
    );
    let first = at(&answer, "securityConfigDetail").clone();

    // The retry carries the version the FIRST call consumed, which is stale now: the token
    // is what makes it one write rather than a conflict, and it mints no second version.
    let (status, code, answer) = update(&router, &id, &version, &patch).await;
    assert_eq!(
        status, 200,
        "case replayed clientToken: expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        at(&answer, "securityConfigDetail"),
        &first,
        "case replayed clientToken: the retry must answer the record the first write stored"
    );
    assert_eq!(
        read_config(&router, &id).await,
        first,
        "case replayed clientToken: the retry writes nothing"
    );

    // A different token is a different write, so it is held to the version precondition
    // like any other: the stale version conflicts and the current one succeeds.
    let (status, code, answer) = update(
        &router,
        &id,
        &version,
        &json!({ "description": "second writer", "clientToken": "other-token" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ConflictException"),
        "case fresh token, stale version: expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
    );
    let current = text(&first, "configVersion");
    let (status, code, answer) = update(
        &router,
        &id,
        &current,
        &json!({ "description": "second writer", "clientToken": "other-token" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case fresh token, current version: expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        text(at(&answer, "securityConfigDetail"), "description"),
        "second writer",
        "case fresh token, current version: the write must take effect, got {answer}"
    );
}
