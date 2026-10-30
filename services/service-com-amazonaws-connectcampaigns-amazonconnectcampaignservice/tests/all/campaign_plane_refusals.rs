// SPDX-License-Identifier: CC0-1.0

//! How the campaign plane REFUSES a request, end to end over HTTP.
//!
//! Three rules, each asserted over a table whose rows differ along what the rule
//! generalises over rather than along the one request that exposed it:
//!
//! * A campaign is created only for an Amazon Connect instance the account holds. The
//!   account's Connect registry is the one record of which instances those are, so an
//!   id it does not hold names nothing to create a campaign for and the create is
//!   refused as a bad request (HTTP 400 `ValidationException`) with the service's fixed
//!   sentence for that class, storing nothing.
//! * A campaign-id lookup that resolves to nothing is refused with the service's
//!   wording for the CLASS the operation belongs to, composed as fixed text around the
//!   id the request carried: the operations addressed to the campaign resource itself
//!   word it one way, and the ones addressed to its run state another.
//! * `ListCampaigns`' `nextToken` is an opaque base64 value, so it is decoded before
//!   anything is resolved from it: a token carrying a character the alphabet does not
//!   hold is refused as the decode failure it is, naming that character by the hex code
//!   of its byte, and a token that decodes but names no position this service issued is
//!   refused in different words.
//! * Every reply this service serves — a success, a refusal a handler raised, a refusal
//!   the framework rendered in front of one, and an unroutable request alike — announces
//!   the media type this service's front end labels its JSON with, rather than the one
//!   its protocol trait implies.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::state::{
    campaign_absent_message, campaign_not_found_message, DANGLING_BASE64_UNIT_MESSAGE,
    MALFORMED_BASE64_UNIT_MESSAGE,
};

use super::common::{call, router, Harness};

/// The region every case here signs for.
const REGION: &str = "us-east-1";

/// The Connect instance the positive controls below create their campaigns for.
const INSTANCE: &str = "control-instance";

/// The sentence the operations addressed to the campaign RESOURCE answer a campaign id
/// that resolves to nothing with, as fixed text around the id the request carried.
///
/// Written out here, not read back out of the implementation, so this suite pins the
/// wording itself; `{id}` is where the request's id goes (see [`worded`]).
const RESOURCE_NOT_FOUND_WORDING: &str = "CampaignId not found {id}.";

/// The sentence the operations addressed to the campaign's RUN STATE answer the same
/// miss with — a different class, worded differently. Written out for the same reason.
const RUN_STATE_NOT_FOUND_WORDING: &str = "Campaign: {id} doesn't exist";

/// The sentence a `nextToken` that DECODES but names no position this service issued is
/// refused with: it carries nothing from the request, so there is no `{id}` to fill.
const UNISSUED_TOKEN_WORDING: &str = "nextToken is not a page token this service issued";

/// `wording` with the request's campaign `id` in the place it names.
fn worded(wording: &str, id: &str) -> String {
    wording.replace("{id}", id)
}

/// A minimal `CreateCampaign` body for `instance`: every member the model marks
/// required, and nothing else.
fn create_body(name: &str, instance: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": instance,
        "dialerConfig": {"progressiveDialerConfig": {"bandwidthAllocation": 0.5}},
        "outboundCallConfig": {"connectContactFlowId": "flow-1"},
    })
}

/// Register `INSTANCE` with the account and create one campaign for it, handing back the
/// minted campaign id — the positive control that the refusals below are the refusal
/// under test and not the operation being broken.
async fn a_stored_campaign(router: &Harness, name: &str) -> String {
    router.register_connect_instance(REGION, INSTANCE);
    let created = call(
        router,
        REGION,
        "PUT",
        "/campaigns",
        &create_body(name, INSTANCE),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"create the control campaign\": expected HTTP 200, got {} {}",
        created.status, created.body
    );
    created.body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"create the control campaign\": no id in {}", created.body))
        .to_string()
}

