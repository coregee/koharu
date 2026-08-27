mod decensor;
mod detection;
mod inpainting;
mod ocr;
mod translation;

use std::{collections::BTreeSet, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use koharu_scene::{
    AssetRole, At, Edit, EntityId, EntityOrigin, Generation, Origin, Patch, ProducerId,
    RasterLayer, RasterLayerKind, Snapshot,
};

pub use detection::KoharuLayoutRFDetrSeg2XLConfig;
pub use inpainting::{Flux2KleinConfig, RoremMixedConfig};

use crate::{Bounds, ImageCache, InpaintingMask, PipelineConfig, Stage};

fn above_artwork(snapshot: &Snapshot, page: EntityId) -> Result<At> {
    Ok(snapshot
        .children(page)?
        .find(|entity| {
            snapshot
                .component::<RasterLayer>(*entity)
                .ok()
                .flatten()
                .is_some_and(|layer| layer.kind == RasterLayerKind::Artwork)
        })
        .map_or(At::Start, At::After))
}

fn above_cleanup(snapshot: &Snapshot, page: EntityId) -> Result<At> {
    let mut cleanup = None;
    for entity in snapshot.children(page)? {
        if snapshot
            .component::<RasterLayer>(entity)?
            .is_some_and(|layer| layer.kind == RasterLayerKind::Cleanup)
        {
            cleanup = Some(entity);
        }
    }
    match cleanup {
        Some(entity) => Ok(At::After(entity)),
        None => above_artwork(snapshot, page),
    }
}

#[derive(Clone, Copy)]
enum CleanupOwner<'a> {
    User,
    Generated(&'a str),
}

impl CleanupOwner<'_> {
    fn matches(self, origin: &Origin) -> bool {
        match (self, origin) {
            (Self::User, Origin::User) => true,
            (Self::Generated(producer), Origin::Generated(generation)) => {
                generation.producer.as_str() == producer
            }
            (Self::User | Self::Generated(_), _) => false,
        }
    }
}

fn cleanup_target(
    snapshot: &Snapshot,
    page: EntityId,
    owner: CleanupOwner<'_>,
) -> Result<Option<EntityId>> {
    let source = AssetRole::new("source")?;
    let mut target = None;
    for entity in snapshot.children(page)? {
        let Some(lifecycle) = snapshot.component::<EntityOrigin>(entity)? else {
            continue;
        };
        let Some(layer) = snapshot.component::<RasterLayer>(entity)? else {
            continue;
        };
        if layer.kind != RasterLayerKind::Cleanup
            || !owner.matches(&lifecycle.origin)
            || !owner.matches(&layer.origin)
        {
            continue;
        }
        let Some(asset) = snapshot.asset(entity, &source)? else {
            continue;
        };
        if owner.matches(&asset.origin) {
            target = Some(entity);
        }
    }
    Ok(target)
}

#[derive(Clone)]
pub(crate) struct StageInput {
    scene: koharu_scene::Snapshot,
    page: EntityId,
    entities: Option<Arc<BTreeSet<EntityId>>>,
    region: Option<Bounds>,
    images: Arc<ImageCache>,
    inpainting_mask: Option<InpaintingMask>,
}

impl StageInput {
    pub(crate) fn new(
        scene: Snapshot,
        page: EntityId,
        entities: Option<Arc<BTreeSet<EntityId>>>,
        region: Option<Bounds>,
        images: Arc<ImageCache>,
        inpainting_mask: Option<InpaintingMask>,
    ) -> Self {
        Self {
            scene,
            page,
            entities,
            region,
            images,
            inpainting_mask,
        }
    }

    pub(crate) fn page(&self) -> EntityId {
        self.page
    }

    fn contains_entity(&self, entity: EntityId) -> Result<bool> {
        crate::scope::contains_entity(
            &self.scene,
            self.page,
            self.entities.as_deref(),
            self.region,
            entity,
        )
    }

    fn has_entity_scope(&self) -> bool {
        self.entities.is_some()
    }
}

#[async_trait]
trait StageProcessor: Send + Sync {
    fn model(&self) -> &'static str;
    fn skip(&self, _input: &StageInput) -> Result<bool> {
        Ok(false)
    }
    fn unload(&self) -> bool;
    async fn load(&self) -> Result<()>;
    async fn process(&self, input: StageInput) -> Result<Patch>;
}

pub(crate) struct Stages {
    detection: detection::Processor,
    ocr: ocr::Processor,
    translation: translation::Processor,
    inpainting: inpainting::Processor,
    decensor: decensor::Processor,
}

impl Stages {
    pub(crate) fn new(
        config: &PipelineConfig,
        translator: koharu_translator::Translator,
        device: &koharu_ml::Device,
    ) -> Result<Self> {
        Ok(Self {
            detection: detection::Processor::new(config.detection()?, device.clone()),
            ocr: ocr::Processor::new(config.ocr.clone(), device.clone()),
            translation: translation::Processor::new(config.translation.clone(), translator),
            inpainting: inpainting::Processor::new(config.inpainting()?, device.clone())?,
            decensor: decensor::Processor::new(device.clone()),
        })
    }

    fn processor(&self, stage: Stage) -> &dyn StageProcessor {
        match stage {
            Stage::Detection => &self.detection,
            Stage::Ocr => &self.ocr,
            Stage::Translation => &self.translation,
            Stage::Inpainting => &self.inpainting,
            Stage::Decensor => &self.decensor,
        }
    }

