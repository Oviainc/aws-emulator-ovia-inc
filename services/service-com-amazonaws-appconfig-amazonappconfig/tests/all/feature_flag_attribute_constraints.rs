// SPDX-License-Identifier: CC0-1.0

//! A feature flag's declared attribute constraints, as the gate on AUTHORING its values.
//!
//! RULE: a hosted configuration version written to a configuration profile of
//! `state::FEATURE_FLAGS_PROFILE_TYPE` carries both halves of a flag — the attribute
//! CONSTRAINTS the flag declares and the VALUES authored for it — so a document whose
//! values violate what the same document declares is refused with the modelled
//! `BadRequestException` (HTTP 400) and nothing is stored: no version number is consumed,
//! so the next accepted write still gets the next sequential `VersionNumber`. The same
//! value under a constraint that admits it is accepted, and the same bytes written to a
//! profile of any other `Type` are configuration data no flag schema governs, so they are
//! stored.
//!
//! The rows span the constraint kinds the AppConfig User Guide's Type reference for
//! `AWS.AppConfig.FeatureFlags` declares and what each one generalises over: a number
//! attribute's `minimum`/`maximum` (inside the range, at each published bound, one past
//! each bound and far past one), a string attribute's `enum` and `pattern`, a boolean
//! attribute, an array attribute's per-item `elements` (including an element
//! expression), whether an attribute is `required`, the JSON type of the authored value,
//! an attribute that declares no constraints at all, a declaration the emulator cannot
//! evaluate, and the number of declared expressions at the emulator's published bound and
//! one past it. The out-of-range number the recorded defect cited is one row among them,
//! beside the identical document under a relaxed `maximum`.

use serde_json::{json, Map, Value};
use service_com_amazonaws_appconfig_amazonappconfig::state::feature_flags::{
    INVALID_FEATURE_FLAGS_CONTENT_MESSAGE, MAX_PATTERN_CONSTRAINTS,
};
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    FEATURE_FLAGS_PROFILE_TYPE, FREEFORM_PROFILE_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_profile_from, json_call, router, send, Answer,
};

/// One document the rule is asserted over.
struct Row {
    case: &'static str,
    document: Value,
    /// Whether a feature-flags profile stores it: `true` when the authored values satisfy
    /// the constraints the same document declares.
    accepted: bool,
}

/// A document declaring one flag with `attributes`, configured with `values`.
fn document(attributes: Value, values: Value) -> Value {
    json!({
        "version": "1",
        "flags": { "pctflag": { "name": "pctflag", "attributes": attributes } },
        "values": { "pctflag": values },
    })
}

