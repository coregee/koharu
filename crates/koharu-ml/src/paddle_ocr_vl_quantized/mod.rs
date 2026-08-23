//! Quantized PaddleOCR-VL-1.6 inference through llama.cpp.

use anyhow::{Context, Result, ensure};
use image::DynamicImage;
use koharu_llama::mtmd::mtmd_default_marker;

use crate::{
    Device,
    llm::{
        ChatMessage, GenerationControl, GenerationOptions, Input, Llm, LoadOptions, MtmdOptions,
    },
    paddle_ocr_vl::{
        MAX_CONSECUTIVE_UNKNOWN_TOKENS, MAX_NEW_TOKENS, PaddleOCRVLResult, PaddleOCRVLTask,
        REPETITION_PENALTY, prepare_ocr_image, sanitize_ocr_text,
    },
};

crate::model_repository!("PaddlePaddle/PaddleOCR-VL-1.6-GGUF" @ "511b09642bb324401f15f97cc23bc67e8f0a291d" {
    MODEL = "PaddleOCR-VL-1.6-GGUF.gguf",
    PROJECTOR = "PaddleOCR-VL-1.6-GGUF-mmproj.gguf",
});

#[derive(Debug)]
pub struct PaddleOCRVLQuantized {
    model: Llm,
}

impl PaddleOCRVLQuantized {
    pub async fn load(device: Device) -> Result<Self> {
        let (model_path, projector_path) =
            tokio::try_join!(MODEL.resolve(), PROJECTOR.resolve())
                .context("failed to resolve quantized PaddleOCR-VL-1.6 files")?;
        let model = Llm::load_with_options(
            device,
            model_path,
            LoadOptions {
                mtmd: Some(MtmdOptions::new(projector_path)),
                ..LoadOptions::default()
            },
        )
        .await
        .context("failed to load quantized PaddleOCR-VL-1.6")?;
        ensure!(
            model.capabilities().vision,
            "PaddleOCR-VL-1.6 projector does not support images"
        );
        Ok(Self { model })
    }

    pub fn inference(
        &self,
        image: &DynamicImage,
        task: PaddleOCRVLTask,
    ) -> Result<PaddleOCRVLResult> {
        let prepared_ocr;
        let image = if task == PaddleOCRVLTask::Ocr {
            prepared_ocr = prepare_ocr_image(image)?;
            prepared_ocr.as_ref().unwrap_or(image)
        } else {
            image
        };
        let prompt = self.model.render_chat_prompt(&[ChatMessage::user(format!(
            "{}{}",
            mtmd_default_marker(),
            task.prompt()
        ))])?;
        let mut consecutive_unknown_tokens = 0;
        let generation = self.model.inference_with_callback(
            &Input::new(&prompt).with_image(image),
            &GenerationOptions {
                max_tokens: MAX_NEW_TOKENS,
                temperature: 0.0,
                repeat_penalty: REPETITION_PENALTY,
                repeat_last_n: MAX_NEW_TOKENS as i32,
                add_special: true,
                ..GenerationOptions::default()
            },
            |chunk| {
                consecutive_unknown_tokens = if chunk.piece == "<unk>" {
                    consecutive_unknown_tokens + 1
                } else {
                    0
                };
                Ok(
                    if consecutive_unknown_tokens >= MAX_CONSECUTIVE_UNKNOWN_TOKENS {
                        GenerationControl::Stop
                    } else {
                        GenerationControl::Continue
                    },
                )
            },
        )?;
        Ok(PaddleOCRVLResult {
            text: sanitize_ocr_text(generation.text),
        })
    }
}
