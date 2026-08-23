'use client'

import { ChevronDown, Eraser, FileText, Languages, Search, type LucideIcon } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { OutputPicker, type OutputDraft } from '@/components/controls/OutputPicker'
import { InferenceControl } from '@/components/editor/InferenceControl'
import {
  defaultModel,
  modelNames,
  modelOptions,
  replaceStage,
  stageModel,
  type ModelName,
  type ModelStage,
} from '@/components/preferences/models'
import { call, savePreferences } from '@/lib/backend'
import { usePage } from '@/lib/queries'
import { pipelineStages, receivePreferences, useKoharuStore, type PipelineScope } from '@/lib/store'
import { commands, type Scope, type Stage } from '@koharu/bridge/protocol'
import { Button } from '@koharu/ui/components/button'
import { ButtonGroup } from '@koharu/ui/components/button-group'
import {
  Popover,
  PopoverContent,
  PopoverTitle,
  PopoverTrigger,
} from '@koharu/ui/components/popover'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@koharu/ui/components/select'
import { Separator } from '@koharu/ui/components/separator'

const quickStageIcons = {
  detection: Search,
  ocr: FileText,
  inpainting: Eraser,
} satisfies Record<ModelStage, LucideIcon>

const quickModelStages = ['detection', 'ocr', 'inpainting'] as const satisfies readonly ModelStage[]

export function CanvasCommandBar() {
  const { t } = useTranslation()
  const [savingStage, setSavingStage] = useState<ModelStage | null>(null)
  const page = usePage().data
  const selectedPages = useKoharuStore((state) => state.selectedPages)
  const jobs = useKoharuStore((state) => state.jobs)
  const preferences = useKoharuStore((state) => state.preferences)
  const running = Object.values(jobs).find((job) => job.state === 'running')

  const run = (selection: PipelineScope, stages: Stage[]) => {
    if (!page) return
    const scope: Scope =
      selection === 'project'
        ? { scope: 'project' }
        : selection === 'selected-pages'
          ? { scope: 'pages', value: selectedPages }
          : { scope: 'pages', value: [page.id] }
    const operation =
      stages.length === pipelineStages.length
        ? ({ operation: 'full' } as const)
        : stages.length === 1
          ? ({ operation: 'only', stage: stages[0]! } as const)
          : ({ operation: 'stages', stages } as const)
    void call(commands.process, scope, operation).catch(() => undefined)
  }

  const changeModel = (stage: ModelStage, name: ModelName) => {
    if (!preferences || savingStage) return
    if (stageModel(preferences.pipeline, stage).model === name) return

    setSavingStage(stage)
    const pipeline = replaceStage(preferences.pipeline, stage, defaultModel(name))
    void savePreferences(pipeline, preferences.providers, preferences.typesetting)
      .then(receivePreferences)
      .catch(() => undefined)
      .finally(() => setSavingStage(null))
  }

  return (
    <header className='flex h-10 shrink-0 items-center gap-2 border-b border-border/80 bg-[var(--surface-toolbar)] px-2.5'>
      <div className='min-w-0 flex-1' />
      <div className='flex items-center gap-0.5'>
        {quickModelStages.map((stage) => (
          <StageQuickControl
            key={stage}
            stage={stage}
            icon={quickStageIcons[stage]}
            model={preferences ? stageModel(preferences.pipeline, stage).model : null}
            runDisabled={!page || selectedPages.length === 0 || Boolean(running)}
            settingsDisabled={!preferences || Boolean(running) || Boolean(savingStage)}
            onRun={() => run('selected-pages', [stage])}
            onModelChange={(name) => changeModel(stage, name)}
          />
        ))}
        <TranslationQuickControl
          disabled={!page || selectedPages.length === 0 || Boolean(running)}
          onRun={() => run('selected-pages', ['translation'])}
        />
      </div>
      <Separator
        orientation='vertical'
        className='mx-0.5 data-vertical:h-4 data-vertical:self-center'
      />
      <InferenceControl disabled={!page || Boolean(running)} onRun={run} />
    </header>
  )
}

