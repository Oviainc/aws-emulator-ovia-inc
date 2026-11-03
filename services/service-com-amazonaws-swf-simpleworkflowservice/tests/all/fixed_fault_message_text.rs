// SPDX-License-Identifier: CC0-1.0

//! The two faults whose message is FIXED text, end to end over the service's own wire
//! protocol (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Op>`)
//! through the same `MultiServiceRouter` the emulator mounts in production.
//!
//! The rules every case here asserts:
//!
//! * `TooManyTagsFault` carries one fixed sentence — `Too many tags provided.` — with no
//!   part of the request and no part of the store in it, so the refusal reads the same
//!   whichever call tripped the documented maximum of 50 tags per resource and whichever
//!   of the two bounds (the batch the request carries, or the total it would leave the
//!   domain with) tripped first; and the refused call stores nothing.
//! * `DomainAlreadyExistsFault` IDENTIFIES the domain that already exists: its whole
//!   message is the requested name, with no text around it, from both operations that
//!   report it; and the stored record is left exactly as it was.
//!
//! Rows vary what the rule generalises over: how far past the maximum a request reaches,
//! which side of the bound the tags come from, which call reports the fault, the stored
//! domain's `RegistrationStatus`, and the shape and size of the domain name (plain,
//! punctuated, non-ASCII, at the modelled 256-character maximum).

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{
    error_code, register_domain_body, string_at, Emulator,
};

/// Tags one SWF domain may carry, from `TagResource`'s own doc: "Amazon SWF supports a
/// maximum of 50 tags per resource." Mirrors the crate's `MAX_TAGS_PER_DOMAIN`.
const MAX_TAGS_PER_DOMAIN: usize = 50;

/// `TooManyTagsFault`'s message, as Amazon SWF words it. Entirely fixed text: no count,
/// no resource name, nothing of the request.
const TOO_MANY_TAGS: &str = "Too many tags provided.";

/// `tags` carrying `count` distinct keys, numbered from `from` so two calls against one
/// domain can carry disjoint keys.
fn tags(from: usize, count: usize) -> Value {
    Value::Array(
        (from..from + count)
            .map(|index| json!({ "key": format!("k{index}"), "value": format!("v{index}") }))
            .collect(),
    )
}

/// Call `operation` and require HTTP 400 `expected_code` with exactly `expected_message`.
async fn refused_with(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: &Value,
    expected_code: &str,
    expected_message: &str,
) {
    let (status, parsed) = emulator.call(operation, body).await;
    assert_eq!(
        (
            status,
            error_code(&parsed).as_str(),
            parsed.get("message").and_then(Value::as_str),
        ),
        (400, expected_code, Some(expected_message)),
        "case {case:?}: expected {operation} to answer 400 {expected_code} naming \
         {expected_message:?}, got {status} with {parsed:?}"
    );
}

/// Register `domain` with `initial` tags and return the ARN the service minted for it,
/// read back through `DescribeDomain` the way a client learns a resource ARN.
async fn registered(emulator: &Emulator, case: &str, domain: &str, initial: Value) -> String {
    let mut body = register_domain_body(domain, "7");
    if initial.as_array().is_some_and(|tags| !tags.is_empty()) {
        body.as_object_mut()
            .expect("a JSON object")
            .insert("tags".to_string(), initial);
    }
    emulator.ok(case, "RegisterDomain", &body).await;
    let described = emulator
        .ok(case, "DescribeDomain", &json!({ "name": domain }))
        .await;
    string_at(&described, "/domainInfo/arn")
}

/// The tag keys `ListTagsForResource` reports for one resource.
async fn stored_keys(emulator: &Emulator, case: &str, resource_arn: &str) -> Vec<String> {
    let body = emulator
        .ok(
            case,
            "ListTagsForResource",
            &json!({ "resourceArn": resource_arn }),
        )
        .await;
    let mut keys: Vec<String> = body
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags array, got {body:?}"))
        .iter()
        .map(|tag| string_at(tag, "/key"))
        .collect();
    keys.sort();
    keys
}

/// One over-limit `TagResource` call: the tags the domain already carries, the tags the
/// call adds, and which of the two bounds the row is about.
struct TagRow {
    case: &'static str,
    domain: &'static str,
    stored: usize,
    added: usize,
}

