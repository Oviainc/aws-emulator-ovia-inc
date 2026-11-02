// SPDX-License-Identifier: CC0-1.0

//! The ORDER in which this service reads a caller-supplied value — its FORM first, the store
//! second — driven over the wire the service actually speaks, `ec2Query`, through the crate's
//! shared [`super::ec2_query`] harness.
//!
//! The rule under test: an operation that takes a resource identifier, or a parameter whose
//! format AWS publishes, reads that value's SYNTAX before anything is looked up — so a value
//! outside the form its kind is issued or published in is refused HTTP 400 with that kind's
//! malformed / invalid-request code, and only a well-formed value the account resolves
//! nothing for is answered with the kind's own unresolvable code. A client branches on the
//! code and the two halves mean different things: the first is a permanent fault of the
//! request, while the second reads as "the resource is not there" — which a cleanup path
//! routinely treats as already done — or, for a storage location, as a reachability problem
//! to chase permissions for.
//!
//! What is this suite's rather than the per-operation suites': the ORDER itself, observed
//! rather than assumed, ACROSS every operation that reads such a value — the cancel of an
//! instance export task, the deletes of an Amazon FPGA Image, an AMI usage report and a
//! secondary network, and the restore task that reads an Amazon S3 bucket name. The malformed
//! value is PLANTED as the key of a record of its own kind, one a lookup WOULD resolve, and
//! the answer must still be the malformed refusal while the planted record is left exactly as
//! it was. Each kind's own full table of departures from its form, and the wording of each
//! refusal, belong to that kind's suite ([`super::fpga_image_id_form`],
//! [`super::image_usage_reports`], [`super::secondary_networks`],
//! [`super::create_restore_image_task`], [`super::malformed_id_message_template`]); the rows
//! here are two departures per kind, differing in which way the value leaves the form, plus
//! the whole `false` half of the shared Amazon S3 naming catalogue
//! [`super::fpga_image_tasks_and_volume_io::bucket_name_rows`]. Every row is cross-checked
//! against the crate's OWN reader of the form ([`is_well_formed`]), so a row claims a side of
//! it only when the service's reader agrees, while every value this suite ASSERTS — each
//! error code and each state word — is written out here as the recorded response and the
//! modelled enum give it, never read back out of the implementation.

use std::time::Instant;

use ec2::ops::cancel_export_task::export_task_id_is_well_formed;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, fpga_image_id_is_well_formed,
    image_usage_report_id_is_well_formed, s3_bucket_name_is_well_formed,
    secondary_network_id_is_well_formed,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, emulator, error_code, indexed};
use super::fpga_image_tasks_and_volume_io::{
    bucket_name_rows, seed_export_task, seed_fpga_image, seed_image_usage_report,
};
use super::secondary_subnets::create_network;
use super::volume_lifecycle::{REGION, p};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The HTTP status every refusal in this suite carries: a fault of the REQUEST.
const REFUSAL_STATUS: u16 = 400;

/// The status a call that is served answers with.
const SUCCESS_STATUS: u16 = 200;

// The codes below are the recorded responses' own, spelled out here rather than imported from
// the service, so a change to one of its constants fails this suite instead of following it.

/// What a value EC2 cannot read as a resource id of ANY kind it resolves is refused with — the
/// malformed half of an export task id, which carries no kind-specific code.
const CODE_EXPORT_TASK_MALFORMED: &str = "InvalidID";

/// The malformed half of an AFI id.
const CODE_FPGA_IMAGE_MALFORMED: &str = "InvalidFpgaImageID.Malformed";

/// The malformed half of an AMI usage report id.
const CODE_IMAGE_USAGE_REPORT_MALFORMED: &str = "InvalidImageUsageReportId.Malformed";

/// The malformed half of a secondary network id.
const CODE_SECONDARY_NETWORK_MALFORMED: &str = "InvalidSecondaryNetworkId.Malformed";

/// What a request invalid in a way no member-specific code covers is refused with — what a
/// bucket name outside Amazon S3's naming rules is answered.
const CODE_BUCKET_MALFORMED: &str = "InvalidRequest";

