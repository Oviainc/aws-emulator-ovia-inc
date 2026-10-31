// SPDX-License-Identifier: CC0-1.0

//! The wire form of an Amazon MQ CONFIGURATION, and the revision chain a configuration
//! carries, driven over the wire through the same dispatch path the front door uses.
//!
//! RULES these tables pin, each one the whole class rather than any one request:
//!
//! 1. Every timestamp this service answers with is an ISO-8601 / RFC-3339 date-time
//!    STRING: this model declares one timestamp shape, `__timestampIso8601`, and it
//!    carries `@timestampFormat("date-time")`
//!    (`smithy_query(query="shape",shape="__timestampIso8601")`), so the rule holds for
//!    every timestamp member of every operation — a configuration's, a revision's and a
//!    broker's alike.
//! 2. A configuration reports its `EngineType` and `AuthenticationStrategy` in the
//!    spelling the configuration surface uses — the engine's product spelling and the
//!    lower-cased strategy — while the request still sends the modelled enum value.
//! 3. A configuration always carries its `Tags` map, empty when it holds no tag.
//! 4. An `UpdateConfiguration` makes the NEXT revision of the configuration, carrying
//!    the `Description` the request supplied, and every later read reports that one.
//! 5. A `ConfigurationId` that names no configuration is refused: a READ of one —
//!    never minted, or deleted — with the modelled `NotFoundException`, whose sentence
//!    names the requested id and whose `errorAttribute` names the input member at
//!    fault. (The write path tells the two apart: see `configuration_deleted_id`.)
//! 6. `EngineVersion` defaults to the latest version Amazon MQ offers for the engine.
//!
//! The value catalogs the tables span are the model's own enumerations
//! (`ENGINE_TYPE_VALUES`, `AUTHENTICATION_STRATEGY_VALUES`) crossed with the wire
//! spellings recorded for them, never a list invented per row.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use service_com_amazonaws_mq_mq::ops::types::{
    AUTHENTICATION_STRATEGY_VALUES, ENGINE_TYPE_VALUES,
};
use service_com_amazonaws_mq_mq::state::{
    published_engine_versions, FIRST_CONFIGURATION_REVISION, MAX_CONFIGURATION_REVISIONS,
    MAX_MAX_RESULTS, MIN_MAX_RESULTS,
};

use crate::broker_lifecycle::{create_body, string_at, Mq};
use crate::configuration_name::configuration_body;

/// What the configuration surface answers for each modelled `EngineType`.
///
/// Source: the recorded Amazon MQ answer for a configuration, which carries
/// `engineType: "ActiveMQ"` where the model's enum spells the value `ACTIVEMQ`, and the
/// engine names Amazon MQ publishes for the two engines it supports ("Amazon MQ for
/// ActiveMQ", "Amazon MQ for RabbitMQ"). The modelled half of each pair is checked
/// against `ENGINE_TYPE_VALUES` by
/// [`the_wire_spelling_tables_cover_every_modelled_value`].
pub(crate) const ENGINE_TYPE_ON_THE_WIRE: &[(&str, &str)] =
    &[("ACTIVEMQ", "ActiveMQ"), ("RABBITMQ", "RabbitMQ")];

/// What the configuration surface answers for each modelled `AuthenticationStrategy`:
/// the modelled value lower-cased, as the recorded answer's `authenticationStrategy:
/// "simple"` shows for `SIMPLE`.
pub(crate) const AUTHENTICATION_STRATEGY_ON_THE_WIRE: &[(&str, &str)] = &[
    ("SIMPLE", "simple"),
    ("LDAP", "ldap"),
    ("CONFIG_MANAGED", "config_managed"),
];

/// The wire spelling of a modelled value, out of one of the tables above.
fn spelled(table: &[(&str, &str)], modelled: &str) -> String {
    table
        .iter()
        .find(|(value, _)| *value == modelled)
        .map(|(_, spelling)| (*spelling).to_string())
        .unwrap_or_else(|| panic!("the table has no wire spelling for {modelled:?}"))
}

/// The spelling a configuration reports `engine_type` in.
pub(crate) fn engine_type_on_the_wire(engine_type: &str) -> String {
    spelled(ENGINE_TYPE_ON_THE_WIRE, engine_type)
}

/// The spelling a configuration reports `strategy` in.
pub(crate) fn authentication_strategy_on_the_wire(strategy: &str) -> String {
    spelled(AUTHENTICATION_STRATEGY_ON_THE_WIRE, strategy)
}

/// The instant a member reports, read the way a client bound to this model reads it: a
/// `date-time` member is a STRING, and the protocol's default epoch-seconds NUMBER is
/// exactly the defect these tables pin.
pub(crate) fn iso_instant(case: &str, answer: &Value, pointer: &str) -> DateTime {
    match answer.pointer(pointer) {
        Some(Value::String(text)) => {
            let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
                panic!("case {case:?}: {pointer} = {text:?} must parse as an RFC 3339 date-time: {error}")
            });
            // Smithy's `date-time` is RFC 3339 in UTC, so both designators are part of
            // the form rather than of any one value.
            assert!(
                text.contains('T') && text.ends_with('Z'),
                "case {case:?}: {pointer} must be an RFC 3339 UTC date-time, got {text:?}"
            );
            parsed
        }
        other => panic!(
            "case {case:?}: {pointer} is modelled @timestampFormat(\"date-time\"), so it must \
             be an ISO-8601 date-time STRING, got {other:?}"
        ),
    }
}

