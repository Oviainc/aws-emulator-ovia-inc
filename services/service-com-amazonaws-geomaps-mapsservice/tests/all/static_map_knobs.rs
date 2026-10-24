// SPDX-License-Identifier: CC0-1.0

//! `GetStaticMap`'s label and political-view knobs, end to end.
//!
//! Every case drives a real HTTP request through the router the emulator
//! mounts, so what is asserted is the bytes on the wire. The rules pinned here,
//! each over a table of inputs rather than one request:
//!
//! * **`PoliticalView`** selects between the boundary lines a disputed
//!   territory has: a frame over a territory draws a different image under the
//!   view of the country that claims it than under the de-facto boundaries, and
//!   a view with no claim over that territory draws the de-facto ones. Every
//!   (view, territory) pair the model documents is a row.
//! * **`Language`** decides the language the image's labels are written in: two
//!   supported languages are two images, a BCP 47 tag selects by its primary
//!   subtag, and a language the catalog has no label data for falls back to the
//!   catalog's primary language.
//! * **`CropLabels`** leaves off a label that would be cut by the frame's edge,
//!   and changes nothing where no label would be cut.
//!
//! The harness is the three lines a dispatch needs (engine, router, request);
//! `tests/all/map_assets.rs` keeps its own, because an integration-test module
//! cannot reach a sibling suite's private helpers.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use service_com_amazonaws_geomaps_mapsservice::state::assets::{self, Envelope};
use service_com_amazonaws_geomaps_mapsservice::state::boundaries::{
    DisputedTerritory, DISPUTED_TERRITORIES,
};
use service_com_amazonaws_geomaps_mapsservice::state::labels;
use service_com_amazonaws_geomaps_mapsservice::state::FontRecord;
use service_com_amazonaws_geomaps_mapsservice::{
    ComAmazonawsGeomapsMapsserviceState, EmulatedService,
};
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host a `geo-maps` client addresses; its first label is the
/// signing name the router routes on.
const HOST: &str = "geo-maps.us-east-1.amazonaws.com";

/// The font stack the `Standard` style publishes, and so the metrics the labels
/// of a request that names no style are measured with.
const DEFAULT_FONT_STACK: &str = "Amazon Ember Regular";

/// One emulator: the single shared engine plus the router that serves it.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise the static-map knobs, not SigV4; the signature
        // path has its own tests in the framework.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    fn catalog(&self) -> engine::ServiceState<ComAmazonawsGeomapsMapsserviceState> {
        self.engine.state::<ComAmazonawsGeomapsMapsserviceState>()
    }

    async fn get(&self, path_and_query: &str) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method(http::Method::GET)
            .uri(format!("https://{HOST}{path_and_query}"))
            .header(http::header::HOST, HOST)
            .body(Bytes::new())
            .expect("a well-formed test request");
        self.router.dispatch(request).await
    }

    /// The image bytes a request answers with, asserting it answered at all.
    async fn image(&self, case: &str, path_and_query: &str) -> Vec<u8> {
        let response = self.get(path_and_query).await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: expected 200 for {path_and_query:?}, got {} with body {}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let body = response.body().to_vec();
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let digest = sha2::Sha256::digest(&body);
        assert_eq!(
            etag,
            format!("\"{}\"", hex::encode(&digest[..16])),
            "case {case:?}: the ETag must hash the body served"
        );
        assert!(
            !body.is_empty(),
            "case {case:?}: an image must carry bytes, got an empty body"
        );
        body
    }

    /// The font metrics the catalog has for the default style's stack.
    fn default_font(&self) -> FontRecord {
        let catalog = self.catalog();
        let font = catalog
            .lock()
            .fonts
            .get(DEFAULT_FONT_STACK)
            .cloned()
            .unwrap_or_else(|| panic!("the catalog must publish metrics for {DEFAULT_FONT_STACK}"));
        font
    }
}

// ---------------------------------------------------------------------------
// PoliticalView
// ---------------------------------------------------------------------------

