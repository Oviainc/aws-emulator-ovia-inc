// SPDX-License-Identifier: CC0-1.0

//! The published basemap style's LAYER CATALOGUE.
//!
//! A MapLibre style's layer list is published content of the style itself, not
//! something derivable from the tile layer names: a basemap tile carries a
//! handful of layers (`roads`, `landuse`, `water`, ...) and the style draws
//! each of them with DOZENS of layers — the carriageway, its casing, its
//! shadow, its labels, its route shields, the tunnel and bridge grades of the
//! same network, each its own layer with its own zoom range. So the list lives
//! here as catalog data, and [`crate::state::assets`] turns one entry into the
//! style-document layer object by applying the rules below.
//!
//! What is catalog data and what is derived:
//!
//! * the layer ID, the tile layer it reads, what it draws ([`LayerRole`]), what
//!   switches it on ([`LayerGate`]) and the lowest zoom it is drawn at are
//!   catalog data, one entry per published layer;
//! * everything else is DERIVED from those — the MapLibre layer type, the
//!   geometry it filters to, which palette entry paints it, how wide its lines
//!   are and which sprite icon it anchors — so a layer added here needs no
//!   serializer change, and two layers of one family can never be painted from
//!   two different rules.
//!
//! Source for the layer IDs: the style documents the Maps style-descriptor
//! endpoint publishes for the `Standard`, `Monochrome` and `Hybrid` map styles,
//! which is the same catalog `GetStyleDescriptor`'s own documentation points at
//! ("Style dynamic maps" in the Amazon Location Service Developer Guide). The
//! model does not describe the document's body, so the IDs are catalog data in
//! exactly the way the published sprite iconography in [`crate::state`] is.

use super::{
    ADMIN_LAYER, BUILDING_LAYER, LANDUSE_LAYER, POI_LAYER, ROAD_LAYER, TRAFFIC_FLOW_LAYER,
    TRAFFIC_INCIDENT_LAYER, WATER_LAYER,
};

/// What a published style layer draws. It decides the MapLibre layer type, the
/// geometry role the layer filters to, the paint properties it carries and how
/// its colour is derived from the style's palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerRole {
    /// A polygon fill: a surface of its own, drawn opaque.
    Area,
    /// A polygon fill drawn OVER the ground rather than instead of it, so it
    /// is translucent and what lies under it stays visible.
    Shading,
    /// A polygon raised into a three-dimensional extrusion.
    Extrusion,
    /// The line a way is drawn with.
    Way,
    /// The casing drawn under a way: the same colour darkened and wider, which
    /// is what gives a carriageway its outline.
    Casing,
    /// The shadow drawn under a casing: wider still and translucent, which is
    /// what lifts a bridge off what it crosses.
    Shadow,
    /// A text label.
    Label,
    /// An icon-bearing symbol: a route shield, a point-of-interest pin, a
    /// one-way arrow, an incident marker.
    Icon,
}

impl LayerRole {
    /// The `type` a style document declares this layer as.
    pub fn layer_type(self) -> &'static str {
        match self {
            Self::Area | Self::Shading => "fill",
            Self::Extrusion => "fill-extrusion",
            Self::Way | Self::Casing | Self::Shadow => "line",
            Self::Label | Self::Icon => "symbol",
        }
    }

    /// Whether this layer places symbols.
    pub fn is_symbol(self) -> bool {
        matches!(self, Self::Label | Self::Icon)
    }
}

/// What switches a published layer on.
///
/// Only the knobs that select BETWEEN layers of one tile layer are listed: a
/// knob that brings a whole tile layer with it (traffic, contour lines,
/// hillshading, a travel mode) needs no gate, because a layer is drawn only
/// when the tiles behind it carry the layer it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerGate {
    /// Drawn whenever the tiles behind it carry its tile layer.
    Always,
    /// Drawn unless the request hid points of interest.
    Poi,
    /// Drawn only when the request asked for three-dimensional buildings.
    Buildings3D,
}

/// One published style layer.
#[derive(Debug, Clone, Copy)]
pub struct StyleLayerSpec {
    /// The layer's id, which is also its handle in a style document.
    pub id: &'static str,
    /// The tile layer this style layer reads.
    pub source_layer: &'static str,
    pub role: LayerRole,
    pub gate: LayerGate,
    /// The lowest zoom the layer is drawn at.
    pub min_zoom: u32,
}

