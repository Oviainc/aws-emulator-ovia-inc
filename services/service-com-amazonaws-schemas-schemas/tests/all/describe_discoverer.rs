// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DescribeDiscoverer`.
//!
//! RULE: `DescribeDiscoverer` REPORTS the one discoverer its `DiscovererId` names — the
//! response carries exactly the members the model declares for it, each holding what the
//! last write to THAT discoverer stored, and the call writes nothing, so however often it
//! is made every later caller sees the resource unchanged. An id this store holds no
//! discoverer for names no resource, and is refused as a missing resource (HTTP 404) with
//! the service's not-found sentence for a discoverer.
//!
//! The rule is asserted over a TABLE of stored discoverers whose rows differ along what it
//! generalises over: the presence, length and script of `Description`, the presence and
//! value of `CrossAccount`, how many tags the resource carries, the form of the source ARN,
//! and the state the discoverer was last left in. Each row is described once when it is
//! written and again after every other row has been written, so a handler answering from
//! anything other than the named resource — a constant, its own input, or a neighbouring
//! discoverer — would have to disagree with itself.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`; the
//! discoverer routes and the create helper come from `discoverer_state`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    CODE_RESOURCE_ALREADY_EXIST, CODE_RESOURCE_NOT_FOUND,
};

use crate::discoverer_state::{
    create_discoverer, discoverer_uri, source_arn, state_of, stop_uri, DISCOVERERS_URI,
};
use crate::registry_schema_lifecycle::{Emulator, ACCOUNT, REGION};

/// One stored discoverer and the body `DescribeDiscoverer` must then report for it.
#[derive(Debug)]
struct DescribeCase {
    case: &'static str,
    /// The event source this row's discoverer subscribes to; distinct per row, since one
    /// source carries at most one discoverer.
    source_arn: String,
    description: Option<String>,
    /// The `CrossAccount` the create sends, or `None` to omit the member.
    cross_account: Option<bool>,
    tags: BTreeMap<&'static str, &'static str>,
    /// Whether the row is stopped after it is created, so the stored state the read must
    /// report varies across the table.
    stop_after_create: bool,
}

impl DescribeCase {
    /// The `CreateDiscoverer` body this row sends. `Tags` travels as `tags`
    /// (`smithy_query(query="operation", operation="CreateDiscoverer")`: the member carries
    /// `jsonName: "tags"`).
    fn request_body(&self) -> Value {
        let mut body = json!({"SourceArn": self.source_arn});
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(cross_account) = self.cross_account {
            body["CrossAccount"] = json!(cross_account);
        }
        if !self.tags.is_empty() {
            body["tags"] = json!(self.tags);
        }
        body
    }

    /// The state this row's discoverer is left in: `STOPPED` where the row stopped it,
    /// otherwise the `STARTED` a created discoverer is in.
    fn expected_state(&self) -> &'static str {
        if self.stop_after_create {
            "STOPPED"
        } else {
            "STARTED"
        }
    }

    /// The WHOLE body `DescribeDiscoverer` must answer for this row, for the id the service
    /// minted — every member the model declares for the response and no other
    /// (`smithy_query(query="operation", operation="DescribeDiscoverer")`: `Description`,
    /// `DiscovererArn`, `DiscovererId`, `SourceArn`, `State`, `CrossAccount` and `Tags` as
    /// `tags`). `Description` is absent exactly where the resource carries none, and
    /// `CrossAccount` defaults to `true` where the create omitted it
    /// (`smithy_query(query="doc", shape="CreateDiscovererRequest", member="CrossAccount")`:
    /// "(default: true)").
    fn expected_body(&self, discoverer_id: &str) -> Value {
        let mut expected = json!({
            "DiscovererId": discoverer_id,
            "DiscovererArn": format!("arn:aws:schemas:{REGION}:{ACCOUNT}:discoverer/{discoverer_id}"),
            "SourceArn": self.source_arn,
            "State": self.expected_state(),
            "CrossAccount": self.cross_account.unwrap_or(true),
            "tags": self.tags,
        });
        if let Some(description) = &self.description {
            expected["Description"] = json!(description);
        }
        expected
    }
}

