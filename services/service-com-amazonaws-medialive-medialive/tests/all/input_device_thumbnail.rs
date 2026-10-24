// SPDX-License-Identifier: CC0-1.0

//! `DescribeInputDeviceThumbnail` — the latest thumbnail an input device has
//! sent, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests, reading the RAW
//! response body because this operation's payload is an image — so what is
//! asserted is the emulator's wire behaviour, not an internal function's return
//! value.
//!
//! The rules each suite below asserts, each over a table that spans its class
//! rather than one request:
//!
//! 1. The read reports the thumbnail the named device's record holds: the stored
//!    image as the payload, its stored media type as `ContentType`, its stored
//!    update instant as `LastModified`, and a `ContentLength` and an `ETag`
//!    DERIVED from the image data — so the length is the payload's length and the
//!    tag is the same for one image across reads and different for different
//!    images.
//! 2. A device that has sent no thumbnail reports the empty payload `Body`
//!    defaults to, a zero length, and none of the metadata that would describe an
//!    image that was never sent.
//! 3. An `InputDeviceId` naming a device this account has not claimed is the
//!    operation's own modelled `NotFoundException` (404), and an `Accept` that
//!    allows none of the media types the model declares for this operation is its
//!    modelled `BadRequestException` (400).
//! 4. The read is a REPORT: it leaves the device, its thumbnail and every other
//!    device exactly as they were.
//!
//! NOTHING in this emulator sends a thumbnail — the real service receives it from
//! a connected Link unit and no unit is connected here — so a device claimed
//! through the API has none (asserted below), and the suites that exercise what a
//! read reports seed the device through the store's own one writer,
//! `record_input_device_thumbnail`, instead of asking an operation to invent an
//! image.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;
use service_com_amazonaws_medialive_medialive::ops::types::{
    ACCEPT_HEADER_VALUES, CONTENT_TYPE_VALUES,
};
use service_com_amazonaws_medialive_medialive::state::{
    self, InputDeviceThumbnailRecord, MAX_INPUT_DEVICE_THUMBNAIL_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::input_device_maintenance_window::{claim_device, device, start_maintenance_window};
use crate::wire::{call_raw, error_code, error_message, header, medialive_with_engine, DEFAULT_REGION};

/// One `DescribeInputDeviceThumbnail` call
/// (`smithy_query(query="operation",operation="DescribeInputDeviceThumbnail")`:
/// GET /prod/inputDevices/{InputDeviceId}/thumbnailData), carrying `accept` as
/// its `Accept` header when the case names one.
async fn read_thumbnail(
    router: &MultiServiceRouter,
    id: &str,
    accept: Option<&str>,
) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let headers: Vec<(&str, &str)> = accept.map(|value| ("accept", value)).into_iter().collect();
    call_raw(
        router,
        DEFAULT_REGION,
        "GET",
        &format!("/prod/inputDevices/{id}/thumbnailData"),
        None,
        &headers,
    )
    .await
}

/// The media type every row of every table below asks for and stores: the one
/// value the model's own `AcceptHeader` and `ContentType` enums declare
/// (`smithy_query(query="shape",shape="AcceptHeader,ContentType")`), as the
/// generated types carry it.
fn served_media_type() -> &'static str {
    ACCEPT_HEADER_VALUES
        .first()
        .expect("the model declares an AcceptHeader value")
}

/// Seed `thumbnail` as the latest thumbnail of `id` through the store's own one
/// writer, asserting the device took it.
///
/// The store's writer, so what an operation reports is what something sent:
/// there is no second copy for the two to disagree about.
fn send_thumbnail(
    engine: &engine::InMemoryEngine,
    case: &str,
    id: &str,
    thumbnail: InputDeviceThumbnailRecord,
) {
    assert!(
        offer_thumbnail(engine, id, thumbnail),
        "case {case:?}: setup: expected device {id} to take the thumbnail"
    );
}

