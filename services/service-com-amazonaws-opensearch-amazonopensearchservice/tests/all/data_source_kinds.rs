// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH data sources an attachment of an OpenSearch application
//! can complete against — `AttachDataSource`, `DescribeDataSourceAttachment` and
//! `ListDataSourceAttachments` — driven over the real wire path (a `restJson1` HTTP
//! request through the multi-service router into the mounted service), so what is
//! asserted is what a client sees.
//!
//! The rule under test: the ARN an attach names its data source by is resolved against
//! every kind of data source this account can hold, and an attachment of one the account
//! holds completes (ATTACHED) once both it and the application are active, while FAILED
//! is reserved for an attachment whose data source the account holds none of — so the
//! outcome follows whether the named resource exists, not which service's namespace its
//! ARN carries.
//!
//! The rows differ along what the rule generalises over: the kind of resource the ARN
//! names (an OpenSearch domain of this account, a direct query data source of it, this
//! account's AWS Glue Data Catalog), the Region it names, the account it names, and the
//! resource type inside one vendor's namespace — plus a domain that is still being
//! provisioned, where the attachment is owed its completion rather than denied it. Each
//! row is read back through both reads, so the attach, the single-attachment read and the
//! listing cannot report one record differently.
//!
//! The harness comes from [`super::domain_lifecycle`] and the attachment calls from
//! [`super::data_source_attachments`]; this suite adds no second copy of either.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    ATTACHMENT_STATUS_ATTACHED, ATTACHMENT_STATUS_FAILED, ATTACHMENT_STATUS_PENDING,
};

use super::data_source_attachments::{application, attach, describe_attachment, list_attachments};
use super::domain_lifecycle::{string_member, Emulator, PREFIX, REGION, SETTLE_WAIT};

/// An IAM role a direct query data source's union member names, inside
/// `DirectQueryDataSourceRoleArn`'s pattern
/// (`smithy_query(query="shape",shape="CloudWatchDirectQueryDataSource")`).
const DIRECT_QUERY_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/OpenSearchDirectQueryRole";

/// Another account of the same partition, so a row can name a resource of an account
/// this emulator does not serve.
const ANOTHER_ACCOUNT: &str = "210987654321";

/// Another Region of the same partition, so a row can name a resource outside the one
/// the request was signed for.
const ANOTHER_REGION: &str = "ap-southeast-2";

/// The ARN of an account's AWS Glue Data Catalog in one Region:
/// `arn:aws:glue:<region>:<account-id>:catalog`. AWS Glue's Data Catalog is named with no
/// resource id at all — there is exactly one per account and Region, and no API creates
/// or deletes it — so for the account this emulator serves it always names a resource
/// that exists.
fn glue_data_catalog_arn(region: &str, account: &str) -> String {
    format!("arn:aws:glue:{region}:{account}:catalog")
}

/// The ARN of a domain of this account, composed here from the partition, this service's
/// namespace, the Region the request was signed for and the account this emulator serves,
/// so an assertion does not ask the service to confirm its own answer.
fn domain_arn(name: &str) -> String {
    format!(
        "arn:aws:es:{REGION}:{}:domain/{name}",
        ::engine::emulator_account_id()
    )
}

/// `CreateDomain` for `name`, asserting it was accepted and answering with the ARN it is
/// addressed by.
async fn domain(emulator: &Emulator, name: &str) -> String {
    let answer = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body["DomainStatus"], "ARN", name)
}

