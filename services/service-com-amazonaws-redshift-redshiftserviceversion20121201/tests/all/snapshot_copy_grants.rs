// SPDX-License-Identifier: CC0-1.0

//! The snapshot copy grant lifecycle, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: a created grant is reported and stored exactly as the request
//! configured it, whatever the shape of its name, the form of the key it names
//! and how many tags it carries; a request naming no key gets the account's
//! default key; the NAME being what an account's grants are unique on, so a
//! second create under one name is refused and overwrites nothing; the
//! documented name syntax; a name that identifies no grant being refused by the
//! read and by the delete; a delete making the grant absent from every later
//! read while leaving every other record where it was, and freeing the name; a
//! grant a cluster's snapshot copy is configured with being refused by the
//! delete; the creation order, the `TagKeys` / `TagValues` filter, the service's
//! `MaxRecords` window and its `Marker` cursor; and the documented tag
//! restrictions being refused with nothing stored.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::ClusterSnapshotCopyStatus;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// One row of the create table: everything a `CreateSnapshotCopyGrant` request
/// carries.
struct CreateCase {
    case: &'static str,
    name: String,
    /// The `KmsKeyId` the request names, or `None` when it names none.
    kms_key_id: Option<String>,
    tags: Vec<(&'static str, &'static str)>,
}

/// A create row naming no key and carrying no tags.
fn simple_case(case: &'static str, name: &str) -> CreateCase {
    CreateCase {
        case,
        name: name.to_string(),
        kms_key_id: None,
        tags: Vec::new(),
    }
}

/// `CreateSnapshotCopyGrant` with the name, the optional key and the tags of one
/// row.
async fn create_grant(emulator: &Emulator, row: &CreateCase) -> Answer {
    let mut params: Vec<(String, String)> =
        vec![("SnapshotCopyGrantName".to_string(), row.name.clone())];
    if let Some(key) = row.kms_key_id.as_deref() {
        params.push(("KmsKeyId".to_string(), key.to_string()));
    }
    for (position, (key, value)) in row.tags.iter().enumerate() {
        params.push((
            format!("Tags.Tag.{}.Key", position + 1),
            (*key).to_string(),
        ));
        params.push((
            format!("Tags.Tag.{}.Value", position + 1),
            (*value).to_string(),
        ));
    }
    call_with(emulator, "CreateSnapshotCopyGrant", &params).await
}

/// Drive one action with owned parameters, so a case can build its values.
async fn call_with(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// `DeleteSnapshotCopyGrant` for one name.
async fn delete_one(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "DeleteSnapshotCopyGrant",
            &[("SnapshotCopyGrantName", name)],
        )
        .await
}

/// `DescribeSnapshotCopyGrants` for one name.
async fn describe_one(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "DescribeSnapshotCopyGrants",
            &[("SnapshotCopyGrantName", name)],
        )
        .await
}

/// The `<SnapshotCopyGrantName>` of every grant a body carries, in the order the
/// page lists them.
fn listed_grants(body: &str) -> Vec<String> {
    all_text(body, "SnapshotCopyGrantName")
}

/// The `<KmsKeyId>` of every grant a body carries, in document order.
fn listed_keys(body: &str) -> Vec<String> {
    all_text(body, "KmsKeyId")
}

/// The tag key/value pairs a body carries, in document order.
fn listed_tags(body: &str) -> Vec<(String, String)> {
    all_text(body, "Key")
        .into_iter()
        .zip(all_text(body, "Value"))
        .collect()
}

/// The key the store holds for one grant, read straight from this service's
/// state rather than from a response.
fn stored_key(emulator: &Emulator, name: &str) -> Option<String> {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let store = store.lock();
    store
        .snapshot_copy_grant(name)
        .and_then(|grant| grant.kms_key_id.clone())
}

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// A name of `len` characters admitted by the documented syntax, for a row that
/// varies only in size.
fn name_of_length(len: usize) -> String {
    debug_assert!(len >= 1, "a name of length {len} has no first character");
    std::iter::once('g')
        .chain(std::iter::repeat_n('x', len.saturating_sub(1)))
        .collect()
}