/// The same write, answering whether the store took it, for the table of
/// thumbnails the store REFUSES.
fn offer_thumbnail(
    engine: &engine::InMemoryEngine,
    id: &str,
    thumbnail: InputDeviceThumbnailRecord,
) -> bool {
    let store = state::service_state(engine).expect("the medialive store");
    let mut store = store.lock();
    store.record_input_device_thumbnail(id, thumbnail)
}

/// An instant a fixed number of seconds after the Unix epoch, so an asserted
/// timestamp names an instant the test chose rather than one it has to guess.
fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

/// The error body of a refused read, as a client parses it (the payload of a
/// failure is the protocol's JSON envelope rather than an image).
fn error_body(case: &str, bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|_| {
        panic!(
            "case {case:?}: expected a JSON error envelope, got {:?}",
            String::from_utf8_lossy(bytes)
        )
    })
}

/// One row of the thumbnail table: the image a device sent and the instant it
/// last updated it.
struct Thumbnail {
    case: &'static str,
    /// The device the image was sent from, so the rows differ in the device's
    /// type and in the shape of its id too.
    device: &'static str,
    body: Vec<u8>,
    last_modified: SystemTime,
}

/// The thumbnails the suites below run over. They differ along what the rule
/// generalises over: the SIZE of the image (one byte, a dozen, four kibibytes,
/// the largest the store keeps and none at all), its BYTES (including ones that
/// are not valid UTF-8, because the payload is binary), the instant it was
/// updated at, and the device that sent it — because the rule is about reporting
/// whatever image the named device's record holds.
fn thumbnails() -> Vec<Thumbnail> {
    vec![
        Thumbnail {
            case: "a one-byte image",
            device: "hd-123456789abcdef",
            body: vec![0xff],
            last_modified: at(1_700_000_000),
        },
        Thumbnail {
            case: "the leading bytes of a jpeg, none of which are valid UTF-8 text",
            device: "uhd-0a1b2c3d4e5f6071",
            body: vec![
                0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x80, 0xfe,
            ],
            last_modified: at(1_000_000_000),
        },
        Thumbnail {
            case: "a four-kibibyte image",
            device: "UHD-A1B2",
            body: (0..4096).map(|byte| (byte % 251) as u8).collect(),
            last_modified: at(2_000_000_000),
        },
        Thumbnail {
            case: "an image of the largest size the store keeps",
            device: "hd-00112233445566778899aabbccddeeff",
            body: vec![0x5a; MAX_INPUT_DEVICE_THUMBNAIL_BYTES],
            last_modified: at(1_234_567_890),
        },
        Thumbnail {
            case: "an image of no bytes at all, which the device still sent",
            device: "hd-empty00000001",
            body: Vec::new(),
            last_modified: at(1_600_000_000),
        },
    ]
}

/// The `ETag` of the thumbnail the device `id` reports, asserted to be an
/// entity tag: a quoted, non-empty string.
async fn etag(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let (status, _, headers) = read_thumbnail(router, id, Some(served_media_type())).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the thumbnail read to answer 200, got {status}"
    );
    let etag = header(&headers, "etag")
        .unwrap_or_else(|| panic!("case {case:?}: expected an ETag, got {headers:?}"));
    assert!(
        etag.starts_with('"') && etag.ends_with('"') && etag.len() > 2,
        "case {case:?}: expected a quoted entity tag, got {etag:?}"
    );
    etag
}

