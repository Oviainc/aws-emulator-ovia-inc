// SPDX-License-Identifier: CC0-1.0

//! The input-constraint and read-only halves of the report-definition contract.
//!
//! `report_definition_lifecycle.rs` pins what the store does with a request the service
//! accepts. This suite pins the two rules that hold *around* that:
//!
//! 1. **Constraints.** A member value outside the constraint its shape carries in the
//!    model — an enum catalog, a length, a numeric range, a pattern, a required member
//!    that is absent — is refused with HTTP 400 `ValidationException`, and the store is
//!    left exactly as it was; a value *on* a published bound is accepted. Every catalog
//!    and bound asserted below names the `smithy_query` shape or member it comes from,
//!    and is exercised on entries the issue that bootstrapped this service never cited.
//! 2. **Bounded state.** The tag set a report accumulates is capped at the limit the
//!    model's own `Tag` doc states, so repeated tagging cannot grow the store without
//!    limit; a merge that would cross the cap is refused whole, and removing a tag frees
//!    the room again.
//! 3. **Reads report, they do not mutate.** `DescribeReportDefinitions` and
//!    `ListTagsForResource` answer the same thing however often they are called, and
//!    neither a page walk nor a refused request changes what a later caller sees.
//!
//! Both are asserted over TABLES: the rows differ in which member is pushed out of its
//! constraint, in the kind of constraint (catalog / length / range / pattern /
//! requiredness), and in the direction and distance from the bound.

use serde_json::{json, Value};

use super::report_definition_lifecycle::{
    error_code, expected_read_back, minimal_definition, stored, Cur,
};

/// The wire status and error code a refused request must carry.
///
/// `ValidationException` (HTTP 400) is both the modelled client-input error of this
/// service and what the boundary validator answers with on awsJson1_1, so a refusal is
/// the same pair whichever of the two refuses.
const REFUSED: (u16, &str) = (400, "ValidationException");

/// A definition with every required member filled, then `member` replaced by `value`.
///
/// Removing a member instead is spelled with `Value::Null` for `value`: that is what
/// lets one table row drop a required member.
fn definition_with(member: &str, value: Value) -> Value {
    let mut definition = minimal_definition("constraint-report");
    if value.is_null() {
        definition
            .as_object_mut()
            .expect("the definition is an object")
            .remove(member);
    } else {
        definition[member] = value;
    }
    definition
}

/// A string of `len` characters drawn from what the `ReportName` pattern allows, so a
/// length row fails on the LENGTH bound and nothing else.
fn name_of_length(len: usize) -> String {
    "r".repeat(len)
}

