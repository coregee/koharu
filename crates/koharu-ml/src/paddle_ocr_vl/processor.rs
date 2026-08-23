//! Transformers-compatible PaddleOCR-VL image and text processing.
//!
//! Original implementations:
//! https://github.com/huggingface/transformers/blob/63f32a8782cb70da3365acab16f2b67947737985/src/transformers/models/paddleocr_vl/image_processing_paddleocr_vl.py
//! https://github.com/huggingface/transformers/blob/63f32a8782cb70da3365acab16f2b67947737985/src/transformers/models/paddleocr_vl/processing_paddleocr_vl.py

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use image::{DynamicImage, GrayImage, Luma};
use imageproc::{
    contrast::otsu_level,
    distance_transform::Norm,
    morphology::dilate,
    region_labelling::{Connectivity, connected_components},
};
use koharu_torch::{Device, Kind, Tensor};
use serde::{Deserialize, Serialize};
use tokenizers::Tokenizer;

const OCR_RESIZE_FACTOR: i64 = 28;
const OCR_MIN_PIXELS: i64 = 112_896;
const OCR_MAX_PIXELS: i64 = 1_003_520;
const TARGET_PROJECTED_GLYPH_SIZE: f32 = 28.0;
const MAX_SOURCE_UPSCALE: f32 = 4.0;
const ANALYSIS_MAX_SIDE: u32 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaddleOCRVLTask {
    Ocr,
    Table,
    Formula,
    Chart,
    Spotting,
    Seal,
}

