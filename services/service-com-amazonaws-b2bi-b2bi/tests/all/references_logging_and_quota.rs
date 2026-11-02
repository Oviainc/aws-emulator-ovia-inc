// SPDX-License-Identifier: CC0-1.0

//! What a b2bi control-plane call REFUSES, and what it provisions elsewhere.
//!
//! Four rules, each asserted over a table of inputs rather than one recorded request:
//!
//! * a create that references another resource by id resolves every id it names in the
//!   store first, and refuses an unresolvable one with `ResourceNotFoundException`
//!   without persisting anything;
//! * a `ResourceNotFoundException` message is built from the request's own identifier,
//!   with the template this service states for that resource type;
//! * a violated numeric `@range` bound is reported as the AWS validation envelope naming
//!   the member and the bound, and never the value that was rejected;
//! * a profile created with `logging` ENABLED has a real CloudWatch Logs log group under
//!   the `logGroupName` it reports, and one created DISABLED has none — and the account's
//!   documented five-profile limit is charged, and released by a delete.
//!
//! The harness (router, call, seeding) is `onboarding_loop`'s: this suite mounts the same
//! b2bi + CloudWatch Logs pair on one shared engine.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::scaffolded::NOT_FOUND_STATUS;
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{
    b2bi_router, call, call_signed_for, capability_configuration, error_code, given_capability,
    given_capability_buckets, given_partnership, given_profile, given_transformer, text,
    SIGNING_REGION,
};

/// Well-formed ids no store holds: they satisfy the modelled `len 1..64` /
/// `^[a-zA-Z0-9_-]+$` on every b2bi identifier, so what refuses them is the existence
/// check and not input validation. The rows differ in prefix, length and character class.
const UNKNOWN_IDS: &[(&str, &str)] = &[
    ("a plausible generated id", "0123456789abcdef0"),
    ("the recorded-style id", "doesnotexist00000000"),
    ("a single character", "x"),
    ("underscores and dashes", "no_such-RESOURCE-42"),
    (
        "the longest id the model allows",
        "0123456789012345678901234567890123456789012345678901234567890123",
    ),
];

/// One row of the not-found table: the resource kind, the two operations keyed on that
/// resource's id, the request member carrying the id, and the message template.
type NotFoundCase<'a> = (&'a str, [&'a str; 2], &'a str, fn(&str) -> String);

/// Ask the CloudWatch Logs service, over the wire, for every log group whose name starts
/// with `prefix` — the call a client makes when it follows a profile's `logGroupName`.
async fn describe_log_groups(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "Logs_20140328.DescribeLogGroups")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{SIGNING_REGION}/logs/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&json!({ "logGroupNamePrefix": prefix }))
                .expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "DescribeLogGroups({prefix}) must succeed, got {status} {parsed}"
    );
    parsed["logGroups"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|group| text(group, "logGroupName"))
        .collect()
}

