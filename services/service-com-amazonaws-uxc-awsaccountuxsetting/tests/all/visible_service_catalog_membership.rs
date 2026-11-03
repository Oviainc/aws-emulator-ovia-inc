// SPDX-License-Identifier: CC0-1.0

//! Which `visibleServices` values `UpdateAccountCustomizations` admits, end to
//! end over the wire (the shared harness is [`crate::wire`]).
//!
//! The rule: membership in the catalog the service itself enumerates is part of a
//! `visibleServices` value's validity, so an identifier that satisfies every
//! modelled constraint on `Service` but names no entry of that catalog is refused
//! with HTTP 400 `ValidationException`, and the record the account already had is
//! left exactly as it was. `ListServices` "returns a paginated list of ... service
//! identifiers that you can use as values for the `visibleServices` setting" and
//! the member's own doc sends a caller there "for valid service identifiers"
//! (`smithy_query(query="operation", operation="UpdateAccountCustomizations")`,
//! `smithy_query(query="doc", shape="ListServices")`), which is the same list in
//! both directions: everything a caller can discover, it can set, and nothing
//! else.
//!
//! Every refused row here is first asserted to be a value the MODEL admits
//! ([`crate::wire::is_legal_service`]) and absent from the catalog the emulator
//! publishes, so each row exercises the membership rule and not the shape rule —
//! whose refusals arrive in a different channel (HTTP 400
//! `BadRequestException`, at this service's front door) and are
//! `body_validation_refusals`' subject. The contrast is asserted below too.
//!
//! The accepted side of the rule — every catalog entry is settable — lives in
//! `account_customizations` and `visible_services_round_trip`; what this suite
//! adds on that side is the pairwise contrast between a catalog entry and a
//! near miss of it.

