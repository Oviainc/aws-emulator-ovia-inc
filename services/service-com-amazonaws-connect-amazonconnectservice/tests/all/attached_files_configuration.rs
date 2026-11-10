// SPDX-License-Identifier: CC0-1.0

//! The attached-files configuration of an instance's attachment scopes —
//! `UpdateAttachedFilesConfiguration` and the `DescribeAttachedFilesConfiguration`
//! read that proves it — end to end over HTTP through the router the emulator serves
//! with.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * an update STORES the members it names for the (instance, attachment scope) it
//!   names, and the describe reads that same configuration back, carrying the instant
//!   the update happened;
//! * the update is PARTIAL: a member the request leaves out keeps the value a caller
//!   would have read before the call, so an update that names one member changes only
//!   that one;
//! * a scope no update has configured reports that scope's DEFAULTS, and reading them
//!   configures nothing — a later partial update still starts from the defaults;
//! * one scope's configuration is its own: configuring one leaves the other scopes of
//!   the same instance at their defaults;
//! * an `InstanceId` that names no instance is refused with the modelled
//!   `ResourceNotFoundException` (HTTP 404) — never a 501;
//! * a value outside a modelled bound (`MaximumSizeLimitInBytes` 1..104857600, a
//!   `FileExtension` of 1..10 characters matching `^[a-zA-Z0-9-_]+$`) is refused with
//!   HTTP 400 and stores nothing.

use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::ops::types::ATTACHMENT_SCOPE_VALUES;
use service_com_amazonaws_connect_amazonconnectservice::state::DEFAULT_MAXIMUM_SIZE_LIMIT_IN_BYTES;

use super::harness::{connect_managed, Answer, Connect};

/// One row of the table: the attachment scope to configure, the `MaximumSizeLimitInBytes`
/// this row sends (`None` sends the member not at all), and the `AllowedExtensions` it
/// sends (`None` sends no `ExtensionConfiguration` at all).
///
/// The scope names come from `com.amazonaws.connect#AttachmentScope`, which
/// [`every_case_names_a_modelled_attachment_scope`] checks this table against; the
/// extensions are values the modelled `FileExtension` (length 1..10, pattern
/// `^[a-zA-Z0-9-_]+$`) accepts.
#[derive(Debug, Clone, Copy)]
struct Case {
    scope: &'static str,
    maximum: Option<i64>,
    extensions: Option<&'static [&'static str]>,
}

/// The table every case below is driven over: each modelled attachment scope once, and
/// between them every combination of the two optional members, both ends of the
/// `MaximumSizeLimitInBytes` range, and extension sets of one and of several entries
/// spanning the `FileExtension` length bounds and the characters its pattern allows.
const CASES: &[Case] = &[
    Case {
        scope: "CHAT",
        maximum: Some(20_971_520),
        extensions: Some(&["pdf", "jpeg"]),
    },
    Case {
        scope: "EMAIL",
        // The range's lower bound, with no extension configuration at all.
        maximum: Some(1),
        extensions: None,
    },
    Case {
        scope: "CASE",
        // No size limit, so the stored one stays the default.
        maximum: None,
        extensions: Some(&["csv"]),
    },
    Case {
        scope: "TASK",
        // The range's upper bound, and the extension length bounds beside it.
        maximum: Some(104_857_600),
        extensions: Some(&["a", "0123456789", "x-y_z"]),
    },
];

/// `UpdateAttachedFilesConfiguration`:
/// `POST /attached-files-configurations/{InstanceId}/{AttachmentScope}`.
async fn update(connect: &Connect, instance_id: &str, scope: &str, body: Value) -> Answer {
    connect
        .send_answer(
            "POST",
            &format!("/attached-files-configurations/{instance_id}/{scope}"),
            Some(body),
        )
        .await
}

/// `DescribeAttachedFilesConfiguration`:
/// `GET /attached-files-configurations/{InstanceId}/{AttachmentScope}`.
async fn describe(connect: &Connect, instance_id: &str, scope: &str) -> Answer {
    connect
        .send_answer(
            "GET",
            &format!("/attached-files-configurations/{instance_id}/{scope}"),
            None,
        )
        .await
}

