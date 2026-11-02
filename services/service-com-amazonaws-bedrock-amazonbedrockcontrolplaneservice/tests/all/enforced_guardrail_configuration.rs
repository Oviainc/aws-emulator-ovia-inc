// SPDX-License-Identifier: CC0-1.0

//! The account's enforced guardrail configuration end to end:
//! `PutEnforcedGuardrailConfiguration`, `ListEnforcedGuardrailsConfiguration` and
//! `DeleteEnforcedGuardrailConfiguration` over HTTP, through the same `MultiServiceRouter`
//! the emulator boots.
//!
//! # The rule
//!
//! An account holds AT MOST ONE enforced guardrail configuration — the listing that
//! reports it is modelled `@length(min: 0, max: 1)` — so the put INSTALLS that one
//! record, naming a guardrail revision the account already holds, and the listing reports
//! back exactly what the last admitted put stored: the resolved guardrail's id and ARN,
//! the enforced version, and the `selectiveContentGuarding` and `modelEnforcement` the
//! request carried, member for member. A further put REPLACES the record, keeping its
//! `configId` and the instant and principal that first installed it; while nothing is
//! installed the listing answers 200 with an EMPTY list.
//!
//! # What the tables below generalise over
//!
//! The round-trip table spans the axes a put can differ along: which spelling of
//! `guardrailIdentifier` names the guardrail (the bare id and the full ARN), which
//! numbered version is enforced, whether `selectiveContentGuarding` is carried and which
//! of its two members and which of the two `SelectiveGuardingMode` values each takes, and
//! whether `modelEnforcement` is carried and with how many — and which — included and
//! excluded model ids. The model-id catalog is drawn from this service's own published
//! foundation models and from `IncludedModelId`'s published pattern, named per row.
//!
//! The refusal table drives the identifier, the version and the `configId` off the set
//! the account holds, one axis per row.

use serde_json::{json, Value};

use crate::support::{emulator, Emulator, Reply, ACCOUNT, REGION};

/// The URI both `PutEnforcedGuardrailConfiguration` and
/// `ListEnforcedGuardrailsConfiguration` are bound to
/// (`smithy_query(query="operation",operation="PutEnforcedGuardrailConfiguration")`).
const CONFIGURATION_URI: &str = "/enforcedGuardrailsConfiguration";

/// The principal these suites sign as: the harness signs every request with the access
/// key id `AKIDEXAMPLE`, and the emulator reports the IAM principal holding it as the
/// configuration's `createdBy` / `updatedBy`.
const CALLER: &str = "arn:aws:iam::000000000000:user/AKIDEXAMPLE";

/// One put this suite installs and reads back.
struct Case {
    name: &'static str,
    /// Whether the put names the guardrail by its full ARN rather than by its bare id.
    by_arn: bool,
    /// The numbered version the configuration enforces; the fixture cuts two.
    version: &'static str,
    selective_content_guarding: Option<Value>,
    model_enforcement: Option<Value>,
}

/// The configurations the rule must round-trip.
///
/// Model ids are real: `anthropic.claude-3-haiku-20240307-v1:0` and
/// `amazon.titan-text-lite-v1` are published rows of this service's own foundation
/// model catalog, `ALL` is the wildcard `IncludedModelId`'s published pattern admits
/// (`^(ALL|([a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63})…)$`), and
/// `meta.llama3-8b-instruct-v1:0/abcdef123456` carries the optional `/[a-z0-9]{12}`
/// suffix that pattern also admits.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "the bare id, version 1, nothing optional",
            by_arn: false,
            version: "1",
            selective_content_guarding: None,
            model_enforcement: None,
        },
        Case {
            name: "the full ARN, version 2, selective guarding on the system prompt only",
            by_arn: true,
            version: "2",
            selective_content_guarding: Some(json!({ "system": "SELECTIVE" })),
            model_enforcement: None,
        },
        Case {
            name: "messages guarded comprehensively, every model enforced",
            by_arn: false,
            version: "2",
            selective_content_guarding: Some(json!({ "messages": "COMPREHENSIVE" })),
            model_enforcement: Some(json!({
                "includedModels": ["ALL"],
                "excludedModels": [],
            })),
        },
        Case {
            name: "both guarding members, both modes, one included and one excluded model",
            by_arn: true,
            version: "1",
            selective_content_guarding: Some(json!({
                "system": "COMPREHENSIVE",
                "messages": "SELECTIVE",
            })),
            model_enforcement: Some(json!({
                "includedModels": ["anthropic.claude-3-haiku-20240307-v1:0"],
                "excludedModels": ["amazon.titan-text-lite-v1"],
            })),
        },
        Case {
            name: "several included models, a model id with its version-suffix form",
            by_arn: false,
            version: "1",
            selective_content_guarding: Some(json!({
                "system": "SELECTIVE",
                "messages": "SELECTIVE",
            })),
            model_enforcement: Some(json!({
                "includedModels": [
                    "meta.llama3-8b-instruct-v1:0/abcdef123456",
                    "amazon.titan-text-lite-v1",
                    "anthropic.claude-3-haiku-20240307-v1:0",
                ],
                "excludedModels": [
                    "amazon.titan-embed-text-v2:0",
                    "cohere.command-r-v1:0",
                ],
            })),
        },
    ]
}