/// The rows the rule is spanned over. The constraint keywords come from the guide's
/// attribute-constraints schema: `type`, a number's `minimum`/`maximum`, a string's
/// `enum` and `pattern`, an array's `elements`, and `required`.
fn rows() -> Vec<Row> {
    // One declaration, re-used so that the rows below differ only in the VALUE authored
    // against it: a number bounded to 0..=100.
    let percent = json!({ "percent": { "constraints": { "type": "number", "minimum": 0, "maximum": 100 } } });
    vec![
        Row {
            case: "a number inside the declared range",
            document: document(percent.clone(), json!({ "enabled": true, "percent": 50 })),
            accepted: true,
        },
        Row {
            case: "a number at the declared minimum, which bounds inclusively",
            document: document(percent.clone(), json!({ "enabled": true, "percent": 0 })),
            accepted: true,
        },
        Row {
            case: "a number at the declared maximum, which bounds inclusively",
            document: document(percent.clone(), json!({ "enabled": true, "percent": 100 })),
            accepted: true,
        },
        Row {
            case: "a number one past the declared maximum",
            document: document(percent.clone(), json!({ "enabled": true, "percent": 101 })),
            accepted: false,
        },
        Row {
            case: "a number far past the declared maximum",
            document: document(percent.clone(), json!({ "enabled": true, "percent": 4096 })),
            accepted: false,
        },
        Row {
            case: "a fractional number just past the declared maximum",
            document: document(percent.clone(), json!({ "enabled": true, "percent": 100.5 })),
            accepted: false,
        },
        Row {
            case: "a number one below the declared minimum",
            document: document(percent.clone(), json!({ "enabled": true, "percent": -1 })),
            accepted: false,
        },
        Row {
            case: "the same value under a maximum that admits it",
            document: document(
                json!({ "percent": { "constraints": { "type": "number", "maximum": 4096 } } }),
                json!({ "enabled": true, "percent": 101 }),
            ),
            accepted: true,
        },
        Row {
            case: "a number past the declared maximum on a DISABLED flag",
            document: document(percent.clone(), json!({ "enabled": false, "percent": 101 })),
            accepted: false,
        },
        Row {
            case: "a string where a number attribute is declared",
            document: document(
                percent.clone(),
                json!({ "enabled": true, "percent": "fifty" }),
            ),
            accepted: false,
        },
        Row {
            case: "a boolean where a number attribute is declared",
            document: document(percent.clone(), json!({ "enabled": true, "percent": true })),
            accepted: false,
        },
        Row {
            case: "a string the declared enum lists",
            document: document(
                json!({ "tier": { "constraints": { "type": "string", "enum": ["beta", "ga"] } } }),
                json!({ "enabled": true, "tier": "ga" }),
            ),
            accepted: true,
        },
        Row {
            case: "a string the declared enum does not list",
            document: document(
                json!({ "tier": { "constraints": { "type": "string", "enum": ["beta", "ga"] } } }),
                json!({ "enabled": true, "tier": "alpha" }),
            ),
            accepted: false,
        },
        Row {
            case: "a string the declared pattern matches",
            document: document(
                json!({ "region": { "constraints": { "type": "string", "pattern": "^[a-z]{2}-[a-z]+-[0-9]$" } } }),
                json!({ "enabled": true, "region": "us-east-1" }),
            ),
            accepted: true,
        },
        Row {
            case: "a string the declared pattern does not match",
            document: document(
                json!({ "region": { "constraints": { "type": "string", "pattern": "^[a-z]{2}-[a-z]+-[0-9]$" } } }),
                json!({ "enabled": true, "region": "Narnia" }),
            ),
            accepted: false,
        },
        Row {
            case: "a boolean where a boolean attribute is declared",
            document: document(
                json!({ "shadow": { "constraints": { "type": "boolean" } } }),
                json!({ "enabled": true, "shadow": false }),
            ),
            accepted: true,
        },
        Row {
            case: "a string where a boolean attribute is declared",
            document: document(
                json!({ "shadow": { "constraints": { "type": "boolean" } } }),
                json!({ "enabled": true, "shadow": "false" }),
            ),
            accepted: false,
        },
        Row {
            case: "an array whose items all satisfy the declared element constraints",
            document: document(
                json!({ "weights": { "constraints": { "type": "array", "elements": { "type": "number", "maximum": 10 } } } }),
                json!({ "enabled": true, "weights": [1, 2, 10] }),
            ),
            accepted: true,
        },
        Row {
            case: "an array holding one item past the declared element maximum",
            document: document(
                json!({ "weights": { "constraints": { "type": "array", "elements": { "type": "number", "maximum": 10 } } } }),
                json!({ "enabled": true, "weights": [1, 2, 11] }),
            ),
            accepted: false,
        },
        Row {
            case: "an array holding one item of the wrong element type",
            document: document(
                json!({ "weights": { "constraints": { "type": "array", "elements": { "type": "number" } } } }),
                json!({ "enabled": true, "weights": [1, "two"] }),
            ),
            accepted: false,
        },
        Row {
            case: "a value where an array attribute is declared",
            document: document(
                json!({ "weights": { "constraints": { "type": "array" } } }),
                json!({ "enabled": true, "weights": 1 }),
            ),
            accepted: false,
        },
        Row {
            case: "a required attribute the values carry",
            document: document(
                json!({ "tier": { "constraints": { "type": "string", "required": true } } }),
                json!({ "enabled": true, "tier": "ga" }),
            ),
            accepted: true,
        },
        Row {
            case: "a required attribute the values do not carry",
            document: document(
                json!({ "tier": { "constraints": { "type": "string", "required": true } } }),
                json!({ "enabled": true }),
            ),
            accepted: false,
        },
        Row {
            case: "an attribute that is not required and is not configured",
            document: document(
                json!({ "tier": { "constraints": { "type": "string" } } }),
                json!({ "enabled": true }),
            ),
            accepted: true,
        },
        Row {
            case: "any value under an attribute that declares no constraints",
            document: document(
                json!({ "anything": { "description": "unconstrained" } }),
                json!({ "enabled": true, "anything": [{ "deeply": "nested" }] }),
            ),
            accepted: true,
        },
        Row {
            case: "an array whose items all match the declared element expression",
            document: document(
                json!({ "regions": { "constraints": { "type": "array", "elements": { "type": "string", "pattern": "^[a-z]{2}-[a-z]+-[0-9]$" } } } }),
                json!({ "enabled": true, "regions": ["us-east-1", "eu-west-1"] }),
            ),
            accepted: true,
        },
        Row {
            case: "an array holding one item the declared element expression rejects",
            document: document(
                json!({ "regions": { "constraints": { "type": "array", "elements": { "type": "string", "pattern": "^[a-z]{2}-[a-z]+-[0-9]$" } } } }),
                json!({ "enabled": true, "regions": ["us-east-1", "Narnia"] }),
            ),
            accepted: false,
        },
        Row {
            case: "an expression that is no regular expression, so nothing was checked",
            document: document(
                json!({ "tier": { "constraints": { "type": "string", "pattern": "[" } } }),
                json!({ "enabled": true, "tier": "ga" }),
            ),
            accepted: false,
        },
        Row {
            case: "as many declared expressions as the emulator checks",
            document: declared_expressions(MAX_PATTERN_CONSTRAINTS),
            accepted: true,
        },
        Row {
            case: "one declared expression more than the emulator checks",
            document: declared_expressions(MAX_PATTERN_CONSTRAINTS + 1),
            accepted: false,
        },
        Row {
            case: "two attributes, one of them violated",
            document: document(
                json!({
                    "percent": { "constraints": { "type": "number", "maximum": 100 } },
                    "tier": { "constraints": { "type": "string", "enum": ["beta"] } },
                }),
                json!({ "enabled": true, "percent": 50, "tier": "ga" }),
            ),
            accepted: false,
        },
    ]
}