/// One catalog entry, written out so the table below is one line per layer.
const fn layer(
    id: &'static str,
    source_layer: &'static str,
    role: LayerRole,
    gate: LayerGate,
    min_zoom: u32,
) -> StyleLayerSpec {
    StyleLayerSpec {
        id,
        source_layer,
        role,
        gate,
        min_zoom,
    }
}

/// The tile layer the `ContourDensity` knob adds, and the one `Hillshade` adds.
/// Both are `TileAdditionalFeature` members in snake case, which is the one
/// place the layer name is decided (see
/// [`crate::state::assets::additional_feature_layer`]).
const CONTOUR_TILE_LAYER: &str = "contour_lines";
const HILLSHADE_TILE_LAYER: &str = "hillshade";

/// The tile layer the `Truck` travel mode adds: the heavy-goods network, which
/// the truck carriageways and restriction shields are drawn from. The `Transit`
/// travel mode adds the public-transport network the same way.
const LOGISTICS_TILE_LAYER: &str = "logistics";
const TRANSIT_TILE_LAYER: &str = "transit";

use LayerGate::{Always, Buildings3D, Poi};
use LayerRole::{Area, Casing, Extrusion, Icon, Label, Shading, Shadow, Way};

/// Every layer the published basemap styles declare, in DRAW ORDER: the ground
/// first (which the document carries as its `background` layer and is therefore
/// not listed here), then the surfaces, the water, the network from the bottom
/// grade up, the buildings, the boundaries and finally the labels and symbols.
pub const BASEMAP_LAYERS: &[StyleLayerSpec] = &[
    // Land use: the surfaces everything else is drawn on.
    layer("landuse_glacier", LANDUSE_LAYER, Area, Always, 2),
    layer("landuse_desert", LANDUSE_LAYER, Area, Always, 2),
    layer("landuse_beach", LANDUSE_LAYER, Area, Always, 10),
    layer("landuse_greenspace", LANDUSE_LAYER, Area, Always, 6),
    layer("landuse_urban", LANDUSE_LAYER, Area, Always, 6),
    layer("landuse_misc", LANDUSE_LAYER, Area, Always, 10),
    layer("landuse_reservation", LANDUSE_LAYER, Area, Always, 6),
    layer("landuse_aerodrome", LANDUSE_LAYER, Area, Always, 10),
    layer("landuse_runway", LANDUSE_LAYER, Area, Always, 11),
    layer("landuse_public-complex", LANDUSE_LAYER, Area, Always, 12),
    layer("landuse_public-complex-above", LANDUSE_LAYER, Area, Always, 14),
    layer("poi_landuse_park_lowzoom", LANDUSE_LAYER, Area, Always, 4),
    layer("poi_landuse_park", LANDUSE_LAYER, Area, Always, 10),
    layer("poi_landuse_public_complex", LANDUSE_LAYER, Area, Always, 12),
    // Terrain, which is only in the tiles when the request asked for it.
    layer("hillshade", HILLSHADE_TILE_LAYER, Shading, Always, 0),
    layer("contour_lines", CONTOUR_TILE_LAYER, Way, Always, 9),
    // Water.
    layer("water_ocean", WATER_LAYER, Area, Always, 0),
    layer("water_fill", WATER_LAYER, Area, Always, 0),
    layer("water_pier-polygon", WATER_LAYER, Area, Always, 14),
    layer("water_waterway", WATER_LAYER, Way, Always, 8),
    layer("water_pier-line", WATER_LAYER, Way, Always, 14),
    // Rail and ferry.
    layer("rail_road", ROAD_LAYER, Way, Always, 8),
    layer("rail_tie", ROAD_LAYER, Way, Always, 12),
    layer("rail_tunnel", ROAD_LAYER, Way, Always, 12),
    layer("rail_tie_tunnel", ROAD_LAYER, Way, Always, 12),
    layer("rail_bridge", ROAD_LAYER, Way, Always, 12),
    layer("rail_tie_bridge", ROAD_LAYER, Way, Always, 12),
    layer("ferry_line", ROAD_LAYER, Way, Always, 7),
    // Tunnels: the network under the ground, drawn before the surface.
    layer("tunnel_motorway-trunk-casing", ROAD_LAYER, Casing, Always, 12),
    layer("tunnel_motorway-trunk", ROAD_LAYER, Way, Always, 12),
    layer(
        "tunnel_motorway-trunk-link-casing",
        ROAD_LAYER,
        Casing,
        Always,
        12,
    ),
    layer("tunnel_motorway-trunk-link", ROAD_LAYER, Way, Always, 12),
    layer("tunnel_primary-casing", ROAD_LAYER, Casing, Always, 12),
    layer("tunnel_primary", ROAD_LAYER, Way, Always, 12),
    layer("tunnel_primary-link-casing", ROAD_LAYER, Casing, Always, 12),
    layer("tunnel_primary-link", ROAD_LAYER, Way, Always, 12),
    layer("tunnel_tertiary-casing", ROAD_LAYER, Casing, Always, 12),
    layer("tunnel_tertiary", ROAD_LAYER, Way, Always, 12),
    layer("tunnel_tertiary-link-casing", ROAD_LAYER, Casing, Always, 12),
    layer("tunnel_tertiary-link", ROAD_LAYER, Way, Always, 12),
    layer("tunnel_minor-casing", ROAD_LAYER, Casing, Always, 13),
    layer("tunnel_minor", ROAD_LAYER, Way, Always, 13),
    layer("tunnel_service-casing", ROAD_LAYER, Casing, Always, 14),
    layer("tunnel_service", ROAD_LAYER, Way, Always, 14),
    // Surface carriageways, from the highest grade down.
    layer("road_motorway-trunk-casing", ROAD_LAYER, Casing, Always, 5),
    layer("road_motorway-trunk", ROAD_LAYER, Way, Always, 5),
    layer(
        "road_motorway-trunk-link-casing",
        ROAD_LAYER,
        Casing,
        Always,
        10,
    ),
    layer("road_motorway-trunk-link", ROAD_LAYER, Way, Always, 10),
    layer("road_primary-casing", ROAD_LAYER, Casing, Always, 7),
    layer("road_primary", ROAD_LAYER, Way, Always, 7),
    layer("road_primary-link-casing", ROAD_LAYER, Casing, Always, 10),
    layer("road_primary-link", ROAD_LAYER, Way, Always, 10),
    layer("road_tertiary-casing", ROAD_LAYER, Casing, Always, 10),
    layer("road_tertiary", ROAD_LAYER, Way, Always, 10),
    layer("road_tertiary-link-casing", ROAD_LAYER, Casing, Always, 12),
    layer("road_tertiary-link", ROAD_LAYER, Way, Always, 12),
    layer("road_minor-casing", ROAD_LAYER, Casing, Always, 12),
    layer("road_minor", ROAD_LAYER, Way, Always, 12),
    layer("road_service-casing", ROAD_LAYER, Casing, Always, 14),
    layer("road_service", ROAD_LAYER, Way, Always, 14),
    layer("road_path", ROAD_LAYER, Way, Always, 14),
    // Airport movement areas.
    layer("airport_runway-casing", ROAD_LAYER, Casing, Always, 11),
    layer("airport_runway", ROAD_LAYER, Way, Always, 11),
    // Bridges: the network above the ground, drawn over the surface and lifted
    // off it by a shadow.
    layer("bridge_motorway-trunk-shadow", ROAD_LAYER, Shadow, Always, 12),
    layer("bridge_motorway-trunk-casing", ROAD_LAYER, Casing, Always, 12),
    layer("bridge_motorway-trunk", ROAD_LAYER, Way, Always, 12),
    layer(
        "bridge_motorway-trunk-link-shadow",
        ROAD_LAYER,
        Shadow,
        Always,
        12,
    ),
    layer(
        "bridge_motorway-trunk-link-casing",
        ROAD_LAYER,
        Casing,
        Always,
        12,
    ),
    layer("bridge_motorway-trunk-link", ROAD_LAYER, Way, Always, 12),
    layer(
        "bridge_primary-secondary-shadow",
        ROAD_LAYER,
        Shadow,
        Always,
        12,
    ),
    layer("bridge_primary-casing", ROAD_LAYER, Casing, Always, 12),
    layer("bridge_primary", ROAD_LAYER, Way, Always, 12),
    layer(
        "bridge_primary-secondary-link-shadow",
        ROAD_LAYER,
        Shadow,
        Always,
        12,
    ),
    layer("bridge_primary-link-casing", ROAD_LAYER, Casing, Always, 12),
    layer("bridge_primary-link", ROAD_LAYER, Way, Always, 12),
    layer("bridge_tertiary-shadow", ROAD_LAYER, Shadow, Always, 12),
    layer("bridge_tertiary-casing", ROAD_LAYER, Casing, Always, 12),
    layer("bridge_tertiary", ROAD_LAYER, Way, Always, 12),
    layer("bridge_tertiary-link-shadow", ROAD_LAYER, Shadow, Always, 12),
    layer("bridge_tertiary-link-casing", ROAD_LAYER, Casing, Always, 12),
    layer("bridge_tertiary-link", ROAD_LAYER, Way, Always, 12),
    layer("bridge_minor-shadow", ROAD_LAYER, Shadow, Always, 13),
    layer("bridge_minor-casing", ROAD_LAYER, Casing, Always, 13),
    layer("bridge_minor", ROAD_LAYER, Way, Always, 13),
    layer("bridge_service-shadow", ROAD_LAYER, Shadow, Always, 14),
    layer("bridge_service-casing", ROAD_LAYER, Casing, Always, 14),
    layer("bridge_service", ROAD_LAYER, Way, Always, 14),
    layer("bridge_path-under", ROAD_LAYER, Shadow, Always, 14),
    layer("bridge_path", ROAD_LAYER, Way, Always, 14),
    // The heavy-goods network, which the `Truck` travel mode brings into the
    // tiles, and the public-transport network the `Transit` travel mode does.
    layer("road_truck-casing", LOGISTICS_TILE_LAYER, Casing, Always, 10),
    layer("road_truck", LOGISTICS_TILE_LAYER, Way, Always, 10),
    layer(
        "tunnel_truck-casing",
        LOGISTICS_TILE_LAYER,
        Casing,
        Always,
        12,
    ),
    layer("tunnel_truck", LOGISTICS_TILE_LAYER, Way, Always, 12),
    layer(
        "bridge_truck-casing",
        LOGISTICS_TILE_LAYER,
        Casing,
        Always,
        12,
    ),
    layer("bridge_truck", LOGISTICS_TILE_LAYER, Way, Always, 12),
    layer("transit", TRANSIT_TILE_LAYER, Way, Always, 8),
    // The traffic-coloured network, which the `Traffic` knob brings into the
    // tiles. `_left_` is the same carriageway drawn for left-hand traffic.
    layer(
        "road_traffic_motorway-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        6,
    ),
    layer("road_traffic_motorway", TRAFFIC_FLOW_LAYER, Way, Always, 6),
    layer(
        "road_traffic_trunk-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        6,
    ),
    layer("road_traffic_trunk", TRAFFIC_FLOW_LAYER, Way, Always, 6),
    layer(
        "road_traffic_primary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        8,
    ),
    layer("road_traffic_primary", TRAFFIC_FLOW_LAYER, Way, Always, 8),
    layer(
        "road_traffic_tertiary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        10,
    ),
    layer("road_traffic_tertiary", TRAFFIC_FLOW_LAYER, Way, Always, 10),
    layer(
        "road_traffic_minor-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer("road_traffic_minor", TRAFFIC_FLOW_LAYER, Way, Always, 12),
    layer(
        "road_traffic_link-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        10,
    ),
    layer("road_traffic_link", TRAFFIC_FLOW_LAYER, Way, Always, 10),
    layer(
        "road_traffic_left_trunk-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        6,
    ),
    layer("road_traffic_left_trunk", TRAFFIC_FLOW_LAYER, Way, Always, 6),
    layer(
        "road_traffic_left_primary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        8,
    ),
    layer(
        "road_traffic_left_primary",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        8,
    ),
    layer(
        "road_traffic_left_tertiary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        10,
    ),
    layer(
        "road_traffic_left_tertiary",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        10,
    ),
    layer(
        "road_traffic_left_minor-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer(
        "road_traffic_left_minor",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        12,
    ),
    layer(
        "tunnel_traffic_motorway-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer(
        "tunnel_traffic_motorway",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        12,
    ),
    layer(
        "tunnel_traffic_trunk-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer("tunnel_traffic_trunk", TRAFFIC_FLOW_LAYER, Way, Always, 12),
    layer(
        "tunnel_traffic_primary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer("tunnel_traffic_primary", TRAFFIC_FLOW_LAYER, Way, Always, 12),
    layer(
        "tunnel_traffic_tertiary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer(
        "tunnel_traffic_tertiary",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        12,
    ),
    layer(
        "tunnel_traffic_minor-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        13,
    ),
    layer("tunnel_traffic_minor", TRAFFIC_FLOW_LAYER, Way, Always, 13),
    layer(
        "tunnel_traffic_link-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer("tunnel_traffic_link", TRAFFIC_FLOW_LAYER, Way, Always, 12),
    layer(
        "tunnel_traffic_left_trunk-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer(
        "tunnel_traffic_left_trunk",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        12,
    ),
    layer(
        "tunnel_traffic_left_primary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer(
        "tunnel_traffic_left_primary",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        12,
    ),
    layer(
        "tunnel_traffic_left_tertiary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer(
        "tunnel_traffic_left_tertiary",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        12,
    ),
    layer(
        "tunnel_traffic_left_minor-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        13,
    ),
    layer(
        "tunnel_traffic_left_minor",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        13,
    ),
    layer(
        "bridge_traffic_motorway-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer(
        "bridge_traffic_motorway",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        12,
    ),
    layer(
        "bridge_traffic_trunk-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer("bridge_traffic_trunk", TRAFFIC_FLOW_LAYER, Way, Always, 12),
    layer(
        "bridge_traffic_primary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer("bridge_traffic_primary", TRAFFIC_FLOW_LAYER, Way, Always, 12),
    layer(
        "bridge_traffic_tertiary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer(
        "bridge_traffic_tertiary",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        12,
    ),
    layer(
        "bridge_traffic_minor-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        13,
    ),
    layer("bridge_traffic_minor", TRAFFIC_FLOW_LAYER, Way, Always, 13),
    layer(
        "bridge_traffic_link-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer("bridge_traffic_link", TRAFFIC_FLOW_LAYER, Way, Always, 12),
    layer(
        "bridge_traffic_left_trunk-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer(
        "bridge_traffic_left_trunk",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        12,
    ),
    layer(
        "bridge_traffic_left_primary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer(
        "bridge_traffic_left_primary",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        12,
    ),
    layer(
        "bridge_traffic_left_tertiary-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        12,
    ),
    layer(
        "bridge_traffic_left_tertiary",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        12,
    ),
    layer(
        "bridge_traffic_left_minor-casing",
        TRAFFIC_FLOW_LAYER,
        Casing,
        Always,
        13,
    ),
    layer(
        "bridge_traffic_left_minor",
        TRAFFIC_FLOW_LAYER,
        Way,
        Always,
        13,
    ),
    layer("traffic_flow", TRAFFIC_FLOW_LAYER, Way, Always, 6),
    layer("traffic_incidents", TRAFFIC_INCIDENT_LAYER, Way, Always, 6),
    // Buildings.
    layer("building_fill", BUILDING_LAYER, Area, Always, 14),
    layer("building_fill-above-tunnel", BUILDING_LAYER, Area, Always, 14),
    layer("building_line", BUILDING_LAYER, Way, Always, 14),
    layer("building_extrusion", BUILDING_LAYER, Extrusion, Buildings3D, 15),
    layer("building_line_extrusion", BUILDING_LAYER, Way, Buildings3D, 15),
    // Administrative boundaries. A `-disputed` line is the alternative
    // alignment a political view recognises, and its `-mask` hides the
    // de-facto line under it.
    layer("admin_country", ADMIN_LAYER, Way, Always, 1),
    layer("admin_country-disputed", ADMIN_LAYER, Way, Always, 1),
    layer("admin_country-disputed-mask", ADMIN_LAYER, Way, Always, 1),
    layer("admin_region", ADMIN_LAYER, Way, Always, 3),
    layer("admin_region-disputed", ADMIN_LAYER, Way, Always, 3),
    layer("admin_region-disputed-mask", ADMIN_LAYER, Way, Always, 3),
    // Labels and symbols, drawn over everything.
    layer("water_label-point", WATER_LAYER, Label, Always, 4),
    layer("water_label-line", WATER_LAYER, Label, Always, 8),
    layer("nationalpark_label_lowzoom", LANDUSE_LAYER, Label, Always, 4),
    layer("nationalpark_label", LANDUSE_LAYER, Label, Always, 8),
    layer("landuse_reservation_label", LANDUSE_LAYER, Label, Always, 8),
    layer("admin_label_continent", ADMIN_LAYER, Label, Always, 1),
    layer("admin_label_country", ADMIN_LAYER, Label, Always, 2),
    layer("admin_label_region", ADMIN_LAYER, Label, Always, 4),
    layer("place_label_locality_city", POI_LAYER, Label, Always, 4),
    layer("place_label_locality_settlement", POI_LAYER, Label, Always, 8),
    layer(
        "place_label_locality_settlement_small",
        POI_LAYER,
        Label,
        Always,
        11,
    ),
    layer("ferry_label", ROAD_LAYER, Label, Always, 10),
    layer("airport_label", ROAD_LAYER, Label, Always, 11),
    layer("road_label_large", ROAD_LAYER, Label, Always, 10),
    layer("road_label_medium", ROAD_LAYER, Label, Always, 12),
    layer("road_label_small", ROAD_LAYER, Label, Always, 14),
    layer("road_label_service", ROAD_LAYER, Label, Always, 15),
    layer("building_label_number", BUILDING_LAYER, Label, Always, 17),
    layer("tunnel_oneway-arrow", ROAD_LAYER, Icon, Always, 15),
    layer("road_oneway-arrow", ROAD_LAYER, Icon, Always, 15),
    layer("bridge_oneway-arrow", ROAD_LAYER, Icon, Always, 15),
    layer(
        "traffic_incident_icons",
        TRAFFIC_INCIDENT_LAYER,
        Icon,
        Always,
        6,
    ),
    // Route shields, one layer per shield design the published iconography
    // carries, because each design is anchored to a different sprite.
    layer("shield_network", ROAD_LAYER, Icon, Always, 6),
    layer("shield_generic", ROAD_LAYER, Icon, Always, 6),
    layer("shield_au", ROAD_LAYER, Icon, Always, 6),
    layer("shield_autostrade", ROAD_LAYER, Icon, Always, 6),
    layer("shield_be", ROAD_LAYER, Icon, Always, 6),
    layer("shield_blue", ROAD_LAYER, Icon, Always, 6),
    layer("shield_blue-pentagon", ROAD_LAYER, Icon, Always, 6),
    layer("shield_blue-red-kite", ROAD_LAYER, Icon, Always, 6),
    layer("shield_green", ROAD_LAYER, Icon, Always, 6),
    layer("shield_green-hexagon", ROAD_LAYER, Icon, Always, 6),
    layer("shield_green-red-kite", ROAD_LAYER, Icon, Always, 6),
    layer("shield_green-square-kite", ROAD_LAYER, Icon, Always, 6),
    layer("shield_in", ROAD_LAYER, Icon, Always, 6),
    layer("shield_it-blue", ROAD_LAYER, Icon, Always, 6),
    layer("shield_jp-expressway", ROAD_LAYER, Icon, Always, 6),
    layer("shield_kr-expressway", ROAD_LAYER, Icon, Always, 6),
    layer("shield_mashreq", ROAD_LAYER, Icon, Always, 6),
    layer("shield_md", ROAD_LAYER, Icon, Always, 6),
    layer("shield_mx", ROAD_LAYER, Icon, Always, 6),
    layer("shield_my", ROAD_LAYER, Icon, Always, 6),
    layer("shield_na", ROAD_LAYER, Icon, Always, 6),
    layer("shield_nl-s-road", ROAD_LAYER, Icon, Always, 6),
    layer("shield_red", ROAD_LAYER, Icon, Always, 6),
    layer("shield_us-state", ROAD_LAYER, Icon, Always, 6),
    layer("shield_ve", ROAD_LAYER, Icon, Always, 6),
    layer("shield_white-kite", ROAD_LAYER, Icon, Always, 6),
    layer("shield_white-red-square-kite", ROAD_LAYER, Icon, Always, 6),
    layer("shield_white-square-kite", ROAD_LAYER, Icon, Always, 6),
    layer("shield_yellow", ROAD_LAYER, Icon, Always, 6),
    // The heavy-goods restriction signs, which come with the `Truck` network.
    layer(
        "shield_truck-hazmat",
        LOGISTICS_TILE_LAYER,
        Icon,
        Always,
        10,
    ),
    layer(
        "shield_truck-hazmat-jp",
        LOGISTICS_TILE_LAYER,
        Icon,
        Always,
        10,
    ),
    layer(
        "shield_truck-hazmat-us",
        LOGISTICS_TILE_LAYER,
        Icon,
        Always,
        10,
    ),
    layer(
        "shield_truck-restriction",
        LOGISTICS_TILE_LAYER,
        Icon,
        Always,
        10,
    ),
    layer(
        "shield_truck-restriction-jp",
        LOGISTICS_TILE_LAYER,
        Icon,
        Always,
        10,
    ),
    layer(
        "shield_truck-restriction-us",
        LOGISTICS_TILE_LAYER,
        Icon,
        Always,
        10,
    ),
    // Points of interest, one layer per category band the published style
    // reveals them in.
    layer("poi_100_food_drink", POI_LAYER, Icon, Poi, 15),
    layer("poi_200_going_out_entertainment", POI_LAYER, Icon, Poi, 15),
    layer("poi_400_transit_logistics", POI_LAYER, Icon, Poi, 14),
    layer("poi_600_shopping", POI_LAYER, Icon, Poi, 15),
    layer("poi_700_business_services_logistics", POI_LAYER, Icon, Poi, 16),
];

/// The sprite icon a symbol layer anchors to when a feature carries no icon
/// name of its own.
///
/// Every name answered here is an icon the published sheets really carry (see
/// the iconography in [`crate::state`]), so a client that falls back to it
/// finds it in the sprite the same document points at.
pub fn fallback_icon(spec: &StyleLayerSpec) -> &'static str {
    if spec.id.starts_with("shield_truck-") {
        "shield-hgv-general_truck_restriction"
    } else if spec.id.starts_with("shield_") {
        "shield-rectangle-white"
    } else if spec.id.ends_with("oneway-arrow") {
        "oneway"
    } else if spec.source_layer == TRAFFIC_INCIDENT_LAYER {
        "congestion"
    } else {
        "other"
    }
}

/// How wide, in style pixels, a line layer draws at its lowest zoom.
///
/// The width is the GRADE of the way the layer draws, which the layer's id
/// states: a motorway is drawn wider than a service road at the same zoom. A
/// casing adds [`CASING_MARGIN`] on each side, which is what makes it an
/// outline, and a shadow adds that again.
pub fn line_width(spec: &StyleLayerSpec) -> f64 {
    let grade = if spec.id.contains("motorway") || spec.id.contains("trunk") {
        2.0
    } else if spec.id.contains("primary") {
        1.6
    } else if spec.id.contains("tertiary") || spec.id.contains("runway") {
        1.3
    } else if spec.id.contains("service") || spec.id.contains("path") {
        0.8
    } else if spec.id.contains("_tie") || spec.id.contains("waterway") {
        0.6
    } else {
        1.0
    };
    match spec.role {
        LayerRole::Casing => grade + CASING_MARGIN,
        LayerRole::Shadow => grade + 2.0 * CASING_MARGIN,
        _ => grade,
    }
}

/// Whether this layer draws the alternative boundary alignment a POLITICAL
/// VIEW recognises rather than the de-facto one.
///
/// Such a layer is part of every published document — a client switches views
/// without fetching another style — so what the requested view decides is the
/// features it selects, not whether it is declared.
pub fn is_disputed(spec: &StyleLayerSpec) -> bool {
    spec.id.contains(DISPUTED_SUFFIX)
}

/// The id suffix a disputed-alignment layer carries.
const DISPUTED_SUFFIX: &str = "-disputed";

/// Whether this layer MASKS what is drawn under it rather than drawing
/// something of its own: the de-facto boundary a political view replaces is
/// hidden by a line in the ground's colour drawn over it.
pub fn is_mask(spec: &StyleLayerSpec) -> bool {
    spec.id.ends_with(MASK_SUFFIX)
}

/// The id suffix a masking layer carries.
const MASK_SUFFIX: &str = "-mask";

/// How much wider than the way it outlines a casing is drawn.
const CASING_MARGIN: f64 = 2.0;