/// The unresolvable half of an export task id.
const CODE_EXPORT_TASK_NOT_FOUND: &str = "InvalidExportTaskID.NotFound";

/// The unresolvable half of an AFI id.
const CODE_FPGA_IMAGE_NOT_FOUND: &str = "InvalidFpgaImageID.NotFound";

/// The unresolvable half of an AMI usage report id.
const CODE_IMAGE_USAGE_REPORT_NOT_FOUND: &str = "InvalidImageUsageReportId.NotFound";

/// The unresolvable half of a secondary network id.
const CODE_SECONDARY_NETWORK_NOT_FOUND: &str = "InvalidSecondaryNetworkId.NotFound";

/// What a bucket name that COULD be a bucket is answered, there being no Amazon S3 object for
/// any location to name: the location could not be read.
const CODE_INACCESSIBLE_LOCATION: &str = "InaccessibleStorageLocation";

/// An export task that is still running: the `ExportTaskState` value
/// (`smithy_query(query="shape",shape="ExportTaskState")`) a task a cancel may be asked for
/// carries.
const EXPORT_TASK_ACTIVE: &str = "active";

/// An export task a cancel has started taking down: the non-terminal `ExportTaskState` value
/// the clock owes `cancelled`.
const EXPORT_TASK_CANCELLING: &str = "cancelling";

/// A secondary network a delete has started taking down: the `SecondaryNetworkState` value
/// (`smithy_query(query="shape",shape="SecondaryNetworkState")`) it reports until the clock
/// finishes the deletion.
const SECONDARY_NETWORK_DELETING: &str = "delete-in-progress";

/// The object key the restore rows name in the bucket they name. The rule under test is about
/// the BUCKET NAME, so every one of those rows carries the same well-formed key.
const STORED_OBJECT_KEY: &str = "ami/stored.bin";

/// The AMI a seeded usage report names: a well-formed AMI id, so nothing about a seeded report
/// is itself malformed.
const REPORTED_IMAGE_ID: &str = "ami-0123456789abcdef0";

/// What a planting path expects of the helper it just called.
const EXPECT_STORED: &str = "the record just stored is in the store";

/// The five calls this rule is read through: each takes ONE caller-supplied value whose form
/// it reads before anything is looked up.
#[derive(Clone, Copy, Debug)]
enum Call {
    /// `CancelExportTask`, reading `ExportTaskId`.
    CancelExportTask,
    /// `DeleteFpgaImage`, reading `FpgaImageId`.
    DeleteFpgaImage,
    /// `DeleteImageUsageReport`, reading `ReportId`.
    DeleteImageUsageReport,
    /// `DeleteSecondaryNetwork`, reading `SecondaryNetworkId`.
    DeleteSecondaryNetwork,
    /// `CreateRestoreImageTask`, reading the Amazon S3 `Bucket` the stored AMI object is in.
    CreateRestoreImageTask,
}

