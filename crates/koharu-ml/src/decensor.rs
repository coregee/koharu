//! DiffusionCreamPy-inspired bar and mosaic removal through stable-diffusion.cpp.
//!
//! Both passes use the workflow's model, 1024px crop, Euler/Beta sampler, ten
//! steps, and masked-image conditioning. Bar removal follows
//! `DiffusionCreamPy-Bar-CNet++.json` with an edge-filled input, CFG 5, and full
//! denoise. Mosaic removal follows `DiffusionCreamPy-Mosaic-CNet++.json` with a
//! source-preserving input, CFG 7, and 0.5 denoise. The mosaic workflow's tile
//! ControlNet cannot be loaded by Koharu's current stable-diffusion.cpp runtime,
//! whose ControlNet implementation is SD 1.5-only, so the mosaic pass uses its
//! demosaiced source as masked SDXL img2img conditioning instead.

use std::{collections::VecDeque, sync::Mutex};

use anyhow::{Context as _, Result, anyhow, ensure};
use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, GenericImageView as _, GrayImage, Luma, RgbImage};
use koharu_diffusion::{
    Context, ContextParams, GuidanceParams, ImageGenerationParams, SampleMethod, SampleParams,
    Scheduler,
};
use koharu_runtime::Feature;

use crate::Backend;

crate::model_repository!("cagliostrolab/animagine-xl-4.0" @ "2b7c1b397761bf5bd3cc42e5b39ec99314a75a96" {
    CHECKPOINT = "animagine-xl-4.0-opt.safetensors",
});
const RESOLUTION: u32 = 1024;

#[derive(Debug)]
pub struct Decensor {
    context: Mutex<Context>,
}

impl Decensor {
    pub async fn load(device: crate::Device) -> Result<Self> {
        let device = crate::device_for_feature(device, Feature::Diffusion);
        let checkpoint = CHECKPOINT
            .resolve()
            .await
            .context("failed to resolve DiffusionCreamPy model assets")?;
        let accelerated = device.backend != Backend::Cpu;
        let context = Context::new(&ContextParams {
            model_path: Some(checkpoint),
            enable_mmap: true,
            flash_attention: accelerated,
            diffusion_flash_attention: accelerated,
            backend: Some(if accelerated {
                device.name.to_ascii_lowercase()
            } else {
                "cpu".to_owned()
            }),
            ..ContextParams::default()
        })
        .context("failed to load DiffusionCreamPy decensor components")?;
        ensure!(
            context.supports_image_generation(),
            "DiffusionCreamPy context does not support image generation"
        );
        Ok(Self {
            context: Mutex::new(context),
        })
    }

    pub fn inference_bar(
        &self,
        image: &DynamicImage,
        mask: &GrayImage,
        seed: i64,
    ) -> Result<RgbImage> {
        self.inference(image, mask, seed, DecensorPass::Bar)
    }

    pub fn inference_mosaic(
        &self,
        image: &DynamicImage,
        mask: &GrayImage,
        seed: i64,
    ) -> Result<RgbImage> {
        self.inference(image, mask, seed, DecensorPass::Mosaic)
    }

    fn inference(
        &self,
        image: &DynamicImage,
        mask: &GrayImage,
        seed: i64,
        pass: DecensorPass,
    ) -> Result<RgbImage> {
        ensure!(
            image.dimensions() == mask.dimensions(),
            "decensor image and mask dimensions differ"
        );
        ensure!(
            image.width() > 0 && image.height() > 0,
            "decensor crop is empty"
        );

        let output_dimensions = image.dimensions();
        let image = resize_rgb(&image.to_rgb8(), RESOLUTION, RESOLUTION)?;
        let mask = resize_mask(mask, RESOLUTION, RESOLUTION)?;
        let init_image = match pass {
            DecensorPass::Bar => edge_fill(&image, &mask),
            DecensorPass::Mosaic => demosaic(&image),
        };
        let (text_cfg, strength) = match pass {
            DecensorPass::Bar => (5.0, 1.0),
            DecensorPass::Mosaic => (7.0, 0.5),
        };
        let mut context = self
            .context
            .lock()
            .map_err(|_| anyhow!("DiffusionCreamPy context lock was poisoned"))?;
        let generated = context
            .generate_image(&ImageGenerationParams {
                prompt: String::new(),
                negative_prompt: "censored".to_owned(),
                init_image: Some(init_image),
                mask_image: Some(mask),
                width: RESOLUTION as i32,
                height: RESOLUTION as i32,
                sample: SampleParams {
                    guidance: GuidanceParams {
                        text_cfg,
                        ..GuidanceParams::default()
                    },
                    scheduler: Scheduler::Beta,
                    sample_method: SampleMethod::Euler,
                    sample_steps: 10,
                    ..SampleParams::default()
                },
                strength,
                seed,
                batch_count: 1,
                ..ImageGenerationParams::default()
            })
            .context("DiffusionCreamPy decensor inference failed")?
            .into_iter()
            .next()
            .context("DiffusionCreamPy returned no image")?;
        resize_rgb(&generated, output_dimensions.0, output_dimensions.1)
    }
}

