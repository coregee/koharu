use std::fmt::Write;

use anyhow::Context;
use indoc::indoc;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Value, json};

use crate::{Language, TranslationContext, TranslationPurpose, TranslationRequest};

pub const DEFAULT_SYSTEM_PROMPT_TEMPLATE: &str = indoc! {"
    You are a professional manga translator.

    Translation requirements:
    - Translate every input segment from {source} into natural {target}.
    {purpose_requirements}
    - Use surrounding segments only for disambiguation and continuity; never merge or split segments.
    - Write every translated `text` value only in {target}; do not include source text, notes, explanations, or alternatives.
    - Never preserve or repeat original-language text; translate names and terms using natural {target} conventions.

    Output requirements:
    - The JSON input array delimits the text areas. Each segment has a numeric `id`, a `role`, and its OCR `text`.
    - Return only a JSON object whose `translations` array contains one object with `id` and translated `text` for every input segment.
    - Copy every input ID exactly once; order does not matter.
    - Never merge, split, omit, duplicate, or add segments.
    {context_requirements}
    {image_requirements}
    {additional_instructions}
"};

pub(crate) fn prompts(request: &TranslationRequest) -> anyhow::Result<(String, String)> {
    let system = translation_system_prompt(request);
    let context = budgeted_context(request, &system)?;
    let input = TranslationInput {
        source_language: request.source_language,
        target_language: request.target_language,
        context,
        segments: request
            .segments
            .iter()
            .enumerate()
            .map(|(id, segment)| TranslationInputSegment {
                id,
                role: segment.role,
                text: &segment.text,
            })
            .collect(),
    };
    let user = serde_json::to_string(&input).context("failed to serialize translation input")?;
    Ok((system, user))
}

pub(crate) fn translations(
    provider: &str,
    text: &str,
    source_segments: &[crate::TranslationSegment],
) -> anyhow::Result<Vec<String>> {
    let output = serde_json::from_str::<TranslationOutput>(text).with_context(|| {
        format!(
            "{provider} returned invalid translation JSON for {} segments; response was: {}",
            source_segments.len(),
            snippet(text),
        )
    })?;
    let mut translations = source_segments
        .iter()
        .map(|segment| segment.text.clone())
        .collect::<Vec<_>>();
    let mut translated = vec![false; source_segments.len()];

    for translation in output.translations {
        if translation.id < translations.len() && !translated[translation.id] {
            translations[translation.id] = translation.text;
            translated[translation.id] = true;
        }
    }

    Ok(translations)
}

/// Keeps a model response readable in a log line without truncating so hard
/// that the shape of the failure is lost.
///
/// Responses are usually pretty-printed, so line breaks and other control
/// characters are escaped rather than passed through: the snippet is formatted
/// into an error context, and one failure should stay one line.
pub(crate) fn snippet(text: &str) -> String {
    const LIMIT: usize = 2000;
    let trimmed = text.trim();
    let (visible, elided) = match trimmed.char_indices().nth(LIMIT) {
        Some((end, _)) => (&trimmed[..end], true),
        None => (trimmed, false),
    };

    let mut output = String::with_capacity(visible.len());
    for character in visible.chars() {
        match character {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            control if control.is_control() => {
                let _ = write!(output, "\\u{{{:04x}}}", control as u32);
            }
            character => output.push(character),
        }
    }
    if elided {
        let _ = write!(output, "… ({} bytes total)", trimmed.len());
    }
    output
}

pub(crate) fn output_schema(expected: usize) -> Value {
    json!({
        "type": "object",
        "properties": {
            "translations": {
                "type": "array",
                "minItems": expected,
                "maxItems": expected,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": expected.saturating_sub(1),
                            "description": "The ID copied from the corresponding input segment."
                        },
                        "text": {
                            "type": "string",
                            "description": "The translation of the input segment with this ID."
                        }
                    },
                    "required": ["id", "text"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["translations"],
        "additionalProperties": false
    })
}

fn translation_system_prompt(request: &TranslationRequest) -> String {
    let source = request
        .source_language
        .map(|language| language.to_string())
        .unwrap_or_else(|| "the detected source language".to_owned());
    let purpose_requirements = match request.purpose {
        TranslationPurpose::Dialogue => indoc! {"
            - Preserve meaning, character voice, emotional tone, relationship nuance, and emphasis.
            - Use the `role` of each segment (Narration or Speech) when choosing voice and phrasing.
            - Localize idioms naturally while keeping wording concise enough for its text area.
        "},
        TranslationPurpose::SoundEffects => indoc! {"
            - Translate each SFX as a concise, natural sound effect.
            - Prefer the target language's established onomatopoeia conventions.
        "},
    };
    let context_requirements = if request.context.is_empty() {
        ""
    } else {
        indoc! {"
            Context requirements:
            Use the supplied context only to preserve terminology, character voice, and dialogue continuity.
            Do not translate or return the context entries.
        "}
        .trim_end()
    };
    let image_requirements = if request.image.is_some() {
        indoc! {"
            Image requirements:
            Use the attached original page image as visual context for speaker identity, tone, layout, and ambiguous OCR.
            Translate only the supplied segments; do not add text seen in the image that is absent from the input segments.
        "}
        .trim_end()
    } else {
        ""
    };
    let additional_instructions = request
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|instructions| !instructions.is_empty())
        .map(|instructions| format!("Additional instructions:\n{instructions}"))
        .unwrap_or_default();
    request
        .system_prompt_template
        .as_deref()
        .unwrap_or(DEFAULT_SYSTEM_PROMPT_TEMPLATE)
        .replace("{source}", &source)
        .replace("{target}", &request.target_language.to_string())
        .replace("{purpose_requirements}", purpose_requirements.trim_end())
        .replace("{context_requirements}", context_requirements)
        .replace("{image_requirements}", image_requirements)
        .replace("{additional_instructions}", &additional_instructions)
        .trim()
        .to_owned()
}

#[derive(Serialize)]
struct TranslationInput<'a> {
    source_language: Option<Language>,
    target_language: Language,
    context: Vec<&'a TranslationContext>,
    segments: Vec<TranslationInputSegment<'a>>,
}

#[derive(Serialize)]
struct TranslationInputSegment<'a> {
    id: usize,
    role: crate::TranslationRole,
    text: &'a str,
}

fn budgeted_context<'a>(
    request: &'a TranslationRequest,
    system: &str,
) -> anyhow::Result<Vec<&'a TranslationContext>> {
    let reserve = usize::from(request.response_reserve_percent.min(90));
    let input_budget = request
        .context_window
        .saturating_mul(100_usize.saturating_sub(reserve))
        / 100;
    let mut context = request.context.iter().collect::<Vec<_>>();
    loop {
        let input = TranslationInput {
            source_language: request.source_language,
            target_language: request.target_language,
            context: context.clone(),
            segments: request
                .segments
                .iter()
                .enumerate()
                .map(|(id, segment)| TranslationInputSegment {
                    id,
                    role: segment.role,
                    text: &segment.text,
                })
                .collect(),
        };
        let payload = serde_json::to_string(&input)
            .context("failed to serialize translation input for token budgeting")?;
        if estimated_tokens(system) + estimated_tokens(&payload) + estimated_image_tokens(request)
            <= input_budget
            || context.is_empty()
        {
            return Ok(context);
        }
        context.remove(0);
    }
}

fn estimated_image_tokens(request: &TranslationRequest) -> usize {
    request.image.as_ref().map_or(0, |image| {
        let horizontal_tiles = (image.width() as usize).div_ceil(512);
        let vertical_tiles = (image.height() as usize).div_ceil(512);
        horizontal_tiles
            .saturating_mul(vertical_tiles)
            .saturating_mul(512)
    })
}

fn estimated_tokens(text: &str) -> usize {
    let (ascii, non_ascii) = text.chars().fold((0_usize, 0_usize), |counts, character| {
        if character.is_ascii() {
            (counts.0 + 1, counts.1)
        } else {
            (counts.0, counts.1 + 1)
        }
    });
    ascii.div_ceil(4).saturating_add(non_ascii)
}

#[derive(Debug, Deserialize)]
struct TranslationOutput {
    translations: Vec<TranslationOutputSegment>,
}

#[derive(Debug, Deserialize)]
struct TranslationOutputSegment {
    #[serde(deserialize_with = "deserialize_segment_id")]
    id: usize,
    text: String,
}

fn deserialize_segment_id<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SegmentId {
        Number(usize),
        String(String),
    }

    match SegmentId::deserialize(deserializer)? {
        SegmentId::Number(id) => Ok(id),
        SegmentId::String(id) => id.trim().parse().map_err(de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(role: crate::TranslationRole, text: &str) -> crate::TranslationSegment {
        crate::TranslationSegment::new(role, text)
    }

    #[test]
    fn snippet_keeps_a_response_on_one_line() {
        let flattened = snippet("{\n  \"translations\": [\n\t\"hello\"\n  ]\n}");

        assert!(!flattened.contains('\n'), "{flattened}");
        assert!(!flattened.contains('\t'), "{flattened}");
        assert_eq!(flattened, r#"{\n  "translations": [\n\t"hello"\n  ]\n}"#);
    }

    #[test]
    fn snippet_escapes_other_control_characters() {
        assert_eq!(snippet("before\u{7}after"), r"before\u{0007}after");
    }

    #[test]
    fn snippet_reports_the_length_it_elided() {
        let flattened = snippet(&"x".repeat(2_500));

        assert!(flattened.starts_with(&"x".repeat(2_000)));
        assert!(
            flattened.ends_with("\u{2026} (2500 bytes total)"),
            "{flattened}"
        );
    }

    #[test]
    fn parses_plain_json() {
        let source = [
            segment(crate::TranslationRole::Speech, "one"),
            segment(crate::TranslationRole::Speech, "two"),
        ];
        let response = r#"{"translations":[{"id":0,"text":"hello"},{"id":1,"text":"world"}]}"#;

        assert_eq!(
            translations("test", response, &source).unwrap(),
            ["hello", "world"]
        );
    }

    #[test]
    fn rejects_wrapped_and_malformed_json() {
        let source = [
            segment(crate::TranslationRole::Speech, "one"),
            segment(crate::TranslationRole::Speech, "two"),
        ];
        for response in [
            "```json\n{\"translations\":[{\"id\":0,\"text\":\"hello\"},{\"id\":1,\"text\":\"world\"}]}\n```",
            r#"{translations: [{id: 0, text: 'hello'}, {id: 1, text: 'world'},],}"#,
            r#"Here is the result: {"translations": [{"id": 0, "text": "hello"}, {"id": 1, "text": "world"},]}"#,
            "{\"translations\":[{\"id\":0,\"text\":\"hello\"},{\"id\":1,\"text\":\"world\"",
        ] {
            assert!(
                translations("test", response, &source).is_err(),
                "{response}"
            );
        }
    }

    #[test]
    fn parse_error_preserves_the_root_failure_and_response() {
        let source = [segment(crate::TranslationRole::Speech, "one")];
        let response = "{\n  \"translations\": [{\"id\": 0}]\n}";
        let error = format!(
            "{:#}",
            translations("test", response, &source).expect_err("missing text should fail")
        );

        assert!(error.contains("missing field `text`"), "{error}");
        assert!(
            error.contains(r#"response was: {\n  "translations": [{"id": 0}]\n}"#),
            "{error}"
        );
    }

    #[test]
    fn restores_input_order_from_ids() {
        let source = [
            segment(crate::TranslationRole::Speech, "one"),
            segment(crate::TranslationRole::Speech, "two"),
        ];
        let response = r#"{"translations":[{"id":1,"text":"world"},{"id":0,"text":"hello"}]}"#;
        assert_eq!(
            translations("test", response, &source).unwrap(),
            ["hello", "world"]
        );
    }

    #[test]
    fn tolerates_duplicate_missing_and_out_of_range_ids() {
        let source = [
            segment(crate::TranslationRole::Speech, "one"),
            segment(crate::TranslationRole::Speech, "two"),
        ];
        let short = r#"{"translations":[{"id":1,"text":"world"}]}"#;
        assert_eq!(
            translations("test", short, &source).unwrap(),
            ["one", "world"]
        );

        let response = concat!(
            r#"{"translations":["#,
            r#"{"id":0,"text":"hello"},"#,
            r#"{"id":0,"text":"duplicate"},"#,
            r#"{"id":9,"text":"extra"}"#,
            "]}"
        );
        assert_eq!(
            translations("test", response, &source).unwrap(),
            ["hello", "two"]
        );
    }

    #[test]
    fn prompt_payload_contains_ordered_context() {
        let request = TranslationRequest::new(
            [segment(crate::TranslationRole::Narration, "new")],
            Language::English,
        )
        .with_context([TranslationContext::new(
            1,
            crate::TranslationRole::Speech,
            "old",
            Some("previous".to_owned()),
        )]);
        let (_, user) = prompts(&request).unwrap();
        let input: serde_json::Value = serde_json::from_str(&user).unwrap();
        assert_eq!(input["context"][0]["source"], "old");
        assert_eq!(input["context"][0]["translation"], "previous");
        assert_eq!(input["context"][0]["page"], 1);
        assert_eq!(input["context"][0]["role"], "Speech");
        assert_eq!(input["segments"][0]["id"], 0);
        assert_eq!(input["segments"][0]["role"], "Narration");
        assert_eq!(input["segments"][0]["text"], "new");
    }

    #[test]
    fn system_prompt_encodes_invariants_and_custom_instructions() {
        let request = TranslationRequest::new(
            [segment(crate::TranslationRole::Speech, "hello")],
            Language::Korean,
        )
        .with_source_language(Language::Japanese)
        .with_instructions("Use informal speech.");
        let prompt = translation_system_prompt(&request);
        assert!(prompt.contains("from Japanese into natural Korean"));
        assert!(prompt.contains("Copy every input ID exactly once"));
        assert!(prompt.contains("Use informal speech."));
    }

    #[test]
    fn custom_system_prompt_template_resolves_request_fields() {
        let request = TranslationRequest::new(
            [segment(crate::TranslationRole::Speech, "hello")],
            Language::Korean,
        )
        .with_source_language(Language::Japanese)
        .with_instructions("Use informal speech.")
        .with_system_prompt_template(
            "Translate {source} into {target}.\n{purpose_requirements}\n{additional_instructions}",
        );

        let prompt = translation_system_prompt(&request);

        assert!(prompt.contains("Translate Japanese into Korean."));
        assert!(prompt.contains("Preserve meaning, character voice"));
        assert!(prompt.contains("Additional instructions:\nUse informal speech."));
    }

    #[test]
    fn schema_requires_the_expected_number_of_id_text_pairs() {
        let schema = output_schema(3);
        let translations = &schema["properties"]["translations"];
        assert_eq!(translations["minItems"], 3);
        assert_eq!(translations["maxItems"], 3);
        assert_eq!(translations["items"]["properties"]["id"]["minimum"], 0);
        assert_eq!(translations["items"]["properties"]["id"]["maximum"], 2);
        assert_eq!(translations["items"]["additionalProperties"], false);
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn empty_custom_instructions_are_ignored() {
        let request = TranslationRequest::new(
            [segment(crate::TranslationRole::Speech, "hello")],
            Language::English,
        )
        .with_instructions("  ");
        assert!(!translation_system_prompt(&request).contains("Additional instructions"));
    }

    #[test]
    fn context_is_reference_only() {
        let request = TranslationRequest::new(
            [segment(crate::TranslationRole::Speech, "Where is she?")],
            Language::Japanese,
        )
        .with_context([TranslationContext::new(
            1,
            crate::TranslationRole::Speech,
            "I saw Alice.",
            Some("アリスを見た。".to_owned()),
        )]);
        let prompt = translation_system_prompt(&request);
        assert!(prompt.contains("dialogue continuity"));
        assert!(prompt.contains("Do not translate or return the context"));
    }

    #[test]
    fn image_context_does_not_expand_the_translation_scope() {
        let request = TranslationRequest::new(
            [segment(crate::TranslationRole::Speech, "text")],
            Language::English,
        )
        .with_image(std::sync::Arc::new(image::DynamicImage::new_rgb8(1, 1)));
        let prompt = translation_system_prompt(&request);
        assert!(prompt.contains("attached original page image"));
        assert!(prompt.contains("Translate only the supplied segments"));
    }

    #[test]
    fn token_budget_discards_oldest_context_first_and_keeps_response_space() {
        let context = (0..12)
            .map(|page| {
                TranslationContext::new(
                    page + 1,
                    crate::TranslationRole::Speech,
                    format!("source-{page}-{}", "x".repeat(400)),
                    Some(format!("translation-{page}-{}", "y".repeat(400))),
                )
            })
            .collect::<Vec<_>>();
        let request = TranslationRequest::new(
            [segment(crate::TranslationRole::Speech, "current")],
            Language::English,
        )
        .with_context(context)
        .with_context_window(1_024, 30);

        let (_, user) = prompts(&request).unwrap();
        let input: serde_json::Value = serde_json::from_str(&user).unwrap();
        let retained = input["context"].as_array().unwrap();
        assert!(retained.len() < 12);
        assert_eq!(retained.last().unwrap()["page"], 12);
        if let Some(first) = retained.first() {
            assert!(first["page"].as_u64().unwrap() > 1);
        }
    }

    #[test]
    fn sound_effect_pass_has_a_specialized_context_free_prompt() {
        let request = TranslationRequest::new(
            [segment(crate::TranslationRole::Sfx, "ドン")],
            Language::English,
        )
        .with_purpose(TranslationPurpose::SoundEffects);
        let (system, user) = prompts(&request).unwrap();
        let input: serde_json::Value = serde_json::from_str(&user).unwrap();

        assert!(system.contains("concise, natural sound effect"));
        assert!(!system.contains("Context requirements"));
        assert_eq!(input["segments"][0]["role"], "Sfx");
    }
}