/// A square image framed on `territory`, widened by half the territory's span
/// on each side so both of the boundary lines the territory can have are well
/// inside the frame.
///
/// The box is written the way the modeled `BoundingBox` pattern requires:
/// `longitude,latitude` pairs in plain decimal.
fn frame_over(territory: &DisputedTerritory, side: u32) -> String {
    let margin_lon = (territory.max_lon - territory.min_lon) / 2.0;
    let margin_lat = (territory.max_lat - territory.min_lat) / 2.0;
    let min_lon = (territory.min_lon - margin_lon).clamp(-179.0, 179.0);
    let max_lon = (territory.max_lon + margin_lon).clamp(-179.0, 179.0);
    let min_lat = (territory.min_lat - margin_lat).clamp(-85.0, 85.0);
    let max_lat = (territory.max_lat + margin_lat).clamp(-85.0, 85.0);
    format!(
        "/v2/static/map?width={side}&height={side}&bounding-box={min_lon:.4},{min_lat:.4},{max_lon:.4},{max_lat:.4}"
    )
}

/// A country code the boundary catalog records no claim for: a user-assigned
/// ISO 3166-1 alpha-2 code, so it satisfies the modeled `CountryCode` pattern
/// and claims nothing.
const UNCLAIMING_VIEW: &str = "ZZ";

/// Every (view, territory) pair the model documents: the claiming country's own
/// view draws the disputed boundary somewhere else than the de-facto one, and a
/// view with no claim over that territory draws the de-facto one.
#[tokio::test]
async fn a_political_view_moves_the_boundary_of_the_territory_it_claims() {
    let emulator = Emulator::new();
    assert!(
        DISPUTED_TERRITORIES.len() >= 14,
        "the catalog must carry one record per documented (view, territory) pair, got {}",
        DISPUTED_TERRITORIES.len()
    );
    for territory in DISPUTED_TERRITORIES {
        let case = format!("{} on {}", territory.view, territory.territory);
        let frame = frame_over(territory, 400);
        let de_facto = emulator.image(&case, &frame).await;
        let claimed = emulator
            .image(
                &case,
                &format!("{frame}&political-view={}", territory.view),
            )
            .await;
        assert_ne!(
            de_facto, claimed,
            "case {case:?}: the claiming country's view must draw this territory's \
             boundary somewhere else than the de-facto view does"
        );
        let unclaiming = emulator
            .image(&case, &format!("{frame}&political-view={UNCLAIMING_VIEW}"))
            .await;
        assert_eq!(
            de_facto, unclaiming,
            "case {case:?}: a view with no claim over this territory must draw the \
             de-facto boundary, as a request naming no view does"
        );
    }
}

/// A political view moves only the boundaries of the territories it claims: a
/// frame nowhere near one of them renders the same image under every view.
#[tokio::test]
async fn a_political_view_changes_nothing_outside_the_territories_it_claims() {
    let emulator = Emulator::new();
    // A frame in the middle of the Pacific, which no documented territory
    // reaches, checked against the catalog rather than asserted by eye.
    let path = "/v2/static/map?width=256&height=256&center=-150.0,-20.0&zoom=6";
    let envelope = Envelope::from_center_zoom((-150.0, -20.0), 6.0, 256, 256);
    assert!(
        DISPUTED_TERRITORIES
            .iter()
            .all(|territory| !territory.overlaps(envelope)),
        "this frame must reach no disputed territory, or the case proves nothing"
    );
    let plain = emulator.image("no view", path).await;
    for view in DISPUTED_TERRITORIES
        .iter()
        .map(|territory| territory.view)
        .chain([UNCLAIMING_VIEW])
    {
        let image = emulator
            .image(view, &format!("{path}&political-view={view}"))
            .await;
        assert_eq!(
            plain, image,
            "case {view:?}: a view must not change a frame that carries none of \
             the territories it claims"
        );
    }
}

/// The report's own reproduction: two requests that differ only in
/// `political-view`, over India's view on Gilgit-Baltistan, must not answer
/// with the same bytes.
#[tokio::test]
async fn the_reported_political_views_answer_with_different_images() {
    let emulator = Emulator::new();
    let frame = "/v2/static/map?width=600&height=400&center=77.5,34.0&zoom=6";
    let india = emulator
        .image("IND", &format!("{frame}&political-view=IND"))
        .await;
    let china = emulator
        .image("CHN", &format!("{frame}&political-view=CHN"))
        .await;
    assert_ne!(
        india, china,
        "two political views of the same frame must draw the disputed boundary \
         differently"
    );
}

// ---------------------------------------------------------------------------
// Language
// ---------------------------------------------------------------------------

/// A frame whose place lattice lands on land, so it carries labels for
/// `Language` and `CropLabels` to act on.
const LABELLED_FRAME: &str = "/v2/static/map?width=400&height=400&center=0,0&zoom=4";

