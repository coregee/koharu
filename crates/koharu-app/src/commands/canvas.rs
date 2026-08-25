use std::sync::Arc;

use anyhow::{Context as _, bail};
use image::{GrayImage, ImageEncoder as _, codecs::png::PngEncoder};
use koharu_desktop::{CanvasState, Desktop, Frame, TransformFrame};
use koharu_rasterizer::ResourceId;
use koharu_scene::{EntityId, RasterLayerKind, Revision};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{
    AppHandle, Cef, Manager as _, State,
    ipc::{Channel, IpcResponse},
};

use super::{
    ChannelExt as _, Error, processing,
    processing::{JobChannel, JobId, Processing},
    project::{CurrentProject, Page, Project, RasterStrokeMode},
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, Type)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InpaintSelection {
    Pen { points: Vec<Point>, diameter: f32 },
    Circle { start: Point, end: Point },
    Square { start: Point, end: Point },
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
    name = "paint_committed",
    skip_all,
    fields(origin = "user", point_count = points.len(), size = f64::from(brush.diameter)),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_paint(
    expected_revision: Revision,
    layer: Option<EntityId>,
    points: Vec<Point>,
    brush: PaintBrush,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    commit_raster_stroke(
        expected_revision,
        layer,
        points,
        brush.diameter,
        brush.color,
        RasterStrokeMode::Paint,
        RasterLayerKind::Paint,
        &desktop,
        &project,
        &canvas_channel,
    )
    .await
}

#[tracing::instrument(
    target = "koharu_metrics",
    name = "censor_mask_committed",
    skip_all,
    fields(origin = "user", point_count = points.len(), size = f64::from(diameter)),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_censor(
    expected_revision: Revision,
    layer: Option<EntityId>,
    points: Vec<Point>,
    diameter: f32,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    commit_raster_stroke(
        expected_revision,
        layer,
        points,
        diameter,
        [0, 255, 0, 180],
        RasterStrokeMode::Paint,
        RasterLayerKind::Censor,
        &desktop,
        &project,
        &canvas_channel,
    )
    .await
}

#[tracing::instrument(
    target = "koharu_metrics",
    name = "erase_committed",
    skip_all,
    fields(origin = "user", point_count = points.len(), size = f64::from(diameter)),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_erase(
    expected_revision: Revision,
    layer: EntityId,
    points: Vec<Point>,
    diameter: f32,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<LayerCommit, Error> {
    commit_raster_stroke(
        expected_revision,
        Some(layer),
        points,
        diameter,
        [0; 4],
        RasterStrokeMode::Erase,
        RasterLayerKind::Paint,
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
    let (commit, page, revision) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        ensure_revision(project.snapshot().revision(), expected_revision)?;
        let page = project
            .active_page()
            .context("the project has no active page")?;
        let commit = project
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
        (commit, project.active_page(), revision)
    };
    if let Some(commit) = commit {
        desktop.synchronize(&commit.snapshot, page, &commit).await?;
        canvas_channel.channel.publish(desktop.canvas_state());
    }
    Ok(LayerCommit { revision, layer })
}

#[allow(clippy::too_many_arguments)]
async fn commit_raster_stroke(
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
    name = "inpaint_requested",
    skip_all,
    fields(origin = "user", selection_count = selections.len()),
)]
#[tauri::command]
#[specta::specta]
pub(crate) async fn commit_inpaint(
    expected_revision: Revision,
    selections: Vec<InpaintSelection>,
    handle: AppHandle<Cef>,
    project: State<'_, CurrentProject>,
) -> Result<Option<JobId>, Error> {
    validate_inpaint_selections(&selections)?;
    let (page, width, height) = {
        let project = project.project.lock().await;
        let project = project.as_ref().context("no project is open")?;
        let snapshot = project.snapshot();
        ensure_revision(snapshot.revision(), expected_revision)?;
        let page = project
            .active_page()
            .context("the project has no active page")?;
        let page_value = snapshot.page(page)?.page()?;
        (
            page,
            page_value.width.round() as u32,
            page_value.height.round() as u32,
        )
    };
    let (png, bounds) =
        tokio::task::spawn_blocking(move || encode_mask(width, height, &selections))
            .await
            .context("inpaint mask worker stopped unexpectedly")??;
    *handle.state::<Processing>().inpainting_mask.lock() = Some(koharu_pipeline::InpaintingMask {
        page,
        png: Arc::from(png),
    });
    Ok(Some(
        processing::process(
            handle.clone(),
            koharu_pipeline::Scope::Region { page, bounds },
            koharu_pipeline::Operation::Only {
                stage: koharu_pipeline::Stage::Inpainting,
            },
            handle.state::<CurrentProject>(),
            handle.state::<Processing>(),
            handle.state::<JobChannel>(),
        )
        .await?,
    ))
}

fn validate_inpaint_selections(selections: &[InpaintSelection]) -> anyhow::Result<()> {
    if selections.is_empty() {
        bail!("an inpaint request requires at least one selection");
    }
    for selection in selections {
        match selection {
            InpaintSelection::Pen { points, diameter } => {
                if points.is_empty() || !diameter.is_finite() || *diameter <= 0.0 {
                    bail!("an inpaint pen selection requires points and a positive diameter");
                }
                validate_points(points)?;
            }
            InpaintSelection::Circle { start, end } | InpaintSelection::Square { start, end } => {
                validate_points(&[*start, *end])?;
            }
        }
    }
    Ok(())
}

fn validate_points(points: &[Point]) -> anyhow::Result<()> {
    if points
        .iter()
        .any(|point| !point.x.is_finite() || !point.y.is_finite())
    {
        bail!("inpaint selection points must be finite");
    }
    Ok(())
}

fn ensure_revision(actual: Revision, expected: Revision) -> anyhow::Result<()> {
    if actual != expected {
        bail!("canvas edit expected revision {expected}, but the project is at {actual}");
    }
    Ok(())
}

fn encode_mask(
    width: u32,
    height: u32,
    selections: &[InpaintSelection],
) -> anyhow::Result<(Vec<u8>, koharu_pipeline::Bounds)> {
    if width == 0 || height == 0 {
        bail!("page dimensions must be positive");
    }
    let mut image = GrayImage::new(width, height);
    let mut dirty = None::<[u32; 4]>;
    for selection in selections {
        match selection {
            InpaintSelection::Pen { points, diameter } => {
                paint_pen(&mut image, &mut dirty, points, *diameter);
            }
            InpaintSelection::Circle { start, end } => {
                paint_circle(&mut image, &mut dirty, *start, *end);
            }
            InpaintSelection::Square { start, end } => {
                paint_square(&mut image, &mut dirty, *start, *end);
            }
        }
    }
    let [left, top, right, bottom] =
        dirty.context("inpaint selection does not intersect the page")?;
    let mut png = Vec::new();
    PngEncoder::new(&mut png).write_image(
        image.as_raw(),
        width,
        height,
        image::ExtendedColorType::L8,
    )?;
    Ok((
        png,
        koharu_pipeline::Bounds {
            x: f64::from(left),
            y: f64::from(top),
            width: f64::from(right - left + 1),
            height: f64::from(bottom - top + 1),
        },
    ))
}

fn paint_pen(image: &mut GrayImage, dirty: &mut Option<[u32; 4]>, points: &[Point], diameter: f32) {
    let width = image.width();
    let height = image.height();
    let radius = f64::from(diameter) * 0.5;
    for (start, end) in points
        .iter()
        .zip(points.iter().skip(1))
        .chain(points.last().map(|point| (point, point)))
    {
        let left = (start.x.min(end.x) - radius - 0.5).floor().max(0.0) as u32;
        let top = (start.y.min(end.y) - radius - 0.5).floor().max(0.0) as u32;
        let right = (start.x.max(end.x) + radius + 0.5)
            .ceil()
            .min(f64::from(width)) as u32;
        let bottom = (start.y.max(end.y) + radius + 0.5)
            .ceil()
            .min(f64::from(height)) as u32;
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let length_squared = dx.mul_add(dx, dy * dy);
        for y in top..bottom {
            for x in left..right {
                let px = f64::from(x) + 0.5;
                let py = f64::from(y) + 0.5;
                let progress = if length_squared <= f64::EPSILON {
                    0.0
                } else {
                    (((px - start.x) * dx + (py - start.y) * dy) / length_squared).clamp(0.0, 1.0)
                };
                let nearest_x = start.x + progress * dx;
                let nearest_y = start.y + progress * dy;
                let distance = (px - nearest_x).hypot(py - nearest_y);
                let coverage = ((radius + 0.5 - distance).clamp(0.0, 1.0) * 255.0).round() as u8;
                if coverage == 0 {
                    continue;
                }
                paint_pixel(image, dirty, x, y, coverage);
            }
        }
    }
}

fn paint_circle(image: &mut GrayImage, dirty: &mut Option<[u32; 4]>, start: Point, end: Point) {
    let [left, top, right, bottom] = constrained_bounds(start, end);
    let center_x = (left + right) * 0.5;
    let center_y = (top + bottom) * 0.5;
    let radius = (right - left) * 0.5;
    let pixel_bounds =
        clipped_pixel_bounds(image, left - 0.5, top - 0.5, right + 0.5, bottom + 0.5);
    for y in pixel_bounds[1]..pixel_bounds[3] {
        for x in pixel_bounds[0]..pixel_bounds[2] {
            let distance = (f64::from(x) + 0.5 - center_x).hypot(f64::from(y) + 0.5 - center_y);
            let coverage = ((radius + 0.5 - distance).clamp(0.0, 1.0) * 255.0).round() as u8;
            if coverage > 0 {
                paint_pixel(image, dirty, x, y, coverage);
            }
        }
    }
}

fn paint_square(image: &mut GrayImage, dirty: &mut Option<[u32; 4]>, start: Point, end: Point) {
    let [left, top, right, bottom] = constrained_bounds(start, end);
    let pixel_bounds = clipped_pixel_bounds(image, left, top, right, bottom);
    for y in pixel_bounds[1]..pixel_bounds[3] {
        for x in pixel_bounds[0]..pixel_bounds[2] {
            paint_pixel(image, dirty, x, y, 255);
        }
    }
}

fn constrained_bounds(start: Point, end: Point) -> [f64; 4] {
    let delta_x = end.x - start.x;
    let delta_y = end.y - start.y;
    let size = delta_x.abs().max(delta_y.abs()).max(1.0);
    let end_x = start.x + size.copysign(if delta_x == 0.0 { 1.0 } else { delta_x });
    let end_y = start.y + size.copysign(if delta_y == 0.0 { 1.0 } else { delta_y });
    [
        start.x.min(end_x),
        start.y.min(end_y),
        start.x.max(end_x),
        start.y.max(end_y),
    ]
}

fn clipped_pixel_bounds(
    image: &GrayImage,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
) -> [u32; 4] {
    [
        left.floor().max(0.0) as u32,
        top.floor().max(0.0) as u32,
        right.ceil().min(f64::from(image.width())).max(0.0) as u32,
        bottom.ceil().min(f64::from(image.height())).max(0.0) as u32,
    ]
}

fn paint_pixel(image: &mut GrayImage, dirty: &mut Option<[u32; 4]>, x: u32, y: u32, coverage: u8) {
    let pixel = image.get_pixel_mut(x, y);
    pixel.0[0] = pixel.0[0].max(coverage);
    *dirty = Some(match *dirty {
        Some([min_x, min_y, max_x, max_y]) => {
            [min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]
        }
        None => [x, y, x, y],
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_inpaint_selections_share_one_mask_and_bounds() {
        let selections = [
            InpaintSelection::Pen {
                points: vec![Point { x: 2.0, y: 2.0 }, Point { x: 4.0, y: 2.0 }],
                diameter: 2.0,
            },
            InpaintSelection::Square {
                start: Point { x: 6.0, y: 6.0 },
                end: Point { x: 8.0, y: 7.0 },
            },
        ];

        let (png, bounds) = encode_mask(12, 12, &selections).unwrap();
        let mask = image::load_from_memory(&png).unwrap().into_luma8();

        assert!(mask.get_pixel(2, 2).0[0] > 0);
        assert_eq!(mask.get_pixel(7, 7).0[0], 255);
        assert_eq!(mask.get_pixel(10, 10).0[0], 0);
        assert!(bounds.x <= 2.0);
        assert!(bounds.y <= 2.0);
        assert!(bounds.x + bounds.width >= 8.0);
        assert!(bounds.y + bounds.height >= 8.0);
    }

    #[test]
    fn circle_selection_does_not_fill_its_constrained_square_corners() {
        let selections = [InpaintSelection::Circle {
            start: Point { x: 2.0, y: 2.0 },
            end: Point { x: 8.0, y: 4.0 },
        }];

        let (png, _) = encode_mask(12, 12, &selections).unwrap();
        let mask = image::load_from_memory(&png).unwrap().into_luma8();

        assert_eq!(mask.get_pixel(2, 2).0[0], 0);
        assert_eq!(mask.get_pixel(5, 5).0[0], 255);
    }

    #[test]
    fn inpaint_selection_validation_rejects_empty_requests() {
        assert!(validate_inpaint_selections(&[]).is_err());
        assert!(
            validate_inpaint_selections(&[InpaintSelection::Pen {
                points: Vec::new(),
                diameter: 12.0,
            }])
            .is_err()
        );
    }
}