/// A create reports the grant the request describes and stores it for every
/// later read, resolving the account's default key when the request names none.
///
/// The rows differ along everything the rule generalises over: the form and size
/// of the name, the form of the key named (absent, a bare key id, an alias, a
/// full ARN) and how many tags the request carries.
#[tokio::test]
async fn a_create_reports_and_stores_what_the_request_describes() {
    let emulator = Emulator::new();

    let rows = vec![
        simple_case("the minimal grant", "grant-one"),
        CreateCase {
            case: "a bare key id and one tag",
            name: "grant-2-west-01".to_string(),
            kms_key_id: Some("1234abcd-12ab-34cd-56ef-1234567890ab".to_string()),
            tags: vec![("owner", "admin")],
        },
        CreateCase {
            case: "an alias and two tags",
            name: "grant-alias".to_string(),
            kms_key_id: Some("alias/copy-grants".to_string()),
            tags: vec![("owner", "admin"), ("environment", "test")],
        },
        CreateCase {
            case: "a full key ARN",
            name: "grant-arn".to_string(),
            kms_key_id: Some(
                "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab"
                    .to_string(),
            ),
            tags: vec![("environment", "production")],
        },
        CreateCase {
            case: "a name at the documented maximum length",
            // The documented syntax admits 1 to 63 characters.
            name: name_of_length(63),
            kms_key_id: Some("alias/long-name-grant".to_string()),
            tags: vec![("owner", "fleet")],
        },
        CreateCase {
            case: "a name of one character",
            name: "g".to_string(),
            kms_key_id: None,
            tags: Vec::new(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let created = create_grant(&emulator, row).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} with body {}",
            created.status, created.body
        );

        // A request that names no key gets the account's default key, which the
        // grant then reports like any other.
        let expected_key = row
            .kms_key_id
            .clone()
            .unwrap_or_else(|| state::DEFAULT_SNAPSHOT_COPY_GRANT_KMS_KEY.to_string());
        assert_eq!(
            listed_grants(&created.body),
            vec![row.name.clone()],
            "case {case}: expected the created grant named {:?}, got {:?}",
            row.name,
            listed_grants(&created.body)
        );
        assert_eq!(
            listed_keys(&created.body),
            vec![expected_key.clone()],
            "case {case}: expected key {expected_key:?}, got {:?}",
            listed_keys(&created.body)
        );

        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            listed_tags(&created.body),
            expected_tags,
            "case {case}: expected tags {expected_tags:?}, got {:?}",
            listed_tags(&created.body)
        );

        // The round trip: the same record every later read reports, and the same
        // key the store holds.
        let read = describe_one(&emulator, &row.name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the created grant to be readable, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            listed_grants(&read.body),
            vec![row.name.clone()],
            "case {case}: the read reported {:?}, not the created grant",
            listed_grants(&read.body)
        );
        assert_eq!(
            listed_keys(&read.body),
            vec![expected_key.clone()],
            "case {case}: the read reported key {:?}, not what the create did",
            listed_keys(&read.body)
        );
        assert_eq!(
            listed_tags(&read.body),
            expected_tags,
            "case {case}: the read reported tags {:?}, not what the create did",
            listed_tags(&read.body)
        );
        let stored = stored_key(&emulator, &row.name);
        assert_eq!(
            stored,
            Some(expected_key.clone()),
            "case {case}: expected the store to hold key {expected_key:?}, got {stored:?}"
        );
    }

    // Every row is one record of the account, in creation order.
    let listed = emulator.call("DescribeSnapshotCopyGrants", &[]).await;
    let expected_order: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    assert_eq!(
        listed_grants(&listed.body),
        expected_order,
        "case the whole account: expected the creation order {expected_order:?}, got {:?}",
        listed_grants(&listed.body)
    );
}