/// A document declaring `count` attributes that each carry the SAME expression constraint
/// and a value satisfying it, so two rows can differ only in how many expressions one
/// document declares: at the bound the emulator publishes
/// (`MAX_PATTERN_CONSTRAINTS`, the count past which a document is not CHECKED and so not
/// stored) and one past it.
fn declared_expressions(count: usize) -> Value {
    let mut attributes = Map::new();
    let mut values = Map::new();
    values.insert("enabled".to_string(), Value::Bool(true));
    for index in 0..count {
        let attribute = format!("a{index}");
        attributes.insert(
            attribute.clone(),
            json!({ "constraints": { "type": "string", "pattern": "^ok$" } }),
        );
        values.insert(attribute, Value::from("ok"));
    }
    document(Value::Object(attributes), Value::Object(values))
}

/// Create a hosted configuration profile of `profile_type` and return its id.
async fn typed_profile(
    router: &MultiServiceRouter,
    application: &str,
    name: &str,
    profile_type: &str,
) -> String {
    let created = create_profile_from(
        router,
        application,
        name,
        &json!({ "Name": name, "LocationUri": "hosted", "Type": profile_type }),
    )
    .await;
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: CreateConfigurationProfile answered no Id"))
}

/// `POST` one hosted configuration version holding `document`.
async fn write_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    document: &Value,
) -> Answer {
    let content = serde_json::to_vec(document).expect("the document must serialize");
    send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content,
        &[],
    )
    .await
}

