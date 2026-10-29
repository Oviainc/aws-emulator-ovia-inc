// SPDX-License-Identifier: CC0-1.0

//! Every caller-supplied string and list INSIDE an origin endpoint's authorization and
//! packaging configuration is size-bounded before it is stored.
//!
//! RULE under test: a member of a `CreateOriginEndpoint` or `UpdateOriginEndpoint` request
//! whose value the store keeps verbatim is bounded before anything is written — a string at
//! `state::MAX_STRING_CHARS` characters, a list at its own maximum entry count with each
//! entry bounded as a string — so no one request decides how much memory its record
//! occupies. A request past either bound is refused with `UnprocessableEntityException`
//! (HTTP 422) keyed by the member path the wire names, persisting nothing: the create
//! stores no endpoint and the update leaves the stored one exactly as it was. A value AT
//! the bound is stored and read back unchanged.
//!
//! The members are the complete set of caller-supplied strings and lists an endpoint's
//! `Authorization` and packaging blocks carry that target the unconstrained `__string`
//! shape (or a list), from
//! `smithy_query(query="shape",shape="com.amazonaws.mediapackage#Authorization,com.amazonaws.mediapackage#SpekeKeyProvider,com.amazonaws.mediapackage#HlsPackage,com.amazonaws.mediapackage#HlsEncryption,com.amazonaws.mediapackage#DashPackage,com.amazonaws.mediapackage#CmafPackageCreateOrUpdateParameters,com.amazonaws.mediapackage#CmafEncryption,com.amazonaws.mediapackage#HlsManifestCreateOrUpdateParameters",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`.
//! A member targeting an `enum` shape is not a row: its value set is closed, so only the
//! ENTRY COUNT of a list of such values is the caller's to decide, and those lists are rows.
//! The issue cited four of these members; the rows below span the rest, so the rule is
//! tested where it holds rather than where it was reported.
//!
//! The rows differ along what the rule generalises over: which packaging format carries the
//! member (HLS, DASH, CMAF, MSS), how deeply it is nested (a block's own member, its
//! encryption, the SPEKE key provider under it, a CMAF child manifest), whether the value
//! is a free string, an ARN the service also shape-checks, or a list, which of the two ways
//! a list grows (entry count, entry length), the size sent (at the bound, one past it, and
//! far past it) and which operation carries it (create or update).
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — so what is asserted is the wire behaviour, not a handler call.
//! The client, the region and the helpers are the ones `tests/all/channel_lifecycle.rs`
//! defines.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::ops::types::shared_types as types;
use service_com_amazonaws_mediapackage_mediapackage::state;

use crate::channel_lifecycle::{call, emulator, error_code};

/// The channel every endpoint in this suite packages.
const CHANNEL: &str = "packaging-bounds";

/// The four packaging configurations an origin endpoint can be created with, as the wire
/// names them — the `HlsPackage` / `DashPackage` / `CmafPackage` / `MssPackage` members of
/// the create request.
const BLOCKS: &[&str] = &["hlsPackage", "dashPackage", "cmafPackage", "mssPackage"];

/// An IAM role ARN this service admits for a role member (`state::IAM_ROLE_ARN`), as its
/// fixed leading fields; a row pads the role NAME out to the length it needs.
const IAM_ROLE_PREFIX: &str = "arn:aws:iam::000000000000:role/";

/// A Certificate Manager certificate ARN, as its fixed leading fields
/// (`state::ACM_CERTIFICATE_ARN`).
const ACM_CERTIFICATE_PREFIX: &str = "arn:aws:acm:eu-west-1:000000000000:certificate/";

/// A Secrets Manager secret ARN, as its fixed leading fields
/// (`state::SECRETS_MANAGER_SECRET_ARN`).
const SECRET_PREFIX: &str = "arn:aws:secretsmanager:eu-west-1:000000000000:secret:";

/// A key-provider URL, as the fixed part a padded row keeps.
const SPEKE_URL_PREFIX: &str = "https://speke.example.com/";

/// One DRM system id, the form a `SystemIds` entry carries.
const SYSTEM_ID: &str = "81376844-f976-481e-a84e-cc25d39b0b33";