/// RULE: `CreateCapability` resolves `configuration.edi.transformerId` in the store
/// before minting anything — an id no transformer carries is the modelled
/// `ResourceNotFoundException` and no capability is created.
#[tokio::test]
async fn creating_a_capability_for_an_unknown_transformer_is_refused_as_not_found() {
    let router = b2bi_router();
    // A transformer a client really created, so the positive row below is the same
    // request as the refused ones with one id changed.
    let transformer_id = given_transformer(&router, "edi-850-to-json").await;
    // The locations the configuration names are this account's own, so what refuses the
    // rows below is the transformer reference rather than the buckets.
    given_capability_buckets(&router).await;

    let (_, before) = call(&router, "ListCapabilities", json!({})).await;
    let capabilities_before = before["capabilities"].as_array().map(Vec::len).unwrap_or(0);

    for (case, unknown) in UNKNOWN_IDS {
        let mut configuration =
            capability_configuration(&transformer_id, "X12_850", "VERSION_4010");
        configuration["edi"]["transformerId"] = json!(unknown);
        let (status, body) = call(
            &router,
            "CreateCapability",
            json!({
                "name": format!("capability for {case}"),
                "type": "edi",
                "configuration": configuration
            }),
        )
        .await;

        assert_eq!(
            status,
            NOT_FOUND_STATUS,
            "case {case:?}: a capability naming transformer {unknown:?} must be refused, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            text(&body, "message"),
            format!("Transformer with transformerId {unknown} not found"),
            "case {case:?}: the message must name the transformerId from the request, got {:?}",
            text(&body, "message")
        );
    }

    // Nothing leaked: the refused calls persisted no capability...
    let (_, after) = call(&router, "ListCapabilities", json!({})).await;
    assert_eq!(
        after["capabilities"].as_array().map(Vec::len).unwrap_or(0),
        capabilities_before,
        "a refused CreateCapability must store nothing, got {after}"
    );
    // ...while the SAME request naming the transformer that exists succeeds, end to end
    // from CreateTransformer through CreateCapability to GetCapability.
    let (status, created) = call(
        &router,
        "CreateCapability",
        json!({
            "name": "capability for the transformer that exists",
            "type": "edi",
            "configuration": capability_configuration(&transformer_id, "X12_850", "VERSION_4010")
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a capability naming a created transformer must be accepted, got {status} {created}"
    );
    let capability_id = text(&created, "capabilityId");
    let (status, fetched) = call(
        &router,
        "GetCapability",
        json!({ "capabilityId": capability_id }),
    )
    .await;
    assert_eq!(status, 200, "GetCapability failed: {status} {fetched}");
    assert_eq!(
        fetched["configuration"]["edi"]["transformerId"],
        json!(transformer_id),
        "the stored capability must keep the transformer it was created against, got {fetched}"
    );
}

/// RULE: `CreatePartnership` resolves EVERY element of the required `capabilities` list
/// (and the `profileId`) in the store before minting anything; the first unresolvable id
/// is the modelled `ResourceNotFoundException`.
#[tokio::test]
async fn creating_a_partnership_with_an_unknown_capability_is_refused_as_not_found() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "acme").await;
    let real = given_capability(&router, "edi-850").await;
    let other = given_capability(&router, "edi-810").await;

    // (case, the capabilities list, the id the refusal must name)
    let cases: Vec<(String, Vec<String>, String)> = UNKNOWN_IDS
        .iter()
        .map(|(case, unknown)| {
            (
                format!("only an unknown capability: {case}"),
                vec![(*unknown).to_string()],
                (*unknown).to_string(),
            )
        })
        .chain([
            (
                "an unknown id after a real one".to_string(),
                vec![real.clone(), "cap-doesnotexist0000000000".to_string()],
                "cap-doesnotexist0000000000".to_string(),
            ),
            (
                "an unknown id between two real ones".to_string(),
                vec![real.clone(), "cap-missing-middle".to_string(), other.clone()],
                "cap-missing-middle".to_string(),
            ),
            (
                "two unknown ids: the FIRST is named".to_string(),
                vec!["cap-first-gap".to_string(), "cap-second-gap".to_string()],
                "cap-first-gap".to_string(),
            ),
        ])
        .collect();

    for (case, capabilities, named) in &cases {
        let (status, body) = call(
            &router,
            "CreatePartnership",
            json!({
                "profileId": profile_id,
                "name": format!("partner for {case}"),
                "email": "partner@example.com",
                "capabilities": capabilities
            }),
        )
        .await;
        assert_eq!(
            status,
            NOT_FOUND_STATUS,
            "case {case:?}: a partnership naming {capabilities:?} must be refused, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            text(&body, "message"),
            format!("Capability ID {named} not found."),
            "case {case:?}: the message must name the unresolved capability, got {:?}",
            text(&body, "message")
        );
    }

    // Nothing leaked, and the same call with only real capabilities succeeds.
    let (_, listed) = call(&router, "ListPartnerships", json!({})).await;
    assert_eq!(
        listed["partnerships"].as_array().map(Vec::len).unwrap_or(0),
        0,
        "a refused CreatePartnership must store nothing, got {listed}"
    );
    let partnership_id = given_partnership(&router, &profile_id, &real, "acme-partner").await;
    let (status, fetched) = call(
        &router,
        "GetPartnership",
        json!({ "partnershipId": partnership_id }),
    )
    .await;
    assert_eq!(status, 200, "GetPartnership failed: {status} {fetched}");
    assert_eq!(
        fetched["capabilities"],
        json!([real]),
        "the stored partnership must keep the capabilities it was created against, got {fetched}"
    );
}

