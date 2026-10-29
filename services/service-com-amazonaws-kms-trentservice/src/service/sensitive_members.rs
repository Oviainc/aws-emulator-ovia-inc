// SPDX-License-Identifier: CC0-1.0

//! Which members of this service a refusal states NO value for.
//!
//! A constraint refusal names the member it refused and, for most members, echoes the
//! value it was handed — that echo is the one place the service reports the offending
//! input back, so a caller can see what to fix. A member the model marks `@sensitive` is
//! the exception: its value is key material, a plaintext or a password the caller sent,
//! and a service that echoes it back has disclosed it into every log and trace the
//! response passes through. Such a clause names the member and the requirement and
//! states no value at all.
//!
//! WHICH members those are is read from this service's own Smithy model — the same file
//! the generated schemas are built from — rather than listed here, so the rule holds for
//! every `@sensitive` member of every operation in this service, at whatever depth a
//! refusal found it, and nothing is keyed to an operation name, a member name or a
//! value. It has to be read here because the runtime schemas the validating walk carries
//! hold the CONSTRAINT traits and not `@sensitive`
//! (`vendor/smithy-schema-server/src/constraints.rs`, which is read-only from here): the
//! walk can say which member broke which bound, not which member may be quoted.
//!
//! The clause, the envelope, the member paths and every requirement stay where they
//! already live — the framework writes them and
//! [`service_common::validation_message`] restates the value half — so this module
//! decides one thing only: which member paths are withheld.

use std::collections::HashSet;
use std::sync::LazyLock;

use aws_smithy_schema_server::constraints::path;
use serde_json::{Map, Value};

/// This service's Smithy model: the one source for which members are `@sensitive`.
const MODEL_JSON: &str = include_str!("../../smithy-model/com-amazonaws-kms-trentservice.json");

/// The Smithy trait that marks a shape or a member as one whose value is not to be
/// disclosed.
const SENSITIVE_TRAIT: &str = "smithy.api#sensitive";

/// Every member path this service withholds the value of, as the model declares them.
///
/// Built once: the model is a compile-time constant, so the answer cannot change while
/// the process runs.
static WITHHELD_MEMBERS: LazyLock<HashSet<String>> = LazyLock::new(withheld_members);

/// Whether a refusal naming `member_path` states no value.
///
/// The path is the dotted one the validating walk builds (`plaintext`,
/// `xksProxyAuthenticationCredential.accessKeyId`), so a member nested inside a
/// structure or reached through a collection is answered for too: a segment naming a
/// withheld member makes the whole value one this service does not disclose, since what
/// sits under a sensitive member is part of its value.
pub(crate) fn withholds_value(member_path: &str) -> bool {
    member_path
        .split('.')
        .any(|segment| WITHHELD_MEMBERS.contains(segment))
}

/// The member paths of every `@sensitive` member the model declares, spelled the way the
/// validating walk spells a path segment ([`path::member`], asked rather than copied so
/// the two cannot disagree about how one member is named).
fn withheld_members() -> HashSet<String> {
    let model: Value = serde_json::from_str(MODEL_JSON).unwrap_or(Value::Null);
    let Some(shapes) = model.get("shapes").and_then(Value::as_object) else {
        return HashSet::new();
    };
    shapes
        .values()
        .filter_map(|shape| shape.get("members").and_then(Value::as_object))
        .flat_map(|members| members.iter())
        .filter(|(_, member)| is_sensitive(member, shapes))
        .map(|(name, _)| path::member(name))
        .collect()
}

/// Whether `member` is one whose value is not disclosed: the trait sits on the member
/// itself, or on the shape the member targets (a `@sensitive` blob type makes every
/// member of it sensitive).
fn is_sensitive(member: &Value, shapes: &Map<String, Value>) -> bool {
    if carries_sensitive_trait(member) {
        return true;
    }
    member
        .get("target")
        .and_then(Value::as_str)
        .and_then(|target| shapes.get(target))
        .is_some_and(carries_sensitive_trait)
}

/// Whether a shape or member node declares [`SENSITIVE_TRAIT`].
fn carries_sensitive_trait(node: &Value) -> bool {
    node.get("traits")
        .and_then(Value::as_object)
        .is_some_and(|traits| traits.contains_key(SENSITIVE_TRAIT))
}

#[cfg(test)]
mod tests {
    use super::{withholds_value, WITHHELD_MEMBERS};

    /// The members this service withholds are the ones the model marks `@sensitive`,
    /// read off the model rather than listed: `Encrypt$Plaintext` and the `Message` of
    /// `Sign`, `Verify`, `GenerateMac` and `VerifyMac` (`PlaintextType`), the
    /// `KeyStorePassword` of `CreateCustomKeyStore` and `UpdateCustomKeyStore`
    /// (`KeyStorePasswordType`), and the two members of
    /// `XksProxyAuthenticationCredentialType` (`smithy_query(query="operation",…)` and
    /// `smithy_query(query="shape",…)` report `sensitive: true` for each).
    #[test]
    fn every_sensitive_member_of_the_model_is_withheld() {
        for member in [
            "plaintext",
            "message",
            "keyStorePassword",
            "accessKeyId",
            "rawSecretAccessKey",
        ] {
            assert!(
                WITHHELD_MEMBERS.contains(member),
                "case {member:?}: expected a @sensitive member to be withheld, got \
                 {WITHHELD_MEMBERS:?}"
            );
        }
    }

    /// A member the model does NOT mark sensitive keeps its value half, including the
    /// constrained members whose refusals this service's recordings show echoing a
    /// value.
    #[test]
    fn a_member_the_model_does_not_mark_sensitive_is_not_withheld() {
        for member in [
            "keyId",
            "aliasName",
            "targetKeyId",
            "policy",
            "signature",
            "mac",
            "ciphertextBlob",
            "importToken",
            "encryptedKeyMaterial",
            "grantTokens",
            "limit",
            "description",
        ] {
            assert!(
                !withholds_value(member),
                "case {member:?}: expected a member the model does not mark @sensitive to \
                 keep its value"
            );
        }
    }

    /// The decision is made over the dotted PATH, so a withheld member reached through a
    /// structure or a collection is withheld too.
    #[test]
    fn a_withheld_member_is_answered_for_at_whatever_depth_it_sits() {
        for path in [
            "plaintext",
            "message",
            "xksProxyAuthenticationCredential.accessKeyId",
            "xksProxyAuthenticationCredential.rawSecretAccessKey",
        ] {
            assert!(
                withholds_value(path),
                "case {path:?}: expected the path of a @sensitive member to be withheld"
            );
        }
    }
}
