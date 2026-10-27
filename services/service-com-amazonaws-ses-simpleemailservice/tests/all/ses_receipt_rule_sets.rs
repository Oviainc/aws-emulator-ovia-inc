// SPDX-License-Identifier: CC0-1.0

//! The receipt rule set plane, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE each operation follows over
//! a SET of inputs, not one recorded request:
//!
//! * a set is recorded only under a name of the documented form, only when no
//!   set already holds that name, and only while the account is under its
//!   rule-set limit — each refusal carrying the modelled error for its own cause;
//! * `CloneReceiptRuleSet` records nothing at all unless the SOURCE set exists,
//!   and otherwise copies the source's rules into an independent set;
//! * `DescribeReceiptRuleSet` reports exactly what was stored.

use crate::harness::{call, element, field, Field};
use service_com_amazonaws_ses_simpleemailservice::ops::types::ReceiptRule;
use service_com_amazonaws_ses_simpleemailservice::state::{
    ComAmazonawsSesSimpleemailserviceState, ReceiptRuleSet, MAX_RECEIPT_RULE_SETS,
    MAX_RECEIPT_RULE_SET_NAME,
};
use service_com_amazonaws_ses_simpleemailservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::SystemTime;

/// A router holding only this service, beside the engine it was built over — the
/// same construction `crate::harness::emulator` performs, handing back the
/// engine as well so a test can seed a rule set holding RULES directly, keeping
/// each case about the operations under test rather than about
/// `CreateReceiptRule`.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// `CreateReceiptRuleSet`.
async fn create_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateReceiptRuleSet",
        &[field("RuleSetName", name)],
    )
    .await
}

/// `CloneReceiptRuleSet`.
async fn clone_rule_set(
    router: &MultiServiceRouter,
    original: &str,
    name: &str,
) -> (u16, String) {
    let params: Vec<Field> = vec![
        field("OriginalRuleSetName", original),
        field("RuleSetName", name),
    ];
    call(router, "CloneReceiptRuleSet", &params).await
}

/// `DescribeReceiptRuleSet`.
async fn describe_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "DescribeReceiptRuleSet",
        &[field("RuleSetName", name)],
    )
    .await
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope.
fn error(body: &str) -> (Option<String>, Option<String>) {
    (element(body, "Code"), element(body, "Message"))
}

/// A rule to seed a source set with, so a clone has something to copy.
fn seeded_rule(name: &str, recipients: &[&str], enabled: bool) -> ReceiptRule {
    ReceiptRule {
        name: name.to_string(),
        enabled,
        tls_policy: Some("Optional".to_string()),
        recipients: Some(recipients.iter().map(|at| (*at).to_string()).collect()),
        actions: None,
        scan_enabled: true,
    }
}

/// Store a receipt rule set holding `rules` directly in the service's state.
fn seed_rule_set(engine: &engine::InMemoryEngine, name: &str, rules: Vec<ReceiptRule>) {
    engine
        .state::<ComAmazonawsSesSimpleemailserviceState>()
        .lock()
        .receipt_rule_sets
        .insert(
            name.to_string(),
            ReceiptRuleSet {
                name: name.to_string(),
                created_at: SystemTime::now(),
                rules,
            },
        );
}

/// Cloning from a source the account does not hold is refused as
/// `RuleSetDoesNotExist`, and records nothing — whatever the form of the name.
#[tokio::test]
async fn cloning_an_absent_source_is_refused_and_records_nothing() {
    let (router, _engine) = emulator_with_engine();
    // The one set that DOES exist, so a refusal below is about the named source
    // and not about an empty account.
    let (status, body) = create_rule_set(&router, "present-rule-set").await;
    assert_eq!(
        status, 200,
        "CreateReceiptRuleSet(present-rule-set): expected 200, got {status} with {body}"
    );

    // The rows differ in the form and size of the ABSENT source name, including
    // one that is not even a legal rule set name: a name the account cannot hold
    // is a name it does not hold.
    let absent_sources = [
        "absent",
        "a",
        "Absent_Rule-Set_2024",
        &"n".repeat(MAX_RECEIPT_RULE_SET_NAME),
        "not a legal name!",
    ];
    for (index, original) in absent_sources.iter().enumerate() {
        let target = format!("clone-target-{index}");
        let (status, body) = clone_rule_set(&router, original, &target).await;
        assert_eq!(
            status, 400,
            "case original={original:?}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("RuleSetDoesNotExist"),
            "case original={original:?}: expected the modelled RuleSetDoesNotExist, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule set does not exist: {original}").as_str()),
            "case original={original:?}: expected the absent source named in the message, \
             got {body}"
        );

        // Nothing was recorded for the refused clone.
        let (status, body) = describe_rule_set(&router, &target).await;
        assert_eq!(
            status, 400,
            "case original={original:?}: the refused clone must not record {target:?}, \
             got {status} with {body}"
        );
        assert_eq!(
            error(&body).0.as_deref(),
            Some("RuleSetDoesNotExist"),
            "case original={original:?}: {target:?} must not exist, got {body}"
        );
    }
}