/// How many campaigns `ListCampaigns` reports, over every page.
async fn stored_campaign_count(router: &Harness) -> usize {
    let listed = call(
        router,
        REGION,
        "POST",
        "/campaigns-summary",
        &json!({"maxResults": 50}),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case \"list the stored campaigns\": expected HTTP 200, got {} {}",
        listed.status, listed.body
    );
    listed.body["campaignSummaryList"]
        .as_array()
        .map_or(0, Vec::len)
}

/// Connect instance ids the account does not hold, spanning `InstanceId` (len 0..256,
/// pattern `^[a-zA-Z0-9_\-.]*$`, from
/// `smithy_query(query="shape", shape="InstanceId")`): the uuid form a Connect instance
/// id takes, the empty id the length minimum admits, every punctuation character the
/// pattern allows, and the longest id the bound allows.
fn unheld_instances() -> Vec<(&'static str, String)> {
    vec![
        (
            "the uuid form a Connect instance id takes",
            "1b4d2c7e-0a55-4f81-9d2c-7e0a554f8100".to_string(),
        ),
        ("the empty id the length minimum admits", String::new()),
        (
            "every punctuation character the pattern admits",
            "a.b_c-D9".to_string(),
        ),
        ("256 characters (the length maximum)", "i".repeat(256)),
    ]
}

#[tokio::test]
async fn a_campaign_is_created_only_for_an_instance_the_account_holds() {
    let router = router();
    let control = a_stored_campaign(&router, "control").await;

    let mut rows = unheld_instances();
    // An id the account HELD and has since given up is as unheld as one it never had:
    // the gate is the registry's present contents, not the id's shape or history.
    let surrendered = "surrendered-instance";
    router.register_connect_instance(REGION, surrendered);
    assert!(
        router.deregister_connect_instance(surrendered),
        "case \"an id the account held and gave up\": expected the registered instance to \
         be removable"
    );
    rows.push((
        "an id the account held and gave up",
        surrendered.to_string(),
    ));

    for (case, instance) in rows {
        let refused = call(
            &router,
            REGION,
            "PUT",
            "/campaigns",
            &create_body("rejected", &instance),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for an instance the account does not hold, \
             got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            "Connect Instance does not exist or is not enabled to use Campaigns.",
            "case {case:?}: expected the service's fixed sentence for an instance Campaigns \
             cannot be used with, got {}",
            refused.body
        );
        assert!(
            refused.body.get("id").is_none() && refused.body.get("arn").is_none(),
            "case {case:?}: a refused create mints nothing, got {}",
            refused.body
        );
    }

    // Nothing the refusals above sent was stored: the only campaign is the control.
    let count = stored_campaign_count(&router).await;
    assert_eq!(
        count, 1,
        "case \"the store after every refusal\": expected only the control campaign to be \
         stored, got {count} campaigns"
    );

    // And the control campaign — created for an instance the account does hold — reads
    // back through DescribeCampaign.
    let described = call(
        &router,
        REGION,
        "GET",
        &format!("/campaigns/{control}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        (
            described.status,
            described
                .body
                .pointer("/campaign/connectInstanceId")
                .and_then(Value::as_str)
                .unwrap_or("<absent>")
        ),
        (200, INSTANCE),
        "case \"the campaign created for a held instance\": expected it to read back for \
         that instance, got {} {}",
        described.status,
        described.body
    );
}

/// Campaign ids no campaign carries, spanning `CampaignId` (len 0..256, pattern
/// `^[\S]*$`, from `smithy_query(query="shape", shape="CampaignId")`): the uuid form, the
/// shortest id, a punctuation-laden id the pattern admits, the longest id the bound
/// allows, and the 32-hex form this emulator mints (with a digit changed, so it names no
/// campaign).
fn unknown_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "the uuid form",
            "7c3ad1e9-55b0-4a62-8f31-20d4e6b7c900".to_string(),
        ),
        ("one character (the length minimum a label can carry)", "z".to_string()),
        (
            "punctuation the pattern admits",
            "a:b-D9.e_f".to_string(),
        ),
        ("256 characters (the length maximum)", "c".repeat(256)),
        (
            "the minted id form",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
    ]
}

