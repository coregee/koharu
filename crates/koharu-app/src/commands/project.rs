use std::{
    collections::{HashSet, VecDeque},
    io::Cursor,
    path::PathBuf,
};

use anyhow::{Context as _, Result, anyhow, bail};
use image::{DynamicImage, ImageFormat, RgbaImage};
use koharu_desktop::Frame;
use koharu_scene::{
    AssetInput, AssetMetadata, AssetRole, At, Authored, BubbleRegion,
    BubbleStyle as SceneBubbleStyle, Commit, Edit as SceneEdit, EntityId, EntityOrigin, FitsTo,
    Generation, Geometry as SceneGeometry, Group as SceneGroup, LayerLock as SceneLayerLock,
    OcrCandidate as SceneOcrCandidate, Origin, PageDraft, Point as ScenePoint, Presents,
    Project as SceneProject, RasterLayer as SceneRasterLayer, RasterLayerKind, RecognizedFrom,
    Region as SceneRegion, RegionShape as SceneRegionShape, RegionShapeKind, RemovePolicy,
    Revision, Session, Snapshot, SourceText as SceneSourceText, TextGroup as SceneTextGroup,
    TextGroupKind as SceneTextGroupKind, TextLayout as SceneTextLayout, TextLayoutKind,
    TextRegion as SceneTextRegion, TextRole as SceneTextRole, Translation as SceneTranslation,
    Typography as SceneTypography, Visibility as SceneVisibility,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::sync::Mutex;

use super::{
    canvas::Point,
    editing::{GeometryUpdate, TypographyUpdate},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RasterStrokeMode {
    Paint,
    Erase,
}

/// Maps source pixels into page space so erasing follows an image layer's durable geometry.
#[derive(Clone, Copy, Debug)]
struct RasterPlacement {
    origin: [f64; 2],
    x_axis: [f64; 2],
    y_axis: [f64; 2],
}

impl RasterPlacement {
    const IDENTITY: Self = Self {
        origin: [0.0, 0.0],
        x_axis: [1.0, 0.0],
        y_axis: [0.0, 1.0],
    };

    fn from_geometry(geometry: &SceneGeometry, width: u32, height: u32) -> Result<Self> {
        let [top_left, top_right, bottom_right, bottom_left] = geometry.points.as_slice() else {
            bail!("an editable image layer must have rectangular geometry");
        };
        let placement = Self {
            origin: [top_left.x, top_left.y],
            x_axis: [
                (top_right.x - top_left.x) / f64::from(width),
                (top_right.y - top_left.y) / f64::from(width),
            ],
            y_axis: [
                (bottom_left.x - top_left.x) / f64::from(height),
                (bottom_left.y - top_left.y) / f64::from(height),
            ],
        };
        let determinant = placement.determinant();
        let expected_bottom_right = (
            top_right.x + bottom_left.x - top_left.x,
            top_right.y + bottom_left.y - top_left.y,
        );
        let closure_error = (bottom_right.x - expected_bottom_right.0)
            .hypot(bottom_right.y - expected_bottom_right.1);
        let closure_tolerance = (placement.axis_length(placement.x_axis) * f64::from(width))
            .max(placement.axis_length(placement.y_axis) * f64::from(height))
            * 1e-6;
        if !determinant.is_finite()
            || closure_error > closure_tolerance
            || determinant.abs()
                <= placement.axis_length(placement.x_axis)
                    * placement.axis_length(placement.y_axis)
                    * f64::EPSILON
        {
            bail!("an editable image layer must have non-degenerate geometry");
        }
        Ok(placement)
    }

    fn determinant(self) -> f64 {
        self.x_axis[0] * self.y_axis[1] - self.x_axis[1] * self.y_axis[0]
    }

    fn axis_length(self, axis: [f64; 2]) -> f64 {
        axis[0].hypot(axis[1])
    }

    fn pixel_size(self) -> f64 {
        self.axis_length(self.x_axis)
            .max(self.axis_length(self.y_axis))
    }

    fn raster_to_page(self, x: f64, y: f64) -> (f64, f64) {
        (
            self.origin[0] + self.x_axis[0] * x + self.y_axis[0] * y,
            self.origin[1] + self.x_axis[1] * x + self.y_axis[1] * y,
        )
    }

    fn page_to_raster(self, x: f64, y: f64) -> (f64, f64) {
        let x = x - self.origin[0];
        let y = y - self.origin[1];
        let determinant = self.determinant();
        (
            (x * self.y_axis[1] - y * self.y_axis[0]) / determinant,
            (self.x_axis[0] * y - self.x_axis[1] * x) / determinant,
        )
    }
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct ProjectInfo {
    pub name: String,
    pub revision: Revision,
    pub active_page: Option<EntityId>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub can_paste: bool,
}

#[derive(Clone)]
struct LayerClipboard {
    source_page: EntityId,
    last_paste_page: Option<EntityId>,
    paste_count: u32,
    layers: Vec<CopiedLayer>,
}

#[derive(Clone)]
enum CopiedLayer {
    Text {
        layout: SceneTextLayout,
        frame: Option<SceneGeometry>,
        typography: Option<SceneTypography>,
        visibility: Option<SceneVisibility>,
        lock: Option<SceneLayerLock>,
        source: Option<SceneSourceText>,
        translation: Option<SceneTranslation>,
        role: Option<SceneTextRole>,
        ocr_candidate: Option<SceneOcrCandidate>,
    },
    Raster {
        layer: SceneRasterLayer,
        visibility: Option<SceneVisibility>,
        lock: Option<SceneLayerLock>,
        asset: Option<CopiedAsset>,
    },
    Image {
        geometry: SceneGeometry,
        visibility: Option<SceneVisibility>,
        lock: Option<SceneLayerLock>,
        asset: CopiedAsset,
    },
    Bubble {
        geometry: SceneGeometry,
        shape: RegionShapeKind,
        style: SceneBubbleStyle,
        visibility: Option<SceneVisibility>,
        lock: Option<SceneLayerLock>,
    },
}

#[derive(Clone)]
struct CopiedAsset {
    bytes: std::sync::Arc<[u8]>,
    media_type: String,
    metadata: AssetMetadata,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct ProjectSummary {
    pub name: String,
}

#[derive(Clone, Copy, Debug, Serialize, Type)]
pub struct PageSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct PageSummary {
    pub id: EntityId,
    pub label: String,
    pub size: PageSize,
    pub source_asset: Option<String>,
    #[specta(type = f64)]
    pub layer_count: usize,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct Page {
    pub id: EntityId,
    pub label: String,
    pub size: PageSize,
    pub layers: Vec<Layer>,
    pub regions: Vec<AnalysisRegion>,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct AddedLayer {
    pub page: Page,
    pub layer: EntityId,
}

#[derive(Clone, Debug, Serialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Layer {
    Group {
        id: EntityId,
        parent: Option<EntityId>,
        locked: bool,
        visibility: LayerVisibility,
        name: String,
        role: Option<GroupRole>,
    },
    Text {
        id: EntityId,
        parent: Option<EntityId>,
        locked: bool,
        geometry: Option<Geometry>,
        visibility: LayerVisibility,
        content: Box<TextContent>,
        typography: Option<Typography>,
        layout: TextLayoutKind,
    },
    Raster {
        id: EntityId,
        parent: Option<EntityId>,
        locked: bool,
        visibility: LayerVisibility,
        image: Option<String>,
        name: String,
        kind: RasterLayerKind,
    },
    Image {
        id: EntityId,
        parent: Option<EntityId>,
        locked: bool,
        geometry: Geometry,
        visibility: LayerVisibility,
        image: String,
    },
    Bubble {
        id: EntityId,
        parent: Option<EntityId>,
        locked: bool,
        geometry: Geometry,
        visibility: LayerVisibility,
        shape: RegionShapeKind,
        fill_color: [u8; 4],
        outline_color: [u8; 4],
        outline_width: f32,
    },
    Artwork {
        id: EntityId,
        parent: Option<EntityId>,
        locked: bool,
        geometry: Geometry,
        visibility: LayerVisibility,
        image: String,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum GroupRole {
    MainText,
    SfxText,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct TextContent {
    pub id: EntityId,
    pub source: Option<SourceText>,
    pub translation: Option<Translation>,
    pub classification: Option<TextClassification>,
    pub source_region: Option<EntityId>,
    pub include_for_ocr: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TextClassification {
    Speech,
    Narration,
    Sfx,
}

impl TextClassification {
    fn from_role(role: &str) -> Option<Self> {
        match role {
            SceneTextRole::SPEECH => Some(Self::Speech),
            SceneTextRole::NARRATION => Some(Self::Narration),
            SceneTextRole::SFX => Some(Self::Sfx),
            _ => None,
        }
    }

    fn role(self) -> &'static str {
        match self {
            Self::Speech => SceneTextRole::SPEECH,
            Self::Narration => SceneTextRole::NARRATION,
            Self::Sfx => SceneTextRole::SFX,
        }
    }
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct AnalysisRegion {
    pub id: EntityId,
    pub parent: Option<EntityId>,
    pub geometry: Geometry,
    pub kind: String,
    pub label: Option<String>,
    pub shape: RegionShapeKind,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct Geometry {
    pub points: Vec<Point>,
}

#[derive(Clone, Copy, Debug, Serialize, Type)]
pub struct LayerVisibility {
    pub visible: bool,
    pub opacity: f32,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct SourceText {
    pub text: String,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Serialize, Type)]
pub struct Translation {
    pub text: String,
    pub language: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize, Serialize, Type)]
pub struct Typography {
    pub preferred_font: Option<String>,
    pub font_weight: Option<u16>,
    pub font_style: Option<koharu_scene::FontStyle>,
    pub size: Option<f32>,
    pub auto_fit: bool,
    pub color: Option<[u8; 4]>,
    pub stroke_color: Option<[u8; 4]>,
    pub stroke_width: Option<f32>,
    pub paragraph_spacing: Option<f32>,
    pub style_ranges: Vec<koharu_scene::TextStyleRange>,
    pub alignment: Option<koharu_scene::TextAlignment>,
    pub writing_mode: Option<koharu_scene::WritingMode>,
}

#[derive(Clone, Debug, serde::Deserialize, Serialize, Type)]
pub struct BubbleStyleUpdate {
    pub shape: RegionShapeKind,
    pub fill_color: [u8; 4],
    pub outline_color: [u8; 4],
    pub outline_width: f32,
}

pub(crate) struct CurrentProject {
    pub(crate) project: Mutex<Option<Project>>,
}

#[derive(Clone)]
pub(crate) struct ProjectLibrary {
    root: PathBuf,
}

impl ProjectLibrary {
    pub(crate) fn new() -> Result<Self> {
        let root = dirs::document_dir()
            .context("the Documents directory is unavailable")?
            .join("Koharu");
        std::fs::create_dir_all(&root)
            .with_context(|| format!("failed to create {}", root.display()))?;
        Ok(Self { root })
    }

    pub(crate) fn list(&self) -> Result<Vec<ProjectSummary>> {
        let mut projects = std::fs::read_dir(&self.root)
            .with_context(|| format!("failed to read {}", self.root.display()))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| {
                let path = entry.path();
                let is_project_directory = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("khrproj"))
                    && (path.join("state-a.khr").is_file() || path.join("state-b.khr").is_file());
                if !is_project_directory {
                    return None;
                }
                let last_used = ["state-a.khr", "state-b.khr"]
                    .into_iter()
                    .filter_map(|file| std::fs::metadata(path.join(file)).ok()?.modified().ok())
                    .max()
                    .unwrap_or(std::time::UNIX_EPOCH);
                Some((
                    last_used,
                    ProjectSummary {
                        name: path.file_stem()?.to_str()?.to_owned(),
                    },
                ))
            })
            .collect::<Vec<_>>();
        projects.sort_unstable_by(|(left_used, left), (right_used, right)| {
            right_used
                .cmp(left_used)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(projects.into_iter().map(|(_, project)| project).collect())
    }

    pub(crate) async fn create(&self, name: &str) -> Result<Project> {
        let (name, path) = self.resolve(name)?;
        Project::create(name, path).await
    }

    pub(crate) async fn open(&self, name: &str) -> Result<Project> {
        let (name, path) = self.resolve(name)?;
        Project::open(name, path).await
    }

    pub(crate) fn delete(&self, name: &str) -> Result<()> {
        let (_, path) = self.resolve(name)?;
        if !path.is_dir() {
            bail!("project {name:?} does not exist");
        }
        std::fs::remove_dir_all(&path)
            .with_context(|| format!("failed to delete {}", path.display()))
    }

    fn resolve(&self, name: &str) -> Result<(String, PathBuf)> {
        let name = validate_project_name(name)?;
        Ok((name.clone(), self.root.join(format!("{name}.khrproj"))))
    }
}

pub(crate) struct Project {
    pub(crate) session: Session,
    pub(crate) name: String,
    pub(crate) active_page: Option<EntityId>,
    pub(crate) undo: Vec<Vec<Revision>>,
    pub(crate) redo: Vec<Vec<Revision>>,
    clipboard: Option<LayerClipboard>,
}

impl Project {
    pub(crate) async fn create(name: String, path: PathBuf) -> Result<Self> {
        let session = Session::create(&path)
            .await
            .with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Self::new(session, name))
    }

    pub(crate) async fn open(name: String, path: PathBuf) -> Result<Self> {
        let session = Session::open(&path)
            .await
            .with_context(|| format!("failed to open {}", path.display()))?;
        Ok(Self::new(session, name))
    }

    fn new(session: Session, name: String) -> Self {
        let active_page = session.snapshot().pages().next().map(|page| page.id());
        Self {
            session,
            name,
            active_page,
            undo: Vec::new(),
            redo: Vec::new(),
            clipboard: None,
        }
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        self.session.snapshot()
    }

    pub(crate) fn revision(&self) -> Revision {
        self.snapshot().revision()
    }

    pub(crate) fn active_page(&self) -> Option<EntityId> {
        self.active_page
    }

    pub(crate) fn select_page(&mut self, page: EntityId) -> Result<()> {
        self.snapshot().page(page)?;
        self.active_page = Some(page);
        Ok(())
    }

    pub(crate) fn reconcile_page(&mut self) {
        let snapshot = self.snapshot();
        if self
            .active_page
            .is_none_or(|page| snapshot.page(page).is_err())
        {
            self.active_page = snapshot.pages().next().map(|page| page.id());
        }
    }

    pub(crate) fn info(&self) -> ProjectInfo {
        ProjectInfo {
            name: self.name.clone(),
            revision: self.revision(),
            active_page: self.active_page,
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
            can_paste: self.clipboard.is_some(),
        }
    }

    pub(crate) async fn set_translation_system_prompt(
        &mut self,
        template: Option<String>,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let mut settings = snapshot
            .project_component::<SceneProject>()?
            .unwrap_or_default();
        settings.translation_system_prompt = template;
        let patch = snapshot.patch(|edit| edit.set_project(&settings))?;
        self.commit(patch).await
    }

    pub(crate) fn pages(snapshot: &Snapshot) -> Result<Vec<PageSummary>> {
        snapshot
            .pages()
            .map(|page| {
                let value = page.page()?;
                let source_asset = Self::asset_id(snapshot, page.id(), "source")?;
                let layer_count = snapshot
                    .descendants(page.id())?
                    .map(|entity| Self::is_content_layer(snapshot, entity.id()))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .filter(|present| *present)
                    .count()
                    + usize::from(source_asset.is_some());
                Ok(PageSummary {
                    id: page.id(),
                    label: value.label,
                    size: PageSize {
                        width: value.width,
                        height: value.height,
                    },
                    source_asset,
                    layer_count,
                })
            })
            .collect()
    }

    pub(crate) async fn rename_page(&mut self, page: EntityId, label: String) -> Result<Commit> {
        let snapshot = self.snapshot();
        let current = snapshot.page(page)?.page()?;
        let patch = snapshot.patch(|edit| {
            edit.set_page(page, PageDraft::new(label, current.width, current.height))
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn delete_pages(&mut self, pages: Vec<EntityId>) -> Result<Commit> {
        let snapshot = self.snapshot();
        let pages = Self::unique_roots(&snapshot, pages)?;
        let patch = snapshot.patch(|edit| {
            for page in pages {
                edit.remove_entity(page, RemovePolicy::Cascade)?;
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn move_page(&mut self, page: EntityId, index: usize) -> Result<Commit> {
        let snapshot = self.snapshot();
        let siblings = snapshot.pages().map(|page| page.id()).collect::<Vec<_>>();
        let at = Self::placement(&siblings, page, index);
        let patch = snapshot.patch(|edit| edit.move_entity(page, None, at))?;
        self.commit(patch).await
    }

    pub(crate) async fn add_text_box(
        &mut self,
        page: EntityId,
        frame: Frame,
    ) -> Result<(Commit, EntityId)> {
        let snapshot = self.snapshot();
        let geometry = Self::geometry_from_frame(frame)?;
        let mut layer = None;
        let patch = snapshot.patch(|edit| {
            let region =
                edit.add_analysis_region::<SceneTextRegion>(page, At::End, &geometry, None)?;
            let content = edit.add_text_content(page, At::End)?;
            edit.set(
                content,
                &SceneSourceText {
                    text: Authored::user(String::new()),
                    language: None,
                },
            )?;
            edit.set(
                content,
                &SceneOcrCandidate {
                    origin: Origin::User,
                },
            )?;
            let added_layer = edit.add_text_layer(
                page,
                At::End,
                content,
                &SceneTextLayout {
                    origin: Origin::User,
                    kind: TextLayoutKind::Paragraph,
                },
            )?;
            layer = Some(added_layer);
            edit.relate::<RecognizedFrom>(content, region)?;
            edit.relate::<FitsTo>(added_layer, region)?;
            edit.set(
                added_layer,
                &SceneTypography {
                    origin: Origin::User,
                    preferred_font: None,
                    font_weight: None,
                    font_style: None,
                    size: None,
                    auto_fit: true,
                    color: None,
                    stroke_color: None,
                    stroke_width: None,
                    paragraph_spacing: None,
                    style_ranges: Vec::new(),
                    alignment: None,
                    writing_mode: None,
                    extensions: Default::default(),
                },
            )?;
            Ok(())
        })?;
        Ok((
            self.commit(patch).await?,
            layer.expect("text layer was added while building the patch"),
        ))
    }

    pub(super) async fn add_image_layer(
        &mut self,
        page: EntityId,
        imported: super::import::Page,
    ) -> Result<(Commit, EntityId)> {
        let snapshot = self.snapshot();
        let page_value = snapshot.page(page)?.page()?;
        let source = AssetRole::new("source")?;
        let mut layer = None;
        let patch = snapshot.patch(|edit| {
            let at = snapshot
                .page(page)?
                .text_group(SceneTextGroupKind::Main)?
                .map_or(At::End, |group| At::Before(group.id()));
            let added = edit.add_entity(page, at)?;
            edit.set(
                added,
                &SceneGeometry {
                    origin: Origin::User,
                    points: vec![
                        ScenePoint { x: 0.0, y: 0.0 },
                        ScenePoint {
                            x: page_value.width,
                            y: 0.0,
                        },
                        ScenePoint {
                            x: page_value.width,
                            y: page_value.height,
                        },
                        ScenePoint {
                            x: 0.0,
                            y: page_value.height,
                        },
                    ],
                },
            )?;
            edit.set_asset(
                added,
                &source,
                AssetInput::new(
                    imported.bytes,
                    imported.format.to_mime_type(),
                    AssetMetadata {
                        width: Some(imported.width),
                        height: Some(imported.height),
                        attributes: [("name".to_owned(), imported.name)].into_iter().collect(),
                    },
                ),
            )?;
            layer = Some(added);
            Ok(())
        })?;
        Ok((
            self.commit(patch).await?,
            layer.expect("image layer was added while building the patch"),
        ))
    }

    pub(crate) async fn add_bubble_layer(
        &mut self,
        page: EntityId,
        shape: RegionShapeKind,
    ) -> Result<(Commit, EntityId)> {
        let snapshot = self.snapshot();
        let page_value = snapshot.page(page)?.page()?;
        let width = (page_value.width * 0.32)
            .clamp(120.0, 360.0)
            .min(page_value.width);
        let height = (width * 0.58).clamp(80.0, 220.0).min(page_value.height);
        let geometry = SceneGeometry::rectangle(
            (page_value.width - width) * 0.5,
            (page_value.height - height) * 0.5,
            width,
            height,
        );
        let at = snapshot
            .page(page)?
            .text_group(SceneTextGroupKind::Main)?
            .map_or(At::End, |group| At::Before(group.id()));
        let mut layer = None;
        let patch = snapshot.patch(|edit| {
            let added = edit.add_analysis_region::<BubbleRegion>(page, at, &geometry, None)?;
            edit.set(
                added,
                &SceneRegionShape {
                    origin: Origin::User,
                    kind: shape,
                },
            )?;
            edit.set(
                added,
                &SceneBubbleStyle {
                    origin: Origin::User,
                    fill_color: [255, 255, 255, 255],
                    outline_color: [0, 0, 0, 255],
                    outline_width: 3.0,
                },
            )?;
            layer = Some(added);
            Ok(())
        })?;
        Ok((
            self.commit(patch).await?,
            layer.expect("bubble layer was added while building the patch"),
        ))
    }

    pub(crate) async fn set_bubble_style(
        &mut self,
        layer: EntityId,
        style: BubbleStyleUpdate,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        snapshot
            .component::<SceneBubbleStyle>(layer)?
            .context("entity is not a speech-bubble layer")?;
        if Self::is_effectively_locked(&snapshot, layer)? {
            bail!("locked layers cannot be edited");
        }
        let patch = snapshot.patch(|edit| {
            edit.promote_entity_to_user(layer)?;
            edit.set(
                layer,
                &SceneRegionShape {
                    origin: Origin::User,
                    kind: style.shape,
                },
            )?;
            edit.set(
                layer,
                &SceneBubbleStyle {
                    origin: Origin::User,
                    fill_color: style.fill_color,
                    outline_color: style.outline_color,
                    outline_width: style.outline_width,
                },
            )
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_source_text(
        &mut self,
        layer: EntityId,
        text: String,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let content = Self::text_content(&snapshot, layer)?;
        let language = snapshot
            .component::<SceneSourceText>(content)?
            .and_then(|source| source.language);
        let patch = snapshot.patch(|edit| {
            edit.promote_entity_to_user(layer)?;
            edit.promote_entity_to_user(content)?;
            edit.set(
                content,
                &SceneSourceText {
                    text: Authored::user(text),
                    language,
                },
            )
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_text_classification(
        &mut self,
        layer: EntityId,
        classification: Option<TextClassification>,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let content = Self::text_content(&snapshot, layer)?;
        let patch = snapshot.patch(|edit| {
            edit.promote_entity_to_user(layer)?;
            edit.promote_entity_to_user(content)?;
            match classification {
                Some(classification) => edit.set_text_role(
                    content,
                    Some(&SceneTextRole {
                        origin: Origin::User,
                        role: classification.role().to_owned(),
                    }),
                ),
                None => edit.set_text_role(content, None),
            }
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_ocr_inclusion(
        &mut self,
        layer: EntityId,
        included: bool,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let content = Self::text_content(&snapshot, layer)?;
        let patch = snapshot.patch(|edit| {
            edit.promote_entity_to_user(layer)?;
            edit.promote_entity_to_user(content)?;
            if included {
                edit.set(
                    content,
                    &SceneOcrCandidate {
                        origin: Origin::User,
                    },
                )
            } else {
                edit.remove::<SceneOcrCandidate>(content)
            }
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_translation(
        &mut self,
        layer: EntityId,
        text: Option<String>,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let content = Self::text_content(&snapshot, layer)?;
        let translation = snapshot.component::<SceneTranslation>(content)?;
        let language = translation
            .as_ref()
            .and_then(|value| value.language.clone());
        let old_text = translation
            .map(|value| value.text.value)
            .unwrap_or_default();
        let typography =
            snapshot
                .component::<SceneTypography>(layer)?
                .and_then(|mut typography| {
                    let previous = typography.style_ranges.clone();
                    reconcile_style_ranges(
                        &mut typography.style_ranges,
                        &old_text,
                        text.as_deref().unwrap_or_default(),
                    );
                    (typography.style_ranges != previous).then_some(typography)
                });
        let patch = snapshot.patch(|edit| {
            edit.promote_entity_to_user(layer)?;
            edit.promote_entity_to_user(content)?;
            if let Some(typography) = typography {
                edit.set(layer, &typography)?;
            }
            match text {
                Some(text) => edit.set(
                    content,
                    &SceneTranslation {
                        text: Authored::user(text),
                        language,
                    },
                ),
                None => edit.remove::<SceneTranslation>(content),
            }
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_typography(
        &mut self,
        updates: Vec<TypographyUpdate>,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let updates = updates
            .into_iter()
            .map(|update| {
                let content = Self::text_content(&snapshot, update.layer)?;
                Ok((update, content))
            })
            .collect::<Result<Vec<_>>>()?;
        let patch = snapshot.patch(|edit| {
            for (update, content) in updates {
                edit.promote_entity_to_user(update.layer)?;
                edit.promote_entity_to_user(content)?;
                edit.set(
                    update.layer,
                    &SceneTypography {
                        origin: Origin::User,
                        preferred_font: update.typography.preferred_font,
                        font_weight: update.typography.font_weight,
                        font_style: update.typography.font_style,
                        size: update.typography.size.filter(|size| *size > 0.0),
                        auto_fit: update.typography.auto_fit,
                        color: update.typography.color,
                        stroke_color: update.typography.stroke_color,
                        stroke_width: update.typography.stroke_width,
                        paragraph_spacing: update.typography.paragraph_spacing,
                        style_ranges: update.typography.style_ranges,
                        alignment: update.typography.alignment,
                        writing_mode: update.typography.writing_mode,
                        extensions: Default::default(),
                    },
                )?;
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_geometry(&mut self, updates: Vec<GeometryUpdate>) -> Result<Commit> {
        let snapshot = self.snapshot();
        let updates = updates
            .into_iter()
            .map(|update| {
                if !Self::is_transformable_layer(&snapshot, update.layer)? {
                    bail!("only text and image layers can change geometry");
                }
                if Self::is_effectively_locked(&snapshot, update.layer)? {
                    bail!("locked layers cannot change geometry");
                }
                let content = snapshot
                    .component::<SceneTextLayout>(update.layer)?
                    .map(|_| Self::text_content(&snapshot, update.layer))
                    .transpose()?;
                if update.points.is_none() {
                    if content.is_none()
                        || snapshot
                            .text_layer(update.layer)?
                            .automatic_target()?
                            .is_none()
                    {
                        bail!("only automatically placed text can reset its geometry");
                    }
                }
                Ok((update, content))
            })
            .collect::<Result<Vec<_>>>()?;
        let patch = snapshot.patch(|edit| {
            for (update, content) in updates {
                edit.promote_entity_to_user(update.layer)?;
                if let Some(content) = content {
                    edit.promote_entity_to_user(content)?;
                }
                match update.points {
                    Some(points) => edit.set(
                        update.layer,
                        &SceneGeometry {
                            origin: Origin::User,
                            points: points
                                .into_iter()
                                .map(|point| ScenePoint {
                                    x: point.x,
                                    y: point.y,
                                })
                                .collect(),
                        },
                    )?,
                    None => edit.remove::<SceneGeometry>(update.layer)?,
                }
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_region_geometry(
        &mut self,
        region: EntityId,
        frame: Frame,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        snapshot.analysis_region(region)?;
        let geometry = Self::geometry_from_frame(frame)?;
        let patch = snapshot.patch(|edit| {
            edit.promote_entity_to_user(region)?;
            edit.set(region, &geometry)
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_region_shape(
        &mut self,
        region: EntityId,
        shape: RegionShapeKind,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        snapshot.analysis_region(region)?;
        let patch = snapshot.patch(|edit| {
            edit.promote_entity_to_user(region)?;
            edit.set(
                region,
                &SceneRegionShape {
                    origin: Origin::User,
                    kind: shape,
                },
            )
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_visibility(
        &mut self,
        layers: Vec<EntityId>,
        visible: Option<bool>,
        opacity: Option<f32>,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let patch = snapshot.patch(|edit| {
            for layer in layers {
                let mut value =
                    snapshot
                        .component::<SceneVisibility>(layer)?
                        .unwrap_or(SceneVisibility {
                            origin: Origin::User,
                            visible: true,
                            opacity: 1.0,
                        });
                if let Some(visible) = visible {
                    value.visible = visible;
                }
                if let Some(opacity) = opacity {
                    value.opacity = opacity;
                }
                value.origin = Origin::User;
                edit.set(layer, &value)?;
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn set_layer_locked(
        &mut self,
        layers: Vec<EntityId>,
        locked: bool,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        for layer in &layers {
            if !Self::is_layer(&snapshot, *layer)? {
                bail!("only layers can be locked");
            }
        }
        let mut generated_segmentations = HashSet::new();
        for layer in &layers {
            if Self::segmentation_generation(&snapshot, *layer)?.is_some() {
                generated_segmentations.insert(*layer);
            }
        }
        let patch = snapshot.patch(|edit| {
            for layer in layers {
                if !generated_segmentations.contains(&layer) {
                    edit.promote_entity_to_user(layer)?;
                }
                if locked {
                    edit.set(
                        layer,
                        &SceneLayerLock {
                            origin: Origin::User,
                            locked: true,
                        },
                    )?;
                } else {
                    edit.remove::<SceneLayerLock>(layer)?;
                }
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn copy_layers(&mut self, layers: Vec<EntityId>) -> Result<()> {
        let snapshot = self.snapshot();
        let mut selected = HashSet::new();
        let mut source_page = None;
        for layer in layers {
            let page = if snapshot.page(layer).is_ok() {
                layer
            } else {
                Self::containing_page(&snapshot, layer)?
            };
            if source_page
                .replace(page)
                .is_some_and(|current| current != page)
            {
                bail!("layers copied together must belong to one page");
            }
            if layer == page {
                selected.insert(layer);
            } else if snapshot.component::<SceneGroup>(layer)?.is_some() {
                for descendant in snapshot.descendants(layer)? {
                    if Self::is_content_layer(&snapshot, descendant.id())? {
                        selected.insert(descendant.id());
                    }
                }
            } else if Self::is_content_layer(&snapshot, layer)? {
                selected.insert(layer);
            } else {
                bail!("only content layers and layer groups can be copied");
            }
        }
        let source_page = source_page.context("copy requires at least one layer")?;
        if selected.is_empty() {
            bail!("the selected groups contain no copyable layers");
        }
        let mut ordered = Vec::with_capacity(selected.len());
        if selected.remove(&source_page) {
            ordered.push(source_page);
        }
        ordered.extend(
            snapshot
                .descendants(source_page)?
                .filter_map(|entity| selected.contains(&entity.id()).then_some(entity.id())),
        );
        let mut copied = Vec::with_capacity(ordered.len());
        for layer in ordered {
            copied.push(Self::copy_layer(&snapshot, layer).await?);
        }
        self.clipboard = Some(LayerClipboard {
            source_page,
            last_paste_page: None,
            paste_count: 0,
            layers: copied,
        });
        Ok(())
    }

    pub(crate) async fn paste_layers(&mut self, page: EntityId) -> Result<(Commit, Vec<EntityId>)> {
        let clipboard = self
            .clipboard
            .clone()
            .context("the layer clipboard is empty")?;
        let snapshot = self.snapshot();
        snapshot.page(page)?;
        let root_at = snapshot
            .page(page)?
            .text_group(SceneTextGroupKind::Main)?
            .map_or(At::End, |group| At::Before(group.id()));
        let previous_pastes = if clipboard.last_paste_page == Some(page) {
            clipboard.paste_count
        } else {
            0
        };
        let offset_steps = if clipboard.source_page == page {
            previous_pastes.saturating_add(1)
        } else {
            previous_pastes
        };
        let offset = 10.0 * f64::from(offset_steps);
        let mut pasted = Vec::with_capacity(clipboard.layers.len());
        let patch = snapshot.patch(|edit| {
            for copied in clipboard.layers {
                let layer = match copied {
                    CopiedLayer::Text {
                        layout,
                        frame,
                        typography,
                        visibility,
                        lock,
                        source,
                        translation,
                        role,
                        ocr_candidate,
                    } => {
                        let content = edit.add_text_content(page, At::End)?;
                        if let Some(value) = source {
                            edit.set(content, &value)?;
                        }
                        if let Some(value) = translation {
                            edit.set(content, &value)?;
                        }
                        edit.set_text_role(content, role.as_ref())?;
                        if let Some(value) = ocr_candidate {
                            edit.set(content, &value)?;
                        }
                        let layer = edit.add_text_layer(page, At::End, content, &layout)?;
                        if let Some(mut value) = frame {
                            offset_geometry(&mut value, offset);
                            edit.set(layer, &value)?;
                        }
                        if let Some(value) = typography {
                            edit.set(layer, &value)?;
                        }
                        if let Some(value) = visibility {
                            edit.set(layer, &value)?;
                        }
                        if let Some(value) = lock {
                            edit.set(layer, &value)?;
                        }
                        layer
                    }
                    CopiedLayer::Raster {
                        layer: value,
                        visibility,
                        lock,
                        asset,
                    } => {
                        let layer = edit.add_entity(page, root_at)?;
                        edit.set(layer, &value)?;
                        if let Some(value) = visibility {
                            edit.set(layer, &value)?;
                        }
                        if let Some(value) = lock {
                            edit.set(layer, &value)?;
                        }
                        if let Some(asset) = asset {
                            set_copied_asset(edit, layer, &asset)?;
                        }
                        layer
                    }
                    CopiedLayer::Image {
                        mut geometry,
                        visibility,
                        lock,
                        asset,
                    } => {
                        let layer = edit.add_entity(page, root_at)?;
                        offset_geometry(&mut geometry, offset);
                        edit.set(layer, &geometry)?;
                        if let Some(value) = visibility {
                            edit.set(layer, &value)?;
                        }
                        if let Some(value) = lock {
                            edit.set(layer, &value)?;
                        }
                        set_copied_asset(edit, layer, &asset)?;
                        layer
                    }
                    CopiedLayer::Bubble {
                        mut geometry,
                        shape,
                        style,
                        visibility,
                        lock,
                    } => {
                        offset_geometry(&mut geometry, offset);
                        let layer = edit
                            .add_analysis_region::<BubbleRegion>(page, root_at, &geometry, None)?;
                        edit.set(
                            layer,
                            &SceneRegionShape {
                                origin: Origin::User,
                                kind: shape,
                            },
                        )?;
                        edit.set(layer, &style)?;
                        if let Some(value) = visibility {
                            edit.set(layer, &value)?;
                        }
                        if let Some(value) = lock {
                            edit.set(layer, &value)?;
                        }
                        layer
                    }
                };
                pasted.push(layer);
            }
            Ok(())
        })?;
        let commit = self.commit(patch).await?;
        if let Some(clipboard) = self.clipboard.as_mut() {
            clipboard.paste_count = if clipboard.last_paste_page == Some(page) {
                clipboard.paste_count.saturating_add(1)
            } else {
                1
            };
            clipboard.last_paste_page = Some(page);
        }
        Ok((commit, pasted))
    }

    async fn copy_layer(snapshot: &Snapshot, layer: EntityId) -> Result<CopiedLayer> {
        let visibility = snapshot.component::<SceneVisibility>(layer)?;
        let lock = snapshot.component::<SceneLayerLock>(layer)?;
        if let Ok(page) = snapshot.page(layer) {
            let page = page.page()?;
            let asset = Self::copy_asset(snapshot, layer)
                .await?
                .context("the original artwork has no source asset")?;
            return Ok(CopiedLayer::Image {
                geometry: SceneGeometry::rectangle(0.0, 0.0, page.width, page.height),
                visibility,
                lock,
                asset,
            });
        }
        if snapshot.component::<SceneTextLayout>(layer)?.is_some() {
            let text = snapshot.text_layer(layer)?;
            let content = text.content()?;
            return Ok(CopiedLayer::Text {
                layout: text.layout()?,
                frame: text.frame()?,
                typography: text.typography()?,
                visibility,
                lock,
                source: content.source()?,
                translation: content.translation()?,
                role: content.role()?,
                ocr_candidate: snapshot.component::<SceneOcrCandidate>(content.id())?,
            });
        }
        if let Some(value) = snapshot.component::<SceneRasterLayer>(layer)? {
            return Ok(CopiedLayer::Raster {
                layer: value,
                visibility,
                lock,
                asset: Self::copy_asset(snapshot, layer).await?,
            });
        }
        if let Some(style) = snapshot.component::<SceneBubbleStyle>(layer)? {
            return Ok(CopiedLayer::Bubble {
                geometry: snapshot
                    .component::<SceneGeometry>(layer)?
                    .context("bubble layer has no geometry")?,
                shape: snapshot
                    .component::<SceneRegionShape>(layer)?
                    .map_or(RegionShapeKind::Rectangular, |shape| shape.kind),
                style,
                visibility,
                lock,
            });
        }
        let geometry = snapshot
            .component::<SceneGeometry>(layer)?
            .context("image layer has no geometry")?;
        let asset = Self::copy_asset(snapshot, layer)
            .await?
            .context("image layer has no source asset")?;
        Ok(CopiedLayer::Image {
            geometry,
            visibility,
            lock,
            asset,
        })
    }

    async fn copy_asset(snapshot: &Snapshot, layer: EntityId) -> Result<Option<CopiedAsset>> {
        let source = AssetRole::new("source")?;
        let Some(asset) = snapshot.asset(layer, &source)? else {
            return Ok(None);
        };
        let bytes = snapshot.read_blob(asset.blob).await?;
        Ok(Some(CopiedAsset {
            bytes: std::sync::Arc::from(bytes.as_ref()),
            media_type: asset.media_type,
            metadata: asset.metadata,
        }))
    }

    pub(crate) async fn delete_layers(&mut self, layers: Vec<EntityId>) -> Result<Commit> {
        let snapshot = self.snapshot();
        let mut expanded = Vec::new();
        for layer in layers {
            if Self::is_effectively_locked(&snapshot, layer)? {
                bail!("locked layers cannot be deleted");
            }
            if snapshot.component::<SceneTextGroup>(layer)?.is_some() {
                expanded.extend(snapshot.children(layer)?);
            } else {
                expanded.push(layer);
            }
        }
        for layer in &expanded {
            if Self::is_effectively_locked(&snapshot, *layer)? {
                bail!("groups containing locked layers cannot be deleted");
            }
        }
        let layers = Self::unique_roots(&snapshot, expanded)?;
        let mut orphaned_contents = Vec::new();
        for layer in &layers {
            let Some(relation) = snapshot.relation_from::<Presents>(*layer)? else {
                continue;
            };
            let content = relation.value().target;
            if snapshot.relations_to_as::<Presents>(content).count() == 1 {
                orphaned_contents.push(content);
            }
        }
        let patch = snapshot.patch(|edit| {
            for layer in layers {
                if snapshot.page(layer).is_ok() {
                    return Err(koharu_scene::Error::Invalid(
                        "delete pages with delete_pages".to_owned(),
                    ));
                }
                edit.remove_entity(layer, RemovePolicy::Cascade)?;
            }
            for content in orphaned_contents {
                if snapshot.entity(content).is_ok() {
                    edit.remove_entity(content, RemovePolicy::Cascade)?;
                }
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    #[tracing::instrument(level = "info", skip_all, fields(layer = %layer, parent = %parent))]
    pub(crate) async fn move_layer(
        &mut self,
        layer: EntityId,
        parent: EntityId,
        index: usize,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        if Self::is_effectively_locked(&snapshot, layer)? {
            bail!("locked layers cannot be reordered");
        }
        if snapshot.parent(layer)? != Some(parent) {
            bail!("layers can only be reordered within their current group");
        }
        let siblings = snapshot
            .children(parent)?
            .map(|candidate| Ok(Self::is_layer(&snapshot, candidate)?.then_some(candidate)))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let at = Self::placement(&siblings, layer, index);
        let segmentation_generation = Self::segmentation_generation(&snapshot, layer)?;
        let mut edit = segmentation_generation
            .clone()
            .map_or_else(|| snapshot.edit(), |owner| snapshot.edit_as(owner));
        if segmentation_generation.is_none() {
            edit.promote_entity_to_user(layer)?;
            if snapshot.component::<SceneTextGroup>(parent)?.is_some() {
                edit.promote_entity_to_user(parent)?;
            }
        }
        edit.move_entity(layer, Some(parent), at)?;
        let patch = edit.finish()?;
        self.commit(patch).await
    }

    pub(crate) async fn set_geometries(
        &mut self,
        geometries: impl IntoIterator<Item = (EntityId, SceneGeometry)>,
    ) -> Result<Commit> {
        let snapshot = self.snapshot();
        let geometries = geometries
            .into_iter()
            .map(|(element, geometry)| {
                if !Self::is_transformable_layer(&snapshot, element)? {
                    bail!("only text and image layers can change geometry");
                }
                if Self::is_effectively_locked(&snapshot, element)? {
                    bail!("locked layers cannot change geometry");
                }
                let content = snapshot
                    .component::<SceneTextLayout>(element)?
                    .map(|_| Self::text_content(&snapshot, element))
                    .transpose()?;
                Ok((element, geometry, content))
            })
            .collect::<Result<Vec<_>>>()?;
        let patch = snapshot.patch(|edit| {
            for (element, mut geometry, content) in geometries {
                edit.promote_entity_to_user(element)?;
                if let Some(content) = content {
                    edit.promote_entity_to_user(content)?;
                }
                geometry.origin = Origin::User;
                edit.set(element, &geometry)?;
            }
            Ok(())
        })?;
        self.commit(patch).await
    }

    pub(crate) async fn apply_raster_stroke(
        &mut self,
        page: EntityId,
        layer: Option<EntityId>,
        mode: RasterStrokeMode,
        new_layer_kind: RasterLayerKind,
        color: [u8; 4],
        diameter: f32,
        points: Vec<ScenePoint>,
    ) -> Result<(Commit, EntityId)> {
        if !diameter.is_finite() || diameter <= 0.0 || points.is_empty() {
            bail!("a raster stroke requires a positive diameter and at least one point");
        }
        if points
            .iter()
            .any(|point| !point.x.is_finite() || !point.y.is_finite())
        {
            bail!("raster stroke points must be finite");
        }
        let snapshot = self.snapshot();
        if let Some(layer) = layer
            && Self::is_effectively_locked(&snapshot, layer)?
        {
            bail!("locked raster layers cannot be edited");
        }
        let page_value = snapshot.page(page)?.page()?;
        let width = page_value.width.round() as u32;
        let height = page_value.height.round() as u32;
        if width == 0 || height == 0 {
            bail!("page dimensions must be positive");
        }
        let mut raster_layer = None;
        let mut promote_layer = false;
        let mut raster_generation = None;
        let source = AssetRole::new("source")?;
        let mut asset_attributes = Default::default();
        let mut placement = RasterPlacement::IDENTITY;
        let mut image = if let Some(layer) = layer {
            if layer == page {
                if mode != RasterStrokeMode::Erase {
                    bail!("the original artwork only accepts erase strokes");
                }
                let asset = snapshot
                    .asset(page, &source)?
                    .context("the original artwork has no source asset")?;
                asset_attributes = asset.metadata.attributes;
                let bytes = snapshot.read_blob(asset.blob).await?;
                let image = image::load_from_memory(&bytes)?.to_rgba8();
                placement = RasterPlacement::from_geometry(
                    &SceneGeometry::rectangle(0.0, 0.0, page_value.width, page_value.height),
                    image.width(),
                    image.height(),
                )?;
                promote_layer = snapshot
                    .component::<EntityOrigin>(page)?
                    .is_some_and(|origin| origin.origin != Origin::User);
                image
            } else if snapshot.parent(layer)? != Some(page) {
                bail!("the raster target must be a layer on the active page");
            } else if let Some(target) = snapshot.component::<SceneRasterLayer>(layer)? {
                if mode == RasterStrokeMode::Paint && target.kind != new_layer_kind {
                    bail!("the raster target kind does not match the selected brush");
                }
                if target.kind == RasterLayerKind::Segmentation {
                    raster_generation = Self::segmentation_generation(&snapshot, layer)?;
                } else {
                    let entity_origin = snapshot.component::<EntityOrigin>(layer)?;
                    promote_layer = target.origin != Origin::User
                        || entity_origin.is_some_and(|origin| origin.origin != Origin::User);
                }
                raster_layer = Some(target);
                match snapshot.asset(layer, &source)? {
                    Some(asset) => {
                        asset_attributes = asset.metadata.attributes;
                        let bytes = snapshot.read_blob(asset.blob).await?;
                        image::load_from_memory(&bytes)?.to_rgba8()
                    }
                    None => RgbaImage::new(width, height),
                }
            } else {
                if mode != RasterStrokeMode::Erase
                    || snapshot.component::<SceneTextLayout>(layer)?.is_some()
                    || snapshot.component::<SceneRegion>(layer)?.is_some()
                    || snapshot.component::<SceneGroup>(layer)?.is_some()
                {
                    bail!("the raster target must be a raster or imported image layer");
                }
                let geometry = snapshot
                    .component::<SceneGeometry>(layer)?
                    .context("the imported image target has no geometry")?;
                let asset = snapshot
                    .asset(layer, &source)?
                    .context("the imported image target has no source asset")?;
                asset_attributes = asset.metadata.attributes;
                let bytes = snapshot.read_blob(asset.blob).await?;
                let image = image::load_from_memory(&bytes)?.to_rgba8();
                placement =
                    RasterPlacement::from_geometry(&geometry, image.width(), image.height())?;
                promote_layer = snapshot
                    .component::<EntityOrigin>(layer)?
                    .is_some_and(|origin| origin.origin != Origin::User);
                image
            }
        } else {
            if mode == RasterStrokeMode::Erase {
                bail!("eraser requires a raster layer target");
            }
            RgbaImage::new(width, height)
        };
        if raster_layer.is_some() && image.dimensions() != (width, height) {
            bail!("raster layer dimensions must match the page");
        }
        rasterize_stroke_with_placement(&mut image, mode, color, diameter, &points, placement);
        let (output_width, output_height) = image.dimensions();
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image).write_to(&mut bytes, ImageFormat::Png)?;
        let layer_label = match new_layer_kind {
            RasterLayerKind::Censor => "Censor",
            RasterLayerKind::Paint => "Paint",
            RasterLayerKind::Cleanup => "Cleanup",
            RasterLayerKind::Segmentation => "Segmentation",
        };
        let name = format!(
            "{layer_label} {}",
            snapshot
                .children(page)?
                .filter(|entity| {
                    snapshot
                        .component::<SceneRasterLayer>(*entity)
                        .ok()
                        .flatten()
                        .is_some_and(|layer| layer.kind == new_layer_kind)
                })
                .count()
                + 1
        );
        let mut committed_layer = layer;
        let mut edit =
            raster_generation.map_or_else(|| snapshot.edit(), |owner| snapshot.edit_as(owner));
        {
            let layer = if let Some(layer) = committed_layer {
                if promote_layer {
                    edit.promote_entity_to_user(layer)?;
                    if let Some(raster) = raster_layer.as_mut() {
                        raster.origin = Origin::User;
                        edit.set(layer, raster)?;
                    }
                }
                layer
            } else {
                let at = snapshot
                    .page(page)?
                    .text_group(SceneTextGroupKind::Main)?
                    .map_or(At::End, |group| At::Before(group.id()));
                let layer = edit.add_entity(page, at)?;
                edit.set(
                    layer,
                    &SceneRasterLayer {
                        origin: Origin::User,
                        name,
                        kind: new_layer_kind,
                    },
                )?;
                committed_layer = Some(layer);
                layer
            };
            edit.set_asset(
                layer,
                &source,
                AssetInput::new(
                    bytes.into_inner(),
                    "image/png",
                    AssetMetadata {
                        width: Some(output_width),
                        height: Some(output_height),
                        attributes: asset_attributes,
                    },
                ),
            )?;
        }
        let patch = edit.finish()?;
        Ok((
            self.commit(patch).await?,
            committed_layer.expect("raster layer was selected or added while building the patch"),
        ))
    }

    pub(crate) async fn apply_raster_fill(
        &mut self,
        page: EntityId,
        layer: EntityId,
        point: ScenePoint,
        color: [u8; 4],
        tolerance: u8,
    ) -> Result<Option<Commit>> {
        if !point.x.is_finite() || !point.y.is_finite() {
            bail!("raster fill point must be finite");
        }
        let snapshot = self.snapshot();
        if Self::is_effectively_locked(&snapshot, layer)? {
            bail!("locked raster layers cannot be edited");
        }
        let page_value = snapshot.page(page)?.page()?;
        let page_width = page_value.width.round() as u32;
        let page_height = page_value.height.round() as u32;
        if page_width == 0 || page_height == 0 {
            bail!("page dimensions must be positive");
        }

        let source = AssetRole::new("source")?;
        let mut asset_attributes = Default::default();
        let mut raster_layer = None;
        let promote_layer;
        let (mut image, placement) = if layer == page {
            let asset = snapshot
                .asset(page, &source)?
                .context("the original artwork has no source asset")?;
            asset_attributes = asset.metadata.attributes;
            let bytes = snapshot.read_blob(asset.blob).await?;
            let image = image::load_from_memory(&bytes)?.to_rgba8();
            let placement = RasterPlacement::from_geometry(
                &SceneGeometry::rectangle(0.0, 0.0, page_value.width, page_value.height),
                image.width(),
                image.height(),
            )?;
            promote_layer = snapshot
                .component::<EntityOrigin>(page)?
                .is_some_and(|origin| origin.origin != Origin::User);
            (image, placement)
        } else {
            if snapshot.parent(layer)? != Some(page) {
                bail!("the fill target must be a layer on the active page");
            }
            if let Some(target) = snapshot.component::<SceneRasterLayer>(layer)? {
                if target.kind != RasterLayerKind::Paint {
                    bail!("only paint raster layers accept color fills");
                }
                promote_layer = target.origin != Origin::User
                    || snapshot
                        .component::<EntityOrigin>(layer)?
                        .is_some_and(|origin| origin.origin != Origin::User);
                raster_layer = Some(target);
                let image = match snapshot.asset(layer, &source)? {
                    Some(asset) => {
                        asset_attributes = asset.metadata.attributes;
                        let bytes = snapshot.read_blob(asset.blob).await?;
                        image::load_from_memory(&bytes)?.to_rgba8()
                    }
                    None => RgbaImage::new(page_width, page_height),
                };
                if image.dimensions() != (page_width, page_height) {
                    bail!("raster layer dimensions must match the page");
                }
                (image, RasterPlacement::IDENTITY)
            } else {
                if snapshot.component::<SceneTextLayout>(layer)?.is_some()
                    || snapshot.component::<SceneRegion>(layer)?.is_some()
                    || snapshot.component::<SceneGroup>(layer)?.is_some()
                {
                    bail!("the fill target must be a paint or image layer");
                }
                let geometry = snapshot
                    .component::<SceneGeometry>(layer)?
                    .context("the imported image target has no geometry")?;
                let asset = snapshot
                    .asset(layer, &source)?
                    .context("the imported image target has no source asset")?;
                asset_attributes = asset.metadata.attributes;
                let bytes = snapshot.read_blob(asset.blob).await?;
                let image = image::load_from_memory(&bytes)?.to_rgba8();
                let placement =
                    RasterPlacement::from_geometry(&geometry, image.width(), image.height())?;
                promote_layer = snapshot
                    .component::<EntityOrigin>(layer)?
                    .is_some_and(|origin| origin.origin != Origin::User);
                (image, placement)
            }
        };

        let (seed_x, seed_y) = placement.page_to_raster(point.x, point.y);
        if !seed_x.is_finite()
            || !seed_y.is_finite()
            || seed_x < 0.0
            || seed_y < 0.0
            || seed_x >= f64::from(image.width())
            || seed_y >= f64::from(image.height())
        {
            bail!("fill point is outside the selected image layer");
        }
        if !rasterize_fill(
            &mut image,
            seed_x.floor() as u32,
            seed_y.floor() as u32,
            color,
            tolerance,
        ) {
            return Ok(None);
        }

        let (width, height) = image.dimensions();
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image).write_to(&mut bytes, ImageFormat::Png)?;
        let patch = snapshot.patch(|edit| {
            if promote_layer {
                edit.promote_entity_to_user(layer)?;
                if let Some(mut raster) = raster_layer {
                    raster.origin = Origin::User;
                    edit.set(layer, &raster)?;
                }
            }
            edit.set_asset(
                layer,
                &source,
                AssetInput::new(
                    bytes.into_inner(),
                    "image/png",
                    AssetMetadata {
                        width: Some(width),
                        height: Some(height),
                        attributes: asset_attributes,
                    },
                ),
            )
        })?;
        Ok(Some(self.commit(patch).await?))
    }

    pub(crate) async fn undo(&mut self) -> Result<Commit> {
        let revisions = self.undo.pop().ok_or_else(|| anyhow!("nothing to undo"))?;
        let commit = match self.session.undo_many(revisions.iter().copied()).await {
            Ok(commit) => commit,
            Err(error) => {
                self.undo.push(revisions);
                return Err(error.into());
            }
        };
        self.redo.push(vec![commit.revision]);
        Ok(commit)
    }

    pub(crate) async fn redo(&mut self) -> Result<Commit> {
        let revisions = self.redo.pop().ok_or_else(|| anyhow!("nothing to redo"))?;
        let commit = match self.session.undo_many(revisions.iter().copied()).await {
            Ok(commit) => commit,
            Err(error) => {
                self.redo.push(revisions);
                return Err(error.into());
            }
        };
        self.undo.push(vec![commit.revision]);
        Ok(commit)
    }

    pub(crate) fn record(&mut self, revisions: Vec<Revision>) {
        if !revisions.is_empty() {
            self.undo.push(revisions);
            self.redo.clear();
        }
    }

    pub(crate) fn record_commit(&mut self, commit: &Commit) {
        if commit.changes.to != commit.changes.from {
            self.record(vec![commit.revision]);
        }
    }

    pub(crate) async fn commit_rebased(
        &mut self,
        patch: koharu_scene::Patch,
    ) -> Result<Option<Commit>> {
        let current = self.snapshot();
        let patch = match patch.rebase_on(&current) {
            Ok(patch) => patch,
            Err(
                error @ (koharu_scene::Error::PatchConflict(_)
                | koharu_scene::Error::EntityNotFound(_)
                | koharu_scene::Error::RelationNotFound(_)),
            ) => {
                tracing::debug!(%error, "pipeline output was superseded by a document edit");
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        if patch.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.commit(patch).await?))
    }

    async fn commit(&mut self, patch: koharu_scene::Patch) -> Result<Commit> {
        Ok(self.session.commit(patch).await?)
    }

    pub(crate) fn page(snapshot: &Snapshot, page: EntityId) -> Result<Page> {
        let value = snapshot.page(page)?.page()?;
        let source = Self::asset_id(snapshot, page, "source")?;
        let mut layers = Vec::new();
        Self::collect_layer_views(snapshot, page, &mut layers)?;
        if let Some(image) = source.clone() {
            layers.insert(
                0,
                Layer::Artwork {
                    id: page,
                    parent: None,
                    locked: true,
                    geometry: Geometry {
                        points: vec![
                            Point { x: 0.0, y: 0.0 },
                            Point {
                                x: value.width,
                                y: 0.0,
                            },
                            Point {
                                x: value.width,
                                y: value.height,
                            },
                            Point {
                                x: 0.0,
                                y: value.height,
                            },
                        ],
                    },
                    visibility: LayerVisibility {
                        visible: true,
                        opacity: 1.0,
                    },
                    image,
                },
            );
        }
        let regions = snapshot
            .descendants(page)?
            .map(|entity| {
                let id = entity.id();
                snapshot
                    .component::<SceneRegion>(id)?
                    .map(|region| Self::analysis_region(snapshot, id, region))
                    .transpose()
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        Ok(Page {
            id: page,
            label: value.label,
            size: PageSize {
                width: value.width,
                height: value.height,
            },
            layers,
            regions,
        })
    }

    fn layer_view(snapshot: &Snapshot, layer: EntityId) -> Result<Layer> {
        let parent = snapshot.parent(layer)?;
        let locked = Self::layer_locked(snapshot, layer)?;
        let visibility = Self::layer_visibility(snapshot, layer)?;
        if let Some(group) = snapshot.component::<SceneGroup>(layer)? {
            return Ok(Layer::Group {
                id: layer,
                parent,
                locked,
                visibility,
                name: group.name,
                role: snapshot
                    .component::<SceneTextGroup>(layer)?
                    .map(|group| match group.kind {
                        SceneTextGroupKind::Main => GroupRole::MainText,
                        SceneTextGroupKind::Sfx => GroupRole::SfxText,
                    }),
            });
        }
        if let Some(layout) = snapshot.component::<SceneTextLayout>(layer)? {
            let text_layer = snapshot.text_layer(layer)?;
            let content = text_layer.content()?;
            let source = content.source()?.map(|source| SourceText {
                text: source.text.value,
                language: source.language.map(|language| language.to_string()),
            });
            let translation = content.translation()?.map(|translation| Translation {
                text: translation.text.value,
                language: translation.language.map(|language| language.to_string()),
            });
            let classification = content
                .role()?
                .and_then(|role| TextClassification::from_role(&role.role));
            let source_region = content.source_region()?.map(|region| region.id());
            let include_for_ocr = snapshot
                .component::<SceneOcrCandidate>(content.id())?
                .is_some();
            let typography = text_layer.typography()?.map(Self::typography_view);
            return Ok(Layer::Text {
                id: layer,
                parent,
                locked,
                geometry: snapshot
                    .component::<SceneGeometry>(layer)?
                    .map(Self::geometry_view),
                visibility,
                content: Box::new(TextContent {
                    id: content.id(),
                    source,
                    translation,
                    classification,
                    source_region,
                    include_for_ocr,
                }),
                typography,
                layout: layout.kind,
            });
        }
        if let Some(raster) = snapshot.component::<SceneRasterLayer>(layer)? {
            return Ok(Layer::Raster {
                id: layer,
                parent,
                locked,
                visibility,
                image: Self::asset_id(snapshot, layer, "source")?,
                name: raster.name,
                kind: raster.kind,
            });
        }
        if let Some(style) = snapshot.component::<SceneBubbleStyle>(layer)? {
            return Ok(Layer::Bubble {
                id: layer,
                parent,
                locked,
                geometry: snapshot
                    .component::<SceneGeometry>(layer)?
                    .map(Self::geometry_view)
                    .context("bubble layer has no geometry")?,
                visibility,
                shape: snapshot
                    .component::<SceneRegionShape>(layer)?
                    .map_or(RegionShapeKind::Rectangular, |shape| shape.kind),
                fill_color: style.fill_color,
                outline_color: style.outline_color,
                outline_width: style.outline_width,
            });
        }
        let geometry = snapshot
            .component::<SceneGeometry>(layer)?
            .map(Self::geometry_view)
            .context("image layer has no geometry")?;
        let image = Self::asset_id(snapshot, layer, "source")?
            .context("image layer has no source asset")?;
        Ok(Layer::Image {
            id: layer,
            parent,
            locked,
            geometry,
            visibility,
            image,
        })
    }

    fn analysis_region(
        snapshot: &Snapshot,
        id: EntityId,
        region: SceneRegion,
    ) -> Result<AnalysisRegion> {
        let geometry = snapshot
            .component::<SceneGeometry>(id)?
            .map(Self::geometry_view)
            .context("analysis region has no geometry")?;
        Ok(AnalysisRegion {
            id,
            parent: snapshot.parent(id)?,
            geometry,
            kind: region.kind.as_str().to_owned(),
            label: region.label,
            shape: snapshot
                .component::<SceneRegionShape>(id)?
                .map_or(RegionShapeKind::Rectangular, |shape| shape.kind),
        })
    }

    fn layer_visibility(snapshot: &Snapshot, layer: EntityId) -> Result<LayerVisibility> {
        Ok(snapshot.component::<SceneVisibility>(layer)?.map_or(
            LayerVisibility {
                visible: true,
                opacity: 1.0,
            },
            |visibility| LayerVisibility {
                visible: visibility.visible,
                opacity: visibility.opacity,
            },
        ))
    }

    fn layer_locked(snapshot: &Snapshot, layer: EntityId) -> Result<bool> {
        Ok(snapshot
            .component::<SceneLayerLock>(layer)?
            .is_some_and(|lock| lock.locked))
    }

    fn is_effectively_locked(snapshot: &Snapshot, layer: EntityId) -> Result<bool> {
        let mut current = Some(layer);
        while let Some(entity) = current {
            if Self::layer_locked(snapshot, entity)? {
                return Ok(true);
            }
            current = snapshot.parent(entity)?;
        }
        Ok(false)
    }

    fn typography_view(typography: SceneTypography) -> Typography {
        let writing_mode = if matches!(&typography.origin, Origin::User) {
            typography.writing_mode
        } else {
            None
        };
        Typography {
            preferred_font: typography.preferred_font,
            font_weight: typography.font_weight,
            font_style: typography.font_style,
            size: typography.size,
            auto_fit: typography.auto_fit,
            color: typography.color,
            stroke_color: typography.stroke_color,
            stroke_width: typography.stroke_width,
            paragraph_spacing: typography.paragraph_spacing,
            style_ranges: typography.style_ranges,
            alignment: typography.alignment,
            writing_mode,
        }
    }

    fn geometry_view(geometry: SceneGeometry) -> Geometry {
        Geometry {
            points: geometry
                .points
                .into_iter()
                .map(|point| Point {
                    x: point.x,
                    y: point.y,
                })
                .collect(),
        }
    }

    fn asset_id(snapshot: &Snapshot, entity: EntityId, role: &str) -> Result<Option<String>> {
        Ok(snapshot
            .asset(entity, &AssetRole::new(role)?)?
            .map(|asset| asset.blob.to_string()))
    }

    fn is_layer(snapshot: &Snapshot, entity: EntityId) -> Result<bool> {
        Ok(snapshot.component::<SceneGroup>(entity)?.is_some()
            || snapshot.component::<SceneTextLayout>(entity)?.is_some()
            || snapshot.component::<SceneRasterLayer>(entity)?.is_some()
            || snapshot.component::<SceneBubbleStyle>(entity)?.is_some()
            || (snapshot.component::<SceneGeometry>(entity)?.is_some()
                && snapshot
                    .asset(entity, &AssetRole::new("source")?)?
                    .is_some()))
    }

    fn segmentation_generation(
        snapshot: &Snapshot,
        entity: EntityId,
    ) -> Result<Option<Generation>> {
        if !snapshot
            .component::<SceneRasterLayer>(entity)?
            .is_some_and(|layer| layer.kind == RasterLayerKind::Segmentation)
        {
            return Ok(None);
        }
        Ok(snapshot
            .component::<EntityOrigin>(entity)?
            .and_then(|origin| match origin.origin {
                Origin::Generated(generation) => Some(generation),
                Origin::User => None,
            }))
    }

    fn is_transformable_layer(snapshot: &Snapshot, entity: EntityId) -> Result<bool> {
        Ok(snapshot.component::<SceneTextLayout>(entity)?.is_some()
            || snapshot.component::<SceneBubbleStyle>(entity)?.is_some()
            || (snapshot.component::<SceneGeometry>(entity)?.is_some()
                && snapshot.component::<SceneRegion>(entity)?.is_none()
                && snapshot.component::<SceneRasterLayer>(entity)?.is_none()
                && snapshot.component::<SceneGroup>(entity)?.is_none()
                && snapshot
                    .asset(entity, &AssetRole::new("source")?)?
                    .is_some()))
    }

    fn is_content_layer(snapshot: &Snapshot, entity: EntityId) -> Result<bool> {
        Ok(
            snapshot.component::<SceneGroup>(entity)?.is_none()
                && Self::is_layer(snapshot, entity)?,
        )
    }

    fn containing_page(snapshot: &Snapshot, entity: EntityId) -> Result<EntityId> {
        snapshot.entity(entity)?;
        let mut current = entity;
        loop {
            if snapshot.page(current).is_ok() {
                return Ok(current);
            }
            current = snapshot
                .parent(current)?
                .context("layer is not contained by a page")?;
        }
    }

    fn collect_layer_views(
        snapshot: &Snapshot,
        parent: EntityId,
        layers: &mut Vec<Layer>,
    ) -> Result<()> {
        for child in snapshot.children(parent)? {
            if !Self::is_layer(snapshot, child)? {
                continue;
            }
            let group = snapshot.component::<SceneGroup>(child)?.is_some();
            layers.push(Self::layer_view(snapshot, child)?);
            if group {
                Self::collect_layer_views(snapshot, child, layers)?;
            }
        }
        Ok(())
    }

    fn text_content(snapshot: &Snapshot, layer: EntityId) -> Result<EntityId> {
        snapshot
            .text_layer(layer)?
            .content()
            .map(|content| content.id())
            .map_err(Into::into)
    }

    fn placement(siblings: &[EntityId], moving: EntityId, index: usize) -> At {
        siblings
            .iter()
            .copied()
            .filter(|entity| *entity != moving)
            .nth(index)
            .map_or(At::End, At::Before)
    }

    fn unique_roots(snapshot: &Snapshot, entities: Vec<EntityId>) -> Result<Vec<EntityId>> {
        let selected = entities.into_iter().collect::<HashSet<_>>();
        let mut roots = Vec::new();
        for entity in selected.iter().copied() {
            let mut parent = snapshot.parent(entity)?;
            let mut nested = false;
            while let Some(value) = parent {
                if selected.contains(&value) {
                    nested = true;
                    break;
                }
                parent = snapshot.parent(value)?;
            }
            if !nested {
                roots.push(entity);
            }
        }
        roots.sort_unstable();
        Ok(roots)
    }

    fn geometry_from_frame(frame: Frame) -> Result<SceneGeometry> {
        if !frame.x.is_finite()
            || !frame.y.is_finite()
            || !frame.width.is_finite()
            || !frame.height.is_finite()
            || !frame.angle_degrees.is_finite()
            || frame.width <= 0.0
            || frame.height <= 0.0
        {
            bail!("frame must contain finite coordinates and positive dimensions");
        }
        let center_x = f64::from(frame.x + frame.width * 0.5);
        let center_y = f64::from(frame.y + frame.height * 0.5);
        let half_width = f64::from(frame.width) * 0.5;
        let half_height = f64::from(frame.height) * 0.5;
        let (sin, cos) = f64::from(frame.angle_degrees).to_radians().sin_cos();
        Ok(SceneGeometry {
            origin: Origin::User,
            points: [
                (-half_width, -half_height),
                (half_width, -half_height),
                (half_width, half_height),
                (-half_width, half_height),
            ]
            .map(|(x, y)| ScenePoint {
                x: center_x + x * cos - y * sin,
                y: center_y + x * sin + y * cos,
            })
            .into(),
        })
    }
}

fn offset_geometry(geometry: &mut SceneGeometry, offset: f64) {
    for point in &mut geometry.points {
        point.x += offset;
        point.y += offset;
    }
}

fn set_copied_asset(
    edit: &mut SceneEdit,
    layer: EntityId,
    asset: &CopiedAsset,
) -> koharu_scene::Result<()> {
    edit.set_asset(
        layer,
        &AssetRole::new("source")?,
        AssetInput::new(
            asset.bytes.clone(),
            asset.media_type.clone(),
            asset.metadata.clone(),
        ),
    )?;
    Ok(())
}

fn validate_project_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("project name cannot be empty");
    }
    if name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character))
    {
        bail!("project name contains characters that cannot be used in a file name");
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
    {
        bail!("project name is reserved by Windows");
    }
    Ok(name.to_owned())
}

#[cfg(test)]
fn rasterize_stroke(
    image: &mut RgbaImage,
    mode: RasterStrokeMode,
    color: [u8; 4],
    diameter: f32,
    points: &[ScenePoint],
) {
    rasterize_stroke_with_placement(
        image,
        mode,
        color,
        diameter,
        points,
        RasterPlacement::IDENTITY,
    );
}

fn rasterize_stroke_with_placement(
    image: &mut RgbaImage,
    mode: RasterStrokeMode,
    color: [u8; 4],
    diameter: f32,
    points: &[ScenePoint],
    placement: RasterPlacement,
) {
    let radius = f64::from(diameter) * 0.5;
    let transition = placement.pixel_size().max(f64::EPSILON);
    let edge = transition * 0.5;
    for (start, end) in points
        .iter()
        .zip(points.iter().skip(1))
        .chain(points.last().map(|point| (point, point)))
    {
        let page_left = start.x.min(end.x) - radius - edge;
        let page_top = start.y.min(end.y) - radius - edge;
        let page_right = start.x.max(end.x) + radius + edge;
        let page_bottom = start.y.max(end.y) + radius + edge;
        let corners = [
            placement.page_to_raster(page_left, page_top),
            placement.page_to_raster(page_right, page_top),
            placement.page_to_raster(page_right, page_bottom),
            placement.page_to_raster(page_left, page_bottom),
        ];
        let left = corners
            .iter()
            .map(|point| point.0)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0) as u32;
        let top = corners
            .iter()
            .map(|point| point.1)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0) as u32;
        let right = corners
            .iter()
            .map(|point| point.0)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .min(f64::from(image.width())) as u32;
        let bottom = corners
            .iter()
            .map(|point| point.1)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .min(f64::from(image.height())) as u32;
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let length_squared = dx * dx + dy * dy;
        for y in top..bottom {
            for x in left..right {
                let (px, py) = placement.raster_to_page(f64::from(x) + 0.5, f64::from(y) + 0.5);
                let t = if length_squared == 0.0 {
                    0.0
                } else {
                    (((px - start.x) * dx + (py - start.y) * dy) / length_squared).clamp(0.0, 1.0)
                };
                let distance =
                    ((px - (start.x + t * dx)).powi(2) + (py - (start.y + t * dy)).powi(2)).sqrt();
                let coverage = ((radius + edge - distance) / transition).clamp(0.0, 1.0) as f32;
                if coverage == 0.0 {
                    continue;
                }
                let pixel = image.get_pixel_mut(x, y);
                match mode {
                    RasterStrokeMode::Paint => {
                        let source_alpha = f32::from(color[3]) / 255.0 * coverage;
                        let destination_alpha = f32::from(pixel[3]) / 255.0;
                        let output_alpha = source_alpha + destination_alpha * (1.0 - source_alpha);
                        if output_alpha > 0.0 {
                            for channel in 0..3 {
                                let source = f32::from(color[channel]) / 255.0;
                                let destination = f32::from(pixel[channel]) / 255.0;
                                pixel[channel] = (((source * source_alpha
                                    + destination * destination_alpha * (1.0 - source_alpha))
                                    / output_alpha)
                                    * 255.0)
                                    .round() as u8;
                            }
                        }
                        pixel[3] = (output_alpha * 255.0).round() as u8;
                    }
                    RasterStrokeMode::Erase => {
                        pixel[3] = (f32::from(pixel[3]) * (1.0 - coverage)).round() as u8;
                    }
                }
            }
        }
    }
}

fn rasterize_fill(
    image: &mut RgbaImage,
    seed_x: u32,
    seed_y: u32,
    replacement: [u8; 4],
    tolerance: u8,
) -> bool {
    let width = image.width();
    let height = image.height();
    if seed_x >= width || seed_y >= height {
        return false;
    }
    let seed = image.get_pixel(seed_x, seed_y).0;
    let matches_seed = |pixel: [u8; 4]| {
        if seed[3] == 0 && pixel[3] == 0 {
            true
        } else {
            seed.iter()
                .zip(pixel)
                .all(|(left, right)| left.abs_diff(right) <= tolerance)
        }
    };
    let length = usize::try_from(u64::from(width) * u64::from(height))
        .expect("decoded image dimensions fit in addressable memory");
    let mut visited = vec![false; length];
    let mut pending = VecDeque::from([(seed_x, seed_y)]);
    visited[usize::try_from(u64::from(seed_y) * u64::from(width) + u64::from(seed_x))
        .expect("pixel index fits in addressable memory")] = true;
    let mut changed = false;

    while let Some((x, y)) = pending.pop_front() {
        let pixel = image.get_pixel(x, y).0;
        if !matches_seed(pixel) {
            continue;
        }
        if pixel != replacement {
            image.put_pixel(x, y, image::Rgba(replacement));
            changed = true;
        }
        for (next_x, next_y) in [
            x.checked_sub(1).map(|next| (next, y)),
            x.checked_add(1)
                .filter(|next| *next < width)
                .map(|next| (next, y)),
            y.checked_sub(1).map(|next| (x, next)),
            y.checked_add(1)
                .filter(|next| *next < height)
                .map(|next| (x, next)),
        ]
        .into_iter()
        .flatten()
        {
            let index = usize::try_from(u64::from(next_y) * u64::from(width) + u64::from(next_x))
                .expect("pixel index fits in addressable memory");
            if !visited[index] {
                visited[index] = true;
                pending.push_back((next_x, next_y));
            }
        }
    }
    changed
}

fn reconcile_style_ranges(
    ranges: &mut Vec<koharu_scene::TextStyleRange>,
    old_text: &str,
    new_text: &str,
) {
    if old_text == new_text || ranges.is_empty() {
        return;
    }

    let old = old_text.chars().collect::<Vec<_>>();
    let new = new_text.chars().collect::<Vec<_>>();
    let common_prefix = old
        .iter()
        .zip(&new)
        .take_while(|(left, right)| left == right)
        .count();
    let common_suffix = old[common_prefix..]
        .iter()
        .rev()
        .zip(new[common_prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let utf16_len = |text: &[char]| {
        text.iter()
            .map(|value| value.len_utf16() as u32)
            .sum::<u32>()
    };
    let prefix = utf16_len(&old[..common_prefix]);
    let suffix = utf16_len(&old[old.len() - common_suffix..]);
    let old_suffix_start = utf16_len(&old).saturating_sub(suffix);
    let new_suffix_start = utf16_len(&new).saturating_sub(suffix);
    let map = |position: u32, end: bool| {
        if position < prefix {
            position
        } else if position > old_suffix_start {
            new_suffix_start.saturating_add(position - old_suffix_start)
        } else if prefix == old_suffix_start {
            if end { prefix } else { new_suffix_start }
        } else if position == prefix {
            prefix
        } else if position == old_suffix_start {
            new_suffix_start
        } else if end {
            new_suffix_start
        } else {
            prefix
        }
    };

    for range in ranges.iter_mut() {
        range.start = map(range.start, false);
        range.end = map(range.end, true);
    }
    ranges.retain(|range| range.start < range.end);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use koharu_scene::{
        Generation, OcrCandidate, ProducerId, RegionSpec, TextStyleRange, WritingMode,
    };

    use super::*;

    #[test]
    fn text_edits_reconcile_utf16_style_ranges() {
        let color = Some([255, 0, 0, 255]);
        let mut ranges = vec![TextStyleRange {
            start: 3,
            end: 5,
            color,
            stroke_color: None,
        }];

        reconcile_style_ranges(&mut ranges, "A🙂BC", "A🙂xBC");
        assert_eq!(ranges[0].start, 4);
        assert_eq!(ranges[0].end, 6);

        let mut deleted = vec![TextStyleRange {
            start: 1,
            end: 3,
            color,
            stroke_color: None,
        }];
        reconcile_style_ranges(&mut deleted, "A🙂BC", "ABC");
        assert!(deleted.is_empty());
    }

    #[test]
    fn only_user_authored_direction_is_projected_as_an_override() {
        let mut typography = SceneTypography {
            origin: Origin::Generated(Generation::new(
                ProducerId::new("dev.koharu.pipeline.detection").expect("valid producer"),
            )),
            preferred_font: None,
            font_weight: None,
            font_style: None,
            size: None,
            auto_fit: true,
            color: None,
            stroke_color: None,
            stroke_width: None,
            paragraph_spacing: None,
            style_ranges: Vec::new(),
            alignment: None,
            writing_mode: Some(WritingMode::Vertical),
            extensions: Default::default(),
        };

        assert_eq!(
            Project::typography_view(typography.clone()).writing_mode,
            None
        );
        typography.origin = Origin::User;
        assert_eq!(
            Project::typography_view(typography).writing_mode,
            Some(WritingMode::Vertical)
        );
    }

    #[tokio::test]
    async fn added_text_boxes_are_blank_ocr_candidates_with_source_regions() {
        let mut session = Session::memory().await.unwrap();
        let mut page = None;
        let patch = session
            .snapshot()
            .patch(|edit| {
                page = Some(edit.add_page(PageDraft::new("page", 500.0, 700.0), At::End)?);
                Ok(())
            })
            .unwrap();
        session.commit(patch).await.unwrap();
        let page = page.unwrap();
        let mut project = Project::new(session, "test".to_owned());

        let (_, layer) = project
            .add_text_box(
                page,
                Frame {
                    x: 20.0,
                    y: 30.0,
                    width: 140.0,
                    height: 80.0,
                    angle_degrees: 0.0,
                },
            )
            .await
            .unwrap();

        let snapshot = project.snapshot();
        let text = snapshot.text_layer(layer).unwrap();
        let content = text.content().unwrap();
        let region = content.source_region().unwrap().unwrap();
        assert_eq!(content.source().unwrap().unwrap().text.value, "");
        assert!(
            snapshot
                .component::<OcrCandidate>(content.id())
                .unwrap()
                .is_some()
        );
        assert_eq!(text.fit_target().unwrap().unwrap().id(), region.id());
        assert!(
            snapshot
                .component::<SceneGeometry>(layer)
                .unwrap()
                .is_none()
        );
        assert_eq!(text.layout().unwrap().kind, TextLayoutKind::Paragraph);

        project.set_ocr_inclusion(layer, false).await.unwrap();
        assert!(
            project
                .snapshot()
                .component::<OcrCandidate>(content.id())
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn imported_images_fill_the_page_and_persist_transform_locks() {
        let mut session = Session::memory().await.unwrap();
        let mut page = None;
        let patch = session
            .snapshot()
            .patch(|edit| {
                page = Some(edit.add_page(PageDraft::new("page", 500.0, 700.0), At::End)?);
                Ok(())
            })
            .unwrap();
        session.commit(patch).await.unwrap();
        let page = page.unwrap();
        let mut project = Project::new(session, "test".to_owned());

        let (_, layer) = project
            .add_image_layer(
                page,
                crate::commands::import::Page {
                    name: "alternate.png".to_owned(),
                    bytes: Arc::from([1_u8, 2, 3].as_slice()),
                    format: ImageFormat::Png,
                    width: 100,
                    height: 200,
                },
            )
            .await
            .unwrap();
        let snapshot = project.snapshot();
        assert_eq!(snapshot.parent(layer).unwrap(), Some(page));
        assert_eq!(
            snapshot
                .component::<SceneGeometry>(layer)
                .unwrap()
                .unwrap()
                .points,
            vec![
                ScenePoint { x: 0.0, y: 0.0 },
                ScenePoint { x: 500.0, y: 0.0 },
                ScenePoint { x: 500.0, y: 700.0 },
                ScenePoint { x: 0.0, y: 700.0 },
            ]
        );

        project.set_layer_locked(vec![layer], true).await.unwrap();
        assert!(
            project
                .set_geometries([(
                    layer,
                    SceneGeometry {
                        origin: Origin::User,
                        points: vec![
                            ScenePoint { x: 0.0, y: 0.0 },
                            ScenePoint { x: 250.0, y: 0.0 },
                            ScenePoint { x: 250.0, y: 350.0 },
                            ScenePoint { x: 0.0, y: 350.0 },
                        ],
                    },
                )])
                .await
                .is_err()
        );
        assert!(
            project
                .snapshot()
                .component::<SceneLayerLock>(layer)
                .unwrap()
                .is_some_and(|lock| lock.locked)
        );
    }

    #[tokio::test]
    async fn bubble_layers_create_edit_copy_and_undo_as_regions() {
        let mut session = Session::memory().await.unwrap();
        let mut page = None;
        let patch = session
            .snapshot()
            .patch(|edit| {
                page = Some(edit.add_page(PageDraft::new("page", 500.0, 700.0), At::End)?);
                Ok(())
            })
            .unwrap();
        session.commit(patch).await.unwrap();
        let page = page.unwrap();
        let mut project = Project::new(session, "test".to_owned());

        let (_, bubble) = project
            .add_bubble_layer(page, RegionShapeKind::Circular)
            .await
            .unwrap();
        let snapshot = project.snapshot();
        assert_eq!(
            snapshot
                .component::<SceneRegion>(bubble)
                .unwrap()
                .unwrap()
                .kind,
            BubbleRegion::kind()
        );
        let geometry = snapshot
            .component::<SceneGeometry>(bubble)
            .unwrap()
            .unwrap();
        assert_eq!(geometry.points[0], ScenePoint { x: 170.0, y: 303.6 });
        assert!(matches!(
            Project::page(&snapshot, page)
                .unwrap()
                .layers
                .into_iter()
                .find(|layer| matches!(layer, Layer::Bubble { id, .. } if *id == bubble))
                .unwrap(),
            Layer::Bubble {
                shape: RegionShapeKind::Circular,
                fill_color: [255, 255, 255, 255],
                outline_color: [0, 0, 0, 255],
                outline_width: 3.0,
                ..
            }
        ));

        project
            .set_bubble_style(
                bubble,
                BubbleStyleUpdate {
                    shape: RegionShapeKind::Rectangular,
                    fill_color: [240, 220, 200, 255],
                    outline_color: [20, 30, 40, 255],
                    outline_width: 6.5,
                },
            )
            .await
            .unwrap();
        project.copy_layers(vec![bubble]).await.unwrap();
        let (commit, pasted) = project.paste_layers(page).await.unwrap();
        project.record_commit(&commit);
        let pasted = pasted[0];
        let snapshot = project.snapshot();
        assert_eq!(
            snapshot
                .component::<SceneBubbleStyle>(pasted)
                .unwrap()
                .unwrap()
                .outline_width,
            6.5
        );
        assert_eq!(
            snapshot
                .component::<SceneGeometry>(pasted)
                .unwrap()
                .unwrap()
                .points[0],
            ScenePoint { x: 180.0, y: 313.6 }
        );

        project.undo().await.unwrap();
        assert!(project.snapshot().entity(pasted).is_err());
    }

    #[tokio::test]
    async fn copied_text_and_image_layers_paste_with_detached_content_and_undo() {
        let mut session = Session::memory().await.unwrap();
        let mut page = None;
        let patch = session
            .snapshot()
            .patch(|edit| {
                page = Some(edit.add_page(PageDraft::new("page", 500.0, 700.0), At::End)?);
                Ok(())
            })
            .unwrap();
        session.commit(patch).await.unwrap();
        let page = page.unwrap();
        let mut project = Project::new(session, "test".to_owned());

        let (_, text) = project
            .add_text_box(
                page,
                Frame {
                    x: 20.0,
                    y: 30.0,
                    width: 140.0,
                    height: 80.0,
                    angle_degrees: 0.0,
                },
            )
            .await
            .unwrap();
        project
            .set_source_text(text, "boom".to_owned())
            .await
            .unwrap();
        project
            .set_translation(text, Some("kaboom".to_owned()))
            .await
            .unwrap();
        project
            .set_text_classification(text, Some(TextClassification::Sfx))
            .await
            .unwrap();
        let image_bytes = Arc::<[u8]>::from([1_u8, 2, 3].as_slice());
        let (_, image) = project
            .add_image_layer(
                page,
                crate::commands::import::Page {
                    name: "alternate.png".to_owned(),
                    bytes: image_bytes.clone(),
                    format: ImageFormat::Png,
                    width: 100,
                    height: 200,
                },
            )
            .await
            .unwrap();

        project.copy_layers(vec![text, image]).await.unwrap();
        assert!(project.info().can_paste);
        let (commit, pasted) = project.paste_layers(page).await.unwrap();
        project.record_commit(&commit);
        assert_eq!(pasted.len(), 2);

        let snapshot = project.snapshot();
        let pasted_text = pasted
            .iter()
            .copied()
            .find(|layer| {
                snapshot
                    .component::<SceneTextLayout>(*layer)
                    .unwrap()
                    .is_some()
            })
            .unwrap();
        let pasted_image = pasted
            .iter()
            .copied()
            .find(|layer| *layer != pasted_text)
            .unwrap();
        let content = snapshot.text_layer(pasted_text).unwrap().content().unwrap();
        assert_eq!(
            content.source().unwrap().unwrap().text.value,
            "boom".to_owned()
        );
        assert_eq!(
            content.translation().unwrap().unwrap().text.value,
            "kaboom".to_owned()
        );
        assert_eq!(content.role().unwrap().unwrap().role, SceneTextRole::SFX);
        assert!(
            snapshot
                .component::<SceneOcrCandidate>(content.id())
                .unwrap()
                .is_some()
        );
        let text_parent = snapshot.parent(pasted_text).unwrap().unwrap();
        assert_eq!(
            snapshot
                .component::<SceneTextGroup>(text_parent)
                .unwrap()
                .unwrap()
                .kind,
            SceneTextGroupKind::Sfx
        );
        assert_eq!(
            snapshot
                .component::<SceneGeometry>(pasted_text)
                .unwrap()
                .unwrap()
                .points[0],
            ScenePoint { x: 30.0, y: 40.0 }
        );
        assert_eq!(
            snapshot
                .component::<SceneGeometry>(pasted_image)
                .unwrap()
                .unwrap()
                .points[0],
            ScenePoint { x: 10.0, y: 10.0 }
        );
        let source = AssetRole::new("source").unwrap();
        let asset = snapshot.asset(pasted_image, &source).unwrap().unwrap();
        assert_eq!(
            snapshot.read_blob(asset.blob).await.unwrap().as_ref(),
            image_bytes.as_ref()
        );

        project.undo().await.unwrap();
        let snapshot = project.snapshot();
        assert!(pasted.iter().all(|layer| snapshot.entity(*layer).is_err()));
    }

    #[tokio::test]
    async fn original_and_imported_image_erasing_are_undoable() {
        let mut session = Session::memory().await.unwrap();
        let mut page = None;
        let patch = session
            .snapshot()
            .patch(|edit| {
                page = Some(edit.add_page(PageDraft::new("page", 100.0, 200.0), At::End)?);
                Ok(())
            })
            .unwrap();
        session.commit(patch).await.unwrap();
        let page = page.unwrap();
        let mut project = Project::new(session, "test".to_owned());

        let image = RgbaImage::from_pixel(10, 20, image::Rgba([40, 80, 120, 255]));
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let image_bytes = Arc::<[u8]>::from(encoded.into_inner());
        let source = AssetRole::new("source").unwrap();
        let patch = project
            .snapshot()
            .patch(|edit| {
                edit.set_asset(
                    page,
                    &source,
                    AssetInput::new(
                        image_bytes.clone(),
                        "image/png",
                        AssetMetadata {
                            width: Some(10),
                            height: Some(20),
                            attributes: Default::default(),
                        },
                    ),
                )
            })
            .unwrap();
        project.commit(patch).await.unwrap();
        let original_page_blob = project
            .snapshot()
            .asset(page, &source)
            .unwrap()
            .unwrap()
            .blob;
        let (_, layer) = project
            .add_image_layer(
                page,
                crate::commands::import::Page {
                    name: "alternate.png".to_owned(),
                    bytes: image_bytes,
                    format: ImageFormat::Png,
                    width: 10,
                    height: 20,
                },
            )
            .await
            .unwrap();
        project
            .set_geometries([(layer, SceneGeometry::rectangle(20.0, 30.0, 50.0, 100.0))])
            .await
            .unwrap();
        let original_blob = project
            .snapshot()
            .asset(layer, &source)
            .unwrap()
            .unwrap()
            .blob;

        let (page_commit, committed_page) = project
            .apply_raster_stroke(
                page,
                Some(page),
                RasterStrokeMode::Erase,
                RasterLayerKind::Paint,
                [0; 4],
                20.0,
                vec![ScenePoint { x: 55.0, y: 105.0 }],
            )
            .await
            .unwrap();
        assert_eq!(committed_page, page);
        project.record_commit(&page_commit);
        let erased_page = project.snapshot();
        let erased_page_asset = erased_page.asset(page, &source).unwrap().unwrap();
        assert_ne!(erased_page_asset.blob, original_page_blob);
        let bytes = erased_page.read_blob(erased_page_asset.blob).await.unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(image.get_pixel(5, 10)[3], 0);
        project.undo().await.unwrap();
        assert_eq!(
            project
                .snapshot()
                .asset(page, &source)
                .unwrap()
                .unwrap()
                .blob,
            original_page_blob
        );

        let (commit, committed_layer) = project
            .apply_raster_stroke(
                page,
                Some(layer),
                RasterStrokeMode::Erase,
                RasterLayerKind::Paint,
                [0; 4],
                10.0,
                vec![ScenePoint { x: 47.5, y: 82.5 }],
            )
            .await
            .unwrap();
        assert_eq!(committed_layer, layer);
        project.record_commit(&commit);

        let erased = project.snapshot();
        let erased_asset = erased.asset(layer, &source).unwrap().unwrap();
        assert_eq!(erased_asset.metadata.width, Some(10));
        assert_eq!(erased_asset.metadata.height, Some(20));
        assert_ne!(erased_asset.blob, original_blob);
        let erased_blob = erased_asset.blob;
        let bytes = erased.read_blob(erased_blob).await.unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(image.get_pixel(5, 10)[3], 0);
        assert!(project.info().can_undo);

        project.undo().await.unwrap();
        assert_eq!(
            project
                .snapshot()
                .asset(layer, &source)
                .unwrap()
                .unwrap()
                .blob,
            original_blob
        );
        assert!(project.info().can_redo);

        project.redo().await.unwrap();
        assert_eq!(
            project
                .snapshot()
                .asset(layer, &source)
                .unwrap()
                .unwrap()
                .blob,
            erased_blob
        );
    }

    #[tokio::test]
    async fn segmentation_erasing_is_undoable_without_changing_detector_ownership() {
        let mut session = Session::memory().await.unwrap();
        let mut page = None;
        let create = session
            .snapshot()
            .patch(|edit| {
                let id = edit.add_page(PageDraft::new("page", 10.0, 10.0), At::End)?;
                let sibling = edit.add_entity(id, At::End)?;
                edit.set(
                    sibling,
                    &SceneRasterLayer {
                        origin: Origin::User,
                        name: "Paint".to_owned(),
                        kind: RasterLayerKind::Paint,
                    },
                )?;
                page = Some(id);
                Ok(())
            })
            .unwrap();
        let snapshot = session.commit(create).await.unwrap().snapshot;
        let page = page.unwrap();
        let detector = Generation::new(
            ProducerId::new("dev.koharu.pipeline.detection").expect("valid producer"),
        );
        let image = RgbaImage::from_pixel(10, 10, image::Rgba([168, 85, 247, 255]));
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let mut edit = snapshot.edit_as(detector.clone());
        let layer = edit.add_entity(page, At::Start).unwrap();
        edit.set(
            layer,
            &SceneRasterLayer {
                origin: Origin::User,
                name: "Text Segmentation".to_owned(),
                kind: RasterLayerKind::Segmentation,
            },
        )
        .unwrap();
        edit.set_asset(
            layer,
            &AssetRole::new("source").unwrap(),
            AssetInput::new(
                Arc::<[u8]>::from(encoded.into_inner()),
                "image/png",
                AssetMetadata {
                    width: Some(10),
                    height: Some(10),
                    attributes: Default::default(),
                },
            ),
        )
        .unwrap();
        session.commit(edit.finish().unwrap()).await.unwrap();
        let segmentation = layer;
        let mut project = Project::new(session, "test".to_owned());
        let source = AssetRole::new("source").unwrap();
        let original_blob = project
            .snapshot()
            .asset(segmentation, &source)
            .unwrap()
            .unwrap()
            .blob;

        project
            .set_layer_locked(vec![segmentation], true)
            .await
            .unwrap();
        project
            .set_layer_locked(vec![segmentation], false)
            .await
            .unwrap();
        project.move_layer(segmentation, page, 1).await.unwrap();
        assert_eq!(
            project
                .snapshot()
                .component::<EntityOrigin>(segmentation)
                .unwrap()
                .unwrap()
                .origin,
            Origin::Generated(detector.clone())
        );

        let (commit, committed_layer) = project
            .apply_raster_stroke(
                page,
                Some(segmentation),
                RasterStrokeMode::Erase,
                RasterLayerKind::Paint,
                [0; 4],
                4.0,
                vec![ScenePoint { x: 5.0, y: 5.0 }],
            )
            .await
            .unwrap();
        project.record_commit(&commit);

        assert_eq!(committed_layer, segmentation);
        let erased = project.snapshot();
        let erased_asset = erased.asset(segmentation, &source).unwrap().unwrap();
        let erased_image =
            image::load_from_memory(&erased.read_blob(erased_asset.blob).await.unwrap())
                .unwrap()
                .to_rgba8();
        assert_eq!(erased_image.get_pixel(5, 5)[3], 0);
        assert_eq!(
            erased
                .component::<EntityOrigin>(segmentation)
                .unwrap()
                .unwrap()
                .origin,
            Origin::Generated(detector.clone())
        );

        project.undo().await.unwrap();
        let restored = project.snapshot();
        assert_eq!(
            restored.asset(segmentation, &source).unwrap().unwrap().blob,
            original_blob
        );
        assert_eq!(
            restored
                .component::<EntityOrigin>(segmentation)
                .unwrap()
                .unwrap()
                .origin,
            Origin::Generated(detector)
        );
    }

    #[tokio::test]
    async fn color_fill_is_contiguous_tolerant_and_undoable() {
        let mut session = Session::memory().await.unwrap();
        let mut page = None;
        let patch = session
            .snapshot()
            .patch(|edit| {
                page = Some(edit.add_page(PageDraft::new("page", 40.0, 20.0), At::End)?);
                Ok(())
            })
            .unwrap();
        session.commit(patch).await.unwrap();
        let page = page.unwrap();
        let mut project = Project::new(session, "test".to_owned());
        let image = RgbaImage::from_fn(4, 2, |x, y| {
            let value = match (x, y) {
                (0, _) => 10,
                (1, 0) => 14,
                (1, 1) => 80,
                _ => 120,
            };
            image::Rgba([value, value, value, 255])
        });
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let source = AssetRole::new("source").unwrap();
        let patch = project
            .snapshot()
            .patch(|edit| {
                edit.set_asset(
                    page,
                    &source,
                    AssetInput::new(
                        encoded.into_inner(),
                        "image/png",
                        AssetMetadata {
                            width: Some(4),
                            height: Some(2),
                            attributes: Default::default(),
                        },
                    ),
                )
            })
            .unwrap();
        project.commit(patch).await.unwrap();
        let original_blob = project
            .snapshot()
            .asset(page, &source)
            .unwrap()
            .unwrap()
            .blob;

        let commit = project
            .apply_raster_fill(
                page,
                page,
                ScenePoint { x: 5.0, y: 5.0 },
                [255, 0, 0, 255],
                5,
            )
            .await
            .unwrap()
            .unwrap();
        project.record_commit(&commit);
        let filled = project.snapshot();
        let asset = filled.asset(page, &source).unwrap().unwrap();
        assert_ne!(asset.blob, original_blob);
        let bytes = filled.read_blob(asset.blob).await.unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(0, 1).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(1, 0).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(1, 1).0, [80, 80, 80, 255]);
        assert_eq!(image.get_pixel(2, 0).0, [120, 120, 120, 255]);

        project.undo().await.unwrap();
        assert_eq!(
            project
                .snapshot()
                .asset(page, &source)
                .unwrap()
                .unwrap()
                .blob,
            original_blob
        );
    }

    #[tokio::test]
    async fn pipeline_commit_rebases_or_yields_to_the_manual_edit() {
        let mut session = Session::memory().await.unwrap();
        let mut setup = session.snapshot().edit();
        let pipeline_page = setup
            .add_page(PageDraft::new("pipeline", 100.0, 100.0), At::End)
            .unwrap();
        let manual_page = setup
            .add_page(PageDraft::new("manual", 100.0, 100.0), At::End)
            .unwrap();
        session.commit(setup.finish().unwrap()).await.unwrap();
        let mut project = Project::new(session, "test".to_owned());

        let base = project.snapshot();
        let pipeline = base
            .patch(|edit| {
                edit.set_page(
                    pipeline_page,
                    PageDraft::new("pipeline result", 100.0, 100.0),
                )
            })
            .unwrap();
        let manual = base
            .patch(|edit| edit.set_page(manual_page, PageDraft::new("manual edit", 100.0, 100.0)))
            .unwrap();
        project.commit(manual).await.unwrap();

        let commit = project.commit_rebased(pipeline).await.unwrap().unwrap();
        assert_eq!(
            commit
                .snapshot
                .page(pipeline_page)
                .unwrap()
                .page()
                .unwrap()
                .label,
            "pipeline result"
        );
        assert_eq!(
            commit
                .snapshot
                .page(manual_page)
                .unwrap()
                .page()
                .unwrap()
                .label,
            "manual edit"
        );

        let base = project.snapshot();
        let pipeline = base
            .patch(|edit| {
                edit.set_page(
                    pipeline_page,
                    PageDraft::new("stale pipeline", 100.0, 100.0),
                )
            })
            .unwrap();
        let manual = base
            .patch(|edit| {
                edit.set_page(pipeline_page, PageDraft::new("latest manual", 100.0, 100.0))
            })
            .unwrap();
        project.commit(manual).await.unwrap();

        assert!(project.commit_rebased(pipeline).await.unwrap().is_none());
        assert_eq!(
            project
                .snapshot()
                .page(pipeline_page)
                .unwrap()
                .page()
                .unwrap()
                .label,
            "latest manual"
        );
    }

    #[test]
    fn raster_strokes_are_continuous_and_erasable() {
        let mut image = RgbaImage::new(32, 16);
        let points = [
            ScenePoint { x: 4.0, y: 8.0 },
            ScenePoint { x: 28.0, y: 8.0 },
        ];
        rasterize_stroke(
            &mut image,
            RasterStrokeMode::Paint,
            [210, 40, 20, 255],
            5.0,
            &points,
        );
        for x in 4..28 {
            assert_eq!(image.get_pixel(x, 8).0, [210, 40, 20, 255]);
        }

        rasterize_stroke(
            &mut image,
            RasterStrokeMode::Erase,
            [0, 0, 0, 0],
            5.0,
            &[ScenePoint { x: 16.0, y: 8.0 }],
        );
        assert_eq!(image.get_pixel(16, 8)[3], 0);
        assert_eq!(image.get_pixel(4, 8)[3], 255);

        let mut white = RgbaImage::new(4, 4);
        rasterize_stroke(
            &mut white,
            RasterStrokeMode::Paint,
            [255, 255, 255, 255],
            3.0,
            &[ScenePoint { x: 2.0, y: 2.0 }],
        );
        assert_eq!(white.get_pixel(2, 2).0, [255, 255, 255, 255]);
    }
}