function TranslationQuickControl({ disabled, onRun }: { disabled: boolean; onRun: () => void }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const [saving, setSaving] = useState(false)
  const preferences = useKoharuStore((state) => state.preferences)
  const translation = preferences?.pipeline.translation ?? null

  const saveOutput = (draft: OutputDraft) => {
    if (!preferences || !translation || saving) return
    setSaving(true)
    const pipeline = {
      ...preferences.pipeline,
      translation: {
        ...translation,
        source_language: draft.sourceLanguage,
        target_language: draft.targetLanguage,
        instructions: draft.instructions || null,
      },
    }
    void savePreferences(pipeline, preferences.providers, preferences.typesetting)
      .then(receivePreferences)
      .catch(() => undefined)
      .finally(() => setSaving(false))
  }

  return (
    <ButtonGroup>
      <Button
        type='button'
        variant='ghost'
        size='sm'
        className='h-7 px-2 text-[11px] font-normal text-muted-foreground hover:bg-primary/10 hover:text-foreground'
        disabled={disabled}
        onClick={onRun}
      >
        <Languages data-icon='inline-start' />
        {t('inference.quickStage.translation')}
      </Button>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger
          render={
            <Button
              type='button'
              variant='ghost'
              size='sm'
              aria-label={t('inference.configureStage', {
                stage: t('inference.quickStage.translation'),
              })}
              className='h-7 w-5 px-0 text-muted-foreground hover:bg-primary/10 hover:text-foreground'
              disabled={disabled || !translation}
            />
          }
        >
          <ChevronDown className='size-3' />
        </PopoverTrigger>
        <PopoverContent
          align='start'
          sideOffset={4}
          className='w-72 min-w-0 gap-0 overflow-hidden rounded-xl border border-border/50 p-1 shadow-sm ring-0'
        >
          {translation && preferences && (
            <OutputPicker
              sourceLanguage={translation.source_language}
              targetLanguage={translation.target_language}
              instructions={translation.instructions}
              languages={preferences.languages}
              disabled={disabled}
              saving={saving}
              onBack={() => setOpen(false)}
              onChange={saveOutput}
            />
          )}
        </PopoverContent>
      </Popover>
    </ButtonGroup>
  )
}

function StageQuickControl({
  stage,
  icon: Icon,
  model,
  runDisabled,
  settingsDisabled,
  onRun,
  onModelChange,
}: {
  stage: ModelStage
  icon: LucideIcon
  model: ModelName | null
  runDisabled: boolean
  settingsDisabled: boolean
  onRun: () => void
  onModelChange: (model: ModelName) => void
}) {
  const { t } = useTranslation()
  const label = t(`inference.quickStage.${stage}`)
  const modelLabel = t(`inference.quickModelLabel.${stage}`)

  return (
    <ButtonGroup>
      <Button
        type='button'
        variant='ghost'
        size='sm'
        className='h-7 px-2 text-[11px] font-normal text-muted-foreground hover:bg-primary/10 hover:text-foreground'
        disabled={runDisabled}
        onClick={onRun}
      >
        <Icon data-icon='inline-start' />
        {label}
      </Button>
      <Popover>
        <PopoverTrigger
          render={
            <Button
              type='button'
              variant='ghost'
              size='sm'
              aria-label={t('inference.configureStage', { stage: label })}
              className='h-7 w-5 px-0 text-muted-foreground hover:bg-primary/10 hover:text-foreground'
              disabled={settingsDisabled}
            />
          }
        >
          <ChevronDown className='size-3' />
        </PopoverTrigger>
        <PopoverContent
          align='start'
          sideOffset={4}
          className='w-64 gap-1.5 rounded-xl border border-border/50 p-2 shadow-sm ring-0'
        >
          <PopoverTitle className='text-[11px]'>{modelLabel}</PopoverTitle>
          <Select
            value={model ?? undefined}
            disabled={settingsDisabled}
            items={Object.fromEntries(modelOptions[stage].map((name) => [name, modelNames[name]]))}
            onValueChange={(name) => {
              if (name) onModelChange(name as ModelName)
            }}
          >
            <SelectTrigger aria-label={modelLabel} className='w-full text-[11px]'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent align='start'>
              {modelOptions[stage].map((name) => (
                <SelectItem key={name} value={name}>
                  {modelNames[name]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </PopoverContent>
      </Popover>
    </ButtonGroup>
  )
}
