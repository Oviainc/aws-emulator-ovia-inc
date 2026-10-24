// SPDX-License-Identifier: CC0-1.0

//! The political boundaries a map draws, and where the requested political view
//! puts them.
//!
//! `PoliticalView` asks for "a country's own view of a disputed boundary", and
//! the member's documentation names every view the service supports together
//! with the territory each one is a view ON
//! (`smithy_query(query="doc", shape="GetStaticMapRequest", member="PoliticalView")`).
//! That is the whole content of this module: one record per (view, territory)
//! pair, and the rule that turns a record into the boundary line a given view
//! recognises.
//!
//! The rule, stated once: a disputed territory is enclosed by boundary lines
//! either way, and the line that MOVES is the one between the territory and the
//! country that claims it. Under that country's own view the territory lies
//! inside it, so there is no boundary on the claimant's side and the recognised
//! boundary runs along the territory's far edge; under any other view the
//! territory lies outside the claimant, so the boundary runs along the edge
//! facing it. Boundaries are drawn whether or not a view was asked for — a map
//! always has borders — so a view selects BETWEEN boundary lines rather than
//! adding one.

use super::assets::{Envelope, Position};

/// Which side of a disputed territory the country that claims it lies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    North,
    South,
    East,
    West,
}

impl Side {
    /// The opposite side of the same territory.
    pub fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::East => Self::West,
            Self::West => Self::East,
        }
    }

    /// This side's edge of a rectangle, as an index into the corner ring
    /// `[south-west, south-east, north-east, north-west]`: edge `i` runs from
    /// corner `i` to corner `i + 1`.
    fn edge(self) -> usize {
        match self {
            Self::South => 0,
            Self::East => 1,
            Self::North => 2,
            Self::West => 3,
        }
    }
}

/// One territory whose boundary depends on the political view.
///
/// `view` is the `CountryCode` whose view the model documents for this
/// territory, and `territory` is the name that documentation gives it. The
/// four coordinates are the territory's approximate extent in WGS 84, and
/// `claimant_side` is the side of that extent on which the claiming country's
/// undisputed territory lies — which is what decides which edge of it the
/// disputed boundary is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisputedTerritory {
    pub view: &'static str,
    pub territory: &'static str,
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
    pub claimant_side: Side,
}

impl DisputedTerritory {
    /// Whether this territory's extent overlaps `envelope` at all, i.e. whether
    /// its boundary can appear on an image framed by that envelope.
    pub fn overlaps(&self, envelope: Envelope) -> bool {
        self.min_lon <= envelope.max_lon
            && self.max_lon >= envelope.min_lon
            && self.min_lat <= envelope.max_lat
            && self.max_lat >= envelope.min_lat
    }

    /// The boundary this territory contributes under `view`: the three edges of
    /// its extent that are boundaries, as one open polyline.
    ///
    /// The edge left out is the one that is not a boundary under this view: the
    /// claimant's side when the claimant's own view is asked for (the territory
    /// is inside it), the opposite side otherwise.
    pub fn boundary(&self, view: Option<&str>) -> Vec<Position> {
        let ring = [
            (self.min_lon, self.min_lat),
            (self.max_lon, self.min_lat),
            (self.max_lon, self.max_lat),
            (self.min_lon, self.max_lat),
        ];
        let open = if self.claimed_by(view) {
            self.claimant_side
        } else {
            self.claimant_side.opposite()
        };
        let first = open.edge();
        (1..=4).map(|step| ring[(first + step) % 4]).collect()
    }

    /// Whether `view` is the view of the country that claims this territory.
    pub fn claimed_by(&self, view: Option<&str>) -> bool {
        view == Some(self.view)
    }
}