    pub(crate) fn model(&self, stage: Stage) -> &'static str {
        self.processor(stage).model()
    }

    pub(crate) fn skip(&self, stage: Stage, input: &StageInput) -> Result<bool> {
        self.processor(stage).skip(input)
    }

    pub(crate) async fn load(&self, stage: Stage) -> Result<()> {
        self.processor(stage).load().await
    }

    pub(crate) async fn process(&self, stage: Stage, input: StageInput) -> Result<Patch> {
        self.processor(stage).process(input).await
    }

    pub(crate) fn unload(&self, stage: Stage) -> bool {
        self.processor(stage).unload()
    }
}

fn generation(producer: &str, model: &str) -> Result<Generation> {
    let mut generation = Generation::new(ProducerId::new(producer)?);
    generation.model = Some(model.to_owned());
    Ok(generation)
}

fn finish(edit: Edit) -> Result<Patch> {
    edit.finish().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(bytes: &'static [u8]) -> koharu_scene::AssetInput {
        koharu_scene::AssetInput::new(
            bytes,
            "image/png",
            koharu_scene::AssetMetadata {
                width: Some(1),
                height: Some(1),
                attributes: std::collections::BTreeMap::new(),
            },
        )
    }

    #[tokio::test]
    async fn registry_contains_every_stage() {
        let translator = koharu_translator::Translator::from_config(
            koharu_ml::Device::cpu(),
            koharu_config::Config::memory(koharu_translator::ProvidersConfig::default()),
        )
        .unwrap();
        let stages = Stages::new(
            &PipelineConfig::default(),
            translator,
            &koharu_ml::Device::cpu(),
        )
        .unwrap();

        assert_eq!(
            Stage::ALL.map(|stage| stages.model(stage)),
            [
                "koharu-layout-rfdetr-seg-2xl",
                "paddleocr-vl-1.6",
                "local",
                "lama",
                "diffusioncreampy-sdxl",
            ]
        );
    }

    #[tokio::test]
    async fn generated_rasters_are_inserted_above_editable_artwork() {
        let mut session = koharu_scene::Session::memory().await.unwrap();
        let mut page = None;
        let mut artwork = None;
        let patch = session
            .snapshot()
            .patch(|edit| {
                let page_id =
                    edit.add_page(koharu_scene::PageDraft::new("page", 1.0, 1.0), At::End)?;
                let artwork_id = edit.add_entity(page_id, At::Start)?;
                edit.set(
                    artwork_id,
                    &RasterLayer {
                        origin: koharu_scene::Origin::User,
                        name: "Artwork".to_owned(),
                        kind: RasterLayerKind::Artwork,
                    },
                )?;
                page = Some(page_id);
                artwork = Some(artwork_id);
                Ok(())
            })
            .unwrap();
        let snapshot = session.commit(patch).await.unwrap().snapshot;
        let page = page.unwrap();
        let artwork = artwork.unwrap();
        let mut edit = snapshot.edit();
        let cleanup = edit
            .add_entity(page, above_artwork(&snapshot, page).unwrap())
            .unwrap();
        let snapshot = snapshot.preview([&edit.finish().unwrap()]).unwrap();

        assert_eq!(
            snapshot.children(page).unwrap().collect::<Vec<_>>(),
            [artwork, cleanup]
        );
    }

    #[tokio::test]
    async fn translation_and_inpainting_compose_without_weakening_text_guards() {
        let mut session = koharu_scene::Session::memory().await.unwrap();
        let mut setup = session.snapshot().edit();
        let page = setup
            .add_page(
                koharu_scene::PageDraft::new("page", 1.0, 1.0),
                koharu_scene::At::End,
            )
            .unwrap();
        let text = setup.add_text_content(page, koharu_scene::At::End).unwrap();
        setup
            .set(
                text,
                &koharu_scene::SourceText {
                    text: koharu_scene::Authored::user("before".to_owned()),
                    language: None,
                },
            )
            .unwrap();
        setup
            .set_asset(
                page,
                &koharu_scene::AssetRole::new("source").unwrap(),
                asset(b"source"),
            )
            .unwrap();
        session.commit(setup.finish().unwrap()).await.unwrap();
        let base = session.snapshot();

        let mut text_edit = base.edit();
        text_edit.observe::<koharu_scene::SourceText>(text).unwrap();
        text_edit
            .observe::<koharu_scene::Translation>(text)
            .unwrap();
        text_edit
            .set(
                text,
                &koharu_scene::Translation {
                    text: koharu_scene::Authored::user("after".to_owned()),
                    language: None,
                },
            )
            .unwrap();
        let text_patch = text_edit.finish().unwrap();

        let mut image_edit = base.edit();
        image_edit.observe_assets(page).unwrap();
        let cleanup = image_edit
            .add_entity(page, koharu_scene::At::Start)
            .unwrap();
        image_edit
            .set(
                cleanup,
                &koharu_scene::RasterLayer {
                    origin: koharu_scene::Origin::User,
                    name: "Cleanup".to_owned(),
                    kind: koharu_scene::RasterLayerKind::Cleanup,
                },
            )
            .unwrap();
        image_edit
            .set_asset(
                cleanup,
                &koharu_scene::AssetRole::new("source").unwrap(),
                asset(b"clean"),
            )
            .unwrap();
        let image_patch = image_edit.finish().unwrap();

        let image_first = base.preview([&image_patch]).unwrap();
        assert!(text_patch.rebase_on(&image_first).is_ok());
        let text_first = base.preview([&text_patch]).unwrap();
        assert!(image_patch.rebase_on(&text_first).is_ok());

        let changed_source = base
            .patch(|edit| {
                edit.set(
                    text,
                    &koharu_scene::SourceText {
                        text: koharu_scene::Authored::user("changed".to_owned()),
                        language: None,
                    },
                )
            })
            .unwrap();
        let changed_source = base.preview([&changed_source]).unwrap();
        assert!(text_patch.rebase_on(&changed_source).is_err());
    }
}
