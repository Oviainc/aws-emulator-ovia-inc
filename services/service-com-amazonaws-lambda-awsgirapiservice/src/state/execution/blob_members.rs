// SPDX-License-Identifier: CC0-1.0

//! WHICH values of one answered call carry BYTES, read from the called service's own model.
//!
//! A JSON protocol has no byte type: a member whose shape is a `blob` travels as the base64
//! text of its bytes. An AWS SDK gives the caller the BYTES — `bytes` in Python, a byte array
//! in JavaScript — because the SDK knows from the service's model which members are blobs and
//! decodes exactly those. So the runtime has to know the same thing: this module reads it out
//! of the model and answers WHERE in an answer document those values sit.
//!
//! The answer is a list of PATHS — each a list of object keys and array indices from the root
//! of the output document — rather than a rewritten document, because the seam between the
//! host and a run's engine carries JSON text ([`super::aws_api::Answer`]): the host says which
//! values are bytes and the SDK the guest calls materialises them
//! ([`super::aws_sdk_guest`], [`super::node_sdk_guest`]). Nothing about a value is changed
//! here, and a path is reported only where the document really carries a value.
//!
//! What is KEPT between calls is a [`BlobPlan`]: the few shapes of that service whose values
//! can carry bytes at all, and which member of each leads to them. The model document itself
//! is parsed once to build that plan and then dropped, so a service whose answers carry bytes
//! in one corner of one operation does not cost this process its whole model for the life of
//! the run. The plan is a GRAPH keyed by shape id rather than a tree, which is what lets a
//! shape that contains itself — a DynamoDB attribute value holds a list of attribute values —
//! be described at all.
//!
//! The walk over an answer is driven by the DOCUMENT: it descends only where the answer
//! carries a value, so a plan whose shapes refer to one another cannot make it recurse without
//! end — the document's own nesting, which `serde_json`'s parse limit bounds, bounds the walk.
//!
//! Two kinds of blob are deliberately left out, because neither travels as base64 in a JSON
//! body:
//!
//! * a `@streaming` blob, which an SDK hands over as a stream rather than as bytes and which
//!   this runtime reports separately ([`super::aws_api::Answer::streams`]);
//! * a member an answer binds somewhere other than its JSON body — an HTTP payload, a header,
//!   the response code — whose value is the bytes or the text of that binding
//!   ([`super::rest_json`] reads those).

use ::std::collections::{BTreeMap, BTreeSet};
use ::std::sync::OnceLock;

use ::serde_json::{Map, Value};

/// The trait that makes a blob a STREAM rather than bytes an SDK hands over whole.
const STREAMING: &str = "smithy.api#streaming";

/// The name a member travels under in a JSON body, when it is not the member's own name.
const JSON_NAME: &str = "smithy.api#jsonName";

/// The traits that bind a member of an ANSWER somewhere other than its JSON body, so what the
/// value reads as is that binding's and not the base64 a JSON body carries.
const OUTSIDE_THE_BODY: [&str; 4] = [
    "smithy.api#httpPayload",
    "smithy.api#httpHeader",
    "smithy.api#httpPrefixHeaders",
    "smithy.api#httpResponseCode",
];

/// One service's Smithy model, and where the plan read out of it is kept.
///
/// The source is the SAME file that service's own emulator crate is generated from, embedded by
/// [`super::aws_api`], so what is read here cannot drift from the surface that service serves.
/// The plan is built once per process, on the first call to the service.
#[derive(Debug)]
pub struct ServiceModel {
    /// The model document.
    pub source: &'static str,
    /// The plan read out of it.
    pub plan: OnceLock<BlobPlan>,
}

impl ServiceModel {
    /// The plan, reading it on first use.
    pub fn plan(&self) -> &BlobPlan {
        self.plan.get_or_init(|| BlobPlan::of(self.source))
    }

    /// Where the answer of `operation` carries BYTES, in this service.
    pub fn answer_paths(&self, operation: &str, document: &Value) -> Vec<Value> {
        self.plan().answer_paths(operation, document)
    }
}

/// Which values of one service's answers carry bytes: the output shape of each operation whose
/// answer can carry any, and the shapes that lead to them.
#[derive(Debug, Default)]
pub struct BlobPlan {
    /// The output shape of each operation whose answer can carry bytes, by the operation's own
    /// short name — which is the name a client calls it by.
    outputs: BTreeMap<String, String>,
    /// Every shape whose values can carry bytes, by shape id.
    shapes: BTreeMap<String, Carrier>,
}