#[derive(Clone, Copy)]
enum DecensorPass {
    Bar,
    Mosaic,
}

fn resize_rgb(image: &RgbImage, width: u32, height: u32) -> Result<RgbImage> {
    if image.dimensions() == (width, height) {
        return Ok(image.clone());
    }
    let mut output = RgbImage::new(width, height);
    Resizer::new().resize(
        image,
        &mut output,
        &ResizeOptions::new()
            .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))
            .use_alpha(false),
    )?;
    Ok(output)
}

fn resize_mask(image: &GrayImage, width: u32, height: u32) -> Result<GrayImage> {
    let mut binary = image.clone();
    for pixel in binary.pixels_mut() {
        *pixel = Luma([if pixel[0] >= 32 { 255 } else { 0 }]);
    }
    if binary.dimensions() == (width, height) {
        return Ok(binary);
    }
    let mut output = GrayImage::new(width, height);
    Resizer::new().resize(
        &binary,
        &mut output,
        &ResizeOptions::new()
            .resize_alg(ResizeAlg::Nearest)
            .use_alpha(false),
    )?;
    Ok(output)
}

fn edge_fill(image: &RgbImage, mask: &GrayImage) -> RgbImage {
    let mut output = image.clone();
    let mut resolved = vec![false; mask.as_raw().len()];
    let mut queue = VecDeque::new();
    for y in 0..output.height() {
        for x in 0..output.width() {
            let index = (y * output.width() + x) as usize;
            if mask.as_raw()[index] < 128 {
                resolved[index] = true;
                queue.push_back((x, y));
            }
        }
    }
    while let Some((x, y)) = queue.pop_front() {
        let color = *output.get_pixel(x, y);
        let neighbours = [
            x.checked_sub(1).map(|x| (x, y)),
            (x + 1 < output.width()).then_some((x + 1, y)),
            y.checked_sub(1).map(|y| (x, y)),
            (y + 1 < output.height()).then_some((x, y + 1)),
        ];
        for (nx, ny) in neighbours.into_iter().flatten() {
            let index = (ny * output.width() + nx) as usize;
            if !resolved[index] {
                resolved[index] = true;
                output.put_pixel(nx, ny, color);
                queue.push_back((nx, ny));
            }
        }
    }
    output
}

fn demosaic(image: &RgbImage) -> RgbImage {
    DynamicImage::ImageRgb8(image.clone()).blur(4.0).to_rgb8()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_fill_replaces_masked_pixels_from_the_surrounding_image() {
        let image = RgbImage::from_fn(3, 3, |x, y| image::Rgb([(x + y * 3) as u8, 0, 0]));
        let mut mask = GrayImage::new(3, 3);
        mask.put_pixel(1, 1, Luma([255]));

        let filled = edge_fill(&image, &mask);

        assert_eq!(filled.get_pixel(0, 0), image.get_pixel(0, 0));
        assert_eq!(filled.get_pixel(1, 1), image.get_pixel(1, 0));
        assert_ne!(filled.get_pixel(1, 1), image.get_pixel(1, 1));
    }

    #[test]
    fn demosaic_preserves_dimensions_and_smooths_pixel_boundaries() {
        let image = RgbImage::from_fn(9, 9, |x, _| {
            image::Rgb(if x < 4 { [0, 0, 0] } else { [255, 255, 255] })
        });

        let blurred = demosaic(&image);

        assert_eq!(blurred.dimensions(), image.dimensions());
        assert!(blurred.get_pixel(4, 4)[0] > 0);
        assert!(blurred.get_pixel(4, 4)[0] < 255);
    }
}
