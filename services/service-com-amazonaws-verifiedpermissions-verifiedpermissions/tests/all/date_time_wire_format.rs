// SPDX-License-Identifier: CC0-1.0

//! The WIRE FORM of every timestamp this service reports.
//!
//! RULE: a member whose model target carries `@timestampFormat("date-time")` is rendered
//! on the wire as an RFC 3339 / ISO-8601 date-time STRING — never as a bare epoch number —
//! and the instant it denotes survives a read-back unchanged. The rule is about the member,
//! not about any one operation: it holds for every such member of every shape this service
//! serves, at every nesting depth.
//!
//! Where the catalogue comes from: every timestamp member of
//! `com.amazonaws.verifiedpermissions` targets
//! `com.amazonaws.verifiedpermissions#TimestampFormat`, and
//! `smithy_query(query="shape", shape="TimestampFormat")` reports
//! `{"type": "timestamp", "timestampFormat": "date-time"}` for it. The per-member lists
//! below come from `smithy_query(query="shape", shape=<shape>)`.
//!
//! Two independent checks, because they can fail apart:
//!
//! * over the wire, through the same `MultiServiceRouter` the emulator binary serves, for
//!   every implemented operation that reports a timestamp (a table whose rows differ along
//!   the operation, the shape, the member name and the nesting depth);
//! * on the member SCHEMAS, which is what the JSON codec consults
//!   (`Schema::timestamp_format()`), for every `date-time` member this service declares —
//!   including the ones belonging to operations that still answer 501, so implementing one
//!   cannot reintroduce the epoch-number form.

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::ops::types as vp;

use crate::harness::{ok, router, store_with_mode};

/// A Cedar schema with one namespace, so `PutSchema`/`GetSchema` have something real to
/// store and report.
fn photo_app_schema() -> String {
    serde_json::to_string(&json!({
        "PhotoApp": {
            "entityTypes": { "User": { "shape": { "type": "Record", "attributes": {} } } },
            "actions": {
                "viewPhoto": {
                    "appliesTo": { "principalTypes": ["User"], "resourceTypes": ["User"] },
                },
            },
        },
    }))
    .expect("the schema document serializes")
}

/// Assert one value is the RFC 3339 rendering of a real instant, and return that instant.
///
/// Three things are checked, because each is a different way the member can be wrong: the
/// JSON type (a string, and in particular NOT a number — the defect this suite exists for),
/// the text (parses under `date-time`, and is the canonical rendering of the instant it
/// denotes rather than some other spelling of it), and the value (a real instant, not the
/// epoch).
fn assert_date_time(case: &str, value: &Value) -> DateTime {
    assert!(
        value.is_string(),
        "case {case:?}: a @timestampFormat(\"date-time\") member is a JSON string, got {value}"
    );
    assert!(
        value.as_f64().is_none(),
        "case {case:?}: a @timestampFormat(\"date-time\") member is never a JSON number, \
         got {value}"
    );
    let text = value.as_str().unwrap_or_default();
    let parsed = DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {text:?} is not an RFC 3339 date-time: {e}"));
    let rendered = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {text:?} cannot be re-rendered: {e}"));
    assert_eq!(
        rendered, text,
        "case {case:?}: the member must carry the canonical RFC 3339 rendering of its \
         instant, got {text:?}"
    );
    // 2001-09-09T01:46:40Z. Any plausible "now" is past it; epoch 0, and an epoch-seconds
    // number that was stringified rather than formatted, are not.
    assert!(
        parsed.as_secs_f64() > 1_000_000_000.0,
        "case {case:?}: expected a real instant, got {text:?} ({} s)",
        parsed.as_secs_f64()
    );
    parsed
}

// ---------------------------------------------------------------------------
// Over the wire
// ---------------------------------------------------------------------------