fn labelled_frame_envelope() -> Envelope {
    Envelope::from_center_zoom((0.0, 0.0), 4.0, 400, 400)
}

/// Languages spanning the scripts the supported set is written in: the rows
/// differ in language, and so in the label text drawn.
const LANGUAGE_ROWS: &[&str] = &[
    "en", "de", "fr", "pt", "ru", "sr", "el", "ar", "he", "hy", "ka", "hi", "mr", "bn", "pa", "gu",
    "or", "ta", "te", "kn", "ml", "th", "my", "km", "ja", "ko", "zh",
];

/// Each supported language labels the image in its own language, so no two of
/// them answer with the same bytes.
#[tokio::test]
async fn every_supported_language_labels_the_image_differently() {
    let emulator = Emulator::new();
    let places = assets::labelled_places(labelled_frame_envelope(), "en");
    assert!(
        !places.is_empty(),
        "this frame must carry places to label, or the language cases prove nothing"
    );

    let mut seen: BTreeMap<Vec<u8>, &str> = BTreeMap::new();
    for language in LANGUAGE_ROWS {
        let image = emulator
            .image(language, &format!("{LABELLED_FRAME}&lang={language}"))
            .await;
        if let Some(previous) = seen.insert(image, language) {
            panic!(
                "case {language:?}: this language answered with the image {previous:?} \
                 already served, so the label language changed nothing"
            );
        }
    }
    assert_eq!(
        seen.len(),
        LANGUAGE_ROWS.len(),
        "every language row must answer with its own image, got {} distinct images",
        seen.len()
    );
}

/// A BCP 47 tag selects its primary language subtag, and a language the catalog
/// has no label data for falls back to the catalog's primary language — which
/// is also what a request naming no language gets.
#[tokio::test]
async fn a_language_tag_resolves_to_the_label_data_the_catalog_has() {
    let emulator = Emulator::new();
    let primary = {
        let catalog = emulator.catalog();
        let primary = catalog.lock().primary_label_language.clone();
        primary
    };
    let expected = emulator
        .image(&primary, &format!("{LABELLED_FRAME}&lang={primary}"))
        .await;

    // Rows: the ways a request can end up at the catalog's primary language.
    let rows = [
        ("no language at all", LABELLED_FRAME.to_string()),
        (
            "the primary language uppercased",
            format!("{LABELLED_FRAME}&lang={}", primary.to_uppercase()),
        ),
        (
            "a region subtag on the primary language",
            format!("{LABELLED_FRAME}&lang={primary}-GB"),
        ),
        (
            "a language code the catalog has no data for",
            format!("{LABELLED_FRAME}&lang=zz"),
        ),
        (
            "a tag whose primary subtag the catalog has no data for",
            format!("{LABELLED_FRAME}&lang=zz-Latn-419"),
        ),
    ];
    for (case, path) in rows {
        let image = emulator.image(case, &path).await;
        assert_eq!(
            image, expected,
            "case {case:?}: the labels must be written in the catalog's primary \
             language {primary:?}"
        );
    }

    // And the set of languages is the catalog's: withdraw one and a request for
    // it falls back, where a moment ago it had its own labels.
    let withdrawn = "ja";
    let before = emulator
        .image(withdrawn, &format!("{LABELLED_FRAME}&lang={withdrawn}"))
        .await;
    assert_ne!(
        before, expected,
        "case {withdrawn:?}: a supported language must label the image in its own \
         language before it is withdrawn"
    );
    {
        let catalog = emulator.catalog();
        let mut catalog = catalog.lock();
        catalog.label_languages.remove(withdrawn);
    }
    let after = emulator
        .image(withdrawn, &format!("{LABELLED_FRAME}&lang={withdrawn}"))
        .await;
    assert_eq!(
        after, expected,
        "case {withdrawn:?}: a language the catalog no longer carries must fall \
         back to the primary language"
    );
}

// ---------------------------------------------------------------------------
// CropLabels
// ---------------------------------------------------------------------------

/// One `CropLabels` row: a frame, the side of its square image, and whether it
/// draws labels at all.
struct CropRow {
    path: &'static str,
    side: u32,
    labelled: bool,
}

