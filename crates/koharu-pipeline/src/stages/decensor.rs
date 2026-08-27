use std::{
    collections::BTreeMap,
    io::Cursor,
    sync::{Arc, Mutex},
};

use anyhow::{Context as _, Result, anyhow, bail};
use async_trait::async_trait;
use image::{DynamicImage, GenericImageView as _, GrayImage, ImageFormat, Luma, Rgba, RgbaImage};
use imageproc::{
    distance_transform::Norm,
    morphology::dilate,
    region_labelling::{Connectivity, connected_components},
};
use koharu_ml::decensor::Decensor;
#[cfg(test)]
use koharu_scene::At;
use koharu_scene::{
    AssetInput, AssetMetadata, AssetRole, Origin, RasterLayer, RasterLayerKind, Visibility,
};

use super::{CleanupOwner, StageInput, StageProcessor, above_cleanup, cleanup_target, finish};
use crate::ModelCell;

const MODEL: &str = "diffusioncreampy-sdxl";
const CROP_PADDING: u32 = 128;
const BAR_MASK_EXPANSION: u8 = 2;
const MOSAIC_MASK_EXPANSION: u8 = 8;

pub(super) struct Processor {
    device: koharu_ml::Device,
    model: ModelCell<Arc<Mutex<Decensor>>>,
}

impl Processor {
    pub(super) fn new(device: koharu_ml::Device) -> Self {
        Self {
            device,
            model: ModelCell::new(),
        }
    }
}

#[async_trait]
impl StageProcessor for Processor {
    fn model(&self) -> &'static str {
        MODEL
    }

    fn skip(&self, input: &StageInput) -> Result<bool> {
        for entity in input.scene.children(input.page)? {
            if input.contains_entity(entity)?
                && input
                    .scene
                    .component::<RasterLayer>(entity)?
                    .is_some_and(|layer| {
                        matches!(
                            layer.kind,
                            RasterLayerKind::Censor | RasterLayerKind::Mosaic
                        )
                    })
                && input
                    .scene
                    .asset(entity, &AssetRole::new("source")?)?
                    .is_some()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn unload(&self) -> bool {
        self.model.unload()
    }

    async fn load(&self) -> Result<()> {
        self.model
            .ensure(|| async {
                Ok(Arc::new(Mutex::new(
                    Decensor::load(self.device.clone()).await?,
                )))
            })
            .await
    }

    async fn process(&self, input: StageInput) -> Result<koharu_scene::Patch> {
        let model = self
            .model
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow!("decensor model is not loaded"))?;
        let prepared = prepare(&input).await?;
        if mask_is_empty(&prepared.bar_mask) && mask_is_empty(&prepared.mosaic_mask) {
            return finish(input.scene.edit());
        }
        let image = prepared.image.clone();
        let bar_mask = prepared.bar_mask.clone();
        let mosaic_mask = prepared.mosaic_mask.clone();
        let generated = tokio::task::spawn_blocking(move || {
            let mut output = image.to_rgb8();
            if !mask_is_empty(&bar_mask) {
                output = decensor_regions(
                    &model,
                    &DynamicImage::ImageRgb8(output),
                    &bar_mask,
                    DecensorPass::Bar,
                )?;
            }
            if !mask_is_empty(&mosaic_mask) {
                let expanded = dilate(&mosaic_mask, Norm::LInf, MOSAIC_MASK_EXPANSION);
                output = decensor_regions(
                    &model,
                    &DynamicImage::ImageRgb8(output),
                    &expanded,
                    DecensorPass::Mosaic,
                )?;
            }
            Ok::<_, anyhow::Error>(output)
        })
        .await
        .context("decensor task panicked")??;

        write_result(&input.scene, input.page, prepared, generated)
    }
}

