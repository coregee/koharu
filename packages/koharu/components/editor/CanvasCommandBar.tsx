'use client'

import { Eraser, FileText, Languages, Search, type LucideIcon } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { InferenceControl } from '@/components/editor/InferenceControl'
import { call } from '@/lib/backend'
import { usePage } from '@/lib/queries'
import { pipelineStages, useKoharuStore, type PipelineScope } from '@/lib/store'
import { commands, type Scope, type Stage } from '@koharu/bridge/protocol'
import { Button } from '@koharu/ui/components/button'
import { Separator } from '@koharu/ui/components/separator'

const quickStageIcons = {
  detection: Search,
  ocr: FileText,
  translation: Languages,
  inpainting: Eraser,
} satisfies Record<Stage, LucideIcon>

export function CanvasCommandBar() {
  const { t } = useTranslation()
  const page = usePage().data
  const selectedPages = useKoharuStore((state) => state.selectedPages)
  const jobs = useKoharuStore((state) => state.jobs)
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

  return (
    <header className='flex h-10 shrink-0 items-center gap-2 border-b border-border/80 bg-[var(--surface-toolbar)] px-2.5'>
      <div className='min-w-0 flex-1' />
      <div className='flex items-center gap-0.5'>
        {pipelineStages.map((stage) => {
          const Icon = quickStageIcons[stage]
          return (
            <Button
              key={stage}
              type='button'
              variant='ghost'
              size='sm'
              className='h-7 rounded-lg px-2 text-[11px] font-normal text-muted-foreground hover:bg-primary/10 hover:text-foreground'
              disabled={!page || Boolean(running)}
              onClick={() => run('page', [stage])}
            >
              <Icon data-icon='inline-start' />
              {t(`inference.quickStage.${stage}`)}
            </Button>
          )
        })}
      </div>
      <Separator
        orientation='vertical'
        className='mx-0.5 data-vertical:h-4 data-vertical:self-center'
      />
      <InferenceControl disabled={!page || Boolean(running)} onRun={run} />
    </header>
  )
}
