// SPDX-License-Identifier: CC0-1.0

//! The upload content plane: the locator `CreateUpload` publishes as `Upload$url`.
//!
//! # Not a modelled operation
//!
//! `Upload$url` is documented as "the presigned Amazon S3 URL that was used to store a
//! file using a PUT request": the content of an upload is stored by a plain HTTP `PUT`
//! against the published URL, which carries no `X-Amz-Target`, names no modelled operation
//! and is followed with no credential of its own. Nothing in the generated routing table
//! can name it, so it is reached through the two seams an emulated data plane uses:
//!
//! * [`crate::service::EmulatedService::can_handle`] also claims a request whose path is a
//!   content locator of an upload this store holds — the workspace's reserved data-plane
//!   path namespace plus this plane's own segment, read back with
//!   [`crate::state::upload_content_resource_id`] — so the router does not answer
//!   `UnknownService` for a URL this service handed out;
//! * [`smithy_server_runtime::Service::override_routing_error`] serves it, and the router
//!   reaches that seam BEFORE authentication — which is what lets the published URL be
//!   followed unsigned, as a presigned URL is.
//!
//! # What is served
//!
//! `PUT <locator>` takes the request body as the content of the upload the path names and
//! puts that upload into processing: the content is read as the package format the
//! upload's TYPE names, and the outcome of that read becomes the upload's terminal status
//! once [`crate::state::UPLOAD_PROCESSING_SPAN`] has passed (see
//! [`crate::state::UploadRecord::begin_processing`]). An upload holds one content, so a
//! second `PUT` replaces the first and restarts its processing.
//!
//! What is READ is bounded: content past [`MAX_CONTENT_BYTES`] is refused rather than
//! read, only [`MAX_ARCHIVE_ENTRIES`] of an archive's entries are visited, and only
//! [`MAX_ENTRY_PREFIX_BYTES`] of the one entry a format requires are inflated — so a large
//! or hostile upload costs those bounds and nothing more. The bytes themselves are not
//! retained: the observable result of an upload is its processing outcome, and no Device
//! Farm operation reads the content back.
//!
//! Every other method on this path space is refused: the published URL describes a write,
//! and answering a read on it would serve content the URL grants no read of.
//!
//! # The error envelope
//!
//! This plane is plain HTTP rather than a modelled operation, so a refusal is answered in
//! its own small JSON envelope carrying the code and a reason — never in the service's
//! awsJson1_1 shapes, which belong to the modelled operations.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;

use crate::state::{ProcessingOutcome, UploadContentFormat};

/// The media type a refusal of this plane is stated in.
const ERROR_CONTENT_TYPE: &str = "application/json";

/// What this plane says to a method the published locator does not describe.
const METHOD_NOT_ALLOWED_MESSAGE: &str = "This upload is written with PUT";

/// What this plane says when the path names no upload this store holds — an upload this
/// emulator never minted, or one that has since been deleted. One sentence carrying
/// nothing from the request.
const NO_SUCH_UPLOAD_MESSAGE: &str = "The specified upload does not exist";

/// What this plane says when the body could not be read at all. The router buffers every
/// request before dispatch, so a body it cannot hand back in memory is a failure of the
/// server, never a caller's malformed upload.
const UNREADABLE_BODY_MESSAGE: &str = "The server could not read the uploaded content";

/// What this plane says when this service's store is not reachable. A code of this plane's
/// own, never one of the service's modelled error shapes: those belong to the modelled
/// operations.
const STORE_UNAVAILABLE_MESSAGE: &str = "The Device Farm store is unavailable";

/// How many bytes of content one store of an upload accepts.
///
/// The emulator's OWN bound, not a claim about any published limit: it is what keeps the
/// work of reading one archive's directory proportional to something fixed. The body has
/// already been materialised by the framework by the time this plane runs (see this
/// module's header), so what this refusal bounds is the PROCESSING, and what the store
/// holds is the outcome of that processing rather than the content — so no caller can grow
/// the state by uploading.
const MAX_CONTENT_BYTES: u64 = 64 * 1024 * 1024;

/// How many entries of one archive are visited while looking for the files the package
/// format requires: enough for a real app package's manifest to be found, bounded so a
/// hostile archive's entry count is not this process's problem.
const MAX_ARCHIVE_ENTRIES: usize = 4096;

/// What this plane says to content past [`MAX_CONTENT_BYTES`], naming the bound it is past
/// — `EntityTooLarge`, which is what a `PUT` of an object past a size limit is answered.
fn too_large() -> RtResponse {
    refusal(
        400,
        "EntityTooLarge",
        &format!("One store of an upload's content accepts at most {MAX_CONTENT_BYTES} bytes"),
    )
}