/// RULE: a `ResourceNotFoundException` message is built from the request's own
/// identifier, with the template this service states for that resource type — never an
/// emulator-local phrase. Rows span all three resource kinds, both the reader and the
/// delete of each, and every id shape in [`UNKNOWN_IDS`].
#[tokio::test]
async fn a_not_found_message_names_the_requested_id_in_the_services_own_template() {
    let router = b2bi_router();

    // (resource kind, the operations keyed on it, the request member, the template)
    let kinds: &[NotFoundCase<'_>] = &[
        (
            "profile",
            ["GetProfile", "DeleteProfile"],
            "profileId",
            |id| format!("Profile ID {id} not found."),
        ),
        (
            "capability",
            ["GetCapability", "DeleteCapability"],
            "capabilityId",
            |id| format!("Capability ID {id} not found."),
        ),
        (
            "partnership",
            ["GetPartnership", "DeletePartnership"],
            "partnershipId",
            |id| format!("Partnership with partnershipId {id} not found."),
        ),
    ];

    for (kind, operations, member, template) in kinds {
        for operation in operations {
            for (id_case, unknown) in UNKNOWN_IDS {
                let case = format!("{operation} on an unknown {kind} ({id_case})");
                let (status, body) =
                    call(&router, operation, json!({ *member: unknown })).await;
                assert_eq!(
                    status,
                    NOT_FOUND_STATUS,
                    "case {case:?}: expected {NOT_FOUND_STATUS}, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {body}"
                );
                assert_eq!(
                    text(&body, "message"),
                    template(unknown),
                    "case {case:?}: expected the service's own not-found template, got {:?}",
                    text(&body, "message")
                );
            }
        }
    }
}

