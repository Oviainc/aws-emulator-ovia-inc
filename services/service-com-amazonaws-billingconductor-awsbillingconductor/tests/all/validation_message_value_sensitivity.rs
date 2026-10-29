// SPDX-License-Identifier: CC0-1.0

//! WHICH value a constraint clause states and which it withholds — the one half of this
//! service's `ValidationException.Message` that is read off the MODEL rather than off the
//! way a member happens to be spelled.
//!
//! RULE under test: a clause states the value the caller submitted, at the member's dotted
//! path, with the requirement it failed — and states NO value exactly when the model marks
//! the value's shape, the member declaring it, or (for a collection, whose rendering states
//! what it holds) its elements `@sensitive`. A member's NAME decides nothing: this model
//! spells two filter members `Names`, one targeting a list of a `@sensitive` string and the
//! other a list of a plain structure, and the two are answered differently because the
//! model declares them differently.
//!
//! The rows span what the rule generalises over: the operation, the member, WHERE the
//! constraint lives (on a list itself, on a list's elements, on a member of a structure
//! nested inside a list, on a map entry's value, on a scalar), the constraint class
//! (`@length` at each end, `@pattern`, `@range`) and whether the model marks the value
//! sensitive — with the two `names` filters as the pair the rule turns on. The premise
//! those rows rest on is read off the model itself, not asserted from memory
//! (`the_model_marks_one_names_filter_sensitive_and_not_the_other`).
//!
//! Every assertion goes over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

// The tagging operations bind their `ResourceArn` to a non-greedy `@httpLabel`, so an ARN
// travels as ONE encoded path segment; that encoding has one home in this test crate.
use super::tagging_surface::encode_label;

/// This service's Smithy model, read at compile time so a model-derived test cannot drift
/// from the model this crate is generated from.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-billingconductor-awsbillingconductor.json"
));

/// The trait a model writes on a shape, or on the member declaring it, whose value a
/// message never echoes.
const SENSITIVE_TRAIT: &str = "smithy.api#sensitive";

/// `SearchValue`'s `@pattern`, as a clause states it: the model's expression without the
/// `^`/`$` anchors (`smithy_query(query="shape",shape="StringSearch")` — member
/// `SearchValue`). Note the space the character class admits, which
/// `CustomLineItemName`'s does not.
const SEARCH_VALUE_PATTERN: &str = r"[a-zA-Z0-9_\+=\.\-@ ]+";

/// `CustomLineItemName`'s and `BillingGroupName`'s `@pattern`, anchors stripped
/// (`smithy_query(query="shape",shape="CustomLineItemName,BillingGroupName")`). Both
/// shapes are `@sensitive`, so a clause about one states the pattern and no value.
const NAME_PATTERN: &str = r"[a-zA-Z0-9_\+=\.\-@]+";

/// `AccountId`'s `@pattern`, anchors stripped
/// (`smithy_query(query="shape",shape="AccountIdList")` — the list's member).
const ACCOUNT_ID_PATTERN: &str = "[0-9]{12}";

/// A value `BillingGroupArn` admits in its bare-id form, so a row can break ONE member
/// and leave the rest of the request conforming.
const VALID_GROUP_ID: &str = "ABCDEFGHIJ";