/// A SPEKE key provider whose every `@required` member carries a value this service stores
/// (`ResourceId`, `RoleArn`, `SystemIds` and `Url` are all required), so a row that
/// oversizes ONE member is refused for that member alone.
fn speke() -> Value {
    json!({
        "resourceId": "packaging-bounds",
        "roleArn": format!("{IAM_ROLE_PREFIX}mediapackage-speke"),
        "systemIds": [SYSTEM_ID],
        "url": format!("{SPEKE_URL_PREFIX}v1/copyProtection")
    })
}

/// [`speke`] with `member` carrying `value` instead.
fn speke_with(member: &str, value: Value) -> Value {
    let mut provider = speke();
    provider
        .as_object_mut()
        .expect("the provider is a JSON object")
        .insert(member.to_string(), value);
    provider
}

/// A request body naming `block` as its packaging configuration, carrying every member of
/// `members` inside it.
///
/// A CMAF package has to publish at least one child manifest
/// (`state::MIN_CMAF_HLS_MANIFESTS`), so a CMAF body always carries one: a row is then
/// refused for the member it oversized rather than for a package that publishes nothing.
fn package(block: &str, members: Value) -> Value {
    let mut configuration = if block == "cmafPackage" {
        json!({ "hlsManifests": [{ "id": "child" }] })
    } else {
        json!({})
    };
    let target = configuration
        .as_object_mut()
        .expect("the packaging configuration is a JSON object");
    for (member, value) in members
        .as_object()
        .expect("the members to merge are a JSON object")
    {
        target.insert(member.clone(), value.clone());
    }
    json!({ block: configuration })
}

/// One caller-supplied STRING member of an authorization or packaging block.
struct StringRow {
    /// The wire path the refusal is keyed by.
    path: String,
    /// What a value of this member has to START with to satisfy the CONTENT rules this
    /// service also applies to it — an ARN member names a resource of the documented
    /// service whatever its length — with the rest of the value padding.
    prefix: &'static str,
    /// The request body carrying `value` at [`Self::path`].
    body: Box<dyn Fn(&str) -> Value>,
}

/// The caller-supplied strings of a SPEKE key provider, with what each one's value has to
/// start with: every member of `SpekeKeyProvider` targeting `__string`.
const SPEKE_STRINGS: &[(&str, &str)] = &[
    ("resourceId", ""),
    ("roleArn", IAM_ROLE_PREFIX),
    ("url", SPEKE_URL_PREFIX),
    ("certificateArn", ACM_CERTIFICATE_PREFIX),
];

/// Every string row: the SPEKE provider's own strings under each of the four packaging
/// blocks, the two members of a CDN `Authorization`, and the strings a packaging block
/// carries directly.
fn string_rows() -> Vec<StringRow> {
    let mut rows: Vec<StringRow> = Vec::new();
    for block in BLOCKS {
        for (member, prefix) in SPEKE_STRINGS {
            rows.push(StringRow {
                path: format!("{block}.encryption.spekeKeyProvider.{member}"),
                prefix,
                body: Box::new(move |value| {
                    package(
                        block,
                        json!({
                            "encryption": {
                                "spekeKeyProvider": speke_with(member, json!(value))
                            }
                        }),
                    )
                }),
            });
        }
    }
    // Both members of an `Authorization` are `@required`, so each row sends the other with
    // a value this service stores.
    rows.push(StringRow {
        path: "authorization.cdnIdentifierSecret".to_string(),
        prefix: SECRET_PREFIX,
        body: Box::new(|value| {
            json!({
                "hlsPackage": {},
                "authorization": {
                    "cdnIdentifierSecret": value,
                    "secretsRoleArn": format!("{IAM_ROLE_PREFIX}mediapackage-secrets")
                }
            })
        }),
    });
    rows.push(StringRow {
        path: "authorization.secretsRoleArn".to_string(),
        prefix: IAM_ROLE_PREFIX,
        body: Box::new(|value| {
            json!({
                "hlsPackage": {},
                "authorization": {
                    "cdnIdentifierSecret": format!("{SECRET_PREFIX}cdn-identifier"),
                    "secretsRoleArn": value
                }
            })
        }),
    });
    for block in ["hlsPackage", "cmafPackage"] {
        rows.push(StringRow {
            path: format!("{block}.encryption.constantInitializationVector"),
            prefix: "",
            body: Box::new(move |value| {
                package(
                    block,
                    json!({
                        "encryption": {
                            "constantInitializationVector": value,
                            "spekeKeyProvider": speke()
                        }
                    }),
                )
            }),
        });
    }
    rows.push(StringRow {
        path: "cmafPackage.segmentPrefix".to_string(),
        prefix: "",
        body: Box::new(|value| package("cmafPackage", json!({ "segmentPrefix": value }))),
    });
    rows.push(StringRow {
        path: "dashPackage.utcTimingUri".to_string(),
        prefix: "",
        body: Box::new(|value| package("dashPackage", json!({ "utcTimingUri": value }))),
    });
    rows
}