/// The table. Rows differ along the presence, length and script of `Description` (at the
/// published bounds of `__stringMin0Max256`, len 0..256 — source:
/// `smithy_query(query="operation", operation="CreateDiscoverer")`), the presence and value
/// of `CrossAccount`, how many tags the resource carries, the form of the source ARN
/// (partition, region and account fields), and the state the discoverer was last left in.
fn describe_cases() -> Vec<DescribeCase> {
    vec![
        DescribeCase {
            case: "nothing but the required source, left started",
            source_arn: source_arn("described-plain"),
            description: None,
            cross_account: None,
            tags: BTreeMap::new(),
            stop_after_create: false,
        },
        DescribeCase {
            case: "described, cross-account off, three tags, stopped",
            source_arn: source_arn("described-rich"),
            description: Some("orders bus discoverer".to_string()),
            cross_account: Some(false),
            tags: BTreeMap::from([("env", "test"), ("owner", "schemas"), ("tier", "")]),
            stop_after_create: true,
        },
        DescribeCase {
            case: "empty description at the modelled lower bound, one tag",
            source_arn: source_arn("described-empty"),
            description: Some(String::new()),
            cross_account: Some(true),
            tags: BTreeMap::from([("only", "one")]),
            stop_after_create: false,
        },
        DescribeCase {
            case: "description at the modelled upper bound, stopped",
            source_arn: source_arn("described-long"),
            description: Some("d".repeat(256)),
            cross_account: None,
            tags: BTreeMap::new(),
            stop_after_create: true,
        },
        DescribeCase {
            case: "a non-ASCII description and a source in another partition",
            source_arn: "arn:aws-cn:events:cn-north-1:210987654321:event-bus/other".to_string(),
            description: Some("caf\u{e9} \u{2014} \u{30b9}\u{30ad}\u{30fc}\u{30de}".to_string()),
            cross_account: Some(true),
            tags: BTreeMap::from([("team", "\u{30b9}\u{30ad}\u{30fc}\u{30de}")]),
            stop_after_create: false,
        },
        DescribeCase {
            case: "a source ARN whose region and account fields are empty, stopped",
            source_arn: "arn:aws:events:::event-bus/global-events-bus".to_string(),
            description: Some("global".to_string()),
            cross_account: Some(false),
            tags: BTreeMap::new(),
            stop_after_create: true,
        },
    ]
}

/// RULE: the read reports the named discoverer member for member — the modelled member set
/// exactly, each member holding what the last write stored — and goes on reporting it once
/// every other discoverer in the store has been written too.
#[tokio::test]
async fn describe_reports_every_member_of_the_stored_discoverer() {
    let emulator = Emulator::new();
    let cases = describe_cases();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut described_ids: Vec<String> = Vec::new();

    for case in &cases {
        let created = emulator
            .call("POST", DISCOVERERS_URI, Some(case.request_body()))
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateDiscoverer, got {} {:?}",
            case.case, created.status, created.body
        );
        let discoverer_id = created.str_at("DiscovererId");
        assert!(
            minted.insert(discoverer_id.clone()),
            "case {:?}: no two discoverers share an id, got {discoverer_id:?}",
            case.case
        );

        if case.stop_after_create {
            let stopped = emulator.call("POST", &stop_uri(&discoverer_id), None).await;
            assert_eq!(
                state_of(&stopped, case.case),
                "STOPPED",
                "case {:?}: the row must be stopped before it is read, got {:?}",
                case.case,
                stopped.body
            );
        }

        let described = emulator
            .call("GET", &discoverer_uri(&discoverer_id), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected 200 from DescribeDiscoverer, got {} {:?}",
            case.case, described.status, described.body
        );
        assert_eq!(
            described.body,
            case.expected_body(&discoverer_id),
            "case {:?}: expected {:?}, got {:?}",
            case.case,
            case.expected_body(&discoverer_id),
            described.body
        );
        described_ids.push(discoverer_id);
    }

    // Second pass: with every row written, each id still reports its OWN resource, so no
    // member of one discoverer leaks into another's answer.
    for (case, discoverer_id) in cases.iter().zip(&described_ids) {
        let described = emulator.call("GET", &discoverer_uri(discoverer_id), None).await;
        assert_eq!(
            described.body,
            case.expected_body(discoverer_id),
            "case {:?}: after every other discoverer was written, expected {:?}, got {:?}",
            case.case,
            case.expected_body(discoverer_id),
            described.body
        );
    }
}

