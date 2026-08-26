'use client'

import {
  Brush,
  Circle,
  Eraser,
  Hand,
  Minus,
  MousePointer2,
  PaintBucket,
  Palette,
  Pipette,
  Plus,
  ScanLine,
  Sparkles,
  Square,
  Type,
  WandSparkles,
} from 'lucide-react'
import type { ReactNode } from 'react'
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
  type ToolMode,
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

const modes = [
  ['removal', Sparkles],
  ['repair', WandSparkles],
  ['decensor', ScanLine],
  ['paint', Palette],
] as const satisfies ReadonlyArray<readonly [ToolMode, typeof Brush]>

const contextualTools = [
  ['brush', Brush],
  ['eraser', Eraser],
  ['oval', Circle],
  ['rectangle', Square],
  ['eyedropper', Pipette],
  ['fill', PaintBucket],
] as const satisfies ReadonlyArray<readonly [CanvasTool, typeof Brush]>

const supplementalTools = [
  ['select', MousePointer2],
  ['text', Type],
  ['pan', Hand],
] as const satisfies ReadonlyArray<readonly [CanvasTool, typeof Brush]>

export function ToolBar() {
  const { t } = useTranslation()
  const page = usePage().data
  const mode = useKoharuStore((state) => state.toolMode)
  const tool = useKoharuStore((state) => state.tool)
  const brush = useKoharuStore((state) => state.brush)
  const fillTolerance = useKoharuStore((state) => state.fillTolerance)
  const setToolMode = useKoharuStore((state) => state.setToolMode)
  const setTool = useKoharuStore((state) => state.setTool)
  const setBrush = useKoharuStore((state) => state.setBrush)
  const setFillTolerance = useKoharuStore((state) => state.setFillTolerance)
  const shortcuts = useKoharuStore((state) => state.shortcuts)
  const showsColor =
    mode === 'paint' &&
    (tool === 'brush' || tool === 'oval' || tool === 'rectangle' || tool === 'fill')
  const showsSize = isBrushTool(tool)
  const showsSettings = showsColor || showsSize || tool === 'fill'

  return (
    <aside
      aria-label={t('tools.toolbar')}
      className='absolute top-3 left-3 z-20 flex w-11 flex-col rounded-2xl border border-border bg-[var(--surface-floating)] p-1 shadow-[var(--shadow-toolrail)]'
    >
      <ToolbarSection label={t('tools.sections.mode')}>
        {modes.map(([candidate, Icon]) => (
          <ToolbarButton
            key={candidate}
            Icon={Icon}
            label={t(`tools.modes.${candidate}`)}
            shortcut={shortcuts[candidate]}
            active={mode === candidate}
            disabled={!page}
            onClick={() => setToolMode(candidate)}
          />
        ))}
      </ToolbarSection>

      <ToolbarSection label={t('tools.sections.tool')} divided>
        {contextualTools.map(([candidate, Icon]) => (
          <ToolbarButton
            key={candidate}
            Icon={Icon}
            label={t(`tools.${candidate}`)}
            shortcut={shortcuts[candidate]}
            active={tool === candidate}
            disabled={!page}
            onClick={() => setTool(candidate)}
          />
        ))}
      </ToolbarSection>

      {showsSettings && (
        <ToolbarSection label={t('tools.sections.settings')} divided>
          {showsColor && (
            <ColorWell value={brush.color} onChange={(color) => setBrush({ ...brush, color })} />
          )}
          {showsSize && (
            <BrushSize
              value={brush.diameter}
              onChange={(diameter) => setBrush({ ...brush, diameter })}
            />
          )}
          {tool === 'fill' && <FillTolerance value={fillTolerance} onChange={setFillTolerance} />}
        </ToolbarSection>
      )}

      <ToolbarSection label={t('tools.sections.supplemental')} divided>
        {supplementalTools.map(([candidate, Icon]) => (
          <ToolbarButton
            key={candidate}
            Icon={Icon}
            label={t(`tools.${candidate}`)}
            shortcut={shortcuts[candidate]}
            active={tool === candidate}
            disabled={!page}
            onClick={() => setTool(candidate)}
          />
        ))}
      </ToolbarSection>
    </aside>
  )
}

function ToolbarSection({
  label,
  divided = false,
  children,
}: {
  label: string
  divided?: boolean
  children: ReactNode
}) {
  return (
    <div
      role='group'
      aria-label={label}
      className={`flex flex-col items-center gap-0.5 py-1 ${divided ? 'border-t border-border/80' : ''}`}
    >
      {children}
    </div>
  )
}

function ToolbarButton({
  Icon,
  label,
  shortcut,
  active,
  disabled,
  onClick,
}: {
  Icon: typeof Brush
  label: string
  shortcut: string
  active: boolean
  disabled: boolean
  onClick: () => void
}) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            type='button'
            variant='ghost'
            size='icon'
            disabled={disabled}
            aria-label={label}
            data-active={active}
            className='relative size-8 rounded-xl text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground disabled:opacity-30 data-[active=true]:bg-accent data-[active=true]:text-accent-foreground'
            onClick={onClick}
          />
        }
      >
        <Icon className='size-4' />
      </TooltipTrigger>
      <TooltipContent side='right'>
        {label}
        <span className='ml-2 opacity-60'>{shortcut.toUpperCase()}</span>
      </TooltipContent>
    </Tooltip>
  )
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