fn write_result(
    scene: &koharu_scene::Snapshot,
    page: koharu_scene::EntityId,
    prepared: Prepared,
    generated: image::RgbImage,
) -> Result<koharu_scene::Patch> {
    let mut cleanup = prepared
        .cleanup
        .unwrap_or_else(|| RgbaImage::new(generated.width(), generated.height()));
    let bar_blend =
        DynamicImage::ImageLuma8(dilate(&prepared.bar_mask, Norm::LInf, BAR_MASK_EXPANSION))
            .blur(4.0)
            .to_luma8();
    let mosaic_blend = DynamicImage::ImageLuma8(dilate(
        &prepared.mosaic_mask,
        Norm::LInf,
        MOSAIC_MASK_EXPANSION,
    ))
    .blur(16.0)
    .to_luma8();
    let blend_mask = merge_masks(&bar_blend, &mosaic_blend);
    for (x, y, target) in cleanup.enumerate_pixels_mut() {
        let alpha = blend_mask.get_pixel(x, y)[0];
        if alpha == 0 {
            continue;
        }
        let pixel = generated.get_pixel(x, y);
        *target = Rgba([pixel[0], pixel[1], pixel[2], alpha]);
    }
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(cleanup).write_to(&mut bytes, ImageFormat::Png)?;

    // A decensor mask is an explicit user-authored delegation. Its result,
    // the cleanup pixels, and hiding the consumed mask therefore remain
    // user edits rather than claiming ownership for the pipeline producer.
    let mut edit = scene.edit();
    edit.observe_assets(page)?;
    let cleanup_entity = if let Some(entity) = prepared.cleanup_entity {
        edit.observe::<RasterLayer>(entity)?;
        edit.observe_assets(entity)?;
        entity
    } else {
        let entity = edit.add_entity(page, above_cleanup(scene, page)?)?;
        edit.set(
            entity,
            &RasterLayer {
                origin: Origin::User,
                name: "Cleanup".to_owned(),
                kind: RasterLayerKind::Cleanup,
            },
        )?;
        entity
    };
    edit.set_asset(
        cleanup_entity,
        &AssetRole::new("source")?,
        AssetInput::new(
            bytes.into_inner(),
            "image/png",
            AssetMetadata {
                width: Some(generated.width()),
                height: Some(generated.height()),
                attributes: prepared.cleanup_attributes,
            },
        ),
    )?;
    for entity in prepared.mask_entities {
        edit.observe::<Visibility>(entity)?;
        edit.set(
            entity,
            &Visibility {
                origin: Origin::User,
                visible: false,
                opacity: 1.0,
            },
        )?;
    }
    finish(edit)
}

struct Prepared {
    image: DynamicImage,
    bar_mask: GrayImage,
    mosaic_mask: GrayImage,
    cleanup: Option<RgbaImage>,
    cleanup_entity: Option<koharu_scene::EntityId>,
    cleanup_attributes: BTreeMap<String, String>,
    mask_entities: Vec<koharu_scene::EntityId>,
}

async fn prepare(input: &StageInput) -> Result<Prepared> {
    let original = input
        .images
        .get(&input.scene, input.page, "source")
        .await?
        .ok_or_else(|| anyhow!("page {} has no source image", input.page))?;
    let mut composite = original.to_rgba8();
    let cleanup_entity = cleanup_target(&input.scene, input.page, CleanupOwner::User)?;
    let cleanup = if let Some(entity) = cleanup_entity {
        input
            .images
            .get(&input.scene, entity, "source")
            .await?
            .map(|image| image.to_rgba8())
    } else {
        None
    };
    let cleanup_attributes = if let Some(entity) = cleanup_entity {
        input
            .scene
            .asset(entity, &koharu_scene::AssetRole::new("source")?)?
            .map_or_else(BTreeMap::new, |asset| asset.metadata.attributes)
    } else {
        BTreeMap::new()
    };
    let mut mask_entities = Vec::new();
    let mut bar_mask = GrayImage::new(original.width(), original.height());
    let mut mosaic_mask = GrayImage::new(original.width(), original.height());
    for entity in input.scene.children(input.page)? {
        let Some(layer) = input.scene.component::<RasterLayer>(entity)? else {
            continue;
        };
        let Some(image) = input.images.get(&input.scene, entity, "source").await? else {
            continue;
        };
        if image.dimensions() != original.dimensions() {
            bail!("raster layer dimensions do not match page {}", input.page);
        }
        match layer.kind {
            RasterLayerKind::Artwork => {
                image::imageops::overlay(&mut composite, &image.to_rgba8(), 0, 0);
            }
            RasterLayerKind::Cleanup => {
                let image = image.to_rgba8();
                image::imageops::overlay(&mut composite, &image, 0, 0);
            }
            RasterLayerKind::Paint => {
                image::imageops::overlay(&mut composite, &image.to_rgba8(), 0, 0);
            }
            RasterLayerKind::Censor if input.contains_entity(entity)? => {
                for (target, source) in bar_mask.iter_mut().zip(image.to_rgba8().pixels()) {
                    *target = (*target).max(source[3]);
                }
                mask_entities.push(entity);
            }
            RasterLayerKind::Mosaic if input.contains_entity(entity)? => {
                for (target, source) in mosaic_mask.iter_mut().zip(image.to_rgba8().pixels()) {
                    *target = (*target).max(source[3]);
                }
                mask_entities.push(entity);
            }
            RasterLayerKind::Censor => {}
            RasterLayerKind::Mosaic => {}
            RasterLayerKind::Segmentation => {}
            RasterLayerKind::Repair => {}
        }
    }
    Ok(Prepared {
        image: DynamicImage::ImageRgba8(composite),
        bar_mask,
        mosaic_mask,
        cleanup,
        cleanup_entity,
        cleanup_attributes,
        mask_entities,
    })
}