/// Rule 1: the read reports the image the named device's record holds, with the
/// stored media type and update instant, and with the length and the entity tag
/// derived from the image data.
#[tokio::test]
async fn the_read_reports_the_image_the_device_sent() {
    for Thumbnail {
        case,
        device: id,
        body,
        last_modified,
    } in thumbnails()
    {
        let (router, engine) = medialive_with_engine();
        claim_device(&router, case, id).await;
        send_thumbnail(
            &engine,
            case,
            id,
            InputDeviceThumbnailRecord::new(body.clone(), served_media_type(), last_modified),
        );

        let (status, payload, headers) = read_thumbnail(&router, id, Some(served_media_type())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {:?}",
            String::from_utf8_lossy(&payload)
        );
        assert_eq!(
            payload, body,
            "case {case:?}: expected the {} stored bytes as the payload, got {} bytes",
            body.len(),
            payload.len()
        );
        assert_eq!(
            header(&headers, "content-length").as_deref(),
            Some(body.len().to_string().as_str()),
            "case {case:?}: expected the length of the image, got {:?}",
            header(&headers, "content-length")
        );
        assert_eq!(
            header(&headers, "content-type").as_deref(),
            Some(served_media_type()),
            "case {case:?}: expected the stored media type, got {:?}",
            header(&headers, "content-type")
        );

        let modified = header(&headers, "last-modified")
            .unwrap_or_else(|| panic!("case {case:?}: expected a Last-Modified, got {headers:?}"));
        let parsed = DateTime::from_str(&modified, Format::HttpDate)
            .unwrap_or_else(|_| panic!("case {case:?}: expected an HTTP date, got {modified:?}"));
        assert_eq!(
            parsed,
            DateTime::from(last_modified),
            "case {case:?}: expected the instant the device updated the image at, got {modified:?}"
        );

        // The tag is derived from the content, so it is the same on a second
        // read of the same image.
        let first = etag(&router, case, id).await;
        let again = etag(&router, case, id).await;
        assert_eq!(
            first, again,
            "case {case:?}: expected the entity tag of one image to be stable across reads"
        );
    }
}

/// Rule 1, the part that makes an entity tag a VERSION: two devices holding the
/// same image report the same tag, and a device whose image changes reports a
/// different one.
#[tokio::test]
async fn the_entity_tag_is_the_version_of_the_image() {
    let (router, engine) = medialive_with_engine();
    let rows = thumbnails();
    for Thumbnail {
        case,
        device: id,
        body,
        last_modified,
    } in &rows
    {
        claim_device(&router, case, id).await;
        send_thumbnail(
            &engine,
            case,
            id,
            InputDeviceThumbnailRecord::new(body.clone(), served_media_type(), *last_modified),
        );
    }

    // A device holding a COPY of another row's image: same bytes, another device,
    // another update instant.
    let copy = "hd-copy000000001";
    let copied = &rows[1];
    claim_device(&router, copied.case, copy).await;
    send_thumbnail(
        &engine,
        copied.case,
        copy,
        InputDeviceThumbnailRecord::new(copied.body.clone(), served_media_type(), at(1_500_000_000)),
    );
    assert_eq!(
        etag(&router, copied.case, copy).await,
        etag(&router, copied.case, copied.device).await,
        "case {:?}: expected the same image to have the same entity tag wherever it is held",
        copied.case
    );

    // Every distinct image has a distinct tag.
    let mut seen: Vec<(&str, String)> = Vec::new();
    for Thumbnail {
        case,
        device: id,
        body,
        ..
    } in &rows
    {
        let tag = etag(&router, case, id).await;
        for (other_case, other) in &seen {
            assert_ne!(
                &tag, other,
                "case {case:?}: expected a different entity tag from case {other_case:?}, \
                 whose image differs"
            );
        }
        seen.push((case, tag.clone()));

        // And the tag follows the image: sending a different one changes it.
        // The change flips a byte rather than appending one, so a row that is
        // already the largest image the store keeps stays inside that bound.
        let mut changed = body.clone();
        match changed.last_mut() {
            Some(last) => *last ^= 0xff,
            None => changed.push(0x01),
        }
        send_thumbnail(
            &engine,
            case,
            id,
            InputDeviceThumbnailRecord::new(changed, served_media_type(), at(1_800_000_000)),
        );
        assert_ne!(
            etag(&router, case, id).await,
            tag,
            "case {case:?}: expected a new image to carry a new entity tag"
        );
    }
}