#[tokio::test]
async fn put_refuses_a_definition_member_outside_its_modelled_constraint() {
    // Rows differ in the member, the kind of constraint and the direction from the
    // bound. Catalogs and bounds come from
    // smithy_query(shape="ReportDefinition,TimeUnit,ReportFormat,CompressionFormat,\
    // SchemaElement,AdditionalArtifact,ReportVersioning,AWSRegion,ReportStatus,LastStatus").
    let table: Vec<(&str, Value, bool)> = vec![
        // Accepted: the definition the other rows perturb.
        ("required members only", definition_with("S3Prefix", json!("cur/")), true),
        // TimeUnit catalog: HOURLY | DAILY | MONTHLY. MONTHLY is accepted, WEEKLY is
        // not a member of the catalog.
        ("TimeUnit MONTHLY", definition_with("TimeUnit", json!("MONTHLY")), true),
        ("TimeUnit WEEKLY", definition_with("TimeUnit", json!("WEEKLY")), false),
        ("TimeUnit lowercase daily", definition_with("TimeUnit", json!("daily")), false),
        // ReportFormat catalog: textORcsv | Parquet. The Parquet value is accepted with
        // the Compression that states the same preference — a definition that states
        // Parquet in only one of the two is refused, which
        // `service_populated_read_back.rs` asserts as its own rule.
        (
            "Format and Compression Parquet",
            {
                let mut definition = definition_with("Format", json!("Parquet"));
                definition["Compression"] = json!("Parquet");
                definition
            },
            true,
        ),
        ("Format JSON", definition_with("Format", json!("JSON")), false),
        // CompressionFormat catalog: ZIP | GZIP | Parquet.
        ("Compression ZIP", definition_with("Compression", json!("ZIP")), true),
        ("Compression BROTLI", definition_with("Compression", json!("BROTLI")), false),
        // SchemaElement catalog: RESOURCES | SPLIT_COST_ALLOCATION_DATA |
        // MANUAL_DISCOUNT_COMPATIBILITY.
        (
            "AdditionalSchemaElements SPLIT_COST_ALLOCATION_DATA",
            definition_with("AdditionalSchemaElements", json!(["SPLIT_COST_ALLOCATION_DATA"])),
            true,
        ),
        (
            "AdditionalSchemaElements RESOURCE_IDS",
            definition_with("AdditionalSchemaElements", json!(["RESOURCE_IDS"])),
            false,
        ),
        (
            "AdditionalSchemaElements one good one bad entry",
            definition_with("AdditionalSchemaElements", json!(["RESOURCES", "TAGS"])),
            false,
        ),
        // AdditionalArtifact catalog: REDSHIFT | QUICKSIGHT | ATHENA.
        (
            "AdditionalArtifacts QUICKSIGHT",
            definition_with("AdditionalArtifacts", json!(["QUICKSIGHT"])),
            true,
        ),
        (
            "AdditionalArtifacts GLUE",
            definition_with("AdditionalArtifacts", json!(["GLUE"])),
            false,
        ),
        // ReportVersioning catalog: CREATE_NEW_REPORT | OVERWRITE_REPORT.
        (
            "ReportVersioning CREATE_NEW_REPORT",
            definition_with("ReportVersioning", json!("CREATE_NEW_REPORT")),
            true,
        ),
        (
            "ReportVersioning APPEND_REPORT",
            definition_with("ReportVersioning", json!("APPEND_REPORT")),
            false,
        ),
        // AWSRegion catalog: 28 entries, including the China partition. cn-north-1 and
        // me-central-1 are in it; mars-central-1 is not, and neither is the
        // GovCloud region the catalog omits.
        ("S3Region cn-north-1", definition_with("S3Region", json!("cn-north-1")), true),
        ("S3Region me-central-1", definition_with("S3Region", json!("me-central-1")), true),
        ("S3Region mars-central-1", definition_with("S3Region", json!("mars-central-1")), false),
        ("S3Region us-gov-west-1", definition_with("S3Region", json!("us-gov-west-1")), false),
        // ReportName: length 0..256, pattern ^[0-9A-Za-z!\-_.*\'()]+$ — both bounds and
        // one character past each.
        (
            "ReportName at the 256-character length bound",
            definition_with("ReportName", json!(name_of_length(256))),
            true,
        ),
        (
            "ReportName one character past the length bound",
            definition_with("ReportName", json!(name_of_length(257))),
            false,
        ),
        ("ReportName empty", definition_with("ReportName", json!("")), false),
        (
            "ReportName with a space, outside the pattern",
            definition_with("ReportName", json!("report with spaces")),
            false,
        ),
        (
            "ReportName with a slash, outside the pattern",
            definition_with("ReportName", json!("reports/daily")),
            false,
        ),
        // S3Bucket: pattern ^[A-Za-z0-9_\.\-]+$.
        ("S3Bucket with dots and dashes", definition_with("S3Bucket", json!("my.bucket-1")), true),
        ("S3Bucket with a slash", definition_with("S3Bucket", json!("my-bucket/cur")), false),
        // S3Prefix: pattern ^[0-9A-Za-z!\-_.*\'()/]*$, so it may be empty but may not
        // hold a space.
        ("S3Prefix empty", definition_with("S3Prefix", json!("")), true),
        ("S3Prefix with a space", definition_with("S3Prefix", json!("cur reports/")), false),
        // ReportStatus$lastDelivery: length 16..20, pattern
        // ^[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})$.
        (
            "ReportStatus lastDelivery with an offset and lastStatus ERROR_PERMISSIONS",
            definition_with(
                "ReportStatus",
                json!({ "lastDelivery": "20240101T000000+0200", "lastStatus": "ERROR_PERMISSIONS" }),
            ),
            true,
        ),
        (
            "ReportStatus lastDelivery as an ISO-8601 instant, outside the pattern",
            definition_with("ReportStatus", json!({ "lastDelivery": "2024-01-01T00:00:00Z" })),
            false,
        ),
        (
            "ReportStatus lastStatus outside its catalog",
            definition_with("ReportStatus", json!({ "lastStatus": "PENDING" })),
            false,
        ),
        // BillingViewArn: length 0..128, pattern
        // ^(arn:aws(-cn)?:billing::[0-9]{12}:billingview/)?[a-zA-Z0-9_\+=\.\-@].{1,30}$.
        (
            "BillingViewArn in the aws-cn partition",
            definition_with(
                "BillingViewArn",
                json!("arn:aws-cn:billing::123456789012:billingview/123456789012"),
            ),
            true,
        ),
        (
            "BillingViewArn with an 11-digit account id, outside the pattern",
            definition_with(
                "BillingViewArn",
                json!("arn:aws:billing::12345678901:billingview/123456789012"),
            ),
            false,
        ),
        // Required members: the model marks all eight as required.
        ("S3Bucket absent", definition_with("S3Bucket", Value::Null), false),
        ("S3Prefix absent", definition_with("S3Prefix", Value::Null), false),
        ("S3Region absent", definition_with("S3Region", Value::Null), false),
        ("TimeUnit absent", definition_with("TimeUnit", Value::Null), false),
        ("Format absent", definition_with("Format", Value::Null), false),
        ("Compression absent", definition_with("Compression", Value::Null), false),
        (
            "AdditionalSchemaElements absent",
            definition_with("AdditionalSchemaElements", Value::Null),
            false,
        ),
        ("ReportName absent", definition_with("ReportName", Value::Null), false),
    ];

    for (case, definition, accepted) in table {
        let cur = Cur::new();
        let (status, body) = cur.put(&definition, None).await;

        if accepted {
            assert_eq!(status, 200, "case {case:?}: expected HTTP 200, got {status} {body:?}");
            let stored_reports = cur.read_back_all(case).await;
            assert_eq!(
                stored_reports.len(),
                1,
                "case {case:?}: expected the accepted definition to be stored, got {stored_reports:?}"
            );
            assert_eq!(
                stored_reports[0],
                expected_read_back(&definition),
                "case {case:?}: expected the stored definition plus the service-populated \
                 members, got {:?}",
                stored_reports[0]
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                REFUSED,
                "case {case:?}: expected {REFUSED:?}, got {status} {body:?}"
            );
            let stored_reports = cur.describe_all().await;
            assert!(
                stored_reports.is_empty(),
                "case {case:?}: a refused PutReportDefinition must store nothing, got {stored_reports:?}"
            );
        }
    }
}

