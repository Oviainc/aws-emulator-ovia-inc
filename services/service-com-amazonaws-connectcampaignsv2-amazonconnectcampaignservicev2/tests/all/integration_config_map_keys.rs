// SPDX-License-Identifier: CC0-1.0

//! The keys of an `objectTypeNames` map, and the bounds of everything else inside an
//! `integrationConfig`, end to end over the wire.
//!
//! The rule under test: a map whose key shape is an enumerated type admits only the
//! values that enum declares, so a request carrying any other key is refused WHOLE
//! with HTTP 400 `ValidationException` and nothing of that map is stored — not even
//! the admissible keys sitting beside the inadmissible one. The refusal carries one
//! clause per inadmissible key, naming the key and the value set it failed, in the
//! validation envelope every constraint refusal in this emulator is worded with; the
//! clauses are ordered by key, so one request is refused with one sentence however the
//! map reached the handler.
//!
//! The rule is asserted on both enum-keyed maps this service accepts, which must agree
//! because they are one rule: the `EventType`-keyed `objectTypeNames` of
//! `PutConnectInstanceIntegration` and the `DayOfWeek`-keyed `dailyHours` of
//! `CreateCampaign`.
//!
//! The last case spans the rest of the `integrationConfig` union: its ARN members and
//! its object-type names are constrained LEAVES, which a constraint walk does reach, so
//! each published bound is asserted accepted and a value past it refused — the key is
//! the one thing inside the union no walk reaches and a handler has to weigh itself.

use serde_json::{json, Map, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    DAY_OF_WEEK_VALUES, EVENT_TYPE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, onboard_instance, padded_arn, router};

/// The region every case here signs for; nothing in an integration record is
/// region-dependent — the ARNs it holds are the caller's — so one is enough.
const REGION: &str = "eu-west-1";

/// The member an `objectTypeNames` map arrives under, which is what a refusal clause
/// names: the request member, the union member in force, then the map member.
const OBJECT_TYPE_NAMES_MEMBER: &str = "integrationConfig.customerProfiles.objectTypeNames";

/// The member a `dailyHours` map arrives under on `CreateCampaign`, spelled the same
/// way: the request member, the channel, the `openHours` union member, then the map.
const DAILY_HOURS_MEMBER: &str = "communicationTimeConfig.telephony.openHours.dailyHours";

/// A Customer Profiles domain ARN the model's `Arn` shape admits, used wherever a case
/// is about something other than the ARN.
const DOMAIN_ARN: &str = "arn:aws:profile:eu-west-1:000000000000:domains/map-keys-domain";

/// The head [`padded_arn`] grows a domain ARN from, for the published bounds of `Arn`'s
/// `@length` (20..500, from `smithy_query(query="shape", shape="Arn")`).
const DOMAIN_ARN_HEAD: &str = "arn:aws:profile:eu-west-1:000000000000:domains/";

/// A `customerProfiles` integration config over `keys`, each mapped to an object type
/// name the value shape admits.
fn profiles_config(keys: &[&str]) -> Value {
    let mut map = Map::new();
    for key in keys {
        map.insert((*key).to_string(), json!("object-type"));
    }
    json!({"customerProfiles": {"domainArn": DOMAIN_ARN, "objectTypeNames": map}})
}

/// The whole message a map at `member` is refused with when `offenders` are not values
/// `allowed` holds: the validation envelope, carrying one clause per offending key,
/// ordered by key.
fn expected_message(member: &str, allowed: &[&str], offenders: &[&str]) -> String {
    let mut ordered: Vec<&str> = offenders.to_vec();
    ordered.sort_unstable();
    ordered.dedup();
    let clauses: Vec<String> = ordered
        .iter()
        .map(|key| {
            format!(
                "Value '{key}' at '{member}.{key}.member' failed to satisfy constraint: \
                 Member must satisfy enum value set: [{}]",
                allowed.join(", ")
            )
        })
        .collect();
    if clauses.len() == 1 {
        format!("1 validation error detected: {}", clauses[0])
    } else {
        format!(
            "{} validation errors detected: {}",
            clauses.len(),
            clauses.join("; ")
        )
    }
}

