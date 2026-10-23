// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of Entity Resolution's real-time matching pair,
//! `GenerateMatchId` and `GetMatchId`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the sibling `schema_mapping_workflow_lifecycle` suite owns — so what is
//! asserted is what an AWS SDK client would see.
//!
//! The rule under test: `GenerateMatchId` resolves each submitted record against the
//! records the named rule-based workflow has ALREADY processed, using that workflow's
//! own `ruleBasedProperties.rules` in declared order — the first rule all of whose
//! `matchingKeys` are present and equal in both records joins the submitted record to
//! the group of the record it matched, and the group is reported with that rule's
//! `ruleName`; a record that matches nothing keeps the `matchId` a previous call gave
//! it, or opens a new group; and `GetMatchId` reads the same persisted index without
//! changing it.
//!
//! Each table row differs along what the rule generalises over: the `processingType`
//! (over the whole modelled `ProcessingType` enum plus the absent member), which rule
//! of several matches and in which order they are declared, how many schema fields a
//! matching key groups, whether the input source asks for normalization and in what
//! way two values differ, and the `resolutionType` of the workflow. The recorded case
//! — one record submitted twice, expecting one Match ID — is a single row of the
//! first table. Two further suites pin the bounds the emulator enforces on this
//! data-plane path: the modelled batch maximum, the modelled enum of processing
//! modes, and the per-workflow index bound that `DeleteMatchingWorkflow` releases.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_entityresolution_awsveniceservice::state::MAX_INDEXED_RECORDS_PER_WORKFLOW;

use crate::schema_mapping_workflow_lifecycle::{glue_table_arn, role_arn, string_member, Emulator, Reply};

/// The schema mapping every workflow in this suite reads its `matchKey` grouping from.
const SCHEMA_NAME: &str = "people";

/// The schema mapping's fields.
///
/// `email` carries the `matchKey` `email`, and `first` and `last` share the `matchKey`
/// `fullname`, which is what makes `fullname` a matching key over TWO columns
/// (`SchemaInputAttribute$matchKey`: "A key that allows grouping of multiple input
/// attributes into a unified matching group",
/// `smithy_query(query="shape",shape="SchemaInputAttribute")`). `phone` carries no
/// `matchKey`, so no rule here can match on it.
fn schema_request() -> Value {
    json!({
        "schemaName": SCHEMA_NAME,
        "mappedInputFields": [
            { "fieldName": "id", "type": "UNIQUE_ID" },
            { "fieldName": "email", "type": "EMAIL_ADDRESS", "matchKey": "email" },
            { "fieldName": "first", "type": "NAME_FIRST", "groupName": "NAME",
              "matchKey": "fullname" },
            { "fieldName": "last", "type": "NAME_LAST", "groupName": "NAME",
              "matchKey": "fullname" },
            { "fieldName": "phone", "type": "PHONE_NUMBER" },
        ],
    })
}

/// A `resolutionTechniques` for a rule-based workflow carrying `rules`, with
/// `enableRealTimeMatching` present only when `enable_real_time_matching` is `Some`.
fn rule_based(rules: Value, enable_real_time_matching: Option<bool>) -> Value {
    let mut techniques = json!({
        "resolutionType": "RULE_MATCHING",
        "ruleBasedProperties": {
            "rules": rules,
            "attributeMatchingModel": "ONE_TO_ONE",
        },
    });
    if let Some(flag) = enable_real_time_matching {
        techniques["enableRealTimeMatching"] = json!(flag);
    }
    techniques
}

/// The single rule most cases use: match on the `email` matching key.
fn by_email() -> Value {
    json!([{ "ruleName": "by email", "matchingKeys": ["email"] }])
}

/// A `CreateMatchingWorkflow` request over [`SCHEMA_NAME`], with
/// `inputSourceConfig[0].applyNormalization` present only when `normalize` is `Some`.
fn workflow_request(name: &str, resolution_techniques: Value, normalize: Option<bool>) -> Value {
    let mut source = json!({
        "inputSourceARN": glue_table_arn("people"),
        "schemaName": SCHEMA_NAME,
    });
    if let Some(flag) = normalize {
        source["applyNormalization"] = json!(flag);
    }
    json!({
        "workflowName": name,
        "inputSourceConfig": [source],
        "outputSourceConfig": [{
            "outputS3Path": "s3://er-output/match",
            "output": [{ "name": "id" }],
        }],
        "resolutionTechniques": resolution_techniques,
        "roleArn": role_arn(),
    })
}

/// One `Record`: the `inputSourceARN` of the workflow's input source, a `uniqueId` and
/// the attributes to match on.
fn record(unique_id: &str, attributes: Value) -> Value {
    json!({
        "inputSourceARN": glue_table_arn("people"),
        "uniqueId": unique_id,
        "recordAttributeMap": attributes,
    })
}