/// `AddDirectQueryDataSource` for `name`, answering with the ARN it minted — a data
/// source of the ACCOUNT rather than of one domain.
async fn direct_query_data_source(emulator: &Emulator, name: &str) -> String {
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/directQueryDataSource"),
            Some(json!({
                "DataSourceName": name,
                "DataSourceType": { "CloudWatchLog": { "RoleArn": DIRECT_QUERY_ROLE_ARN } },
                "OpenSearchArns": [domain_arn("direct-query-reader")],
            })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case AddDirectQueryDataSource({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "DataSourceArn", name)
}

/// The status the attach of `data_source_arn` answered with.
async fn attached_status(emulator: &Emulator, id: &str, data_source_arn: &str, case: &str) -> String {
    let answer = attach(emulator, id, json!({ "dataSourceArn": data_source_arn })).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from AttachDataSource, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["dataSourceArn"],
        json!(data_source_arn),
        "case {case}: the attach reports the data source it was given, got {:?}",
        answer.body
    );
    string_member(&answer.body, "status", case)
}

/// The status the single-attachment READ reports for `data_source_arn`.
async fn read_status(emulator: &Emulator, id: &str, data_source_arn: &str, case: &str) -> String {
    let answer = describe_attachment(emulator, id, data_source_arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from DescribeDataSourceAttachment, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "status", case)
}

/// The status the LISTING reports for `data_source_arn`, so the set and the single read
/// are asserted to agree.
async fn listed_status(emulator: &Emulator, id: &str, data_source_arn: &str, case: &str) -> String {
    let answer = list_attachments(emulator, id, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from ListDataSourceAttachments, got {} with {:?}",
        answer.status, answer.body
    );
    let entries: Vec<Value> = answer.body["attachments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let found = entries
        .iter()
        .find(|entry| entry["dataSourceArn"] == json!(data_source_arn))
        .unwrap_or_else(|| {
            panic!("case {case}: expected the listing to report {data_source_arn}, got {entries:?}")
        });
    string_member(found, "status", case)
}

/// RULE: an attach of a data source this account HOLDS completes, whichever kind of
/// resource the ARN names, and one of a data source it holds none of is the non-retryable
/// FAILED — so the outcome follows whether the named resource exists for the account.
#[tokio::test]
async fn an_attachment_completes_for_every_data_source_kind_this_account_holds() {
    let emulator = Emulator::new();
    let id = application(&emulator, "dskinds").await;
    let held_domain = domain(&emulator, "ds-kinds-held").await;
    let held_direct_query = direct_query_data_source(&emulator, "ds_kinds_held").await;
    let account = ::engine::emulator_account_id();

    // The application and the domain have finished provisioning, so each row below is
    // decided by its own data source rather than by something else still coming up.
    tokio::time::sleep(SETTLE_WAIT).await;

    // Each row: the case, the data source ARN, the status the attach answers with, and the
    // status the attachment settles to once the clock has passed whatever it owed.
    let rows: Vec<(&str, String, &str, &str)> = vec![
        (
            "this account's Data Catalog in the Region the request was signed for",
            glue_data_catalog_arn(REGION, account),
            ATTACHMENT_STATUS_ATTACHED,
            ATTACHMENT_STATUS_ATTACHED,
        ),
        (
            "this account's Data Catalog in another Region, which it holds too",
            glue_data_catalog_arn(ANOTHER_REGION, account),
            ATTACHMENT_STATUS_ATTACHED,
            ATTACHMENT_STATUS_ATTACHED,
        ),
        (
            "a domain of this account that is already active",
            held_domain.clone(),
            ATTACHMENT_STATUS_ATTACHED,
            ATTACHMENT_STATUS_ATTACHED,
        ),
        (
            "a direct query data source of this account",
            held_direct_query.clone(),
            ATTACHMENT_STATUS_ATTACHED,
            ATTACHMENT_STATUS_ATTACHED,
        ),
        (
            "a Data Catalog of another account, which this one does not hold",
            glue_data_catalog_arn(REGION, ANOTHER_ACCOUNT),
            ATTACHMENT_STATUS_PENDING,
            ATTACHMENT_STATUS_FAILED,
        ),
        (
            "another resource type of the same vendor, which is no data source",
            format!("arn:aws:glue:{REGION}:{account}:database/ds-kinds-absent"),
            ATTACHMENT_STATUS_PENDING,
            ATTACHMENT_STATUS_FAILED,
        ),
        (
            "a Data Catalog ARN naming no Region, which names no catalog",
            format!("arn:aws:glue::{account}:catalog"),
            ATTACHMENT_STATUS_PENDING,
            ATTACHMENT_STATUS_FAILED,
        ),
        (
            "a domain of this account under another name",
            domain_arn("ds-kinds-absent"),
            ATTACHMENT_STATUS_PENDING,
            ATTACHMENT_STATUS_FAILED,
        ),
    ];

    for (case, data_source_arn, attached, _) in &rows {
        assert_eq!(
            attached_status(&emulator, &id, data_source_arn, case).await,
            *attached,
            "case {case}: expected the attach to answer {attached}"
        );
        assert_eq!(
            read_status(&emulator, &id, data_source_arn, case).await,
            *attached,
            "case {case}: the read reports the status the attach answered"
        );
    }

    // Nothing asks for the move: the span the clock owed each attachment passes, and every
    // one of them is then in the status its own data source decides.
    tokio::time::sleep(SETTLE_WAIT).await;
    for (case, data_source_arn, _, settled) in &rows {
        assert_eq!(
            listed_status(&emulator, &id, data_source_arn, case).await,
            *settled,
            "case {case}: expected the settled attachment to read {settled}"
        );
        assert_eq!(
            read_status(&emulator, &id, data_source_arn, case).await,
            *settled,
            "case {case}: the read and the listing report one record"
        );
    }
}

/// RULE: an attachment whose data source this account holds but which is not active yet
/// is owed its completion rather than failed — the attach answers PENDING and the clock
/// lands it as ATTACHED with nothing asking for the move.
#[tokio::test]
async fn an_attachment_of_a_data_source_that_is_not_active_yet_is_owed_its_completion() {
    let emulator = Emulator::new();
    let id = application(&emulator, "dspending").await;
    let account = ::engine::emulator_account_id();

    // Each row names a data source this account holds that the attachment cannot complete
    // against at the instant it is asked for, because one of the two is still coming up: a
    // domain still being provisioned, and the account's own Data Catalog — which is always
    // active — named while the APPLICATION is the one still coming up. Both attaches are
    // made before anything is waited for, so each row really is decided while neither side
    // is active.
    let provisioning = domain(&emulator, "ds-kinds-pending").await;
    let rows = [
        ("a domain still being provisioned", provisioning),
        (
            "the account's Data Catalog while the application is provisioning",
            glue_data_catalog_arn(REGION, account),
        ),
    ];
    for (case, data_source_arn) in &rows {
        assert_eq!(
            attached_status(&emulator, &id, data_source_arn, case).await,
            ATTACHMENT_STATUS_PENDING,
            "case {case}: expected the attach to answer {ATTACHMENT_STATUS_PENDING}"
        );
    }

    tokio::time::sleep(SETTLE_WAIT).await;
    for (case, data_source_arn) in &rows {
        assert_eq!(
            listed_status(&emulator, &id, data_source_arn, case).await,
            ATTACHMENT_STATUS_ATTACHED,
            "case {case}: expected the clock to land the attachment as \
             {ATTACHMENT_STATUS_ATTACHED}"
        );
        assert_eq!(
            read_status(&emulator, &id, data_source_arn, case).await,
            ATTACHMENT_STATUS_ATTACHED,
            "case {case}: the read and the listing report one record"
        );
    }
}