impl Call {
    /// The ec2Query `Action` this call is made with.
    fn action(self) -> &'static str {
        match self {
            Self::CancelExportTask => "CancelExportTask",
            Self::DeleteFpgaImage => "DeleteFpgaImage",
            Self::DeleteImageUsageReport => "DeleteImageUsageReport",
            Self::DeleteSecondaryNetwork => "DeleteSecondaryNetwork",
            Self::CreateRestoreImageTask => "CreateRestoreImageTask",
        }
    }

    /// This call's parameters, with `value` in the member whose form is under test.
    fn params(self, value: &str) -> Params {
        match self {
            Self::CancelExportTask => vec![p("ExportTaskId", value)],
            Self::DeleteFpgaImage => vec![p("FpgaImageId", value)],
            Self::DeleteImageUsageReport => vec![p("ReportId", value)],
            Self::DeleteSecondaryNetwork => vec![p("SecondaryNetworkId", value)],
            Self::CreateRestoreImageTask => {
                vec![p("Bucket", value), p("ObjectKey", STORED_OBJECT_KEY)]
            }
        }
    }

    /// The code a value OUTSIDE this member's form is refused with, before any lookup.
    fn malformed_code(self) -> &'static str {
        match self {
            Self::CancelExportTask => CODE_EXPORT_TASK_MALFORMED,
            Self::DeleteFpgaImage => CODE_FPGA_IMAGE_MALFORMED,
            Self::DeleteImageUsageReport => CODE_IMAGE_USAGE_REPORT_MALFORMED,
            Self::DeleteSecondaryNetwork => CODE_SECONDARY_NETWORK_MALFORMED,
            Self::CreateRestoreImageTask => CODE_BUCKET_MALFORMED,
        }
    }

    /// The code a value INSIDE that form which resolves to nothing is refused with: the
    /// kind's own "no such resource" code, or — for a storage location, which names no
    /// resource of this account — the one answer for a location that could not be read.
    fn unresolvable_code(self) -> &'static str {
        match self {
            Self::CancelExportTask => CODE_EXPORT_TASK_NOT_FOUND,
            Self::DeleteFpgaImage => CODE_FPGA_IMAGE_NOT_FOUND,
            Self::DeleteImageUsageReport => CODE_IMAGE_USAGE_REPORT_NOT_FOUND,
            Self::DeleteSecondaryNetwork => CODE_SECONDARY_NETWORK_NOT_FOUND,
            Self::CreateRestoreImageTask => CODE_INACCESSIBLE_LOCATION,
        }
    }

    /// Make this call with `value`, signed for the Region the suite works in. Nothing in the
    /// rule depends on that Region: the value is read for its form before anything regional
    /// is resolved.
    async fn invoke(self, router: &MultiServiceRouter, value: &str) -> (u16, String) {
        super::ec2_query::call(router, REGION, self.action(), &self.params(value)).await
    }
}

/// Whether `value` is inside the form the call's member is read in, according to the CRATE'S
/// OWN reader of that form — the one the handler asks — so every row of the tables below is
/// cross-checked against the service rather than against this file's reading of it.
fn is_well_formed(call: Call, value: &str) -> bool {
    match call {
        Call::CancelExportTask => export_task_id_is_well_formed(value),
        Call::DeleteFpgaImage => fpga_image_id_is_well_formed(value),
        Call::DeleteImageUsageReport => image_usage_report_id_is_well_formed(value),
        Call::DeleteSecondaryNetwork => secondary_network_id_is_well_formed(value),
        Call::CreateRestoreImageTask => s3_bucket_name_is_well_formed(value),
    }
}

/// The values a call is driven with that are OUTSIDE its member's form: two departures per id
/// kind — the suffix the mint can never issue, and a value whose shape is not that kind's at
/// all — plus the `false` half of the shared Amazon S3 naming catalogue for the bucket name.
/// The kinds' fuller tables live in their own suites (see the module docs).
fn malformed_rows() -> Vec<(Call, String, String)> {
    let mut rows: Vec<(Call, String, String)> = vec![
        // An export task id's kind segment is part of its form: `export-i-` and
        // `export-ami-` are the two prefixes the minting operations use.
        (Call::CancelExportTask, "no kind segment", "export-00000000"),
        (
            Call::CancelExportTask,
            "a digit outside lowercase hexadecimal",
            "export-ami-0123456789abcdefg",
        ),
        // The null (all-zero) id is outside the AFI, report and secondary network forms
        // because the mint counts from one, so no resource can ever hold it.
        (
            Call::DeleteFpgaImage,
            "the null id in the long form",
            "afi-00000000000000000",
        ),
        (
            Call::DeleteFpgaImage,
            "an upper-case suffix",
            "afi-0123456789ABCDEF0",
        ),
        (
            Call::DeleteImageUsageReport,
            "the null id in the long form",
            "imgusgrpt-00000000000000000",
        ),
        (
            Call::DeleteImageUsageReport,
            "another resource kind's prefix",
            "report-0123456789abcdef0",
        ),
        (
            Call::DeleteSecondaryNetwork,
            "the null id in the long form",
            "secnet-00000000000000000",
        ),
        (
            Call::DeleteSecondaryNetwork,
            "the short form, which this kind was never issued in",
            "secnet-0123abcd",
        ),
    ]
    .into_iter()
    .map(|(call, case, value)| (call, case.to_string(), value.to_string()))
    .collect();
    rows.extend(
        bucket_name_rows()
            .into_iter()
            .filter(|(_, _, well_formed)| !well_formed)
            .map(|(case, value, _)| (Call::CreateRestoreImageTask, case.to_string(), value)),
    );
    rows
}