/// `POST /matchingworkflows/{workflowName}/generateMatches`, the modelled
/// `GenerateMatchId` binding (`smithy_query(query="operation",operation="GenerateMatchId")`).
async fn generate_match_id(
    emulator: &Emulator,
    workflow_name: &str,
    records: Value,
    processing_type: Option<&str>,
) -> Reply {
    let mut body = json!({ "records": records });
    if let Some(mode) = processing_type {
        body["processingType"] = json!(mode);
    }
    emulator
        .call(
            "POST",
            &format!("/matchingworkflows/{workflow_name}/generateMatches"),
            Some(&body),
        )
        .await
}

/// `POST /matchingworkflows/{workflowName}/matches`, the modelled `GetMatchId`
/// binding, with `applyNormalization` present only when the caller sets it.
async fn get_match_id(
    emulator: &Emulator,
    workflow_name: &str,
    attributes: Value,
    apply_normalization: Option<bool>,
) -> Reply {
    let mut body = json!({ "record": attributes });
    if let Some(flag) = apply_normalization {
        body["applyNormalization"] = json!(flag);
    }
    emulator
        .call(
            "POST",
            &format!("/matchingworkflows/{workflow_name}/matches"),
            Some(&body),
        )
        .await
}

/// Create [`SCHEMA_NAME`] plus one workflow, and fail loudly if either setup call does.
async fn seed(emulator: &Emulator, workflow: &Value) {
    let schema = emulator.create_schema_mapping(&schema_request()).await;
    assert_eq!(
        schema.status, 200,
        "seeding the schema mapping failed: {schema:?}"
    );
    let created = emulator.create_matching_workflow(workflow).await;
    assert_eq!(
        created.status, 200,
        "seeding the matching workflow failed: {created:?}"
    );
}

/// The `failedRecords` of a reply, treating an omitted list as empty.
fn failed_records(reply: &Reply) -> Vec<Value> {
    reply
        .body
        .get("failedRecords")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The ONE match group a single-record call answers, with the call's success and empty
/// `failedRecords` asserted on the way.
fn only_group(case: &str, reply: &Reply) -> Value {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {reply:?}"
    );
    let groups = reply
        .body
        .get("matchGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: matchGroups is not a list in {:?}", reply.body));
    assert_eq!(
        groups.len(),
        1,
        "case {case:?}: expected exactly one match group, got {groups:?}"
    );
    assert!(
        failed_records(reply).is_empty(),
        "case {case:?}: expected no failed records, got {:?}",
        failed_records(reply)
    );
    groups[0].clone()
}

/// The `recordId`s of a group, in the order the group lists them.
fn group_record_ids(case: &str, group: &Value) -> Vec<String> {
    group
        .get("records")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: records is not a list in {group:?}"))
        .iter()
        .map(|record| string_member(record, "recordId"))
        .collect()
}

/// A `matchId` is opaque — `MatchGroup$matchId` is a plain `String` with no modelled
/// form — so what every case asserts about it is only that it is a single non-empty
/// token the service minted.
fn assert_match_id_shape(case: &str, match_id: &str) {
    assert!(
        !match_id.is_empty() && !match_id.chars().any(char::is_whitespace),
        "case {case:?}: expected a non-empty whitespace-free matchId, got {match_id:?}"
    );
}

/// One `processingType` and whether that mode consults the records the workflow has
/// already processed.
struct ProcessingCase {
    case: &'static str,
    /// `None` is the absent member, which the model documents as `CONSISTENT`.
    processing_type: Option<&'static str>,
    /// Does a second call for the same record answer the SAME `matchId`?
    looks_up: bool,
}

/// The `processingType` table: the whole modelled `ProcessingType` enum
/// (`smithy_query(query="shape",shape="ProcessingType")` → `CONSISTENT | EVENTUAL |
/// EVENTUAL_NO_LOOKUP`) plus the absent member, which defaults to `CONSISTENT`.
/// Only `EVENTUAL_NO_LOOKUP` "generates new match IDs without checking existing
/// matches".
fn processing_cases() -> Vec<ProcessingCase> {
    vec![
        ProcessingCase {
            case: "processingType absent, which defaults to CONSISTENT",
            processing_type: None,
            looks_up: true,
        },
        ProcessingCase {
            case: "CONSISTENT",
            processing_type: Some("CONSISTENT"),
            looks_up: true,
        },
        ProcessingCase {
            case: "EVENTUAL",
            processing_type: Some("EVENTUAL"),
            looks_up: true,
        },
        ProcessingCase {
            case: "EVENTUAL_NO_LOOKUP",
            processing_type: Some("EVENTUAL_NO_LOOKUP"),
            looks_up: false,
        },
    ]
}

