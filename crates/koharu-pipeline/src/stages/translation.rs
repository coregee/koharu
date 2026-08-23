use std::{collections::BTreeMap, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use koharu_scene::{
    Authored, EntityId, LanguageTag, OcrCandidate, Origin, Snapshot, SourceText, TextRole,
    Translation,
};
use koharu_translator::{
    TranslationContext, TranslationPurpose, TranslationRequest, TranslationRole,
    TranslationSegment, Translator,
};

use crate::TranslationConfig;

use super::{StageInput, StageProcessor, finish, generation};

const PRODUCER: &str = "dev.koharu.pipeline.translation";

pub(super) struct Processor {
    config: TranslationConfig,
    translator: Translator,
}

impl Processor {
    pub(super) fn new(config: TranslationConfig, translator: Translator) -> Self {
        Self { config, translator }
    }

    async fn translate(
        &self,
        targets: &[PageSegment],
        purpose: TranslationPurpose,
        context: Vec<TranslationContext>,
        image: Option<Arc<image::DynamicImage>>,
    ) -> Result<(&'static str, Vec<String>)> {
        let mut request = TranslationRequest::new(
            targets
                .iter()
                .map(|target| TranslationSegment::new(target.role, target.source.clone())),
            self.config.target_language,
        )
        .with_context(context)
        .with_context_window(
            self.config
                .context_window
                .or(self.config.model.context_window)
                .unwrap_or(32_768) as usize,
            self.config.response_reserve_percent,
        )
        .with_purpose(purpose);
        if let Some(source_language) = self.config.source_language {
            request = request.with_source_language(source_language);
        }
        if let Some(instructions) = self.config.instructions.as_deref() {
            request = request.with_instructions(instructions);
        }
        if let Some(image) = image {
            request = request.with_image(image);
        }
        self.translator
            .translate(&self.config.model, self.config.generation, request)
            .await
    }
}

#[async_trait]
impl StageProcessor for Processor {
    fn model(&self) -> &'static str {
        Translator::model(&self.config.model)
    }

    fn unload(&self) -> bool {
        self.translator.unload()
    }

    async fn load(&self) -> Result<()> {
        self.translator.load_model(&self.config.model).await
    }

    async fn process(&self, input: StageInput) -> Result<koharu_scene::Patch> {
        let page_segments = page_segments(&input.scene, input.page)?;
        let mut selected = Vec::new();
        for segment in &page_segments {
            if input.contains_entity(segment.layer)? {
                selected.push(segment.clone());
            }
        }
        let dialogue = selected
            .iter()
            .filter(|segment| segment.role != TranslationRole::Sfx)
            .cloned()
            .collect::<Vec<_>>();
        let sound_effects = selected
            .iter()
            .filter(|segment| segment.role == TranslationRole::Sfx)
            .cloned()
            .collect::<Vec<_>>();
        if dialogue.is_empty() && sound_effects.is_empty() {
            return finish(input.scene.edit());
        }

        let image = if !dialogue.is_empty()
            && Translator::supports_vision(&self.config.model, &self.config.generation)
        {
            input.images.get(&input.scene, input.page, "source").await?
        } else {
            None
        };
        let context = if dialogue.is_empty() {
            Vec::new()
        } else {
            dialogue_context(
                &input.scene,
                input.page,
                &page_segments,
                &dialogue,
                input.has_entity_scope(),
                self.config.target_language.tag(),
            )?
        };
        let mut translated = BTreeMap::new();
        let mut provider = None;
        if !dialogue.is_empty() {
            let (used_provider, values) = self
                .translate(&dialogue, TranslationPurpose::Dialogue, context, image)
                .await?;
            provider = Some(used_provider);
            translated.extend(dialogue.iter().map(|target| target.content).zip(values));
        }
        if !sound_effects.is_empty() {
            let (used_provider, values) = self
                .translate(
                    &sound_effects,
                    TranslationPurpose::SoundEffects,
                    Vec::new(),
                    None,
                )
                .await?;
            provider = Some(used_provider);
            translated.extend(
                sound_effects
                    .iter()
                    .map(|target| target.content)
                    .zip(values),
            );
        }

        let language = LanguageTag::new(self.config.target_language.tag())?;
        let generated = generation(
            PRODUCER,
            provider.expect("a non-empty translation pass returned a provider"),
        )?;
        let mut edit = input.scene.edit_as(generated.clone());
        for target in &selected {
            edit.observe::<SourceText>(target.content)?;
            edit.observe::<Translation>(target.content)?;
            if input
                .scene
                .component::<Translation>(target.content)?
                .is_some_and(|value| matches!(value.text.origin, Origin::User))
                && !target.delegated
            {
                continue;
            }
            let Some(text) = translated.remove(&target.content) else {
                continue;
            };
            let text = if target.source.trim() == "\u{2026}" {
                "\u{2026}".to_owned()
            } else {
                text
            };
            if target.delegated {
                edit.accept_ocr_delegation(target.content)?;
            }
            edit.set(
                target.content,
                &Translation {
                    text: Authored::generated(text, generated.clone()),
                    language: Some(language.clone()),
                },
            )?;
        }
        finish(edit)
    }
}

#[derive(Clone)]
struct PageSegment {
    layer: EntityId,
    content: EntityId,
    source: String,
    translation: Option<Translation>,
    role: TranslationRole,
    delegated: bool,
}