/// Every (view, territory) pair the `PoliticalView` documentation names.
///
/// The view codes and the territory names are that member's documentation
/// verbatim; each extent is the approximate WGS 84 bounding box of the named
/// territory, and each `claimant_side` is the side of it the claiming country
/// lies on.
pub const DISPUTED_TERRITORIES: &[DisputedTerritory] = &[
    DisputedTerritory {
        view: "ARG",
        territory: "Southern Patagonian Ice Field",
        min_lon: -73.6,
        min_lat: -51.5,
        max_lon: -72.8,
        max_lat: -48.3,
        claimant_side: Side::East,
    },
    DisputedTerritory {
        view: "ARG",
        territory: "Tierra Del Fuego",
        min_lon: -68.7,
        min_lat: -55.1,
        max_lon: -66.3,
        max_lat: -52.6,
        claimant_side: Side::East,
    },
    DisputedTerritory {
        view: "ARG",
        territory: "Falkland Islands",
        min_lon: -61.4,
        min_lat: -52.5,
        max_lon: -57.7,
        max_lat: -51.0,
        claimant_side: Side::West,
    },
    DisputedTerritory {
        view: "ARG",
        territory: "South Georgia and South Sandwich Islands",
        min_lon: -38.3,
        min_lat: -59.5,
        max_lon: -26.2,
        max_lat: -53.9,
        claimant_side: Side::West,
    },
    DisputedTerritory {
        view: "EGY",
        territory: "Bir Tawil",
        min_lon: 33.2,
        min_lat: 21.7,
        max_lon: 34.1,
        max_lat: 22.0,
        claimant_side: Side::North,
    },
    DisputedTerritory {
        view: "IND",
        territory: "Gilgit-Baltistan",
        min_lon: 73.5,
        min_lat: 34.3,
        max_lon: 77.1,
        max_lat: 37.1,
        claimant_side: Side::South,
    },
    DisputedTerritory {
        view: "KEN",
        territory: "Ilemi Triangle",
        min_lon: 34.0,
        min_lat: 4.3,
        max_lon: 35.9,
        max_lat: 5.5,
        claimant_side: Side::South,
    },
    DisputedTerritory {
        view: "MAR",
        territory: "Western Sahara",
        min_lon: -17.1,
        min_lat: 20.8,
        max_lon: -8.7,
        max_lat: 27.7,
        claimant_side: Side::North,
    },
    DisputedTerritory {
        view: "RUS",
        territory: "Crimea",
        min_lon: 32.4,
        min_lat: 44.3,
        max_lon: 36.7,
        max_lat: 46.2,
        claimant_side: Side::East,
    },
    DisputedTerritory {
        view: "SDN",
        territory: "Halaib Triangle",
        min_lon: 33.3,
        min_lat: 21.0,
        max_lon: 37.2,
        max_lat: 22.2,
        claimant_side: Side::South,
    },
    DisputedTerritory {
        view: "SRB",
        territory: "Kosovo",
        min_lon: 20.0,
        min_lat: 41.8,
        max_lon: 21.8,
        max_lat: 43.3,
        claimant_side: Side::North,
    },
    DisputedTerritory {
        view: "SRB",
        territory: "Vukovar and Sarengrad Islands",
        min_lon: 18.9,
        min_lat: 45.1,
        max_lon: 19.1,
        max_lat: 45.4,
        claimant_side: Side::East,
    },
    DisputedTerritory {
        view: "SUR",
        territory: "Courantyne Headwaters",
        min_lon: -58.1,
        min_lat: 1.9,
        max_lon: -57.2,
        max_lat: 3.6,
        claimant_side: Side::East,
    },
    DisputedTerritory {
        view: "SUR",
        territory: "Lawa Headwaters",
        min_lon: -54.6,
        min_lat: 2.0,
        max_lon: -53.9,
        max_lat: 3.3,
        claimant_side: Side::West,
    },
    DisputedTerritory {
        view: "SYR",
        territory: "Golan Heights",
        min_lon: 35.6,
        min_lat: 32.6,
        max_lon: 36.0,
        max_lat: 33.3,
        claimant_side: Side::East,
    },
    DisputedTerritory {
        view: "TUR",
        territory: "Cyprus and Northern Cyprus",
        min_lon: 32.2,
        min_lat: 35.0,
        max_lon: 34.6,
        max_lat: 35.7,
        claimant_side: Side::North,
    },
    DisputedTerritory {
        view: "TZA",
        territory: "Lake Malawi",
        min_lon: 33.9,
        min_lat: -12.0,
        max_lon: 35.0,
        max_lat: -9.3,
        claimant_side: Side::North,
    },
    DisputedTerritory {
        view: "URY",
        territory: "Rincon de Artigas",
        min_lon: -56.8,
        min_lat: -31.1,
        max_lon: -56.4,
        max_lat: -30.8,
        claimant_side: Side::South,
    },
    DisputedTerritory {
        view: "VNM",
        territory: "Paracel Islands",
        min_lon: 111.0,
        min_lat: 15.7,
        max_lon: 113.0,
        max_lat: 17.1,
        claimant_side: Side::West,
    },
    DisputedTerritory {
        view: "VNM",
        territory: "Spratly Islands",
        min_lon: 112.0,
        min_lat: 7.0,
        max_lon: 117.3,
        max_lat: 11.6,
        claimant_side: Side::West,
    },
];

/// The boundary polylines an image framed by `envelope` draws under `view`.
///
/// Only the territories the envelope reaches contribute: everything else is off
/// the image. A view the catalog knows no claim for — any other country code —
/// gets the same lines as a request that named no view at all, which is the
/// de-facto boundary.
pub fn boundary_lines(envelope: Envelope, view: Option<&str>) -> Vec<Vec<Position>> {
    DISPUTED_TERRITORIES
        .iter()
        .filter(|territory| territory.overlaps(envelope))
        .map(|territory| territory.boundary(view))
        .collect()
}

/// Every supported view, in catalog order and without repeats: the country
/// codes `PoliticalView` documents.
pub fn supported_views() -> Vec<&'static str> {
    let mut views: Vec<&'static str> = Vec::new();
    for territory in DISPUTED_TERRITORIES {
        if !views.contains(&territory.view) {
            views.push(territory.view);
        }
    }
    views
}