#[tokio::test]
async fn a_record_keeps_its_match_id_across_calls_unless_the_mode_skips_the_lookup() {
    let emulator = Emulator::new();
    seed(
        &emulator,
        &workflow_request("wf-dedup", rule_based(by_email(), Some(true)), None),
    )
    .await;

    // Every case gets its own record, so no case can match another's: what is under
    // test is the SAME record submitted twice.
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (index, mode) in processing_cases().into_iter().enumerate() {
        let case = mode.case;
        let unique_id = format!("dedup-{index}");
        let attributes = json!({ "email": format!("person{index}@example.com") });
        let submission = json!([record(&unique_id, attributes.clone())]);

        let first = generate_match_id(&emulator, "wf-dedup", submission.clone(), mode.processing_type).await;
        let first_group = only_group(case, &first);
        let first_id = string_member(&first_group, "matchId");
        assert_match_id_shape(case, &first_id);
        assert_eq!(
            group_record_ids(case, &first_group),
            vec![unique_id.clone()],
            "case {case:?}: the new group must hold exactly the submitted record, got {first_group:?}"
        );
        assert_eq!(
            string_member(&first_group, "matchRule"),
            "",
            "case {case:?}: a record that matched nothing names no rule, got {first_group:?}"
        );

        let second = generate_match_id(&emulator, "wf-dedup", submission, mode.processing_type).await;
        let second_group = only_group(case, &second);
        let second_id = string_member(&second_group, "matchId");
        assert_match_id_shape(case, &second_id);
        if mode.looks_up {
            assert_eq!(
                second_id, first_id,
                "case {case:?}: a second call for the same record must answer the same matchId, \
                 got {second_id:?} after {first_id:?}"
            );
        } else {
            assert_ne!(
                second_id, first_id,
                "case {case:?}: a mode that checks no existing matches must mint a new matchId, \
                 got {second_id:?} again"
            );
        }
        assert_eq!(
            group_record_ids(case, &second_group),
            vec![unique_id.clone()],
            "case {case:?}: the group must still hold exactly that record, got {second_group:?}"
        );

        assert!(
            minted.insert(first_id.clone()),
            "case {case:?}: matchId {first_id:?} was already minted for another group"
        );
        if !mode.looks_up {
            assert!(
                minted.insert(second_id.clone()),
                "case {case:?}: matchId {second_id:?} was already minted for another group"
            );
        }
    }
}

/// One record submitted after the table's first record, and the group the rule says it
/// must land in.
struct JoinCase {
    case: &'static str,
    unique_id: &'static str,
    attributes: Value,
    /// The `uniqueId`s the answering group must hold, in `uniqueId` order.
    group: Vec<&'static str>,
    /// The `ruleName` the group must be reported with — the empty string when no rule
    /// joined the record to anything.
    match_rule: &'static str,
    /// Does this record share the group of the record named here?
    same_group_as: Option<&'static str>,
}