#[tokio::test]
async fn every_too_many_tags_refusal_carries_the_same_fixed_message() {
    // Rows span both bounds and reach past the maximum by one and by far more: a single
    // batch above it whatever the domain holds, and a batch within it that would still
    // carry the domain past it.
    let rows = [
        TagRow {
            case: "one past the maximum in a single batch",
            domain: "tags-batch-51",
            stored: 0,
            added: MAX_TAGS_PER_DOMAIN + 1,
        },
        TagRow {
            case: "twice the maximum in a single batch",
            domain: "tags-batch-100",
            stored: 0,
            added: MAX_TAGS_PER_DOMAIN * 2,
        },
        TagRow {
            case: "an already-tagged domain carried one past the maximum",
            domain: "tags-merge-2-49",
            stored: 2,
            added: MAX_TAGS_PER_DOMAIN - 1,
        },
        TagRow {
            case: "a full domain and one more tag",
            domain: "tags-merge-50-1",
            stored: MAX_TAGS_PER_DOMAIN,
            added: 1,
        },
    ];

    let emulator = Emulator::new();
    for row in &rows {
        let case = row.case;
        let arn = registered(&emulator, case, row.domain, tags(0, row.stored)).await;
        let before = stored_keys(&emulator, case, &arn).await;
        assert_eq!(
            before.len(),
            row.stored,
            "case {case:?}: the domain starts with the tags it was registered with, got \
             {before:?}"
        );
        refused_with(
            &emulator,
            case,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": tags(100, row.added) }),
            "TooManyTagsFault",
            TOO_MANY_TAGS,
        )
        .await;
        let after = stored_keys(&emulator, case, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused tagging stores nothing, got {after:?}"
        );
    }

    // The same bound reported by the other call that carries tags, with the same message.
    let mut registration = register_domain_body("tags-at-registration", "7");
    registration
        .as_object_mut()
        .expect("a JSON object")
        .insert("tags".to_string(), tags(0, MAX_TAGS_PER_DOMAIN + 1));
    refused_with(
        &emulator,
        "a registration above the maximum",
        "RegisterDomain",
        &registration,
        "TooManyTagsFault",
        TOO_MANY_TAGS,
    )
    .await;
    emulator
        .refused(
            "a registration above the maximum",
            "DescribeDomain",
            &json!({ "name": "tags-at-registration" }),
            "UnknownResourceFault",
        )
        .await;

    // The bound itself is where the doc puts it: exactly the maximum is accepted, by
    // either path, and re-tagging a key the domain already carries does not add one.
    let arn = registered(&emulator, "at the maximum", "tags-at-maximum", tags(0, 1)).await;
    emulator
        .ok(
            "at the maximum",
            "TagResource",
            &json!({ "resourceArn": arn, "tags": tags(1, MAX_TAGS_PER_DOMAIN - 1) }),
        )
        .await;
    let full = stored_keys(&emulator, "at the maximum", &arn).await;
    assert_eq!(
        full.len(),
        MAX_TAGS_PER_DOMAIN,
        "case \"at the maximum\": the documented maximum is accepted, got {full:?}"
    );
    emulator
        .ok(
            "re-tagging a stored key",
            "TagResource",
            &json!({ "resourceArn": arn, "tags": [{ "key": "k0", "value": "changed" }] }),
        )
        .await;
    let unchanged = stored_keys(&emulator, "re-tagging a stored key", &arn).await;
    assert_eq!(
        unchanged, full,
        "case \"re-tagging a stored key\": a stored key takes the new value rather than \
         adding a tag, got {unchanged:?}"
    );
}

/// One domain that already exists, and the operation refused on it.
struct ConflictRow {
    case: &'static str,
    name: String,
    /// Whether the stored record is deprecated before the conflicting call.
    deprecate_first: bool,
    operation: &'static str,
}

#[tokio::test]
async fn a_domain_already_exists_fault_names_the_domain_and_nothing_else() {
    let rows = [
        ConflictRow {
            case: "undeprecate a registered domain",
            name: "conflict-plain".to_string(),
            deprecate_first: false,
            operation: "UndeprecateDomain",
        },
        ConflictRow {
            case: "undeprecate a registered domain with a punctuated name",
            name: "home.domain_1-a".to_string(),
            deprecate_first: false,
            operation: "UndeprecateDomain",
        },
        ConflictRow {
            case: "undeprecate a registered domain with a non-ASCII name",
            name: "注文ドメイン".to_string(),
            deprecate_first: false,
            operation: "UndeprecateDomain",
        },
        ConflictRow {
            case: "register over a registered domain",
            name: "conflict-register".to_string(),
            deprecate_first: false,
            operation: "RegisterDomain",
        },
        ConflictRow {
            case: "register over a DEPRECATED domain",
            name: "conflict-deprecated".to_string(),
            deprecate_first: true,
            operation: "RegisterDomain",
        },
        ConflictRow {
            case: "a name at the modelled maximum length",
            name: "d".repeat(256),
            deprecate_first: false,
            operation: "UndeprecateDomain",
        },
    ];

    let emulator = Emulator::new();
    for row in &rows {
        let case = row.case;
        emulator
            .ok(case, "RegisterDomain", &register_domain_body(&row.name, "7"))
            .await;
        if row.deprecate_first {
            emulator
                .ok(case, "DeprecateDomain", &json!({ "name": row.name }))
                .await;
        }
        let expected_status = if row.deprecate_first {
            "DEPRECATED"
        } else {
            "REGISTERED"
        };

        let body = match row.operation {
            "RegisterDomain" => register_domain_body(&row.name, "7"),
            _ => json!({ "name": row.name }),
        };
        refused_with(
            &emulator,
            case,
            row.operation,
            &body,
            "DomainAlreadyExistsFault",
            &row.name,
        )
        .await;

        let described = emulator
            .ok(case, "DescribeDomain", &json!({ "name": row.name }))
            .await;
        assert_eq!(
            string_at(&described, "/domainInfo/status"),
            expected_status,
            "case {case:?}: the refused call leaves the stored record as it was, got \
             {described:?}"
        );
    }
}