#[tokio::test]
async fn tag_input_outside_its_modelled_constraint_is_refused() {
    // Tag$Key length 1..128, Tag$Value length 0..256, both pattern `.*`; TagList length
    // 0..200 (smithy_query(shape="Tag,TagList")). Rows walk each bound and one past it,
    // through both operations that take tags. The per-REPORT limit is 50 ("You can add
    // up to 50 tags to a report definition", the modelled `Tag` doc), so a list that is
    // within the modelled 200-entry request bound can still be refused for crossing it.
    let table: Vec<(&str, Value, bool)> = vec![
        ("no tags at all", json!([]), true),
        ("one tag", json!([{ "Key": "team", "Value": "fin" }]), true),
        ("an empty value, which the 0 length minimum allows", json!([{ "Key": "k", "Value": "" }]), true),
        (
            "a key at the 128-character length bound",
            json!([{ "Key": "k".repeat(128), "Value": "v" }]),
            true,
        ),
        (
            "a key one character past the length bound",
            json!([{ "Key": "k".repeat(129), "Value": "v" }]),
            false,
        ),
        ("an empty key, below the 1 character minimum", json!([{ "Key": "", "Value": "v" }]), false),
        (
            "a value at the 256-character length bound",
            json!([{ "Key": "k", "Value": "v".repeat(256) }]),
            true,
        ),
        (
            "a value one character past the length bound",
            json!([{ "Key": "k", "Value": "v".repeat(257) }]),
            false,
        ),
        (
            "a list at the 50-tag per-report bound",
            Value::Array(
                (0..50)
                    .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
                    .collect(),
            ),
            true,
        ),
        (
            "a list one tag past the per-report bound",
            Value::Array(
                (0..51)
                    .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
                    .collect(),
            ),
            false,
        ),
        (
            "a list at the 200-entry request bound, which is past the per-report bound",
            Value::Array(
                (0..200)
                    .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
                    .collect(),
            ),
            false,
        ),
        (
            "a list one entry past the 200-entry request bound",
            Value::Array(
                (0..201)
                    .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
                    .collect(),
            ),
            false,
        ),
        ("a tag with no Value, which is required", json!([{ "Key": "k" }]), false),
        ("a tag with no Key, which is required", json!([{ "Value": "v" }]), false),
    ];

    for (case, tags, accepted) in table {
        let expected_tag_count = tags.as_array().map_or(0, Vec::len);

        // Through PutReportDefinition, which takes tags beside the definition...
        let cur = Cur::new();
        let definition = minimal_definition("tagged-report");
        let (status, body) = cur.put(&definition, Some(&tags)).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?} via PutReportDefinition: expected HTTP 200, got {status} {body:?}"
            );
            let (_, listed) = cur.tags_of("tagged-report").await;
            let listed_count = listed["Tags"].as_array().map_or(0, Vec::len);
            assert_eq!(
                listed_count, expected_tag_count,
                "case {case:?}: expected {expected_tag_count} tags persisted, got {listed:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                REFUSED,
                "case {case:?} via PutReportDefinition: expected {REFUSED:?}, got {status} {body:?}"
            );
            let stored_reports = cur.describe_all().await;
            assert!(
                stored_reports.is_empty(),
                "case {case:?}: a refused PutReportDefinition must store nothing, got {stored_reports:?}"
            );
        }

        // ...and through TagResource, which takes them on their own.
        let cur = Cur::new();
        assert_eq!(
            cur.put(&minimal_definition("tagged-report"), None).await.0,
            200,
            "case {case:?}: the report TagResource tags must exist first"
        );
        let (status, body) = cur
            .call(
                "TagResource",
                &json!({ "ReportName": "tagged-report", "Tags": tags }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?} via TagResource: expected HTTP 200, got {status} {body:?}"
            );
            let (_, listed) = cur.tags_of("tagged-report").await;
            let listed_count = listed["Tags"].as_array().map_or(0, Vec::len);
            assert_eq!(
                listed_count, expected_tag_count,
                "case {case:?}: expected {expected_tag_count} tags persisted, got {listed:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                REFUSED,
                "case {case:?} via TagResource: expected {REFUSED:?}, got {status} {body:?}"
            );
            let (_, listed) = cur.tags_of("tagged-report").await;
            let listed_count = listed["Tags"].as_array().map_or(0, Vec::len);
            assert_eq!(
                listed_count, 0,
                "case {case:?}: a refused TagResource must attach nothing, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_tag_set_of_a_report_is_bounded_however_often_it_is_tagged() {
    // The per-report tag limit comes from the modelled `Tag` doc: "Describes a tag. A
    // tag is a key-value pair. You can add up to 50 tags to a report definition."
    // (smithy_query(shape="Tag")). The rule is about the report's resulting KEY SET, so
    // rows differ in how many keys the report already holds, how many fresh keys the
    // second call adds and how many keys it merely overwrites.
    const LIMIT: usize = 50;
    let table: Vec<(&str, usize, usize, usize, bool)> = vec![
        // case, keys already held, fresh keys added, held keys overwritten, accepted
        ("empty report, one fresh key", 0, 1, 0, true),
        ("empty report, a full set of keys", 0, LIMIT, 0, true),
        ("empty report, one key past the limit", 0, LIMIT + 1, 0, false),
        ("report at the limit, overwriting keys it holds", LIMIT, 0, 10, true),
        ("report at the limit, one fresh key", LIMIT, 1, 0, false),
        ("report one key below the limit, one fresh key", LIMIT - 1, 1, 0, true),
        ("report one key below the limit, two fresh keys", LIMIT - 1, 2, 0, false),
        ("report part full, filling it exactly", 30, 20, 0, true),
        ("report part full, one key too many", 30, 21, 0, false),
        ("report part full, fresh keys plus overwrites", 30, 20, 5, true),
    ];

    for (case, held, fresh, overwritten, accepted) in table {
        let cur = Cur::new();
        let existing: Vec<Value> = (0..held)
            .map(|index| json!({ "Key": format!("held-{index}"), "Value": "original" }))
            .collect();
        let (status, body) = cur
            .put(
                &minimal_definition("bounded-report"),
                Some(&Value::Array(existing)),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: seeding {held} tags must be accepted, got {status} {body:?}"
        );

        let mut merged: Vec<Value> = (0..fresh)
            .map(|index| json!({ "Key": format!("fresh-{index}"), "Value": "added" }))
            .collect();
        merged.extend(
            (0..overwritten).map(|index| json!({ "Key": format!("held-{index}"), "Value": "updated" })),
        );
        let (status, body) = cur
            .call(
                "TagResource",
                &json!({ "ReportName": "bounded-report", "Tags": merged }),
            )
            .await;

        let (_, listed) = cur.tags_of("bounded-report").await;
        let tags = listed["Tags"].as_array().cloned().unwrap_or_default();
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body:?}"
            );
            assert_eq!(
                tags.len(),
                held + fresh,
                "case {case:?}: expected {} tags after the merge, got {tags:?}",
                held + fresh
            );
            assert!(
                tags.len() <= LIMIT,
                "case {case:?}: a report must never hold more than {LIMIT} tags, got {tags:?}"
            );
            let updated = tags
                .iter()
                .filter(|tag| tag["Value"] == json!("updated"))
                .count();
            assert_eq!(
                updated, overwritten,
                "case {case:?}: expected {overwritten} overwritten values, got {tags:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                REFUSED,
                "case {case:?}: expected {REFUSED:?}, got {status} {body:?}"
            );
            assert_eq!(
                tags.len(),
                held,
                "case {case:?}: a refused merge must leave the tag set as it was ({held} tags), got {tags:?}"
            );
            assert!(
                tags.iter().all(|tag| tag["Value"] == json!("original")),
                "case {case:?}: a refused merge must not overwrite a value either, got {tags:?}"
            );
        }
    }

    // Removing a tag frees room again: the limit bounds what is held, not what has ever
    // been named.
    let cur = Cur::new();
    let full: Vec<Value> = (0..LIMIT)
        .map(|index| json!({ "Key": format!("held-{index}"), "Value": "original" }))
        .collect();
    assert_eq!(
        cur.put(&minimal_definition("recycled-report"), Some(&Value::Array(full)))
            .await
            .0,
        200,
        "case reclamation: a report at the limit must be creatable"
    );
    let one_more = json!({ "ReportName": "recycled-report", "Tags": [{ "Key": "fresh", "Value": "added" }] });
    let (status, body) = cur.call("TagResource", &one_more).await;
    assert_eq!(
        (status, error_code(&body)),
        REFUSED,
        "case reclamation: a report at the limit must refuse a fresh key, got {status} {body:?}"
    );
    let (status, body) = cur
        .call(
            "UntagResource",
            &json!({ "ReportName": "recycled-report", "TagKeys": ["held-0"] }),
        )
        .await;
    assert_eq!(status, 200, "case reclamation: UntagResource got {body:?}");
    let (status, body) = cur.call("TagResource", &one_more).await;
    assert_eq!(
        status, 200,
        "case reclamation: with a key removed the fresh key must be accepted, got {status} {body:?}"
    );
    let (_, listed) = cur.tags_of("recycled-report").await;
    let keys: Vec<&str> = listed["Tags"]
        .as_array()
        .expect("Tags is a list")
        .iter()
        .map(|tag| tag["Key"].as_str().unwrap_or("<none>"))
        .collect();
    assert_eq!(
        keys.len(),
        LIMIT,
        "case reclamation: expected {LIMIT} tags, got {listed:?}"
    );
    assert!(
        keys.contains(&"fresh") && !keys.contains(&"held-0"),
        "case reclamation: expected the removed key gone and the fresh key present, got {listed:?}"
    );
}