/// The message a response carries.
fn message_of(response: &Value) -> &str {
    response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// `PutConnectInstanceIntegration` for `instance` with `config`.
async fn put_integration(
    router: &MultiServiceRouter,
    instance: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance}/integrations"),
        &json!({"integrationConfig": config}),
    )
    .await
}

/// Every integration `instance` holds, as `ListConnectInstanceIntegrations` reports
/// them — the reading a refused put must leave exactly as it was.
async fn stored_integrations(
    router: &MultiServiceRouter,
    instance: &str,
    case: &str,
) -> Vec<Value> {
    let (status, body) = call(
        router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance}/integrations?maxResults=50"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConnectInstanceIntegrations expected HTTP 200, got {status} {body}"
    );
    body.get("integrationSummaryList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One `objectTypeNames` map a request can carry: what to call it, the keys it holds,
/// and which of those keys the `EventType` catalog does not hold. An empty `offenders`
/// means the whole map is admissible.
struct MapCase {
    case: &'static str,
    keys: Vec<&'static str>,
    offenders: Vec<&'static str>,
}

impl MapCase {
    fn new(case: &'static str, keys: &[&'static str], offenders: &[&'static str]) -> Self {
        Self {
            case,
            keys: keys.to_vec(),
            offenders: offenders.to_vec(),
        }
    }
}

/// The maps one put can carry.
///
/// The rows differ along what the rule generalises over: how a key misses the catalog
/// (a channel it does not name, a catalog value in the wrong case, an abbreviation of
/// one, one with trailing space, the catalog value of a DIFFERENT enum, the empty
/// key), how many inadmissible keys the map holds (none, one, two, three) and how many
/// admissible keys share the map with them (none, one, two). Catalog values the issue
/// this suite was written for never cited are asserted ACCEPTED, so the check refuses
/// only what the catalog leaves out.
fn map_cases() -> Vec<MapCase> {
    vec![
        MapCase::new("the empty map", &[], &[]),
        MapCase::new("one catalog value", &["Campaign-WebNotification"], &[]),
        MapCase::new(
            "two catalog values",
            &["Campaign-WhatsApp", "Campaign-Orchestration"],
            &[],
        ),
        MapCase::new("every catalog value", EVENT_TYPE_VALUES, &[]),
        MapCase::new(
            "a channel the catalog does not name",
            &["Campaign-Fax"],
            &["Campaign-Fax"],
        ),
        MapCase::new(
            "a catalog value in the wrong case",
            &["campaign-email"],
            &["campaign-email"],
        ),
        MapCase::new(
            "an abbreviation of a catalog value",
            &["Campaign-Tel"],
            &["Campaign-Tel"],
        ),
        MapCase::new(
            "a catalog value with trailing space",
            &["Campaign-SMS "],
            &["Campaign-SMS "],
        ),
        MapCase::new(
            "the catalog value of another enum",
            &["MONDAY"],
            &["MONDAY"],
        ),
        MapCase::new("the empty key", &[""], &[""]),
        MapCase::new(
            "one key outside the catalog beside two in it",
            &["Campaign-Email", "Campaign-Telephony", "Campaign-Pager"],
            &["Campaign-Pager"],
        ),
        MapCase::new(
            "two keys outside the catalog beside one in it",
            &["Campaign-Fax", "Campaign-Email", "campaign-sms"],
            &["Campaign-Fax", "campaign-sms"],
        ),
        MapCase::new(
            "three keys outside the catalog",
            &["Campaign-Fax", "Campaign-Pager", "Campaign-Telex"],
            &["Campaign-Fax", "Campaign-Pager", "Campaign-Telex"],
        ),
    ]
}

#[tokio::test]
async fn an_object_type_key_outside_the_modelled_catalog_is_refused_and_stores_nothing() {
    let router = router();
    let instance = "integration-map-keys";
    onboard_instance(&router, REGION, instance).await;

    // What the instance holds, grown by each put that is ACCEPTED: every refused put is
    // asserted to leave exactly this. A put under an identity the instance already
    // holds is an update, and every row here carries the same domain ARN, so the
    // accepted rows replace one record rather than adding to it.
    let mut expected_stored: Vec<Value> = Vec::new();

    for row in map_cases() {
        let MapCase {
            case,
            keys,
            offenders,
        } = row;
        let config = profiles_config(&keys);
        let (status, body) = put_integration(&router, instance, &config).await;

        if offenders.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the put, got {status} {body}"
            );
            expected_stored = vec![config.clone()];
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from the put, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {}",
                error_code(&body)
            );
            assert_eq!(
                message_of(&body),
                expected_message(OBJECT_TYPE_NAMES_MEMBER, EVENT_TYPE_VALUES, &offenders),
                "case {case:?}: the refusal was worded {:?}",
                message_of(&body)
            );
        }

        // Nothing of a refused map is written — not even the admissible keys beside the
        // inadmissible one — and an accepted map is exactly what the list reports.
        let stored = stored_integrations(&router, instance, case).await;
        assert_eq!(
            stored, expected_stored,
            "case {case:?}: expected the instance to hold {expected_stored:?}, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_key_outside_the_catalog_is_refused_before_the_instance_is_resolved() {
    let router = router();

    // A bad request is a bad request whatever instance it names: the map's keys are
    // weighed before anything is looked up, so an id no onboarded instance answers to
    // is still HTTP 400 rather than the 404 a well-formed put against it would get.
    for case in ["never-onboarded", "integration-map-keys-absent", "x"] {
        let (status, body) =
            put_integration(&router, case, &profiles_config(&["Campaign-Fax"])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a bad map key, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );

        // And a well-formed put against the same id IS the 404, so the 400 above came
        // from the map key and not from the instance.
        let (status, body) =
            put_integration(&router, case, &profiles_config(&["Campaign-Email"])).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for an instance that is not onboarded, \
             got {status} {body}"
        );
    }
}