/// A created set is readable back, under every name form the model documents,
/// with the name and creation timestamp it was stored with and no rules.
#[tokio::test]
async fn a_created_rule_set_round_trips_through_describe() {
    let (router, _engine) = emulator_with_engine();
    // The documented name form (`CreateReceiptRuleSetRequest$RuleSetName`):
    // ASCII letters, numbers, underscores or dashes, starting and ending with a
    // letter or number, 64 characters or fewer. The rows span that span: one
    // character, a digit only, both separators, mixed case, and the published
    // maximum length.
    let names: Vec<String> = vec![
        "a".to_string(),
        "9".to_string(),
        "rule-set".to_string(),
        "rule_set".to_string(),
        "Prod-Rules_2024".to_string(),
        "x".repeat(MAX_RECEIPT_RULE_SET_NAME),
    ];
    for name in &names {
        let (status, body) = create_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from CreateReceiptRuleSet, got {status} with {body}"
        );

        let (status, body) = describe_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        let metadata = element(&body, "Metadata").unwrap_or_default();
        assert_eq!(
            element(&metadata, "Name").as_deref(),
            Some(name.as_str()),
            "case {name:?}: Metadata.Name must be the stored name, got {body}"
        );
        let created = element(&metadata, "CreatedTimestamp").unwrap_or_default();
        let parsed = ::aws_smithy_types::DateTime::from_str(
            &created,
            ::aws_smithy_types::date_time::Format::DateTime,
        );
        let seconds = match parsed {
            Ok(timestamp) => timestamp.as_secs_f64(),
            Err(parse_error) => panic!(
                "case {name:?}: CreatedTimestamp {created:?} must be a timestamp, \
                 got {parse_error}"
            ),
        };
        assert!(
            seconds > 1_600_000_000.0,
            "case {name:?}: CreatedTimestamp must be the instant the set was stored, \
             got {seconds}"
        );
        // "Creates an empty receipt rule set": the list is served, and empty.
        assert_eq!(
            element(&body, "Rules").as_deref(),
            Some(""),
            "case {name:?}: a created set holds no rules, got {body}"
        );
    }

    // The same name twice is a naming conflict, for a create and for a clone
    // alike — and the second call leaves the first set alone.
    for name in &names {
        let (status, body) = create_rule_set(&router, name).await;
        assert_eq!(
            status, 400,
            "case re-create {name:?}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("AlreadyExists"),
            "case re-create {name:?}: expected the modelled AlreadyExists, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule set already exists: {name}").as_str()),
            "case re-create {name:?}: expected the conflicting name in the message, got {body}"
        );

        let (status, body) = clone_rule_set(&router, "a", name).await;
        assert_eq!(
            status, 400,
            "case clone onto {name:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error(&body).0.as_deref(),
            Some("AlreadyExists"),
            "case clone onto {name:?}: expected the modelled AlreadyExists, got {body}"
        );
    }
}

/// A clone holds a COPY of the source's rules: the new set reports the same rule
/// list, and a later change to the source does not reach it.
#[tokio::test]
async fn a_clone_copies_the_source_rules_into_an_independent_set() {
    // The rows differ in how many rules the source holds and in what each rule
    // carries, since that is what the copy generalises over.
    let sources: Vec<(&str, Vec<ReceiptRule>)> = vec![
        ("empty-source", Vec::new()),
        (
            "one-rule-source",
            vec![seeded_rule("inbound", &["sales@example.com"], true)],
        ),
        (
            "three-rule-source",
            vec![
                seeded_rule("first", &[], true),
                seeded_rule("second", &["a@example.com", "b@example.net"], false),
                seeded_rule("third", &["example.org"], true),
            ],
        ),
    ];
    for (source_name, rules) in sources {
        let (router, engine) = emulator_with_engine();
        let rule_count = rules.len();
        seed_rule_set(&engine, source_name, rules);

        let clone_name = format!("{source_name}-copy");
        let (status, body) = clone_rule_set(&router, source_name, &clone_name).await;
        assert_eq!(
            status, 200,
            "case {source_name:?}: expected 200 from CloneReceiptRuleSet, got {status} \
             with {body}"
        );

        let (status, source_body) = describe_rule_set(&router, source_name).await;
        assert_eq!(
            status, 200,
            "case {source_name:?}: expected 200 describing the source, got {status} \
             with {source_body}"
        );
        let (status, clone_body) = describe_rule_set(&router, &clone_name).await;
        assert_eq!(
            status, 200,
            "case {source_name:?}: expected 200 describing the clone, got {status} \
             with {clone_body}"
        );

        let source_rules = element(&source_body, "Rules").unwrap_or_default();
        let clone_rules = element(&clone_body, "Rules").unwrap_or_default();
        assert_eq!(
            clone_rules, source_rules,
            "case {source_name:?}: the clone must report the source's {rule_count} rule(s), \
             got {clone_body}"
        );
        let clone_metadata = element(&clone_body, "Metadata").unwrap_or_default();
        assert_eq!(
            element(&clone_metadata, "Name").as_deref(),
            Some(clone_name.as_str()),
            "case {source_name:?}: the clone is its own set, got {clone_body}"
        );

        // "completely independent of the source rule set": changing the source
        // after the clone leaves the clone's rules where they were.
        engine
            .state::<ComAmazonawsSesSimpleemailserviceState>()
            .lock()
            .receipt_rule_sets
            .remove(source_name);
        let (status, body) = describe_rule_set(&router, &clone_name).await;
        assert_eq!(
            status, 200,
            "case {source_name:?}: the clone outlives its source, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Rules").unwrap_or_default(),
            source_rules,
            "case {source_name:?}: the clone's rules are its own copy, got {body}"
        );
    }
}

