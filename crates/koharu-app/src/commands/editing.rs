use anyhow::Context as _;
use koharu_desktop::{CanvasState, Desktop, Frame};
use koharu_scene::{EntityId, RegionShapeKind};
use serde::Deserialize;
use specta::Type;
use tauri::{State, WebviewWindow};
use tauri_runtime_cef::CefRuntime;

use super::{
    ChannelExt as _, Error,
    canvas::{CanvasChannel, Point},
    import,
    project::{
        AddedLayer, BubbleStyleUpdate, CurrentProject, Page, Project, TextClassification,
        Typography,
    },
};
async fn synchronize_canvas(
    desktop: &Desktop,
    commit: &koharu_scene::Commit,
    page: Option<EntityId>,
) -> anyhow::Result<CanvasState> {
    desktop.synchronize(&commit.snapshot, page, commit).await?;
    Ok(desktop.canvas_state())
}

#[derive(Clone, Debug, Deserialize, Type)]
pub struct GeometryUpdate {
    pub layer: EntityId,
    pub points: Option<Vec<Point>>,
}

#[derive(Clone, Debug, Deserialize, Type)]
pub struct TypographyUpdate {
    pub layer: EntityId,
    pub typography: Typography,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn rename_page(
    page: EntityId,
    label: String,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.rename_page(page, label).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_translation_system_prompt(
    template: Option<String>,
    project: State<'_, CurrentProject>,
) -> Result<(), Error> {
    let mut project = project.project.lock().await;
    let project = project.as_mut().context("no project is open")?;
    let commit = project.set_translation_system_prompt(template).await?;
    project.record_commit(&commit);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn delete_pages(
    pages: Vec<EntityId>,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.delete_pages(pages).await?;
        project.record_commit(&commit);
        project.reconcile_page();
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn move_page(
    page: EntityId,
    index: u32,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.move_page(page, index as usize).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_source_text(
    layer: EntityId,
    text: String,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.set_source_text(layer, text).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_text_classification(
    layer: EntityId,
    classification: Option<TextClassification>,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project
            .set_text_classification(layer, classification)
            .await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_translation(
    layer: EntityId,
    text: Option<String>,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.set_translation(layer, text).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_typography(
    updates: Vec<TypographyUpdate>,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.set_typography(updates).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_geometry(
    updates: Vec<GeometryUpdate>,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.set_geometry(updates).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_region_geometry(
    region: EntityId,
    frame: Frame,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.set_region_geometry(region, frame).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_ocr_inclusion(
    layer: EntityId,
    included: bool,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.set_ocr_inclusion(layer, included).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_region_shape(
    region: EntityId,
    shape: RegionShapeKind,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.set_region_shape(region, shape).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_visibility(
    layers: Vec<EntityId>,
    visible: Option<bool>,
    opacity: Option<f32>,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.set_visibility(layers, visible, opacity).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_layer_locked(
    layers: Vec<EntityId>,
    locked: bool,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<Page, Error> {
    let (commit, page, view) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.set_layer_locked(layers, locked).await?;
        project.record_commit(&commit);
        let page = project.active_page().context("no active page")?;
        let view = Project::page(&commit.snapshot, page)?;
        (commit, page, view)
    };
    let canvas = synchronize_canvas(&desktop, &commit, Some(page)).await?;
    canvas_channel.channel.publish(canvas);
    Ok(view)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn add_image_layer(
    page: EntityId,
    window: WebviewWindow<CefRuntime>,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<Option<Page>, Error> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
        .set_parent(&window)
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_owned();
    let imported = tokio::task::spawn_blocking(move || import::import(vec![path]))
        .await
        .context("image layer import worker stopped unexpectedly")??
        .into_iter()
        .next()
        .context("the selected image did not contain a page")?;
    let (commit, view) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let (commit, _) = project.add_image_layer(page, imported).await?;
        project.record_commit(&commit);
        let view = Project::page(&commit.snapshot, page)?;
        (commit, view)
    };
    let canvas = synchronize_canvas(&desktop, &commit, Some(page)).await?;
    canvas_channel.channel.publish(canvas);
    Ok(Some(view))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn add_bubble_layer(
    page: EntityId,
    shape: RegionShapeKind,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<AddedLayer, Error> {
    let (commit, layer, view) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let (commit, layer) = project.add_bubble_layer(page, shape).await?;
        project.record_commit(&commit);
        let view = Project::page(&commit.snapshot, page)?;
        (commit, layer, view)
    };
    let canvas = synchronize_canvas(&desktop, &commit, Some(page)).await?;
    canvas_channel.channel.publish(canvas);
    Ok(AddedLayer { page: view, layer })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn set_bubble_style(
    layer: EntityId,
    style: BubbleStyleUpdate,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<Page, Error> {
    let (commit, page, view) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.set_bubble_style(layer, style).await?;
        project.record_commit(&commit);
        let page = project.active_page().context("no active page")?;
        let view = Project::page(&commit.snapshot, page)?;
        (commit, page, view)
    };
    let canvas = synchronize_canvas(&desktop, &commit, Some(page)).await?;
    canvas_channel.channel.publish(canvas);
    Ok(view)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn copy_layers(
    layers: Vec<EntityId>,
    project: State<'_, CurrentProject>,
) -> Result<(), Error> {
    let mut project = project.project.lock().await;
    let project = project.as_mut().context("no project is open")?;
    project.copy_layers(layers).await?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn paste_layers(
    page: EntityId,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<Vec<EntityId>, Error> {
    let (commit, pasted) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let (commit, pasted) = project.paste_layers(page).await?;
        project.record_commit(&commit);
        (commit, pasted)
    };
    let canvas = synchronize_canvas(&desktop, &commit, Some(page)).await?;
    canvas_channel.channel.publish(canvas);
    Ok(pasted)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn delete_layers(
    layers: Vec<EntityId>,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.delete_layers(layers).await?;
        project.record_commit(&commit);
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn move_layer(
    layer: EntityId,
    parent: EntityId,
    index: u32,
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<Page, Error> {
    let (commit, page, view) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.move_layer(layer, parent, index as usize).await?;
        project.record_commit(&commit);
        let page = project.active_page().context("no active page")?;
        let view = Project::page(&commit.snapshot, page)?;
        (commit, page, view)
    };
    desktop
        .synchronize(&commit.snapshot, Some(page), &commit)
        .await?;
    canvas_channel.channel.publish(desktop.canvas_state());
    Ok(view)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn undo(
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.undo().await?;
        project.reconcile_page();
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn redo(
    desktop: State<'_, Desktop>,
    project: State<'_, CurrentProject>,
    canvas_channel: State<'_, CanvasChannel>,
) -> Result<(), Error> {
    let (commit, page) = {
        let mut project = project.project.lock().await;
        let project = project.as_mut().context("no project is open")?;
        let commit = project.redo().await?;
        project.reconcile_page();
        (commit, project.active_page())
    };
    let canvas = synchronize_canvas(&desktop, &commit, page).await?;
    canvas_channel.channel.publish(canvas);
    Ok(())
}
