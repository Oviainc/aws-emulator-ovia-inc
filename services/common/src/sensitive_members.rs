// SPDX-License-Identifier: CC0-1.0

//! Which member paths a boundary refusal states NO value for, read from a service's
//! own Smithy model.
//!
//! A constraint refusal names the member it refused and, for most members, echoes the
//! value it was handed — that echo is the one place a service reports the offending
//! input back, so a caller can see what to fix. A member the model marks `@sensitive`
//! is the exception: its value is key material, a prompt, a flow definition or a
//! password the caller sent, and a service that echoes it back has disclosed it into
//! every log and trace the response passes through. Such a clause names the member and
//! the requirement and states no value at all —
//! [`crate::validation_message::aws_validation_refusal_withholding`] is what drops that
//! half, and this module is the DECISION it asks for.
//!
//! WHICH members those are is read from the service's own model — the same file the
//! generated schemas are built from — rather than listed per service, so the rule holds
//! for every `@sensitive` member of every operation, at whatever depth a refusal found
//! one, and nothing is keyed to an operation, a member name or a value. It has to be
//! read from the model because the runtime schemas the validating walk carries hold the
//! CONSTRAINT traits and not `@sensitive`
//! (`vendor/smithy-schema-server/src/constraints.rs`, read-only from here): the walk can
//! say which member broke which bound, not which member may be quoted.
//!
//! What a refusal path carries is member NAMES — the walk spells a path as
//! `<member>.<position>.member.<member>` — and not the shapes they were reached
//! through, so the decision is made per name, over the members an operation's INPUT can
//! reach:
//!
//! * a name every input-reachable member of which is `@sensitive` is withheld, at
//!   whatever depth a path names it;
//! * a name the model marks on one input member and not on another is NOT withheld: the
//!   path cannot tell the two apart, and withholding both would drop the value half
//!   from clauses the service states it in;
//! * a member reachable only from an OUTPUT (an error shape's `message`, say) decides
//!   nothing, since no refusal is ever reported at one.
//!
//! This is the shared home for that reading. Two services carry their own copy of it —
//! `services/service-com-amazonaws-kms-trentservice/src/service/sensitive_members.rs`
//! and
//! `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/service/sensitive_members.rs`,
//! each pinned by that service's own tests — and migrating them onto this module is
//! their own change; nothing NEW should state the rule again.

use std::collections::{HashMap, HashSet};

use aws_smithy_schema_server::constraints::path;
use serde_json::{Map, Value};

/// The Smithy trait that marks a shape or a member as one whose value is not to be
/// disclosed.
const SENSITIVE_TRAIT: &str = "smithy.api#sensitive";

/// The members of one service's model whose value a refusal withholds.
///
/// Built once per service from the model text (a compile-time constant, so the answer
/// cannot change while the process runs) and asked per refusal.
#[derive(Debug, Clone, Default)]
pub struct WithheldMembers {
    /// Each withheld member, spelled the way the validating walk spells a path SEGMENT.
    names: HashSet<String>,
}

impl WithheldMembers {
    /// The withheld members `model_json` declares, by the rule in the module header.
    ///
    /// A model that cannot be read yields an empty set, which withholds nothing and
    /// keeps the framework's own wording — the fail-safe direction, since that wording
    /// is what the framework states without this seam at all.
    #[must_use]
    pub fn from_model(model_json: &str) -> Self {
        let model: Value = serde_json::from_str(model_json).unwrap_or(Value::Null);
        let Some(shapes) = model.get("shapes").and_then(Value::as_object) else {
            return Self::default();
        };

        // Every member name an operation's input can reach, against whether EVERY
        // member of that name is one the model marks `@sensitive`.
        let mut sensitive_everywhere: HashMap<String, bool> = HashMap::new();
        let mut walked: HashSet<&str> = HashSet::new();
        let mut walk: Vec<&str> = shapes
            .values()
            .filter(|shape| shape.get("type").and_then(Value::as_str) == Some("operation"))
            .filter_map(|operation| target_of(operation.get("input")))
            .collect();

        while let Some(shape_id) = walk.pop() {
            if !walked.insert(shape_id) {
                continue;
            }
            let Some(shape) = shapes.get(shape_id) else {
                continue;
            };
            // A structure's and a union's members are the ones a path NAMES.
            if let Some(members) = shape.get("members").and_then(Value::as_object) {
                for (name, member) in members {
                    let sensitive = is_sensitive(member, shapes);
                    sensitive_everywhere
                        .entry(path::member(name))
                        .and_modify(|every| *every &= sensitive)
                        .or_insert(sensitive);
                    walk.extend(target_of(Some(member)));
                }
            }
            // A collection's element, key and value are steps a path takes without
            // naming a member of its own, so they are walked but not recorded.
            for step in ["member", "key", "value"] {
                walk.extend(target_of(shape.get(step)));
            }
        }

        Self {
            names: sensitive_everywhere
                .into_iter()
                .filter(|(_, sensitive)| *sensitive)
                .map(|(name, _)| name)
                .collect(),
        }
    }

    /// Whether a refusal reported at `member_path` states no value.
    ///
    /// ANY segment decides, not only the last: what sits under a `@sensitive` member is
    /// part of that member's value, so a clause about a member nested inside one
    /// withholds the value too. The path is the dotted one the validating walk builds,
    /// which spells a collection step as `<1-based position>.member` rather than as a
    /// subscript, so splitting on `.` reaches every segment of it.
    #[must_use]
    pub fn withholds_value(&self, member_path: &str) -> bool {
        member_path
            .split('.')
            .any(|segment| self.names.contains(segment))
    }