/// The modelled `ExtensionConfiguration` carrying `extensions`.
fn extension_configuration(extensions: &[&str]) -> Value {
    json!({
        "AllowedExtensions": extensions
            .iter()
            .map(|extension| json!({ "Extension": extension }))
            .collect::<Vec<Value>>(),
    })
}

/// The request body one row updates with: only the members the row names.
fn request(maximum: Option<i64>, extensions: Option<&[&str]>) -> Value {
    let mut body = json!({});
    if let Some(maximum) = maximum {
        body["MaximumSizeLimitInBytes"] = json!(maximum);
    }
    if let Some(extensions) = extensions {
        body["ExtensionConfiguration"] = extension_configuration(extensions);
    }
    body
}

/// The configuration a scope holds: the size limit and the extension set it should
/// report, as a comparable JSON object. `LastModifiedTime` is asserted separately, so
/// it is not part of this.
fn configuration(
    instance_id: &str,
    scope: &str,
    maximum: i64,
    extensions: Option<&[&str]>,
) -> Value {
    let mut expected = json!({
        "InstanceId": instance_id,
        "AttachmentScope": scope,
        "MaximumSizeLimitInBytes": maximum,
    });
    if let Some(extensions) = extensions {
        expected["ExtensionConfiguration"] = extension_configuration(extensions);
    }
    expected
}

/// An answered configuration without its `LastModifiedTime`, for comparison against
/// [`configuration`].
fn without_last_modified(answered: &Value) -> Value {
    let mut value = answered.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("LastModifiedTime");
    }
    value
}

/// The `LastModifiedTime` of an answered configuration, as the epoch seconds the JSON
/// protocol renders a timestamp as.
fn last_modified(answered: &Value) -> Option<f64> {
    answered["LastModifiedTime"].as_f64()
}

/// The table is written by hand, so it is pinned to the model's own value list: a row
/// naming anything else is a stale table, not a failing emulator.
#[test]
fn every_case_names_a_modelled_attachment_scope() {
    for case in CASES {
        assert!(
            ATTACHMENT_SCOPE_VALUES.contains(&case.scope),
            "case {case:?}: AttachmentScope does not name {}",
            case.scope
        );
    }
    for scope in ATTACHMENT_SCOPE_VALUES {
        assert!(
            CASES.iter().any(|case| &case.scope == scope),
            "the table leaves the attachment scope {scope} unexercised"
        );
    }
}

/// The update stores what it names, and the describe reads it back: every row
/// round-trips, with the members it left out holding their defaults.
#[tokio::test]
async fn an_update_is_read_back_by_describe_for_every_attachment_scope() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("attached-files-round-trip"))
        .await;

    for case in CASES {
        let answer = update(
            &connect,
            &instance_id,
            case.scope,
            request(case.maximum, case.extensions),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status, answer.body
        );

        let expected = configuration(
            &instance_id,
            case.scope,
            case.maximum.unwrap_or(DEFAULT_MAXIMUM_SIZE_LIMIT_IN_BYTES),
            case.extensions,
        );
        assert_eq!(
            without_last_modified(&answer.body),
            expected,
            "case {case:?}: expected the update to answer {expected}, got {}",
            answer.body
        );
        let modified = last_modified(&answer.body).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an update to report a LastModifiedTime, got {}",
                answer.body
            )
        });
        assert!(
            modified > 0.0,
            "case {case:?}: expected a LastModifiedTime after the epoch, got {modified}"
        );

        let read = describe(&connect, &instance_id, case.scope).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the configuration to be readable with HTTP 200, got {} {}",
            read.status, read.body
        );
        let answered = &read.body["AttachedFilesConfiguration"];
        assert_eq!(
            without_last_modified(answered),
            expected,
            "case {case:?}: expected the stored configuration {expected}, got {answered}"
        );
        assert_eq!(
            last_modified(answered),
            Some(modified),
            "case {case:?}: expected the read to report the update's LastModifiedTime \
             {modified}, got {answered}"
        );
    }
}