/// The length the request declares for the content, when it declares one this plane can
/// read as a byte count.
fn declared_length(req: &RtRequest) -> Option<u64> {
    req.headers()
        .get(::http::header::CONTENT_LENGTH.as_str())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

/// Serve one request to a published content locator.
///
/// `resource_id` is the upload resource id the path named; whether it still names an
/// upload is resolved against the store here, under one lock, so a locator whose upload
/// was deleted is refused rather than served.
pub fn serve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    resource_id: &str,
) -> RtResponse {
    if req.method() != "PUT" {
        return refusal(405, "MethodNotAllowed", METHOD_NOT_ALLOWED_MESSAGE);
    }
    // The DECLARED length is judged first, so content stating a size past the bound is
    // refused from the headers, before anything proportional to it runs.
    if declared_length(req).is_some_and(|declared| declared > MAX_CONTENT_BYTES) {
        return too_large();
    }
    let Some(body) = req.body().bytes() else {
        return refusal(500, "InternalFailure", UNREADABLE_BODY_MESSAGE);
    };
    // And the same bound on what actually ARRIVED, which is the authority: a declaration is
    // only a claim.
    if u64::try_from(body.len()).unwrap_or(u64::MAX) > MAX_CONTENT_BYTES {
        return too_large();
    }
    let Some(state) = crate::state::service_state(engine) else {
        return refusal(500, "InternalFailure", STORE_UNAVAILABLE_MESSAGE);
    };
    let mut store = state.lock();
    let Some(record) = store.upload_of_resource_id_mut(resource_id) else {
        return refusal(404, "NoSuchKey", NO_SUCH_UPLOAD_MESSAGE);
    };
    // The content is judged against the type the upload was REGISTERED under, which is the
    // only thing that says what the bytes are supposed to be.
    let outcome = processing_outcome(&record.r#type, body);
    record.begin_processing(outcome, ::std::time::Instant::now());
    stored()
}

/// What an upload whose content is not the package its type names reports: the step of the
/// processing that could not read it, the sentence on `Upload$message`, and the JSON
/// document on `Upload$metadata` a caller reads the failure out of.
///
/// Device Farm reports the failure of a processing step as an `errorCode` on the metadata
/// document, named for the upload's type and the step: the unpacking of the archive is
/// `<TYPE>_UNZIP_FAILED`, and the steps that read inside it carry their own tool's name
/// (an Android package's badging is read with `aapt debug badging`, so a package whose
/// badging cannot be read is `ANDROID_APP_AAPT_DEBUG_BADGING_FAILED`). The model fixes no
/// text for either member ("a message about the upload's result", "the upload's
/// metadata"), so the sentence states what the step required of the content.
fn failed(error_code: String, required: &str) -> ProcessingOutcome {
    let message = format!("We could not parse your file. Please make sure that it is {required}.");
    ProcessingOutcome {
        status: crate::state::UPLOAD_STATUS_FAILED,
        metadata: Some(
            ::serde_json::json!({ "errorCode": error_code, "errorMessage": message }).to_string(),
        ),
        message: Some(message),
    }
}

/// The `errorCode` of an upload of `upload_type` whose content could not be unpacked as an
/// archive at all.
fn unzip_failed(upload_type: &str) -> String {
    format!("{upload_type}_UNZIP_FAILED")
}

/// The `errorCode` of an Android application package whose badging — the manifest `aapt
/// debug badging` reads — could not be read.
const ANDROID_BADGING_FAILED: &str = "ANDROID_APP_AAPT_DEBUG_BADGING_FAILED";

/// The `errorCode` of an iOS application archive that carries no app bundle property list
/// to read.
const IOS_PLIST_NOT_FOUND: &str = "IOS_APP_PLIST_FILE_NOT_FOUND";

/// How the content `bytes` stored against an upload of `upload_type` was read.
///
/// The judgement is a real read of the content: the zip container is opened by the `zip`
/// crate, the entry the format requires is looked for in it, and for an Android package the
/// manifest's own bytes are read. Content the service does not read
/// ([`UploadContentFormat::Opaque`]) succeeds whatever it carries, which is what lets an
/// `EXTERNAL_DATA` upload hold arbitrary data.
fn processing_outcome(upload_type: &str, read: &[u8]) -> ProcessingOutcome {
    let succeeded = ProcessingOutcome {
        status: crate::state::UPLOAD_STATUS_SUCCEEDED,
        // "A message about the upload's result": an upload that processed cleanly has
        // nothing to report about it.
        message: None,
        // "Information that is parsed from the manifest": this plane reads the content far
        // enough to judge the format it is in, and parses no manifest out of it, so a
        // clean upload reports no document rather than a synthesised one.
        metadata: None,
    };
    match crate::state::upload_content_format(upload_type) {
        UploadContentFormat::AndroidApp => match read_entry(read, is_android_manifest_entry) {
            // An APK's manifest is Android's own BINARY XML, not text: a zip that merely
            // carries a file of that name is not an application package, so the manifest
            // itself is read rather than just found.
            ArchiveEntry::Found(manifest) if is_android_binary_xml(&manifest) => succeeded,
            ArchiveEntry::NotAnArchive => failed(
                unzip_failed(upload_type),
                "a valid Android application package (APK)",
            ),
            ArchiveEntry::Found(_) | ArchiveEntry::Absent => failed(
                ANDROID_BADGING_FAILED.to_string(),
                "a valid Android application package (APK) carrying an AndroidManifest.xml",
            ),
        },
        UploadContentFormat::IosApp => match read_entry(read, is_ios_app_bundle_entry) {
            ArchiveEntry::Found(_) => succeeded,
            ArchiveEntry::NotAnArchive => failed(
                unzip_failed(upload_type),
                "a valid iOS application archive (IPA)",
            ),
            ArchiveEntry::Absent => failed(
                IOS_PLIST_NOT_FOUND.to_string(),
                "a valid iOS application archive (IPA) carrying a Payload/*.app bundle with \
                 an Info.plist",
            ),
        },
        UploadContentFormat::ZipArchive if is_zip_archive(read) => succeeded,
        UploadContentFormat::ZipArchive => {
            failed(unzip_failed(upload_type), "a valid .zip archive")
        }
        UploadContentFormat::Opaque => succeeded,
    }
}

/// Whether `bytes` is a zip container at all — the only thing a test package's content
/// has to be, since what it carries is the caller's test code rather than a fixed layout.
///
/// Opening the container is the `zip` crate's job; nothing inside it is read.
fn is_zip_archive(bytes: &[u8]) -> bool {
    ::zip::ZipArchive::new(::std::io::Cursor::new(bytes)).is_ok()
}

/// The manifest every Android application package carries at its root.
const ANDROID_MANIFEST: &str = "AndroidManifest.xml";

/// The file every iOS app bundle carries its properties in.
const IOS_APP_INFO_PLIST: &str = "Info.plist";

/// Whether an archive entry is the manifest of an Android application package.
fn is_android_manifest_entry(name: &str) -> bool {
    name == ANDROID_MANIFEST
}

/// Whether an archive entry is the properties file of the app bundle an iOS application
/// archive carries: `Payload/<name>.app/Info.plist`.
fn is_ios_app_bundle_entry(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("Payload/") else {
        return false;
    };
    let mut segments = rest.split('/');
    matches!(
        (segments.next(), segments.next(), segments.next()),
        (Some(bundle), Some(IOS_APP_INFO_PLIST), None) if bundle.ends_with(".app")
    )
}

/// Whether `manifest` is a file in Android's binary-XML form, which is how an application
/// package carries its manifest.
///
/// The form is the platform's resource-chunk framing: a little-endian 16-bit chunk type of
/// `0x0003` (an XML document), a 16-bit header size of 8, and a 32-bit size of the whole
/// file that covers at least that header. A manifest in any other form — plain text XML, or
/// anything else a zip entry of that name may hold — is not an application package's
/// manifest.
fn is_android_binary_xml(manifest: &[u8]) -> bool {
    /// The resource-chunk type of an XML document.
    const XML_CHUNK_TYPE: u16 = 0x0003;
    /// The size the chunk header of an XML document declares.
    const XML_HEADER_SIZE: u16 = 8;
    let Some(header) = manifest.get(..8) else {
        return false;
    };
    let word = |at: usize| u16::from_le_bytes([header[at], header[at + 1]]);
    let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    word(0) == XML_CHUNK_TYPE && word(2) == XML_HEADER_SIZE && size >= u32::from(XML_HEADER_SIZE)
}

/// What reading a zip container for one entry found.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArchiveEntry {
    /// The content is not a zip container at all.
    NotAnArchive,
    /// A readable zip container that carries no entry the predicate accepts.
    Absent,
    /// The first [`MAX_ENTRY_PREFIX_BYTES`] of the entry that was found.
    Found(Vec<u8>),
}