/// The NAME is what an account's snapshot copy grants are unique on, so a second
/// create under one name is refused and overwrites nothing.
#[tokio::test]
async fn a_second_create_of_one_name_is_refused_and_overwrites_nothing() {
    let emulator = Emulator::new();

    for (case, name) in [
        ("a plain name", "grant-dup"),
        ("a name with digits", "grant-dup-2"),
        ("a one-character name", "d"),
    ] {
        let first = CreateCase {
            kms_key_id: Some("alias/first-key".to_string()),
            tags: vec![("owner", "admin")],
            ..simple_case(case, name)
        };
        assert_eq!(
            create_grant(&emulator, &first).await.status,
            200,
            "case {case}: the first create must be served"
        );

        // A second create differing in every member BUT the name: the name alone
        // decides, and the stored record is untouched.
        let second = CreateCase {
            case,
            name: name.to_string(),
            kms_key_id: Some("alias/other-key".to_string()),
            tags: vec![("owner", "someone-else")],
        };
        let answer = create_grant(&emulator, &second).await;
        assert_refused(case, &answer, 400, "SnapshotCopyGrantAlreadyExistsFault");

        let read = describe_one(&emulator, name).await;
        assert_eq!(
            listed_keys(&read.body),
            vec!["alias/first-key".to_string()],
            "case {case}: the refused create changed the stored key: {}",
            read.body
        );
        assert_eq!(
            listed_tags(&read.body),
            vec![("owner".to_string(), "admin".to_string())],
            "case {case}: the refused create changed the stored tags: {}",
            read.body
        );
        assert_eq!(
            listed_grants(&read.body).len(),
            1,
            "case {case}: expected exactly one record for the name, got {:?}",
            listed_grants(&read.body)
        );
    }
}

/// A name outside the documented syntax is refused and stores nothing, while
/// every name inside it is served.
///
/// The syntax is the one `state::SNAPSHOT_COPY_GRANT_NAME_RULE` carries
/// (`smithy_query(query="doc", shape="CreateSnapshotCopyGrantMessage",
/// member="SnapshotCopyGrantName")`): 1 to 63 lowercase alphanumeric characters
/// or hyphens, a letter first, no trailing hyphen and no two consecutive
/// hyphens. It is tested on each published bound and one past it.
#[tokio::test]
async fn names_outside_the_documented_syntax_are_refused() {
    let emulator = Emulator::new();

    let maximum = state::SNAPSHOT_COPY_GRANT_NAME_RULE.max_len;
    let rows: Vec<(&str, String, bool)> = vec![
        ("one character, the smallest admitted", "a".to_string(), true),
        ("at the maximum length", name_of_length(maximum), true),
        ("one past the maximum length", name_of_length(maximum + 1), false),
        ("digits and hyphens inside", "grant-07-west".to_string(), true),
        ("an uppercase letter", "Grant-upper".to_string(), false),
        ("a leading digit", "7grant".to_string(), false),
        ("a trailing hyphen", "grant-".to_string(), false),
        ("two consecutive hyphens", "grant--two".to_string(), false),
        ("an underscore", "grant_one".to_string(), false),
        ("the empty name", String::new(), false),
    ];

    let mut served: Vec<String> = Vec::new();
    for (case, name, admitted) in &rows {
        let row = CreateCase {
            case: "a syntax row",
            name: name.clone(),
            kms_key_id: None,
            tags: Vec::new(),
        };
        let answer = create_grant(&emulator, &row).await;
        if *admitted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the name {name:?} served with 200, got {} with body {}",
                answer.status, answer.body
            );
            served.push(name.clone());
        } else {
            assert_refused(case, &answer, 400, "ValidationException");
        }
    }

    // Only the admitted rows are stored: every refused create stored nothing.
    let listed = emulator.call("DescribeSnapshotCopyGrants", &[]).await;
    assert_eq!(
        listed_grants(&listed.body),
        served,
        "case the whole account: expected only {served:?}, got {:?}",
        listed_grants(&listed.body)
    );
}

