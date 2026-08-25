'use client'

import {
  Brush,
  Circle,
  Eraser,
  Hand,
  Minus,
  MousePointer2,
  PaintBucket,
  Pipette,
  Plus,
  Sparkles,
  ScanLine,
  Square,
  Type,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { ColorWell } from '@/components/controls/ColorWell'
import { usePage } from '@/lib/queries'
import {
  isBrushTool,
  MAX_BRUSH_DIAMETER,
  MAX_FILL_TOLERANCE,
  MIN_BRUSH_DIAMETER,
  MIN_FILL_TOLERANCE,
  useKoharuStore,
  type CanvasTool,
  type CensorSelectionTool,
  type RemoveSelectionTool,
} from '@/lib/store'
import { Button } from '@koharu/ui/components/button'
import {
  NumberField,
  NumberFieldDecrement,
  NumberFieldGroup,
  NumberFieldIncrement,
  NumberFieldInput,
} from '@koharu/ui/components/number-field'
import {
  Popover,
  PopoverContent,
  PopoverTitle,
  PopoverTrigger,
} from '@koharu/ui/components/popover'
import { Slider } from '@koharu/ui/components/slider'
import { Tooltip, TooltipContent, TooltipTrigger } from '@koharu/ui/components/tooltip'

const tools = [
  ['select', MousePointer2],
  ['text', Type],
  ['draw', Brush],
  ['fill', PaintBucket],
  ['eraser', Eraser],
  ['color_picker', Pipette],
  ['remove', Sparkles],
  ['censor', ScanLine],
  ['pan', Hand],
] as const satisfies ReadonlyArray<readonly [CanvasTool, typeof MousePointer2]>

export function ToolBar() {
  const { t } = useTranslation()
  const page = usePage().data
  const active = useKoharuStore((state) => state.tool)
  const brush = useKoharuStore((state) => state.brush)
  const fillTolerance = useKoharuStore((state) => state.fillTolerance)
  const removeSelectionTool = useKoharuStore((state) => state.removeSelectionTool)
  const censorSelectionTool = useKoharuStore((state) => state.censorSelectionTool)
  const setTool = useKoharuStore((state) => state.setTool)
  const setBrush = useKoharuStore((state) => state.setBrush)
  const setFillTolerance = useKoharuStore((state) => state.setFillTolerance)
  const setRemoveSelectionTool = useKoharuStore((state) => state.setRemoveSelectionTool)
  const setCensorSelectionTool = useKoharuStore((state) => state.setCensorSelectionTool)
  const shortcuts = useKoharuStore((state) => state.shortcuts)
  const hasBrush = isBrushTool(active) && (active !== 'censor' || censorSelectionTool === 'brush')
  const hasControls = hasBrush || active === 'fill' || active === 'remove' || active === 'censor'

  return (
    <aside className='absolute top-3 left-3 z-20 flex w-11 flex-col rounded-2xl border border-border bg-[var(--surface-floating)] p-1 shadow-[var(--shadow-toolrail)]'>
      <div className='flex flex-col items-center py-0.5'>
        {tools.map(([tool, Icon], index) => (
          <div key={tool} className='contents'>
            {index === tools.length - 1 && <span className='my-1 h-px w-5 bg-border' />}
            <Tooltip>
              <TooltipTrigger
                render={
                  <Button
                    type='button'
                    variant='ghost'
                    size='icon'
                    disabled={!page}
                    aria-label={t(`tools.${tool}`)}
                    data-active={active === tool}
                    className='relative text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground disabled:opacity-30 data-[active=true]:bg-accent data-[active=true]:text-accent-foreground'
                    onClick={() => setTool(tool)}
                  />
                }
              >
                <Icon className='size-4' />
              </TooltipTrigger>
              <TooltipContent side='right'>
                {t(`tools.${tool}`)}
                <span className='ml-2 opacity-60'>{shortcuts[tool].toUpperCase()}</span>
              </TooltipContent>
            </Tooltip>
          </div>
        ))}
      </div>

      {hasControls && (
        <div className='flex flex-col items-center gap-1 border-t border-border/80 py-1.5'>
          {(active === 'draw' || active === 'fill') && (
            <ColorWell value={brush.color} onChange={(color) => setBrush({ ...brush, color })} />
          )}
          {active === 'remove' && (
            <RemoveSelectionTools value={removeSelectionTool} onChange={setRemoveSelectionTool} />
          )}
          {active === 'censor' && (
            <CensorSelectionTools value={censorSelectionTool} onChange={setCensorSelectionTool} />
          )}
          {(hasBrush || (active === 'remove' && removeSelectionTool === 'pen')) && (
            <BrushSize
              value={brush.diameter}
              onChange={(diameter) => setBrush({ ...brush, diameter })}
            />
          )}
          {active === 'fill' && <FillTolerance value={fillTolerance} onChange={setFillTolerance} />}
        </div>
      )}
    </aside>
  )
}

const censorSelectionTools = [
  ['brush', Brush],
  ['circle', Circle],
  ['rectangle', Square],
] as const satisfies ReadonlyArray<readonly [CensorSelectionTool, typeof Brush]>

function CensorSelectionTools({
  value,
  onChange,
}: {
  value: CensorSelectionTool
  onChange: (value: CensorSelectionTool) => void
}) {
  const { t } = useTranslation()
  return censorSelectionTools.map(([tool, Icon]) => (
    <Tooltip key={tool}>
      <TooltipTrigger
        render={
          <Button
            type='button'
            variant='ghost'
            size='icon'
            aria-label={t(`tools.censorSelection.${tool}`)}
            data-active={value === tool}
            className='size-8 rounded-xl text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground data-[active=true]:bg-accent data-[active=true]:text-accent-foreground'
            onClick={() => onChange(tool)}
          />
        }
      >
        <Icon className='size-4' />
      </TooltipTrigger>
      <TooltipContent side='right'>{t(`tools.censorSelection.${tool}`)}</TooltipContent>
    </Tooltip>
  ))
}

const removeSelectionTools = [
  ['pen', Brush],
  ['circle', Circle],
  ['square', Square],
] as const satisfies ReadonlyArray<readonly [RemoveSelectionTool, typeof Brush]>

function RemoveSelectionTools({
  value,
  onChange,
}: {
  value: RemoveSelectionTool
  onChange: (value: RemoveSelectionTool) => void
}) {
  const { t } = useTranslation()
  return removeSelectionTools.map(([tool, Icon]) => (
    <Tooltip key={tool}>
      <TooltipTrigger
        render={
          <Button
            type='button'
            variant='ghost'
            size='icon'
            aria-label={t(`tools.removeSelection.${tool}`)}
            data-active={value === tool}
            className='size-8 rounded-xl text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground data-[active=true]:bg-accent data-[active=true]:text-accent-foreground'
            onClick={() => onChange(tool)}
          />
        }
      >
        <Icon className='size-4' />
      </TooltipTrigger>
      <TooltipContent side='right'>{t(`tools.removeSelection.${tool}`)}</TooltipContent>
    </Tooltip>
  ))
}

function BrushSize({ value, onChange }: { value: number; onChange: (value: number) => void }) {
  const { t } = useTranslation()
  const roundedValue = Math.round(value)

  return (
    <Popover>
      <Tooltip>
        <PopoverTrigger
          render={
            <TooltipTrigger
              render={
                <Button
                  type='button'
                  variant='ghost'
                  size='icon'
                  aria-label={t('tools.brushSizePixels', { size: roundedValue })}
                  className='size-8 rounded-xl font-sans text-[11px] leading-none font-medium tracking-[-0.02em] text-muted-foreground tabular-nums hover:bg-foreground/[0.06] hover:text-foreground'
                />
              }
            >
              {roundedValue}
            </TooltipTrigger>
          }
        />
        <TooltipContent side='right'>
          {t('tools.brushSizeShort', { size: roundedValue })}
        </TooltipContent>
      </Tooltip>
      <PopoverContent
        side='right'
        align='center'
        sideOffset={8}
        className='w-48 gap-2.5 rounded-xl p-2.5'
      >
        <div className='flex items-center justify-between gap-3'>
          <PopoverTitle className='text-[11px]'>{t('tools.brushSize')}</PopoverTitle>
          <NumberField
            min={MIN_BRUSH_DIAMETER}
            max={MAX_BRUSH_DIAMETER}
            step={1}
            value={roundedValue}
            className='w-20'
            onValueChange={(next) => {
              if (next !== null) onChange(clamp(next, MIN_BRUSH_DIAMETER, MAX_BRUSH_DIAMETER))
            }}
          >
            <NumberFieldGroup className='h-7'>
              <NumberFieldDecrement aria-label={t('tools.decreaseBrushSize')}>
                <Minus />
              </NumberFieldDecrement>
              <NumberFieldInput aria-label={t('tools.brushSizeInput')} />
              <NumberFieldIncrement aria-label={t('tools.increaseBrushSize')}>
                <Plus />
              </NumberFieldIncrement>
            </NumberFieldGroup>
          </NumberField>
        </div>
        <Slider
          aria-label={t('tools.brushSize')}
          min={MIN_BRUSH_DIAMETER}
          max={MAX_BRUSH_DIAMETER}
          step={1}
          value={value}
          className='py-1 [&_[data-slot=slider-thumb]]:size-2.5'
          onValueChange={onChange}
        />
      </PopoverContent>
    </Popover>
  )
}

function FillTolerance({ value, onChange }: { value: number; onChange: (value: number) => void }) {
  const { t } = useTranslation()
  const roundedValue = Math.round(value)

  return (
    <Popover>
      <Tooltip>
        <PopoverTrigger
          render={
            <TooltipTrigger
              render={
                <Button
                  type='button'
                  variant='ghost'
                  size='icon'
                  aria-label={t('tools.fillToleranceValue', { value: roundedValue })}
                  className='size-8 rounded-xl font-sans text-[11px] leading-none font-medium tracking-[-0.02em] text-muted-foreground tabular-nums hover:bg-foreground/[0.06] hover:text-foreground'
                />
              }
            >
              {roundedValue}
            </TooltipTrigger>
          }
        />
        <TooltipContent side='right'>
          {t('tools.fillToleranceValue', { value: roundedValue })}
        </TooltipContent>
      </Tooltip>
      <PopoverContent
        side='right'
        align='center'
        sideOffset={8}
        className='w-48 gap-2.5 rounded-xl p-2.5'
      >
        <div className='flex items-center justify-between gap-3'>
          <PopoverTitle className='text-[11px]'>{t('tools.fillTolerance')}</PopoverTitle>
          <NumberField
            min={MIN_FILL_TOLERANCE}
            max={MAX_FILL_TOLERANCE}
            step={1}
            value={roundedValue}
            className='w-20'
            onValueChange={(next) => {
              if (next !== null) onChange(clamp(next, MIN_FILL_TOLERANCE, MAX_FILL_TOLERANCE))
            }}
          >
            <NumberFieldGroup className='h-7'>
              <NumberFieldDecrement aria-label={t('tools.decreaseFillTolerance')}>
                <Minus />
              </NumberFieldDecrement>
              <NumberFieldInput aria-label={t('tools.fillTolerance')} />
              <NumberFieldIncrement aria-label={t('tools.increaseFillTolerance')}>
                <Plus />
              </NumberFieldIncrement>
            </NumberFieldGroup>
          </NumberField>
        </div>
        <Slider
          aria-label={t('tools.fillTolerance')}
          min={MIN_FILL_TOLERANCE}
          max={MAX_FILL_TOLERANCE}
          step={1}
          value={value}
          className='py-1 [&_[data-slot=slider-thumb]]:size-2.5'
          onValueChange={onChange}
        />
      </PopoverContent>
    </Popover>
  )
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Number.isFinite(value) ? value : min))
}