/// Rule 2: a device that has sent no thumbnail — which is every device claimed
/// through this emulator's API, because no Link unit is connected to it — reports
/// the empty payload its `Body` member defaults to, a zero length, and none of
/// the metadata that would describe an image that was never sent.
///
/// The rows differ in the device's type and id shape, and the device is read in
/// each state the rest of this service can put it in (idle, streaming, and with a
/// maintenance window open), because the rule is about having sent no image
/// rather than about any one device or state.
#[tokio::test]
async fn a_device_that_sent_no_image_reports_an_empty_payload() {
    for (case, id) in [
        ("the hd device the operation documents", "hd-123456789abcdef"),
        ("a uhd device", "uhd-0a1b2c3d4e5f6071"),
        ("a uhd device whose label is printed in upper case", "UHD-A1B2"),
        (
            "an hd device with a long identifier",
            "hd-00112233445566778899aabbccddeeff",
        ),
    ] {
        let (router, _engine) = medialive_with_engine();
        claim_device(&router, case, id).await;

        for stage in ["claimed", "started", "in a maintenance window"] {
            if stage == "started" {
                let (status, _, _) = call_raw(
                    &router,
                    DEFAULT_REGION,
                    "POST",
                    &format!("/prod/inputDevices/{id}/start"),
                    None,
                    &[],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: StartInputDevice: expected 200, got {status}"
                );
            }
            if stage == "in a maintenance window" {
                let (status, body) = start_maintenance_window(&router, id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: StartInputDeviceMaintenanceWindow: expected 200, \
                     got {status}: {body}"
                );
            }

            let (status, payload, headers) =
                read_thumbnail(&router, id, Some(served_media_type())).await;
            assert_eq!(
                status, 200,
                "case {case:?} ({stage}): expected 200, got {status}: {:?}",
                String::from_utf8_lossy(&payload)
            );
            assert!(
                payload.is_empty(),
                "case {case:?} ({stage}): expected no image, got {} bytes",
                payload.len()
            );
            assert_eq!(
                header(&headers, "content-length").as_deref(),
                Some("0"),
                "case {case:?} ({stage}): expected a zero length, got {:?}",
                header(&headers, "content-length")
            );
            for absent in ["etag", "last-modified"] {
                assert_eq!(
                    header(&headers, absent),
                    None,
                    "case {case:?} ({stage}): expected no {absent} for an image that was \
                     never sent, got {:?}",
                    header(&headers, absent)
                );
            }
        }
    }
}

/// Rule 3: an `InputDeviceId` naming a device this account has not claimed is the
/// operation's own modelled `NotFoundException` (404), carrying this service's
/// not-found wording — resource type then identifier.
///
/// The ids differ in prefix, length and character class, and include one that is
/// not of the documented device-id form at all, because the rule is about
/// resolution against the store rather than about any one id shape.
#[tokio::test]
async fn a_device_the_account_has_not_claimed_is_not_found() {
    let (router, engine) = medialive_with_engine();
    // One device with an image exists, so the refusals are about the id and not
    // about an empty store.
    let claimed = "hd-claimed000001";
    claim_device(&router, "claimed", claimed).await;
    send_thumbnail(
        &engine,
        "claimed",
        claimed,
        InputDeviceThumbnailRecord::new(vec![0xff, 0xd8], served_media_type(), at(1_700_000_000)),
    );

    for id in [
        "hd-123456789abcdef",
        "uhd-0a1b2c3d",
        "hd-claimed00000",
        "not-a-device",
    ] {
        let (status, payload, _) = read_thumbnail(&router, id, Some(served_media_type())).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 for a device no claim registered, got {status}"
        );
        let body = error_body(id, &payload);
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {id:?}: expected the operation's modelled NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("InputDevice {id} not found"),
            "case {id:?}: expected the message to name the resource type then the identifier, \
             got {:?}",
            error_message(&body)
        );
    }
}