/// A `SnapshotCopyGrantName` that identifies no grant is refused by the read,
/// while naming none reports every grant the account has; naming both a grant and
/// a cursor is refused.
#[tokio::test]
async fn reading_a_name_that_identifies_no_grant_is_refused() {
    let emulator = Emulator::new();

    // On an empty account every name identifies nothing.
    for (case, name) in [
        ("an empty account", "grant-absent"),
        ("a name with separators", "grant-absent-2"),
    ] {
        let answer = describe_one(&emulator, name).await;
        assert_refused(case, &answer, 400, "SnapshotCopyGrantNotFoundFault");
    }

    let present = simple_case("a stored grant", "grant-present");
    assert_eq!(
        create_grant(&emulator, &present).await.status,
        200,
        "case a stored grant: the create must be served"
    );

    // One stored grant does not make a different name resolve, and the match is
    // exact: a prefix or a neighbouring name identifies no record.
    for (case, name) in [
        ("a neighbouring name", "grant-present-2"),
        ("a prefix of the stored name", "grant-pres"),
    ] {
        let answer = describe_one(&emulator, name).await;
        assert_refused(case, &answer, 400, "SnapshotCopyGrantNotFoundFault");
    }

    let listed = emulator.call("DescribeSnapshotCopyGrants", &[]).await;
    assert_eq!(
        listed_grants(&listed.body),
        vec!["grant-present".to_string()],
        "case naming no grant: expected the one stored grant, got {:?}",
        listed_grants(&listed.body)
    );

    // "You can specify either the SnapshotCopyGrantName parameter or the Marker
    // parameter, but not both."
    for (case, name, cursor) in [
        ("a stored name with a cursor", "grant-present", "0.deadbeef"),
        ("an unknown name with a cursor", "grant-absent", "0.deadbeef"),
    ] {
        let answer = emulator
            .call(
                "DescribeSnapshotCopyGrants",
                &[("SnapshotCopyGrantName", name), ("Marker", cursor)],
            )
            .await;
        assert_refused(case, &answer, 400, "InvalidParameterCombination");
    }
}

/// The `TagKeys` / `TagValues` filter selects GRANTS: a grant one of its tags
/// matches reports every tag it carries, and a grant with no matching tag is left
/// out.
#[tokio::test]
async fn the_tag_filter_selects_whole_grants() {
    let emulator = Emulator::new();

    for (name, tags) in [
        ("grant-owner-admin", vec![("owner", "admin")]),
        (
            "grant-owner-admin-test",
            vec![("owner", "admin"), ("environment", "test")],
        ),
        ("grant-environment-prod", vec![("environment", "production")]),
        ("grant-untagged", vec![]),
    ] {
        let row = CreateCase {
            tags,
            ..simple_case("a tagged grant", name)
        };
        assert_eq!(
            create_grant(&emulator, &row).await.status,
            200,
            "case {name}: the create must be served"
        );
    }

    for (case, filter, expected) in [
        (
            "one tag key",
            vec![("TagKeys.TagKey.1", "owner")],
            vec!["grant-owner-admin", "grant-owner-admin-test"],
        ),
        (
            "one tag value",
            vec![("TagValues.TagValue.1", "production")],
            vec!["grant-environment-prod"],
        ),
        (
            "two tag keys",
            vec![
                ("TagKeys.TagKey.1", "owner"),
                ("TagKeys.TagKey.2", "environment"),
            ],
            vec![
                "grant-owner-admin",
                "grant-owner-admin-test",
                "grant-environment-prod",
            ],
        ),
        (
            "a key no grant carries",
            vec![("TagKeys.TagKey.1", "absent")],
            vec![],
        ),
        (
            "no filter at all",
            vec![],
            vec![
                "grant-owner-admin",
                "grant-owner-admin-test",
                "grant-environment-prod",
                "grant-untagged",
            ],
        ),
    ] {
        let answer = emulator.call("DescribeSnapshotCopyGrants", &filter).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        assert_eq!(
            listed_grants(&answer.body),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            listed_grants(&answer.body)
        );
    }

    // A selected grant reports EVERY tag it carries, not only the tag that
    // matched.
    let answer = emulator
        .call(
            "DescribeSnapshotCopyGrants",
            &[
                ("SnapshotCopyGrantName", "grant-owner-admin-test"),
                ("TagKeys.TagKey.1", "owner"),
            ],
        )
        .await;
    assert_eq!(
        listed_tags(&answer.body),
        vec![
            ("owner".to_string(), "admin".to_string()),
            ("environment".to_string(), "test".to_string()),
        ],
        "case a matched grant: expected every tag it carries, got {:?}",
        listed_tags(&answer.body)
    );
}

