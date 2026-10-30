// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdatePartnership`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.<Operation>`) through the same router the emulator boots, so every
//! assertion is about what a real SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (where in the list the
//! unresolvable reference sits, how many there are, what form the id takes, the size and
//! the ordering of the list that replaces the stored one, which members the update names).
//!
//! The wire harness (`b2bi_router`, `call`, `error_code`, `text`) and the `given_*`
//! fixtures are reused from the sibling suites rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{
    b2bi_router, call, error_code, given_capability, given_partnership, given_profile,
    given_transformer, text, ACCOUNT, SIGNING_REGION,
};

/// The `ResourceNotFoundException` message AWS reports for a `capabilityId` that resolves
/// to no capability: fixed text around the offending id, taken from the request's own
/// `capabilities` list.
fn capability_not_found(capability_id: &str) -> String {
    format!("Capability ID {capability_id} not found.")
}

/// Read a partnership back through `GetPartnership`.
async fn get_partnership(router: &MultiServiceRouter, partnership_id: &str) -> Value {
    let (status, fetched) = call(
        router,
        "GetPartnership",
        json!({ "partnershipId": partnership_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetPartnership({partnership_id}) must succeed, got {status} {fetched}"
    );
    fetched
}

/// RULE: every id in an update's reference list is resolved against the service's stored
/// resources BEFORE any member is written, so an update naming an id that resolves to
/// nothing is refused whole with the modelled `ResourceNotFoundException` (HTTP 404)
/// naming the FIRST such id, and the stored record is left exactly as it was — no partial
/// write of the ids that did resolve, and no `modifiedAt` stamp.
///
/// The rows span the position of the unresolvable id in the list (only, first, middle,
/// last), the number of unresolvable ids (one, two — the first is the one named), the list
/// size, and the form of the id: a never-minted id, a capability that existed and was
/// deleted, an id another b2bi resource kind really does hold (a profile and a
/// transformer), and ids at both ends of the modelled `CapabilityId` length (`len 1..64`,
/// `^[a-zA-Z0-9_-]+$` — `smithy_query(query="shape",shape="PartnershipCapabilities")`).
#[tokio::test]
async fn an_update_naming_an_unresolvable_capability_is_refused_and_writes_nothing() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "acme").await;
    let bound = given_capability(&router, "edi-850").await;
    let other = given_capability(&router, "edi-855").await;
    let partnership_id = given_partnership(&router, &profile_id, &bound, "Partner One").await;

    // A capability that really existed and was then deleted: its id resolves to nothing
    // from the delete on, so an update naming it is refused exactly like a never-minted id.
    let deleted = given_capability(&router, "edi-860").await;
    let (status, body) = call(
        &router,
        "DeleteCapability",
        json!({ "capabilityId": deleted }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the capability under test must be deletable, got {status} {body}"
    );

    // Ids of other b2bi resource kinds: they exist, but not as capabilities.
    let transformer_id = given_transformer(&router, "update-partnership-404").await;

    // (case, the submitted `capabilities` list, the id the refusal must name)
    let cases: Vec<(&str, Vec<String>, String)> = vec![
        (
            "the only element is unknown",
            vec!["c-0123456789abcdef0".to_string()],
            "c-0123456789abcdef0".to_string(),
        ),
        (
            "unknown first, existing second",
            vec!["c-ffffffffffffffff0".to_string(), bound.clone()],
            "c-ffffffffffffffff0".to_string(),
        ),
        (
            "existing first, unknown last",
            vec![bound.clone(), "cap-doesnotexist0000".to_string()],
            "cap-doesnotexist0000".to_string(),
        ),
        (
            "unknown in the middle of three",
            vec![bound.clone(), "c-1111111111111111a".to_string(), other.clone()],
            "c-1111111111111111a".to_string(),
        ),
        (
            "two unknown ids: the first in list order is named",
            vec![
                bound.clone(),
                "c-2222222222222222b".to_string(),
                "c-3333333333333333c".to_string(),
            ],
            "c-2222222222222222b".to_string(),
        ),
        (
            "a capability that was deleted",
            vec![deleted.clone(), bound.clone()],
            deleted.clone(),
        ),
        (
            "a profile id that really exists",
            vec![profile_id.clone()],
            profile_id.clone(),
        ),
        (
            "a transformer id that really exists",
            vec![bound.clone(), transformer_id.clone()],
            transformer_id.clone(),
        ),
        (
            "a one-character id, the modelled minimum length",
            vec!["c".to_string()],
            "c".to_string(),
        ),
        (
            "an id of the modelled maximum length",
            vec!["c".repeat(64)],
            "c".repeat(64),
        ),
    ];

    let created = get_partnership(&router, &partnership_id).await;

    for (case, capabilities, offender) in &cases {
        let (status, body) = call(
            &router,
            "UpdatePartnership",
            json!({
                "partnershipId": partnership_id.clone(),
                "name": format!("renamed for {case}"),
                "capabilities": capabilities.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: an unresolvable capability must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        // `ResourceNotFoundException`'s member is `message` —
        // `smithy_query(query="shape",shape="ResourceNotFoundException")`.
        assert_eq!(
            text(&body, "message"),
            capability_not_found(offender),
            "case {case:?}: expected the refusal to name {offender:?}, got {:?}",
            text(&body, "message")
        );

        // Nothing was written: every member, including the `name` the same request asked
        // to change, is still the created one, and the record carries no `modifiedAt`.
        let fetched = get_partnership(&router, &partnership_id).await;
        assert_eq!(
            fetched, created,
            "case {case:?}: the refused update must leave the partnership untouched, got \
             {fetched} instead of {created}"
        );
        assert_eq!(
            fetched["capabilities"],
            json!(vec![bound.clone()]),
            "case {case:?}: the stored capability list must be the created one, got {:?}",
            fetched["capabilities"]
        );
        assert_eq!(
            fetched["modifiedAt"],
            Value::Null,
            "case {case:?}: a refused update must not stamp modifiedAt, got {:?}",
            fetched["modifiedAt"]
        );
    }
}

/// One row of the update table: the case name, the `capabilities` list the update
/// submits (absent when it names no list) and the `capabilityOptions` it submits.
type UpdateCase<'a> = (&'a str, Option<Vec<String>>, Option<&'a Value>);

/// RULE: an update writes exactly the members the request names onto the stored record —
/// a `capabilities` list REPLACES the stored one wholesale, in the order submitted —
/// leaves every member it omits as it was, keeps the system-assigned identity
/// (`partnershipId`, `partnershipArn`, `profileId`, `tradingPartnerId`, `createdAt`),
/// stamps `modifiedAt`, and answers with that stored record, which `GetPartnership` then
/// reports member for member.
///
/// The rows span capability-list sizes 0..3, the same ids in a different order, re-setting
/// the list the record already holds, swapping to a different capability, an update that
/// names only `name`, only `capabilityOptions`, both together, and an update naming no
/// optional member at all.
#[tokio::test]
async fn an_update_replaces_the_named_members_and_leaves_the_rest() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "acme").await;
    let capabilities: Vec<String> = {
        let mut ids = Vec::new();
        for index in 0..3 {
            ids.push(given_capability(&router, &format!("edi-{index}")).await);
        }
        ids
    };
    let partnership_id =
        given_partnership(&router, &profile_id, &capabilities[0], "Partner One").await;
    let created = get_partnership(&router, &partnership_id).await;

    // Members of `com.amazonaws.b2bi#CapabilityOptions`
    // (`smithy_query(query="shape",shape="CapabilityOptions")`).
    let outbound = json!({
        "outboundEdi": { "x12": { "common": { "validateEdi": true, "gs05TimeFormat": "HHMM" } } }
    });
    let wrapped = json!({
        "outboundEdi": { "x12": { "wrapOptions": { "wrapBy": "SEGMENT" } } }
    });

    // (case, the `capabilities` the update submits, the `capabilityOptions` it submits)
    let cases: Vec<UpdateCase<'_>> = vec![
        (
            "re-set the list the record already holds",
            Some(vec![capabilities[0].clone()]),
            None,
        ),
        (
            "swap to a different single capability",
            Some(vec![capabilities[1].clone()]),
            None,
        ),
        (
            "grow to two capabilities",
            Some(vec![capabilities[1].clone(), capabilities[2].clone()]),
            Some(&outbound),
        ),
        (
            "the same two in the other order",
            Some(vec![capabilities[2].clone(), capabilities[1].clone()]),
            None,
        ),
        (
            "all three capabilities",
            Some(capabilities.clone()),
            Some(&wrapped),
        ),
        ("an empty list unbinds every capability", Some(Vec::new()), None),
        (
            "back to one capability after the empty list",
            Some(vec![capabilities[2].clone()]),
            None,
        ),
        (
            "capabilityOptions rewritten with no capabilities list",
            None,
            Some(&outbound),
        ),
        ("name only: the stored list and options are kept", None, None),
    ];

    // What the store holds as the rows are applied one after another.
    let mut expected_capabilities = json!(vec![capabilities[0].clone()]);
    let mut expected_options = Value::Null;

    for (index, (case, submitted, options)) in cases.iter().enumerate() {
        let name = format!("partner-{index}");
        let mut request = json!({ "partnershipId": partnership_id.clone(), "name": name });
        if let Some(submitted) = submitted {
            request["capabilities"] = json!(submitted);
            expected_capabilities = json!(submitted);
        }
        if let Some(options) = options {
            request["capabilityOptions"] = (*options).clone();
            expected_options = (*options).clone();
        }

        let (status, updated) = call(&router, "UpdatePartnership", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdatePartnership must succeed, got {status} {updated}"
        );
        assert_eq!(
            updated["capabilities"], expected_capabilities,
            "case {case:?}: the capability list must be the submitted one, in order, got {:?}",
            updated["capabilities"]
        );
        assert_eq!(
            updated["capabilityOptions"], expected_options,
            "case {case:?}: capabilityOptions must be the value last written, got {:?}",
            updated["capabilityOptions"]
        );
        assert_eq!(
            text(&updated, "name"),
            name,
            "case {case:?}: the new name must be stored, got {:?}",
            updated["name"]
        );

        // The system-assigned identity and the members this operation cannot write are
        // the create's: an update never re-mints or re-points them.
        for member in [
            "partnershipId",
            "partnershipArn",
            "profileId",
            "tradingPartnerId",
            "email",
            "phone",
            "createdAt",
        ] {
            assert_eq!(
                updated[member], created[member],
                "case {case:?}: {member} must stay the created value, got {:?} instead of {:?}",
                updated[member], created[member]
            );
        }
        assert_eq!(
            text(&updated, "partnershipArn"),
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:partnership/{partnership_id}"),
            "case {case:?}: the ARN must name the request's region and the emulator account"
        );
        assert!(
            updated["modifiedAt"].as_f64().is_some(),
            "case {case:?}: an applied update must stamp modifiedAt, got {:?}",
            updated["modifiedAt"]
        );

        // Round trip: the independent reader reports the very record the update wrote.
        let fetched = get_partnership(&router, &partnership_id).await;
        for member in [
            "partnershipId",
            "partnershipArn",
            "profileId",
            "tradingPartnerId",
            "name",
            "email",
            "phone",
            "capabilities",
            "capabilityOptions",
            "createdAt",
            "modifiedAt",
        ] {
            assert_eq!(
                fetched[member], updated[member],
                "case {case:?}: GetPartnership disagrees with UpdatePartnership on {member}: \
                 {:?} vs {:?}",
                fetched[member], updated[member]
            );
        }
    }
}

/// RULE: the resource an update addresses is resolved before the references it carries, so
/// an update for a `partnershipId` this account's store does not hold is the modelled
/// `ResourceNotFoundException` (HTTP 404) naming that id — never a minted partnership, and
/// never a refusal about the capabilities list, however that list looks.
///
/// The rows span a never-minted id, ids at both ends of the modelled `PartnershipId`
/// length (`len 1..64`, `^[a-zA-Z0-9_-]+$`), an id another b2bi resource kind really does
/// hold, and an unknown partnership whose request also names an unknown capability.
#[tokio::test]
async fn updating_an_unknown_partnership_is_a_404() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "acme").await;
    let capability_id = given_capability(&router, "edi-850").await;

    // (case, the addressed partnershipId, the members the update names beside it)
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "a never-minted partnership id",
            "ps-0123456789abcdef0".to_string(),
            json!({ "capabilities": [capability_id.clone()] }),
        ),
        (
            "a one-character id",
            "p".to_string(),
            json!({ "name": "renamed" }),
        ),
        (
            "an id of the modelled maximum length",
            "p".repeat(64),
            json!({ "capabilities": [] }),
        ),
        (
            "a profile id that really exists",
            profile_id.clone(),
            json!({ "capabilities": [capability_id.clone()] }),
        ),
        (
            "an unknown partnership and an unknown capability",
            "ps-fedcba98765432100".to_string(),
            json!({ "capabilities": ["c-9999999999999999d"] }),
        ),
    ];

    for (case, partnership_id, members) in &cases {
        let mut request = members.clone();
        request["partnershipId"] = json!(partnership_id);
        let (status, body) = call(&router, "UpdatePartnership", request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {partnership_id:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            text(&body, "message").contains(partnership_id),
            "case {case:?}: the refusal must name the partnership that was not found, got {:?}",
            text(&body, "message")
        );

        // The refusal minted nothing: the reader still does not know that id.
        let (status, fetched) = call(
            &router,
            "GetPartnership",
            json!({ "partnershipId": partnership_id }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused update must not have created a partnership, got \
             {status} {fetched}"
        );
    }
}