/// The values a call is driven with that are INSIDE its member's form and that the account
/// resolves nothing for: one id per kind, and the `true` half of the shared Amazon S3 naming
/// catalogue for the bucket name.
fn well_formed_rows() -> Vec<(Call, String, String)> {
    let mut rows: Vec<(Call, String, String)> = vec![
        (
            Call::CancelExportTask,
            "an instance export task id in the long form",
            "export-i-0123456789abcdef0",
        ),
        (
            Call::DeleteFpgaImage,
            "an AFI id in the short form",
            "afi-0123abcd",
        ),
        (
            Call::DeleteImageUsageReport,
            "a report id in the long form",
            "imgusgrpt-0123456789abcdef0",
        ),
        (
            Call::DeleteSecondaryNetwork,
            "a secondary network id in the long form",
            "secnet-0123456789abcdef0",
        ),
    ]
    .into_iter()
    .map(|(call, case, value)| (call, case.to_string(), value.to_string()))
    .collect();
    rows.extend(
        bucket_name_rows()
            .into_iter()
            .filter(|(_, _, well_formed)| *well_formed)
            .map(|(case, value, _)| (Call::CreateRestoreImageTask, case.to_string(), value)),
    );
    rows
}

// ---------------------------------------------------------------------------
// Planting: the record a lookup WOULD have resolved
// ---------------------------------------------------------------------------

/// Store one secondary network through the shared seeding helper
/// ([`super::secondary_subnets::create_network`], which writes it through the state writer
/// that derives every member the service owns) and answer the id it was given.
///
/// That helper names its own Region in the network's ARN; nothing here reads it — a secondary
/// network is resolved by id alone, as `DeleteSecondaryNetwork` does.
fn insert_secondary_network(engine: &engine::InMemoryEngine) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    create_network(&mut guard, 0)
}

/// Plant, under the key `value`, a record of the kind the call resolves — one a lookup would
/// find. Answers whether anything was planted: a bucket name is no key of this store at all
/// (the restore reads an Amazon S3 object, and this emulator holds none), so that row's whole
/// assertion is the code the response carries.
///
/// Each kind is stored through the SHARED seeding path the other suites of this crate use and
/// the record is then copied under `value`, so a planted record is one the service itself
/// built and cannot drift from the records those suites seed.
fn plant(engine: &engine::InMemoryEngine, call: Call, value: &str) -> bool {
    match call {
        Call::CancelExportTask => {
            let minted = seed_export_task(engine, EXPORT_TASK_ACTIVE);
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            let record = guard.export_tasks.get(&minted).expect(EXPECT_STORED).clone();
            guard.export_tasks.insert(value.to_string(), record);
            true
        }
        Call::DeleteFpgaImage => {
            let (minted, _) = seed_fpga_image(engine, "planted-afi");
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            let record = guard.fpga_images.get(&minted).expect(EXPECT_STORED).clone();
            guard.fpga_images.insert(value.to_string(), record);
            true
        }
        Call::DeleteImageUsageReport => {
            let minted = seed_image_usage_report(engine, REPORTED_IMAGE_ID);
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            let record = guard
                .image_usage_reports
                .get(&minted)
                .expect(EXPECT_STORED)
                .clone();
            guard.image_usage_reports.insert(value.to_string(), record);
            true
        }
        Call::DeleteSecondaryNetwork => {
            let minted = insert_secondary_network(engine);
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            let record = guard
                .secondary_networks
                .get(&minted)
                .expect(EXPECT_STORED)
                .clone();
            guard.secondary_networks.insert(value.to_string(), record);
            true
        }
        Call::CreateRestoreImageTask => false,
    }
}

