use anyhow::{Context as _, bail};
use koharu_desktop::{CanvasState, Desktop, Frame, TransformFrame};
use koharu_rasterizer::ResourceId;
use koharu_scene::{EntityId, RasterLayerKind, Revision};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{
    AppHandle, Cef, State,
    ipc::{Channel, IpcResponse},
};

use super::{
    ChannelExt as _, Error,
    processing::{self, JobChannel, JobId, Processing},
    project::{CurrentProject, Page, Project, RasterStrokeMode},
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, Type)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CanvasRasterMode {
    Paint,
    Removal,
    Repair,
    BarCensor,
    MosaicCensor,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RasterShape {
    Oval { frame: Frame },
    Rectangle { frame: Frame },
}

#[derive(Clone, Copy, Debug, Deserialize, Type)]
pub struct PaintBrush {
    pub diameter: f32,
    pub color: [u8; 4],
}

#[derive(Clone, Copy, Debug, Serialize, Type)]
pub struct LayerCommit {
    pub revision: Revision,
    pub layer: EntityId,
}

#[derive(Type)]
#[specta(transparent)]
pub(crate) struct CanvasBytes(#[specta(type = Vec<u8>)] Vec<u8>);

#[derive(Clone, Copy, Deserialize, Type)]
#[specta(transparent)]
pub(crate) struct CanvasGeneration(#[specta(type = f64)] u64);

#[derive(Clone, Debug, Serialize, Type)]
pub struct CanvasPagePreparation {
    pub revision: Revision,
    pub page: Page,
}

impl IpcResponse for CanvasBytes {
    fn body(self) -> tauri::Result<tauri::ipc::InvokeResponseBody> {
        Ok(self.0.into())
    }
}

#[derive(Default)]
pub(crate) struct CanvasChannel {
    pub(crate) channel: Mutex<Option<Channel<CanvasState>>>,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_canvas_manifest(
    generation: CanvasGeneration,
    desktop: State<'_, Desktop>,
) -> Result<CanvasBytes, Error> {
    Ok(CanvasBytes(desktop.frame_manifest_bytes(generation.0)?))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_canvas_resource(
    generation: CanvasGeneration,
    resource: String,
    desktop: State<'_, Desktop>,
) -> Result<CanvasBytes, Error> {
    let resource = resource
        .parse::<ResourceId>()
        .context("canvas resource id is invalid")?;
    Ok(CanvasBytes(
        desktop.frame_resource_bytes(generation.0, resource)?,
    ))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn prepare_canvas_page(
    page: EntityId,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
) -> Result<Option<CanvasPagePreparation>, Error> {
    let (snapshot, prepared_page) = {
        let project = project.project.lock().await;
        let project = project.as_ref().context("no project is open")?;
        let snapshot = project.snapshot();
        let prepared_page = Project::page(&snapshot, page)?;
        (snapshot, prepared_page)
    };
    let revision = snapshot.revision();
    Ok(desktop
        .prepare_page(&snapshot, page)
        .await?
        .then_some(CanvasPagePreparation {
            revision,
            page: prepared_page,
        }))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_canvas_page_manifest(
    page: EntityId,
    revision: Revision,
    desktop: State<'_, Desktop>,
) -> Result<CanvasBytes, Error> {
    Ok(CanvasBytes(desktop.page_manifest_bytes(page, revision)?))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn get_canvas_page_resource(
    page: EntityId,
    revision: Revision,
    resource: String,
    desktop: State<'_, Desktop>,
) -> Result<CanvasBytes, Error> {
    let resource = resource
        .parse::<ResourceId>()
        .context("canvas resource id is invalid")?;
    Ok(CanvasBytes(
        desktop.page_resource_bytes(page, revision, resource)?,
    ))
}

#[tracing::instrument(
    target = "koharu_metrics",
    name = "text_box_added",
    skip_all,
    fields(origin = "user", entity_count = 1_u64)
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn add_text_box(
    frame: Frame,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    let (commit, page, layer) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let page = project
            .active_page()
            .context("the project has no active page")?;
        let (commit, layer) = project.add_text_box(page, frame).await?;
        project.record_commit(&commit);
        (commit, project.active_page(), layer)
    };
    desktop.synchronize(&commit.snapshot, page, &commit).await?;
    canvas_channel.channel.publish(desktop.canvas_state());
    Ok(LayerCommit {
        revision: commit.revision,
        layer,
    })
}

#[tracing::instrument(
    target = "koharu_metrics",
    name = "raster_brush_committed",
    skip_all,
    fields(origin = "user", mode = ?mode, point_count = points.len(), size = f64::from(brush.diameter)),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_raster_brush(
    expected_revision: Revision,
    layer: Option<EntityId>,
    mode: CanvasRasterMode,
    points: Vec<Point>,
    brush: PaintBrush,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    let (kind, color) = raster_style(mode, brush.color);
    commit_raster_edit(
        expected_revision,
        layer,
        points,
        brush.diameter,
        color,
        RasterStrokeMode::Paint,
        kind,
        &desktop,
        &project,
        &canvas_channel,
    )
    .await
}

#[tracing::instrument(
    target = "koharu_metrics",
    name = "raster_shape_committed",
    skip_all,
    fields(origin = "user", mode = ?mode, shape = ?shape),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_raster_shape(
    expected_revision: Revision,
    layer: Option<EntityId>,
    mode: CanvasRasterMode,
    shape: RasterShape,
    color: [u8; 4],
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    let (width, height) = {
        let project = project.project.lock().await;
        let project = project.as_ref().context("no project is open")?;
        let snapshot = project.snapshot();
        ensure_revision(snapshot.revision(), expected_revision)?;
        let page = project
            .active_page()
            .context("the project has no active page")?;
        let page_value = snapshot.page(page)?.page()?;
        (
            page_value.width.round() as u32,
            page_value.height.round() as u32,
        )
    };
    let (points, diameter) = shape_stroke(width, height, shape)?;
    let (kind, color) = raster_style(mode, color);
    commit_raster_edit(
        expected_revision,
        layer,
        points,
        diameter,
        color,
        RasterStrokeMode::Paint,
        kind,
        &desktop,
        &project,
        &canvas_channel,
    )
    .await
}

#[tracing::instrument(
    target = "koharu_metrics",
    name = "raster_erase_committed",
    skip_all,
    fields(origin = "user", mode = ?mode, point_count = points.len(), size = f64::from(diameter)),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_raster_erase(
    expected_revision: Revision,
    layer: EntityId,
    mode: CanvasRasterMode,
    points: Vec<Point>,
    diameter: f32,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    let (kind, _) = raster_style(mode, [0; 4]);
    commit_raster_edit(
        expected_revision,
        Some(layer),
        points,
        diameter,
        [0; 4],
        RasterStrokeMode::Erase,
        kind,
        &desktop,
        &project,
        &canvas_channel,
    )
    .await
}

#[tracing::instrument(
    target = "koharu_metrics",
    name = "fill_committed",
    skip_all,
    fields(origin = "user", tolerance = tolerance),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_fill(
    expected_revision: Revision,
    layer: EntityId,
    point: Point,
    color: [u8; 4],
    tolerance: u8,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    let (commit, page, revision, layer) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        ensure_revision(project.snapshot().revision(), expected_revision)?;
        let page = project
            .active_page()
            .context("the project has no active page")?;
        let (commit, layer) = project
            .apply_raster_fill(
                page,
                layer,
                koharu_scene::Point {
                    x: point.x,
                    y: point.y,
                },
                color,
                tolerance,
            )
            .await?;
        let revision = if let Some(commit) = &commit {
            project.record_commit(commit);
            commit.revision
        } else {
            project.snapshot().revision()
        };
        (commit, project.active_page(), revision, layer)
    };
    if let Some(commit) = commit {
        desktop.synchronize(&commit.snapshot, page, &commit).await?;
        canvas_channel.channel.publish(desktop.canvas_state());
    }
    Ok(LayerCommit { revision, layer })
}

#[allow(clippy::too_many_arguments)]
async fn commit_raster_edit(
    expected_revision: Revision,
    layer: Option<EntityId>,
    points: Vec<Point>,
    diameter: f32,
    color: [u8; 4],
    mode: RasterStrokeMode,
    new_layer_kind: RasterLayerKind,
    desktop: &Desktop,
    project: &CurrentProject,
    canvas_channel: &CanvasChannel,
) -> Result<LayerCommit, Error> {
    let (commit, page, element) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        ensure_revision(project.snapshot().revision(), expected_revision)?;
        let page = project
            .active_page()
            .context("the project has no active page")?;
        let (commit, element) = project
            .apply_raster_stroke(
                page,
                layer,
                mode,
                new_layer_kind,
                color,
                diameter,
                points
                    .into_iter()
                    .map(|point| koharu_scene::Point {
                        x: point.x,
                        y: point.y,
                    })
                    .collect(),
            )
            .await?;
        project.record_commit(&commit);
        (commit, project.active_page(), element)
    };
    desktop.synchronize(&commit.snapshot, page, &commit).await?;
    canvas_channel.channel.publish(desktop.canvas_state());
    Ok(LayerCommit {
        revision: commit.revision,
        layer: element,
    })
}

#[tracing::instrument(
    target = "koharu_metrics",
    name = "transform_committed",
    skip_all,
    fields(origin = "user", entity_count = elements.len()),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_transform(
    expected_revision: Revision,
    elements: Vec<TransformFrame>,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<Option<Revision>, Error> {
    let geometries = desktop.transform_geometries(expected_revision, &elements)?;
    if geometries.is_empty() {
        return Ok(None);
    }
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        ensure_revision(project.snapshot().revision(), expected_revision)?;
        let commit = project.set_geometries(geometries).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    desktop.synchronize(&commit.snapshot, page, &commit).await?;
    canvas_channel.channel.publish(desktop.canvas_state());
    Ok(Some(commit.revision))
}

#[tracing::instrument(
    target = "koharu_metrics",
    name = "repair_requested",
    skip_all,
    fields(origin = "user")
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn repair_inpaint(
    expected_revision: Revision,
    layer: EntityId,
    handle: AppHandle<Cef>,
    project: State<'_, CurrentProject>,
    processing: State<'_, Processing>,
    job_channel: State<'_, JobChannel>,
) -> Result<JobId, Error> {
    let (snapshot, page, mask) = {
        let project = project.project.lock().await;
        let project = project.as_ref().context("no project is open")?;
        ensure_revision(project.snapshot().revision(), expected_revision)?;
        let page = project
            .active_page()
            .context("the project has no active page")?;
        let mask = project.repair_mask(page, layer).await?;
        (project.snapshot(), page, mask)
    };
    processing::start_process(
        handle,
        snapshot,
        koharu_pipeline::Scope::Pages(vec![page]),
        koharu_pipeline::Operation::Only {
            stage: koharu_pipeline::Stage::Inpainting,
        },
        Some(mask),
        &processing,
        &job_channel,
    )
}

fn raster_style(mode: CanvasRasterMode, paint: [u8; 4]) -> (RasterLayerKind, [u8; 4]) {
    match mode {
        CanvasRasterMode::Paint => (RasterLayerKind::Paint, paint),
        CanvasRasterMode::Removal => (RasterLayerKind::Segmentation, [168, 85, 247, 255]),
        CanvasRasterMode::Repair => (RasterLayerKind::Repair, [14, 165, 233, 255]),
        CanvasRasterMode::BarCensor => (RasterLayerKind::Censor, [0, 255, 0, 180]),
        CanvasRasterMode::MosaicCensor => (RasterLayerKind::Mosaic, [0, 255, 0, 180]),
    }
}

fn shape_stroke(width: u32, height: u32, shape: RasterShape) -> anyhow::Result<(Vec<Point>, f32)> {
    if width == 0 || height == 0 {
        bail!("page dimensions must be positive");
    }
    let (frame, oval) = match shape {
        RasterShape::Oval { frame } => (frame, true),
        RasterShape::Rectangle { frame } => (frame, false),
    };
    if ![
        frame.x,
        frame.y,
        frame.width,
        frame.height,
        frame.angle_degrees,
    ]
    .into_iter()
    .all(f32::is_finite)
    {
        bail!("raster shape frame must be finite");
    }
    if frame.width <= 0.0 || frame.height <= 0.0 {
        bail!("raster shape dimensions must be positive");
    }

    let frame_x = f64::from(frame.x);
    let frame_y = f64::from(frame.y);
    let frame_width = f64::from(frame.width);
    let frame_height = f64::from(frame.height);
    let center = Point {
        x: frame_x + frame_width * 0.5,
        y: frame_y + frame_height * 0.5,
    };
    let angle = f64::from(frame.angle_degrees).to_radians();
    let width_axis = Point {
        x: angle.cos(),
        y: angle.sin(),
    };
    let height_axis = Point {
        x: -width_axis.y,
        y: width_axis.x,
    };
    let radius_x = frame_width * 0.5;
    let radius_y = frame_height * 0.5;
    let extent_x = if oval {
        (radius_x * width_axis.x).hypot(radius_y * height_axis.x)
    } else {
        radius_x * width_axis.x.abs() + radius_y * height_axis.x.abs()
    };
    let extent_y = if oval {
        (radius_x * width_axis.y).hypot(radius_y * height_axis.y)
    } else {
        radius_x * width_axis.y.abs() + radius_y * height_axis.y.abs()
    };
    if center.x + extent_x <= 0.0
        || center.x - extent_x >= f64::from(width)
        || center.y + extent_y <= 0.0
        || center.y - extent_y >= f64::from(height)
    {
        bail!("raster shape does not intersect the page");
    }

    let page_corners = [
        Point { x: 0.0, y: 0.0 },
        Point {
            x: f64::from(width),
            y: 0.0,
        },
        Point {
            x: f64::from(width),
            y: f64::from(height),
        },
        Point {
            x: 0.0,
            y: f64::from(height),
        },
    ];
    let projected_y = page_corners
        .map(|point| (point.x - center.x) * height_axis.x + (point.y - center.y) * height_axis.y);
    let projected_x = page_corners
        .map(|point| (point.x - center.x) * width_axis.x + (point.y - center.y) * width_axis.y);
    if projected_x.into_iter().fold(f64::NEG_INFINITY, f64::max) <= -radius_x
        || projected_x.into_iter().fold(f64::INFINITY, f64::min) >= radius_x
    {
        bail!("raster shape does not intersect the page");
    }
    let local_top = projected_y
        .into_iter()
        .fold(f64::INFINITY, f64::min)
        .max(-radius_y);
    let local_bottom = projected_y
        .into_iter()
        .fold(f64::NEG_INFINITY, f64::max)
        .min(radius_y);
    if local_top >= local_bottom {
        bail!("raster shape does not intersect the page");
    }
    let row_count = (local_bottom - local_top).ceil().max(1.0) as usize;
    let row_height = (local_bottom - local_top) / row_count as f64;
    let mut points = Vec::with_capacity(row_count * 2);
    for row in 0..row_count {
        let local_y = local_top + (row as f64 + 0.5) * row_height;
        let half_width = if oval {
            radius_x * (1.0 - (local_y / radius_y).powi(2)).max(0.0).sqrt()
        } else {
            radius_x
        };
        let endpoints = [-half_width, half_width].map(|local_x| Point {
            x: center.x + local_x * width_axis.x + local_y * height_axis.x,
            y: center.y + local_x * width_axis.y + local_y * height_axis.y,
        });
        points.extend(if row.is_multiple_of(2) {
            endpoints
        } else {
            [endpoints[1], endpoints[0]]
        });
    }
    if points.is_empty() {
        bail!("raster shape does not cover any page pixels");
    }
    Ok((points, 1.0))
}

fn ensure_revision(actual: Revision, expected: Revision) -> anyhow::Result<()> {
    if actual != expected {
        bail!("canvas edit expected revision {expected}, but the project is at {actual}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oval_shapes_preserve_the_drag_aspect() {
        let (oval, diameter) = shape_stroke(
            20,
            10,
            RasterShape::Oval {
                frame: Frame {
                    x: 2.0,
                    y: 2.0,
                    width: 6.0,
                    height: 2.0,
                    angle_degrees: 0.0,
                },
            },
        )
        .unwrap();
        assert_eq!(diameter, 1.0);
        assert_eq!(oval.len(), 4);
        assert!(oval.iter().all(|point| (2.0..=8.0).contains(&point.x)));
        assert!(oval.iter().all(|point| (2.0..=4.0).contains(&point.y)));
        assert!(oval[0].x < oval[1].x);
        assert!(oval[2].x > oval[3].x);
    }

    #[test]
    fn rectangle_shapes_can_cross_the_page_edge() {
        let (rectangle, diameter) = shape_stroke(
            20,
            10,
            RasterShape::Rectangle {
                frame: Frame {
                    x: -2.0,
                    y: 3.0,
                    width: 6.0,
                    height: 2.0,
                    angle_degrees: 0.0,
                },
            },
        )
        .unwrap();
        assert_eq!(diameter, 1.0);
        assert_eq!(rectangle.first(), Some(&Point { x: -2.0, y: 3.5 }));
        assert_eq!(rectangle.last(), Some(&Point { x: -2.0, y: 4.5 }));
    }

    #[test]
    fn rotated_shapes_generate_rotated_scanlines() {
        let (rectangle, _) = shape_stroke(
            20,
            20,
            RasterShape::Rectangle {
                frame: Frame {
                    x: 5.0,
                    y: 7.0,
                    width: 10.0,
                    height: 4.0,
                    angle_degrees: 45.0,
                },
            },
        )
        .unwrap();
        assert!(rectangle[0].x < rectangle[1].x);
        assert!(rectangle[0].y < rectangle[1].y);
    }
}