/// How many bytes of a found entry are read: enough to judge the form of the file's
/// header, and bounded so an entry that claims to be huge costs no more than that.
const MAX_ENTRY_PREFIX_BYTES: u64 = 64;

/// Read the zip container `bytes` for the first entry `wanted` accepts.
///
/// Reading the container is the `zip` crate's job; what this function owns is the bounds on
/// how many entries one upload's processing visits and how much of the entry it reads.
fn read_entry(bytes: &[u8], wanted: impl Fn(&str) -> bool) -> ArchiveEntry {
    let Ok(mut archive) = ::zip::ZipArchive::new(::std::io::Cursor::new(bytes)) else {
        return ArchiveEntry::NotAnArchive;
    };
    let visiting = archive.len().min(MAX_ARCHIVE_ENTRIES);
    for index in 0..visiting {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        // zip reports an entry's name as a `Result`, because a name that is not valid
        // UTF-8 cannot be reported as a file path at all.
        let Ok(name) = entry.name() else {
            continue;
        };
        if !wanted(&name) {
            continue;
        }
        let mut prefix: Vec<u8> = Vec::new();
        if ::std::io::Read::read_to_end(
            &mut ::std::io::Read::take(entry, MAX_ENTRY_PREFIX_BYTES),
            &mut prefix,
        )
        .is_err()
        {
            // An entry whose bytes cannot be inflated is not a file of the package
            // either: keep looking, and report the container as carrying none.
            continue;
        }
        return ArchiveEntry::Found(prefix);
    }
    ArchiveEntry::Absent
}