use crate::support::members;
use crate::wire::{
    build_router, catalog_services, generated_service_id, get_customizations, is_legal_service,
    legal_service, strings, update_customizations, whole_catalog,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The error code this service's model declares for an input that fails its
/// constraints (`smithy_query(query="operation",
/// operation="UpdateAccountCustomizations")`), and the channel the membership
/// refusal travels in.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code a request BODY outside a MODELLED constraint is refused with
/// instead — the front door's, before any handler runs. Named here only for the
/// contrast: the two refusals must stay distinguishable.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The recorded wording of the membership refusal.
const INVALID_SERVICES: &str = "Invalid services provided";

/// Identifiers that are SigV4 signing names / endpoint prefixes rather than
/// console service identifiers, so no console service home is published under
/// them and the catalog does not name them. Their source is the namespace
/// contrast `service_catalog` states from the `ListServices` doc (the catalog is
/// the console identifier namespace, not the endpoint-prefix one); each row is
/// re-checked against the catalog this emulator publishes before it is used.
const ENDPOINT_PREFIXES: &[&str] = &[
    "cognito-idp",
    "logs",
    "monitoring",
    "elasticmapreduce",
    "elasticfilesystem",
    "execute-api",
    "sts",
    "cassandra",
    "es",
];

/// Prime the account with a complete, settable record, and return it as the state
/// every refusal below must leave untouched.
async fn prime(router: &MultiServiceRouter) -> Value {
    let primed = catalog_services(router, 3).await;
    let (status, error_type, body) = update_customizations(
        router,
        json!({
            "accountColor": "red",
            "visibleServices": primed,
            "visibleRegions": ["eu-west-1", "us-east-1"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "priming the record must succeed, got {status} [{error_type}] ({body})"
    );
    body
}

/// One row of the refusal table: a case name and the `visibleServices` sequence
/// to supply.
struct Row {
    case: String,
    supplied: Vec<String>,
}

/// The table the rule is asserted over. It spans the FORM of the non-member (a
/// generated identifier, one of each length bound the model admits, an invented
/// hyphenated name, a near miss of a catalog entry at both ends, an endpoint
/// prefix the console namespace does not name), its POSITION in the list (alone,
/// first, middle, last), the list's SIZE (1 to 5), and HOW MANY of its members are
/// non-members (one of several, several of several, all of them) — which is what
/// the rule generalises over.
async fn refusal_rows(catalog: &[String]) -> Vec<Row> {
    let settable = catalog_services(&build_router(), 3).await;
    let first = settable[0].clone();
    let second = settable[1].clone();
    let third = settable[2].clone();

    // Non-members of every form. `legal_service` walks the model's own pattern,
    // so these are values the shape layer admits.
    let mut outsiders: Vec<(String, String)> = vec![
        ("the harness's generated identifier".to_string(), generated_service_id(7)),
        (
            "one character, the model's @length minimum".to_string(),
            "z".to_string(),
        ),
        (
            "64 characters, the model's @length maximum".to_string(),
            "a".repeat(64),
        ),
        (
            "an invented hyphenated name".to_string(),
            "zzqq-no-such-service".to_string(),
        ),
        (
            "a catalog entry with a digit appended".to_string(),
            format!("{first}0"),
        ),
        (
            "a catalog entry with another segment appended".to_string(),
            format!("{second}-console"),
        ),
        (
            "a catalog entry with its last character dropped".to_string(),
            third[..third.len() - 1].to_string(),
        ),
    ];
    for n in [3_usize, 11, 23] {
        outsiders.push((format!("the generator's {n}th pattern-legal value"), legal_service(n)));
    }
    for prefix in ENDPOINT_PREFIXES {
        outsiders.push((
            format!("the endpoint prefix {prefix:?}"),
            (*prefix).to_string(),
        ));
    }
    // Near misses spread across the whole catalog, not only its head.
    for (index, entry) in catalog.iter().enumerate().filter(|(index, _)| index % 37 == 5) {
        outsiders.push((
            format!("a digit appended to catalog entry {index}"),
            format!("{entry}9"),
        ));
    }

    let mut rows: Vec<Row> = Vec::new();
    for (name, outsider) in &outsiders {
        assert!(
            is_legal_service(outsider),
            "case {name:?}: a row must itself satisfy Service @length(1, 64) and \
             @pattern ^[a-z0-9]+(-[a-z0-9]+)*$, got {outsider:?}"
        );
        assert!(
            !catalog.contains(outsider),
            "case {name:?}: a row must be absent from the catalog to exercise membership, \
             but {outsider:?} is at index {:?}",
            catalog.iter().position(|entry| entry == outsider)
        );

        rows.push(Row {
            case: format!("{name}, alone"),
            supplied: vec![outsider.clone()],
        });
        rows.push(Row {
            case: format!("{name}, first of three"),
            supplied: vec![outsider.clone(), first.clone(), second.clone()],
        });
        rows.push(Row {
            case: format!("{name}, in the middle of three"),
            supplied: vec![first.clone(), outsider.clone(), second.clone()],
        });
        rows.push(Row {
            case: format!("{name}, last of three"),
            supplied: vec![first.clone(), second.clone(), outsider.clone()],
        });
    }

    // Several non-members in one list, and a list of nothing but non-members.
    let (_, one) = &outsiders[0];
    let (_, two) = &outsiders[3];
    let (_, three) = &outsiders[4];
    rows.push(Row {
        case: "two non-members around three catalog entries".to_string(),
        supplied: vec![
            one.clone(),
            first.clone(),
            second.clone(),
            third.clone(),
            two.clone(),
        ],
    });
    rows.push(Row {
        case: "nothing but non-members".to_string(),
        supplied: vec![one.clone(), two.clone(), three.clone()],
    });
    rows.push(Row {
        case: "the same non-member named twice".to_string(),
        supplied: vec![two.clone(), first.clone(), two.clone()],
    });

    rows
}

/// The rule, over that table: the request is refused with HTTP 400 and the
/// modelled `ValidationException` carrying the recorded wording, the record the
/// account already had is unchanged, and the account is not wedged — a settable
/// update straight afterwards still succeeds.
#[tokio::test]
async fn an_identifier_the_catalog_does_not_name_is_refused_and_nothing_is_written() {
    let catalog = whole_catalog(&build_router()).await;
    let rows = refusal_rows(&catalog).await;
    assert!(
        rows.len() > 20,
        "the table must span the class, got {} rows",
        rows.len()
    );

    for Row { case, supplied } in rows {
        let router = build_router();
        let primed = prime(&router).await;

        let (status, error_type, answered) =
            update_customizations(&router, json!({"visibleServices": supplied})).await;
        assert_eq!(
            status, 400,
            "case {case:?} {supplied:?}: expected HTTP 400, got {status} ({answered})"
        );
        assert_eq!(
            error_type, VALIDATION_EXCEPTION,
            "case {case:?} {supplied:?}: expected the modelled {VALIDATION_EXCEPTION}, got \
             {error_type:?} ({answered})"
        );
        assert_eq!(
            answered.get("message").and_then(Value::as_str),
            Some(INVALID_SERVICES),
            "case {case:?} {supplied:?}: expected the message {INVALID_SERVICES:?}, got \
             {answered:?}"
        );
        assert!(
            answered.get("fieldList").is_none(),
            "case {case:?} {supplied:?}: this refusal breaks no member's own modelled \
             constraint, so it names no field, got {answered:?}"
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched, primed,
            "case {case:?} {supplied:?}: a refused update must leave the record untouched, got \
             {fetched:?}"
        );

        let settable = catalog_services(&router, 2).await;
        let (status, _, recovered) =
            update_customizations(&router, json!({"visibleServices": settable.clone()})).await;
        assert_eq!(
            status, 200,
            "case {case:?} {supplied:?}: a settable update after a refusal must succeed, got \
             {status} ({recovered})"
        );
        assert_eq!(
            members(&recovered, "visibleServices"),
            settable,
            "case {case:?} {supplied:?}: the settable update must take effect, got {recovered:?}"
        );
    }
}

/// Nothing in a refused body is applied — not the settings the same request named
/// beside the refused one. Rows differ along which other settings travel with the
/// non-member.
#[tokio::test]
async fn a_refused_request_applies_none_of_its_other_settings() {
    let outsider = generated_service_id(2);
    let cases: Vec<(&str, Value)> = vec![
        (
            "a colour beside the refused list",
            json!({"accountColor": "green", "visibleServices": [outsider.clone()]}),
        ),
        (
            "Region codes beside the refused list",
            json!({"visibleServices": [outsider.clone()], "visibleRegions": ["ap-south-1"]}),
        ),
        (
            "every setting at once",
            json!({
                "accountColor": "pink",
                "visibleServices": [outsider.clone()],
                "visibleRegions": ["sa-east-1", "us-west-2"],
            }),
        ),
    ];

    for (case, body) in cases {
        let router = build_router();
        let primed = prime(&router).await;

        let (status, error_type, answered) = update_customizations(&router, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({answered})"
        );
        assert_eq!(
            error_type, VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {error_type:?} ({answered})"
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched, primed,
            "case {case:?}: no setting of a refused request may be applied, got {fetched:?}"
        );
    }
}

/// Membership is what decides, not the shape of the identifier: for catalog
/// entries spread across the catalog, the entry itself is accepted and read back
/// while a near miss of that very entry — the same string with one legal
/// character appended, so the same form and the same constraints — is refused.
#[tokio::test]
async fn a_catalog_entry_is_accepted_where_a_near_miss_of_it_is_refused() {
    let catalog = whole_catalog(&build_router()).await;
    assert!(
        catalog.len() > 50,
        "the catalog must hold enough entries to sample across, got {}",
        catalog.len()
    );

    for entry in catalog.iter().step_by(catalog.len() / 12) {
        let near_miss = format!("{entry}1");
        assert!(
            is_legal_service(&near_miss) && !catalog.contains(&near_miss),
            "case {entry:?}: the near miss {near_miss:?} must be model-legal and absent from \
             the catalog"
        );

        let router = build_router();
        let (status, error_type, accepted) =
            update_customizations(&router, json!({"visibleServices": [entry]})).await;
        assert_eq!(
            status, 200,
            "case {entry:?}: a catalog entry must be settable, got {status} [{error_type}] \
             ({accepted})"
        );
        assert_eq!(
            accepted.get("visibleServices"),
            Some(&json!([entry])),
            "case {entry:?}: the accepted entry must be read back, got {accepted:?}"
        );

        let (status, error_type, refused) =
            update_customizations(&router, json!({"visibleServices": [near_miss.clone()]})).await;
        assert_eq!(
            status, 400,
            "case {near_miss:?}: a near miss must be refused, got {status} ({refused})"
        );
        assert_eq!(
            error_type, VALIDATION_EXCEPTION,
            "case {near_miss:?}: expected {VALIDATION_EXCEPTION}, got {error_type:?} ({refused})"
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get("visibleServices"),
            Some(&json!([entry])),
            "case {near_miss:?}: the refused update must leave the entry stored, got {fetched:?}"
        );
    }
}

/// The two refusals stay distinguishable: a value outside a MODELLED constraint is
/// the front door's `BadRequestException`, and a body that breaks both a modelled
/// constraint and the membership rule is refused as the body violation it is —
/// the front door never reaches the handler.
///
/// Rows pair a shape violation with a non-member in one request, at each position,
/// and name the shape violation alone as the control.
#[tokio::test]
async fn a_shape_violation_in_the_same_list_keeps_the_front_doors_code() {
    let outsider = generated_service_id(5);
    let settable = catalog_services(&build_router(), 1).await;
    let entry = settable[0].clone();

    // (case, the supplied sequence, the expected error code)
    let cases: Vec<(&str, Vec<String>, &str)> = vec![
        (
            "a shape violation alone",
            vec!["Invalid_Service!".to_string()],
            BAD_REQUEST_EXCEPTION,
        ),
        (
            "a shape violation before a non-member",
            vec!["S3".to_string(), outsider.clone()],
            BAD_REQUEST_EXCEPTION,
        ),
        (
            "a non-member before a shape violation",
            vec![outsider.clone(), "ec2-".to_string()],
            BAD_REQUEST_EXCEPTION,
        ),
        (
            "a shape violation between a catalog entry and a non-member",
            vec![entry.clone(), "".to_string(), outsider.clone()],
            BAD_REQUEST_EXCEPTION,
        ),
        (
            "a non-member alone",
            vec![outsider.clone()],
            VALIDATION_EXCEPTION,
        ),
        (
            "a non-member beside a catalog entry",
            vec![entry.clone(), outsider.clone()],
            VALIDATION_EXCEPTION,
        ),
    ];

    for (case, supplied, expected_code) in cases {
        let router = build_router();
        let primed = prime(&router).await;

        let borrowed: Vec<&str> = supplied.iter().map(String::as_str).collect();
        let (status, error_type, answered) =
            update_customizations(&router, json!({"visibleServices": strings(&borrowed)})).await;
        assert_eq!(
            status, 400,
            "case {case:?} {supplied:?}: expected HTTP 400, got {status} ({answered})"
        );
        assert_eq!(
            error_type, expected_code,
            "case {case:?} {supplied:?}: expected the error code {expected_code}, got \
             {error_type:?} ({answered})"
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched, primed,
            "case {case:?} {supplied:?}: either refusal must leave the record untouched, got \
             {fetched:?}"
        );
    }
}

/// Resetting the setting is not a membership question: an empty list names no
/// identifier, so it is accepted, and so is a request that omits the member
/// altogether.
#[tokio::test]
async fn resetting_the_setting_names_no_identifier_and_is_accepted() {
    let router = build_router();
    prime(&router).await;

    let (status, error_type, emptied) =
        update_customizations(&router, json!({"visibleServices": []})).await;
    assert_eq!(
        status, 200,
        "an empty list must be accepted, got {status} [{error_type}] ({emptied})"
    );
    assert_eq!(
        emptied.get("visibleServices"),
        Some(&json!([])),
        "an empty list must be stored as empty, got {emptied:?}"
    );

    let (status, _, colour_only) =
        update_customizations(&router, json!({"accountColor": "teal"})).await;
    assert_eq!(
        status, 200,
        "an update naming no service list must be accepted, got {status} ({colour_only})"
    );
}
