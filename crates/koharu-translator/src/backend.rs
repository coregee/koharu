use std::{io::Cursor, sync::Arc};

use anyhow::Context;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, ImageEncoder, RgbImage, codecs::jpeg::JpegEncoder};

use crate::Language;

const MAX_IMAGE_DIMENSION: u32 = 2048;
const JPEG_QUALITY: u8 = 88;

#[derive(Debug, Clone, PartialEq)]
pub struct TranslationRequest {
    pub segments: Vec<TranslationSegment>,
    pub source_language: Option<Language>,
    pub target_language: Language,
    pub instructions: Option<String>,
    pub context: Vec<TranslationContext>,
    pub context_window: usize,
    pub response_reserve_percent: u8,
    pub purpose: TranslationPurpose,
    pub image: Option<Arc<DynamicImage>>,
}

impl TranslationRequest {
    #[must_use]
    pub fn new(
        segments: impl IntoIterator<Item = TranslationSegment>,
        target_language: Language,
    ) -> Self {
        Self {
            segments: segments.into_iter().collect(),
            source_language: None,
            target_language,
            instructions: None,
            context: Vec::new(),
            context_window: 32_768,
            response_reserve_percent: 30,
            purpose: TranslationPurpose::Dialogue,
            image: None,
        }
    }

    #[must_use]
    pub fn with_source_language(mut self, language: Language) -> Self {
        self.source_language = Some(language);
        self
    }

    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    #[must_use]
    pub fn with_image(mut self, image: Arc<DynamicImage>) -> Self {
        self.image = Some(image);
        self
    }

    #[must_use]
    pub fn with_context(mut self, context: impl IntoIterator<Item = TranslationContext>) -> Self {
        self.context = context.into_iter().collect();
        self
    }

    #[must_use]
    pub fn with_context_window(
        mut self,
        context_window: usize,
        response_reserve_percent: u8,
    ) -> Self {
        self.context_window = context_window;
        self.response_reserve_percent = response_reserve_percent;
        self
    }

    #[must_use]
    pub fn with_purpose(mut self, purpose: TranslationPurpose) -> Self {
        self.purpose = purpose;
        self
    }

    #[must_use]
    pub fn source_segments(&self) -> Vec<String> {
        self.segments
            .iter()
            .map(|segment| segment.text.clone())
            .collect()
    }

    pub(crate) fn prepare_image(&mut self) -> anyhow::Result<()> {
        let Some(image) = self.image.as_ref() else {
            return Ok(());
        };
        let (width, height) = (image.width(), image.height());
        let longest = width.max(height);
        if longest <= MAX_IMAGE_DIMENSION {
            return Ok(());
        }
        let resized_width =
            (u64::from(width) * u64::from(MAX_IMAGE_DIMENSION) / u64::from(longest)).max(1) as u32;
        let resized_height =
            (u64::from(height) * u64::from(MAX_IMAGE_DIMENSION) / u64::from(longest)).max(1) as u32;
        let source = image.to_rgb8();
        let mut resized = RgbImage::new(resized_width, resized_height);
        Resizer::new()
            .resize(
                &source,
                &mut resized,
                &ResizeOptions::new()
                    .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))
                    .use_alpha(false),
            )
            .context("failed to resize translation image")?;
        self.image = Some(Arc::new(DynamicImage::ImageRgb8(resized)));
        Ok(())
    }

    pub(crate) fn remove_image(&mut self) {
        self.image = None;
    }
}

pub(crate) struct EncodedImage {
    pub(crate) data: String,
}

impl EncodedImage {
    pub(crate) fn data_url(&self) -> String {
        format!("data:image/jpeg;base64,{}", self.data)
    }
}

pub(crate) fn encode_image(image: &DynamicImage) -> anyhow::Result<EncodedImage> {
    let rgb = image.to_rgb8();
    let mut bytes = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .context("failed to encode translation image")?;
    Ok(EncodedImage {
        data: STANDARD.encode(bytes.into_inner()),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TranslationContext {
    pub page: usize,
    pub role: TranslationRole,
    pub source: String,
    pub translation: Option<String>,
}

impl TranslationContext {
    #[must_use]
    pub fn new(
        page: usize,
        role: TranslationRole,
        source: impl Into<String>,
        translation: Option<String>,
    ) -> Self {
        Self {
            page,
            role,
            source: source.into(),
            translation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum TranslationRole {
    Narration,
    Speech,
    Sfx,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationSegment {
    pub role: TranslationRole,
    pub text: String,
}

impl TranslationSegment {
    #[must_use]
    pub fn new(role: TranslationRole, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TranslationPurpose {
    #[default]
    Dialogue,
    SoundEffects,
}