/// A name outside the documented form is refused, and nothing is stored under
/// it, for the create and the clone alike.
#[tokio::test]
async fn a_name_outside_the_documented_form_is_refused() {
    let (router, _engine) = emulator_with_engine();
    let (status, body) = create_rule_set(&router, "source").await;
    assert_eq!(
        status, 200,
        "CreateReceiptRuleSet(source): expected 200, got {status} with {body}"
    );

    // One row per clause of the documented rule: a character outside
    // letters/numbers/underscore/dash (space, dot, plus), a name that does not
    // START with a letter or number, one that does not END with one, a non-ASCII
    // letter, and one character past the published 64-character maximum.
    let rejected: Vec<String> = vec![
        "has space".to_string(),
        "dotted.name".to_string(),
        "plus+name".to_string(),
        "_leading".to_string(),
        "-leading".to_string(),
        "trailing_".to_string(),
        "trailing-".to_string(),
        "ünïcode".to_string(),
        "y".repeat(MAX_RECEIPT_RULE_SET_NAME + 1),
    ];
    for name in &rejected {
        for action in ["CreateReceiptRuleSet", "CloneReceiptRuleSet"] {
            let (status, body) = if action == "CreateReceiptRuleSet" {
                create_rule_set(&router, name).await
            } else {
                clone_rule_set(&router, "source", name).await
            };
            assert_eq!(
                status, 400,
                "case {action} {name:?}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error(&body).0.as_deref(),
                Some("ValidationException"),
                "case {action} {name:?}: expected a validation refusal, got {body}"
            );
        }

        let (status, body) = describe_rule_set(&router, name).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a refused name must store nothing, got {status} with {body}"
        );
    }
}

/// The account's rule-set limit bounds the store: the published number of sets
/// is accepted, and the one past it is refused as `LimitExceeded` — by the
/// create and by the clone, since both would grow the store.
#[tokio::test]
async fn the_rule_set_limit_bounds_the_store() {
    let (router, _engine) = emulator_with_engine();
    // SES documents 20 receipt rule sets per account, which the crate publishes
    // as `MAX_RECEIPT_RULE_SETS`; every one of them is accepted.
    for index in 0..MAX_RECEIPT_RULE_SETS {
        let name = format!("limit-rs-{index:02}");
        let (status, body) = create_rule_set(&router, &name).await;
        assert_eq!(
            status, 200,
            "case set {index} of {MAX_RECEIPT_RULE_SETS}: expected 200, got {status} \
             with {body}"
        );
    }

    let expected_message = format!(
        "Rule set limit exceeded: this account may hold {MAX_RECEIPT_RULE_SETS} receipt \
         rule sets."
    );
    for action in ["CreateReceiptRuleSet", "CloneReceiptRuleSet"] {
        let (status, body) = if action == "CreateReceiptRuleSet" {
            create_rule_set(&router, "limit-rs-past-the-bound").await
        } else {
            clone_rule_set(&router, "limit-rs-00", "limit-rs-past-the-bound").await
        };
        assert_eq!(
            status, 400,
            "case {action} one past the bound: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("LimitExceeded"),
            "case {action} one past the bound: expected the modelled LimitExceeded, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(expected_message.as_str()),
            "case {action} one past the bound: expected the limit in the message, got {body}"
        );
    }

    // At the bound a conflicting name is still a conflict, not a limit: the
    // store would not grow.
    let (status, body) = clone_rule_set(&router, "limit-rs-00", "limit-rs-01").await;
    assert_eq!(
        status, 400,
        "case clone onto an existing name at the bound: expected HTTP 400, got {status} \
         with {body}"
    );
    assert_eq!(
        error(&body).0.as_deref(),
        Some("AlreadyExists"),
        "case clone onto an existing name at the bound: expected AlreadyExists, got {body}"
    );

    // And every set stored up to the bound is still readable.
    for index in 0..MAX_RECEIPT_RULE_SETS {
        let name = format!("limit-rs-{index:02}");
        let (status, body) = describe_rule_set(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {name:?} after the refusals: expected 200, got {status} with {body}"
        );
    }
}