/// The answer to content this plane took: it has been read, the upload it belongs to is
/// being processed, and there is nothing to report back here — the upload's own status and
/// the outcome of the read are what `GetUpload` reports.
fn stored() -> RtResponse {
    let mut resp = RtResponse::new(
        StatusCode::try_from(200u16).expect("200 is a valid status"),
        SdkBody::empty(),
    );
    resp.headers_mut().insert("content-length", "0");
    resp
}

/// One content-plane refusal: the status, the code in `x-amzn-errortype`, and a JSON body
/// carrying the reason.
fn refusal(status: u16, code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ "code": code, "message": message }).to_string();
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", ERROR_CONTENT_TYPE);
    resp.headers_mut().insert("x-amzn-errortype", code.to_string());
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An APK's manifest is told from a file that merely carries its name: only Android's
    /// binary-XML framing is a manifest, and a text document of the same name is not.
    #[test]
    fn only_android_binary_xml_is_a_manifest() {
        // (case, manifest bytes, is binary XML)
        let cases: Vec<(&str, Vec<u8>, bool)> = vec![
            (
                "the binary-XML framing an APK's manifest carries",
                vec![0x03, 0x00, 0x08, 0x00, 0x40, 0x00, 0x00, 0x00],
                true,
            ),
            (
                "a text XML document of the same name",
                b"<?xml version=\"1.0\"?><manifest/>".to_vec(),
                false,
            ),
            ("an empty manifest entry", Vec::new(), false),
            (
                "a header shorter than the framing",
                vec![0x03, 0x00, 0x08, 0x00],
                false,
            ),
            (
                "the framing of another resource chunk type",
                vec![0x02, 0x00, 0x0c, 0x00, 0x40, 0x00, 0x00, 0x00],
                false,
            ),
            (
                "the XML chunk type with a size smaller than its own header",
                vec![0x03, 0x00, 0x08, 0x00, 0x04, 0x00, 0x00, 0x00],
                false,
            ),
        ];
        for (case, manifest, expected) in cases {
            assert_eq!(
                is_android_binary_xml(&manifest),
                expected,
                "case {case:?}: expected is_android_binary_xml {expected}, got {}",
                is_android_binary_xml(&manifest)
            );
        }
    }

    /// Which format each `UploadType` names, and the file-name extensions that type may
    /// carry, over one type of every kind the enum carries
    /// (`smithy_query(query="shape",shape="UploadType")`).
    #[test]
    fn each_upload_type_names_the_format_its_content_is_read_as() {
        // (case, upload type, expected format, expected allowed file-name extensions)
        let cases: [(&str, &str, UploadContentFormat, Option<&[&str]>); 7] = [
            (
                "an Android app",
                "ANDROID_APP",
                UploadContentFormat::AndroidApp,
                Some(&[".apk"]),
            ),
            (
                "an iOS app",
                "IOS_APP",
                UploadContentFormat::IosApp,
                Some(&[".ipa"]),
            ),
            (
                "an Appium test package",
                "APPIUM_PYTHON_TEST_PACKAGE",
                UploadContentFormat::ZipArchive,
                None,
            ),
            (
                "an instrumentation test package",
                "INSTRUMENTATION_TEST_PACKAGE",
                UploadContentFormat::ZipArchive,
                None,
            ),
            (
                "a test spec",
                "APPIUM_PYTHON_TEST_SPEC",
                UploadContentFormat::Opaque,
                Some(&[".yaml", ".yml"]),
            ),
            (
                "external data",
                "EXTERNAL_DATA",
                UploadContentFormat::Opaque,
                None,
            ),
            ("a web app", "WEB_APP", UploadContentFormat::Opaque, None),
        ];
        for (case, upload_type, expected, extensions) in cases {
            assert_eq!(
                crate::state::upload_content_format(upload_type),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                crate::state::upload_content_format(upload_type)
            );
            let allowed = crate::state::upload_name_rule(upload_type)
                .map(|(extensions, _sentence)| extensions);
            assert_eq!(
                allowed, extensions,
                "case {case:?}: expected the allowed extensions {extensions:?}, got {allowed:?}"
            );
        }
    }
}