/// One caller-supplied LIST member inside a packaging block.
struct ListRow {
    /// The wire path the refusal is keyed by.
    path: String,
    /// One entry the member accepts: a trigger list targets a modelled enum, so an entry of
    /// its own value set, and a `SystemIds` entry is free text.
    entry: String,
    /// Whether an ENTRY of this list is a caller-sized string too, which only a list of
    /// `__string` is — a trigger list's entries are the enum's own values.
    sized_entries: bool,
    /// The request body carrying `entries` at [`Self::path`].
    body: Box<dyn Fn(Value) -> Value>,
}

/// Every list row: the SPEKE provider's `systemIds` under each of the four packaging blocks,
/// the trigger lists of the HLS and DASH blocks, and the trigger list of a CMAF child
/// manifest. The trigger entries come from the value sets the model declares
/// (`types::__AD_TRIGGERS_ELEMENT_VALUES`, `types::__PERIOD_TRIGGERS_ELEMENT_VALUES`).
fn list_rows() -> Vec<ListRow> {
    let mut rows: Vec<ListRow> = Vec::new();
    for block in BLOCKS {
        rows.push(ListRow {
            path: format!("{block}.encryption.spekeKeyProvider.systemIds"),
            entry: SYSTEM_ID.to_string(),
            sized_entries: true,
            body: Box::new(move |entries| {
                package(
                    block,
                    json!({
                        "encryption": { "spekeKeyProvider": speke_with("systemIds", entries) }
                    }),
                )
            }),
        });
    }
    for (block, member, entry) in [
        (
            "hlsPackage",
            "adTriggers",
            types::__AD_TRIGGERS_ELEMENT_VALUES[0],
        ),
        (
            "dashPackage",
            "adTriggers",
            types::__AD_TRIGGERS_ELEMENT_VALUES[7],
        ),
        (
            "dashPackage",
            "periodTriggers",
            types::__PERIOD_TRIGGERS_ELEMENT_VALUES[0],
        ),
    ] {
        rows.push(ListRow {
            path: format!("{block}.{member}"),
            entry: entry.to_string(),
            sized_entries: false,
            body: Box::new(move |entries| package(block, json!({ member: entries }))),
        });
    }
    rows.push(ListRow {
        path: "cmafPackage.hlsManifests[0].adTriggers".to_string(),
        entry: types::__AD_TRIGGERS_ELEMENT_VALUES[3].to_string(),
        sized_entries: false,
        body: Box::new(|entries| {
            json!({
                "cmafPackage": {
                    "hlsManifests": [{ "id": "child", "adTriggers": entries }]
                }
            })
        }),
    });
    rows
}

/// The requirement a string past the bound is refused with, as this service words every
/// violated length bound, read from the service's own constant.
fn chars_requirement(chars: usize) -> String {
    format!(
        "may be at most {} characters, got {chars}",
        state::MAX_STRING_CHARS
    )
}

/// The requirement a list past its entry bound is refused with, as this service words every
/// violated entry count.
fn entries_requirement(count: usize) -> String {
    format!(
        "may carry at most {} entries, got {count}",
        state::MAX_PACKAGE_LIST_ENTRIES
    )
}

/// The requirement an oversized ENTRY of a list is refused with: the member, the part of it
/// at fault, and the string bound it broke.
fn entry_chars_requirement(chars: usize) -> String {
    format!(
        "an entry may be at most {} characters, got {chars}",
        state::MAX_STRING_CHARS
    )
}

/// A value for `row` exactly `chars` characters long: the prefix its content rule requires,
/// padded out with one repeated character.
fn sized(row: &StringRow, chars: usize) -> String {
    let padding = chars.saturating_sub(row.prefix.chars().count());
    format!("{}{}", row.prefix, "a".repeat(padding))
}

/// The shortest value `row` accepts in this suite: its prefix plus one character.
fn accepted(row: &StringRow) -> String {
    sized(row, row.prefix.chars().count() + 1)
}