#[tokio::test]
async fn the_first_rule_that_matches_an_indexed_record_decides_the_group() {
    let emulator = Emulator::new();
    // Two rules, in this order: the second is reached only when the first does not
    // match, which is what the "first rule in declared order" part of the rule means.
    let rules = json!([
        { "ruleName": "by email", "matchingKeys": ["email"] },
        { "ruleName": "by fullname", "matchingKeys": ["fullname"] },
    ]);
    seed(
        &emulator,
        &workflow_request("wf-rules", rule_based(rules, Some(true)), None),
    )
    .await;

    // The record every case is matched against.
    let seeded = json!([record(
        "anchor",
        json!({ "email": "ada@example.com", "first": "Ada", "last": "Lovelace" }),
    )]);
    let anchor = generate_match_id(&emulator, "wf-rules", seeded, None).await;
    let anchor_id = string_member(&only_group("the anchor record", &anchor), "matchId");

    let cases = vec![
        JoinCase {
            case: "the first rule matches: same email, different name",
            unique_id: "same-email",
            attributes: json!({ "email": "ada@example.com", "first": "A", "last": "L" }),
            group: vec!["anchor", "same-email"],
            match_rule: "by email",
            same_group_as: Some("anchor"),
        },
        JoinCase {
            case: "the first rule does not match but the second does: same grouped name",
            unique_id: "same-name",
            attributes: json!({
                "email": "ada.lovelace@example.com",
                "first": "Ada",
                "last": "Lovelace",
            }),
            group: vec!["anchor", "same-email", "same-name"],
            match_rule: "by fullname",
            same_group_as: Some("anchor"),
        },
        JoinCase {
            case: "only one field of the grouped matching key agrees, so neither rule matches",
            unique_id: "half-name",
            attributes: json!({
                "email": "ada.byron@example.com",
                "first": "Ada",
                "last": "Byron",
            }),
            group: vec!["half-name"],
            match_rule: "",
            same_group_as: None,
        },
        JoinCase {
            case: "no matching key agrees at all",
            unique_id: "stranger",
            attributes: json!({
                "email": "grace@example.com",
                "first": "Grace",
                "last": "Hopper",
            }),
            group: vec!["stranger"],
            match_rule: "",
            same_group_as: None,
        },
        JoinCase {
            case: "a record matching only the second rule's group, not the anchor's email",
            unique_id: "hopper-twin",
            attributes: json!({ "first": "Grace", "last": "Hopper" }),
            group: vec!["hopper-twin", "stranger"],
            match_rule: "by fullname",
            same_group_as: Some("stranger"),
        },
    ];

    let mut group_of: std::collections::BTreeMap<String, String> =
        [("anchor".to_string(), anchor_id)].into_iter().collect();

    for case in cases {
        let label = case.case;
        let reply = generate_match_id(
            &emulator,
            "wf-rules",
            json!([record(case.unique_id, case.attributes)]),
            None,
        )
        .await;
        let group = only_group(label, &reply);
        let match_id = string_member(&group, "matchId");
        assert_match_id_shape(label, &match_id);
        assert_eq!(
            string_member(&group, "matchRule"),
            case.match_rule,
            "case {label:?}: expected matchRule {:?}, got {group:?}",
            case.match_rule
        );
        assert_eq!(
            group_record_ids(label, &group),
            case.group,
            "case {label:?}: expected the group to hold {:?}, got {group:?}",
            case.group
        );
        match case.same_group_as {
            Some(other) => assert_eq!(
                Some(&match_id),
                group_of.get(other),
                "case {label:?}: expected the matchId of {other:?}, got {match_id:?}"
            ),
            None => assert!(
                !group_of.values().any(|existing| *existing == match_id),
                "case {label:?}: expected a fresh matchId, got the existing {match_id:?}"
            ),
        }
        group_of.insert(case.unique_id.to_string(), match_id);
    }
}

/// Two values that differ only in a way normalization erases.
struct NormalizationCase {
    case: &'static str,
    seeded: &'static str,
    submitted: &'static str,
}

/// The normalization table: the ways `applyNormalization` says two values of the same
/// schema attribute are the same value — letter case, and leading, trailing and
/// repeated whitespace.
fn normalization_cases() -> Vec<NormalizationCase> {
    vec![
        NormalizationCase {
            case: "letter case",
            seeded: "ada@example.com",
            submitted: "Ada@Example.COM",
        },
        NormalizationCase {
            case: "leading and trailing whitespace",
            seeded: "grace@example.com",
            submitted: "  grace@example.com\t",
        },
        NormalizationCase {
            case: "repeated inner whitespace and case together",
            seeded: "Jean  Bartik",
            submitted: "jean bartik",
        },
    ]
}