/// Whether the record stored under `value` is still exactly as it was stored: present, and
/// carrying the state a call that reached it would have moved.
fn record_is_untouched(engine: &engine::InMemoryEngine, call: Call, value: &str) -> bool {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    let now = Instant::now();
    match call {
        Call::CancelExportTask => guard.export_tasks.get(value).is_some_and(|record| {
            record.cancelled_at.is_none() && record.state_at(now) == EXPORT_TASK_ACTIVE
        }),
        Call::DeleteFpgaImage => guard.fpga_images.contains_key(value),
        Call::DeleteImageUsageReport => guard.image_usage_reports.contains_key(value),
        Call::DeleteSecondaryNetwork => guard
            .secondary_networks
            .get(value)
            .is_some_and(|record| record.delete_complete_at.is_none()),
        Call::CreateRestoreImageTask => true,
    }
}

/// Whether the call DID what it is for to the record `value` names: the export task carries
/// the instant its cancellation falls due and reports the state that cancel put it in, each
/// delete that removes its resource has removed it, and the secondary network — whose record
/// stays in the store, because a network being deleted is still reported — carries the
/// instant its deletion falls due.
///
/// The positive counterpart of [`record_is_untouched`], not its negation: "gone from the
/// store" and "still stored but moved" are different propositions, and which one a kind
/// answers with is a fact of that kind.
fn record_was_acted_on(engine: &engine::InMemoryEngine, call: Call, value: &str) -> bool {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    let now = Instant::now();
    match call {
        Call::CancelExportTask => guard.export_tasks.get(value).is_some_and(|record| {
            record.cancelled_at.is_some() && record.state_at(now) == EXPORT_TASK_CANCELLING
        }),
        Call::DeleteFpgaImage => !guard.fpga_images.contains_key(value),
        Call::DeleteImageUsageReport => !guard.image_usage_reports.contains_key(value),
        Call::DeleteSecondaryNetwork => guard
            .secondary_networks
            .get(value)
            .is_some_and(|record| record.delete_complete_at.is_some()),
        // Nothing of this store is named by a bucket name, so there is nothing to have acted
        // on: the restore is refused for the object it cannot read.
        Call::CreateRestoreImageTask => false,
    }
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// A value outside its member's form is refused with its kind's malformed code — and the
/// store is never reached: a record planted under that very value, which a lookup would have
/// resolved, is left as it was.
#[tokio::test]
async fn a_value_outside_its_form_is_refused_as_malformed_before_the_store_is_read() {
    let (router, engine) = emulator();

    for (call, case, value) in malformed_rows() {
        assert!(
            !is_well_formed(call, &value),
            "case {call:?}/{case:?} with {value:?}: the row claims to be outside the form, \
             but the service's own reader accepts it"
        );
        let planted = plant(&engine, call, &value);

        let (status, body) = call.invoke(&router, &value).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {call:?}/{case:?} with {value:?}: expected HTTP {REFUSAL_STATUS}, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            call.malformed_code(),
            "case {call:?}/{case:?} with {value:?}: expected {}, got {body}",
            call.malformed_code()
        );
        if planted {
            assert!(
                record_is_untouched(&engine, call, &value),
                "case {call:?}/{case:?} with {value:?}: the record planted under that value \
                 must be left exactly as it was, so the refusal came before the lookup"
            );
        }
    }
}

/// A value INSIDE its member's form that the account resolves nothing for is the OTHER
/// answer: the kind's own unresolvable code, never its malformed one.
#[tokio::test]
async fn a_well_formed_value_the_account_cannot_resolve_is_the_other_answer() {
    let (router, _engine) = emulator();

    for (call, case, value) in well_formed_rows() {
        assert!(
            is_well_formed(call, &value),
            "case {call:?}/{case:?} with {value:?}: the row claims to be inside the form, \
             but the service's own reader refuses it"
        );

        let (status, body) = call.invoke(&router, &value).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {call:?}/{case:?} with {value:?}: expected HTTP {REFUSAL_STATUS}, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            call.unresolvable_code(),
            "case {call:?}/{case:?} with {value:?}: expected {}, got {body}",
            call.unresolvable_code()
        );
    }
}