/// `CropLabels` leaves off the labels a frame would cut, and changes nothing
/// where there is nothing to cut.
#[tokio::test]
async fn crop_labels_leaves_off_only_the_labels_the_frame_would_cut() {
    let emulator = Emulator::new();
    let font = emulator.default_font();
    let rows = [
        CropRow {
            path: LABELLED_FRAME,
            side: 400,
            labelled: true,
        },
        CropRow {
            path: "/v2/static/map?width=256&height=256&center=0,0&zoom=3",
            side: 256,
            labelled: true,
        },
        CropRow {
            path: "/v2/static/map?width=256&height=256&center=0,0&zoom=5",
            side: 256,
            labelled: true,
        },
        // The control: with the places switched off there is no label to cut,
        // so the flag must change nothing.
        CropRow {
            path: "/v2/static/map?width=400&height=400&center=0,0&zoom=4&pois=Disabled",
            side: 400,
            labelled: false,
        },
    ];

    let mut cutting_rows = 0_usize;
    let mut intact_rows = 0_usize;
    for row in rows {
        let plain = emulator.image(row.path, row.path).await;
        let explicit_false = emulator
            .image(row.path, &format!("{}&crop-labels=false", row.path))
            .await;
        assert_eq!(
            plain, explicit_false,
            "case {:?}: crop-labels=false must render what an unset flag renders",
            row.path
        );

        let cropped = emulator
            .image(row.path, &format!("{}&crop-labels=true", row.path))
            .await;
        let cut = row.labelled && a_label_runs_past_the_edge(row.side, &font);
        if cut {
            cutting_rows += 1;
            assert_ne!(
                plain, cropped,
                "case {:?}: crop-labels=true must leave off the labels this frame \
                 would cut",
                row.path
            );
        } else {
            intact_rows += 1;
            assert_eq!(
                plain, cropped,
                "case {:?}: crop-labels=true must change nothing where no label \
                 would be cut",
                row.path
            );
        }
    }

    // The table has to span both sides of the rule, or one of the two branches
    // above was never taken.
    assert!(
        cutting_rows > 0 && intact_rows > 0,
        "the rows must cover both a frame that cuts a label and one that does not, \
         got {cutting_rows} cutting and {intact_rows} intact"
    );
}

/// Whether a square frame of `side` pixels, centred on `0,0` at the zoom its
/// path states, carries a label that runs past the image's right edge: the
/// condition `CropLabels` acts on.
///
/// Computed from the same catalog metrics the renderer measures with, so the
/// case states which rows have something to crop rather than assuming it.
fn a_label_runs_past_the_edge(side: u32, font: &FontRecord) -> bool {
    let zoom = match side {
        400 => 4.0,
        256 => 3.0,
        _ => 5.0,
    };
    let envelope = Envelope::from_center_zoom((0.0, 0.0), zoom, side, side);
    assets::labelled_places(envelope, "en")
        .iter()
        .any(|(position, name)| {
            let (px, _) = envelope.project(*position, side, side);
            let (width, _) = labels::label_extent(font, name, labels::LABEL_TEXT_SIZE);
            px + i64::from(width) > i64::from(side) - 1
        })
}

// ---------------------------------------------------------------------------
// The knobs together
// ---------------------------------------------------------------------------

/// The three knobs are part of the request's identity: a repeat of the same
/// request is byte-identical, and changing any one of them changes the image.
#[tokio::test]
async fn the_label_and_view_knobs_are_part_of_the_images_identity() {
    let emulator = Emulator::new();
    let base = "/v2/static/map?width=320&height=320&center=35.8,33.0&zoom=7";
    // Rows: the same frame with one knob changed at a time. The frame is over
    // the Golan Heights, so `SYR`'s claim has somewhere to land.
    let rows = [
        "",
        "&lang=ja",
        "&lang=ar",
        "&crop-labels=true",
        "&political-view=SYR",
        "&political-view=SYR&lang=ja&crop-labels=true&label-size=Large",
    ];
    let mut seen: BTreeMap<Vec<u8>, &str> = BTreeMap::new();
    for row in rows {
        let path = format!("{base}{row}");
        let image = emulator.image(row, &path).await;
        let repeat = emulator.image(row, &path).await;
        assert_eq!(
            image, repeat,
            "case {row:?}: a repeated request must answer with the same bytes"
        );
        if let Some(previous) = seen.insert(image, row) {
            panic!(
                "case {row:?}: this request answered with the image {previous:?} \
                 already served, so the knob changed nothing"
            );
        }
    }
}
