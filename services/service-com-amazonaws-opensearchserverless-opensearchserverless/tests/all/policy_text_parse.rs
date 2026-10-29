// SPDX-License-Identifier: CC0-1.0

//! Every operation that WRITES a policy document reads its `policy` text one way.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body, the wire
//! contract in this crate's `OPERATIONS.md`), through the harness this crate's
//! `collection_lifecycle` suite already owns, so the boundary validator, the protocol codec
//! and the handler all take part exactly as they do for an SDK client.
//!
//! The rule under test: a request's `policy` text is read as a JSON document — text that
//! does not parse is refused with a `ValidationException` naming the parse error, a
//! document nesting past the depth this emulator converts is refused with a
//! `ValidationException` naming that bound, and anything else is stored PARSED and is what
//! the following read reports. One path serves it, so no two policy-writing operations can
//! answer a given text differently.
//!
//! The table spans what the rule generalises over: the ways a text fails to be JSON (not
//! JSON at all, blank, truncated, trailing characters, a non-JSON literal, a quoting the
//! grammar does not allow, nesting past the parser's own recursion limit), the depth bound
//! at its value, one past it and far past it, and the accepted forms (an object, a text
//! whose whitespace the stored document drops, a list, a bare scalar). Each row is driven
//! through every policy-writing operation this service has — `CreateSecurityPolicy` and
//! `UpdateSecurityPolicy` for both `SecurityPolicyType` values, `CreateAccessPolicy` and
//! `UpdateAccessPolicy`, `CreateLifecyclePolicy` and `UpdateLifecyclePolicy` — and each
//! accepted row is read back through that family's reader.

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_DOCUMENT_DEPTH;
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// How a refused policy text is refused: the two refusals the one path owes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    /// The text is not a JSON document, so the refusal names the parse error.
    Parse,
    /// The text parses, but the document nests past [`MAX_DOCUMENT_DEPTH`].
    Depth,
}

/// What every refusal of a text that is not JSON opens with.
const PARSE_PREFIX: &str = "policy must be a JSON document: ";

/// A JSON array nested `depth` levels deep: `[[[ … ]]]`.
fn nested_array(depth: usize) -> String {
    format!("{}{}", "[".repeat(depth), "]".repeat(depth))
}

/// A `retention` lifecycle document whose rules are valid and which nests `total` levels
/// deep: the rules sit well inside the bound, and a member beside them carries the nesting.
///
/// The lifecycle operations check the document's CONTENT against the retention syntax as
/// well, so a nesting case for them has to be a document whose rules that check accepts —
/// otherwise it would be answering the content refusal rather than the one under test.
fn retention_nested(total: usize) -> String {
    format!(
        r#"{{"Rules":[{{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"24h"}}],"Deep":{}}}"#,
        nested_array(total - 1)
    )
}

/// The shallow `retention` document the lifecycle family's baseline policy carries.
const RETENTION_POLICY: &str =
    r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"30d"}]}"#;

/// One policy text and what every policy-writing operation owes for it.
struct TextCase {
    /// The spelling under test, named in every assertion of the row.
    case: &'static str,
    /// The `policy` text an operation whose document content is unconstrained carries.
    policy: String,
    /// The `policy` text the lifecycle family carries: the same text, except where the row
    /// has to be a valid `retention` document for that family's own content check to let
    /// the text under test reach the one path.
    lifecycle_policy: String,
    /// `None` when every operation accepts the text.
    refusal: Option<Refusal>,
}

/// A row whose text is the same for every family.
fn shared(case: &'static str, policy: &str, refusal: Option<Refusal>) -> TextCase {
    TextCase {
        case,
        policy: policy.to_string(),
        lifecycle_policy: policy.to_string(),
        refusal,
    }
}