/// The `VersionNumber` of every version `ListHostedConfigurationVersions` reports for a
/// profile, in ascending order. The order the listing itself serves is asserted by
/// `list_hosted_configuration_versions`; what matters here is WHICH versions exist.
async fn listed_versions(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
) -> Vec<i64> {
    let listing = json_call(
        router,
        "GET",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        &Value::Null,
    )
    .await;
    assert_eq!(
        listing.status,
        200,
        "ListHostedConfigurationVersions must answer 200, got {} / {}",
        listing.status,
        String::from_utf8_lossy(&listing.body)
    );
    listing
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            let mut numbers: Vec<i64> = items
                .iter()
                .filter_map(|item| item.get("VersionNumber").and_then(Value::as_i64))
                .collect();
            numbers.sort_unstable();
            numbers
        })
        .unwrap_or_default()
}

/// Every row, written to one feature-flags profile: an accepted document is stored and
/// takes the next version number, a violating one is refused with the extension's own
/// message and consumes nothing — and the same bytes on a free-form profile are stored
/// either way, because no flag schema governs them there.
#[tokio::test]
async fn a_flag_value_is_held_to_the_constraints_its_own_flag_declares() {
    let router = router();
    let application = create_application(&router, "flag-constraints").await;
    let flags = typed_profile(&router, &application, "flags", FEATURE_FLAGS_PROFILE_TYPE).await;
    let freeform =
        typed_profile(&router, &application, "freeform", FREEFORM_PROFILE_TYPE).await;

    // What the profile's version numbering is at: a refused write must not move it.
    let mut stored: Vec<i64> = Vec::new();
    for row in rows() {
        let case = row.case;
        let written = write_version(&router, &application, &flags, &row.document).await;

        if row.accepted {
            assert_eq!(
                written.status,
                201,
                "case {case:?}: a document whose values satisfy their constraints must be \
                 stored, got {} / {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
            let number = written
                .header("Version-Number")
                .and_then(|number| number.parse::<i64>().ok())
                .unwrap_or_else(|| panic!("case {case:?}: no Version-Number header"));
            assert_eq!(
                number,
                stored.last().copied().unwrap_or(0) + 1,
                "case {case:?}: an accepted write must take the next sequential version \
                 number, got {number}"
            );
            stored.push(number);
            // The stored bytes come back, so the accepted value really persisted rather
            // than being echoed.
            let read_back = json_call(
                &router,
                "GET",
                &format!(
                    "/applications/{application}/configurationprofiles/{flags}/hostedconfigurationversions/{number}"
                ),
                &Value::Null,
            )
            .await;
            assert_eq!(
                read_back.body.as_ref(),
                written.body.as_ref(),
                "case {case:?}: the stored document must come back byte for byte, got {}",
                String::from_utf8_lossy(&read_back.body)
            );
        } else {
            assert_eq!(
                written.status,
                400,
                "case {case:?}: a document whose values violate their own constraints must \
                 be refused, got {} / {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
            assert!(
                written.error_code().contains("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {:?}",
                written.error_code()
            );
            assert_eq!(
                written.string("Message").as_deref(),
                Some(INVALID_FEATURE_FLAGS_CONTENT_MESSAGE),
                "case {case:?}: expected the feature-flags helper's own message, got {:?}",
                written.string("Message")
            );
        }

        // No version number is consumed by a refusal: the profile holds exactly the
        // versions the accepted writes made.
        assert_eq!(
            listed_versions(&router, &application, &flags).await,
            stored,
            "case {case:?}: the profile must hold exactly the versions that were accepted"
        );

        // The same bytes on a profile that carries no flag schema are configuration data,
        // so they are stored whatever the flag constraints would have said.
        let elsewhere = write_version(&router, &application, &freeform, &row.document).await;
        assert_eq!(
            elsewhere.status,
            201,
            "case {case:?}: the same bytes on a free-form profile are not held to any flag \
             constraint, got {} / {}",
            elsewhere.status,
            String::from_utf8_lossy(&elsewhere.body)
        );
    }

    // Both halves of the rule were exercised, so neither branch went untested.
    assert!(
        rows().iter().any(|row| row.accepted) && rows().iter().any(|row| !row.accepted),
        "the table must hold both documents that satisfy their constraints and documents \
         that violate them"
    );
}