impl PaddleOCRVLTask {
    pub(crate) fn prompt(self) -> &'static str {
        match self {
            Self::Ocr => "OCR:",
            Self::Table => "Table Recognition:",
            Self::Formula => "Formula Recognition:",
            Self::Chart => "Chart Recognition:",
            Self::Spotting => "Spotting:",
            Self::Seal => "Seal Recognition:",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaddleOCRVLResult {
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PaddleOCRVLImageProcessor {
    pub do_convert_rgb: bool,
    pub do_normalize: bool,
    pub do_rescale: bool,
    pub do_resize: bool,
    pub image_mean: Vec<f32>,
    pub image_std: Vec<f32>,
    pub max_pixels: i64,
    pub min_pixels: i64,
    pub merge_size: i64,
    pub patch_size: i64,
    pub rescale_factor: f64,
    pub resample: i64,
    pub temporal_patch_size: i64,
}

impl Default for PaddleOCRVLImageProcessor {
    fn default() -> Self {
        Self {
            do_convert_rgb: true,
            do_normalize: true,
            do_rescale: true,
            do_resize: true,
            image_mean: vec![0.5, 0.5, 0.5],
            image_std: vec![0.5, 0.5, 0.5],
            max_pixels: 1_003_520,
            min_pixels: 112_896,
            merge_size: 2,
            patch_size: 14,
            rescale_factor: 1.0 / 255.0,
            resample: 3,
            temporal_patch_size: 1,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Processor {
    image_processor: PaddleOCRVLImageProcessor,
    tokenizer: Tokenizer,
    image_token_id: i64,
}

impl Processor {
    pub(crate) fn from_files(
        processor_path: impl AsRef<Path>,
        tokenizer_path: impl AsRef<Path>,
        image_token_id: i64,
    ) -> Result<Self> {
        let processor_path = processor_path.as_ref();
        let tokenizer_path = tokenizer_path.as_ref();
        let image_processor = serde_json::from_str(&std::fs::read_to_string(processor_path)?)
            .with_context(|| format!("failed to parse {}", processor_path.display()))?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|error| anyhow::anyhow!(error.to_string()))
            .with_context(|| format!("failed to parse {}", tokenizer_path.display()))?;
        Ok(Self {
            image_processor,
            tokenizer,
            image_token_id,
        })
    }

    pub(crate) fn preprocess(
        &self,
        image: &DynamicImage,
        task: PaddleOCRVLTask,
        device: Device,
    ) -> Result<(Tensor, [i64; 3])> {
        ensure!(
            image.width() > 0 && image.height() > 0,
            "image dimensions must be non-zero"
        );
        let image =
            if task == PaddleOCRVLTask::Spotting && image.width() < 1500 && image.height() < 1500 {
                image.resize_exact(
                    image.width() * 2,
                    image.height() * 2,
                    image::imageops::FilterType::Lanczos3,
                )
            } else {
                image.clone()
            };
        let prepared_ocr;
        let image = if task == PaddleOCRVLTask::Ocr {
            prepared_ocr = prepare_ocr_image(&image)?;
            prepared_ocr.as_ref().unwrap_or(&image)
        } else {
            &image
        };
        let rgb = image.to_rgb8();
        let max_pixels = if task == PaddleOCRVLTask::Spotting {
            2048 * 28 * 28
        } else {
            self.image_processor.max_pixels
        };
        let (height, width) = smart_resize(
            i64::from(rgb.height()),
            i64::from(rgb.width()),
            self.image_processor.patch_size * self.image_processor.merge_size,
            self.image_processor.min_pixels,
            max_pixels,
        )?;

        let mut pixels = Tensor::from_slice(rgb.as_raw())
            .view([1, i64::from(rgb.height()), i64::from(rgb.width()), 3])
            .permute([0, 3, 1, 2]);
        if self.image_processor.do_resize {
            if self.image_processor.resample != 3 {
                bail!("PaddleOCR-VL only supports Transformers' bicubic resampling mode");
            }
            // Torch does not implement antialiased uint8 bicubic resize on CUDA.
            // Match Transformers on CPU, then make the only host-to-device copy.
            pixels = pixels.internal_upsample_bicubic2d_aa(
                [height, width],
                false,
                None::<f64>,
                None::<f64>,
            );
        }
        pixels = pixels.to_device(device).to_kind(Kind::Float);
        if self.image_processor.do_rescale {
            pixels *= self.image_processor.rescale_factor;
        }
        if self.image_processor.do_normalize {
            ensure!(
                self.image_processor.image_mean.len() == 3
                    && self.image_processor.image_std.len() == 3,
                "PaddleOCR-VL image_mean and image_std must contain three values"
            );
            let mean = Tensor::from_slice(&self.image_processor.image_mean)
                .view([1, 3, 1, 1])
                .to_device(device);
            let std = Tensor::from_slice(&self.image_processor.image_std)
                .view([1, 3, 1, 1])
                .to_device(device);
            pixels = (pixels - mean) / std;
        }

        let grid_t = 1;
        let grid_h = height / self.image_processor.patch_size;
        let grid_w = width / self.image_processor.patch_size;
        let patch = self.image_processor.patch_size;
        let pixel_values = pixels
            .view(&[1, grid_t, 3, grid_h, patch, grid_w, patch][..])
            .permute([0, 1, 3, 5, 2, 4, 6])
            .reshape([grid_t * grid_h * grid_w, 3, patch, patch]);

        Ok((pixel_values, [grid_t, grid_h, grid_w]))
    }

    pub(crate) fn encode_prompt(
        &self,
        task: PaddleOCRVLTask,
        grid: [i64; 3],
    ) -> Result<(Vec<i64>, Vec<i64>)> {
        let image_tokens = grid.into_iter().product::<i64>()
            / (self.image_processor.merge_size * self.image_processor.merge_size);
        let prompt = format!(
            "<|begin_of_sentence|>User: <|IMAGE_START|>{}<|IMAGE_END|>{}\nAssistant:\n",
            "<|IMAGE_PLACEHOLDER|>".repeat(image_tokens as usize),
            task.prompt(),
        );
        let encoding = self
            .tokenizer
            .encode(prompt, false)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let input_ids = encoding
            .get_ids()
            .iter()
            .map(|&id| i64::from(id))
            .collect::<Vec<_>>();
        let mm_token_type_ids = input_ids
            .iter()
            .map(|&id| i64::from(id == self.image_token_id))
            .collect();
        Ok((input_ids, mm_token_type_ids))
    }

    pub(crate) fn decode(&self, token_ids: &[i64]) -> Result<PaddleOCRVLResult> {
        let token_ids = token_ids
            .iter()
            .map(|&id| u32::try_from(id))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let text = self
            .tokenizer
            .decode(&token_ids, false)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Ok(PaddleOCRVLResult {
            text: super::sanitize_ocr_text(text),
        })
    }
}

/// Improves undersampled OCR crops before Paddle's projector applies its own
/// aspect-preserving min/max pixel budget. The complete user-authored region is
/// retained: this deliberately does not tile, segment, crop, or square-pad it.
///
/// Paddle's pixel budget is based on total image area, which does not guarantee
/// that glyphs from a low-resolution source occupy enough vision patches. An
/// upscale is useful only while the prepared image remains below that budget;
/// beyond it, the projector would immediately downsample by the same amount.
pub(crate) fn prepare_ocr_image(image: &DynamicImage) -> Result<Option<DynamicImage>> {
    ensure!(
        image.width() > 0 && image.height() > 0,
        "image dimensions must be non-zero"
    );
    let Some(source_glyph_size) = estimate_glyph_size(image) else {
        tracing::debug!(
            width = image.width(),
            height = image.height(),
            "could not estimate Paddle OCR glyph scale; preserving the source crop"
        );
        return Ok(None);
    };
    let projected = projected_glyph_size(image.width(), image.height(), source_glyph_size)?;
    if projected >= TARGET_PROJECTED_GLYPH_SIZE {
        tracing::debug!(
            width = image.width(),
            height = image.height(),
            source_glyph_size,
            projected_glyph_size = projected,
            "Paddle OCR crop already has sufficient glyph resolution"
        );
        return Ok(None);
    }

    let source_pixels = f64::from(image.width()) * f64::from(image.height());
    let pixel_limited_scale = ((OCR_MAX_PIXELS as f64 / source_pixels).sqrt()) as f32;
    let maximum_scale = pixel_limited_scale.min(MAX_SOURCE_UPSCALE);
    if maximum_scale <= 1.0 {
        tracing::debug!(
            width = image.width(),
            height = image.height(),
            source_glyph_size,
            projected_glyph_size = projected,
            "Paddle OCR crop is pixel-budget limited; preserving it without a redundant upscale"
        );
        return Ok(None);
    }

    let maximum_projected = projected_glyph_size_at_scale(
        image.width(),
        image.height(),
        source_glyph_size,
        maximum_scale,
    )?;
    if maximum_projected < projected * 1.1 {
        return Ok(None);
    }

    let scale = if maximum_projected < TARGET_PROJECTED_GLYPH_SIZE {
        maximum_scale
    } else {
        let mut low = 1.0f32;
        let mut high = maximum_scale;
        for _ in 0..12 {
            let candidate = (low + high) * 0.5;
            if projected_glyph_size_at_scale(
                image.width(),
                image.height(),
                source_glyph_size,
                candidate,
            )? >= TARGET_PROJECTED_GLYPH_SIZE
            {
                high = candidate;
            } else {
                low = candidate;
            }
        }
        high
    };
    let width = ((image.width() as f32 * scale).round() as u32).max(1);
    let height = ((image.height() as f32 * scale).round() as u32).max(1);
    let prepared = image.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
    tracing::debug!(
        source_width = image.width(),
        source_height = image.height(),
        prepared_width = width,
        prepared_height = height,
        source_glyph_size,
        projected_glyph_size = projected,
        prepared_projected_glyph_size =
            projected_glyph_size(width, height, source_glyph_size * scale)?,
        "upscaled undersampled Paddle OCR crop"
    );
    Ok(Some(prepared))
}

fn projected_glyph_size(width: u32, height: u32, source_glyph_size: f32) -> Result<f32> {
    let (projected_height, projected_width) = smart_resize(
        i64::from(height),
        i64::from(width),
        OCR_RESIZE_FACTOR,
        OCR_MIN_PIXELS,
        OCR_MAX_PIXELS,
    )?;
    let scale =
        (projected_width as f32 / width as f32).min(projected_height as f32 / height as f32);
    Ok(source_glyph_size * scale)
}

fn projected_glyph_size_at_scale(
    width: u32,
    height: u32,
    source_glyph_size: f32,
    scale: f32,
) -> Result<f32> {
    let width = ((width as f32 * scale).round() as u32).max(1);
    let height = ((height as f32 * scale).round() as u32).max(1);
    projected_glyph_size(width, height, source_glyph_size * scale)
}

fn estimate_glyph_size(image: &DynamicImage) -> Option<f32> {
    let longest_side = image.width().max(image.height());
    let analysis = if longest_side > ANALYSIS_MAX_SIDE {
        image.resize(
            ANALYSIS_MAX_SIDE,
            ANALYSIS_MAX_SIDE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image.clone()
    };
    let gray = analysis.to_luma8();
    let threshold = otsu_level(&gray);
    let border = border_median(&gray);
    let light_background = border >= threshold;
    let binary = GrayImage::from_fn(gray.width(), gray.height(), |x, y| {
        let value = gray.get_pixel(x, y)[0];
        let foreground = if light_background {
            value <= threshold
        } else {
            value > threshold
        };
        Luma([u8::from(foreground) * u8::MAX])
    });
    // One analysis-space pixel joins the separated strokes of a glyph without
    // merging the whitespace between normally spaced characters.
    let binary = dilate(&binary, Norm::LInf, 1);
    let labels = connected_components(&binary, Connectivity::Eight, Luma([0]));
    let mut components = Vec::<Option<Component>>::new();
    for (x, y, pixel) in labels.enumerate_pixels() {
        let label = pixel[0] as usize;
        if label == 0 {
            continue;
        }
        if label >= components.len() {
            components.resize_with(label + 1, || None);
        }
        if let Some(component) = &mut components[label] {
            component.min_x = component.min_x.min(x);
            component.min_y = component.min_y.min(y);
            component.max_x = component.max_x.max(x);
            component.max_y = component.max_y.max(y);
            component.area += 1;
        } else {
            components[label] = Some(Component {
                min_x: x,
                min_y: y,
                max_x: x,
                max_y: y,
                area: 1,
            });
        }
    }

    let image_area = u64::from(gray.width()) * u64::from(gray.height());
    let mut sizes = components
        .into_iter()
        .flatten()
        .filter_map(|component| {
            let width = component.max_x - component.min_x + 1;
            let height = component.max_y - component.min_y + 1;
            let bounds_area = u64::from(width) * u64::from(height);
            let aspect = width as f32 / height as f32;
            (component.area >= 4
                && width >= 3
                && height >= 3
                && bounds_area * 25 <= image_area
                && (0.15..=6.0).contains(&aspect)
                && component.area as f32 / bounds_area as f32 >= 0.08)
                .then_some(width.max(height).saturating_sub(2).max(1))
        })
        .collect::<Vec<_>>();
    if sizes.len() < 3 {
        return None;
    }
    sizes.sort_unstable();
    let analysis_glyph_size = sizes[(sizes.len() * 3 / 4).min(sizes.len() - 1)] as f32;
    let source_scale = (image.width() as f32 / analysis.width() as f32)
        .max(image.height() as f32 / analysis.height() as f32);
    Some(analysis_glyph_size * source_scale)
}

fn border_median(image: &GrayImage) -> u8 {
    let mut values = Vec::with_capacity((image.width() + image.height()) as usize * 2);
    for x in 0..image.width() {
        values.push(image.get_pixel(x, 0)[0]);
        if image.height() > 1 {
            values.push(image.get_pixel(x, image.height() - 1)[0]);
        }
    }
    for y in 1..image.height().saturating_sub(1) {
        values.push(image.get_pixel(0, y)[0]);
        if image.width() > 1 {
            values.push(image.get_pixel(image.width() - 1, y)[0]);
        }
    }
    values.sort_unstable();
    values[values.len() / 2]
}

struct Component {
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
    area: u64,
}

fn smart_resize(
    mut height: i64,
    mut width: i64,
    factor: i64,
    min_pixels: i64,
    max_pixels: i64,
) -> Result<(i64, i64)> {
    if height < factor {
        width = (width as f64 * factor as f64 / height as f64).round_ties_even() as i64;
        height = factor;
    }
    if width < factor {
        height = (height as f64 * factor as f64 / width as f64).round_ties_even() as i64;
        width = factor;
    }
    ensure!(
        height.max(width) as f64 / height.min(width) as f64 <= 200.0,
        "absolute aspect ratio must be smaller than 200"
    );
    let mut resized_height = ((height as f64 / factor as f64).round_ties_even() as i64) * factor;
    let mut resized_width = ((width as f64 / factor as f64).round_ties_even() as i64) * factor;
    if resized_height * resized_width > max_pixels {
        let beta = ((height * width) as f64 / max_pixels as f64).sqrt();
        resized_height =
            factor.max(((height as f64 / beta / factor as f64).floor() as i64) * factor);
        resized_width = factor.max(((width as f64 / beta / factor as f64).floor() as i64) * factor);
    } else if resized_height * resized_width < min_pixels {
        let beta = (min_pixels as f64 / (height * width) as f64).sqrt();
        resized_height = ((height as f64 * beta / factor as f64).ceil() as i64) * factor;
        resized_width = ((width as f64 * beta / factor as f64).ceil() as i64) * factor;
    }
    Ok((resized_height, resized_width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn smart_resize_matches_transformers_fixture_shapes() -> Result<()> {
        let cases = [
            ((178, 99), (476, 252)),
            ((251, 188), (392, 308)),
            ((103, 62), (448, 280)),
            ((115, 406), (196, 644)),
            ((313, 69), (728, 168)),
            ((54, 204), (196, 672)),
        ];
        for ((height, width), expected) in cases {
            assert_eq!(
                smart_resize(height, width, 28, 112_896, 1_003_520)?,
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn smart_resize_rejects_extreme_aspect_ratios() {
        assert!(smart_resize(28, 28 * 201, 28, 112_896, 1_003_520).is_err());
    }

    #[test]
    fn undersampled_glyphs_are_upscaled_without_changing_aspect_ratio() -> Result<()> {
        let image = synthetic_bubble(180, 120, 6);

        let prepared = prepare_ocr_image(&image)?.expect("small glyphs should be upscaled");

        assert!(prepared.width() > image.width());
        assert!(prepared.height() > image.height());
        assert_eq!(
            prepared.width() * image.height(),
            prepared.height() * image.width()
        );
        Ok(())
    }

    #[test]
    fn adequately_sampled_glyphs_preserve_the_source_crop() -> Result<()> {
        let image = synthetic_bubble(600, 400, 32);

        assert!(prepare_ocr_image(&image)?.is_none());
        Ok(())
    }

    #[test]
    fn an_upscale_is_not_added_when_paddle_would_downsample_it_again() -> Result<()> {
        let image = synthetic_bubble(1600, 800, 8);

        assert!(prepare_ocr_image(&image)?.is_none());
        Ok(())
    }

    #[test]
    fn blank_regions_are_left_unchanged() -> Result<()> {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(160, 100, Rgb([255; 3])));

        assert!(prepare_ocr_image(&image)?.is_none());
        Ok(())
    }

    fn synthetic_bubble(width: u32, height: u32, glyph: u32) -> DynamicImage {
        let mut image = RgbImage::from_pixel(width, height, Rgb([255; 3]));
        let gap = glyph.max(4);
        let mut x = gap;
        let mut y = gap;
        for _ in 0..12 {
            for py in y..(y + glyph).min(height) {
                for px in x..(x + glyph).min(width) {
                    image.put_pixel(px, py, Rgb([0; 3]));
                }
            }
            x += glyph + gap;
            if x + glyph >= width {
                x = gap;
                y += glyph + gap;
            }
        }
        DynamicImage::ImageRgb8(image)
    }
}