fn decensor_regions(
    model: &Arc<Mutex<Decensor>>,
    image: &DynamicImage,
    mask: &GrayImage,
    pass: DecensorPass,
) -> Result<image::RgbImage> {
    let binary = GrayImage::from_fn(mask.width(), mask.height(), |x, y| {
        Luma([(mask.get_pixel(x, y)[0] >= 32) as u8 * u8::MAX])
    });
    let labels = connected_components(&binary, Connectivity::Eight, Luma([0]));
    let max_label = labels.pixels().map(|pixel| pixel[0]).max().unwrap_or(0);
    let mut output = image.to_rgb8();
    for label in 1..=max_label {
        let mut bounds = [mask.width(), mask.height(), 0, 0];
        for (x, y, pixel) in labels.enumerate_pixels() {
            if pixel[0] == label {
                bounds[0] = bounds[0].min(x);
                bounds[1] = bounds[1].min(y);
                bounds[2] = bounds[2].max(x + 1);
                bounds[3] = bounds[3].max(y + 1);
            }
        }
        if bounds[2] <= bounds[0] || bounds[3] <= bounds[1] {
            continue;
        }
        let left = bounds[0].saturating_sub(CROP_PADDING);
        let top = bounds[1].saturating_sub(CROP_PADDING);
        let right = bounds[2].saturating_add(CROP_PADDING).min(mask.width());
        let bottom = bounds[3].saturating_add(CROP_PADDING).min(mask.height());
        let crop =
            image::imageops::crop_imm(&output, left, top, right - left, bottom - top).to_image();
        let crop_mask =
            image::imageops::crop_imm(mask, left, top, right - left, bottom - top).to_image();
        let model = model
            .lock()
            .map_err(|_| anyhow!("decensor model lock was poisoned"))?;
        let generated = match pass {
            DecensorPass::Bar => {
                model.inference_bar(&DynamicImage::ImageRgb8(crop), &crop_mask, -1)?
            }
            DecensorPass::Mosaic => {
                model.inference_mosaic(&DynamicImage::ImageRgb8(crop), &crop_mask, -1)?
            }
        };
        let generated = DynamicImage::ImageRgb8(generated)
            .resize_exact(
                right - left,
                bottom - top,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgb8();
        for y in top..bottom {
            for x in left..right {
                if labels.get_pixel(x, y)[0] == label {
                    output.put_pixel(x, y, *generated.get_pixel(x - left, y - top));
                }
            }
        }
    }
    Ok(output)
}

#[derive(Clone, Copy)]
enum DecensorPass {
    Bar,
    Mosaic,
}

fn mask_is_empty(mask: &GrayImage) -> bool {
    mask.as_raw().iter().all(|value| *value == 0)
}

fn merge_masks(first: &GrayImage, second: &GrayImage) -> GrayImage {
    GrayImage::from_fn(first.width(), first.height(), |x, y| {
        Luma([first.get_pixel(x, y)[0].max(second.get_pixel(x, y)[0])])
    })
}

#[cfg(test)]
mod tests {
    use image::{Rgb, RgbImage};
    use koharu_scene::{PageDraft, Session};

    use super::*;

    #[tokio::test]
    async fn result_updates_user_cleanup_and_hides_consumed_decensor_masks() {
        let mut session = Session::memory().await.unwrap();
        let mut page = None;
        let mut cleanup = None;
        let mut censor = None;
        let mut mosaic = None;
        let patch = session
            .snapshot()
            .patch(|edit| {
                let page_id = edit.add_page(PageDraft::new("page", 2.0, 2.0), At::End)?;
                let cleanup_id = edit.add_entity(page_id, At::Start)?;
                edit.set(
                    cleanup_id,
                    &RasterLayer {
                        origin: Origin::User,
                        name: "Cleanup".to_owned(),
                        kind: RasterLayerKind::Cleanup,
                    },
                )?;
                edit.set_asset(
                    cleanup_id,
                    &AssetRole::new("source")?,
                    AssetInput::new(
                        [0],
                        "image/png",
                        AssetMetadata {
                            width: Some(2),
                            height: Some(2),
                            attributes: BTreeMap::new(),
                        },
                    ),
                )?;
                let censor_id = edit.add_entity(page_id, At::End)?;
                edit.set(
                    censor_id,
                    &RasterLayer {
                        origin: Origin::User,
                        name: "Censor".to_owned(),
                        kind: RasterLayerKind::Censor,
                    },
                )?;
                edit.set(
                    censor_id,
                    &Visibility {
                        origin: Origin::User,
                        visible: true,
                        opacity: 1.0,
                    },
                )?;
                page = Some(page_id);
                cleanup = Some(cleanup_id);
                censor = Some(censor_id);
                let mosaic_id = edit.add_entity(page_id, At::End)?;
                edit.set(
                    mosaic_id,
                    &RasterLayer {
                        origin: Origin::User,
                        name: "Mosaic".to_owned(),
                        kind: RasterLayerKind::Mosaic,
                    },
                )?;
                edit.set(
                    mosaic_id,
                    &Visibility {
                        origin: Origin::User,
                        visible: true,
                        opacity: 1.0,
                    },
                )?;
                mosaic = Some(mosaic_id);
                Ok(())
            })
            .unwrap();
        let snapshot = session.commit(patch).await.unwrap().snapshot;
        let prepared = Prepared {
            image: DynamicImage::new_rgba8(2, 2),
            bar_mask: GrayImage::from_pixel(2, 2, Luma([255])),
            mosaic_mask: GrayImage::new(2, 2),
            cleanup: Some(RgbaImage::new(2, 2)),
            cleanup_entity: cleanup,
            cleanup_attributes: BTreeMap::from([(
                "dev.koharu.test".to_owned(),
                "preserved".to_owned(),
            )]),
            mask_entities: vec![censor.unwrap(), mosaic.unwrap()],
        };

        let patch = write_result(
            &snapshot,
            page.unwrap(),
            prepared,
            RgbImage::from_pixel(2, 2, Rgb([12, 34, 56])),
        )
        .unwrap();
        let snapshot = session.commit(patch).await.unwrap().snapshot;

        assert_eq!(
            snapshot
                .asset(cleanup.unwrap(), &AssetRole::new("source").unwrap())
                .unwrap()
                .unwrap()
                .metadata
                .attributes
                .get("dev.koharu.test")
                .map(String::as_str),
            Some("preserved")
        );
        assert_eq!(
            snapshot
                .component::<Visibility>(censor.unwrap())
                .unwrap()
                .unwrap()
                .visible,
            false
        );
        assert_eq!(
            snapshot
                .component::<Visibility>(mosaic.unwrap())
                .unwrap()
                .unwrap()
                .visible,
            false
        );
    }
}
