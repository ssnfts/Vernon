//! Stage 7/7b Solar and Flatten: sun position, sky model, incident radiation,
//! surface temperature vs HDT, panel flattening.
//!
//! The solar chain is `weather_import` / `design_hour` -> `sky_matrix` ->
//! `incident_radiation` -> `surface_balance` -> `hdt_check`; each stage emits
//! a [`vernon_core::Report`] whose metrics the next one gates on. The
//! flatten nodes cut a panel into a disk, flatten it and report the worst
//! length change. Specs: `docs/specs/solar-thermal.md` and
//! `docs/specs/rhino8-mesh-tools.md`.

pub mod bvh;
pub mod flatten;
pub mod lscm;
pub mod radiation;
pub mod sky;
pub mod sparse;
pub mod strain;
pub mod sun;
pub mod thermal;
pub mod topo;
pub mod util;
pub mod weather;

use vernon_graph::Registry;

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    reg.add(weather::WeatherImportNode);
    reg.add(weather::DesignHourNode);
    reg.add(weather::PeakHourNode);
    reg.add(weather::SkyTemperatureNode);
    reg.add(sun::SunPositionNode);
    reg.add(sun::SunPathNode);
    reg.add(sky::SkyMatrixNode);
    reg.add(radiation::IncidentRadiationNode);
    reg.add(radiation::SunExposureNode);
    reg.add(radiation::UvDoseNode);
    reg.add(thermal::MaterialTableNode);
    reg.add(thermal::SurfaceBalanceNode);
    reg.add(thermal::BalanceSweepNode);
    reg.add(thermal::HdtCheckNode);
    reg.add(flatten::SeamCutNode);
    reg.add(flatten::ConformalMapNode);
    reg.add(flatten::StretchAwareNode);
    reg.add(flatten::StrainReportNode);
    reg.add(flatten::UvOverlayNode);
    reg.add(flatten::MapBackNode);
}

#[cfg(test)]
mod tests;