#[tokio::test]
async fn the_sibling_enum_keyed_map_is_refused_by_the_same_rule() {
    let router = router();
    let instance = "integration-map-keys-daily-hours";
    onboard_instance(&router, REGION, instance).await;

    // A `dailyHours` map is keyed by `DayOfWeek`, the other enumerated key shape this
    // service accepts, so the same rule refuses it with the same sentence — built from
    // its own catalog and its own member path. The rows span the same ground as the
    // object-type rows: one key off the catalog, a catalog value in the wrong case, the
    // empty key, two off-catalog keys beside an admissible one, and a catalog value the
    // other rows never use, which is accepted.
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("a day the catalog does not hold", vec!["FUNDAY"], vec!["FUNDAY"]),
        ("a catalog value in the wrong case", vec!["monday"], vec!["monday"]),
        ("the empty key", vec![""], vec![""]),
        (
            "two days off the catalog beside one in it",
            vec!["FRIDAY", "TUE", "funday"],
            vec!["TUE", "funday"],
        ),
        ("a catalog value", vec!["SATURDAY"], vec![]),
    ];

    for (case, days, offenders) in cases {
        let mut daily_hours = Map::new();
        for day in &days {
            daily_hours.insert(
                (*day).to_string(),
                json!([{"startTime": "T09:00", "endTime": "T17:00"}]),
            );
        }
        let config = json!({
            "localTimeZoneConfig": {"defaultTimeZone": "UTC"},
            "telephony": {"openHours": {"dailyHours": daily_hours}},
        });
        let (status, body) = call(
            &router,
            REGION,
            "PUT",
            "/v2/campaigns",
            &json!({
                "name": format!("daily hours {case}"),
                "connectInstanceId": instance,
                "channelSubtypeConfig": {
                    "telephony": {
                        "outboundMode": {"agentless": {}},
                        "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"},
                    }
                },
                "communicationTimeConfig": config,
            }),
        )
        .await;

        if offenders.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateCampaign, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from CreateCampaign, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {}",
                error_code(&body)
            );
            assert_eq!(
                message_of(&body),
                expected_message(DAILY_HOURS_MEMBER, DAY_OF_WEEK_VALUES, &offenders),
                "case {case:?}: the refusal was worded {:?}",
                message_of(&body)
            );
        }
    }
}