#[tokio::test]
async fn describe_refuses_a_page_request_outside_its_modelled_constraints() {
    // MaxResults carries range 5..5 and NextToken length 0..256 with pattern
    // ^[A-Za-z0-9_\.\-=]*$ (smithy_query(operation="DescribeReportDefinitions")): 5 is
    // the only page size the range admits, and each side of it is out.
    let in_pattern_token = "a".repeat(256);
    let table: Vec<(&str, Value, bool)> = vec![
        ("no paging members", json!({}), true),
        ("MaxResults at the only value the 5..5 range allows", json!({ "MaxResults": 5 }), true),
        ("MaxResults one below the range", json!({ "MaxResults": 4 }), false),
        ("MaxResults one above the range", json!({ "MaxResults": 6 }), false),
        ("MaxResults zero", json!({ "MaxResults": 0 }), false),
        ("MaxResults negative", json!({ "MaxResults": -1 }), false),
        ("MaxResults far above the range", json!({ "MaxResults": 1000 }), false),
        ("NextToken empty, which the 0 length minimum allows", json!({ "NextToken": "" }), true),
        (
            "NextToken at the 256-character length bound, well formed but not one this service issued",
            json!({ "NextToken": in_pattern_token }),
            false,
        ),
        (
            "NextToken one character past the length bound",
            json!({ "NextToken": "a".repeat(257) }),
            false,
        ),
        (
            "NextToken holding a character outside the pattern",
            json!({ "NextToken": "not a token!" }),
            false,
        ),
        (
            "NextToken that is in-pattern but not hex, so it names no report",
            json!({ "NextToken": "Zz_.-=" }),
            false,
        ),
    ];

    for (case, request, accepted) in table {
        let cur = Cur::new();
        cur.seed(vec![stored("paged-report")]);

        let (status, body) = cur.describe(&request).await;
        if accepted {
            assert_eq!(status, 200, "case {case:?}: expected HTTP 200, got {status} {body:?}");
            let names: Vec<&str> = body["ReportDefinitions"]
                .as_array()
                .expect("ReportDefinitions is a list")
                .iter()
                .map(|definition| definition["ReportName"].as_str().unwrap_or("<none>"))
                .collect();
            assert_eq!(
                names,
                vec!["paged-report"],
                "case {case:?}: expected the stored report on the page, got {body:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                REFUSED,
                "case {case:?}: expected {REFUSED:?}, got {status} {body:?}"
            );
        }

        // Whatever the verdict, a page request stores nothing and removes nothing.
        let stored_reports = cur.describe_all().await;
        assert_eq!(
            stored_reports.len(),
            1,
            "case {case:?}: DescribeReportDefinitions must leave the store alone, got {stored_reports:?}"
        );
    }
}