/// Every `date-time` member of every implemented operation comes back as an RFC 3339
/// string, and the reader reports the same instant the writer minted.
///
/// The rows differ along the operation (create vs read vs list), the shape the member
/// belongs to (an operation output, and a list element nested inside one), the member name
/// (`createdDate`, `lastUpdatedDate`, `createdAt`), the member's POSITION in its shape
/// (first member, middle, last) and whether the member is `@required`.
#[tokio::test]
async fn every_date_time_member_on_the_wire_is_an_rfc3339_string() {
    let router = router();
    let store_id = store_with_mode(&router, "OFF").await;

    let created_store = ok(
        &router,
        "CreatePolicyStore",
        &json!({ "validationSettings": { "mode": "OFF" } }),
    )
    .await;
    let store_of_created = created_store["policyStoreId"]
        .as_str()
        .expect("policyStoreId is required on the CreatePolicyStore response")
        .to_string();
    let fetched_store = ok(
        &router,
        "GetPolicyStore",
        &json!({ "policyStoreId": store_of_created }),
    )
    .await;

    let created_policy = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": {
                "static": { "statement": "permit(principal, action, resource);" },
            },
        }),
    )
    .await;
    let policy_id = created_policy["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string();
    let fetched_policy = ok(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": policy_id }),
    )
    .await;
    let listed_policies = ok(
        &router,
        "ListPolicies",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    let listed_policy = listed_policies["policies"][0].clone();

    let put_schema = ok(
        &router,
        "PutSchema",
        &json!({
            "policyStoreId": store_id,
            "definition": { "cedarJson": photo_app_schema() },
        }),
    )
    .await;
    let fetched_schema = ok(&router, "GetSchema", &json!({ "policyStoreId": store_id })).await;

    let alias_name = "policy-store-alias/wire-format";
    let created_alias = ok(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
    )
    .await;
    let fetched_alias = ok(
        &router,
        "GetPolicyStoreAlias",
        &json!({ "aliasName": alias_name }),
    )
    .await;

    // An identity source, and a batch read: the remaining implemented shapes that carry
    // timestamps. `createdDate` is the FIRST member of an identity source's shape rather
    // than a middle one, and `BatchGetPolicy` reports its timestamps inside a list element.
    let created_source = ok(
        &router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store_id,
            "configuration": {
                "openIdConnectConfiguration": {
                    "issuer": "https://auth.example.com",
                    "tokenSelection": {
                        "accessTokenOnly": { "principalIdClaim": "sub" },
                    },
                },
            },
            "principalEntityType": "MyCorp::User",
        }),
    )
    .await;
    let source_id = created_source["identitySourceId"]
        .as_str()
        .expect("identitySourceId is required on the CreateIdentitySource response")
        .to_string();
    let fetched_source = ok(
        &router,
        "GetIdentitySource",
        &json!({ "policyStoreId": store_id, "identitySourceId": source_id }),
    )
    .await;

    let batch = ok(
        &router,
        "BatchGetPolicy",
        &json!({
            "requests": [{ "policyStoreId": store_id, "policyId": policy_id }],
        }),
    )
    .await;
    let batch_item = batch["results"][0].clone();

    // (case, the member's value) — one row per `date-time` member of every implemented
    // operation that reports one.
    let rows: &[(&str, &Value)] = &[
        (
            "CreatePolicyStore.createdDate",
            &created_store["createdDate"],
        ),
        (
            "CreatePolicyStore.lastUpdatedDate",
            &created_store["lastUpdatedDate"],
        ),
        ("GetPolicyStore.createdDate", &fetched_store["createdDate"]),
        (
            "GetPolicyStore.lastUpdatedDate",
            &fetched_store["lastUpdatedDate"],
        ),
        ("CreatePolicy.createdDate", &created_policy["createdDate"]),
        (
            "CreatePolicy.lastUpdatedDate",
            &created_policy["lastUpdatedDate"],
        ),
        ("GetPolicy.createdDate", &fetched_policy["createdDate"]),
        (
            "GetPolicy.lastUpdatedDate",
            &fetched_policy["lastUpdatedDate"],
        ),
        (
            "ListPolicies.policies[0].createdDate",
            &listed_policy["createdDate"],
        ),
        (
            "ListPolicies.policies[0].lastUpdatedDate",
            &listed_policy["lastUpdatedDate"],
        ),
        ("PutSchema.createdDate", &put_schema["createdDate"]),
        ("PutSchema.lastUpdatedDate", &put_schema["lastUpdatedDate"]),
        ("GetSchema.createdDate", &fetched_schema["createdDate"]),
        (
            "GetSchema.lastUpdatedDate",
            &fetched_schema["lastUpdatedDate"],
        ),
        (
            "CreatePolicyStoreAlias.createdAt",
            &created_alias["createdAt"],
        ),
        (
            "GetPolicyStoreAlias.createdAt",
            &fetched_alias["createdAt"],
        ),
        (
            "CreateIdentitySource.createdDate",
            &created_source["createdDate"],
        ),
        (
            "CreateIdentitySource.lastUpdatedDate",
            &created_source["lastUpdatedDate"],
        ),
        (
            "GetIdentitySource.createdDate",
            &fetched_source["createdDate"],
        ),
        (
            "GetIdentitySource.lastUpdatedDate",
            &fetched_source["lastUpdatedDate"],
        ),
        (
            "BatchGetPolicy.results[0].createdDate",
            &batch_item["createdDate"],
        ),
        (
            "BatchGetPolicy.results[0].lastUpdatedDate",
            &batch_item["lastUpdatedDate"],
        ),
    ];
    for (case, value) in rows {
        assert_date_time(case, value);
    }

    // The same instant, in the same spelling, on both sides of a read-back: the format is
    // not a rendering choice one operation makes and another does not.
    let round_trips: &[(&str, &Value, &Value)] = &[
        (
            "policy store createdDate",
            &created_store["createdDate"],
            &fetched_store["createdDate"],
        ),
        (
            "policy store lastUpdatedDate",
            &created_store["lastUpdatedDate"],
            &fetched_store["lastUpdatedDate"],
        ),
        (
            "policy createdDate",
            &created_policy["createdDate"],
            &fetched_policy["createdDate"],
        ),
        (
            "policy lastUpdatedDate",
            &created_policy["lastUpdatedDate"],
            &fetched_policy["lastUpdatedDate"],
        ),
        (
            "listed policy createdDate",
            &created_policy["createdDate"],
            &listed_policy["createdDate"],
        ),
        (
            "schema createdDate",
            &put_schema["createdDate"],
            &fetched_schema["createdDate"],
        ),
        (
            "alias createdAt",
            &created_alias["createdAt"],
            &fetched_alias["createdAt"],
        ),
        (
            "identity source createdDate",
            &created_source["createdDate"],
            &fetched_source["createdDate"],
        ),
        (
            "identity source lastUpdatedDate",
            &created_source["lastUpdatedDate"],
            &fetched_source["lastUpdatedDate"],
        ),
        (
            "batch-read policy createdDate",
            &created_policy["createdDate"],
            &batch_item["createdDate"],
        ),
    ];
    for (case, written, read) in round_trips {
        let written = assert_date_time(case, written);
        let read = assert_date_time(case, read);
        assert_eq!(
            written.as_nanos(),
            read.as_nanos(),
            "case {case:?}: the reader must report the instant that was persisted, got \
             {read:?} for {written:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// On the member schemas
// ---------------------------------------------------------------------------

/// The format the JSON codec reads for one member schema.
fn format_of(schema: &Schema) -> Option<TimestampFormat> {
    schema.timestamp_format().map(|trait_| trait_.format())
}

/// EVERY `date-time` member this service declares reports `@timestampFormat("date-time")`
/// on its own member schema — the one thing the JSON codec consults when it renders a
/// timestamp (`write_timestamp` -> `Schema::timestamp_format()`), and the thing whose
/// absence made the protocol fall back to epoch seconds.
///
/// The catalogue is every member of every shape of this service's model whose target is
/// `TimestampFormat`, from `smithy_query(query="shape", shape=<shape>)` — including the
/// shapes of operations that still answer 501, which the wire test above cannot reach.
#[tokio::test]
async fn every_modelled_date_time_member_schema_carries_the_date_time_format() {
    let date_time_members: &[(&str, &Schema)] = &[
        (
            "BatchGetPolicyOutputItem$createdDate",
            &vp::BatchGetPolicyOutputItem::M_CREATED_DATE,
        ),
        (
            "BatchGetPolicyOutputItem$lastUpdatedDate",
            &vp::BatchGetPolicyOutputItem::M_LAST_UPDATED_DATE,
        ),
        (
            "CreateIdentitySourceOutput$createdDate",
            &vp::CreateIdentitySourceOutput::M_CREATED_DATE,
        ),
        (
            "CreateIdentitySourceOutput$lastUpdatedDate",
            &vp::CreateIdentitySourceOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "CreatePolicyOutput$createdDate",
            &vp::CreatePolicyOutput::M_CREATED_DATE,
        ),
        (
            "CreatePolicyOutput$lastUpdatedDate",
            &vp::CreatePolicyOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "CreatePolicyStoreAliasOutput$createdAt",
            &vp::CreatePolicyStoreAliasOutput::M_CREATED_AT,
        ),
        (
            "CreatePolicyStoreOutput$createdDate",
            &vp::CreatePolicyStoreOutput::M_CREATED_DATE,
        ),
        (
            "CreatePolicyStoreOutput$lastUpdatedDate",
            &vp::CreatePolicyStoreOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "CreatePolicyTemplateOutput$createdDate",
            &vp::CreatePolicyTemplateOutput::M_CREATED_DATE,
        ),
        (
            "CreatePolicyTemplateOutput$lastUpdatedDate",
            &vp::CreatePolicyTemplateOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "GetIdentitySourceOutput$createdDate",
            &vp::GetIdentitySourceOutput::M_CREATED_DATE,
        ),
        (
            "GetIdentitySourceOutput$lastUpdatedDate",
            &vp::GetIdentitySourceOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "GetPolicyOutput$createdDate",
            &vp::GetPolicyOutput::M_CREATED_DATE,
        ),
        (
            "GetPolicyOutput$lastUpdatedDate",
            &vp::GetPolicyOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "GetPolicyStoreAliasOutput$createdAt",
            &vp::GetPolicyStoreAliasOutput::M_CREATED_AT,
        ),
        (
            "GetPolicyStoreOutput$createdDate",
            &vp::GetPolicyStoreOutput::M_CREATED_DATE,
        ),
        (
            "GetPolicyStoreOutput$lastUpdatedDate",
            &vp::GetPolicyStoreOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "GetPolicyTemplateOutput$createdDate",
            &vp::GetPolicyTemplateOutput::M_CREATED_DATE,
        ),
        (
            "GetPolicyTemplateOutput$lastUpdatedDate",
            &vp::GetPolicyTemplateOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "GetSchemaOutput$createdDate",
            &vp::GetSchemaOutput::M_CREATED_DATE,
        ),
        (
            "GetSchemaOutput$lastUpdatedDate",
            &vp::GetSchemaOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "IdentitySourceItem$createdDate",
            &vp::IdentitySourceItem::M_CREATED_DATE,
        ),
        (
            "IdentitySourceItem$lastUpdatedDate",
            &vp::IdentitySourceItem::M_LAST_UPDATED_DATE,
        ),
        ("PolicyItem$createdDate", &vp::PolicyItem::M_CREATED_DATE),
        (
            "PolicyItem$lastUpdatedDate",
            &vp::PolicyItem::M_LAST_UPDATED_DATE,
        ),
        (
            "PolicyStoreAliasItem$createdAt",
            &vp::PolicyStoreAliasItem::M_CREATED_AT,
        ),
        (
            "PolicyStoreItem$createdDate",
            &vp::PolicyStoreItem::M_CREATED_DATE,
        ),
        (
            "PolicyStoreItem$lastUpdatedDate",
            &vp::PolicyStoreItem::M_LAST_UPDATED_DATE,
        ),
        (
            "PolicyTemplateItem$createdDate",
            &vp::PolicyTemplateItem::M_CREATED_DATE,
        ),
        (
            "PolicyTemplateItem$lastUpdatedDate",
            &vp::PolicyTemplateItem::M_LAST_UPDATED_DATE,
        ),
        (
            "PutSchemaOutput$createdDate",
            &vp::PutSchemaOutput::M_CREATED_DATE,
        ),
        (
            "PutSchemaOutput$lastUpdatedDate",
            &vp::PutSchemaOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "UpdateIdentitySourceOutput$createdDate",
            &vp::UpdateIdentitySourceOutput::M_CREATED_DATE,
        ),
        (
            "UpdateIdentitySourceOutput$lastUpdatedDate",
            &vp::UpdateIdentitySourceOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "UpdatePolicyOutput$createdDate",
            &vp::UpdatePolicyOutput::M_CREATED_DATE,
        ),
        (
            "UpdatePolicyOutput$lastUpdatedDate",
            &vp::UpdatePolicyOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "UpdatePolicyStoreOutput$createdDate",
            &vp::UpdatePolicyStoreOutput::M_CREATED_DATE,
        ),
        (
            "UpdatePolicyStoreOutput$lastUpdatedDate",
            &vp::UpdatePolicyStoreOutput::M_LAST_UPDATED_DATE,
        ),
        (
            "UpdatePolicyTemplateOutput$createdDate",
            &vp::UpdatePolicyTemplateOutput::M_CREATED_DATE,
        ),
        (
            "UpdatePolicyTemplateOutput$lastUpdatedDate",
            &vp::UpdatePolicyTemplateOutput::M_LAST_UPDATED_DATE,
        ),
    ];
    for (case, schema) in date_time_members {
        let format = format_of(schema);
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case:?}: the member schema must carry @timestampFormat(\"date-time\"), \
             got {format:?}"
        );
    }

    // The control rows: a member the model gives no `@timestampFormat` reports none, so the
    // assertion above is about the trait and not about every member schema in the crate.
    let plain_members: &[(&str, &Schema)] = &[
        (
            "CreatePolicyStoreOutput$policyStoreId",
            &vp::CreatePolicyStoreOutput::M_POLICY_STORE_ID,
        ),
        ("GetPolicyStoreOutput$tags", &vp::GetPolicyStoreOutput::M_TAGS),
        (
            "GetPolicyStoreOutput$validationSettings",
            &vp::GetPolicyStoreOutput::M_VALIDATION_SETTINGS,
        ),
        ("GetPolicyOutput$actions", &vp::GetPolicyOutput::M_ACTIONS),
    ];
    for (case, schema) in plain_members {
        let format = format_of(schema);
        assert!(
            format.is_none(),
            "case {case:?}: the model gives this member no @timestampFormat, got {format:?}"
        );
    }
}