/// The update is partial: a member the request leaves out keeps the value it had, so an
/// update naming one member changes that one only.
#[tokio::test]
async fn an_update_leaves_the_members_it_does_not_name_alone() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("attached-files-partial"))
        .await;

    for case in CASES {
        // A configuration carrying BOTH members, whatever the row itself sends, so the
        // partial updates below each have a value to retain.
        let both = ["txt", "docx"];
        let first = update(
            &connect,
            &instance_id,
            case.scope,
            request(Some(1024), Some(&both)),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            first.status, first.body
        );

        // Only the size limit: the extension set is the one already stored.
        let limit_only = update(&connect, &instance_id, case.scope, request(Some(2048), None)).await;
        let expected = configuration(&instance_id, case.scope, 2048, Some(&both));
        assert_eq!(
            without_last_modified(&limit_only.body),
            expected,
            "case {case:?}: expected a size-only update to answer {expected}, got {}",
            limit_only.body
        );

        // Only the extensions: the size limit is the one already stored.
        let extensions = ["png"];
        let extensions_only =
            update(&connect, &instance_id, case.scope, request(None, Some(&extensions))).await;
        let expected = configuration(&instance_id, case.scope, 2048, Some(&extensions));
        assert_eq!(
            without_last_modified(&extensions_only.body),
            expected,
            "case {case:?}: expected an extensions-only update to answer {expected}, got {}",
            extensions_only.body
        );

        // An update that names NOTHING changes nothing but the modification instant.
        let empty = update(&connect, &instance_id, case.scope, json!({})).await;
        assert_eq!(
            without_last_modified(&empty.body),
            expected,
            "case {case:?}: expected an empty update to answer {expected}, got {}",
            empty.body
        );

        let read = describe(&connect, &instance_id, case.scope).await;
        let answered = &read.body["AttachedFilesConfiguration"];
        assert_eq!(
            without_last_modified(answered),
            expected,
            "case {case:?}: expected the stored configuration {expected}, got {answered}"
        );
        // Every update is a modification, so the instant never goes backwards.
        let (earliest, latest) = (
            last_modified(&first.body).expect("the first update reports a LastModifiedTime"),
            last_modified(answered).expect("the stored configuration carries a LastModifiedTime"),
        );
        assert!(
            latest >= earliest,
            "case {case:?}: expected the later update's LastModifiedTime {latest} not to \
             precede the first's {earliest}"
        );
    }
}

/// A scope nobody has configured reports that scope's defaults, and reading them
/// configures nothing: the read is repeatable, and a later partial update still starts
/// from the defaults.
#[tokio::test]
async fn an_unconfigured_scope_reports_the_defaults_and_reading_them_stores_nothing() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("attached-files-defaults"))
        .await;

    for scope in ATTACHMENT_SCOPE_VALUES {
        let defaults = configuration(
            &instance_id,
            scope,
            DEFAULT_MAXIMUM_SIZE_LIMIT_IN_BYTES,
            None,
        );
        for read_number in ["first", "second"] {
            let read = describe(&connect, &instance_id, scope).await;
            assert_eq!(
                read.status, 200,
                "case {scope}/{read_number} read: expected HTTP 200, got {} {}",
                read.status, read.body
            );
            let answered = &read.body["AttachedFilesConfiguration"];
            assert_eq!(
                answered, &defaults,
                "case {scope}/{read_number} read: expected the defaults {defaults}, got \
                 {answered}"
            );
            assert_eq!(
                last_modified(answered), None,
                "case {scope}/{read_number} read: an unconfigured scope has never been \
                 modified, got {answered}"
            );
        }
    }

    // One scope is configured, with one member only: it keeps the default for the other,
    // which is what proves the update started from the defaults the reads above saw.
    let configured = "CHAT";
    let answer = update(
        &connect,
        &instance_id,
        configured,
        request(None, Some(&["pdf"])),
    )
    .await;
    let expected = configuration(
        &instance_id,
        configured,
        DEFAULT_MAXIMUM_SIZE_LIMIT_IN_BYTES,
        Some(&["pdf"]),
    );
    assert_eq!(
        without_last_modified(&answer.body),
        expected,
        "case {configured}: expected an extensions-only first update to answer {expected}, \
         got {}",
        answer.body
    );

    // And one scope's configuration is its own: the others are still at their defaults.
    for scope in ATTACHMENT_SCOPE_VALUES.iter().filter(|s| **s != configured) {
        let read = describe(&connect, &instance_id, scope).await;
        let defaults = configuration(
            &instance_id,
            scope,
            DEFAULT_MAXIMUM_SIZE_LIMIT_IN_BYTES,
            None,
        );
        assert_eq!(
            read.body["AttachedFilesConfiguration"], defaults,
            "case {scope}: expected configuring {configured} to leave this scope at its \
             defaults {defaults}, got {}",
            read.body["AttachedFilesConfiguration"]
        );
    }
}

