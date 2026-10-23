'use client'

import { ChevronDown, RotateCcw } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { ModelPicker } from '@/components/controls/ModelPicker'
import { GenerationPreferences } from '@/components/preferences/GenerationPreferences'
import {
  NumberField,
  PreferencePage,
  PreferenceRow,
  PreferenceSection,
} from '@/components/preferences/PreferenceFields'
import { modelKey, orderedLanguageChoices, providerName } from '@/lib/translation'
import type {
  LanguageChoice,
  Model,
  ProviderPreference,
  TranslationConfig as TranslationSettings,
  TranslationPromptSettings,
} from '@koharu/bridge/protocol'
import { Badge } from '@koharu/ui/components/badge'
import { Popover, PopoverContent, PopoverTrigger } from '@koharu/ui/components/popover'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@koharu/ui/components/select'
import { Textarea } from '@koharu/ui/components/textarea'

export function TranslationPreferences({
  value,
  modelChoices,
  providers,
  languages,
  onChange,
  systemPrompt,
  systemPromptDraft,
  onSystemPromptChange,
}: {
  value: TranslationSettings
  modelChoices: Model[]
  providers: ProviderPreference[]
  languages: LanguageChoice[]
  onChange: (value: TranslationSettings) => void
  systemPrompt: TranslationPromptSettings | null
  systemPromptDraft: string | null
  onSystemPromptChange: (value: string) => void
}) {
  const { t } = useTranslation()
  const [modelOpen, setModelOpen] = useState(false)
  const selected =
    modelChoices.find((candidate) => modelKey(candidate) === modelKey(value.model)) ?? null
  const current: Model = selected ?? {
    ...value.model,
    model: value.model.model ?? null,
    name: value.model.model ?? providerName(providers, value.model.provider),
    quantizations: [],
    vision: value.model.vision ?? false,
    reasoning: value.model.reasoning ?? false,
    context_window: value.model.context_window ?? null,
  }
  const choices = selected ? modelChoices : [current, ...modelChoices]
  const quantization =
    current.quantizations.find((quantization) => quantization.id === value.model.quantization) ??
    current.quantizations[0]
  const languageChoices = useMemo(() => orderedLanguageChoices(languages), [languages])
  return (
    <PreferencePage
      title={t('settings.translation.title')}
      description={t('settings.translation.description')}
    >
      <PreferenceSection
        title={t('settings.translation.model')}
        description={t('settings.translation.modelDescription')}
      >
        <PreferenceRow
          title={t('settings.translation.translationModel')}
          description={t('settings.translation.translationModelDescription')}
        >
          <Popover open={modelOpen} onOpenChange={setModelOpen}>
            <PopoverTrigger
              type='button'
              aria-label={t('settings.translation.translationModel')}
              className='flex h-9 w-full min-w-0 items-center justify-between gap-2 rounded-lg border border-input bg-transparent px-2.5 text-[11px] transition-colors outline-none hover:bg-foreground/[0.03] focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50'
            >
              <span className='min-w-0 flex-1 text-left'>
                <ModelLabel
                  model={current}
                  providers={providers}
                  quantization={quantization?.name ?? value.model.quantization}
                />
              </span>
              <ChevronDown className='size-3.5 shrink-0 text-muted-foreground' />
            </PopoverTrigger>
            <PopoverContent
              align='start'
              sideOffset={4}
              className='w-(--anchor-width) min-w-64 gap-0 overflow-hidden rounded-xl border border-border/50 p-1 shadow-sm ring-0'
            >
              <ModelPicker
                value={value.model}
                models={choices}
                providers={providers}
                onBack={() => setModelOpen(false)}
                onSelect={(model) => {
                  onChange({
                    ...value,
                    model,
                  })
                  setModelOpen(false)
                }}
              />
            </PopoverContent>
          </Popover>
        </PreferenceRow>
      </PreferenceSection>

      <GenerationPreferences
        value={value.generation}
        onChange={(generation) => onChange({ ...value, generation })}
      />

      <PreferenceSection title={t('settings.translation.output')}>
        <PreferenceRow
          title={t('model.sourceLanguage')}
          description={t('settings.translation.sourceLanguageDescription')}
        >
          <Select
            value={value.source_language ?? '__auto__'}
            items={{
              __auto__: t('outputPicker.autoDetect'),
              ...Object.fromEntries(
                languageChoices.map((language) => [language.tag, language.name]),
              ),
            }}
            onValueChange={(source_language) =>
              source_language &&
              onChange({
                ...value,
                source_language: source_language === '__auto__' ? null : source_language,
              })
            }
          >
            <SelectTrigger
              aria-label={t('model.sourceLanguage')}
              className='h-8 w-full text-[11px]'
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value='__auto__'>{t('outputPicker.autoDetect')}</SelectItem>
              {languageChoices.map((language) => (
                <SelectItem key={language.tag} value={language.tag}>
                  {language.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </PreferenceRow>
        <PreferenceRow
          title={t('model.targetLanguage')}
          description={t('settings.translation.targetLanguageDescription')}
        >
          <Select
            value={value.target_language}
            items={Object.fromEntries(
              languageChoices.map((language) => [language.tag, language.name]),
            )}
            onValueChange={(target_language) =>
              target_language && onChange({ ...value, target_language })
            }
          >
            <SelectTrigger
              aria-label={t('model.targetLanguage')}
              className='h-8 w-full text-[11px]'
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {languageChoices.map((language) => (
                <SelectItem key={language.tag} value={language.tag}>
                  {language.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </PreferenceRow>
        <PreferenceRow
          title={t('model.instructions')}
          description={t('settings.translation.instructionsDescription')}
          align='start'
        >
          <Textarea
            aria-label={t('settings.translation.instructionsLabel')}
            value={value.instructions ?? ''}
            className='field-sizing-fixed min-h-24 resize-y overflow-y-auto text-[12px] leading-5'
            placeholder={t('settings.translation.instructionsPlaceholder')}
            onChange={(event) =>
              onChange({ ...value, instructions: event.currentTarget.value || null })
            }
          />
        </PreferenceRow>
      </PreferenceSection>

      <PreferenceSection
        title={t('settings.translation.systemPrompt')}
        description={t('settings.translation.systemPromptDescription')}
      >
        <PreferenceRow
          title={t('settings.translation.systemPromptTemplate')}
          description={t('settings.translation.systemPromptTemplateDescription')}
          align='start'
        >
          <div className='grid gap-2'>
            <Textarea
              aria-label={t('settings.translation.systemPromptTemplate')}
              value={systemPromptDraft ?? ''}
              disabled={!systemPrompt?.project_open}
              className='field-sizing-fixed min-h-80 resize-y overflow-y-auto font-mono text-[11px] leading-5'
              onChange={(event) => onSystemPromptChange(event.currentTarget.value)}
            />
            <button
              type='button'
              disabled={
                !systemPrompt?.project_open || systemPromptDraft === systemPrompt.default_template
              }
              className='inline-flex h-8 items-center justify-center gap-1.5 rounded-lg border border-input px-2.5 text-[11px] disabled:opacity-50'
              onClick={() => systemPrompt && onSystemPromptChange(systemPrompt.default_template)}
            >
              <RotateCcw className='size-3.5' />
              {t('settings.translation.resetSystemPrompt')}
            </button>
          </div>
        </PreferenceRow>
      </PreferenceSection>

      <PreferenceSection
        title={t('settings.translation.context')}
        description={t('settings.translation.contextDescription')}
      >
        <PreferenceRow
          title={t('settings.translation.contextBudget')}
          description={t('settings.translation.contextBudgetDescription')}
          align='start'
        >
          <div className='grid grid-cols-2 gap-2'>
            <NumberField
              label={t('settings.translation.contextWindow')}
              value={value.context_window ?? null}
              min={1024}
              step={1024}
              onChange={(context_window) => onChange({ ...value, context_window })}
            />
            <NumberField
              label={t('settings.translation.responseReserve')}
              value={value.response_reserve_percent}
              min={10}
              max={90}
              step={5}
              onChange={(response_reserve_percent) =>
                response_reserve_percent && onChange({ ...value, response_reserve_percent })
              }
            />
          </div>
        </PreferenceRow>
      </PreferenceSection>
    </PreferencePage>
  )
}

function ModelLabel({
  model,
  providers,
  quantization,
}: {
  model: Model
  providers: ProviderPreference[]
  quantization: string | null | undefined
}) {
  return (
    <span className='flex min-w-0 items-center gap-2'>
      <Badge variant='outline' className='shrink-0 px-1.5 py-0 text-[9px] font-medium'>
        {providerName(providers, model.provider)}
      </Badge>
      <span className='truncate'>
        {model.name}
        {quantization && <span className='text-muted-foreground'> · {quantization}</span>}
      </span>
    </span>
  )
}