/// The refusals above are judgements about the VALUE, not an operation that refuses
/// everything: each call serves the well-formed value its store does hold, and what it did is
/// visible afterwards — the cancelled task reports the non-terminal state the cancel put it
/// in, and each delete's resource is gone from the store or carries the instant its deletion
/// falls due.
#[tokio::test]
async fn each_call_serves_the_well_formed_value_its_store_holds() {
    let (router, engine) = emulator();

    // The cancel: a stored ACTIVE task under a minted id moves into `cancelling`, which the
    // describe of that task reports.
    let export_task_id = seed_export_task(&engine, EXPORT_TASK_ACTIVE);
    let (status, body) = Call::CancelExportTask.invoke(&router, &export_task_id).await;
    assert_eq!(
        status, SUCCESS_STATUS,
        "cancelling the stored task {export_task_id:?}: expected HTTP {SUCCESS_STATUS}, \
         got {status} with body {body}"
    );
    let (status, described) = super::ec2_query::call(
        &router,
        REGION,
        "DescribeExportTasks",
        &indexed("ExportTaskId", &[export_task_id.as_str()]),
    )
    .await;
    assert_eq!(
        status, SUCCESS_STATUS,
        "describing the cancelled task {export_task_id:?}: expected HTTP {SUCCESS_STATUS}, \
         got {status} with body {described}"
    );
    assert_eq!(
        element(&described, "state").as_deref(),
        Some(EXPORT_TASK_CANCELLING),
        "describing the cancelled task {export_task_id:?}: expected the state \
         {EXPORT_TASK_CANCELLING:?}, got {described}"
    );
    assert!(
        record_was_acted_on(&engine, Call::CancelExportTask, &export_task_id),
        "cancelling the stored task {export_task_id:?}: the record must carry the instant \
         its cancellation falls due"
    );

    // The AFI delete: the image the id names is removed from the store.
    let (fpga_image_id, _) = seed_fpga_image(&engine, "served-afi");
    let (status, body) = Call::DeleteFpgaImage.invoke(&router, &fpga_image_id).await;
    assert_eq!(
        status, SUCCESS_STATUS,
        "deleting the stored AFI {fpga_image_id:?}: expected HTTP {SUCCESS_STATUS}, \
         got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "deleting the stored AFI {fpga_image_id:?}: expected a true return, got {body}"
    );
    assert!(
        record_was_acted_on(&engine, Call::DeleteFpgaImage, &fpga_image_id),
        "deleting the stored AFI {fpga_image_id:?}: the store must no longer hold it"
    );

    // The report delete: the report the id names is removed from the store.
    let report_id = seed_image_usage_report(&engine, REPORTED_IMAGE_ID);
    let (status, body) = Call::DeleteImageUsageReport.invoke(&router, &report_id).await;
    assert_eq!(
        status, SUCCESS_STATUS,
        "deleting the stored report {report_id:?}: expected HTTP {SUCCESS_STATUS}, \
         got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "deleting the stored report {report_id:?}: expected a true return, got {body}"
    );
    assert!(
        record_was_acted_on(&engine, Call::DeleteImageUsageReport, &report_id),
        "deleting the stored report {report_id:?}: the store must no longer hold it"
    );

    // The network delete: the network the id names reports the deletion the clock now owes
    // it, and the record keeps the due instant that delete gave it.
    let secondary_network_id = insert_secondary_network(&engine);
    let (status, body) = Call::DeleteSecondaryNetwork
        .invoke(&router, &secondary_network_id)
        .await;
    assert_eq!(
        status, SUCCESS_STATUS,
        "deleting the stored network {secondary_network_id:?}: expected HTTP \
         {SUCCESS_STATUS}, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(SECONDARY_NETWORK_DELETING),
        "deleting the stored network {secondary_network_id:?}: expected the state \
         {SECONDARY_NETWORK_DELETING:?}, got {body}"
    );
    assert!(
        record_was_acted_on(&engine, Call::DeleteSecondaryNetwork, &secondary_network_id),
        "deleting the stored network {secondary_network_id:?}: the record must carry the \
         instant its deletion falls due"
    );
}