/// A live emulator holding only this service.
///
/// Local to this suite, like every other refusal suite in this crate: each drives the
/// methods and routes its own rows need.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the payer so
    /// the rows that assert a CONFORMING request is served are reachable. A modelled
    /// constraint is refused at the boundary AHEAD of the payer gate, so the designation
    /// does not affect the refusals this suite is about.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send a JSON body to one of this service's routes and return the status plus the
    /// parsed response body.
    async fn send(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://billingconductor.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "billingconductor.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/billingconductor/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// The wire error code of a failure response (`__type`), without any namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The modelled `Message` member of a response body.
fn message(body: &Value) -> Option<&str> {
    body.get("Message").and_then(Value::as_str)
}

/// One row: the case, the request, the whole `Message` it must be refused with, and a
/// value the response body may not contain anywhere.
struct Case {
    label: &'static str,
    method: &'static str,
    path: String,
    body: Value,
    expected: String,
    /// A value the caller sent that the model marks `@sensitive`; the whole body is
    /// searched for it. `None` for a row that sends no such value.
    withheld: Option<String>,
}

/// THE RULE, over the class of refusable requests: the clause states what was sent unless
/// the model marks it sensitive.
#[tokio::test]
async fn a_clause_states_the_submitted_value_unless_the_model_marks_it_sensitive() {
    let em = Emulator::new();

    // A tag value past `TagValue`'s maximum, and the elided echo the framework stores for
    // it (the value is the caller's own bytes up to the echo cap, then the length of what
    // was really sent).
    let long_tag = "v".repeat(300);
    let elided_tag = format!("{}... [{} bytes]", "v".repeat(256), long_tag.len());
    // 101 custom-line-item names: one past `CustomLineItemNameList`'s maximum, each
    // conforming to `CustomLineItemName`, so the ONLY rule broken is the list's length.
    let many_names: Vec<String> = (0..101).map(|index| format!("Secret_Name_{index}")).collect();

    let cases = vec![
        // The reported case: a list of a plain structure, so the list the caller sent is
        // stated — exactly as the structurally identical sibling filters below are.
        Case {
            label: "ListBillingGroups with an empty Filters.Names",
            method: "POST",
            path: "/list-billing-groups".into(),
            body: json!({"Filters": {"Names": []}}),
            expected: "1 validation error detected: Value '[]' at 'filters.names' failed to \
                       satisfy constraint: Member must have length greater than or equal to 1"
                .to_string(),
            withheld: None,
        },
        Case {
            label: "ListBillingGroups with an empty Filters.Arns",
            method: "POST",
            path: "/list-billing-groups".into(),
            body: json!({"Filters": {"Arns": []}}),
            expected: "1 validation error detected: Value '[]' at 'filters.arns' failed to \
                       satisfy constraint: Member must have length greater than or equal to 1"
                .to_string(),
            withheld: None,
        },
        Case {
            label: "ListBillingGroups with an empty Filters.Statuses",
            method: "POST",
            path: "/list-billing-groups".into(),
            body: json!({"Filters": {"Statuses": []}}),
            expected: "1 validation error detected: Value '[]' at 'filters.statuses' failed to \
                       satisfy constraint: Member must have length greater than or equal to 1"
                .to_string(),
            withheld: None,
        },
        Case {
            label: "ListBillingGroups with an empty Filters.PrimaryAccountIds",
            method: "POST",
            path: "/list-billing-groups".into(),
            body: json!({"Filters": {"PrimaryAccountIds": []}}),
            expected: "1 validation error detected: Value '[]' at 'filters.primaryAccountIds' \
                       failed to satisfy constraint: Member must have length greater than or \
                       equal to 1"
                .to_string(),
            withheld: None,
        },
        Case {
            label: "ListBillingGroups with an empty Filters.BillingGroupTypes",
            method: "POST",
            path: "/list-billing-groups".into(),
            body: json!({"Filters": {"BillingGroupTypes": []}}),
            expected: "1 validation error detected: Value '[]' at 'filters.billingGroupTypes' \
                       failed to satisfy constraint: Member must have length greater than or \
                       equal to 1"
                .to_string(),
            withheld: None,
        },
        Case {
            label: "ListBillingGroups with an empty Filters.ResponsibilityTransferArns",
            method: "POST",
            path: "/list-billing-groups".into(),
            body: json!({"Filters": {"ResponsibilityTransferArns": []}}),
            expected: "1 validation error detected: Value '[]' at \
                       'filters.responsibilityTransferArns' failed to satisfy constraint: \
                       Member must have length greater than or equal to 1"
                .to_string(),
            withheld: None,
        },
        // A member of the structure INSIDE that list: the path ends at a member the model
        // does not mark sensitive, so the value is stated there too.
        Case {
            label: "ListBillingGroups with a SearchValue the pattern refuses",
            method: "POST",
            path: "/list-billing-groups".into(),
            body: json!({
                "Filters": {"Names": [{"SearchOption": "STARTS_WITH", "SearchValue": "bad!value"}]}
            }),
            expected: format!(
                "1 validation error detected: Value 'bad!value' at \
                 'filters.names.1.member.searchValue' failed to satisfy constraint: Member \
                 must satisfy regular expression pattern: {SEARCH_VALUE_PATTERN}"
            ),
            withheld: None,
        },
        // THE SAME member name on the other filter, targeting a list of a @sensitive
        // string: the constraint is stated and the value is not, however the rule was
        // broken.
        Case {
            label: "ListCustomLineItems with an empty Filters.Names",
            method: "POST",
            path: "/list-custom-line-items".into(),
            body: json!({"Filters": {"Names": []}}),
            expected: "1 validation error detected: Value at 'filters.names' failed to satisfy \
                       constraint: Member must have length greater than or equal to 1"
                .to_string(),
            withheld: None,
        },
        Case {
            label: "ListCustomLineItems with one name past Filters.Names' maximum",
            method: "POST",
            path: "/list-custom-line-items".into(),
            body: json!({"Filters": {"Names": many_names}}),
            expected: "1 validation error detected: Value at 'filters.names' failed to satisfy \
                       constraint: Member must have length less than or equal to 100"
                .to_string(),
            withheld: Some("Secret_Name_7".to_string()),
        },
        Case {
            label: "ListCustomLineItems with a name the pattern refuses",
            method: "POST",
            path: "/list-custom-line-items".into(),
            body: json!({"Filters": {"Names": ["a name nobody may log!"]}}),
            expected: format!(
                "1 validation error detected: Value at 'filters.names' failed to satisfy \
                 constraint: Member must satisfy constraint: [Member must satisfy regular \
                 expression pattern: {NAME_PATTERN}]"
            ),
            withheld: Some("a name nobody may log!".to_string()),
        },
        // …while the sibling filters of that same operation state theirs.
        Case {
            label: "ListCustomLineItems with an empty Filters.Arns",
            method: "POST",
            path: "/list-custom-line-items".into(),
            body: json!({"Filters": {"Arns": []}}),
            expected: "1 validation error detected: Value '[]' at 'filters.arns' failed to \
                       satisfy constraint: Member must have length greater than or equal to 1"
                .to_string(),
            withheld: None,
        },
        Case {
            label: "ListCustomLineItems with an account id the pattern refuses",
            method: "POST",
            path: "/list-custom-line-items".into(),
            body: json!({"Filters": {"AccountIds": ["nope"]}}),
            expected: format!(
                "1 validation error detected: Value '[nope]' at 'filters.accountIds' failed to \
                 satisfy constraint: Member must satisfy constraint: [Member must satisfy \
                 regular expression pattern: {ACCOUNT_ID_PATTERN}]"
            ),
            withheld: None,
        },
        // A map entry's value, reached through the entry's caller-chosen key: `TagValue`
        // is not sensitive, so the clause states what was sent (already elided by the
        // framework's own echo cap).
        Case {
            label: "TagResource with a tag value past its modelled length",
            method: "POST",
            path: format!(
                "/tags/{}",
                encode_label(&format!(
                    "arn:aws:billingconductor::123456789012:billinggroup/{VALID_GROUP_ID}"
                ))
            ),
            body: json!({"Tags": {"env": long_tag}}),
            expected: format!(
                "1 validation error detected: Value '{elided_tag}' at 'tags.env' failed to \
                 satisfy constraint: Member must have length less than or equal to 256"
            ),
            withheld: None,
        },
        // A @sensitive SCALAR, on an operation of another surface entirely.
        Case {
            label: "UpdateBillingGroup with a Name the pattern refuses",
            method: "POST",
            path: "/update-billing-group".into(),
            body: json!({"Arn": VALID_GROUP_ID, "Name": "a name nobody may log!"}),
            expected: format!(
                "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: {NAME_PATTERN}"
            ),
            withheld: Some("a name nobody may log!".to_string()),
        },
        // A plain scalar under a @range bound: stated, like every other value the model
        // does not mark.
        Case {
            label: "ListBillingGroups with MaxResults below its modelled minimum",
            method: "POST",
            path: "/list-billing-groups".into(),
            body: json!({"MaxResults": 0}),
            expected: "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
                       constraint: Member must have value greater than or equal to 1"
                .to_string(),
            withheld: None,
        },
    ];

    for case in cases {
        let Case {
            label,
            method,
            path,
            body,
            expected,
            withheld,
        } = case;
        let (status, response) = em.send(method, &path, &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {label:?}: expected the modelled ValidationException, got {response:?}"
        );
        assert_eq!(
            message(&response),
            Some(expected.as_str()),
            "case {label:?}: expected the Message {expected:?}, got {response:?}"
        );
        if let Some(withheld) = withheld {
            let rendered = response.to_string();
            assert!(
                !rendered.contains(&withheld),
                "case {label:?}: the body must not echo the @sensitive value {withheld:?}, got \
                 {rendered:?}"
            );
        }
    }
}

/// RULE, at the one member where a rendering cannot be built from the request: a clause
/// never states a list the caller did not send. A list of STRUCTURES renders no element of
/// its own, so the value the framework stored for the collection stands — what it must not
/// do is report the empty list, and what it must not do either (the defect this suite is
/// about) is withhold the value of a member the model does not mark sensitive.
#[tokio::test]
async fn a_structure_list_is_never_stated_as_a_list_it_is_not() {
    let em = Emulator::new();

    // Rows differ along how far past `StringSearches`' maximum of one element the list
    // goes, so the stated value tracks what was sent rather than a fixed string.
    for count in [2_usize, 3, 5] {
        let label = format!("ListBillingGroups with {count} Filters.Names entries");
        let names: Vec<Value> = (0..count)
            .map(|index| json!({"SearchOption": "STARTS_WITH", "SearchValue": format!("Group{index}")}))
            .collect();
        let (status, response) = em
            .send("POST", "/list-billing-groups", &json!({"Filters": {"Names": names}}))
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {response:?}"
        );
        let expected = format!(
            "1 validation error detected: Value '{count} items' at 'filters.names' failed to \
             satisfy constraint: Member must have length less than or equal to 1"
        );
        assert_eq!(
            message(&response),
            Some(expected.as_str()),
            "case {label:?}: expected the Message {expected:?}, got {response:?}"
        );
    }
}