/// `entry` repeated `count` times, as the wire carries a list.
fn entries(entry: &str, count: usize) -> Value {
    json!(vec![entry; count])
}

/// An id a resource can be addressed by, built from a member path: `state::is_resource_id`
/// admits letters, numbers, `_` and `-` only, so the path's separators become `-`.
fn id(prefix: &str, path: &str, label: &str) -> String {
    format!(
        "{prefix}-{}-{label}",
        path.replace(['.', '[', ']'], "-").to_lowercase()
    )
}

/// The JSON pointer that reads a member back out of an endpoint projection, derived from the
/// member path so the two cannot drift apart.
fn pointer(path: &str) -> String {
    format!("/{}", path.replace("[0]", ".0").replace('.', "/"))
}

/// A request body for `body`, plus the ids a create needs.
fn create_body(body: Value, endpoint_id: &str) -> Value {
    let mut body = body;
    let object = body
        .as_object_mut()
        .expect("every row's body is a JSON object");
    object.insert("id".to_string(), json!(endpoint_id));
    object.insert("channelId".to_string(), json!(CHANNEL));
    body
}

/// Assert `(status, refusal)` is this service's 422 for `member` with `requirement`.
fn assert_refused(case: &str, status: u16, refusal: &Value, member: &str, requirement: &str) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {refusal:?}"
    );
    assert_eq!(
        error_code(refusal),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
    );
    assert_eq!(
        refusal.pointer(&format!("/errors/{member}")),
        Some(&json!(requirement)),
        "case {case:?}: the refusal must be keyed by {member:?} and state {requirement:?}, \
         got {refusal:?}"
    );
}