fn page_segments(scene: &Snapshot, page: EntityId) -> Result<Vec<PageSegment>> {
    let mut segments = Vec::new();
    if let Some(group) = scene.page(page)?.text_group()? {
        for layer in group.text_layers()? {
            let content = layer.content()?;
            let Some(source) = content.source()? else {
                continue;
            };
            if source.text.value.trim().is_empty() {
                continue;
            }
            segments.push(PageSegment {
                layer: layer.id(),
                content: content.id(),
                source: source.text.value,
                translation: content.translation()?,
                role: translation_role(content.role()?.as_ref()),
                delegated: scene.component::<OcrCandidate>(content.id())?.is_some(),
            });
        }
    }
    Ok(segments)
}

fn translation_role(role: Option<&TextRole>) -> TranslationRole {
    match role.map(|role| role.role.as_str()) {
        Some(TextRole::NARRATION) => TranslationRole::Narration,
        Some(TextRole::SFX) => TranslationRole::Sfx,
        _ => TranslationRole::Speech,
    }
}

fn dialogue_context(
    scene: &Snapshot,
    page: EntityId,
    current_page: &[PageSegment],
    targets: &[PageSegment],
    include_same_page: bool,
    target_language: &str,
) -> Result<Vec<TranslationContext>> {
    let mut context = Vec::new();
    let mut current_page_number = 1;
    for (index, candidate) in scene.pages().enumerate() {
        let page_number = index + 1;
        if candidate.id() == page {
            current_page_number = page_number;
            break;
        }
        context.extend(
            page_segments(scene, candidate.id())?
                .into_iter()
                .filter(|segment| segment.role != TranslationRole::Sfx)
                .map(|segment| context_segment(page_number, segment, target_language)),
        );
    }

    if include_same_page
        && let Some(first_target) = targets
            .iter()
            .filter_map(|target| {
                current_page
                    .iter()
                    .position(|segment| segment.content == target.content)
            })
            .min()
    {
        context.extend(
            current_page[..first_target]
                .iter()
                .filter(|segment| segment.role != TranslationRole::Sfx)
                .cloned()
                .map(|segment| context_segment(current_page_number, segment, target_language)),
        );
    }
    Ok(context)
}

fn context_segment(page: usize, segment: PageSegment, target_language: &str) -> TranslationContext {
    let translation = segment.translation.and_then(|translation| {
        let matches_target = translation
            .language
            .as_ref()
            .is_none_or(|language| language.as_str() == target_language);
        (matches_target && !translation.text.value.trim().is_empty())
            .then_some(translation.text.value)
    });
    TranslationContext::new(page, segment.role, segment.source, translation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use koharu_scene::{At, Edit, PageDraft, TextLayout, TextLayoutKind};

    fn add_segment(
        edit: &mut Edit,
        page: EntityId,
        role: &str,
        source: &str,
        translation: Option<&str>,
    ) -> koharu_scene::Result<EntityId> {
        let content = edit.add_text_content(page, At::End)?;
        edit.set(
            content,
            &SourceText {
                text: Authored::user(source.to_owned()),
                language: None,
            },
        )?;
        edit.set(
            content,
            &TextRole {
                origin: Origin::User,
                role: role.to_owned(),
            },
        )?;
        if let Some(translation) = translation {
            edit.set(
                content,
                &Translation {
                    text: Authored::user(translation.to_owned()),
                    language: Some(LanguageTag::new("en-US")?),
                },
            )?;
        }
        edit.add_text_layer(
            page,
            At::End,
            content,
            &TextLayout {
                origin: Origin::User,
                kind: TextLayoutKind::Paragraph,
            },
        )?;
        Ok(content)
    }

    #[tokio::test]
    async fn context_uses_prior_pages_and_preceding_same_page_dialogue_in_layer_order() {
        let mut session = koharu_scene::Session::memory().await.unwrap();
        let mut current_page = None;
        let patch = session
            .snapshot()
            .patch(|edit| {
                let first = edit.add_page(PageDraft::new("one", 1.0, 1.0), At::End)?;
                add_segment(
                    edit,
                    first,
                    TextRole::NARRATION,
                    "Previously",
                    Some("Earlier"),
                )?;
                add_segment(edit, first, TextRole::SFX, "ドン", Some("Boom"))?;

                let second = edit.add_page(PageDraft::new("two", 1.0, 1.0), At::End)?;
                add_segment(edit, second, TextRole::SPEECH, "こんにちは", None)?;
                add_segment(edit, second, TextRole::NARRATION, "今", None)?;
                current_page = Some(second);
                Ok(())
            })
            .unwrap();
        let scene = session.commit(patch).await.unwrap().snapshot;
        let page = current_page.unwrap();
        let segments = page_segments(&scene, page).unwrap();

        let page_context =
            dialogue_context(&scene, page, &segments, &segments, false, "en-US").unwrap();
        assert_eq!(page_context.len(), 1);
        assert_eq!(page_context[0].source, "Previously");
        assert_eq!(page_context[0].translation.as_deref(), Some("Earlier"));

        let bubble_context =
            dialogue_context(&scene, page, &segments, &segments[1..], true, "en-US").unwrap();
        assert_eq!(
            bubble_context
                .iter()
                .map(|entry| entry.source.as_str())
                .collect::<Vec<_>>(),
            ["Previously", "こんにちは"]
        );
        assert_eq!(bubble_context[1].page, 2);
        assert_eq!(bubble_context[1].translation, None);
    }
}