/// The inference configuration a case puts, for the guardrail `identifier`.
fn inference_config(case: &Case, identifier: &str) -> Value {
    let mut config = json!({
        "guardrailIdentifier": identifier,
        "guardrailVersion": case.version,
    });
    if let Some(guarding) = &case.selective_content_guarding {
        config["selectiveContentGuarding"] = guarding.clone();
    }
    if let Some(enforcement) = &case.model_enforcement {
        config["modelEnforcement"] = enforcement.clone();
    }
    config
}

/// `PUT /enforcedGuardrailsConfiguration`.
async fn put(emulator: &Emulator, body: Value) -> Reply {
    emulator.call("PUT", CONFIGURATION_URI, Some(body)).await
}

/// `GET /enforcedGuardrailsConfiguration`, optionally with a `nextToken`.
async fn list(emulator: &Emulator, next_token: Option<&str>) -> Reply {
    let path = match next_token {
        Some(token) => format!("{CONFIGURATION_URI}?nextToken={token}"),
        None => CONFIGURATION_URI.to_string(),
    };
    emulator.call("GET", &path, None).await
}

/// The rows the listing reports.
fn rows(reply: &Reply) -> Vec<Value> {
    reply
        .get("guardrailsConfig")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// A guardrail with two numbered versions, as the id and the ARN both spellings of a
/// `guardrailIdentifier` use.
async fn guardrail_with_two_versions(emulator: &Emulator, name: &str) -> (String, String) {
    let id = emulator.create_named(name).await;
    for version in 1..=2 {
        let cut = emulator.create_version(&id, json!({})).await;
        assert_eq!(
            cut.status, 202,
            "case {name}: CreateGuardrailVersion {version} must answer 202, got {} {:?}",
            cut.status, cut.body
        );
    }
    let arn = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/{id}");
    let read = emulator.get(&id, None).await;
    assert_eq!(
        read.text("guardrailArn"),
        arn,
        "case {name}: the fixture's ARN must be the one the guardrail carries"
    );
    (id, arn)
}

#[tokio::test]
async fn a_listing_before_any_put_reports_no_configuration() {
    let emulator = emulator();
    let reply = list(&emulator, None).await;
    assert_eq!(
        reply.status, 200,
        "case unconfigured: the listing must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        rows(&reply),
        Vec::<Value>::new(),
        "case unconfigured: nothing is installed, so the listing is empty, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.get("nextToken"),
        Value::Null,
        "case unconfigured: a one-page listing issues no continuation token, got {:?}",
        reply.body
    );
}

#[tokio::test]
async fn every_configuration_is_reported_back_exactly_as_it_was_put() {
    for (index, case) in cases().into_iter().enumerate() {
        // One account per row: the account holds a single configuration, so each row is
        // an independent install rather than a replacement of the row before it.
        let emulator = emulator();
        // `GuardrailName` is `[0-9a-zA-Z-_]+`, so the fixture names the guardrail by the
        // row's ordinal; the row's own prose stays in the assertion messages.
        let (id, arn) =
            guardrail_with_two_versions(&emulator, &format!("enforced-row-{index}")).await;
        let identifier = if case.by_arn { arn.clone() } else { id.clone() };

        let installed = put(
            &emulator,
            json!({ "guardrailInferenceConfig": inference_config(&case, &identifier) }),
        )
        .await;
        assert_eq!(
            installed.status, 200,
            "case {}: the put must answer 200, got {} {:?}",
            case.name, installed.status, installed.body
        );
        let config_id = installed.text("configId");
        assert!(
            !config_id.is_empty()
                && config_id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
            "case {}: configId is modelled ^[a-z0-9]+$, got {config_id:?}",
            case.name
        );
        assert_eq!(
            installed.text("updatedBy"),
            CALLER,
            "case {}: updatedBy must report the calling principal, got {:?}",
            case.name,
            installed.get("updatedBy")
        );
        let installed_at = installed.instant("updatedAt");

        let listed = list(&emulator, None).await;
        assert_eq!(
            listed.status, 200,
            "case {}: the listing must answer 200, got {} {:?}",
            case.name, listed.status, listed.body
        );
        let listed_rows = rows(&listed);
        assert_eq!(
            listed_rows.len(),
            1,
            "case {}: one put installs exactly one configuration, got {:?}",
            case.name,
            listed.body
        );
        assert_eq!(
            listed.get("nextToken"),
            Value::Null,
            "case {}: a one-page listing issues no continuation token, got {:?}",
            case.name,
            listed.body
        );

        // The whole row, compared as one value: the resolved guardrail and the optional
        // members as the request carried them, and no member the request did not set.
        let mut expected = json!({
            "configId": config_id,
            "guardrailId": id,
            "guardrailArn": arn,
            "guardrailVersion": case.version,
            "owner": "ACCOUNT",
            "createdBy": CALLER,
            "updatedBy": CALLER,
            "createdAt": listed_rows[0]["createdAt"].clone(),
            "updatedAt": listed_rows[0]["updatedAt"].clone(),
        });
        if let Some(guarding) = &case.selective_content_guarding {
            expected["selectiveContentGuarding"] = guarding.clone();
        }
        if let Some(enforcement) = &case.model_enforcement {
            expected["modelEnforcement"] = enforcement.clone();
        }
        assert_eq!(
            listed_rows[0], expected,
            "case {}: the stored configuration must be reported member for member, got {:?}",
            case.name, listed_rows[0]
        );

        // The two instants the row carries are the put's own instant: the install is the
        // creation, and the row is reported as the put answered it.
        for member in ["createdAt", "updatedAt"] {
            let actual = crate::support::date_time_seconds(
                case.name,
                listed_rows[0][member]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {}: {member} must be a string", case.name)),
            );
            assert!(
                (actual - installed_at).abs() < 0.001,
                "case {}: {member} must be the instant the put reported ({installed_at}), got \
                 {actual}",
                case.name
            );
        }

        // Reading twice changes nothing: this operation is @readonly.
        let again = list(&emulator, None).await;
        assert_eq!(
            rows(&again),
            listed_rows,
            "case {}: a second listing must report the same configuration, got {:?}",
            case.name,
            again.body
        );
    }
}

#[tokio::test]
async fn a_further_put_replaces_the_one_configuration_and_keeps_its_identity() {
    let emulator = emulator();
    let (first_id, _) = guardrail_with_two_versions(&emulator, "replaced-first").await;
    let (second_id, second_arn) =
        guardrail_with_two_versions(&emulator, "replaced-second").await;

    let first = put(
        &emulator,
        json!({
            "guardrailInferenceConfig": {
                "guardrailIdentifier": first_id,
                "guardrailVersion": "1",
                "selectiveContentGuarding": { "system": "SELECTIVE" },
            },
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case first put: must answer 200, got {} {:?}",
        first.status, first.body
    );
    let config_id = first.text("configId");
    let installed_rows = rows(&list(&emulator, None).await);
    let created_at = installed_rows[0]["createdAt"].clone();
    assert!(
        created_at.is_string(),
        "case first put: the installed configuration must report a createdAt, got {:?}",
        installed_rows[0]
    );

    // Each replacement differs from the one before along a different axis: the enforced
    // guardrail, the version, whether the optional members are carried, and whether the
    // put names the configId it is replacing.
    let replacements: Vec<(&str, Value)> = vec![
        (
            "another version of the same guardrail, named without a configId",
            json!({
                "guardrailInferenceConfig": {
                    "guardrailIdentifier": first_id.clone(),
                    "guardrailVersion": "2",
                },
            }),
        ),
        (
            "another guardrail by ARN, naming the installed configId",
            json!({
                "configId": config_id.clone(),
                "guardrailInferenceConfig": {
                    "guardrailIdentifier": second_arn.clone(),
                    "guardrailVersion": "2",
                    "modelEnforcement": {
                        "includedModels": ["ALL"],
                        "excludedModels": ["amazon.titan-text-lite-v1"],
                    },
                },
            }),
        ),
        (
            "the same put again, which is idempotent",
            json!({
                "configId": config_id.clone(),
                "guardrailInferenceConfig": {
                    "guardrailIdentifier": second_arn.clone(),
                    "guardrailVersion": "2",
                    "modelEnforcement": {
                        "includedModels": ["ALL"],
                        "excludedModels": ["amazon.titan-text-lite-v1"],
                    },
                },
            }),
        ),
    ];

    for (case, body) in replacements {
        let replaced = put(&emulator, body.clone()).await;
        assert_eq!(
            replaced.status, 200,
            "case {case}: the put must answer 200, got {} {:?}",
            replaced.status, replaced.body
        );
        assert_eq!(
            replaced.text("configId"),
            config_id,
            "case {case}: an account holds one configuration, so its configId survives a \
             replacement, got {:?}",
            replaced.get("configId")
        );

        let listed = list(&emulator, None).await;
        let listed_rows = rows(&listed);
        assert_eq!(
            listed_rows.len(),
            1,
            "case {case}: a put replaces the configuration rather than adding one, got {:?}",
            listed.body
        );
        let row = &listed_rows[0];
        let config = body
            .get("guardrailInferenceConfig")
            .expect("the body carries an inference config");
        let expected_version = config["guardrailVersion"].as_str().unwrap_or_default();
        assert_eq!(
            row["guardrailVersion"].as_str().unwrap_or_default(),
            expected_version,
            "case {case}: the listing must report the version just put, got {:?}",
            row["guardrailVersion"]
        );
        let expected_id = if config["guardrailIdentifier"] == json!(second_arn) {
            &second_id
        } else {
            &first_id
        };
        assert_eq!(
            row["guardrailId"].as_str().unwrap_or_default(),
            expected_id.as_str(),
            "case {case}: the listing must report the guardrail just put, got {:?}",
            row["guardrailId"]
        );
        // A member the replacement did not carry is CLEARED, not merged in from the
        // configuration it replaced.
        assert_eq!(
            row.get("selectiveContentGuarding").cloned(),
            config.get("selectiveContentGuarding").cloned(),
            "case {case}: selectiveContentGuarding must be exactly what this put carried, got \
             {:?}",
            row.get("selectiveContentGuarding")
        );
        assert_eq!(
            row.get("modelEnforcement").cloned(),
            config.get("modelEnforcement").cloned(),
            "case {case}: modelEnforcement must be exactly what this put carried, got {:?}",
            row.get("modelEnforcement")
        );
        assert_eq!(
            row["createdAt"], created_at,
            "case {case}: the install instant survives a replacement, got {:?}",
            row["createdAt"]
        );
        assert_eq!(
            row["createdBy"].as_str(),
            Some(CALLER),
            "case {case}: the installing principal survives a replacement, got {:?}",
            row["createdBy"]
        );
        let created = crate::support::date_time_seconds(
            case,
            created_at.as_str().expect("createdAt is a string"),
        );
        let updated = crate::support::date_time_seconds(
            case,
            row["updatedAt"].as_str().expect("updatedAt is a string"),
        );
        assert!(
            updated >= created,
            "case {case}: updatedAt must be at or after createdAt, got {updated} and {created}"
        );
    }
}

#[tokio::test]
async fn a_put_naming_something_the_account_does_not_hold_is_refused_and_changes_nothing() {
    let emulator = emulator();
    let (id, arn) = guardrail_with_two_versions(&emulator, "refusals").await;
    let installed = put(
        &emulator,
        json!({
            "guardrailInferenceConfig": {
                "guardrailIdentifier": id.clone(),
                "guardrailVersion": "1",
            },
        }),
    )
    .await;
    assert_eq!(
        installed.status, 200,
        "the baseline put must answer 200, got {} {:?}",
        installed.status, installed.body
    );
    let baseline = rows(&list(&emulator, None).await);
    let config_id = installed.text("configId");

    // One axis per row: the guardrail the identifier names, the version it names, and
    // the configId the put addresses.
    let refused: Vec<(&str, Value)> = vec![
        (
            "a bare guardrail id no guardrail carries",
            json!({
                "guardrailInferenceConfig": {
                    "guardrailIdentifier": "abcdef012345",
                    "guardrailVersion": "1",
                },
            }),
        ),
        (
            "a guardrail ARN no guardrail carries",
            json!({
                "guardrailInferenceConfig": {
                    "guardrailIdentifier":
                        format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/abcdef012345"),
                    "guardrailVersion": "1",
                },
            }),
        ),
        (
            "a version one past the last the guardrail holds",
            json!({
                "guardrailInferenceConfig": {
                    "guardrailIdentifier": arn.clone(),
                    "guardrailVersion": "3",
                },
            }),
        ),
        (
            "a version far past it, inside the member's published pattern",
            json!({
                "guardrailInferenceConfig": {
                    "guardrailIdentifier": id.clone(),
                    "guardrailVersion": "99999999",
                },
            }),
        ),
        (
            "a configId the account does not hold",
            json!({
                "configId": format!("{config_id}0"),
                "guardrailInferenceConfig": {
                    "guardrailIdentifier": id.clone(),
                    "guardrailVersion": "2",
                },
            }),
        ),
    ];

    for (case, body) in refused {
        let reply = put(&emulator, body).await;
        assert_eq!(
            reply.status, 404,
            "case {case}: the put must be refused 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: a resource the account does not hold is a ResourceNotFoundException, \
             got {:?}",
            reply.error
        );
        assert_eq!(
            rows(&list(&emulator, None).await),
            baseline,
            "case {case}: a refused put must change nothing"
        );
    }

    // The DRAFT is outside `GuardrailNumericalVersion`'s published pattern
    // (`^[1-9][0-9]{0,7}$`), so it is a constraint violation rather than a missing
    // resource: an enforced configuration names a published version.
    let draft = put(
        &emulator,
        json!({
            "guardrailInferenceConfig": {
                "guardrailIdentifier": id.clone(),
                "guardrailVersion": "DRAFT",
            },
        }),
    )
    .await;
    assert_eq!(
        draft.status, 400,
        "case the DRAFT version: the put must be refused 400, got {} {:?}",
        draft.status, draft.body
    );
    assert_eq!(
        draft.error.as_deref(),
        Some("ValidationException"),
        "case the DRAFT version: a constraint violation is a ValidationException, got {:?}",
        draft.error
    );
    assert_eq!(
        rows(&list(&emulator, None).await),
        baseline,
        "case the DRAFT version: a refused put must change nothing"
    );
}

#[tokio::test]
async fn a_delete_removes_the_configuration_and_refuses_an_id_the_account_does_not_hold() {
    // `DeleteEnforcedGuardrailConfiguration` is bound to
    // `DELETE /enforcedGuardrailsConfiguration/{configId}` with `@http(code: 200)`
    // (`smithy_query(query="operation",operation="DeleteEnforcedGuardrailConfiguration")`).
    async fn delete(emulator: &Emulator, config_id: &str) -> Reply {
        emulator
            .call("DELETE", &format!("{CONFIGURATION_URI}/{config_id}"), None)
            .await
    }

    /// The `ResourceNotFoundException` message this delete answers for a `configId` the
    /// account holds no configuration under — Amazon Bedrock's wording for that error
    /// class, which carries no part of the request.
    const NOT_FOUND: &str = "No default guardrail configuration found for the specified config identifier.";

    let emulator = emulator();
    let (id, _) = guardrail_with_two_versions(&emulator, "deleted-configuration").await;
    let installed = put(
        &emulator,
        json!({
            "guardrailInferenceConfig": {
                "guardrailIdentifier": id.clone(),
                "guardrailVersion": "1",
            },
        }),
    )
    .await;
    assert_eq!(
        installed.status, 200,
        "case setup: the put must answer 200, got {} {:?}",
        installed.status, installed.body
    );
    let config_id = installed.text("configId");

    // Rows differ along WHICH `configId` the delete addresses, over the ids
    // `AccountEnforcedGuardrailConfigurationId` (`^[a-z0-9]+$`) admits: one the account
    // does not hold is refused while the installed configuration stays listed.
    for (case, addressed) in [
        ("an id nothing minted", "zzzzzzzzzzzz".to_string()),
        ("the installed id with a digit appended", format!("{config_id}0")),
    ] {
        let refused = delete(&emulator, &addressed).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: {addressed:?} addresses no configuration, so the delete must \
             answer 404, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.get("message"),
            json!(NOT_FOUND),
            "case {case:?}: the refusal must carry the service's wording for that class, got \
             {:?}",
            refused.body
        );
        assert_eq!(
            rows(&list(&emulator, None).await).len(),
            1,
            "case {case:?}: a refused delete must leave the configuration installed"
        );
    }

    let removed = delete(&emulator, &config_id).await;
    assert_eq!(
        removed.status, 200,
        "case \"the installed id\": the delete must answer 200, got {} {:?}",
        removed.status, removed.body
    );
    assert_eq!(
        rows(&list(&emulator, None).await),
        Vec::<Value>::new(),
        "case \"the installed id\": nothing is installed once the configuration is deleted"
    );

    // The account holds no configuration any more, so the same id addresses nothing and is
    // refused in the same sentence.
    let again = delete(&emulator, &config_id).await;
    assert_eq!(
        (again.status, again.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"deleted twice\": the second delete must answer 404, got {} {:?}",
        again.status,
        again.body
    );
    assert_eq!(
        again.get("message"),
        json!(NOT_FOUND),
        "case \"deleted twice\": the refusal must carry the same wording, got {:?}",
        again.body
    );
}

#[tokio::test]
async fn a_continuation_token_this_listing_never_issued_is_refused() {
    let emulator = emulator();
    let (id, _) = guardrail_with_two_versions(&emulator, "tokens").await;
    let installed = put(
        &emulator,
        json!({
            "guardrailInferenceConfig": {
                "guardrailIdentifier": id,
                "guardrailVersion": "1",
            },
        }),
    )
    .await;
    assert_eq!(
        installed.status, 200,
        "the baseline put must answer 200, got {} {:?}",
        installed.status, installed.body
    );

    // The listing holds at most one row, so it issues no token: every token is one it
    // never handed out, whatever it spells. Each row differs in length and content, and
    // the last two sit on the published bounds of `PaginationToken` (@length 1..2048).
    let tokens = [
        "x",
        installed.text("configId").as_str(),
        "eyJuZXh0IjoibmV4dCJ9",
        &"t".repeat(2048),
    ]
    .map(str::to_string);
    for token in tokens {
        let reply = list(&emulator, Some(&token)).await;
        assert_eq!(
            reply.status, 400,
            "case token {}: the listing must be refused 400, got {} {:?}",
            token.len(),
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case token {}: a token this listing never issued is a ValidationException, got {:?}",
            token.len(),
            reply.error
        );
    }

    // One past the published upper bound is a constraint violation, refused before the
    // handler and with the same code.
    let too_long = list(&emulator, Some(&"t".repeat(2049))).await;
    assert_eq!(
        too_long.status, 400,
        "case token one past @length max 2048: must be refused 400, got {} {:?}",
        too_long.status, too_long.body
    );

    // And the configuration is still there: a refused listing changes nothing.
    let listed = list(&emulator, None).await;
    assert_eq!(
        rows(&listed).len(),
        1,
        "case after the refusals: the installed configuration is still reported, got {:?}",
        listed.body
    );
}