/// RULE: a request the model ACCEPTS is still served — the filters this suite drives to
/// their bounds are not refused when they are within them.
#[tokio::test]
async fn a_conforming_filter_is_served() {
    let em = Emulator::new();

    let accepted = [
        (
            "ListBillingGroups with the one Names entry the model allows",
            "/list-billing-groups",
            json!({
                "Filters": {"Names": [{"SearchOption": "STARTS_WITH", "SearchValue": "Alpha"}]}
            }),
        ),
        (
            "ListCustomLineItems with one name",
            "/list-custom-line-items",
            json!({"Filters": {"Names": ["Alpha_Item"]}}),
        ),
        (
            "ListBillingGroups with one Arn",
            "/list-billing-groups",
            json!({"Filters": {"Arns": [VALID_GROUP_ID]}}),
        ),
    ];

    for (label, path, body) in accepted {
        let (status, response) = em.send("POST", path, &body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the model accepts this request, so it must be served; got {status} \
             with {response:?}"
        );
    }
}

/// The PREMISE the rows above rest on, read off the model this crate is generated from:
/// of the two filter members spelled `Names`, exactly one targets a tree the model marks
/// `@sensitive`. A clause that decided from the member's name could not tell them apart.
#[test]
fn the_model_marks_one_names_filter_sensitive_and_not_the_other() {
    let model: Value = serde_json::from_str(MODEL).expect("parse the Smithy model");
    let shapes = model["shapes"].as_object().expect("the model's shapes");

    // (case, filter shape, member, whether the member's value is sensitive per the model)
    let rows = [
        (
            "the custom-line-item names filter",
            "com.amazonaws.billingconductor#ListCustomLineItemsFilter",
            "Names",
            true,
        ),
        (
            "the billing-group names filter",
            "com.amazonaws.billingconductor#ListBillingGroupsFilter",
            "Names",
            false,
        ),
        (
            "the billing-group arns filter",
            "com.amazonaws.billingconductor#ListBillingGroupsFilter",
            "Arns",
            false,
        ),
        (
            "the billing-group statuses filter",
            "com.amazonaws.billingconductor#ListBillingGroupsFilter",
            "Statuses",
            false,
        ),
        (
            "the created billing group's name",
            "com.amazonaws.billingconductor#CreateBillingGroupInput",
            "Name",
            true,
        ),
        (
            "the created billing group's tags",
            "com.amazonaws.billingconductor#CreateBillingGroupInput",
            "Tags",
            false,
        ),
    ];

    for (case, shape, member, expected) in rows {
        let declaration = shapes[shape]["members"][member]
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: the model declares {shape}${member}"));
        let sensitive = declares_sensitive(&Value::Object(declaration.clone()))
            || target_is_sensitive(shapes, &Value::Object(declaration.clone()));
        assert_eq!(
            sensitive, expected,
            "case {case:?}: expected the model to mark {shape}${member} sensitive={expected}, \
             got {sensitive}"
        );
    }
}

/// Does this declaration — a shape, or the member declaring one — carry `@sensitive`?
fn declares_sensitive(declaration: &Value) -> bool {
    declaration
        .get("traits")
        .and_then(|traits| traits.get(SENSITIVE_TRAIT))
        .is_some()
}

/// Does the shape a declaration targets keep its value out of a message: the shape itself
/// marked `@sensitive`, or, where it is a list or a map, what it holds?
fn target_is_sensitive(shapes: &Map<String, Value>, declaration: &Value) -> bool {
    let Some(target) = declaration
        .get("target")
        .and_then(Value::as_str)
        .and_then(|target| shapes.get(target))
    else {
        return false;
    };
    if declares_sensitive(target) {
        return true;
    }
    let slots: &[&str] = match target.get("type").and_then(Value::as_str) {
        Some("list") => &["member"],
        Some("map") => &["key", "value"],
        _ => return false,
    };
    slots.iter().any(|slot| {
        target
            .get(*slot)
            .is_some_and(|inner| declares_sensitive(inner) || target_is_sensitive(shapes, inner))
    })
}