/// `DescribeSnapshotCopyGrants` hands out one page of the service's documented
/// `MaxRecords` window at a time, with a `Marker` cursor a later call resumes the
/// same sequence from.
///
/// The window is the one every paginated `Describe*` of this service shares
/// (`state::MAX_RECORDS_MIN` ..= `state::MAX_RECORDS_MAX`, applied by
/// `state::page_bounds`), tested on each bound and one past it.
#[tokio::test]
async fn grants_are_paginated_over_the_documented_window() {
    let emulator = Emulator::new();

    let minimum = state::MAX_RECORDS_MIN;
    let maximum = state::MAX_RECORDS_MAX;
    let total = minimum as usize + 1;
    for ordinal in 0..total {
        let name = format!("grant-page-{ordinal:03}");
        let row = simple_case("a paged grant", &name);
        assert_eq!(
            create_grant(&emulator, &row).await.status,
            200,
            "case {name}: the create must be served"
        );
    }

    for (case, max_records, accepted) in [
        ("one below the window", (minimum - 1).to_string(), false),
        ("at the window's minimum", minimum.to_string(), true),
        ("inside the window", ((minimum + maximum) / 2).to_string(), true),
        ("at the window's maximum", maximum.to_string(), true),
        ("one past the window, which is served", (maximum + 1).to_string(), true),
        ("zero", "0".to_string(), false),
    ] {
        let answer = call_with(
            &emulator,
            "DescribeSnapshotCopyGrants",
            &[("MaxRecords".to_string(), max_records.clone())],
        )
        .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected MaxRecords={max_records} served with 200, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue");
        }
    }

    // The default window takes every grant in one page, with no cursor left.
    let unpaged = emulator.call("DescribeSnapshotCopyGrants", &[]).await;
    assert_eq!(
        listed_grants(&unpaged.body).len(),
        total,
        "case the default window: expected all {total} grants in one page, got {:?}",
        listed_grants(&unpaged.body)
    );
    assert_eq!(
        unpaged.text("Marker"),
        None,
        "case the default window: expected no cursor, got {:?}",
        unpaged.text("Marker")
    );

    // A page at the window's minimum leaves a cursor the next call resumes from,
    // and the two pages are the unpaged sequence split in order.
    let page_size = minimum.to_string();
    let first_page = call_with(
        &emulator,
        "DescribeSnapshotCopyGrants",
        &[("MaxRecords".to_string(), page_size.clone())],
    )
    .await;
    let first_names = listed_grants(&first_page.body);
    assert_eq!(
        first_names.len(),
        minimum as usize,
        "case the first page: expected {minimum} grants, got {first_names:?}"
    );
    let marker = first_page
        .text("Marker")
        .expect("case the first page: records remain, so the page carries a cursor");
    let second_page = call_with(
        &emulator,
        "DescribeSnapshotCopyGrants",
        &[
            ("MaxRecords".to_string(), page_size.clone()),
            ("Marker".to_string(), marker),
        ],
    )
    .await;
    let second_names = listed_grants(&second_page.body);
    assert_eq!(
        second_names.len(),
        total - minimum as usize,
        "case the second page: expected the remaining {} grants, got {second_names:?}",
        total - minimum as usize
    );
    assert_eq!(
        second_page.text("Marker"),
        None,
        "case the second page: nothing remains, so expected no cursor, got {:?}",
        second_page.text("Marker")
    );
    let walked: Vec<String> = first_names.into_iter().chain(second_names).collect();
    assert_eq!(
        walked,
        listed_grants(&unpaged.body),
        "case the walked pages: expected the unpaged sequence, got {walked:?}"
    );

    // A cursor this service did not hand out is refused rather than served as a
    // mid-page offset.
    for (case, cursor) in [
        ("not a cursor at all", "nonsense"),
        ("a bare offset", "20"),
        ("an offset with the wrong tag", "20.deadbeef"),
    ] {
        let bogus = emulator
            .call("DescribeSnapshotCopyGrants", &[("Marker", cursor)])
            .await;
        assert_refused(case, &bogus, 400, "InvalidParameterValue");
    }
}