/// Rule 3, the `Accept` half: the read answers with a media type the model
/// declares for it, so an `Accept` that allows one of those types is served and
/// one that allows none of them is the operation's modelled
/// `BadRequestException` (400).
///
/// The rows span the forms a media range takes: the exact type, either wildcard,
/// a range carrying parameters, a list, another casing, a quality of zero (which
/// is how a caller says a type is NOT acceptable) and types this operation does
/// not serve. They vary the VALUE rather than its presence, because the model
/// makes `Accept` a required member of this request
/// (`smithy_query(query="operation",operation="DescribeInputDeviceThumbnail")`),
/// so the request boundary is what answers a request that carries none.
#[tokio::test]
async fn the_accept_header_decides_whether_the_image_is_served() {
    let served = served_media_type();
    let image = vec![0xff, 0xd8, 0xff, 0xe0, 0x7f];
    // (the Accept header, whether it allows a type this operation serves)
    let table: [(&str, bool); 15] = [
        (served, true),
        ("IMAGE/JPEG", true),
        ("image/*", true),
        ("*/*", true),
        ("image/jpeg;q=0.8", true),
        ("application/json, image/jpeg", true),
        ("text/plain, */*;q=0.1", true),
        ("image/jpeg;q=0.001", true),
        ("image/png;q=0, image/jpeg", true),
        ("image/jpeg;q=1;charset=binary", true),
        ("image/png", false),
        ("application/json", false),
        ("text/plain;q=1", false),
        ("image/jpeg;q=0", false),
        ("*/*;q=0", false),
    ];

    for (accept, allowed) in table {
        let case = accept;
        let (router, engine) = medialive_with_engine();
        let id = "hd-123456789abcdef";
        claim_device(&router, case, id).await;
        send_thumbnail(
            &engine,
            case,
            id,
            InputDeviceThumbnailRecord::new(image.clone(), served, at(1_700_000_000)),
        );

        let (status, payload, headers) = read_thumbnail(&router, id, Some(accept)).await;
        if allowed {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the image to be served, got {status}: {:?}",
                String::from_utf8_lossy(&payload)
            );
            assert_eq!(
                payload, image,
                "case {case:?}: expected the stored image as the payload"
            );
            assert_eq!(
                header(&headers, "content-type").as_deref(),
                Some(served),
                "case {case:?}: expected the stored media type, got {:?}",
                header(&headers, "content-type")
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a refusal of a type this operation cannot answer with, \
                 got {status}: {:?}",
                String::from_utf8_lossy(&payload)
            );
            let body = error_body(case, &payload);
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected the operation's modelled BadRequestException, got {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(case) && message.contains(served),
                "case {case:?}: expected the message to name what was asked for and what is \
                 served, got {message:?}"
            );
        }
    }
}

/// Rule 4: the read is a REPORT — it leaves the image, the device that sent it
/// and every other device exactly as they were, so a second reader sees the same
/// thing whether or not the first read happened.
#[tokio::test]
async fn the_read_changes_nothing() {
    let (router, engine) = medialive_with_engine();
    let rows = thumbnails();
    for Thumbnail {
        case,
        device: id,
        body,
        last_modified,
    } in &rows
    {
        claim_device(&router, case, id).await;
        send_thumbnail(
            &engine,
            case,
            id,
            InputDeviceThumbnailRecord::new(body.clone(), served_media_type(), *last_modified),
        );
    }
    // A device nothing is read for, so the suite can tell one read from a
    // store-wide change.
    let bystander = "hd-bystander0001";
    claim_device(&router, "bystander", bystander).await;

    let bystander_before = device(&router, "bystander", bystander).await;
    for Thumbnail {
        case,
        device: id,
        body,
        ..
    } in &rows
    {
        let described = device(&router, case, id).await;
        let first = read_thumbnail(&router, id, Some(served_media_type())).await;
        let second = read_thumbnail(&router, id, Some(served_media_type())).await;
        assert_eq!(
            (first.0, &first.1),
            (second.0, &second.1),
            "case {case:?}: expected two reads of one image to answer the same"
        );
        assert_eq!(
            &first.1, body,
            "case {case:?}: expected the stored image, unchanged by the read"
        );
        assert_eq!(
            device(&router, case, id).await,
            described,
            "case {case:?}: expected the read to leave the device it named alone"
        );
        assert_eq!(
            device(&router, "bystander", bystander).await,
            bystander_before,
            "case {case:?}: expected the read to leave every other device alone"
        );
        let (status, payload, headers) =
            read_thumbnail(&router, bystander, Some(served_media_type())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the bystander's own read to answer 200, got {status}"
        );
        assert!(
            payload.is_empty() && header(&headers, "etag").is_none(),
            "case {case:?}: expected the bystander to still have sent no image, \
             got {} bytes and {:?}",
            payload.len(),
            header(&headers, "etag")
        );
    }
}