impl Mq {
    /// `UpdateConfiguration`: `description` is sent only when the row supplies one, so
    /// an absent member is exercised as absent rather than as JSON `null`.
    pub(crate) async fn update_configuration(
        &self,
        id: &str,
        data: &str,
        description: Option<&str>,
    ) -> (u16, String, Value) {
        let mut body = json!({"data": data});
        if let Some(description) = description {
            body["description"] = json!(description);
        }
        self.call_with_code("PUT", &format!("/v1/configurations/{id}"), Some(body))
            .await
    }

    pub(crate) async fn delete_configuration(&self, id: &str) -> (u16, String, Value) {
        self.call_with_code("DELETE", &format!("/v1/configurations/{id}"), None)
            .await
    }

    /// `ListConfigurationRevisions`, with `query` appended when the row pages.
    pub(crate) async fn list_configuration_revisions(
        &self,
        id: &str,
        query: &str,
    ) -> (u16, String, Value) {
        let path = if query.is_empty() {
            format!("/v1/configurations/{id}/revisions")
        } else {
            format!("/v1/configurations/{id}/revisions?{query}")
        };
        self.call_with_code("GET", &path, None).await
    }

    pub(crate) async fn describe_configuration_revision(
        &self,
        id: &str,
        revision: &str,
    ) -> (u16, String, Value) {
        self.call_with_code(
            "GET",
            &format!("/v1/configurations/{id}/revisions/{revision}"),
            None,
        )
        .await
    }