/// How one shape's values carry bytes.
#[derive(Debug)]
enum Carrier {
    /// The value IS the bytes: a blob the wire carries as base64 text.
    Bytes,
    /// A structure or a union: these of its members lead to bytes.
    Members(Vec<Member>),
    /// A list or a set: every entry is this shape.
    Entries(String),
    /// A map: every value is this shape.
    Values(String),
}

/// One member of a structure or a union that leads to bytes.
#[derive(Debug)]
struct Member {
    /// The member's own name, which is what a REST-bound answer's document is keyed by.
    name: String,
    /// The name `@jsonName` gives it on the wire, when it carries that trait.
    wire: Option<String>,
    /// The shape it targets.
    target: String,
}

impl BlobPlan {
    /// The plan `source`, a Smithy model document, states.
    #[must_use]
    pub fn of(source: &str) -> Self {
        let Some(shapes) = ::serde_json::from_str::<Value>(source)
            .ok()
            .and_then(|document| document.get("shapes").cloned())
            .and_then(|shapes| match shapes {
                Value::Object(shapes) => Some(shapes),
                _ => None,
            })
        else {
            return Self::default();
        };
        let carrying = carrying_shapes(&shapes);
        let mut plan = Self::default();
        for id in &carrying {
            if let Some(carrier) = carrier_of(&shapes, id, &carrying) {
                plan.shapes.insert(id.clone(), carrier);
            }
        }
        for (id, shape) in &shapes {
            if shape.get("type").and_then(Value::as_str) != Some("operation") {
                continue;
            }
            let Some(output) = shape
                .get("output")
                .and_then(|output| output.get("target"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !plan.shapes.contains_key(output) {
                continue;
            }
            if let Some(name) = id.split('#').next_back().filter(|name| !name.is_empty()) {
                plan.outputs.insert(name.to_string(), output.to_string());
            }
        }
        plan
    }

    /// Where the answer of `operation` carries BYTES: one path per value this plan types as a
    /// blob and the document carries as text.
    ///
    /// `document` is the output document as the guest will read it — the answered body for a
    /// service whose protocol carries the output document as the body, and the document
    /// [`super::rest_json::read`] built for a REST-bound one, which is keyed by the output's
    /// own member names. A key is looked up under the member's own name and under the name
    /// `@jsonName` gives it, so one walk serves both.
    #[must_use]
    pub fn answer_paths(&self, operation: &str, document: &Value) -> Vec<Value> {
        let Some(output) = self.outputs.get(operation) else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        self.walk(output, document, &mut Vec::new(), &mut paths);
        paths
    }

    /// Collect the paths of every blob value `document` carries under the shape `id`.
    fn walk(&self, id: &str, document: &Value, path: &mut Vec<Value>, paths: &mut Vec<Value>) {
        match self.shapes.get(id) {
            Some(Carrier::Bytes) => {
                // A blob the document carries as text is the base64 of its bytes; anything
                // else is not a value this seam decodes.
                if document.is_string() {
                    paths.push(Value::Array(path.clone()));
                }
            }
            Some(Carrier::Members(members)) => {
                let Some(object) = document.as_object() else {
                    return;
                };
                for member in members {
                    let Some((key, carried)) = member.carried(object) else {
                        continue;
                    };
                    path.push(Value::String(key));
                    self.walk(&member.target, carried, path, paths);
                    path.pop();
                }
            }
            Some(Carrier::Entries(entry)) => {
                let Some(entries) = document.as_array() else {
                    return;
                };
                for (index, carried) in entries.iter().enumerate() {
                    path.push(Value::from(index));
                    self.walk(entry, carried, path, paths);
                    path.pop();
                }
            }
            Some(Carrier::Values(value)) => {
                let Some(entries) = document.as_object() else {
                    return;
                };
                for (key, carried) in entries {
                    path.push(Value::String(key.clone()));
                    self.walk(value, carried, path, paths);
                    path.pop();
                }
            }
            None => {}
        }
    }
}

impl Member {
    /// The value `object` carries for this member, and the key it carries it under: the
    /// member's own name, or the name `@jsonName` gives it on the wire.
    fn carried<'a>(&self, object: &'a Map<String, Value>) -> Option<(String, &'a Value)> {
        if let Some(carried) = object.get(&self.name) {
            return Some((self.name.clone(), carried));
        }
        let wire = self.wire.as_deref()?;
        let carried = object.get(wire)?;
        Some((wire.to_string(), carried))
    }
}

/// Every shape of `shapes` whose values can carry bytes.
///
/// Computed by passes over the shapes until nothing more is added, rather than by descending
/// each shape: a shape that contains itself — a DynamoDB attribute value holds a list of
/// attribute values — has no bottom to descend to, while the passes simply stop when the set
/// stops growing. The model's shape count bounds how many passes there can be.
fn carrying_shapes(shapes: &Map<String, Value>) -> BTreeSet<String> {
    let mut carrying: BTreeSet<String> = BTreeSet::new();
    // The blobs themselves, including the prelude's own `smithy.api#Blob`, which a member may
    // target although no service declares it.
    for id in shapes
        .keys()
        .map(String::as_str)
        .chain(::std::iter::once(PRELUDE_BLOB))
    {
        if kind_of(shapes, id) == "blob" && !is_streaming(shapes, id) {
            carrying.insert(id.to_string());
        }
    }
    loop {
        let mut grew = false;
        for id in shapes.keys() {
            if carrying.contains(id) {
                continue;
            }
            if leads_to_bytes(shapes, id, &carrying) {
                carrying.insert(id.clone());
                grew = true;
            }
        }
        if !grew {
            return carrying;
        }
    }
}

/// Whether the shape `id` leads to a value in `carrying`.
fn leads_to_bytes(shapes: &Map<String, Value>, id: &str, carrying: &BTreeSet<String>) -> bool {
    let reached = |field: &str| {
        nested(shapes, id, field).is_some_and(|member| carrying.contains(target_of(member)))
    };
    match kind_of(shapes, id) {
        "structure" | "union" => members_of(shapes, id).is_some_and(|members| {
            members
                .values()
                .any(|member| carrying.contains(target_of(member)))
        }),
        "list" | "set" => reached("member"),
        "map" => reached("value"),
        _ => false,
    }
}

/// How the shape `id` carries bytes, for a shape already known to carry them.
fn carrier_of(
    shapes: &Map<String, Value>,
    id: &str,
    carrying: &BTreeSet<String>,
) -> Option<Carrier> {
    match kind_of(shapes, id) {
        "blob" => Some(Carrier::Bytes),
        "structure" | "union" => {
            let members = members_of(shapes, id)?
                .iter()
                .filter(|(_name, member)| carrying.contains(target_of(member)))
                // A member an answer binds outside its JSON body carries no base64 to
                // decode, so it is not part of the plan at all.
                .filter(|(_name, member)| {
                    !OUTSIDE_THE_BODY
                        .iter()
                        .any(|bound| member_trait(member, bound).is_some())
                })
                .map(|(name, member)| Member {
                    name: name.clone(),
                    wire: member_trait(member, JSON_NAME)
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    target: target_of(member).to_string(),
                })
                .collect::<Vec<Member>>();
            (!members.is_empty()).then_some(Carrier::Members(members))
        }
        "list" | "set" => Some(Carrier::Entries(
            target_of(nested(shapes, id, "member")?).to_string(),
        )),
        "map" => Some(Carrier::Values(
            target_of(nested(shapes, id, "value")?).to_string(),
        )),
        _ => None,
    }
}

/// The prelude's own blob shape, which a member may target although no service declares it.
const PRELUDE_BLOB: &str = "smithy.api#Blob";

/// The type the shape `id` declares (`structure`, `list`, `blob`, …), or the prelude's own
/// type for a shape of the prelude, which a service's model does not declare.
fn kind_of(shapes: &Map<String, Value>, id: &str) -> &'static str {
    if let Some(kind) = shapes
        .get(id)
        .and_then(|shape| shape.get("type"))
        .and_then(Value::as_str)
    {
        return match kind {
            "blob" => "blob",
            "structure" => "structure",
            "union" => "union",
            "list" => "list",
            "set" => "set",
            "map" => "map",
            _ => "",
        };
    }
    match id {
        PRELUDE_BLOB => "blob",
        _ => "",
    }
}

