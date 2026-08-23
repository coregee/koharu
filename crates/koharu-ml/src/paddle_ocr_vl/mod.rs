//! PaddleOCR-VL element recognition backed by commit-pinned general-purpose and
//! manga-tuned checkpoints.

mod config;
mod model;
mod processor;

use anyhow::{Context, Result};
use image::DynamicImage;
use koharu_torch::Device;

use crate::backend::TryIntoDevice;

pub use self::{
    config::{PaddleOCRVLConfig, PaddleOCRVisionConfig, RopeScaling},
    processor::{PaddleOCRVLImageProcessor, PaddleOCRVLResult, PaddleOCRVLTask},
};

pub(crate) use self::processor::prepare_ocr_image;

use self::{model::Model, processor::Processor};

pub(super) const MAX_NEW_TOKENS: usize = 4096;
pub(super) const REPETITION_PENALTY: f32 = 1.2;
// The published tokenizer assigns token zero to both padding and `<unk>`.
// A sustained run is decoder degeneration, not useful OCR output.
pub(crate) const MAX_CONSECUTIVE_UNKNOWN_TOKENS: usize = 8;

pub(crate) fn sanitize_ocr_text(text: String) -> String {
    let Some(start) = pathological_unknown_run_start(&text) else {
        return text;
    };
    text[..start].trim_end().to_owned()
}

fn pathological_unknown_run_start(text: &str) -> Option<usize> {
    const UNKNOWN_TOKEN: &str = "<unk>";

    let mut search_from = 0;
    let mut run_start = 0;
    let mut run_length = 0;
    while let Some(relative_start) = text[search_from..].find(UNKNOWN_TOKEN) {
        let start = search_from + relative_start;
        if run_length == 0 || !text[search_from..start].chars().all(char::is_whitespace) {
            run_start = start;
            run_length = 1;
        } else {
            run_length += 1;
        }
        if run_length >= MAX_CONSECUTIVE_UNKNOWN_TOKENS {
            return Some(run_start);
        }
        search_from = start + UNKNOWN_TOKEN.len();
    }
    None
}

crate::model_repository!("PaddlePaddle/PaddleOCR-VL-1.6" @ "66317acc4c9fc17bd154591ce650735cd2855f3e" {
    BASE_CONFIG = "config.json",
    BASE_WEIGHTS = "model.safetensors",
    BASE_PROCESSOR = "preprocessor_config.json",
    BASE_TOKENIZER = "tokenizer.json",
});

crate::model_repository!("jzhang533/PaddleOCR-VL-For-Manga" @ "1e8aa5f1dd90cc86fe9137c9c0b26ebde613cfe8" {
    MANGA_CONFIG = "config.json",
    MANGA_WEIGHTS = "model.safetensors",
    MANGA_PROCESSOR = "preprocessor_config.json",
    MANGA_TOKENIZER = "tokenizer.json",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaddleOCRVLVariant {
    General,
    Manga,
}

impl PaddleOCRVLVariant {
    fn repository(self) -> &'static str {
        match self {
            Self::General => "PaddlePaddle/PaddleOCR-VL-1.6",
            Self::Manga => "jzhang533/PaddleOCR-VL-For-Manga",
        }
    }

    fn files(
        self,
    ) -> (
        koharu_runtime::HuggingFaceFile<'static>,
        koharu_runtime::HuggingFaceFile<'static>,
        koharu_runtime::HuggingFaceFile<'static>,
        koharu_runtime::HuggingFaceFile<'static>,
    ) {
        match self {
            Self::General => (BASE_CONFIG, BASE_WEIGHTS, BASE_PROCESSOR, BASE_TOKENIZER),
            Self::Manga => (
                MANGA_CONFIG,
                MANGA_WEIGHTS,
                MANGA_PROCESSOR,
                MANGA_TOKENIZER,
            ),
        }
    }
}

#[derive(Debug)]
pub struct PaddleOCRVL {
    device: Device,
    model: Model,
    processor: Processor,
}

impl PaddleOCRVL {
    pub async fn load(device: crate::Device, variant: PaddleOCRVLVariant) -> Result<Self> {
        let device: Device = device.try_into_device()?;
        let repository = variant.repository();
        let (config, weights, processor, tokenizer) = variant.files();
        let config_path = config
            .resolve()
            .await
            .with_context(|| format!("failed to resolve {repository} config"))?;
        let weights_path = weights
            .resolve()
            .await
            .with_context(|| format!("failed to resolve {repository} weights"))?;
        let processor_path = processor
            .resolve()
            .await
            .with_context(|| format!("failed to resolve {repository} image processor"))?;
        let tokenizer_path = tokenizer
            .resolve()
            .await
            .with_context(|| format!("failed to resolve {repository} tokenizer"))?;

        let config = PaddleOCRVLConfig::from_file(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        let processor =
            Processor::from_files(&processor_path, &tokenizer_path, config.image_token_id)?;
        let mut model = Model::new(config, device);
        model
            .load(&weights_path)
            .with_context(|| format!("failed to load {}", weights_path.display()))?;

        Ok(Self {
            device,
            model,
            processor,
        })
    }

    pub fn inference(
        &self,
        image: &DynamicImage,
        task: PaddleOCRVLTask,
    ) -> Result<PaddleOCRVLResult> {
        koharu_torch::no_grad(|| {
            let (pixel_values, image_grid_thw) =
                self.processor.preprocess(image, task, self.device)?;
            let (input_ids, mm_token_type_ids) =
                self.processor.encode_prompt(task, image_grid_thw)?;
            let token_ids = self.model.forward(
                &input_ids,
                &mm_token_type_ids,
                &pixel_values,
                image_grid_thw,
                MAX_NEW_TOKENS,
            )?;
            self.processor.decode(&token_ids)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_pathological_unknown_suffix() {
        let unknowns = "<unk>".repeat(MAX_CONSECUTIVE_UNKNOWN_TOKENS);
        assert_eq!(sanitize_ocr_text(unknowns.clone()), "");
        assert_eq!(
            sanitize_ocr_text(format!("readable text\n{unknowns}<unk>")),
            "readable text"
        );
    }

    #[test]
    fn preserves_isolated_unknown_tokens() {
        let text = "one <unk> two <unk> three".to_owned();
        assert_eq!(sanitize_ocr_text(text.clone()), text);

        let below_threshold = "<unk>".repeat(MAX_CONSECUTIVE_UNKNOWN_TOKENS - 1);
        assert_eq!(sanitize_ocr_text(below_threshold.clone()), below_threshold);
    }

    #[test]
    fn strips_whitespace_separated_unknown_run() {
        let unknowns = ["<unk>"; MAX_CONSECUTIVE_UNKNOWN_TOKENS].join(" \n");
        assert_eq!(sanitize_ocr_text(format!("prefix {unknowns}")), "prefix");
    }
}