    /// Every configuration the listing reports, paged to a size past what these tests
    /// create so one call holds them all.
    async fn configurations(&self) -> Vec<Value> {
        let (status, listed) = self
            .call("GET", "/v1/configurations?maxResults=100", None)
            .await;
        assert_eq!(status, 200, "ListConfigurations: expected 200, got {listed:?}");
        listed
            .pointer("/configurations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

/// An ActiveMQ `broker.xml` and a RabbitMQ Cuttlefish document, base64 as the `Data`
/// member documents ("the base64-encoded XML configuration" / "base64-encoded
/// Cuttlefish"). Encoded here rather than pasted so the rows carry real documents.
pub(crate) fn base64_data(document: &str) -> String {
    aws_smithy_types::base64::encode(document)
}

/// The tables must stay the model's: a value the model declares and this file has no
/// spelling for would otherwise be silently skipped by every row below.
#[test]
fn the_wire_spelling_tables_cover_every_modelled_value() {
    for (table, modelled, what) in [
        (ENGINE_TYPE_ON_THE_WIRE, ENGINE_TYPE_VALUES, "EngineType"),
        (
            AUTHENTICATION_STRATEGY_ON_THE_WIRE,
            AUTHENTICATION_STRATEGY_VALUES,
            "AuthenticationStrategy",
        ),
    ] {
        let mut covered: Vec<&str> = table.iter().map(|(value, _)| *value).collect();
        covered.sort_unstable();
        let mut declared: Vec<&str> = modelled.to_vec();
        declared.sort_unstable();
        assert_eq!(
            covered, declared,
            "case {what:?}: the wire-spelling table must cover exactly the modelled values, \
             got {covered:?} against {declared:?}"
        );
    }
}

/// One row of the create-then-read table.
struct ConfigurationCase {
    engine_type: &'static str,
    strategy: &'static str,
    /// `None` exercises the documented default ("Defaults to the latest available
    /// version for the specified broker engine type"); `Some` a version named by the
    /// request.
    engine_version: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// Rows spanning every modelled engine crossed with every modelled strategy, with the
/// engine version defaulted and named, and with and without tags.
fn configuration_cases() -> Vec<ConfigurationCase> {
    let mut cases = Vec::new();
    for engine_type in ENGINE_TYPE_VALUES {
        for strategy in AUTHENTICATION_STRATEGY_VALUES {
            for engine_version in [None, published_engine_versions(engine_type).first().copied()] {
                cases.push(ConfigurationCase {
                    engine_type,
                    strategy,
                    engine_version,
                    tags: if engine_version.is_some() {
                        &[("env", "prod"), ("team", "payments")]
                    } else {
                        &[]
                    },
                });
            }
        }
    }
    cases
}

/// RULES 1, 2, 3 and 6: a created configuration reads back — through its own describe
/// and through the listing — with the engine and strategy in the spelling the
/// configuration surface uses, every timestamp an ISO-8601 string, the revision Amazon
/// MQ generated for it, and a `tags` map that is present whether or not the
/// configuration carries a tag.
#[tokio::test]
async fn a_configuration_reports_the_amazon_mq_wire_form_on_every_read() {
    let mq = Mq::new("eu-west-2");
    let mut rows = 0usize;

    for case in configuration_cases() {
        let label = format!(
            "{}/{}/{:?}/{} tags",
            case.engine_type,
            case.strategy,
            case.engine_version,
            case.tags.len()
        );
        let name = format!("wire-{rows}").to_lowercase();
        let mut body = configuration_body(&name);
        body["engineType"] = json!(case.engine_type);
        body["authenticationStrategy"] = json!(case.strategy);
        if let Some(version) = case.engine_version {
            body["engineVersion"] = json!(version);
        }
        if !case.tags.is_empty() {
            body["tags"] = json!(case
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect::<serde_json::Map<String, Value>>());
        }

        let (status, created) = mq.call("POST", "/v1/configurations", Some(body)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {created:?}"
        );
        let id = string_at(&created, "/id");

        // The create's own answer already carries the strategy spelling and the
        // date-time renderings: the read path is not the only one that reports them.
        assert_eq!(
            string_at(&created, "/authenticationStrategy"),
            authentication_strategy_on_the_wire(case.strategy),
            "case {label:?}: unexpected authenticationStrategy, got {created:?}"
        );
        let created_at = iso_instant(&label, &created, "/created");
        assert_eq!(
            iso_instant(&label, &created, "/latestRevision/created"),
            created_at,
            "case {label:?}: the first revision is made with the configuration, got {created:?}"
        );
        assert_eq!(
            created.pointer("/latestRevision/isInvalid"),
            Some(&json!(false)),
            "case {label:?}: an accepted revision reports isInvalid false, got {created:?}"
        );

        let expected_version = case
            .engine_version
            .map(ToString::to_string)
            .unwrap_or_else(|| latest_offered_version(case.engine_type));
        let expected_description = format!(
            "Auto-generated default for {name} on {} {expected_version}",
            engine_type_on_the_wire(case.engine_type)
        );
        let expected_tags: Value = json!(case
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<serde_json::Map<String, Value>>());

        // Both read paths report the same stored record, so both rows below assert the
        // whole wire form rather than a member each.
        let (status, _code, described) = mq.describe_configuration(&id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the configuration to read back, got {status} {described:?}"
        );
        let listed = mq.configurations().await;
        let entry = listed
            .iter()
            .find(|entry| string_at(entry, "/id") == id)
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: the listing must hold {id}, got {listed:?}"));

        for (source, answer) in [("DescribeConfiguration", &described), ("ListConfigurations", &entry)] {
            let label = format!("{label} via {source}");
            assert_eq!(
                string_at(answer, "/engineType"),
                engine_type_on_the_wire(case.engine_type),
                "case {label:?}: unexpected engineType, got {answer:?}"
            );
            assert_eq!(
                string_at(answer, "/authenticationStrategy"),
                authentication_strategy_on_the_wire(case.strategy),
                "case {label:?}: unexpected authenticationStrategy, got {answer:?}"
            );
            assert_eq!(
                string_at(answer, "/engineVersion"),
                expected_version,
                "case {label:?}: unexpected engineVersion, got {answer:?}"
            );
            assert_eq!(
                iso_instant(&label, answer, "/created"),
                created_at,
                "case {label:?}: the reported instant must be the stored one, got {answer:?}"
            );
            assert_eq!(
                iso_instant(&label, answer, "/latestRevision/created"),
                created_at,
                "case {label:?}: the revision instant must be the stored one, got {answer:?}"
            );
            assert_eq!(
                answer.pointer("/latestRevision/revision").and_then(Value::as_i64),
                Some(i64::from(FIRST_CONFIGURATION_REVISION)),
                "case {label:?}: a created configuration is at its first revision, got {answer:?}"
            );
            assert_eq!(
                answer.pointer("/latestRevision/isInvalid"),
                Some(&json!(false)),
                "case {label:?}: an accepted revision reports isInvalid false, got {answer:?}"
            );
            assert_eq!(
                string_at(answer, "/latestRevision/description"),
                expected_description,
                "case {label:?}: unexpected revision description, got {answer:?}"
            );
            assert_eq!(
                string_at(answer, "/description"),
                expected_description,
                "case {label:?}: the configuration's description is its latest revision's, \
                 got {answer:?}"
            );
            assert_eq!(
                answer.pointer("/tags"),
                Some(&expected_tags),
                "case {label:?}: the tags map is always reported, got {answer:?}"
            );
        }
        rows += 1;
    }

    assert_eq!(
        rows,
        2 * ENGINE_TYPE_VALUES.len() * AUTHENTICATION_STRATEGY_VALUES.len(),
        "every engine and strategy must be exercised with and without an explicit version, \
         covered {rows}"
    );
}

/// The latest version Amazon MQ offers for `engine_type`: the last entry of the list
/// the service publishes for it, which is what a create that names no `EngineVersion`
/// is documented to get.
fn latest_offered_version(engine_type: &str) -> String {
    published_engine_versions(engine_type)
        .last()
        .copied()
        .unwrap_or_default()
        .to_string()
}

/// RULE 6, catalog half: the versions Amazon MQ offers for creation are MAJOR.MINOR and
/// the newest one is what a create defaults to.
///
/// Source: the recorded `DescribeBrokerInstanceOptions` catalog, whose
/// `supportedEngineVersions` carries `["5.19", "5.18"]` for ActiveMQ and `["3.13"]` for
/// RabbitMQ — the versions Amazon MQ currently offers, at that granularity. RabbitMQ is
/// the engine the reported divergence did not cite.
#[tokio::test]
async fn an_engine_version_defaults_to_the_newest_version_the_engine_offers() {
    let mq = Mq::new("us-east-1");

    for (engine_type, newest) in [("ACTIVEMQ", "5.19"), ("RABBITMQ", "3.13")] {
        let case = format!("{engine_type} default version");
        assert_eq!(
            latest_offered_version(engine_type),
            newest,
            "case {case:?}: the newest offered version must be {newest:?}"
        );
        for offered in published_engine_versions(engine_type) {
            let parts: Vec<&str> = offered.split('.').collect();
            assert!(
                parts.len() == 2 && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())),
                "case {case:?}: an offered version is MAJOR.MINOR, got {offered:?}"
            );
        }

        let mut body = configuration_body(&format!("version-default-{engine_type}").to_lowercase());
        body["engineType"] = json!(engine_type);
        let (status, created) = mq.call("POST", "/v1/configurations", Some(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        let (_status, _code, described) = mq
            .describe_configuration(&string_at(&created, "/id"))
            .await;
        assert_eq!(
            string_at(&described, "/engineVersion"),
            newest,
            "case {case:?}: a create naming no version gets the newest offered one, \
             got {described:?}"
        );
    }
}

/// One row of the revision table: the `Description` an update supplies and the document
/// it sends.
struct RevisionCase {
    what: &'static str,
    description: Option<&'static str>,
    document: &'static str,
}

/// Rows differing along what the rule generalises over: whether a description is sent
/// at all, its length, the characters it carries, and the engine document the revision
/// is made from.
const REVISION_CASES: &[RevisionCase] = &[
    RevisionCase {
        what: "a described revision",
        description: Some("second revision"),
        document: "<broker xmlns=\"http://activemq.apache.org/schema/core\"/>",
    },
    RevisionCase {
        what: "no description at all",
        description: None,
        document: "<broker><destinationPolicy/></broker>",
    },
    RevisionCase {
        what: "an empty description",
        description: Some(""),
        document: "[{rabbit,[{vm_memory_high_watermark,0.4}]}].",
    },
    RevisionCase {
        what: "a one-character description",
        description: Some("x"),
        document: "<broker/>",
    },
    RevisionCase {
        what: "a long description with punctuation",
        description: Some(
            "rollout 2026-10-30: raise the memory watermark, keep the audit log, \
             and pin the prefetch",
        ),
        document: "<broker><plugins/></broker>",
    },
];

/// RULE 4: each update makes the NEXT revision, carrying the description the request
/// supplied, and every later read reports that revision — so the chain grows by one per
/// update and never sticks at the one the create made.
#[tokio::test]
async fn an_update_makes_the_next_revision_and_every_read_reports_it() {
    let mq = Mq::new("us-east-1");
    let (status, _code, created) = mq.create_configuration("revision-chain").await;
    assert_eq!(status, 200, "setup: expected 200, got {created:?}");
    let id = string_at(&created, "/id");
    let arn = string_at(&created, "/arn");

    for (index, case) in REVISION_CASES.iter().enumerate() {
        let label = format!("update {} ({})", index + 1, case.what);
        let expected_revision = i64::from(FIRST_CONFIGURATION_REVISION) + index as i64 + 1;
        let data = base64_data(case.document);

        let (status, code, updated) = mq.update_configuration(&id, &data, case.description).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {code:?} {updated:?}"
        );
        assert_eq!(
            updated.pointer("/latestRevision/revision").and_then(Value::as_i64),
            Some(expected_revision),
            "case {label:?}: an update makes the next revision, got {updated:?}"
        );
        assert_eq!(
            string_at(&updated, "/arn"),
            arn,
            "case {label:?}: an update reports the configuration's own ARN, got {updated:?}"
        );
        assert_eq!(
            updated.pointer("/latestRevision/description").and_then(Value::as_str),
            Some(case.description.unwrap_or("")),
            "case {label:?}: the revision carries the description the update sent, \
             got {updated:?}"
        );
        assert_eq!(
            updated.pointer("/latestRevision/isInvalid"),
            Some(&json!(false)),
            "case {label:?}: an accepted revision reports isInvalid false, got {updated:?}"
        );
        assert_eq!(
            updated.pointer("/warnings"),
            Some(&json!([])),
            "case {label:?}: a document nothing sanitized reports an empty warning list, \
             got {updated:?}"
        );
        let revision_made_at = iso_instant(&label, &updated, "/latestRevision/created");
        assert_eq!(
            iso_instant(&label, &updated, "/created"),
            iso_instant(&label, &created, "/created"),
            "case {label:?}: an update reports the configuration's creation instant, \
             got {updated:?}"
        );

        // Read back: the revision the update made is the one every later answer about
        // this configuration reports.
        let (status, _code, described) = mq.describe_configuration(&id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the configuration to read back, got {described:?}"
        );
        assert_eq!(
            described.pointer("/latestRevision/revision").and_then(Value::as_i64),
            Some(expected_revision),
            "case {label:?}: the read must report the revision the update made, got {described:?}"
        );
        assert_eq!(
            described.pointer("/latestRevision/description").and_then(Value::as_str),
            Some(case.description.unwrap_or("")),
            "case {label:?}: the read must report the update's description, got {described:?}"
        );
        assert_eq!(
            described.pointer("/description").and_then(Value::as_str),
            Some(case.description.unwrap_or("")),
            "case {label:?}: the configuration's description is its latest revision's, \
             got {described:?}"
        );
        assert_eq!(
            iso_instant(&label, &described, "/latestRevision/created"),
            revision_made_at,
            "case {label:?}: the revision instant must be the stored one, got {described:?}"
        );

        let listed = mq.configurations().await;
        let entry = listed
            .iter()
            .find(|entry| string_at(entry, "/id") == id)
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: the listing must hold {id}, got {listed:?}"));
        assert_eq!(
            entry.pointer("/latestRevision/revision").and_then(Value::as_i64),
            Some(expected_revision),
            "case {label:?}: the listing must report the same revision, got {entry:?}"
        );
        // One representation of a description, whichever read reports it: the
        // listing's required member and the describe's optional one agree, for the
        // row that supplies no description as much as for the rows that do.
        for pointer in ["/latestRevision/description", "/description"] {
            assert_eq!(
                entry.pointer(pointer).and_then(Value::as_str),
                Some(case.description.unwrap_or("")),
                "case {label:?}: the listing's {pointer} must agree with the read, got {entry:?}"
            );
        }
    }
}

/// RULE 2 and 4 on the update's own answer: it describes the configuration it revised,
/// so it reports that configuration's identity and its `AuthenticationStrategy` in the
/// spelling the configuration surface uses — for every strategy the model declares.
#[tokio::test]
async fn an_update_reports_the_configuration_it_revised() {
    let mq = Mq::new("us-east-1");
    let data = base64_data("<broker/>");

    for strategy in AUTHENTICATION_STRATEGY_VALUES {
        let case = format!("update a {strategy} configuration");
        let name = format!("revised-{strategy}").to_lowercase();
        let mut body = configuration_body(&name);
        body["authenticationStrategy"] = json!(strategy);
        let (status, created) = mq.call("POST", "/v1/configurations", Some(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        let id = string_at(&created, "/id");

        let (status, code, updated) = mq
            .update_configuration(&id, &data, Some("a revision"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {code:?} {updated:?}"
        );
        assert_eq!(
            string_at(&updated, "/authenticationStrategy"),
            authentication_strategy_on_the_wire(strategy),
            "case {case:?}: the answer reports the configuration's strategy, got {updated:?}"
        );
        for (pointer, expected) in [
            ("/id", id.clone()),
            ("/name", name.clone()),
            ("/arn", string_at(&created, "/arn")),
        ] {
            assert_eq!(
                string_at(&updated, pointer),
                expected,
                "case {case:?}: {pointer} must name the configuration revised, got {updated:?}"
            );
        }
        assert_eq!(
            updated.pointer("/warnings"),
            Some(&json!([])),
            "case {case:?}: a document nothing sanitized reports an empty warning list, \
             got {updated:?}"
        );
    }
}

/// Documents that are not the base64 the `Data` member documents, one per form a caller
/// plausibly sends.
const DATA_OUTSIDE_BASE64: &[(&str, &str)] = &[
    ("raw XML", "<broker/>"),
    ("blank", "   "),
    ("base64 with a stray character", "PGJyb2tlci8+!"),
    ("text with spaces", "not base 64 at all"),
];

/// A `Data` that is not base64 is refused naming the member, and makes no revision: the
/// chain a later read reports is the one the refused call did not touch.
#[tokio::test]
async fn a_data_outside_base64_is_refused_and_makes_no_revision() {
    let mq = Mq::new("us-east-1");
    let (status, _code, created) = mq.create_configuration("revision-refused").await;
    assert_eq!(status, 200, "setup: expected 200, got {created:?}");
    let id = string_at(&created, "/id");

    for (what, data) in DATA_OUTSIDE_BASE64 {
        let case = format!("UpdateConfiguration with {what}");
        let (status, code, answer) = mq.update_configuration(&id, data, Some("refused")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected BadRequestException, got {code:?} {answer:?}"
        );
        assert_eq!(
            answer.pointer("/errorAttribute").and_then(Value::as_str),
            Some("data"),
            "case {case:?}: the refusal must name the member at fault, got {answer:?}"
        );

        let (_status, _code, described) = mq.describe_configuration(&id).await;
        assert_eq!(
            described.pointer("/latestRevision/revision").and_then(Value::as_i64),
            Some(i64::from(FIRST_CONFIGURATION_REVISION)),
            "case {case:?}: a refused update must make no revision, got {described:?}"
        );
    }
}

/// RULE 4, through the revision operations: the chain an update grows is what
/// `ListConfigurationRevisions` pages through — every revision exactly once, oldest
/// first, whatever page size the request asks for — and each revision reads back
/// through `DescribeConfigurationRevision` carrying the document and the description
/// the update that made it sent.
#[tokio::test]
async fn every_revision_reads_back_through_the_revision_operations() {
    let mq = Mq::new("us-east-1");
    let (status, _code, created) = mq.create_configuration("revision-readback").await;
    assert_eq!(status, 200, "setup: expected 200, got {created:?}");
    let id = string_at(&created, "/id");

    // What each revision must then report: the create-time one, which carries the
    // description Amazon MQ generates for it, then one per row of the revision table,
    // in the order the updates were made.
    let mut expected: Vec<(i64, String, Option<String>)> = vec![(
        i64::from(FIRST_CONFIGURATION_REVISION),
        string_at(&created, "/latestRevision/description"),
        None,
    )];
    for case in REVISION_CASES {
        let data = base64_data(case.document);
        let (status, code, updated) = mq.update_configuration(&id, &data, case.description).await;
        assert_eq!(
            status, 200,
            "setup {}: expected 200, got {status} {code:?} {updated:?}",
            case.what
        );
        expected.push((
            expected.len() as i64 + i64::from(FIRST_CONFIGURATION_REVISION),
            case.description.unwrap_or("").to_string(),
            Some(data),
        ));
    }

    // Paged at each published bound of `maxResults` and at a size between them: the
    // pages concatenate to the whole chain, in order, with no revision repeated.
    for size in [MIN_MAX_RESULTS, 2, MAX_MAX_RESULTS] {
        let case = format!("maxResults={size}");
        let mut seen: Vec<i64> = Vec::new();
        let mut token = String::new();
        loop {
            let query = if token.is_empty() {
                format!("maxResults={size}")
            } else {
                format!("maxResults={size}&nextToken={token}")
            };
            let (status, code, page) = mq.list_configuration_revisions(&id, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {code:?} {page:?}"
            );
            assert_eq!(
                string_at(&page, "/configurationId"),
                id,
                "case {case:?}: the answer names the configuration it listed, got {page:?}"
            );
            let revisions = page
                .pointer("/revisions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                revisions.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page may not exceed the size asked for, got {page:?}"
            );
            for entry in &revisions {
                // Every entry is the modelled revision shape, with the wire form the
                // rest of the configuration surface reports.
                iso_instant(&case, entry, "/created");
                assert_eq!(
                    entry.pointer("/isInvalid"),
                    Some(&json!(false)),
                    "case {case:?}: an accepted revision reports isInvalid false, got {entry:?}"
                );
                seen.push(
                    entry
                        .pointer("/revision")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| panic!("case {case:?}: no revision in {entry:?}")),
                );
            }
            match page.pointer("/nextToken").and_then(Value::as_str) {
                Some(next) => token = next.to_string(),
                None => break,
            }
        }
        assert_eq!(
            seen,
            expected.iter().map(|(revision, _, _)| *revision).collect::<Vec<i64>>(),
            "case {case:?}: every revision must appear once, oldest first, got {seen:?}"
        );
    }

    // And each revision reads back on its own, by its number.
    for (revision, description, data) in &expected {
        let case = format!("DescribeConfigurationRevision {revision}");
        let (status, code, answer) = mq
            .describe_configuration_revision(&id, &revision.to_string())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {code:?} {answer:?}"
        );
        assert_eq!(
            string_at(&answer, "/configurationId"),
            id,
            "case {case:?}: the answer names the configuration, got {answer:?}"
        );
        assert_eq!(
            answer.pointer("/revision").and_then(Value::as_i64),
            Some(*revision),
            "case {case:?}: the answer names the revision it describes, got {answer:?}"
        );
        assert_eq!(
            answer.pointer("/description").and_then(Value::as_str),
            Some(description.as_str()),
            "case {case:?}: the revision carries the description it was made with, \
             got {answer:?}"
        );
        assert_eq!(
            answer.pointer("/data").and_then(Value::as_str),
            data.as_deref(),
            "case {case:?}: the revision carries the document the update sent, got {answer:?}"
        );
        iso_instant(&case, &answer, "/created");
    }
}

/// Revision labels that name no revision of a configuration, one per form a caller
/// plausibly sends: outside the chain, below it, and not a revision number at all.
const REVISIONS_NAMING_NOTHING: &[&str] = &["0", "-1", "99", "abc", "1.5", "2x"];

/// A `ConfigurationRevision` that names no revision of the configuration is refused
/// with the modelled `NotFoundException`, naming the label asked for and the member at
/// fault — while the revisions that DO exist are still served.
#[tokio::test]
async fn a_revision_that_names_nothing_is_refused_naming_it() {
    let mq = Mq::new("us-east-1");
    let (status, _code, created) = mq.create_configuration("revision-refusals").await;
    assert_eq!(status, 200, "setup: expected 200, got {created:?}");
    let id = string_at(&created, "/id");

    for label in REVISIONS_NAMING_NOTHING {
        let case = format!("DescribeConfigurationRevision on revision {label:?}");
        let (status, code, answer) = mq.describe_configuration_revision(&id, label).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answer:?}"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException, got {code:?} {answer:?}"
        );
        assert_eq!(
            string_at(&answer, "/message"),
            format!(
                "Configuration revision [{label}] of configuration ID [{id}] can't be found. \
                 Make sure it exists."
            ),
            "case {case:?}: the sentence must name what was asked for, got {answer:?}"
        );
        assert_eq!(
            answer.pointer("/errorAttribute").and_then(Value::as_str),
            Some("configuration-revision"),
            "case {case:?}: the refusal must name the input at fault, got {answer:?}"
        );
    }

    // The revision the create made is still served, so the rows above refused the
    // label rather than the operation.
    let (status, _code, answer) = mq
        .describe_configuration_revision(&id, &FIRST_CONFIGURATION_REVISION.to_string())
        .await;
    assert_eq!(
        status, 200,
        "the configuration's own revision must still be served, got {status} {answer:?}"
    );
}

/// The bound on how far ONE configuration's revision chain may grow: the updates up to
/// it are served and each makes the next revision, and the one past it is refused
/// without storing anything, so the revision a later read reports stays at the bound.
#[tokio::test]
async fn the_revision_chain_bound_refuses_the_update_past_it() {
    let mq = Mq::new("us-east-1");
    let (status, _code, created) = mq.create_configuration("revision-bound").await;
    assert_eq!(status, 200, "setup: expected 200, got {created:?}");
    let id = string_at(&created, "/id");
    let data = base64_data("<broker/>");

    // The create already made the first revision, so the updates that fit inside the
    // bound are the ones after it.
    for index in FIRST_CONFIGURATION_REVISION as usize..MAX_CONFIGURATION_REVISIONS {
        let (status, code, answer) = mq.update_configuration(&id, &data, None).await;
        assert_eq!(
            status, 200,
            "case {index}: an update inside the bound must be served, got {status} {code:?} \
             {answer:?}"
        );
    }

    let (status, code, answer) = mq.update_configuration(&id, &data, None).await;
    assert_eq!(
        status, 400,
        "one past the bound: expected 400, got {status} {answer:?}"
    );
    assert_eq!(
        code, "BadRequestException",
        "one past the bound: expected BadRequestException, got {code:?} {answer:?}"
    );

    let (_status, _code, described) = mq.describe_configuration(&id).await;
    assert_eq!(
        described.pointer("/latestRevision/revision").and_then(Value::as_i64),
        Some(MAX_CONFIGURATION_REVISIONS as i64),
        "a refused update must store nothing, got {described:?}"
    );
}

/// `ConfigurationId`s that name nothing, one per form a caller plausibly sends.
pub(crate) const IDS_NAMING_NOTHING: &[&str] = &[
    "c-doesnotexist000000",
    "c-00000000-0000-0000-0000-000000000000",
    "c-",
    "not-even-a-configuration-id",
];

/// RULE 5: every operation that resolves a `ConfigurationId` refuses one that names
/// nothing with the modelled `NotFoundException` — the sentence naming the requested id
/// and `errorAttribute` naming the member — whatever form the id takes.
#[tokio::test]
async fn a_configuration_id_that_names_nothing_is_refused_naming_it() {
    let mq = Mq::new("us-east-1");
    let data = base64_data("<broker/>");

    for id in IDS_NAMING_NOTHING {
        for (operation, answer) in [
            ("DescribeConfiguration", mq.describe_configuration(id).await),
            (
                "UpdateConfiguration",
                mq.update_configuration(id, &data, Some("never applied")).await,
            ),
            ("DeleteConfiguration", mq.delete_configuration(id).await),
            (
                "ListConfigurationRevisions",
                mq.list_configuration_revisions(id, "").await,
            ),
            (
                "DescribeConfigurationRevision",
                mq.describe_configuration_revision(id, "1").await,
            ),
        ] {
            let case = format!("{operation} on {id:?}");
            let (status, code, body) = answer;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {body:?}"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case:?}: expected NotFoundException, got {code:?} {body:?}"
            );
            assert_eq!(
                string_at(&body, "/message"),
                format!("Configuration ID [{id}] can't be found. Make sure it exists."),
                "case {case:?}: the sentence must name the requested id, got {body:?}"
            );
            assert_eq!(
                body.pointer("/errorAttribute").and_then(Value::as_str),
                Some("configuration-id"),
                "case {case:?}: the refusal must name the input at fault, got {body:?}"
            );
        }
    }
}

/// RULE 5, deletion half: once a configuration is deleted it is gone from the store, so
/// reading it answers the same `NotFoundException` as an id that was never minted, the
/// listing no longer holds it, and a second delete is refused too. The configurations
/// deletion did not name are untouched.
#[tokio::test]
async fn a_deleted_configuration_is_no_longer_served() {
    let mq = Mq::new("us-east-1");
    let data = base64_data("<broker/>");

    // Several configurations, so the row that deletes one shows the others survive; one
    // of them is deleted after a revision was made, so a configuration with a chain
    // deletes exactly like a freshly created one.
    let mut ids = Vec::new();
    for index in 0..3 {
        let (status, _code, created) = mq
            .create_configuration(&format!("delete-{index}"))
            .await;
        assert_eq!(status, 200, "setup {index}: expected 200, got {created:?}");
        ids.push(string_at(&created, "/id"));
    }
    let (status, _code, updated) = mq
        .update_configuration(&ids[1], &data, Some("revised before delete"))
        .await;
    assert_eq!(status, 200, "setup: expected 200, got {updated:?}");

    for (index, id) in ids.clone().into_iter().enumerate() {
        let case = format!("delete {id}");
        let (status, code, deleted) = mq.delete_configuration(&id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {code:?} {deleted:?}"
        );
        assert_eq!(
            string_at(&deleted, "/configurationId"),
            id,
            "case {case:?}: the answer names the configuration it deleted, got {deleted:?}"
        );

        let (status, code, answer) = mq.describe_configuration(&id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deleted configuration must no longer be served, got {status} \
             {answer:?}"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException, got {code:?} {answer:?}"
        );
        assert_eq!(
            string_at(&answer, "/message"),
            format!("Configuration ID [{id}] can't be found. Make sure it exists."),
            "case {case:?}: the sentence must name the requested id, got {answer:?}"
        );
        assert_eq!(
            answer.pointer("/errorAttribute").and_then(Value::as_str),
            Some("configuration-id"),
            "case {case:?}: the refusal must name the input at fault, got {answer:?}"
        );

        // A second delete of the same id is the not-found refusal too: there is
        // nothing left to remove.
        let (status, code, again) = mq.delete_configuration(&id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a second delete must be refused, got {status} {again:?}"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException, got {code:?} {again:?}"
        );

        let remaining: Vec<String> = mq
            .configurations()
            .await
            .iter()
            .map(|entry| string_at(entry, "/id"))
            .collect();
        assert_eq!(
            remaining,
            ids[index + 1..].to_vec(),
            "case {case:?}: only the configurations still held may be listed, got {remaining:?}"
        );
    }
}

/// RULE 1 over the whole service: every member this model marks as a timestamp is a
/// `date-time` one (`__timestampIso8601` is the model's only timestamp shape), so every
/// operation that reports an instant reports a STRING — the broker surface as much as
/// the configuration surface.
#[tokio::test]
async fn every_reported_instant_is_an_iso8601_string() {
    let mq = Mq::new("us-east-1");

    let (status, _code, configuration) = mq.create_configuration("instants").await;
    assert_eq!(status, 200, "setup: expected 200, got {configuration:?}");
    let configuration_id = string_at(&configuration, "/id");
    let (status, _code, updated) = mq
        .update_configuration(
            &configuration_id,
            &base64_data("<broker/>"),
            Some("a revision"),
        )
        .await;
    assert_eq!(status, 200, "setup: expected 200, got {updated:?}");
    let (status, broker) = mq.create_broker(create_body("instants")).await;
    assert_eq!(status, 200, "setup: expected 200, got {status} {broker:?}");
    let broker_id = string_at(&broker, "/brokerId");

    let (_status, _code, described_configuration) =
        mq.describe_configuration(&configuration_id).await;
    let listed_configuration = mq
        .configurations()
        .await
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the listing must hold the configuration, got nothing"));
    let (status, described_broker) = mq.describe_broker(&broker_id).await;
    assert_eq!(status, 200, "DescribeBroker: expected 200, got {described_broker:?}");
    let (status, listed_brokers) = mq.list_brokers("").await;
    assert_eq!(status, 200, "ListBrokers: expected 200, got {listed_brokers:?}");

    // Every (operation, member) this service answers an instant on.
    let answers: &[(&str, &Value, &[&str])] = &[
        (
            "CreateConfiguration",
            &configuration,
            &["/created", "/latestRevision/created"],
        ),
        (
            "UpdateConfiguration",
            &updated,
            &["/created", "/latestRevision/created"],
        ),
        (
            "DescribeConfiguration",
            &described_configuration,
            &["/created", "/latestRevision/created"],
        ),
        (
            "ListConfigurations",
            &listed_configuration,
            &["/created", "/latestRevision/created"],
        ),
        ("DescribeBroker", &described_broker, &["/created"]),
        ("ListBrokers", &listed_brokers, &["/brokerSummaries/0/created"]),
    ];

    for (operation, answer, pointers) in answers {
        for pointer in *pointers {
            let case = format!("{operation}{pointer}");
            // Parsing it as a `date-time` string is the assertion; a number would not
            // reach here at all.
            iso_instant(&case, answer, pointer);
        }
    }
}