/// Whether the shape `id` carries the `@streaming` trait.
fn is_streaming(shapes: &Map<String, Value>, id: &str) -> bool {
    shapes
        .get(id)
        .and_then(|shape| shape.get("traits"))
        .and_then(Value::as_object)
        .is_some_and(|traits| traits.contains_key(STREAMING))
}

/// The members of the shape `id`, or `None` for a shape declaring none.
fn members_of<'a>(shapes: &'a Map<String, Value>, id: &str) -> Option<&'a Map<String, Value>> {
    shapes
        .get(id)
        .and_then(|shape| shape.get("members"))
        .and_then(Value::as_object)
}

/// The nested shape reference `field` of the shape `id` (`member` of a list, `value` of a
/// map), or `None` for a shape that declares none.
fn nested<'a>(shapes: &'a Map<String, Value>, id: &str, field: &str) -> Option<&'a Value> {
    shapes.get(id).and_then(|shape| shape.get(field))
}

/// The shape a member reference targets.
fn target_of(member: &Value) -> &str {
    member
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The value of the trait `name` on `member`, if it carries it.
fn member_trait<'a>(member: &'a Value, name: &str) -> Option<&'a Value> {
    member
        .get("traits")
        .and_then(Value::as_object)
        .and_then(|traits| traits.get(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::serde_json::json;

    /// The paths a plan reported, rendered and SORTED.
    ///
    /// Which order they come in is not part of the rule — each one names one value, and the
    /// SDK materialises them one by one — and it follows the order the document's own keys
    /// come in, so the rows below compare the SET of paths rather than a sequence.
    fn rendered(paths: &[Value]) -> Vec<String> {
        let mut rendered: Vec<String> = paths
            .iter()
            .map(|path| {
                path.as_array()
                    .map(|steps| {
                        steps
                            .iter()
                            .map(|step| match step {
                                Value::String(key) => key.clone(),
                                other => other.to_string(),
                            })
                            .collect::<Vec<String>>()
                            .join(".")
                    })
                    .unwrap_or_default()
            })
            .collect();
        rendered.sort();
        rendered
    }

    /// The model the rows below are read against: one operation whose output carries a blob
    /// member, a blob inside a list, a blob inside a map, a blob under a `@jsonName`, a
    /// streaming blob, a blob bound to the payload, and a text member.
    const MODEL: &str = r#"{
      "smithy": "2.0",
      "shapes": {
        "example#Read": {
          "type": "operation",
          "input": {"target": "example#ReadInput"},
          "output": {"target": "example#ReadOutput"}
        },
        "example#Write": {
          "type": "operation",
          "input": {"target": "example#ReadInput"},
          "output": {"target": "example#ReadInput"}
        },
        "example#ReadInput": {"type": "structure", "members": {}},
        "example#ReadOutput": {
          "type": "structure",
          "members": {
            "Value": {"target": "example#Bytes"},
            "Text": {"target": "smithy.api#String"},
            "Named": {
              "target": "example#Bytes",
              "traits": {"smithy.api#jsonName": "named"}
            },
            "Entries": {"target": "example#Entries"},
            "Held": {"target": "example#Held"},
            "Nested": {"target": "example#Nested"},
            "Stream": {"target": "example#Streamed"},
            "Payload": {
              "target": "example#Bytes",
              "traits": {"smithy.api#httpPayload": {}}
            }
          }
        },
        "example#Nested": {
          "type": "structure",
          "members": {"Inner": {"target": "example#Bytes"}}
        },
        "example#Entries": {"type": "list", "member": {"target": "example#Bytes"}},
        "example#Held": {
          "type": "map",
          "key": {"target": "smithy.api#String"},
          "value": {"target": "example#Bytes"}
        },
        "example#Bytes": {"type": "blob"},
        "example#Streamed": {"type": "blob", "traits": {"smithy.api#streaming": {}}}
      }
    }"#;

    /// Every blob value an answer carries is reported by its path, at whatever depth and
    /// under whichever name the wire carried it — and a value that is not one is not.
    #[test]
    fn every_blob_value_of_an_answer_is_reported_by_its_path() {
        let plan = BlobPlan::of(MODEL);
        // Each row: the answer document, and the paths its blob values sit at.
        let cases: &[(&str, Value, Value)] = &[
            (
                "a blob member of the output structure",
                json!({"Value": "AAE=", "Text": "not bytes"}),
                json!([["Value"]]),
            ),
            (
                "a blob member under the name @jsonName gives it",
                json!({"named": "AAE="}),
                json!([["named"]]),
            ),
            (
                "a blob member under its own name, which @jsonName renames on the wire",
                json!({"Named": "AAE="}),
                json!([["Named"]]),
            ),
            (
                "a blob inside a structure the output carries",
                json!({"Nested": {"Inner": "AAE="}}),
                json!([["Nested", "Inner"]]),
            ),
            (
                "a blob at each index of a list",
                json!({"Entries": ["AAE=", "Ag=="]}),
                json!([["Entries", 0], ["Entries", 1]]),
            ),
            (
                "a blob under each key of a map",
                json!({"Held": {"first": "AAE="}}),
                json!([["Held", "first"]]),
            ),
            (
                "a streaming blob, which is handed over as a stream",
                json!({"Stream": "AAE="}),
                json!([]),
            ),
            (
                "a blob bound to the answer's payload, which is its own bytes",
                json!({"Payload": "not base64 at all"}),
                json!([]),
            ),
            (
                "several blob values at once",
                json!({"Value": "AAE=", "Nested": {"Inner": "Ag=="}}),
                json!([["Nested", "Inner"], ["Value"]]),
            ),
            (
                "an answer carrying none of them",
                json!({"Text": "AAE="}),
                json!([]),
            ),
        ];
        for (case, document, expected) in cases {
            let paths = plan.answer_paths("Read", document);
            let expected = rendered(expected.as_array().expect("the expectation is a list"));
            assert_eq!(
                rendered(&paths),
                expected,
                "case {case:?}: expected the blob paths {expected:?}, got {paths:?}"
            );
        }
    }

    /// An operation the model does not bind, and one whose output carries no blob at all,
    /// report no path rather than guessing at the answer's shape.
    #[test]
    fn an_operation_whose_answer_carries_no_bytes_reports_no_path() {
        let plan = BlobPlan::of(MODEL);
        let document = json!({"Value": "AAE=", "Entries": ["AAE="]});
        for operation in ["Write", "ReadOutput", "Describe", ""] {
            let paths = plan.answer_paths(operation, &document);
            assert!(
                paths.is_empty(),
                "case {operation:?}: expected no blob path, got {paths:?}"
            );
        }
    }

    /// The plan read out of a model this crate really embeds, for an operation the issue that
    /// prompted this module did not cite: Secrets Manager's `GetSecretValue` carries the
    /// secret's bytes in `SecretBinary` (`SecretBinaryType`, a blob) and its text in
    /// `SecretString`, so one is reported and the other is not.
    #[test]
    fn a_blob_member_of_an_embedded_services_model_is_reported() {
        let plan = BlobPlan::of(super::super::aws_api::SECRETSMANAGER_MODEL);
        let paths = plan.answer_paths(
            "GetSecretValue",
            &json!({"SecretBinary": "AAE=", "SecretString": "text", "Name": "a-secret"}),
        );
        assert_eq!(
            rendered(&paths),
            vec!["SecretBinary".to_string()],
            "expected the blob member of GetSecretValue's answer, got {paths:?}"
        );
    }

    /// The same, for the case that makes the DEPTH of the walk the point: a DynamoDB attribute
    /// value carries bytes in `B` and in `BS`, and an attribute value holds lists (`L`) and
    /// maps (`M`) of attribute values, so the bytes of an item sit at whatever depth the item
    /// put them.
    #[test]
    fn a_blob_nested_in_an_embedded_services_model_is_reported_at_its_depth() {
        let plan = BlobPlan::of(super::super::aws_api::DYNAMODB_MODEL);
        let paths = plan.answer_paths(
            "GetItem",
            &json!({
                "Item": {
                    "pk": {"S": "text"},
                    "data": {"B": "AAE="},
                    "set": {"BS": ["AAE=", "Ag=="]},
                    "list": {"L": [{"B": "AAE="}]},
                    "map": {"M": {"inner": {"B": "AAE="}}},
                },
            }),
        );
        assert_eq!(
            rendered(&paths),
            vec![
                "Item.data.B".to_string(),
                "Item.list.L.0.B".to_string(),
                "Item.map.M.inner.B".to_string(),
                "Item.set.BS.0".to_string(),
                "Item.set.BS.1".to_string(),
            ],
            "expected every blob of the item at its own depth, got {paths:?}"
        );
    }
}