/// A create carrying more tags than one resource may hold, or a tag the service
/// reserves for itself, is refused and stores nothing.
#[tokio::test]
async fn refused_tags_store_no_grant() {
    let emulator = Emulator::new();

    let quota = state::MAX_TAGS_PER_RESOURCE;
    let at_quota: Vec<(String, String)> = (0..quota)
        .map(|ordinal| (format!("key-{ordinal}"), format!("value-{ordinal}")))
        .collect();
    let past_quota: Vec<(String, String)> = (0..quota + 1)
        .map(|ordinal| (format!("key-{ordinal}"), format!("value-{ordinal}")))
        .collect();

    /// One row of the tag table: the name the create gives the grant, the tags it
    /// carries and the error code it must answer (`None` for a served create).
    struct TagCase {
        case: &'static str,
        name: &'static str,
        tags: Vec<(String, String)>,
        refusal: Option<&'static str>,
    }

    let rows: Vec<TagCase> = vec![
        TagCase {
            case: "at the documented tag limit",
            name: "grant-tags-at-quota",
            tags: at_quota,
            refusal: None,
        },
        TagCase {
            case: "one tag past the limit",
            name: "grant-tags-past-quota",
            tags: past_quota,
            refusal: Some("TagLimitExceededFault"),
        },
        TagCase {
            case: "a reserved tag key",
            name: "grant-tags-reserved-key",
            tags: vec![("aws:owner".to_string(), "admin".to_string())],
            refusal: Some("InvalidTagFault"),
        },
        TagCase {
            case: "a reserved tag value",
            name: "grant-tags-reserved-value",
            tags: vec![("owner".to_string(), "aws:admin".to_string())],
            refusal: Some("InvalidTagFault"),
        },
        TagCase {
            case: "a tag key past its length limit",
            name: "grant-tags-long-key",
            tags: vec![("k".repeat(state::TAG_KEY_MAX_LEN + 1), "admin".to_string())],
            refusal: Some("InvalidTagFault"),
        },
        TagCase {
            case: "a tag value past its length limit",
            name: "grant-tags-long-value",
            tags: vec![(
                "owner".to_string(),
                "v".repeat(state::TAG_VALUE_MAX_LEN + 1),
            )],
            refusal: Some("InvalidTagFault"),
        },
    ];

    let mut stored: Vec<String> = Vec::new();
    for row in &rows {
        let case = row.case;
        let mut params: Vec<(String, String)> = vec![(
            "SnapshotCopyGrantName".to_string(),
            row.name.to_string(),
        )];
        for (position, (key, value)) in row.tags.iter().enumerate() {
            params.push((format!("Tags.Tag.{}.Key", position + 1), key.clone()));
            params.push((format!("Tags.Tag.{}.Value", position + 1), value.clone()));
        }
        let answer = call_with(&emulator, "CreateSnapshotCopyGrant", &params).await;
        match row.refusal {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200, got {} with body {}",
                    answer.status, answer.body
                );
                stored.push(row.name.to_string());
            }
            Some(code) => assert_refused(case, &answer, 400, code),
        }
    }

    // Only the admitted row is stored: every refused create stored nothing.
    let listed = emulator.call("DescribeSnapshotCopyGrants", &[]).await;
    assert_eq!(
        listed_grants(&listed.body),
        stored,
        "case the whole account: expected only {stored:?}, got {:?}",
        listed_grants(&listed.body)
    );
}