#[tokio::test]
async fn reads_report_the_same_store_however_often_they_are_called() {
    // Rows differ in how many reports are stored relative to the five-report page size,
    // so the repeated read spans a single page, a full page and a page walk.
    for stored_count in [0_usize, 1, 5, 7] {
        let case = format!("{stored_count} reports stored");
        let cur = Cur::new();
        cur.seed(
            (0..stored_count)
                .map(|index| stored(&format!("report-{index}")))
                .collect(),
        );
        if stored_count > 0 {
            let (status, body) = cur
                .call(
                    "TagResource",
                    &json!({
                        "ReportName": "report-0",
                        "Tags": [{ "Key": "team", "Value": "fin" }],
                    }),
                )
                .await;
            assert_eq!(status, 200, "case {case:?}: TagResource: got {body:?}");
        }

        let first_pass = cur.describe_all().await;
        assert_eq!(
            first_pass.len(),
            stored_count,
            "case {case:?}: expected every stored report once, got {first_pass:?}"
        );

        // Reading again — and reading the tags, and asking for a page that does not
        // exist — must answer exactly the same thing: a read reports, it does not
        // consume.
        for pass in 1..=3 {
            let again = cur.describe_all().await;
            assert_eq!(
                again, first_pass,
                "case {case:?}, pass {pass}: a repeated read must answer the same page, got {again:?}"
            );
            if stored_count > 0 {
                let (status, tags) = cur.tags_of("report-0").await;
                assert_eq!(
                    (status, &tags["Tags"]),
                    (200, &json!([{ "Key": "team", "Value": "fin" }])),
                    "case {case:?}, pass {pass}: expected the persisted tags, got {tags:?}"
                );
            }
        }

        let after = cur.describe_all().await;
        assert_eq!(
            after, first_pass,
            "case {case:?}: the store must be unchanged by reads, got {after:?}"
        );
    }
}