/// Nothing is configured, and nothing is read, on an instance that does not exist: both
/// operations answer the modelled not-found error for every attachment scope.
#[tokio::test]
async fn an_unknown_instance_configures_and_reports_nothing() {
    let connect = Connect::new();
    // A live instance stands beside the absent one, so a row cannot pass merely because
    // the store is empty.
    let (live_id, _) = connect
        .create_instance(connect_managed("attached-files-neighbour"))
        .await;
    let unknown = "11111111-2222-3333-4444-555555555555";
    assert_ne!(unknown, live_id, "the row has to name an absent instance");
    let expected_message = format!("Instance {unknown} not found");

    for scope in ATTACHMENT_SCOPE_VALUES {
        let answers = [
            (
                "UpdateAttachedFilesConfiguration",
                update(&connect, unknown, scope, request(Some(1024), None)).await,
            ),
            (
                "DescribeAttachedFilesConfiguration",
                describe(&connect, unknown, scope).await,
            ),
        ];
        for (operation, answer) in answers {
            assert_eq!(
                answer.status, 404,
                "case {scope}/{operation}: expected HTTP 404, got {} {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {scope}/{operation}: expected ResourceNotFoundException, got {:?} {}",
                answer.error_code, answer.body
            );
            let message = answer.body["Message"].as_str().unwrap_or_default();
            assert_eq!(
                message, expected_message,
                "case {scope}/{operation}: expected message {expected_message:?}, got \
                 {message:?}"
            );
        }
    }
}

/// A value outside a modelled bound is refused, and the refusal stores nothing: both
/// ends of `MaximumSizeLimitInBytes` are accepted (the table above sends them) while one
/// step past either end is refused, as is a `FileExtension` longer than the bound or
/// carrying a character its pattern excludes.
#[tokio::test]
async fn a_value_outside_a_modelled_bound_is_refused_and_stores_nothing() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("attached-files-bounds"))
        .await;
    let scope = "CHAT";

    // The configuration the refusals must leave alone.
    let stored = ["txt"];
    let first = update(
        &connect,
        &instance_id,
        scope,
        request(Some(4096), Some(&stored)),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "expected the configuration to be stored first, got {} {}",
        first.status, first.body
    );

    // One step past each published bound, and the two forms a bad extension takes.
    let refused: Vec<(&str, Value)> = vec![
        ("a size limit of 0, one below the range", request(Some(0), None)),
        (
            "a size limit of 104857601, one above the range",
            request(Some(104_857_601), None),
        ),
        (
            "an extension of 11 characters, one above the length bound",
            request(None, Some(&["abcdefghijk"])),
        ),
        (
            "an extension carrying a character the pattern excludes",
            request(None, Some(&[".pdf"])),
        ),
        (
            "an extension of 0 characters, one below the length bound",
            request(None, Some(&[""])),
        ),
    ];

    for (case, body) in refused {
        let answer = update(&connect, &instance_id, scope, body.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({body}): expected HTTP 400, got {} {}",
            answer.status, answer.body
        );

        let read = describe(&connect, &instance_id, scope).await;
        let expected = configuration(&instance_id, scope, 4096, Some(&stored));
        assert_eq!(
            without_last_modified(&read.body["AttachedFilesConfiguration"]),
            expected,
            "case {case:?}: expected the refusal to leave {expected} stored, got {}",
            read.body["AttachedFilesConfiguration"]
        );
    }
}