/// Rule 1, the part that keeps a response REPORTABLE: the store takes only what
/// the operation can report back — an image for a device this account has
/// claimed, in a media type the model's `ContentType` enum declares, of at most
/// the size the store keeps — and a device the store refused an image for goes on
/// reporting that it has sent none.
///
/// The rows are the bound itself and one byte past it, a media type outside the
/// model's enum, and an unclaimed device, because the rule is about what the
/// store admits rather than about one refusal.
#[tokio::test]
async fn an_image_the_store_refuses_is_never_reported() {
    let (router, engine) = medialive_with_engine();
    let id = "hd-123456789abcdef";
    claim_device(&router, "the device", id).await;

    // The published catalog of media types is the model's own
    // (`smithy_query(query="shape",shape="ContentType")`), as the generated types
    // carry it: every value in it is taken, and a type outside it is not.
    for declared in CONTENT_TYPE_VALUES {
        assert!(
            offer_thumbnail(
                &engine,
                id,
                InputDeviceThumbnailRecord::new(vec![0x01], declared, at(1_700_000_000))
            ),
            "case {declared:?}: expected a media type the model declares to be taken"
        );
        let (_, _, headers) = read_thumbnail(&router, id, Some(declared)).await;
        assert_eq!(
            header(&headers, "content-type").as_deref(),
            Some(*declared),
            "case {declared:?}: expected the declared media type to be reported back, got {:?}",
            header(&headers, "content-type")
        );
    }

    // (the case, the device the image was offered for, the image offered)
    let refused: [(&str, &str, InputDeviceThumbnailRecord); 4] = [
        (
            "a media type outside the model's enum",
            id,
            InputDeviceThumbnailRecord::new(vec![0x89, 0x50], "image/png", at(1_700_000_000)),
        ),
        (
            "an image one byte past the size the store keeps",
            id,
            InputDeviceThumbnailRecord::new(
                vec![0x5a; MAX_INPUT_DEVICE_THUMBNAIL_BYTES + 1],
                served_media_type(),
                at(1_700_000_000),
            ),
        ),
        (
            "an image for a device no claim registered",
            "hd-unclaimed0001",
            InputDeviceThumbnailRecord::new(vec![0xff], served_media_type(), at(1_700_000_000)),
        ),
        (
            "an image for an id that is not a device id at all",
            "not-a-device",
            InputDeviceThumbnailRecord::new(vec![0xff], served_media_type(), at(1_700_000_000)),
        ),
    ];

    for (case, device_id, thumbnail) in refused {
        assert!(
            !offer_thumbnail(&engine, device_id, thumbnail),
            "case {case:?}: expected the store to refuse the image"
        );
    }

    // The image the store did take is the one still reported: a refusal wrote
    // nothing over it.
    let (status, payload, headers) = read_thumbnail(&router, id, Some(served_media_type())).await;
    assert_eq!(
        status, 200,
        "expected the device to still report the image the store took, got {status}"
    );
    assert_eq!(
        payload,
        vec![0x01],
        "expected the refusals to leave the stored image alone"
    );
    assert_eq!(
        header(&headers, "content-length").as_deref(),
        Some("1"),
        "expected the length of the stored image, got {:?}",
        header(&headers, "content-length")
    );

    // And a device the store refused an image FOR has still sent none.
    let unclaimed = "hd-unclaimed0001";
    claim_device(&router, "the refused device", unclaimed).await;
    let (status, payload, headers) =
        read_thumbnail(&router, unclaimed, Some(served_media_type())).await;
    assert_eq!(
        status, 200,
        "expected the newly claimed device to be readable, got {status}"
    );
    assert!(
        payload.is_empty() && header(&headers, "etag").is_none(),
        "expected the device the store refused an image for to have sent none, \
         got {} bytes and {:?}",
        payload.len(),
        header(&headers, "etag")
    );
}