#[tokio::test]
async fn the_modelled_bounds_inside_the_integration_config_hold() {
    let router = router();
    let instance = "integration-map-keys-bounds";
    onboard_instance(&router, REGION, instance).await;

    // (case, the config, whether the model admits it). The bounds come from
    // `smithy_query(query="shape", shape="Arn,LambdaArn,ObjectTypeName")`: `Arn` is
    // @length 20..500 with an ARN @pattern, `LambdaArn` @length 1..140 with a Lambda
    // function @pattern, and `ObjectTypeName` @length 1..255. Each published bound is
    // exercised, and one value past it.
    let long_name = "o".repeat(255);
    let too_long_name = "o".repeat(256);
    let arn_500 = padded_arn(DOMAIN_ARN_HEAD, 500);
    let arn_501 = padded_arn(DOMAIN_ARN_HEAD, 501);
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "a domain ARN at the 20-character lower bound",
            // Hand-written rather than padded: 20 characters is shorter than the head
            // `padded_arn` builds from.
            json!({"customerProfiles": {
                "domainArn": "arn:a:a:aa-aa-1:a:xx",
                "objectTypeNames": {"Campaign-Email": "object-type"},
            }}),
            true,
        ),
        (
            "a domain ARN one character under the lower bound",
            json!({"customerProfiles": {
                "domainArn": "arn:a:a:aa-aa-1:a:x",
                "objectTypeNames": {"Campaign-Email": "object-type"},
            }}),
            false,
        ),
        (
            "a domain ARN at the 500-character upper bound",
            json!({"customerProfiles": {
                "domainArn": arn_500,
                "objectTypeNames": {"Campaign-Email": "object-type"},
            }}),
            true,
        ),
        (
            "a domain ARN one character past the upper bound",
            json!({"customerProfiles": {
                "domainArn": arn_501,
                "objectTypeNames": {"Campaign-Email": "object-type"},
            }}),
            false,
        ),
        (
            "a domain ARN that is not an ARN at all",
            json!({"customerProfiles": {
                "domainArn": "not-an-arn-but-long-enough",
                "objectTypeNames": {"Campaign-Email": "object-type"},
            }}),
            false,
        ),
        (
            "an object type name at the 255-character upper bound",
            json!({"customerProfiles": {
                "domainArn": DOMAIN_ARN,
                "objectTypeNames": {"Campaign-SMS": long_name},
            }}),
            true,
        ),
        (
            "an object type name one character past the upper bound",
            json!({"customerProfiles": {
                "domainArn": DOMAIN_ARN,
                "objectTypeNames": {"Campaign-SMS": too_long_name},
            }}),
            false,
        ),
        (
            "an empty object type name",
            json!({"customerProfiles": {
                "domainArn": DOMAIN_ARN,
                "objectTypeNames": {"Campaign-SMS": ""},
            }}),
            false,
        ),
        (
            "a Lambda function ARN",
            json!({"lambda": {
                "functionArn": "arn:aws:lambda:eu-west-1:000000000000:function:bounds-handler",
            }}),
            true,
        ),
        (
            "a Lambda function ARN that names no function",
            json!({"lambda": {
                "functionArn": "arn:aws:lambda:eu-west-1:000000000000:queue:bounds-handler",
            }}),
            false,
        ),
        (
            "a knowledge base ARN",
            json!({"qConnect": {
                "knowledgeBaseArn":
                    "arn:aws:wisdom:eu-west-1:000000000000:knowledge-base/kb-0001",
            }}),
            true,
        ),
        (
            "a knowledge base ARN under the lower bound",
            json!({"qConnect": {"knowledgeBaseArn": "arn:a:a:aa-aa-1:a:x"}}),
            false,
        ),
    ];

    for (case, config, admitted) in cases {
        let before = stored_integrations(&router, instance, case).await;
        let (status, body) = put_integration(&router, instance, &config).await;

        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the put, got {status} {body}"
            );
            let stored = stored_integrations(&router, instance, case).await;
            assert!(
                stored.contains(&config),
                "case {case:?}: expected the list to report {config}, got {stored:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from the put, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {}",
                error_code(&body)
            );
            let stored = stored_integrations(&router, instance, case).await;
            assert_eq!(
                stored, before,
                "case {case:?}: a refused put must store nothing, {before:?} -> {stored:?}"
            );
        }
    }
}