/// A delete removes the grant from the store, so every later read reports it
/// absent while every grant it did not name is left exactly where it was.
///
/// The rows differ along everything the rule generalises over: the form and size
/// of the name deleted, the form of the key the grant named, how many tags it
/// carried, and its position in the account's sequence (first, middle, last) —
/// and the deletes are taken out of creation order, so the order the surviving
/// records keep is the creation order and not an artefact of the walk.
#[tokio::test]
async fn a_delete_removes_the_grant_from_every_later_read() {
    let emulator = Emulator::new();

    let rows = vec![
        simple_case("the minimal grant", "grant-del-one"),
        CreateCase {
            case: "a bare key id and one tag",
            name: "grant-del-2-west-01".to_string(),
            kms_key_id: Some("1234abcd-12ab-34cd-56ef-1234567890ab".to_string()),
            tags: vec![("owner", "admin")],
        },
        CreateCase {
            case: "an alias and two tags",
            name: "grant-del-alias".to_string(),
            kms_key_id: Some("alias/copy-grants".to_string()),
            tags: vec![("owner", "admin"), ("environment", "test")],
        },
        CreateCase {
            case: "a name at the documented maximum length",
            name: name_of_length(state::SNAPSHOT_COPY_GRANT_NAME_RULE.max_len),
            kms_key_id: None,
            tags: Vec::new(),
        },
        CreateCase {
            case: "a one-character name",
            name: "d".to_string(),
            kms_key_id: Some("alias/one-character".to_string()),
            tags: vec![("environment", "production")],
        },
    ];

    for row in &rows {
        assert_eq!(
            create_grant(&emulator, row).await.status,
            200,
            "case {}: the create must be served",
            row.case
        );
    }

    // Out of creation order: a middle record, then the last, then the first,
    // then what is left.
    let order = [2usize, 4, 0, 1, 3];
    let mut remaining: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    for ordinal in order {
        let row = &rows[ordinal];
        let case = row.case;
        let name = row.name.as_str();

        let deleted = delete_one(&emulator, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected the delete of {name:?} served with 200, got {} with body {}",
            deleted.status, deleted.body
        );

        // The deletion is observable: the read of the name is refused, the
        // store holds no record for it, and a second delete of the same name
        // has nothing left to remove.
        assert_refused(
            case,
            &describe_one(&emulator, name).await,
            400,
            "SnapshotCopyGrantNotFoundFault",
        );
        let stored = stored_key(&emulator, name);
        assert_eq!(
            stored, None,
            "case {case}: expected the store to hold no record for {name:?}, got {stored:?}"
        );
        assert_refused(
            case,
            &delete_one(&emulator, name).await,
            400,
            "SnapshotCopyGrantNotFoundFault",
        );

        // Every grant the delete did not name is untouched, in creation order.
        remaining.retain(|held| held != name);
        let listed = emulator.call("DescribeSnapshotCopyGrants", &[]).await;
        assert_eq!(
            listed_grants(&listed.body),
            remaining,
            "case {case}: expected the remaining grants {remaining:?}, got {:?}",
            listed_grants(&listed.body)
        );
    }

    // A name freed by a delete can be created again, carrying whatever the new
    // request configures: the delete released the identity with the record.
    let reused = CreateCase {
        case: "a name freed by a delete",
        name: rows[0].name.clone(),
        kms_key_id: Some("alias/after-delete".to_string()),
        tags: vec![("owner", "fleet")],
    };
    assert_eq!(
        create_grant(&emulator, &reused).await.status,
        200,
        "case a name freed by a delete: expected the create served"
    );
    assert_eq!(
        stored_key(&emulator, &reused.name),
        Some("alias/after-delete".to_string()),
        "case a name freed by a delete: expected the new key stored, got {:?}",
        stored_key(&emulator, &reused.name)
    );
}

/// A `SnapshotCopyGrantName` that identifies no grant is refused by the delete,
/// which removes nothing.
#[tokio::test]
async fn deleting_a_name_that_identifies_no_grant_is_refused() {
    let emulator = Emulator::new();

    // On an empty account every name identifies nothing.
    for (case, name) in [
        ("an empty account", "grant-absent"),
        ("a name with separators", "grant-absent-2"),
    ] {
        let answer = delete_one(&emulator, name).await;
        assert_refused(case, &answer, 400, "SnapshotCopyGrantNotFoundFault");
        let message = answer.text("Message").unwrap_or_default();
        assert!(
            message.contains(name),
            "case {case}: expected the message to name {name:?}, got {message:?}"
        );
    }

    let present = simple_case("a stored grant", "grant-kept");
    assert_eq!(
        create_grant(&emulator, &present).await.status,
        200,
        "case a stored grant: the create must be served"
    );

    // One stored grant does not make a different name resolve, and the match is
    // exact: a prefix, a neighbouring name, a name differing only in case and a
    // name outside the documented syntax all identify no record.
    for (case, name) in [
        ("a neighbouring name", "grant-kept-2"),
        ("a prefix of the stored name", "grant-ke"),
        ("the stored name in another case", "GRANT-KEPT"),
        ("a name outside the documented syntax", "grant_kept"),
    ] {
        let answer = delete_one(&emulator, name).await;
        assert_refused(case, &answer, 400, "SnapshotCopyGrantNotFoundFault");
    }

    // Every refused delete removed nothing.
    let listed = emulator.call("DescribeSnapshotCopyGrants", &[]).await;
    assert_eq!(
        listed_grants(&listed.body),
        vec![present.name.clone()],
        "case the whole account: expected the stored grant to survive, got {:?}",
        listed_grants(&listed.body)
    );
}

