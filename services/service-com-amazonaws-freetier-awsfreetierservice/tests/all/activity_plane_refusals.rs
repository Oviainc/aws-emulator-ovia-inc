// SPDX-License-Identifier: CC0-1.0

//! What the activity plane SAYS when it refuses a request, driven over the wire
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSFreeTierService.<Operation>`)
//! through the same multi-service router the emulator boots.
//!
//! An HTTP status, an error code and an error message are all part of what a
//! client reads, so each of these rules is about one of them over a SET of
//! inputs rather than one call:
//!
//! 1. A listing serves page sizes up to the ceiling IT serves, which can be
//!    smaller than the maximum its `maxResults` member's `@range` declares, and
//!    refuses anything above that ceiling with `ValidationException` before it
//!    pages anything. The clause states the member and the requirement and
//!    quotes NO value, which is what tells a service-enforced bound from one the
//!    model declares.
//! 2. A not-found refusal states the service's fixed sentence for the resource
//!    type and echoes nothing of the request.
//! 3. An enumerated type's value SET is listed in the order the service states
//!    that enum in — the same order for every member targeting it, in any
//!    operation, whether the clause is stated at the member or nested inside one
//!    about the list that carries it.
//! 4. Each listing refuses a `nextToken` it did not issue in ITS OWN wording:
//!    the two listings do not share one sentence.
//!
//! Each rule's table is asserted against constants the service publishes
//! (`crate::state`), so a suite cannot pass by restating a sentence this crate
//! has drifted away from — and every value set is additionally pinned to be a
//! permutation of the one the MODEL declares (`ops::types`), so a published
//! order can reorder a set and never change its membership.

use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::ops::types::{
    ACCOUNT_PLAN_TYPE_VALUES, ACTIVITY_STATUS_VALUES, LANGUAGE_CODE_VALUES, MATCH_OPTION_VALUES,
};
use service_com_amazonaws_freetier_awsfreetierservice::state::{
    stated_enum_value_set, stated_validation, ACCOUNT_PLAN_TYPE_VALUE_SET,
    ACTIVITIES_INVALID_NEXT_TOKEN, ACTIVITIES_MAX_RESULTS_CEILING, ACTIVITY_STATUS_VALUE_SET,
    INVALID_NEXT_TOKEN, LANGUAGE_CODE_VALUE_SET,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::free_tier::{call, error_code, harness, ok, text, ACTIVITIES_PAGE_SIZE_CEILING};

/// The maximum the model declares for `MaxResults`, the shape both listings ask
/// their page size through (`smithy_query(query="shape",shape="MaxResults")`:
/// `@range(min: 1, max: 1000)`). The bound the framework's constraint walk
/// enforces, and the one the activity listing's own ceiling sits below.
const MODELLED_MAX_RESULTS: i32 = 1000;

/// The message of a refusal, with its status and code checked first.
async fn refusal_message(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    code: &str,
) -> String {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        code,
        "case {case}: expected the {code} shape, got {payload}"
    );
    text(&payload, "message")
}

/// Every activity id the catalog holds.
async fn catalog_ids(router: &MultiServiceRouter) -> Vec<String> {
    ok(
        router,
        "ListAccountActivities",
        &json!({"maxResults": ACTIVITIES_PAGE_SIZE_CEILING}),
    )
    .await["activities"]
        .as_array()
        .expect("activities is a list")
        .iter()
        .map(|summary| text(summary, "activityId"))
        .collect()
}

/// RULE 1: the activity listing serves every page size up to the ceiling it
/// serves and refuses every one above it, whatever else the request asked for,
/// and refusing one changes nothing a later caller sees.
///
/// The accepted rows span the class the rule generalises over — the smallest
/// page, sizes inside the collection, sizes past it, and the ceiling itself —
/// and the refused rows span one past the ceiling, well past it, and the
/// modelled maximum, each with and without the other members of the request.
#[tokio::test]
async fn the_activity_listing_refuses_a_page_size_above_the_ceiling_it_serves() {
    let router = harness().0;
    let before = catalog_ids(&router).await;
    assert!(
        !before.is_empty(),
        "case catalog: the account must hold activities for a page size to be observable"
    );
    const {
        assert!(
            ACTIVITIES_MAX_RESULTS_CEILING < MODELLED_MAX_RESULTS,
            "the listing's own ceiling must sit below the maximum the model declares"
        )
    };

    // The one clause the refusal states: the member, the requirement with the
    // ceiling as its only embedded number, and no value.
    let expected = format!(
        "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: \
         Member must have value less than or equal to {ACTIVITIES_MAX_RESULTS_CEILING}"
    );

    // Each refused size, once alone and once beside every other member this
    // listing models, so the bound cannot depend on the rest of the request.
    let also: [(&str, Value); 3] = [
        ("alone", json!({})),
        (
            "with a languageCode",
            json!({"languageCode": LANGUAGE_CODE_VALUES[0]}),
        ),
        (
            "with a status filter",
            json!({"filterActivityStatuses": [ACTIVITY_STATUS_VALUES[0]]}),
        ),
    ];

    for size in [
        ACTIVITIES_MAX_RESULTS_CEILING + 1,
        ACTIVITIES_MAX_RESULTS_CEILING + 2,
        ACTIVITIES_MAX_RESULTS_CEILING * 2,
        MODELLED_MAX_RESULTS - 1,
        MODELLED_MAX_RESULTS,
    ] {
        for (shape, extra) in &also {
            let mut body = extra.clone();
            body["maxResults"] = json!(size);
            let case = format!("maxResults {size} {shape}");
            let actual =
                refusal_message(&router, &case, "ListAccountActivities", &body, "ValidationException")
                    .await;
            assert_eq!(
                actual, expected,
                "case {case}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    // The accepted side of the same bound, so the rows above are not passing
    // merely because the listing refuses everything.
    for size in [
        1,
        2,
        i32::try_from(before.len()).expect("the catalog is small"),
        ACTIVITIES_MAX_RESULTS_CEILING - 1,
        ACTIVITIES_MAX_RESULTS_CEILING,
    ] {
        let page = ok(
            &router,
            "ListAccountActivities",
            &json!({"maxResults": size}),
        )
        .await;
        let reported = page["activities"]
            .as_array()
            .unwrap_or_else(|| panic!("case maxResults {size}: expected a page, got {page}"))
            .len();
        let wanted = usize::try_from(size).expect("a positive page size").min(before.len());
        assert_eq!(
            reported, wanted,
            "case maxResults {size}: expected {wanted} rows, got {reported} in {page}"
        );
    }

    // A value outside the MODEL's own range is still refused as the model's
    // violation, value quoted and the modelled bound named: the listing's
    // ceiling is applied after that walk, not instead of it.
    let actual = refusal_message(
        &router,
        "maxResults past the modelled maximum",
        "ListAccountActivities",
        &json!({"maxResults": MODELLED_MAX_RESULTS + 1}),
        "ValidationException",
    )
    .await;
    let modelled = format!(
        "1 validation error detected: Value '{}' at 'maxResults' failed to satisfy constraint: \
         Member must have value less than or equal to {MODELLED_MAX_RESULTS}",
        MODELLED_MAX_RESULTS + 1
    );
    assert_eq!(
        actual, modelled,
        "case maxResults past the modelled maximum: expected {modelled:?}, got {actual:?}"
    );

    // The ceiling is the LISTING's: the usage listing serves the modelled
    // maximum, so nothing about the member's shape was changed.
    let usage = ok(
        &router,
        "GetFreeTierUsage",
        &json!({"maxResults": MODELLED_MAX_RESULTS}),
    )
    .await;
    assert!(
        usage["freeTierUsages"].is_array(),
        "case the usage listing at the modelled maximum: expected a served page, got {usage}"
    );

    let after = catalog_ids(&router).await;
    assert_eq!(
        after, before,
        "case after the refusals: a refused page size must leave the catalog unchanged, got \
         {after:?} against {before:?}"
    );
}

/// RULE 2: a well-formed `ActivityId` the catalog does not hold is refused with
/// the modelled `ResourceNotFoundException` carrying the service's fixed
/// sentence — the same bytes for every such id, with nothing of the request in
/// them.
///
/// The rows differ along the character classes of the absent id (digits, lower
/// case, upper case, mixed, one that differs from a held id in one character),
/// and along whether the request carried the optional `languageCode`.
#[tokio::test]
async fn an_absent_activity_is_refused_with_one_fixed_sentence() {
    let router = harness().0;
    let held = catalog_ids(&router).await;
    let first = held.first().expect("the account holds activities").clone();

    // A well-formed id that differs from a held one in a single character, so
    // the sentence cannot depend on how far the id is from a real one.
    let mut near_miss: Vec<char> = first.chars().collect();
    near_miss[0] = if near_miss[0] == 'a' { 'b' } else { 'a' };
    let near_miss: String = near_miss.into_iter().collect();

    let absent = [
        "00000000000000000000000000000000".to_string(),
        "f".repeat(32),
        "A".repeat(32),
        "A1b2C3d4E5f60718293a4b5c6d7e8f99".to_string(),
        near_miss,
    ];

    let mut said: Vec<(String, String)> = Vec::new();
    for activity_id in &absent {
        assert!(
            !held.contains(activity_id),
            "case {activity_id}: the probe must name no held activity, got {held:?}"
        );
        for (shape, extra) in [
            ("alone", json!({})),
            (
                "with a languageCode",
                json!({"languageCode": LANGUAGE_CODE_VALUES[0]}),
            ),
        ] {
            let mut body = extra;
            body["activityId"] = json!(activity_id);
            let case = format!("absent id {activity_id} {shape}");
            let message = refusal_message(
                &router,
                &case,
                "GetAccountActivity",
                &body,
                "ResourceNotFoundException",
            )
            .await;
            assert!(
                !message.contains(activity_id.as_str()),
                "case {case}: the sentence must echo nothing of the request, got {message:?}"
            );
            said.push((case, message));
        }
    }

    // One sentence for the whole class, asserted against each other row rather
    // than only against the expectation.
    let (_, expected) = said.first().expect("the table has rows").clone();
    assert!(
        !expected.is_empty(),
        "case the sentence: a not-found body carries a message, got {expected:?}"
    );
    for (case, message) in &said {
        assert_eq!(
            *message, expected,
            "case {case}: every absent id must be refused in one sentence, got {message:?} \
             beside {expected:?}"
        );
    }

    // The control: every id the catalog DOES hold still reads back, so the
    // sentence is answered for absence and nothing else.
    for activity_id in &held {
        let detail = ok(
            &router,
            "GetAccountActivity",
            &json!({"activityId": activity_id}),
        )
        .await;
        assert_eq!(
            text(&detail, "activityId"),
            *activity_id,
            "case held id {activity_id}: the detail view reports the stored row, got {detail}"
        );
    }
}

/// RULE 3: a value outside an enumerated type is refused naming the set that
/// type admits, in the order the SERVICE states that enum in — identically for
/// every member targeting it.
///
/// The rows differ along HOW the submitted value misses the set (wrong case,
/// wrong separator, an unmodelled region, an unmodelled language, the empty
/// string) and along which operation carried it. The published orders are first
/// pinned to be permutations of the sets the MODEL declares, so a published
/// order can only reorder a set and never change what it holds.
#[tokio::test]
async fn an_enum_value_set_is_stated_in_the_order_the_service_states_it() {
    let router = harness().0;
    let id = catalog_ids(&router)
        .await
        .first()
        .expect("the account holds activities")
        .clone();

    for (enumerated, published, declared) in [
        (
            "LanguageCode",
            LANGUAGE_CODE_VALUE_SET.to_vec(),
            LANGUAGE_CODE_VALUES.to_vec(),
        ),
        (
            "ActivityStatus",
            ACTIVITY_STATUS_VALUE_SET.to_vec(),
            ACTIVITY_STATUS_VALUES.to_vec(),
        ),
        (
            "AccountPlanType",
            ACCOUNT_PLAN_TYPE_VALUE_SET.to_vec(),
            ACCOUNT_PLAN_TYPE_VALUES.to_vec(),
        ),
    ] {
        let mut sorted_published = published.clone();
        sorted_published.sort_unstable();
        let mut sorted_declared = declared.clone();
        sorted_declared.sort_unstable();
        assert_eq!(
            sorted_published, sorted_declared,
            "case {enumerated}: the published order must hold exactly the values the model \
             declares, got {published:?} against {declared:?}"
        );
    }

    let language_set = LANGUAGE_CODE_VALUE_SET.join(", ");
    for code in ["xx-XX", "EN-US", "en_US", "en", "en-AU", "sv-SE", ""] {
        assert!(
            !LANGUAGE_CODE_VALUES.contains(&code),
            "case {code:?}: the probe must not be a modelled LanguageCode"
        );
        let expected = format!(
            "1 validation error detected: Value '{code}' at 'languageCode' failed to satisfy \
             constraint: Member must satisfy enum value set: [{language_set}]"
        );
        for (operation, body) in [
            (
                "GetAccountActivity",
                json!({"activityId": id, "languageCode": code}),
            ),
            ("ListAccountActivities", json!({"languageCode": code})),
        ] {
            let case = format!("{operation} languageCode {code:?}");
            let actual =
                refusal_message(&router, &case, operation, &body, "ValidationException").await;
            assert_eq!(
                actual, expected,
                "case {case}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    // The same order, nested inside the clause about the LIST whose elements
    // target the other enum. The rows differ along the number of elements and
    // the position of the bad ones.
    let status_set = ACTIVITY_STATUS_VALUE_SET.join(", ");
    for statuses in [
        vec!["BOGUS_STATUS"],
        vec!["COMPLETED", "BOGUS_STATUS"],
        vec!["COMPLETED", "BOGUS_STATUS", "EXPIRING"],
        vec!["BOGUS_STATUS", "FORGOTTEN"],
    ] {
        let expected = format!(
            "1 validation error detected: Value '[{}]' at 'filterActivityStatuses' failed to \
             satisfy constraint: Member must satisfy constraint: [Member must satisfy enum \
             value set: [{status_set}]]",
            statuses.join(", ")
        );
        let case = format!("filterActivityStatuses {statuses:?}");
        let actual = refusal_message(
            &router,
            &case,
            "ListAccountActivities",
            &json!({"filterActivityStatuses": statuses}),
            "ValidationException",
        )
        .await;
        assert_eq!(
            actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );
    }

    // The same rule on a THIRD enum, carried by another operation's member, so
    // the order is a property of the enum and not of the operation that states
    // it. The set `accountPlanType` is stated in is pinned over a wider table of
    // submitted values in `paid_plan_vetting.rs`.
    let plan_type_set = ACCOUNT_PLAN_TYPE_VALUE_SET.join(", ");
    let actual = refusal_message(
        &router,
        "accountPlanType outside its enum",
        "UpgradeAccountPlan",
        &json!({"accountPlanType": "GOLD"}),
        "ValidationException",
    )
    .await;
    assert_eq!(
        actual,
        format!(
            "1 validation error detected: Value 'GOLD' at 'accountPlanType' failed to satisfy \
             constraint: Member must satisfy enum value set: [{plan_type_set}]"
        ),
        "case accountPlanType outside its enum: the set must be stated in the order the \
         service states that enum in, got {actual:?}"
    );

    // The control: an enum this service publishes no order for keeps the order
    // the set reaches the clause in, so the published orders are the only thing
    // that moved.
    let presented: Vec<String> = MATCH_OPTION_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    assert_eq!(
        stated_enum_value_set(&presented),
        presented,
        "case MatchOption: an unpublished set keeps its presented order, got {:?}",
        stated_enum_value_set(&presented)
    );
}

/// RULE 4: each listing refuses a `nextToken` it did not issue in its own
/// wording.
///
/// The rows differ along how the token fails to be one the listing issued — not
/// a token at all, hex of something else, a token of the OTHER listing, and a
/// well-formed token carrying an offset no page could have stopped at — because
/// the condition is one rule and each listing states it in one sentence.
#[tokio::test]
async fn each_listing_states_its_own_unissued_token_sentence() {
    let router = harness().0;

    /// A token of `kind` carrying `offset`, in the encoding the service issues.
    fn token(kind: &str, offset: &str) -> String {
        ::hex::encode(format!("ftv1:{kind}:{offset}"))
    }

    // (the operation, the listing kind its own tokens name, the sentence it
    // states) — the sentences come from the service's own constants.
    let listings: [(&str, &str, String); 2] = [
        (
            "ListAccountActivities",
            "activities",
            ACTIVITIES_INVALID_NEXT_TOKEN.to_string(),
        ),
        (
            "GetFreeTierUsage",
            "freeTierUsages",
            stated_validation(INVALID_NEXT_TOKEN),
        ),
    ];

    for (operation, kind, expected) in &listings {
        let other = listings
            .iter()
            .find(|(named, _, _)| named != operation)
            .map(|(_, other_kind, _)| *other_kind)
            .expect("the service has two listings");

        for (case, probe) in [
            ("not a token at all", "not-a-real-pagination-token".to_string()),
            ("hex of something else", ::hex::encode("ftv0:activities:1")),
            ("a token of the other listing", token(other, "1")),
            ("an offset no page stops at", token(kind, "0")),
            ("an offset past the end", token(kind, "999")),
        ] {
            let case = format!("{operation} {case}");
            let actual = refusal_message(
                &router,
                &case,
                operation,
                &json!({"nextToken": probe}),
                "ValidationException",
            )
            .await;
            assert_eq!(
                actual, *expected,
                "case {case}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    // The two listings do not share one sentence, so a client must not read one
    // listing's wording off the other.
    let [(first_op, _, first), (second_op, _, second)] = &listings;
    assert_ne!(
        first, second,
        "case the two listings: {first_op} and {second_op} state their own sentence, got \
         {first:?} for both"
    );

    // The control: a token each listing DID issue still resumes it, so the
    // sentences answer an unissued token and nothing else.
    for (operation, items_member) in [
        ("ListAccountActivities", "activities"),
        ("GetFreeTierUsage", "freeTierUsages"),
    ] {
        let page = ok(&router, operation, &json!({"maxResults": 1})).await;
        let issued = text(&page, "nextToken");
        let resumed = ok(
            &router,
            operation,
            &json!({"maxResults": 1, "nextToken": issued}),
        )
        .await;
        assert!(
            resumed[items_member].is_array(),
            "case {operation} an issued token: expected the next page, got {resumed}"
        );
    }
}