/// RULE: a value outside a modelled numeric `@range` is refused with
/// `ValidationException` (HTTP 400) whose message is the AWS validation envelope naming
/// the member and the bound it failed — and NOT the value that was rejected.
///
/// The numeric range this service's paginated input surface carries is `MaxResults`
/// (`smithy_query(query="shape",shape="MaxResults")`: `range {1,100}`), shared by every
/// List operation; the rows are each published bound, one value past each, and values
/// far outside.
#[tokio::test]
async fn a_range_violation_names_the_member_and_the_bound_but_not_the_value() {
    let router = b2bi_router();

    // (case, maxResults, the clause the envelope must carry) — `None` means the value
    // is inside the range and must be SERVED.
    let cases: &[(&str, i64, Option<&str>)] = &[
        ("the published minimum", 1, None),
        ("the published maximum", 100, None),
        ("one past the maximum", 101, Some("less than or equal to 100")),
        ("far past the maximum", 1_000, Some("less than or equal to 100")),
        ("one below the minimum", 0, Some("greater than or equal to 1")),
        ("negative", -1, Some("greater than or equal to 1")),
    ];

    for operation in ["ListProfiles", "ListCapabilities", "ListPartnerships"] {
        for (case, max_results, expected_clause) in cases {
            let label = format!("{operation} maxResults={max_results} ({case})");
            let (status, body) =
                call(&router, operation, json!({ "maxResults": max_results })).await;

            let Some(clause) = expected_clause else {
                assert_eq!(
                    status, 200,
                    "case {label:?}: a maxResults inside the modelled range must be served, \
                     got {status} {body}"
                );
                continue;
            };

            assert_eq!(status, 400, "case {label:?}: expected 400, got {status} {body}");
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                text(&body, "message"),
                format!(
                    "1 validation error detected: Value at 'maxResults' failed to satisfy \
                     constraint: Member must have value {clause}"
                ),
                "case {label:?}: expected the member and the bound and no value, got {:?}",
                text(&body, "message")
            );
            assert!(
                !text(&body, "message").contains(&format!("'{max_results}'")),
                "case {label:?}: the rejected value must not be echoed, got {:?}",
                text(&body, "message")
            );
        }
    }

    // A request that violates the range AND another constraint: the range clause still
    // drops its value, and the other clause reads as the framework states it (the empty
    // `profileId` violates `ProfileId`'s own `len 1..64`).
    let (status, body) = call(
        &router,
        "ListPartnerships",
        json!({ "maxResults": 101, "profileId": "" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case mixed violations: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case mixed violations: expected ValidationException, got {body}"
    );
    let message = text(&body, "message");
    assert!(
        message.contains(" validation errors detected: "),
        "case mixed violations: expected the multi-violation envelope, got {message:?}"
    );
    assert!(
        message.contains(
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             less than or equal to 100"
        ),
        "case mixed violations: the range clause must still name no value, got {message:?}"
    );
    assert!(
        message.contains("at 'profileId' failed to satisfy constraint: Member must have length"),
        "case mixed violations: the other clause must still be reported, got {message:?}"
    );
    assert!(
        !message.contains("'101'"),
        "case mixed violations: the rejected range value must not be echoed, got {message:?}"
    );
}

/// RULE: `logging` ENABLED means the service PROVISIONS the log group the profile
/// reports, so `logGroupName` resolves in CloudWatch Logs; `logging` DISABLED provisions
/// none and reports none. Rows span both enum values and two signing regions.
#[tokio::test]
async fn a_logging_enabled_profile_has_a_real_log_group_and_a_disabled_one_has_none() {
    let router = b2bi_router();

    // (case, signing region, logging)
    let cases: &[(&str, &str, &str)] = &[
        ("enabled, in the signing region", SIGNING_REGION, "ENABLED"),
        ("disabled, in the signing region", SIGNING_REGION, "DISABLED"),
        ("enabled, in another region", "eu-central-1", "ENABLED"),
        ("disabled, in another region", "eu-central-1", "DISABLED"),
    ];

    let mut expected_groups: Vec<String> = Vec::new();
    for (case, region, logging) in cases {
        let (status, created) = call_signed_for(
            &router,
            region,
            "CreateProfile",
            json!({
                "name": format!("profile for {case}"),
                "phone": "555-555-0100",
                "businessName": "Acme Corp",
                "logging": logging
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: CreateProfile failed: {created}");

        let profile_id = text(&created, "profileId");
        if *logging == "ENABLED" {
            let log_group = text(&created, "logGroupName");
            assert_eq!(
                log_group,
                format!("b2bi/{profile_id}-Logs"),
                "case {case:?}: expected the service's log-group name form, got {log_group:?}"
            );
            let found = describe_log_groups(&router, &log_group).await;
            assert!(
                found.contains(&log_group),
                "case {case:?}: the reported log group {log_group:?} must exist in CloudWatch \
                 Logs, found {found:?}"
            );
            expected_groups.push(log_group);
        } else {
            assert_eq!(
                created["logGroupName"],
                Value::Null,
                "case {case:?}: a DISABLED profile reports no logGroupName, got {created}"
            );
            let found = describe_log_groups(&router, &format!("b2bi/{profile_id}-Logs")).await;
            assert!(
                found.is_empty(),
                "case {case:?}: a DISABLED profile must leave no log group, found {found:?}"
            );
        }
    }

    // Exactly the ENABLED profiles' groups exist under this service's prefix — the
    // DISABLED ones added nothing.
    let mut under_prefix = describe_log_groups(&router, "b2bi/").await;
    under_prefix.sort();
    expected_groups.sort();
    assert_eq!(
        under_prefix, expected_groups,
        "only the logging-ENABLED profiles may own a log group"
    );
}

/// RULE: the limit holds under CONCURRENT creation too — racers past the remaining
/// slots are refused with `ServiceQuotaExceededException` rather than creating more
/// profiles than the account allows.
///
/// The rows are the two `Logging` values, because the ENABLED path provisions a log
/// group (an in-process call the handler awaits, so the racers really do interleave
/// there) while the DISABLED path does not.
#[tokio::test]
async fn concurrent_creates_past_the_quota_are_refused_not_served() {
    let limit = service_com_amazonaws_b2bi_b2bi::state::MAX_PROFILES_PER_ACCOUNT;
    // Two racers more than the account has room for. Fixed at seven because
    // `tokio::join!` takes a literal list of futures; the assertion below ties it back
    // to the limit.
    const RACERS: usize = 7;
    assert!(
        RACERS > limit,
        "the race must offer more creates than the {limit}-profile limit"
    );

    for logging in ["DISABLED", "ENABLED"] {
        let router = b2bi_router();
        let racer = |index: usize| {
            call(
                &router,
                "CreateProfile",
                json!({
                    "name": format!("racer-{index}"),
                    "phone": "555-555-0100",
                    "businessName": "Acme Corp",
                    "logging": logging
                }),
            )
        };
        let (a, b, c, d, e, f, g) = tokio::join!(
            racer(0),
            racer(1),
            racer(2),
            racer(3),
            racer(4),
            racer(5),
            racer(6)
        );
        let answers = [a, b, c, d, e, f, g];

        let accepted = answers.iter().filter(|(status, _)| *status == 200).count();
        assert_eq!(
            accepted, limit,
            "case logging={logging}: exactly {limit} of {RACERS} racers may be served, \
             got {answers:?}"
        );
        for (status, body) in answers.iter().filter(|(status, _)| *status != 200) {
            assert_eq!(
                *status, 402,
                "case logging={logging}: a refused racer must answer 402, got {status} {body}"
            );
            assert_eq!(
                error_code(body),
                "ServiceQuotaExceededException",
                "case logging={logging}: expected ServiceQuotaExceededException, got {body}"
            );
        }

        // And the store holds exactly the limit — no racer slipped an extra profile in.
        let (_, listed) = call(&router, "ListProfiles", json!({})).await;
        assert_eq!(
            listed["profiles"].as_array().map(Vec::len).unwrap_or(0),
            limit,
            "case logging={logging}: the account must hold exactly {limit} profiles, \
             got {listed}"
        );
        // Every profile that WAS created owns a log group when logging is on, and the
        // refused racers left none behind.
        let groups = describe_log_groups(&router, "b2bi/").await;
        let expected_groups = if logging == "ENABLED" { limit } else { 0 };
        assert_eq!(
            groups.len(),
            expected_groups,
            "case logging={logging}: expected {expected_groups} log groups, got {groups:?}"
        );
    }
}

/// RULE: the account holds at most the documented number of customer profiles
/// (`MAX_PROFILES_PER_ACCOUNT`, from `CreateProfile`: "You can have up to five customer
/// profiles"); a create past it is the modelled `ServiceQuotaExceededException`
/// (HTTP 402) naming the service and quota codes Service Quotas publishes, and deleting
/// a profile releases the slot.
#[tokio::test]
async fn the_profile_quota_is_enforced_charged_and_released() {
    let limit = service_com_amazonaws_b2bi_b2bi::state::MAX_PROFILES_PER_ACCOUNT;
    // The limit is AWS's own, documented on the operation ("You can have up to five
    // customer profiles"), so the number itself is pinned here and not just the
    // constant's behaviour — and it is the same number Service Quotas publishes for
    // ServiceCode b2bi / quotaCode ProfilesPerAccount.
    assert_eq!(
        limit, 5,
        "the documented customer-profile limit is five, got {limit}"
    );
    let router = b2bi_router();

    let mut created: Vec<String> = Vec::new();
    for index in 0..limit {
        let (status, body) = call(
            &router,
            "CreateProfile",
            json!({
                "name": format!("profile-{index}"),
                "phone": "555-555-0100",
                "businessName": "Acme Corp",
                "logging": "DISABLED"
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "profile #{index} of {limit} must be accepted, got {status} {body}"
        );
        created.push(text(&body, "profileId"));
    }

    // One past the limit, for both values of `logging`: the refusal is the same, and the
    // ENABLED row must not leave a log group behind for a profile that was not created.
    for logging in ["DISABLED", "ENABLED"] {
        let (status, body) = call(
            &router,
            "CreateProfile",
            json!({
                "name": "one too many",
                "phone": "555-555-0100",
                "businessName": "Acme Corp",
                "logging": logging
            }),
        )
        .await;
        assert_eq!(
            status, 402,
            "case logging={logging}: a create past the {limit}-profile limit must be refused, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case logging={logging}: expected ServiceQuotaExceededException, got {body}"
        );
        assert_eq!(
            (
                text(&body, "serviceCode"),
                text(&body, "quotaCode"),
                text(&body, "resourceType")
            ),
            (
                "b2bi".to_string(),
                "ProfilesPerAccount".to_string(),
                "profile".to_string()
            ),
            "case logging={logging}: the refusal must name the published quota, got {body}"
        );
        assert!(
            text(&body, "message").contains(&limit.to_string()),
            "case logging={logging}: the refusal must name the limit, got {:?}",
            text(&body, "message")
        );
    }

    // A refused create leaves no log group behind: the quota is charged before anything
    // is provisioned, so the whole account holds only the groups of profiles that exist.
    let groups = describe_log_groups(&router, "b2bi/").await;
    assert!(
        groups.is_empty(),
        "the five stored profiles have logging DISABLED and the refused ENABLED create \
         must have provisioned nothing, found {groups:?}"
    );

    let (_, listed) = call(&router, "ListProfiles", json!({})).await;
    assert_eq!(
        listed["profiles"].as_array().map(Vec::len).unwrap_or(0),
        limit,
        "a refused create must leave the account at its limit, got {listed}"
    );

    // The limit is a live count, not a high-water mark: a delete releases a slot.
    let (status, body) = call(
        &router,
        "DeleteProfile",
        json!({ "profileId": created[0].clone() }),
    )
    .await;
    assert_eq!(status, 200, "DeleteProfile failed: {status} {body}");

    let (status, body) = call(
        &router,
        "CreateProfile",
        json!({
            "name": "after the delete",
            "phone": "555-555-0100",
            "businessName": "Acme Corp",
            "logging": "ENABLED"
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a create must be accepted once a slot is released, got {status} {body}"
    );
    let log_group = text(&body, "logGroupName");
    let found = describe_log_groups(&router, &log_group).await;
    assert!(
        found.contains(&log_group),
        "the profile created into the released slot must own its log group, found {found:?}"
    );

    // A create that completes holds no slot beyond itself: the id it reserves while it
    // provisions its log group is released whichever way the call ends, so cycling
    // create/delete far more times than the limit never exhausts the account.
    let newest = text(&body, "profileId");
    let mut previous = newest;
    for cycle in 0..(limit * 3) {
        let (status, body) = call(
            &router,
            "DeleteProfile",
            json!({ "profileId": previous.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "cycle {cycle}: DeleteProfile failed: {status} {body}"
        );
        let (status, body) = call(
            &router,
            "CreateProfile",
            json!({
                "name": format!("cycle-{cycle}"),
                "phone": "555-555-0100",
                "businessName": "Acme Corp",
                "logging": "ENABLED"
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "cycle {cycle}: a create must be accepted while the account is below its \
             limit — a leaked reservation would refuse it, got {status} {body}"
        );
        previous = text(&body, "profileId");
    }
}