/// RULE: describing a discoverer is a read — it writes nothing, so repeating it reports the
/// same resource and leaves what every other caller sees exactly as it was.
#[tokio::test]
async fn describing_writes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    for (case, bus, stop_first) in [
        ("a started discoverer read five times", "read-started", false),
        ("a stopped discoverer read five times", "read-stopped", true),
    ] {
        let discoverer_id = create_discoverer(&emulator, case, bus).await;
        let expected_state = if stop_first {
            let stopped = emulator.call("POST", &stop_uri(&discoverer_id), None).await;
            state_of(&stopped, case)
        } else {
            "STARTED".to_string()
        };

        let first = emulator
            .call("GET", &discoverer_uri(&discoverer_id), None)
            .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            first.status, first.body
        );
        for repeat in 1..5 {
            let again = emulator
                .call("GET", &discoverer_uri(&discoverer_id), None)
                .await;
            assert_eq!(
                again.body, first.body,
                "case {case:?}: read {repeat} must report the same resource as the first, \
                 expected {:?}, got {:?}",
                first.body, again.body
            );
        }

        // The reads moved no state...
        let described = emulator
            .call("GET", &discoverer_uri(&discoverer_id), None)
            .await;
        assert_eq!(
            state_of(&described, case),
            expected_state,
            "case {case:?}: the state must still be {expected_state}, got {:?}",
            described.body
        );
        // ...and did not release the subscription the discoverer holds on its source, so a
        // second create for that source is still the same resource asked for twice.
        let again = emulator
            .call(
                "POST",
                DISCOVERERS_URI,
                Some(json!({"SourceArn": source_arn(bus)})),
            )
            .await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: expected 409, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.str_at("Code"),
            CODE_RESOURCE_ALREADY_EXIST,
            "case {case:?}: expected Code {CODE_RESOURCE_ALREADY_EXIST}, got {:?}",
            again.body
        );
    }
}

/// RULE: a discoverer is named by the id the service minted, so an id this store holds no
/// discoverer for names no resource: the read is refused as a missing resource (HTTP 404,
/// the service's one not-found code) with the sentence it uses for a missing discoverer, and
/// the refusal mints nothing, so repeating it is refused the same way.
#[tokio::test]
async fn an_id_naming_no_discoverer_is_refused_as_missing() {
    let emulator = Emulator::new();

    // A discoverer that DOES exist, so the refusals below are about the id named and not
    // about an empty store.
    let existing = create_discoverer(&emulator, "a discoverer that exists", "present").await;
    let existing_body = emulator.call("GET", &discoverer_uri(&existing), None).await;

    // Rows: every shape of id a caller can send that names no discoverer here — a
    // well-formed id nothing minted, one too short and one too long for the minted form, a
    // value not of that form at all, and the existing id perturbed.
    for absent in [
        "0".repeat(32),
        "f".repeat(32),
        "abc".to_string(),
        "e".repeat(64),
        "no-such-discoverer".to_string(),
        existing.to_ascii_uppercase(),
    ] {
        for attempt in 0..3 {
            let row = format!("{absent:?} [attempt {attempt}]");
            let answer = emulator.call("GET", &discoverer_uri(&absent), None).await;
            assert_eq!(
                answer.status, 404,
                "case {row}: expected 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.str_at("Code"),
                CODE_RESOURCE_NOT_FOUND,
                "case {row}: expected Code {CODE_RESOURCE_NOT_FOUND}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_type(),
                "NotFoundException",
                "case {row}: expected the modelled NotFoundException, got {:?}",
                answer.header("x-amzn-errortype")
            );
            assert_eq!(
                answer.str_at("Message"),
                format!("Discoverer with ID {absent} does not exist."),
                "case {row}: the refusal names the discoverer the request asked for, \
                 got {:?}",
                answer.body
            );
        }
    }

    // And the discoverer that does exist reads back exactly as it did before.
    let described = emulator.call("GET", &discoverer_uri(&existing), None).await;
    assert_eq!(
        described.body, existing_body.body,
        "a refused read must leave the discoverer that exists unchanged, expected {:?}, \
         got {:?}",
        existing_body.body, described.body
    );
}