/// A grant a cluster's snapshot copy is configured with is still what Amazon
/// Redshift encrypts that cluster's copied snapshots with, so the delete is
/// refused and the record stays readable; a grant no cluster uses is removed.
///
/// The rows differ along how many clusters hold the grant — none, one, and more
/// than one, "used by one or more clusters" — and one row has a cluster copying
/// under ANOTHER grant, so the refusal follows the grant a cluster names and not
/// the mere presence of a cluster.
///
/// The configuration is seeded straight into the same shared store the handlers
/// read, so this suite exercises the delete rule over several clusters without
/// driving `EnableSnapshotCopy` (which [`crate::snapshot_copy_configuration`]
/// covers end to end) for each of them.
#[tokio::test]
async fn a_grant_a_cluster_copies_under_cannot_be_deleted() {
    let emulator = Emulator::new();

    for (case, name, clusters) in [
        ("no cluster uses it", "grant-used-by-none", &[][..]),
        ("one cluster uses it", "grant-used-by-one", &["copy-cluster-a"][..]),
        (
            "two clusters use it",
            "grant-used-by-two",
            &["copy-cluster-b", "copy-cluster-c"][..],
        ),
    ] {
        assert_eq!(
            create_grant(&emulator, &simple_case(case, name)).await.status,
            200,
            "case {case}: the create must be served"
        );
        for cluster in clusters {
            create_cluster(&emulator, case, cluster).await;
            configure_snapshot_copy(&emulator, cluster, name);
        }

        let answer = delete_one(&emulator, name).await;
        if clusters.is_empty() {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the delete served, got {} with body {}",
                answer.status, answer.body
            );
            assert_refused(
                case,
                &describe_one(&emulator, name).await,
                400,
                "SnapshotCopyGrantNotFoundFault",
            );
            continue;
        }

        assert_refused(case, &answer, 400, "InvalidSnapshotCopyGrantStateFault");
        let message = answer.text("Message").unwrap_or_default();
        for cluster in clusters {
            assert!(
                message.contains(cluster),
                "case {case}: expected the message to name the cluster {cluster}, got {message:?}"
            );
        }

        // The refused delete removed nothing: the grant is still the record a
        // read reports.
        let read = describe_one(&emulator, name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the grant to still be readable, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            listed_grants(&read.body),
            vec![name.to_string()],
            "case {case}: expected the read to report {name}, got {:?}",
            listed_grants(&read.body)
        );
    }

    // A cluster copying under ANOTHER grant does not hold this one: the delete
    // is served and the other grant is untouched.
    let case = "a cluster copying under another grant";
    for name in ["grant-other-held", "grant-other-free"] {
        assert_eq!(
            create_grant(&emulator, &simple_case(case, name)).await.status,
            200,
            "case {case}: the create of {name} must be served"
        );
    }
    create_cluster(&emulator, case, "copy-cluster-d").await;
    configure_snapshot_copy(&emulator, "copy-cluster-d", "grant-other-held");
    let answer = delete_one(&emulator, "grant-other-free").await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the delete of the unheld grant served, got {} with body {}",
        answer.status, answer.body
    );
    let read = describe_one(&emulator, "grant-other-held").await;
    assert_eq!(
        listed_grants(&read.body),
        vec!["grant-other-held".to_string()],
        "case {case}: expected the held grant untouched, got {:?}",
        listed_grants(&read.body)
    );
}

/// Create one cluster, the minimum a `CreateCluster` request carries.
async fn create_cluster(emulator: &Emulator, case: &str, identifier: &str) {
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case}: the cluster {identifier} must be created, got {} with body {}",
        created.status, created.body
    );
}

/// Record on a stored cluster that its cross-region snapshot copy encrypts
/// copied snapshots under the snapshot copy grant `grant` names, which is what
/// `EnableSnapshotCopy` writes to a cluster's `ClusterSnapshotCopyStatus`.
fn configure_snapshot_copy(emulator: &Emulator, cluster: &str, grant: &str) {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    let position = store
        .cluster_position(cluster)
        .unwrap_or_else(|| panic!("the created cluster {cluster}"));
    store.clusters[position]
        .cluster
        .cluster_snapshot_copy_status = Some(ClusterSnapshotCopyStatus {
        destination_region: Some("us-east-1".to_string()),
        retention_period: Some(7),
        manual_snapshot_retention_period: Some(-1),
        snapshot_copy_grant_name: Some(grant.to_string()),
    });
}