#[tokio::test]
async fn applied_normalization_decides_whether_two_spellings_are_one_record() {
    let emulator = Emulator::new();
    seed(
        &emulator,
        &workflow_request("wf-normalized", rule_based(by_email(), Some(true)), Some(true)),
    )
    .await;
    // The same schema and rule, with the input source NOT asking for normalization.
    let raw = emulator
        .create_matching_workflow(&workflow_request(
            "wf-raw",
            rule_based(by_email(), Some(true)),
            Some(false),
        ))
        .await;
    assert_eq!(raw.status, 200, "seeding the raw workflow failed: {raw:?}");

    for (index, case) in normalization_cases().into_iter().enumerate() {
        let label = case.case;
        for (workflow, normalizes) in [("wf-normalized", true), ("wf-raw", false)] {
            let seeded_id = format!("seed-{index}");
            let submitted_id = format!("submit-{index}");
            let first = generate_match_id(
                &emulator,
                workflow,
                json!([record(&seeded_id, json!({ "email": case.seeded }))]),
                None,
            )
            .await;
            let seeded_match = string_member(&only_group(label, &first), "matchId");

            let second = generate_match_id(
                &emulator,
                workflow,
                json!([record(&submitted_id, json!({ "email": case.submitted }))]),
                None,
            )
            .await;
            let group = only_group(label, &second);
            let submitted_match = string_member(&group, "matchId");
            if normalizes {
                assert_eq!(
                    submitted_match, seeded_match,
                    "case {label:?} on {workflow}: normalization must make {:?} and {:?} one \
                     group, got {submitted_match:?} beside {seeded_match:?}",
                    case.seeded, case.submitted
                );
                assert_eq!(
                    string_member(&group, "matchRule"),
                    "by email",
                    "case {label:?} on {workflow}: the joining rule must be named, got {group:?}"
                );
            } else {
                assert_ne!(
                    submitted_match, seeded_match,
                    "case {label:?} on {workflow}: without normalization {:?} and {:?} are \
                     different values, yet both answered {submitted_match:?}",
                    case.seeded, case.submitted
                );
                assert_eq!(
                    string_member(&group, "matchRule"),
                    "",
                    "case {label:?} on {workflow}: no rule joined this record, got {group:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_record_no_rule_applies_to_still_opens_its_own_group() {
    let emulator = Emulator::new();
    seed(
        &emulator,
        &workflow_request("wf-unmatchable", rule_based(by_email(), Some(true)), None),
    )
    .await;

    // The class: the record carries a value for NO matching key of any rule, so no
    // rule can join it to anything — it is new, so it opens its own group, and it
    // "received a generated Match ID", which is what keeps `failedRecords` empty. The
    // rows differ in what the record does carry: a schema field that is no `matchKey`,
    // an attribute the schema does not define, and nothing at all.
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (index, (case, attributes)) in [
        ("only a schema field that is no matchKey", json!({ "phone": "1234567890" })),
        ("only an attribute the schema does not define", json!({ "nickname": "ada" })),
        ("an empty recordAttributeMap", json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        let unique_id = format!("unmatchable-{index}");
        let reply = generate_match_id(
            &emulator,
            "wf-unmatchable",
            json!([record(&unique_id, attributes)]),
            None,
        )
        .await;
        let group = only_group(case, &reply);
        let match_id = string_member(&group, "matchId");
        assert_match_id_shape(case, &match_id);
        assert_eq!(
            string_member(&group, "matchRule"),
            "",
            "case {case:?}: no rule joined this record, so none is named, got {group:?}"
        );
        assert_eq!(
            group_record_ids(case, &group),
            vec![unique_id.clone()],
            "case {case:?}: the group must hold exactly the submitted record, got {group:?}"
        );
        assert!(
            minted.insert(match_id.clone()),
            "case {case:?}: matchId {match_id:?} was already minted for another group"
        );

        // And the record is in the index: submitting it again answers the same group.
        let again = generate_match_id(
            &emulator,
            "wf-unmatchable",
            json!([record(&unique_id, json!({ "phone": "0000000000" }))]),
            None,
        )
        .await;
        assert_eq!(
            string_member(&only_group(case, &again), "matchId"),
            match_id,
            "case {case:?}: the record must keep the Match ID it was given, got {:?}",
            again.body
        );
    }
}

#[tokio::test]
async fn get_match_id_reads_the_group_generate_match_id_wrote_and_changes_nothing() {
    let emulator = Emulator::new();
    // Two rules, so one record can open its own group (no rule) and another can join
    // that group BY a rule — the two stored `matchRule` values the reader must report.
    let rules = json!([
        { "ruleName": "by email", "matchingKeys": ["email"] },
        { "ruleName": "by fullname", "matchingKeys": ["fullname"] },
    ]);
    seed(
        &emulator,
        &workflow_request("wf-reader", rule_based(rules, Some(true)), None),
    )
    .await;

    let attributes = json!({ "email": "ada@example.com", "first": "Ada", "last": "Lovelace" });
    let written = generate_match_id(
        &emulator,
        "wf-reader",
        json!([record("ada", attributes.clone())]),
        None,
    )
    .await;
    let first_group = only_group("the written record", &written);
    let match_id = string_member(&first_group, "matchId");
    assert_eq!(
        string_member(&first_group, "matchRule"),
        "",
        "the first record joined nothing, so the writer stored no rule, got {first_group:?}"
    );

    // A second record of the same group, joined by the SECOND rule, so what the
    // workflow stored for it is `by fullname` — even though a read that carries only
    // its email FINDS it by the first rule.
    let joined_attributes = json!({
        "email": "ada.lovelace@example.com",
        "first": "Ada",
        "last": "Lovelace",
    });
    let joined = generate_match_id(
        &emulator,
        "wf-reader",
        json!([record("ada-2", joined_attributes.clone())]),
        None,
    )
    .await;
    let joined_group = only_group("the joined record", &joined);
    assert_eq!(
        string_member(&joined_group, "matchId"),
        match_id,
        "the second record must join the first record's group, got {joined_group:?}"
    );
    assert_eq!(
        string_member(&joined_group, "matchRule"),
        "by fullname",
        "the second rule is what joined it, got {joined_group:?}"
    );

    // The round trip: the reader answers the group's `matchId` and the `matchRule` the
    // WRITER stored for the record the request identifies — with the other attributes
    // present, absent, or different.
    for (case, read_attributes, expected_rule) in [
        ("the whole first record", attributes.clone(), ""),
        (
            "only the first record's matching key",
            json!({ "email": "ada@example.com" }),
            "",
        ),
        (
            "the matching key beside unrelated attributes",
            json!({ "email": "ada@example.com", "phone": "1234567890" }),
            "",
        ),
        (
            "the joined record, found by a different rule than the one stored for it",
            json!({ "email": "ada.lovelace@example.com" }),
            "by fullname",
        ),
        (
            "the whole joined record",
            joined_attributes.clone(),
            "by fullname",
        ),
    ] {
        let read = get_match_id(&emulator, "wf-reader", read_attributes, None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200, got {read:?}"
        );
        assert_eq!(
            read.body.get("matchId").and_then(Value::as_str),
            Some(match_id.as_str()),
            "case {case:?}: expected matchId {match_id:?}, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("matchRule").and_then(Value::as_str),
            Some(expected_rule),
            "case {case:?}: expected the stored matchRule {expected_rule:?}, got {:?}",
            read.body
        );
    }

    // A record the workflow has not processed has no Match ID, and both output members
    // are optional, so neither is served.
    let unknown = get_match_id(&emulator, "wf-reader", json!({ "email": "grace@example.com" }), None).await;
    assert_eq!(
        unknown.status, 200,
        "an unprocessed record must not be an error, got {unknown:?}"
    );
    assert_eq!(
        unknown.body.get("matchId"),
        None,
        "an unprocessed record carries no matchId, got {:?}",
        unknown.body
    );
    assert_eq!(
        unknown.body.get("matchRule"),
        None,
        "an unprocessed record carries no matchRule, got {:?}",
        unknown.body
    );

    // Reading does not change what a later caller sees: the reader is idempotent, and
    // the writer still answers the same group afterwards.
    let again = get_match_id(&emulator, "wf-reader", attributes.clone(), None).await;
    assert_eq!(
        again.body.get("matchId").and_then(Value::as_str),
        Some(match_id.as_str()),
        "a second read must answer the same group, got {:?}",
        again.body
    );
    let regenerated = generate_match_id(
        &emulator,
        "wf-reader",
        json!([record("ada", attributes)]),
        None,
    )
    .await;
    assert_eq!(
        string_member(&only_group("after two reads", &regenerated), "matchId"),
        match_id,
        "the reads must not have changed the record's group, got {:?}",
        regenerated.body
    );
}

#[tokio::test]
async fn a_workflow_that_cannot_serve_real_time_matching_refuses_both_operations() {
    let emulator = Emulator::new();
    let schema = emulator.create_schema_mapping(&schema_request()).await;
    assert_eq!(schema.status, 200, "seeding the schema mapping failed: {schema:?}");

    // The class: a workflow this pair is not defined over. The rows span the rest of
    // the modelled `ResolutionType` enum (`ML_MATCHING | PROVIDER`, from
    // `smithy_query(query="shape",shape="ResolutionType")`) and, for a RULE_MATCHING
    // workflow, `enableRealTimeMatching` absent and explicitly false — the member that
    // governs whether `GenerateMatchId` may be used with the workflow.
    let provider_arn = "arn:aws:entityresolution:us-east-1::providerservice/testprovider/assignment";
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "a machine-learning workflow",
            "wf-ml",
            json!({ "resolutionType": "ML_MATCHING" }),
            false,
        ),
        (
            "a provider workflow",
            "wf-provider",
            json!({
                "resolutionType": "PROVIDER",
                "providerProperties": { "providerServiceArn": provider_arn },
            }),
            false,
        ),
        (
            "a rule-based workflow with enableRealTimeMatching absent",
            "wf-no-realtime",
            rule_based(by_email(), None),
            true,
        ),
        (
            "a rule-based workflow with enableRealTimeMatching false",
            "wf-realtime-off",
            rule_based(by_email(), Some(false)),
            true,
        ),
    ];

    for (case, name, techniques, rule_based_workflow) in cases {
        let created = emulator
            .create_matching_workflow(&workflow_request(name, techniques, None))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: seeding the workflow failed: {created:?}"
        );

        let reply = generate_match_id(
            &emulator,
            name,
            json!([record("rec", json!({ "email": "ada@example.com" }))]),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 from GenerateMatchId, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected a ValidationException from GenerateMatchId, got {reply:?}"
        );

        // `GetMatchId` is defined over a rule-based workflow too, and accepts the ones
        // whose real-time matching is off: it reads an index rather than writing one.
        let read = get_match_id(&emulator, name, json!({ "email": "ada@example.com" }), None).await;
        if rule_based_workflow {
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected GetMatchId to answer, got {read:?}"
            );
            assert_eq!(
                read.body.get("matchId"),
                None,
                "case {case:?}: the refused GenerateMatchId must have indexed nothing, got {:?}",
                read.body
            );
        } else {
            assert_eq!(
                read.status, 400,
                "case {case:?}: expected HTTP 400 from GetMatchId, got {read:?}"
            );
            assert_eq!(
                read.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected a ValidationException from GetMatchId, got {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_unknown_workflow_name_answers_resource_not_found_on_both_operations() {
    let emulator = Emulator::new();
    seed(
        &emulator,
        &workflow_request("wf-known", rule_based(by_email(), Some(true)), None),
    )
    .await;
    let deleted = emulator.delete_matching_workflow("wf-known").await;
    assert_eq!(deleted.status, 200, "deleting the workflow failed: {deleted:?}");

    // The class: a `workflowName` that is well formed per `EntityName` but names no
    // workflow — never created, or created and then deleted — over the character
    // classes the modelled `^[a-zA-Z_0-9-]*$` admits and the `length` bounds 1 and 255.
    for (case, name) in [
        ("a deleted workflow", "wf-known".to_string()),
        ("a name that was never created", "never_created-9".to_string()),
        ("a single-character name at the modelled length minimum", "a".to_string()),
        ("a name at the modelled length maximum of 255", "w".repeat(255)),
    ] {
        let reply = generate_match_id(
            &emulator,
            &name,
            json!([record("rec", json!({ "email": "ada@example.com" }))]),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404 from GenerateMatchId, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException from GenerateMatchId, got \
             {reply:?}"
        );

        let read = get_match_id(&emulator, &name, json!({ "email": "ada@example.com" }), None).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected HTTP 404 from GetMatchId, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException from GetMatchId, got {read:?}"
        );
    }
}

#[tokio::test]
async fn the_batch_the_mode_and_the_index_are_all_bounded() {
    let emulator = Emulator::new();
    seed(
        &emulator,
        &workflow_request("wf-bounds", rule_based(by_email(), Some(true)), None),
    )
    .await;

    // The modelled `records` length range is 1..1
    // (`smithy_query(query="operation",operation="GenerateMatchId")` → `length 1..1`),
    // so BOTH a longer batch and an empty one are refused, and nothing either carried
    // is indexed.
    for (case, records) in [
        (
            "a batch above the modelled maximum",
            json!([
                record("batch-a", json!({ "email": "a@example.com" })),
                record("batch-b", json!({ "email": "b@example.com" })),
            ]),
        ),
        ("an empty batch, below the modelled minimum", json!([])),
    ] {
        let oversized = generate_match_id(&emulator, "wf-bounds", records, None).await;
        assert_eq!(
            oversized.status, 400,
            "case {case:?}: must be refused, got {oversized:?}"
        );
        assert_eq!(
            oversized.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: must be a ValidationException, got {oversized:?}"
        );
    }
    let leaked = get_match_id(&emulator, "wf-bounds", json!({ "email": "a@example.com" }), None).await;
    assert_eq!(
        leaked.body.get("matchId"),
        None,
        "a refused batch must index nothing, got {:?}",
        leaked.body
    );

    // A `processingType` outside the modelled `ProcessingType` enum is refused rather
    // than read as the default mode.
    for case in ["QUICK", "consistent", "EVENTUAL_LOOKUP", ""] {
        let reply = generate_match_id(
            &emulator,
            "wf-bounds",
            json!([record("mode", json!({ "email": "mode@example.com" }))]),
            Some(case),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an unmodelled processingType must be refused, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: an unmodelled processingType must be a ValidationException, got \
             {reply:?}"
        );
    }

    // The per-workflow index is bounded: filling it leaves room for no NEW record,
    // while a record the workflow already holds is still re-indexed (it replaces its
    // entry rather than growing the store).
    for index in 0..MAX_INDEXED_RECORDS_PER_WORKFLOW {
        let reply = generate_match_id(
            &emulator,
            "wf-bounds",
            json!([record(
                &format!("fill-{index}"),
                json!({ "email": format!("fill{index}@example.com") }),
            )]),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "record {index} below the bound must be indexed, got {reply:?}"
        );
    }
    let over = generate_match_id(
        &emulator,
        "wf-bounds",
        json!([record("one-too-many", json!({ "email": "over@example.com" }))]),
        None,
    )
    .await;
    assert_eq!(
        over.status, 400,
        "a record past the per-workflow bound must be refused, got {over:?}"
    );
    assert_eq!(
        over.error_code.as_deref(),
        Some("ValidationException"),
        "a record past the per-workflow bound must be a ValidationException, got {over:?}"
    );
    let already_held = generate_match_id(
        &emulator,
        "wf-bounds",
        json!([record("fill-0", json!({ "email": "fill0@example.com" }))]),
        None,
    )
    .await;
    assert_eq!(
        already_held.status, 200,
        "a record the workflow already holds must still be served at the bound, got \
         {already_held:?}"
    );

    // And the bound is self-healing: deleting the workflow releases its index, so a
    // workflow created again under the same name starts empty.
    let deleted = emulator.delete_matching_workflow("wf-bounds").await;
    assert_eq!(deleted.status, 200, "deleting the workflow failed: {deleted:?}");
    let recreated = emulator
        .create_matching_workflow(&workflow_request(
            "wf-bounds",
            rule_based(by_email(), Some(true)),
            None,
        ))
        .await;
    assert_eq!(recreated.status, 200, "recreating the workflow failed: {recreated:?}");
    let after_release = get_match_id(
        &emulator,
        "wf-bounds",
        json!({ "email": "fill0@example.com" }),
        None,
    )
    .await;
    assert_eq!(
        after_release.body.get("matchId"),
        None,
        "the deleted workflow's index must be released, got {:?}",
        after_release.body
    );
    let fresh = generate_match_id(
        &emulator,
        "wf-bounds",
        json!([record("after-release", json!({ "email": "over@example.com" }))]),
        None,
    )
    .await;
    assert_eq!(
        fresh.status, 200,
        "a released index must leave room again, got {fresh:?}"
    );
}

#[tokio::test]
async fn a_matching_key_no_schema_defines_matches_nothing() {
    let emulator = Emulator::new();
    // `Rule$matchingKeys` must "have been defined in the SchemaMapping"
    // (`smithy_query(query="shape",shape="Rule")`), and this workflow's rule names a
    // key the schema defines neither as a `matchKey` nor as a `fieldName` — so no
    // record can match on it, however equal the records' attributes of that name are.
    seed(
        &emulator,
        &workflow_request(
            "wf-undefined-key",
            rule_based(
                json!([{ "ruleName": "by undefined", "matchingKeys": ["nickname"] }]),
                Some(true),
            ),
            None,
        ),
    )
    .await;

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, unique_id) in [
        ("the first record carrying the undefined key", "undef-a"),
        ("a second record carrying the SAME value for it", "undef-b"),
    ] {
        let reply = generate_match_id(
            &emulator,
            "wf-undefined-key",
            json!([record(unique_id, json!({ "nickname": "ada" }))]),
            None,
        )
        .await;
        let group = only_group(case, &reply);
        assert_eq!(
            string_member(&group, "matchRule"),
            "",
            "case {case:?}: a key no schema defines joins nothing, got {group:?}"
        );
        assert_eq!(
            group_record_ids(case, &group),
            vec![unique_id.to_string()],
            "case {case:?}: the group must hold only the submitted record, got {group:?}"
        );
        assert!(
            minted.insert(string_member(&group, "matchId")),
            "case {case:?}: the two records must not share a matchId, got {group:?}"
        );
    }

    // And the reader agrees: there is no group to read for that key.
    let read = get_match_id(
        &emulator,
        "wf-undefined-key",
        json!({ "nickname": "ada" }),
        None,
    )
    .await;
    assert_eq!(
        read.body.get("matchId"),
        None,
        "a key no schema defines identifies no group, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn the_reader_honours_the_apply_normalization_the_request_asks_for() {
    let emulator = Emulator::new();
    // The input source normalizes, so the two spellings below are one record for
    // `GenerateMatchId`; `GetMatchId` carries its OWN `applyNormalization`, modelled
    // with a default of `true`.
    seed(
        &emulator,
        &workflow_request("wf-read-normalized", rule_based(by_email(), Some(true)), Some(true)),
    )
    .await;
    let written = generate_match_id(
        &emulator,
        "wf-read-normalized",
        json!([record("ada", json!({ "email": "Ada@Example.com" }))]),
        None,
    )
    .await;
    let match_id = string_member(&only_group("the written record", &written), "matchId");

    for (case, apply_normalization, expected) in [
        ("absent, which the model defaults to true", None, Some(match_id.as_str())),
        ("explicitly true", Some(true), Some(match_id.as_str())),
        ("explicitly false, so the spellings differ", Some(false), None),
    ] {
        let read = get_match_id(
            &emulator,
            "wf-read-normalized",
            json!({ "email": "ada@example.com" }),
            apply_normalization,
        )
        .await;
        assert_eq!(
            read.body.get("matchId").and_then(Value::as_str),
            expected,
            "case {case:?}: expected matchId {expected:?}, got {:?}",
            read.body
        );
    }
}