/// The table. The nesting rows name [`MAX_DOCUMENT_DEPTH`] rather than a literal, so the
/// bound is read from the one place that defines it.
fn rows() -> Vec<TextCase> {
    vec![
        // --- texts that are not JSON documents at all ---------------------------------
        shared("not JSON at all", "not json at all", Some(Refusal::Parse)),
        // `PolicyDocument` is @length(min: 1), so the shortest blank text is one space.
        shared("blank", " ", Some(Refusal::Parse)),
        shared("a truncated object", r#"{"Rules":["#, Some(Refusal::Parse)),
        shared(
            "an object with trailing characters",
            r#"{"Rules":[]} and then some"#,
            Some(Refusal::Parse),
        ),
        shared(
            "single-quoted member names",
            "{'Rules':[]}",
            Some(Refusal::Parse),
        ),
        shared("a non-JSON literal", "NaN", Some(Refusal::Parse)),
        shared("an unquoted word", "Rules", Some(Refusal::Parse)),
        // Past the JSON parser's OWN recursion limit the text never becomes a value, so
        // the parse refusal is what it owes — before any depth of ours is measured.
        TextCase {
            case: "nesting past the JSON parser's own recursion limit",
            policy: nested_array(5_000),
            lifecycle_policy: nested_array(5_000),
            refusal: Some(Refusal::Parse),
        },
        // --- documents that parse but nest past the bound -----------------------------
        TextCase {
            case: "nesting one level past the bound",
            policy: nested_array(MAX_DOCUMENT_DEPTH + 1),
            lifecycle_policy: retention_nested(MAX_DOCUMENT_DEPTH + 1),
            refusal: Some(Refusal::Depth),
        },
        TextCase {
            case: "nesting far past the bound, still inside the parser's limit",
            policy: nested_array(120),
            lifecycle_policy: retention_nested(120),
            refusal: Some(Refusal::Depth),
        },
        // --- documents every operation accepts ----------------------------------------
        TextCase {
            case: "an object",
            policy: r#"{"Rules":[]}"#.to_string(),
            lifecycle_policy: RETENTION_POLICY.to_string(),
            refusal: None,
        },
        TextCase {
            // Stored PARSED, so the read reports the document and not the caller's own
            // formatting of it.
            case: "an object written with whitespace",
            policy: "{ \"Rules\" : [ ]\n}".to_string(),
            lifecycle_policy: format!("{{ \"Rules\" :\n{}\n}}", r#"[{"ResourceType":"index","Resource":[ "index/demo/*" ],"MinIndexRetention":"24h"}]"#),
            refusal: None,
        },
        TextCase {
            case: "a list of statements",
            policy: r#"[{"Rules":[]},{"Rules":[]}]"#.to_string(),
            lifecycle_policy: RETENTION_POLICY.to_string(),
            refusal: None,
        },
        TextCase {
            case: "a bare scalar",
            policy: "42".to_string(),
            lifecycle_policy: RETENTION_POLICY.to_string(),
            refusal: None,
        },
        TextCase {
            case: "nesting exactly on the bound",
            policy: nested_array(MAX_DOCUMENT_DEPTH),
            lifecycle_policy: retention_nested(MAX_DOCUMENT_DEPTH),
            refusal: None,
        },
    ]
}

/// One family of policy-writing operations: its create, its update, the reader that proves
/// what they stored, and the `type` its requests carry.
struct Family {
    create: &'static str,
    update: &'static str,
    /// The reader a stored policy of this family is read back through.
    read: &'static str,
    /// The member a create, an update and a read all carry the policy under.
    detail: &'static str,
    /// The policy `type` the requests carry.
    kind: &'static str,
    /// Whether this family's documents also have to satisfy a content check, and so take
    /// the row's `lifecycle_policy` text.
    content_checked: bool,
    /// The `PolicyName` prefix this family's rows create under (`^[a-z][a-z0-9-]+$`).
    prefix: &'static str,
}

/// The security-policy family under one `SecurityPolicyType` value, so the rows are driven
/// over that whole value list (`smithy_query(query="shape",
/// shape="SecurityPolicyType")` -> `values`: `encryption`, `network`).
fn security_policies(kind: &'static str, prefix: &'static str) -> Family {
    Family {
        create: "CreateSecurityPolicy",
        update: "UpdateSecurityPolicy",
        read: "GetSecurityPolicy",
        detail: "securityPolicyDetail",
        kind,
        content_checked: false,
        prefix,
    }
}

/// The access-policy family, over `AccessPolicyType`'s single value `data`.
fn access_policies() -> Family {
    Family {
        create: "CreateAccessPolicy",
        update: "UpdateAccessPolicy",
        read: "GetAccessPolicy",
        detail: "accessPolicyDetail",
        kind: "data",
        content_checked: false,
        prefix: "acc",
    }
}

/// The lifecycle-policy family, over `LifecyclePolicyType`'s single value `retention`.
fn lifecycle_policies() -> Family {
    Family {
        create: "CreateLifecyclePolicy",
        update: "UpdateLifecyclePolicy",
        read: "BatchGetLifecyclePolicy",
        detail: "lifecyclePolicyDetail",
        kind: "retention",
        content_checked: true,
        prefix: "life",
    }
}

impl Family {
    /// The text this family carries for `row`.
    fn text_for(&self, row: &TextCase) -> String {
        if self.content_checked {
            row.lifecycle_policy.clone()
        } else {
            row.policy.clone()
        }
    }

    /// A document this family always accepts, for the policy a row updates.
    fn baseline(&self) -> String {
        if self.content_checked {
            RETENTION_POLICY.to_string()
        } else {
            r#"{"Rules":[]}"#.to_string()
        }
    }

    /// The stored policy `name` reads as, through this family's own reader.
    async fn read_back(&self, router: &MultiServiceRouter, name: &str) -> Value {
        if self.read == "BatchGetLifecyclePolicy" {
            let (status, code, read) = call(
                router,
                self.read,
                json!({ "identifiers": [{ "type": self.kind, "name": name }] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {}({}/{name}): expected 200, got {status} {code} with {read}",
                self.read, self.kind
            );
            return at(&read, "lifecyclePolicyDetails")
                .as_array()
                .and_then(|details| details.first())
                .cloned()
                .unwrap_or(Value::Null);
        }
        let (status, code, read) = call(
            router,
            self.read,
            json!({ "type": self.kind, "name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}({}/{name}): expected 200, got {status} {code} with {read}",
            self.read, self.kind
        );
        at(&read, self.detail).clone()
    }
}

/// Assert `answer` is the refusal `refusal` owes for `case`.
fn assert_refused(case: &str, refusal: Refusal, status: u16, code: &str, answer: &Value) {
    assert_eq!(
        (status, code),
        (CLIENT_ERROR, "ValidationException"),
        "case {case}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
    );
    let message = text(answer, "message");
    match refusal {
        Refusal::Parse => {
            assert!(
                message.starts_with(PARSE_PREFIX),
                "case {case}: expected a message opening {PARSE_PREFIX:?}, got {message:?}"
            );
            assert!(
                message.len() > PARSE_PREFIX.len(),
                "case {case}: expected the parse error to be named after the prefix, got {message:?}"
            );
        }
        Refusal::Depth => {
            let expected = format!("policy must not nest deeper than {MAX_DOCUMENT_DEPTH} levels");
            assert_eq!(
                message, expected,
                "case {case}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// Drive every row of the table through `family`'s create, its update and its reader.
///
/// One test per family below, so a family's rows run beside the other families' and a
/// failing row does not hide theirs.
async fn every_row_through(family: Family) {
    for (index, row) in rows().into_iter().enumerate() {
        // One emulator per row, so the policy a row reads back is the one it wrote and no
        // row spends another's share of the per-type policy quota.
        let router = emulator();
        let text_under_test = family.text_for(&row);
        let created_name = format!("{}-new-{index}", family.prefix);
        let updated_name = format!("{}-old-{index}", family.prefix);

        // The policy the row's update writes over, created with a document this family
        // always accepts, so the update is refused (or accepted) by the text under test
        // alone.
        let (status, code, answer) = call(
            &router,
            family.create,
            json!({
                "type": family.kind,
                "name": updated_name,
                "policy": family.baseline(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}({}/{updated_name}, the baseline): expected 200, got {status} {code} with {answer}",
            family.create, family.kind
        );
        let baseline = at(&answer, family.detail).clone();
        let baseline_version = text(&baseline, "policyVersion");

        // The create path.
        let case = format!(
            "{}({}/{created_name}, {})",
            family.create, family.kind, row.case
        );
        let (status, code, answer) = call(
            &router,
            family.create,
            json!({
                "type": family.kind,
                "name": created_name,
                "policy": text_under_test,
            }),
        )
        .await;
        match row.refusal {
            Some(refusal) => {
                assert_refused(&case, refusal, status, &code, &answer);
                // A refused create stores nothing, so the reader does not know the name.
                let (status, code, answer) = call(
                    &router,
                    family.read,
                    if family.read == "BatchGetLifecyclePolicy" {
                        json!({ "identifiers": [{ "type": family.kind, "name": created_name }] })
                    } else {
                        json!({ "type": family.kind, "name": created_name })
                    },
                )
                .await;
                let reported = if family.read == "BatchGetLifecyclePolicy" {
                    at(&answer, "lifecyclePolicyDetails")
                        .as_array()
                        .map(Vec::len)
                        .unwrap_or_default()
                        == 0
                } else {
                    (status, code.as_str()) == (CLIENT_ERROR, "ResourceNotFoundException")
                };
                assert!(
                    reported,
                    "case {case}: a refused create must store nothing, but {} reported {status} {code} with {answer}",
                    family.read
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: expected 200, got {status} {code} with {answer}"
                );
                let detail = at(&answer, family.detail).clone();
                let expected: Value =
                    serde_json::from_str(&text_under_test).expect("an accepted row's text is JSON");
                assert_eq!(
                    at(&detail, "policy"),
                    &expected,
                    "case {case}: the stored document is the parsed text, got {detail}"
                );
                assert_eq!(
                    family.read_back(&router, &created_name).await,
                    detail,
                    "case {case}: the read must answer the created record"
                );
            }
        }

        // The update path, over the baseline policy, for the same text.
        let case = format!(
            "{}({}/{updated_name}, {})",
            family.update, family.kind, row.case
        );
        let (status, code, answer) = call(
            &router,
            family.update,
            json!({
                "type": family.kind,
                "name": updated_name,
                "policyVersion": baseline_version,
                "policy": text_under_test,
            }),
        )
        .await;
        match row.refusal {
            Some(refusal) => {
                assert_refused(&case, refusal, status, &code, &answer);
                // A refused update writes nothing: the stored record is still the
                // baseline, version and all.
                assert_eq!(
                    family.read_back(&router, &updated_name).await,
                    baseline,
                    "case {case}: a refused update must leave the stored policy as it was"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: expected 200, got {status} {code} with {answer}"
                );
                let detail = at(&answer, family.detail).clone();
                let expected: Value =
                    serde_json::from_str(&text_under_test).expect("an accepted row's text is JSON");
                assert_eq!(
                    at(&detail, "policy"),
                    &expected,
                    "case {case}: the stored document is the parsed text, got {detail}"
                );
                assert_eq!(
                    family.read_back(&router, &updated_name).await,
                    detail,
                    "case {case}: the read must answer the written record"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_encryption_security_policy_write_reads_its_policy_text_the_one_way() {
    every_row_through(security_policies("encryption", "enc")).await;
}

#[tokio::test]
async fn a_network_security_policy_write_reads_its_policy_text_the_one_way() {
    every_row_through(security_policies("network", "net")).await;
}

#[tokio::test]
async fn an_access_policy_write_reads_its_policy_text_the_one_way() {
    every_row_through(access_policies()).await;
}

#[tokio::test]
async fn a_lifecycle_policy_write_reads_its_policy_text_the_one_way() {
    every_row_through(lifecycle_policies()).await;
}