    /// The member names withheld, for a service's own test to pin against its model.
    #[must_use]
    pub fn names(&self) -> &HashSet<String> {
        &self.names
    }
}

/// The shape id a member or an operation's input/output node targets.
fn target_of(node: Option<&Value>) -> Option<&str> {
    node?.get("target").and_then(Value::as_str)
}

/// Whether `member` is one whose value is not disclosed: the trait sits on the member
/// itself, on the shape it targets (a `@sensitive` structure makes every value inside
/// it sensitive), or on the element, key or value of a collection it targets — a list
/// of a sensitive shape carries sensitive values just as a member of it does.
fn is_sensitive(member: &Value, shapes: &Map<String, Value>) -> bool {
    if carries_sensitive_trait(member) {
        return true;
    }
    let Some(target) = target_of(Some(member)).and_then(|target| shapes.get(target)) else {
        return false;
    };
    if carries_sensitive_trait(target) {
        return true;
    }
    ["member", "key", "value"].iter().any(|step| {
        target.get(*step).is_some_and(|element| {
            carries_sensitive_trait(element)
                || target_of(Some(element))
                    .and_then(|id| shapes.get(id))
                    .is_some_and(carries_sensitive_trait)
        })
    })
}

/// Whether a shape or member node declares [`SENSITIVE_TRAIT`].
fn carries_sensitive_trait(node: &Value) -> bool {
    node.get("traits")
        .and_then(Value::as_object)
        .is_some_and(|traits| traits.contains_key(SENSITIVE_TRAIT))
}

#[cfg(test)]
mod tests {
    use super::WithheldMembers;

    /// A model in the shape a Smithy JSON AST carries, covering each way a member is or
    /// is not withheld: the trait on the member, the trait on the shape it targets, the
    /// trait on a list's element shape, a name marked on one input member and not on
    /// another, a name marked only inside an output, and a name marked nowhere.
    const MODEL: &str = r#"{
        "smithy": "2.0",
        "shapes": {
            "example#Put": {
                "type": "operation",
                "input": { "target": "example#PutRequest" },
                "output": { "target": "example#PutResponse" },
                "errors": [{ "target": "example#Oops" }]
            },
            "example#List": {
                "type": "operation",
                "input": { "target": "example#ListRequest" }
            },
            "example#PutRequest": {
                "type": "structure",
                "members": {
                    "Plaintext": { "target": "example#Blob", "traits": { "smithy.api#sensitive": {} } },
                    "Secret": { "target": "example#SecretString" },
                    "Name": { "target": "smithy.api#String" },
                    "Variants": { "target": "example#Variants" },
                    "Nested": { "target": "example#Nested" }
                }
            },
            "example#ListRequest": {
                "type": "structure",
                "members": {
                    "Secret": { "target": "smithy.api#String" }
                }
            },
            "example#PutResponse": {
                "type": "structure",
                "members": {
                    "Receipt": { "target": "example#SecretString" }
                }
            },
            "example#Oops": {
                "type": "structure",
                "members": {
                    "Message": { "target": "example#SecretString" }
                }
            },
            "example#Nested": {
                "type": "structure",
                "members": {
                    "Label": { "target": "smithy.api#String" }
                }
            },
            "example#SecretString": { "type": "string", "traits": { "smithy.api#sensitive": {} } },
            "example#Variants": { "type": "list", "member": { "target": "example#Variant" } },
            "example#Variant": {
                "type": "structure",
                "traits": { "smithy.api#sensitive": {} },
                "members": { "Label": { "target": "smithy.api#String" } }
            },
            "example#Blob": { "type": "blob" }
        }
    }"#;

    /// RULE: a name every input-reachable member of which the model marks `@sensitive`
    /// is withheld at whatever depth a path names it; a name the model marks on one
    /// input member and not another, a name reachable only from an output, and a name
    /// marked nowhere all keep the framework's value half.
    #[test]
    fn the_model_decides_which_members_are_withheld() {
        let withheld = WithheldMembers::from_model(MODEL);
        // (case, member path, withheld?)
        let rows: [(&str, &str, bool); 9] = [
            ("the trait on the member", "plaintext", true),
            ("the trait on the targeted shape", "secret", false),
            (
                "the trait on a list's element shape",
                "variants.1.member.label",
                true,
            ),
            ("a member marked nowhere", "name", false),
            (
                "a member nested under a withheld one",
                "plaintext.anything",
                true,
            ),
            (
                "a member nested under one marked nowhere",
                "nested.label",
                false,
            ),
            ("a member no shape declares", "somethingElse", false),
            // `message` is sensitive in the error shape only, which no refusal is
            // reported at, so nothing about it is withheld.
            ("a name marked only inside an output", "message", false),
            ("a name marked only inside an output, nested", "receipt", false),
        ];
        for (case, path, expected) in rows {
            assert_eq!(
                withheld.withholds_value(path),
                expected,
                "case {case:?}: expected withholds_value({path:?}) to be {expected}, got the \
                 withheld set {:?}",
                withheld.names()
            );
        }
    }

    /// RULE: a model that cannot be read withholds nothing, so the framework's own
    /// wording stands rather than a value half dropped by accident.
    #[test]
    fn an_unreadable_model_withholds_nothing() {
        for (case, model) in [
            ("not JSON at all", "{"),
            ("JSON carrying no shapes", r#"{"smithy":"2.0"}"#),
            ("an empty document", ""),
        ] {
            let withheld = WithheldMembers::from_model(model);
            assert!(
                withheld.names().is_empty() && !withheld.withholds_value("plaintext"),
                "case {case:?}: expected nothing to be withheld, got {:?}",
                withheld.names()
            );
        }
    }
}