#[tokio::test]
async fn a_campaign_id_that_resolves_to_nothing_is_refused_in_its_operations_own_words() {
    let router = router();
    let stored = a_stored_campaign(&router, "control").await;

    // The operations addressed to the campaign RESOURCE an id names, and the ones
    // addressed to its RUN STATE: two classes, worded differently, as `(name, method,
    // path suffix, the wording that class answers with)`.
    let classes: &[(&str, &str, &str, &str)] = &[
        ("DescribeCampaign", "GET", "", RESOURCE_NOT_FOUND_WORDING),
        ("DeleteCampaign", "DELETE", "", RESOURCE_NOT_FOUND_WORDING),
        (
            "GetCampaignState",
            "GET",
            "/state",
            RUN_STATE_NOT_FOUND_WORDING,
        ),
        (
            "StartCampaign",
            "POST",
            "/start",
            RUN_STATE_NOT_FOUND_WORDING,
        ),
        (
            "PauseCampaign",
            "POST",
            "/pause",
            RUN_STATE_NOT_FOUND_WORDING,
        ),
        ("StopCampaign", "POST", "/stop", RUN_STATE_NOT_FOUND_WORDING),
    ];

    for (operation, method, suffix, wording) in classes {
        for (id_case, id) in unknown_ids() {
            let case = format!("{operation} / {id_case}");
            let refused = call(
                &router,
                REGION,
                method,
                &format!("/campaigns/{id}{suffix}"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                refused.status, 404,
                "case {case:?}: expected HTTP 404 for an id no campaign carries, got {} {}",
                refused.status, refused.body
            );
            assert!(
                refused.error_code.ends_with("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                refused.error_code
            );
            assert_eq!(
                refused.message(),
                worded(wording, &id),
                "case {case:?}: expected this operation's own not-found wording around the \
                 id the request carried, got {}",
                refused.body
            );
        }
    }

    // The two wordings are distinct, which is the point of keeping them apart — and the
    // helpers the handlers compose them with answer the wordings this suite pinned above.
    assert_ne!(
        RESOURCE_NOT_FOUND_WORDING, RUN_STATE_NOT_FOUND_WORDING,
        "case \"the two not-found classes\": the resource wording and the run-state \
         wording must not be unified"
    );
    for (helper, wording) in [
        (
            campaign_not_found_message("an-id"),
            worded(RESOURCE_NOT_FOUND_WORDING, "an-id"),
        ),
        (
            campaign_absent_message("an-id"),
            worded(RUN_STATE_NOT_FOUND_WORDING, "an-id"),
        ),
    ] {
        assert_eq!(
            helper, wording,
            "case \"the shared wording helpers\": expected {wording:?}, got {helper:?}"
        );
    }

    // None of those refusals touched the stored campaign.
    let described = call(
        &router,
        REGION,
        "GET",
        &format!("/campaigns/{stored}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case \"the stored campaign afterwards\": expected HTTP 200, got {} {}",
        described.status, described.body
    );
}

#[tokio::test]
async fn a_deleted_campaign_is_refused_the_same_way_an_id_that_never_existed_is() {
    let router = router();
    let stored = a_stored_campaign(&router, "to-be-deleted").await;

    let deleted = call(
        &router,
        REGION,
        "DELETE",
        &format!("/campaigns/{stored}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete the stored campaign\": expected HTTP 200, got {} {}",
        deleted.status, deleted.body
    );

    for (operation, method, suffix, wording) in [
        ("DescribeCampaign", "GET", "", RESOURCE_NOT_FOUND_WORDING),
        ("DeleteCampaign", "DELETE", "", RESOURCE_NOT_FOUND_WORDING),
        (
            "GetCampaignState",
            "GET",
            "/state",
            RUN_STATE_NOT_FOUND_WORDING,
        ),
    ] {
        let case = format!("{operation} after the delete");
        let refused = call(
            &router,
            REGION,
            method,
            &format!("/campaigns/{stored}{suffix}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            (refused.status, refused.message().to_string()),
            (404, worded(wording, &stored)),
            "case {case:?}: expected this operation's own not-found wording around the \
             deleted id, got {} {}",
            refused.status,
            refused.body
        );
    }
}

/// `nextToken` values that are not base64, as `(case, token, the hex code of the first
/// character the standard alphabet does not hold)`.
///
/// The rows differ along WHICH character is illegal and WHERE it sits: the first
/// character, one after a legal prefix, one at the end, a character of the url-safe
/// alphabet (which the standard one does not hold), and one below the alphabet's digits.
fn undecodable_tokens() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("a hyphen at the front", "-QUJDRA", "2d"),
        ("a hyphen after a legal prefix", "not-a-page-token", "2d"),
        ("a space inside the token", "QUJD RA==", "20"),
        ("the url-safe alphabet's underscore", "QUJD_wAA", "5f"),
        ("an at sign at the end", "QUJDRA@", "40"),
        ("a character below the digits", "QUJD!RA=", "21"),
    ]
}

#[tokio::test]
async fn a_next_token_that_is_not_base64_is_refused_as_the_decode_failure_it_is() {
    let router = router();
    a_stored_campaign(&router, "control").await;

    for (case, token, illegal_hex) in undecodable_tokens() {
        let refused = call(
            &router,
            REGION,
            "POST",
            "/campaigns-summary",
            &json!({"nextToken": token}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a token that is not base64, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            format!("Illegal base64 character {illegal_hex}"),
            "case {case:?}: expected the decode failure to name the first illegal \
             character by the hex code of its byte, got {}",
            refused.body
        );
        assert!(
            !refused.message().contains(token),
            "case {case:?}: the refusal must not echo the token the caller sent, got {}",
            refused.body
        );
    }
}

#[tokio::test]
async fn a_next_token_whose_last_unit_is_not_one_is_refused_as_that_decode_failure() {
    let router = router();
    a_stored_campaign(&router, "control").await;

    // Every character here is one the alphabet holds, so the decode fails on the SHAPE
    // of the value's last 4-byte unit rather than on a character — a different decoder
    // sentence, one row per shape it reports.
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a dangling character after the whole units",
            "QUJDR",
            DANGLING_BASE64_UNIT_MESSAGE,
        ),
        (
            "padding in the middle of a unit",
            "QU=JDRA=",
            MALFORMED_BASE64_UNIT_MESSAGE,
        ),
    ];

    for (case, token, expected) in rows {
        let refused = call(
            &router,
            REGION,
            "POST",
            "/campaigns-summary",
            &json!({"nextToken": token}),
        )
        .await;
        assert_eq!(
            (refused.status, refused.message()),
            (400, expected),
            "case {case:?}: expected the decoder's account of the malformed unit, got {} {}",
            refused.status,
            refused.body
        );
    }
}

#[tokio::test]
async fn a_next_token_that_decodes_but_was_never_issued_is_refused_in_other_words() {
    let router = router();
    a_stored_campaign(&router, "control").await;

    // Every row is legal standard base64, so the decode succeeds and the refusal is the
    // resolve failing instead: the empty token, a short value, one with padding, one
    // without it, one whose last unit carries bits a decoder that pads on demand drops,
    // and one at `NextToken`'s published 1000-character maximum (its `@length`, from
    // `smithy_query(query="shape", shape="NextToken")`).
    let rows: Vec<(&str, String)> = vec![
        ("the empty token", String::new()),
        ("a padded four-character unit", "QUJDRA==".to_string()),
        ("an unpadded unit", "QUJD".to_string()),
        ("a longer legal value", "QUJDRA5GRw".to_string()),
        (
            "a last unit carrying bits a lenient decoder drops",
            "QUJDRB".to_string(),
        ),
        (
            "1000 characters (the length maximum)",
            "A".repeat(1000),
        ),
    ];

    for (case, token) in rows {
        let refused = call(
            &router,
            REGION,
            "POST",
            "/campaigns-summary",
            &json!({"nextToken": token}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a token this service never issued, got \
             {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.message(),
            UNISSUED_TOKEN_WORDING,
            "case {case:?}: expected the unissued-token wording rather than a decode \
             failure, got {}",
            refused.body
        );
        assert!(
            !refused.message().starts_with("Illegal base64 character"),
            "case {case:?}: a token that DECODES must not be reported as a decode failure, \
             got {}",
            refused.body
        );
    }
}

#[tokio::test]
async fn the_next_token_this_service_hands_out_is_base64_and_resumes_the_walk() {
    use base64::Engine as _;

    let router = router();
    router.register_connect_instance(REGION, INSTANCE);
    let mut created = Vec::new();
    for index in 0..3 {
        let answer = call(
            &router,
            REGION,
            "PUT",
            "/campaigns",
            &create_body(&format!("c{index}"), INSTANCE),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case \"create campaign {index}\": expected HTTP 200, got {} {}",
            answer.status, answer.body
        );
        created.push(answer.body["id"].as_str().unwrap_or_default().to_string());
    }

    // One campaign per page, so every page but the last hands out a token; each one must
    // decode as standard base64 and resume the walk where the last page ended.
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..4 {
        let mut request = json!({"maxResults": 1});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let answer = call(&router, REGION, "POST", "/campaigns-summary", &request).await;
        assert_eq!(
            answer.status, 200,
            "case \"page {page}\": expected HTTP 200, got {} {}",
            answer.status, answer.body
        );
        for summary in answer.body["campaignSummaryList"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            walked.push(summary["id"].as_str().unwrap_or_default().to_string());
        }
        token = answer.body["nextToken"].as_str().map(str::to_string);
        if let Some(handed_out) = &token {
            let decoded = base64::engine::general_purpose::STANDARD.decode(handed_out);
            assert!(
                decoded.is_ok(),
                "case \"page {page}\": expected the token handed out to be base64, got \
                 {handed_out:?}"
            );
            assert!(
                !created.iter().any(|id| handed_out.contains(id)),
                "case \"page {page}\": a page token must not carry a campaign id, got \
                 {handed_out:?}"
            );
        } else {
            break;
        }
    }

    assert_eq!(
        walked, created,
        "case \"the whole walk\": expected every campaign once, in creation order, got \
         {walked:?}"
    );
}

#[tokio::test]
async fn every_reply_announces_the_media_type_this_services_front_end_labels_json_with() {
    let router = router();
    let stored = a_stored_campaign(&router, "control").await;

    // One row per kind of producer a reply can come from: a handler's success, a handler's
    // refusal, the refusal the framework renders INSIDE the request walk from a modelled
    // bound, a stub's 501, and a request this service claims but no modelled route serves.
    let rows: Vec<(&str, &str, String, Value)> = vec![
        (
            "a handler's success",
            "POST",
            "/campaigns-summary".to_string(),
            json!({"maxResults": 50}),
        ),
        (
            "a handler's success with a body of its own",
            "GET",
            format!("/campaigns/{stored}"),
            Value::Null,
        ),
        (
            "a handler's refusal",
            "PUT",
            "/campaigns".to_string(),
            create_body("rejected", "an-instance-the-account-does-not-hold"),
        ),
        (
            "a handler's not-found refusal",
            "GET",
            "/campaigns/no-such-campaign".to_string(),
            Value::Null,
        ),
        (
            "the framework's refusal of a value outside a modelled bound",
            "POST",
            "/campaigns-summary".to_string(),
            json!({"maxResults": 0}),
        ),
        (
            "an operation this emulator has not implemented yet",
            "POST",
            format!("/campaigns/{stored}/resume"),
            Value::Null,
        ),
        (
            "a route this service claims but no modelled operation serves",
            "PATCH",
            "/campaigns".to_string(),
            Value::Null,
        ),
    ];

    for (case, method, path, body) in rows {
        let answer = call(&router, REGION, method, &path, &body).await;
        assert_eq!(
            answer.media_type, "application/x-amz-json-1.1",
            "case {case:?}: expected every reply to announce this service's media type, \
             got {:?} on HTTP {} {}",
            answer.media_type, answer.status, answer.body
        );
    }
}