/// Create the channel every endpoint below packages.
async fn channel(router: &smithy_server_runtime::MultiServiceRouter) {
    let (status, body) = call(
        router,
        "POST",
        "/channels",
        Some(&json!({ "id": CHANNEL })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateChannel {CHANNEL:?} expected 200, got {status} {body:?}"
    );
}

/// Assert the service stored no endpoint under `endpoint_id`.
async fn assert_not_stored(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    endpoint_id: &str,
) {
    let (status, described) = call(
        router,
        "GET",
        &format!("/origin_endpoints/{endpoint_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: the refused create must store no endpoint, got {status} {described:?}"
    );
}

/// The sizes past the string bound every string member must refuse: one character past it,
/// and far past it.
fn refused_sizes() -> [usize; 2] {
    [
        state::MAX_STRING_CHARS + 1,
        state::MAX_STRING_CHARS * 64 + 7,
    ]
}

#[tokio::test]
async fn a_packaging_string_past_the_bound_is_refused_on_create_and_stores_nothing() {
    let router = emulator();
    channel(&router).await;

    for row in string_rows() {
        for chars in refused_sizes() {
            let case = format!("create {} at {chars} characters", row.path);
            let endpoint_id = id("refused", &row.path, &chars.to_string());
            let body = create_body((row.body)(&sized(&row, chars)), &endpoint_id);
            let (status, refusal) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
            assert_refused(
                &case,
                status,
                &refusal,
                &row.path,
                &chars_requirement(chars),
            );
            assert_not_stored(&router, &case, &endpoint_id).await;
        }
    }
}

#[tokio::test]
async fn a_packaging_string_at_the_bound_is_stored_and_read_back() {
    let router = emulator();
    channel(&router).await;

    for row in string_rows() {
        let value = sized(&row, state::MAX_STRING_CHARS);
        let case = format!("create {} at the bound", row.path);
        let endpoint_id = id("stored", &row.path, "at-bound");
        let body = create_body((row.body)(&value), &endpoint_id);
        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        let read = pointer(&row.path);
        assert_eq!(
            created.pointer(&read),
            Some(&json!(value)),
            "case {case:?}: the create response must carry the value at {read:?}, \
             got {created:?}"
        );

        let (status, described) = call(
            &router,
            "GET",
            &format!("/origin_endpoints/{endpoint_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: the read must render the stored record, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_packaging_string_past_the_bound_is_refused_on_update_and_changes_nothing() {
    let router = emulator();
    channel(&router).await;

    for row in string_rows() {
        let endpoint_id = id("updated", &row.path, "string");
        let body = create_body((row.body)(&accepted(&row)), &endpoint_id);
        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
        assert_eq!(
            status, 200,
            "case {endpoint_id:?}: CreateOriginEndpoint expected 200, got {status} {created:?}"
        );

        for chars in refused_sizes() {
            let case = format!("update {} at {chars} characters", row.path);
            let (status, refusal) = call(
                &router,
                "PUT",
                &format!("/origin_endpoints/{endpoint_id}"),
                Some(&(row.body)(&sized(&row, chars))),
            )
            .await;
            assert_refused(
                &case,
                status,
                &refusal,
                &row.path,
                &chars_requirement(chars),
            );

            let (status, described) = call(
                &router,
                "GET",
                &format!("/origin_endpoints/{endpoint_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
            );
            assert_eq!(
                described, created,
                "case {case:?}: a refused update must leave the stored record alone, \
                 got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_packaging_list_past_its_entry_bound_is_refused_on_create_and_stores_nothing() {
    let router = emulator();
    channel(&router).await;

    for row in list_rows() {
        for count in [
            state::MAX_PACKAGE_LIST_ENTRIES + 1,
            state::MAX_PACKAGE_LIST_ENTRIES * 50,
        ] {
            let case = format!("create {} with {count} entries", row.path);
            let endpoint_id = id("refused", &row.path, &count.to_string());
            let body = create_body((row.body)(entries(&row.entry, count)), &endpoint_id);
            let (status, refusal) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
            assert_refused(
                &case,
                status,
                &refusal,
                &row.path,
                &entries_requirement(count),
            );
            assert_not_stored(&router, &case, &endpoint_id).await;
        }
    }
}

#[tokio::test]
async fn a_packaging_list_at_its_entry_bound_is_stored_and_read_back() {
    let router = emulator();
    channel(&router).await;

    for row in list_rows() {
        let count = state::MAX_PACKAGE_LIST_ENTRIES;
        let case = format!("create {} with {count} entries", row.path);
        let endpoint_id = id("stored", &row.path, "at-bound");
        let sent = entries(&row.entry, count);
        let body = create_body((row.body)(sent.clone()), &endpoint_id);
        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        let read = pointer(&row.path);
        assert_eq!(
            created.pointer(&read),
            Some(&sent),
            "case {case:?}: the create response must carry the {count} entries at {read:?}, \
             got {:?}",
            created.pointer(&read)
        );

        let (status, described) = call(
            &router,
            "GET",
            &format!("/origin_endpoints/{endpoint_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: the read must render the stored record, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_list_entry_past_the_string_bound_is_refused_and_stores_nothing() {
    let router = emulator();
    channel(&router).await;

    // Only a list of `__string` has caller-sized ENTRIES; a trigger list's entries are the
    // values its enum declares, so there is no entry length for a caller to choose.
    for row in list_rows().into_iter().filter(|row| row.sized_entries) {
        for chars in refused_sizes() {
            let case = format!("create {} with an entry of {chars} characters", row.path);
            let endpoint_id = id("long-entry", &row.path, &chars.to_string());
            let entry = "a".repeat(chars);
            let body = create_body((row.body)(json!([row.entry, entry])), &endpoint_id);
            let (status, refusal) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
            assert_refused(
                &case,
                status,
                &refusal,
                &row.path,
                &entry_chars_requirement(chars),
            );
            assert_not_stored(&router, &case, &endpoint_id).await;
        }
    }
}

#[tokio::test]
async fn a_packaging_list_past_its_entry_bound_is_refused_on_update_and_changes_nothing() {
    let router = emulator();
    channel(&router).await;

    for row in list_rows() {
        let endpoint_id = id("updated", &row.path, "list");
        let body = create_body((row.body)(entries(&row.entry, 1)), &endpoint_id);
        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
        assert_eq!(
            status, 200,
            "case {endpoint_id:?}: CreateOriginEndpoint expected 200, got {status} {created:?}"
        );

        let count = state::MAX_PACKAGE_LIST_ENTRIES + 1;
        let case = format!("update {} with {count} entries", row.path);
        let (status, refusal) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{endpoint_id}"),
            Some(&(row.body)(entries(&row.entry, count))),
        )
        .await;
        assert_refused(
            &case,
            status,
            &refusal,
            &row.path,
            &entries_requirement(count),
        );

        let (status, described) = call(
            &router,
            "GET",
            &format!("/origin_endpoints/{endpoint_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: a refused update must leave the stored record alone, \
             got {described:?}"
        );
    }
}
