'use client'

import type { TFunction } from 'i18next'
import {
  AlignCenter,
  AlignLeft,
  AlignRight,
  ArrowDown,
  ArrowUp,
  Brush,
  Circle,
  ChevronDown,
  ChevronRight,
  Eye,
  EyeOff,
  Folder,
  GripVertical,
  Image as ImageIcon,
  Layers3,
  Languages,
  Lock,
  MessageCircle,
  Minus,
  Plus,
  ScanText,
  Square,
  Trash2,
  Type,
  Unlock,
} from 'lucide-react'
import { type DragEvent, type MouseEvent, useEffect, useId, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { ColorWell } from '@/components/controls/ColorWell'
import { CommitTextarea } from '@/components/controls/CommitTextarea'
import { FontPicker } from '@/components/controls/FontPicker'
import { call } from '@/lib/backend'
import {
  expandLayerSelection,
  effectiveLayerLocked,
  isGroupLayer,
  isTextLayer,
  layerChildren,
} from '@/lib/document'
import {
  pageKey,
  pagesKey,
  projectKey,
  queryClient,
  refresh,
  useFonts,
  usePage,
} from '@/lib/queries'
import { useKoharuStore } from '@/lib/store'
import { previewCanvasOpacity } from '@koharu/bridge/canvas'
import {
  commands,
  type AnalysisRegion,
  type EntityId,
  type FontFamily,
  type FontStyle,
  type Layer,
  type RegionShapeKind,
  type TextClassification,
  type TextAlignment,
  type TextStyleRange,
  type Typography,
  type WritingMode,
} from '@koharu/bridge/protocol'
import { Button } from '@koharu/ui/components/button'
import { Checkbox } from '@koharu/ui/components/checkbox'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '@koharu/ui/components/dropdown-menu'
import {
  NumberField,
  NumberFieldDecrement,
  NumberFieldGroup,
  NumberFieldIncrement,
  NumberFieldInput,
} from '@koharu/ui/components/number-field'
import { ScrollArea } from '@koharu/ui/components/scroll-area'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@koharu/ui/components/select'
import { Slider } from '@koharu/ui/components/slider'
import { Tooltip, TooltipContent, TooltipTrigger } from '@koharu/ui/components/tooltip'

const defaultFont: FontFamily = {
  name: 'CCWildWords',
  metadata: {
    primary_script: 'latn',
    scripts: ['latn'],
    languages: ['en'],
    category: 'HANDWRITING',
    classifications: ['comic', 'dialogue'],
    use_cases: ['comic-dialogue', 'word-balloons'],
  },
  sources: ['bundled'],
  faces: [
    {
      postscript_name: 'CCWildWords-Regular',
      weight: 400,
      weight_range: null,
      style: 'normal',
    },
  ],
}

const defaultTypography: Typography = {
  preferred_font: null,
  font_weight: 400,
  font_style: 'normal',
  size: null,
  auto_fit: true,
  color: [0, 0, 0, 255],
  stroke_color: [255, 255, 255, 255],
  stroke_width: 0,
  paragraph_spacing: null,
  style_ranges: [],
  alignment: null,
  writing_mode: null,
}

export function Inspector() {
  const { t } = useTranslation()
  return (
    <aside className='flex h-full min-h-0 flex-col bg-[var(--surface-panel)]'>
      <div className='flex h-8 shrink-0 items-center gap-1.5 border-b border-border/80 px-2'>
        <Type className='size-3 text-primary' />
        <h2 className='text-[10px] font-semibold'>{t('inspector.type')}</h2>
      </div>

      <section className='h-48 min-w-0 shrink-0 overflow-hidden border-b'>
        <TypeInspector />
      </section>

      <section className='flex min-h-0 flex-1 flex-col'>
        <LayersInspector />
      </section>
    </aside>
  )
}

function TypeInspector() {
  const { t } = useTranslation()
  const borderWidthId = useId()
  const paragraphSpacingId = useId()
  const page = usePage().data
  const selectedIds = useKoharuStore((state) => state.selectedLayers)
  const textSelection = useKoharuStore((state) => state.textSelection)
  const availableFonts = useFonts().data
  const expandedSelection = page ? expandLayerSelection(page.layers, selectedIds) : []
  const selected =
    page?.layers.filter(isTextLayer).filter((layer) => expandedSelection.includes(layer.id)) ?? []
  const current = selected[0]
  const [draft, setDraft] = useState<{
    layer: EntityId
    typography: Typography
  } | null>(null)
  const updateSequence = useRef(0)

  useEffect(() => setDraft(null), [current?.id])

  const submit = (updates: { layer: EntityId; typography: Typography }[]) => {
    const optimistic = current && updates.find(({ layer }) => layer === current.id)
    if (optimistic) setDraft(optimistic)
    const sequence = ++updateSequence.current
    void call(commands.setTypography, updates)
      .then(() => refresh(projectKey, pageKey))
      .catch(() => undefined)
      .finally(() => {
        if (updateSequence.current === sequence) setDraft(null)
      })
  }

  const apply = (update: (value: Typography) => Typography) => {
    if (!selected.length) return
    submit(
      selected.map((layer) => ({
        layer: layer.id,
        typography: update(layer.typography ?? defaultTypography),
      })),
    )
  }

  const typography =
    current && draft?.layer === current.id
      ? draft.typography
      : (current?.typography ?? defaultTypography)
  const disabled = !current
  const families = useMemo(() => {
    const available = availableFonts ?? []
    return available.some(
      (family) => normalizeFontName(family.name) === normalizeFontName(defaultFont.name),
    )
      ? available
      : [...available, defaultFont]
  }, [availableFonts])
  const size = Math.round((typography.size ?? 24) * 100) / 100
  const weight = typography.font_weight ?? 400
  const selectedFamily = findFontFamily(families, typography.preferred_font ?? defaultFont.name)
  const styles = usableFontStyles(selectedFamily)
  const style = styles.includes(typography.font_style ?? 'normal')
    ? (typography.font_style ?? 'normal')
    : (styles[0] ?? 'normal')
  const weights = usableFontWeights(selectedFamily, style)
  const strokeWidth = typography.stroke_width ?? 0
  const strokeColor = typography.stroke_color ?? defaultTypography.stroke_color!
  const strokeEnabled = strokeWidth > 0 && strokeColor[3] > 0
  const displayedStrokeWidth = strokeWidth > 0 ? strokeWidth : 1.5
  const paragraphSpacing = typography.paragraph_spacing ?? 0
  const activeRange =
    current &&
    textSelection?.layer === current.id &&
    textSelection.content === 'translation' &&
    textSelection.start < textSelection.end
      ? { start: textSelection.start, end: textSelection.end }
      : null
  const applyRange = (
    property: 'color' | 'stroke_color',
    color: [number, number, number, number],
    update: (value: Typography) => Typography = (value) => value,
  ) => {
    if (!current || !activeRange) return false
    const next = update(typography)
    submit([
      {
        layer: current.id,
        typography: {
          ...next,
          style_ranges: setStyleRange(
            next.style_ranges,
            activeRange.start,
            activeRange.end,
            property,
            color,
          ),
        },
      },
    ])
    return true
  }
  const writingMode = typography.writing_mode ?? 'Horizontal'
  const writingModeChoice = typography.writing_mode ?? 'Auto'
  const effectiveAlignment =
    typography.alignment ?? (writingMode === 'Vertical' ? 'Start' : 'Center')

  return (
    <div className='min-w-0 p-2' data-testid='type-inspector' aria-disabled={disabled}>
      <div className='grid min-w-0 gap-1.5'>
        <div className='grid min-w-0 grid-cols-[minmax(0,1fr)_2.5rem] gap-1.5'>
          <InspectorField label={t('inspector.font')}>
            <FontPicker
              value={typography.preferred_font ?? defaultFont.name}
              families={families}
              disabled={disabled}
              size='sm'
              onChange={(preferred_font) => {
                const family = findFontFamily(families, preferred_font)
                const nextStyles = usableFontStyles(family)
                const fontStyle = nextStyles.includes(style) ? style : (nextStyles[0] ?? 'normal')
                const nextWeights = usableFontWeights(family, fontStyle)
                const fontWeight = nearestFontWeight(nextWeights, weight)
                apply((value) => ({
                  ...value,
                  preferred_font,
                  font_weight: fontWeight,
                  font_style: fontStyle,
                }))
              }}
            />
          </InspectorField>
          <InspectorField label={t('inspector.color')}>
            <ColorWell
              label={t('inspector.textColor')}
              size='sm'
              disabled={disabled}
              value={rgbaToHex(typography.color ?? defaultTypography.color!)}
              onChange={(color) => {
                const rgba = hexToRgba(color)
                if (applyRange('color', rgba)) return
                apply((value) => ({
                  ...value,
                  color: rgba,
                  style_ranges: clearStyleProperty(value.style_ranges, 'color'),
                }))
              }}
            />
          </InspectorField>
        </div>

        <div className='grid min-w-0 grid-cols-[minmax(0,1fr)_4.25rem_4.75rem] gap-1.5'>
          <InspectorField label={t('inspector.size')}>
            <FontSizeField
              disabled={disabled}
              value={size}
              autoFit={typography.auto_fit}
              onChange={(next) => apply((value) => ({ ...value, size: next, auto_fit: false }))}
              onAutoFit={() =>
                apply((value) => ({
                  ...value,
                  size: value.size ?? size,
                  auto_fit: true,
                }))
              }
            />
          </InspectorField>
          <InspectorField label={t('inspector.weight')}>
            <Select
              disabled={disabled}
              value={String(weight)}
              onValueChange={(font_weight) =>
                apply((value) => ({
                  ...value,
                  font_weight: Number(font_weight),
                }))
              }
            >
              <SelectTrigger
                size='sm'
                aria-label={t('inspector.fontWeight')}
                className='w-full min-w-0'
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {weights.map((value) => (
                  <SelectItem key={value} value={String(value)}>
                    {value}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </InspectorField>
          <InspectorField label={t('inspector.style')}>
            <Select
              disabled={disabled}
              value={style}
              onValueChange={(font_style) => {
                const nextStyle = font_style as FontStyle
                const nextWeights = usableFontWeights(selectedFamily, nextStyle)
                apply((value) => ({
                  ...value,
                  font_style: nextStyle,
                  font_weight: nearestFontWeight(nextWeights, weight),
                }))
              }}
            >
              <SelectTrigger
                size='sm'
                aria-label={t('inspector.fontStyle')}
                className='w-full min-w-0'
              >
                <SelectValue>{t(`inspector.fontStyles.${style}`)}</SelectValue>
              </SelectTrigger>
              <SelectContent align='end'>
                {styles.map((value) => (
                  <SelectItem key={value} value={value}>
                    {t(`inspector.fontStyles.${value}`)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </InspectorField>
        </div>

        <div className='grid min-w-0 grid-cols-[minmax(0,1fr)_minmax(6.5rem,1fr)] gap-1.5'>
          <InspectorField label={t('inspector.alignment')}>
            <div className='grid h-6 grid-cols-3 rounded-md border border-input bg-background p-px'>
              {(
                [
                  [
                    'Start',
                    AlignLeft,
                    writingMode === 'Vertical' ? t('inspector.alignTop') : t('inspector.alignLeft'),
                  ],
                  [
                    'Center',
                    AlignCenter,
                    writingMode === 'Vertical'
                      ? t('inspector.alignMiddle')
                      : t('inspector.alignCenter'),
                  ],
                  [
                    'End',
                    AlignRight,
                    writingMode === 'Vertical'
                      ? t('inspector.alignBottom')
                      : t('inspector.alignRight'),
                  ],
                ] as const
              ).map(([alignment, Icon, label]) => (
                <button
                  key={alignment}
                  type='button'
                  aria-label={label}
                  aria-pressed={effectiveAlignment === alignment}
                  disabled={disabled}
                  data-active={effectiveAlignment === alignment}
                  className='grid place-items-center rounded-[4px] text-muted-foreground hover:text-foreground disabled:cursor-not-allowed disabled:opacity-40 data-[active=true]:bg-primary data-[active=true]:text-primary-foreground data-[active=true]:hover:bg-primary/90'
                  onClick={() =>
                    apply((value) => ({
                      ...value,
                      alignment: alignment as TextAlignment,
                    }))
                  }
                >
                  <Icon className='size-3' />
                </button>
              ))}
            </div>
          </InspectorField>
          <InspectorField label={t('inspector.direction')}>
            <Select
              disabled={disabled}
              value={writingModeChoice}
              onValueChange={(writing_mode) =>
                apply((value) => ({
                  ...value,
                  writing_mode: writing_mode === 'Auto' ? null : (writing_mode as WritingMode),
                }))
              }
            >
              <SelectTrigger size='sm' aria-label={t('inspector.textDirection')} className='w-full'>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='Auto'>{t('inspector.auto')}</SelectItem>
                <SelectItem value='Horizontal'>{t('inspector.horizontal')}</SelectItem>
                <SelectItem value='Vertical'>{t('inspector.vertical')}</SelectItem>
              </SelectContent>
            </Select>
          </InspectorField>
        </div>

        <div className='grid min-w-0 grid-cols-[2.75rem_minmax(4.75rem,1fr)_minmax(4.75rem,1fr)] items-end gap-1.5'>
          <InspectorField label={t('inspector.border')}>
            <ColorWell
              label={t('inspector.borderColor')}
              size='sm'
              disabled={disabled}
              allowTransparent
              value={strokeEnabled ? rgbaToHex(strokeColor) : null}
              onChange={(stroke_color) => {
                const rgba =
                  stroke_color === null ? ([0, 0, 0, 0] as const) : hexToRgba(stroke_color)
                if (
                  applyRange('stroke_color', [...rgba], (value) => {
                    const width = value.stroke_width ?? 0
                    const base = value.stroke_color ?? defaultTypography.stroke_color!
                    return {
                      ...value,
                      stroke_color: width > 0 ? base : [base[0], base[1], base[2], 0],
                      stroke_width: width > 0 ? width : 1.5,
                    }
                  })
                )
                  return
                apply((value) => {
                  const width = value.stroke_width ?? 0
                  if (stroke_color !== null) {
                    return {
                      ...value,
                      stroke_color: hexToRgba(stroke_color),
                      stroke_width: width > 0 ? width : 1.5,
                      style_ranges: clearStyleProperty(value.style_ranges, 'stroke_color'),
                    }
                  }
                  const [red, green, blue] = value.stroke_color ?? defaultTypography.stroke_color!
                  return {
                    ...value,
                    stroke_color: [red, green, blue, 0],
                    stroke_width: width > 0 ? width : 1.5,
                    style_ranges: clearStyleProperty(value.style_ranges, 'stroke_color'),
                  }
                })
              }}
            />
          </InspectorField>
          <InspectorField label={t('inspector.width')}>
            <NumberField
              id={borderWidthId}
              name='border-width'
              className='min-w-0'
              disabled={disabled}
              value={displayedStrokeWidth}
              min={0.5}
              max={32}
              step={0.5}
              onValueChange={(next) => {
                if (next !== null && next >= 0.5 && next <= 32) {
                  apply((value) => ({ ...value, stroke_width: next }))
                }
              }}
            >
              <NumberFieldGroup>
                <NumberFieldDecrement aria-label={t('inspector.decreaseBorderWidth')}>
                  <Minus />
                </NumberFieldDecrement>
                <NumberFieldInput aria-label={t('inspector.borderWidth')} />
                <NumberFieldIncrement aria-label={t('inspector.increaseBorderWidth')}>
                  <Plus />
                </NumberFieldIncrement>
              </NumberFieldGroup>
            </NumberField>
          </InspectorField>
          <InspectorField label={t('inspector.paragraphSpacing')}>
            <NumberField
              id={paragraphSpacingId}
              name='paragraph-spacing'
              className='min-w-0'
              disabled={disabled}
              value={paragraphSpacing}
              min={0}
              max={256}
              step={1}
              onValueChange={(next) => {
                if (next !== null && next >= 0 && next <= 256) {
                  apply((value) => ({ ...value, paragraph_spacing: next }))
                }
              }}
            >
              <NumberFieldGroup>
                <NumberFieldDecrement aria-label={t('inspector.decreaseParagraphSpacing')}>
                  <Minus />
                </NumberFieldDecrement>
                <NumberFieldInput aria-label={t('inspector.paragraphSpacing')} />
                <NumberFieldIncrement aria-label={t('inspector.increaseParagraphSpacing')}>
                  <Plus />
                </NumberFieldIncrement>
              </NumberFieldGroup>
            </NumberField>
          </InspectorField>
        </div>
      </div>
    </div>
  )
}

function setStyleRange(
  ranges: TextStyleRange[],
  start: number,
  end: number,
  property: 'color' | 'stroke_color',
  color: [number, number, number, number],
): TextStyleRange[] {
  const result: TextStyleRange[] = []
  let cursor = start
  for (const range of ranges) {
    if (range.end <= start || range.start >= end) {
      result.push(range)
      continue
    }
    if (range.start < start) result.push({ ...range, end: start })
    const overlapStart = Math.max(range.start, start)
    const overlapEnd = Math.min(range.end, end)
    if (cursor < overlapStart) {
      result.push({
        start: cursor,
        end: overlapStart,
        color: null,
        stroke_color: null,
        [property]: color,
      })
    }
    result.push({ ...range, start: overlapStart, end: overlapEnd, [property]: color })
    cursor = overlapEnd
    if (range.end > end) result.push({ ...range, start: end })
  }
  if (cursor < end) {
    result.push({ start: cursor, end, color: null, stroke_color: null, [property]: color })
  }
  return mergeStyleRanges(result)
}

function clearStyleProperty(
  ranges: TextStyleRange[],
  property: 'color' | 'stroke_color',
): TextStyleRange[] {
  return mergeStyleRanges(
    ranges
      .map((range) => ({ ...range, [property]: null }))
      .filter((range) => range.color !== null || range.stroke_color !== null),
  )
}

function mergeStyleRanges(ranges: TextStyleRange[]): TextStyleRange[] {
  const sorted = ranges.toSorted((left, right) => left.start - right.start)
  const merged: TextStyleRange[] = []
  for (const range of sorted) {
    if (range.start >= range.end || (range.color === null && range.stroke_color === null)) continue
    const previous = merged.at(-1)
    if (
      previous &&
      previous.end === range.start &&
      rgbaEqual(previous.color, range.color) &&
      rgbaEqual(previous.stroke_color, range.stroke_color)
    ) {
      previous.end = range.end
    } else {
      merged.push({ ...range })
    }
  }
  return merged
}

function rgbaEqual(
  left: [number, number, number, number] | null,
  right: [number, number, number, number] | null,
): boolean {
  return (
    left === right || Boolean(left && right && left.every((value, index) => value === right[index]))
  )
}

function findFontFamily(families: FontFamily[], name: string): FontFamily | undefined {
  return families.find((family) => normalizeFontName(family.name) === normalizeFontName(name))
}

function usableFontStyles(family: FontFamily | undefined): FontStyle[] {
  if (!family) return ['normal']
  const styles = new Set(family.faces.map((font) => font.style))
  const available = (['normal', 'italic', 'oblique'] satisfies FontStyle[]).filter((style) =>
    styles.has(style),
  )
  return available.length ? available : ['normal']
}

function usableFontWeights(family: FontFamily | undefined, style: FontStyle): number[] {
  if (!family) return [400]
  const styled = family.faces.filter((font) => font.style === style)
  const faces = styled.length ? styled : family.faces
  const weights = new Set(faces.map((font) => font.weight))
  for (const face of faces) {
    if (!face.weight_range) continue
    weights.add(face.weight_range.minimum)
    weights.add(face.weight_range.maximum)
    for (let weight = 100; weight <= 900; weight += 100) {
      if (weight >= face.weight_range.minimum && weight <= face.weight_range.maximum) {
        weights.add(weight)
      }
    }
  }
  return [...weights].sort((left, right) => left - right)
}

function nearestFontWeight(weights: number[], target: number): number {
  return weights.reduce((nearest, weight) =>
    Math.abs(weight - target) < Math.abs(nearest - target) ? weight : nearest,
  )
}

function normalizeFontName(value: string): string {
  return value.normalize('NFKC').trim().replace(/\s+/g, ' ').toLowerCase()
}

function displayedLayers(layers: Layer[], page: EntityId, collapsedGroups: ReadonlySet<EntityId>) {
  const indexes = new Map(layers.map((layer, index) => [layer.id, index]))
  const rows: { layer: Layer; index: number; depth: number }[] = []
  const append = (layer: Layer, depth: number) => {
    rows.push({ layer, index: indexes.get(layer.id) ?? 0, depth })
    if (!isGroupLayer(layer) || collapsedGroups.has(layer.id)) return
    const children = layerChildren(layers, layer.id)
    const ordered = layer.role !== null ? children : [...children].reverse()
    for (const child of ordered) append(child, depth + 1)
  }
  const roots = layers.filter((layer) => (layer.parent ?? page) === page).reverse()
  for (const layer of roots) append(layer, 0)
  return rows
}

function LayersInspector() {
  const { t } = useTranslation()
  const page = usePage().data
  const selected = useKoharuStore((state) => state.selectedLayers)
  const selectLayers = useKoharuStore((state) => state.selectLayers)
  const [expandedLayer, setExpandedLayer] = useState<EntityId | null>(
    selected.length === 1 ? (selected[0] ?? null) : null,
  )
  const [collapsedGroups, setCollapsedGroups] = useState<Set<EntityId>>(() => new Set())
  const [movingLayer, setMovingLayer] = useState<EntityId | null>(null)
  const [draggedLayer, setDraggedLayer] = useState<EntityId | null>(null)
  const [dropTarget, setDropTarget] = useState<{
    layer: EntityId
    edge: 'before' | 'after'
  } | null>(null)
  const [addingImage, setAddingImage] = useState(false)
  const anchor = useRef<EntityId | null>(null)

  useEffect(() => {
    setExpandedLayer(selected.length === 1 ? (selected[0] ?? null) : null)
  }, [selected])

  const layers = useMemo(
    () => (page ? displayedLayers(page.layers, page.id, collapsedGroups) : []),
    [collapsedGroups, page],
  )

  if (!page) return <EmptyInspector>{t('inspector.selectPage')}</EmptyInspector>

  const shownSiblings = (layer: Layer) => {
    const parent = layer.parent ?? page.id
    const storedSiblings = page.layers.filter(
      (candidate) => candidate.type !== 'artwork' && (candidate.parent ?? page.id) === parent,
    )
    const parentLayer = page.layers.find((candidate) => candidate.id === parent)
    return {
      parent,
      stored: storedSiblings,
      shown:
        parentLayer && isGroupLayer(parentLayer) && parentLayer.role !== null
          ? storedSiblings
          : [...storedSiblings].reverse(),
      canonical: Boolean(parentLayer && isGroupLayer(parentLayer) && parentLayer.role !== null),
    }
  }

  const commitShownOrder = (layer: Layer, shown: Layer[]) => {
    if (movingLayer !== null || effectiveLayerLocked(page.layers, layer)) return
    const siblings = shownSiblings(layer)
    const stored = siblings.canonical ? shown : [...shown].reverse()
    const target = stored.findIndex((candidate) => candidate.id === layer.id)
    if (target < 0 || siblings.stored[target]?.id === layer.id) return

    setMovingLayer(layer.id)
    void call(commands.moveLayer, layer.id, siblings.parent, target).then(
      (next) => {
        queryClient.setQueryData(pageKey, next)
        setMovingLayer(null)
        void refresh(projectKey)
      },
      () => setMovingLayer(null),
    )
  }

  const move = (layer: Layer, displayDelta: number) => {
    const { shown } = shownSiblings(layer)
    const shownSource = shown.findIndex((candidate) => candidate.id === layer.id)
    const shownTarget = shownSource + displayDelta
    if (shownSource < 0 || shownTarget < 0 || shownTarget >= shown.length) return
    const next = [...shown]
    const [moving] = next.splice(shownSource, 1)
    if (!moving) return
    next.splice(shownTarget, 0, moving)
    commitShownOrder(layer, next)
  }

  const drop = (target: Layer, edge: 'before' | 'after') => {
    const moving = page.layers.find((layer) => layer.id === draggedLayer)
    if (
      !moving ||
      moving.id === target.id ||
      (moving.parent ?? page.id) !== (target.parent ?? page.id) ||
      effectiveLayerLocked(page.layers, moving)
    )
      return
    const { shown } = shownSiblings(moving)
    const next = shown.filter((candidate) => candidate.id !== moving.id)
    const targetIndex = next.findIndex((candidate) => candidate.id === target.id)
    if (targetIndex < 0) return
    next.splice(targetIndex + (edge === 'after' ? 1 : 0), 0, moving)
    commitShownOrder(moving, next)
  }

  const clearDrag = () => {
    setDraggedLayer(null)
    setDropTarget(null)
  }

  const deleteLayer = (layer: EntityId) =>
    void call(commands.deleteLayers, [layer])
      .then(() => {
        if (selected.includes(layer)) {
          selectLayers(selected.filter((selectedLayer) => selectedLayer !== layer))
        }
        setExpandedLayer((current) => (current === layer ? null : current))
        return refresh(projectKey, pageKey)
      })
      .catch(() => undefined)

  // Modifier semantics mirror the page rail: ctrl/meta toggles, shift selects a display range.
  const selectLayer = (layer: EntityId, additive: boolean, range: boolean) => {
    if (!additive && !range) {
      anchor.current = layer
      if (selected.length === 1 && selected[0] === layer) {
        setExpandedLayer((current) => (current === layer ? null : layer))
        return
      }
      selectLayers([layer])
      setExpandedLayer(layer)
      return
    }
    const order = layers.map((row) => row.layer.id)
    const anchorIndex = anchor.current ? order.indexOf(anchor.current) : -1
    if (range && anchorIndex >= 0) {
      const targetIndex = order.indexOf(layer)
      const span = layers
        .slice(Math.min(anchorIndex, targetIndex), Math.max(anchorIndex, targetIndex) + 1)
        .filter(
          (row) => row.layer.type !== 'artwork' && !effectiveLayerLocked(page.layers, row.layer),
        )
        .map((row) => row.layer.id)
      selectLayers(additive ? [...selected, ...span] : span)
      return
    }
    anchor.current = layer
    if (!additive) {
      selectLayers([layer])
      return
    }
    selectLayers(
      selected.includes(layer)
        ? selected.filter((selectedLayer) => selectedLayer !== layer)
        : [...selected, layer],
    )
  }

  const toggleGroup = (group: EntityId) => {
    setCollapsedGroups((current) => {
      const next = new Set(current)
      if (next.has(group)) next.delete(group)
      else next.add(group)
      return next
    })
  }

  const addImageLayer = () => {
    if (addingImage) return
    setAddingImage(true)
    void call(commands.addImageLayer, page.id).then(
      (next) => {
        if (next) queryClient.setQueryData(pageKey, next)
        setAddingImage(false)
        if (next) void refresh(projectKey, pagesKey)
      },
      () => setAddingImage(false),
    )
  }

  const addBubbleLayer = (shape: RegionShapeKind) => {
    void call(commands.addBubbleLayer, page.id, shape)
      .then((added) => {
        queryClient.setQueryData(pageKey, added.page)
        selectLayers([added.layer])
        void refresh(projectKey, pagesKey)
      })
      .catch(() => undefined)
  }

  return (
    <div className='flex min-h-0 flex-1 flex-col'>
      <header className='flex h-8 shrink-0 items-center gap-1.5 border-b border-border/80 px-2'>
        <Layers3 className='size-3 text-primary' />
        <h2 className='text-[10px] font-semibold'>{t('layers.title')}</h2>
        <span className='text-[9px] text-muted-foreground tabular-nums'>
          {page.layers.filter((layer) => !isGroupLayer(layer)).length}
        </span>
      </header>

      <ScrollArea className='min-h-0 flex-1'>
        <div className='py-0.5'>
          {layers.map(({ layer, index, depth }) => {
            const locked = effectiveLayerLocked(page.layers, layer)
            const siblings = shownSiblings(layer).shown
            const position = siblings.findIndex((candidate) => candidate.id === layer.id)
            const canAcceptDrop = draggedLayer
              ? page.layers.some(
                  (candidate) =>
                    candidate.id === draggedLayer &&
                    candidate.id !== layer.id &&
                    (candidate.parent ?? page.id) === (layer.parent ?? page.id),
                )
              : false
            return (
              <LayerRow
                key={`${layer.type}:${layer.id}`}
                layer={layer}
                region={
                  isTextLayer(layer) && layer.content.source_region
                    ? page.regions.find((region) => region.id === layer.content.source_region)
                    : undefined
                }
                index={index}
                depth={depth}
                selected={selected.includes(layer.id)}
                expanded={expandedLayer === layer.id}
                childrenExpanded={!isGroupLayer(layer) || !collapsedGroups.has(layer.id)}
                locked={locked}
                lockable={layer.type !== 'artwork' && (!locked || layer.locked)}
                onSelect={(event) =>
                  selectLayer(layer.id, event.ctrlKey || event.metaKey, event.shiftKey)
                }
                onToggleChildren={isGroupLayer(layer) ? () => toggleGroup(layer.id) : undefined}
                onToggle={() =>
                  void call(commands.setVisibility, [layer.id], !layer.visibility.visible, null)
                    .then(() => refresh(projectKey, pageKey))
                    .catch(() => undefined)
                }
                onLockToggle={() =>
                  void call(commands.setLayerLocked, [layer.id], !layer.locked).then(
                    (next) => {
                      queryClient.setQueryData(pageKey, next)
                      void refresh(projectKey)
                    },
                    () => undefined,
                  )
                }
                onMove={(delta) => move(layer, delta)}
                canMoveUp={!locked && position > 0}
                canMoveDown={!locked && position >= 0 && position < siblings.length - 1}
                reordering={movingLayer !== null}
                dragging={draggedLayer === layer.id}
                dropEdge={dropTarget?.layer === layer.id ? dropTarget.edge : null}
                onDragStart={(event) => {
                  if (locked || movingLayer !== null) return
                  event.dataTransfer.effectAllowed = 'move'
                  event.dataTransfer.setData('text/plain', layer.id)
                  setDraggedLayer(layer.id)
                }}
                onDragOver={(event) => {
                  if (!canAcceptDrop) return
                  event.preventDefault()
                  event.dataTransfer.dropEffect = 'move'
                  const bounds = event.currentTarget.getBoundingClientRect()
                  setDropTarget({
                    layer: layer.id,
                    edge: event.clientY < bounds.top + bounds.height / 2 ? 'before' : 'after',
                  })
                }}
                onDrop={(event) => {
                  if (!canAcceptDrop || !dropTarget || dropTarget.layer !== layer.id) return
                  event.preventDefault()
                  drop(layer, dropTarget.edge)
                  clearDrag()
                }}
                onDragEnd={clearDrag}
                onDelete={!locked && !isGroupLayer(layer) ? () => deleteLayer(layer.id) : undefined}
              />
            )
          })}
          {layers.length === 0 && <EmptyInspector>{t('layers.empty')}</EmptyInspector>}
        </div>
      </ScrollArea>
      <div className='shrink-0 border-t border-border/80 p-1.5'>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                type='button'
                variant='ghost'
                size='sm'
                className='h-7 w-full justify-start gap-1.5 text-[10px]'
              />
            }
          >
            <MessageCircle className='size-3.5' />
            {t('layers.addBubbleLayer')}
            <ChevronDown className='ml-auto size-3' />
          </DropdownMenuTrigger>
          <DropdownMenuContent align='end' className='min-w-40'>
            <DropdownMenuItem onClick={() => addBubbleLayer('circular')}>
              <Circle />
              {t('layers.bubbleShapes.oval')}
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => addBubbleLayer('rectangular')}>
              <Square />
              {t('layers.bubbleShapes.rectangular')}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <Button
          type='button'
          variant='ghost'
          size='sm'
          className='h-7 w-full justify-start gap-1.5 text-[10px]'
          disabled={addingImage}
          onClick={addImageLayer}
        >
          <Plus className='size-3.5' />
          {t('layers.addImageLayer')}
        </Button>
      </div>
    </div>
  )
}

function LayerRow({
  layer,
  region,
  index,
  depth,
  selected,
  expanded,
  childrenExpanded,
  locked,
  lockable,
  onSelect,
  onToggleChildren,
  onToggle,
  onLockToggle,
  onMove,
  canMoveUp,
  canMoveDown,
  reordering,
  dragging,
  dropEdge,
  onDragStart,
  onDragOver,
  onDrop,
  onDragEnd,
  onDelete,
}: {
  layer: Layer
  region?: AnalysisRegion
  index: number
  depth: number
  selected: boolean
  expanded: boolean
  childrenExpanded: boolean
  locked: boolean
  lockable: boolean
  onSelect: (event: MouseEvent<HTMLButtonElement>) => void
  onToggleChildren?: () => void
  onToggle: () => void
  onLockToggle: () => void
  onMove: (delta: number) => void
  canMoveUp: boolean
  canMoveDown: boolean
  reordering: boolean
  dragging: boolean
  dropEdge: 'before' | 'after' | null
  onDragStart: (event: DragEvent<HTMLElement>) => void
  onDragOver: (event: DragEvent<HTMLDivElement>) => void
  onDrop: (event: DragEvent<HTMLDivElement>) => void
  onDragEnd: () => void
  onDelete?: () => void
}) {
  const { t } = useTranslation()
  const name = localizedLayerName(layer, index, t)
  const detail = localizedLayerKind(layer, t)
  const Icon = layerIcon(layer)
  return (
    <div
      data-layer-row={layer.id}
      className={`group relative min-w-0 px-1 py-px ${dragging ? 'opacity-45' : ''}`}
      style={{ paddingLeft: `${depth * 10 + 4}px` }}
      onDragOver={onDragOver}
      onDrop={onDrop}
    >
      {dropEdge && (
        <span
          className={`pointer-events-none absolute right-1 left-1 z-20 h-0.5 rounded-full bg-primary ${dropEdge === 'before' ? 'top-0' : 'bottom-0'}`}
        />
      )}
      <div
        data-selected={selected}
        data-expanded={expanded}
        className='min-w-0 overflow-hidden rounded-lg transition-colors duration-150 data-[selected=true]:bg-accent motion-reduce:transition-none'
      >
        <div className='relative flex min-w-0 items-center gap-0.5'>
          {onToggleChildren && (
            <button
              type='button'
              aria-label={
                childrenExpanded ? t('layers.collapse', { name }) : t('layers.expand', { name })
              }
              aria-expanded={childrenExpanded}
              className='grid size-5 shrink-0 place-items-center rounded text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/25'
              onClick={onToggleChildren}
            >
              {childrenExpanded ? (
                <ChevronDown className='size-3.5' />
              ) : (
                <ChevronRight className='size-3.5' />
              )}
            </button>
          )}
          <button
            type='button'
            draggable={!locked && !reordering}
            aria-label={t('layers.reorder', { name })}
            disabled={locked || reordering}
            className='grid size-5 shrink-0 cursor-grab place-items-center rounded text-muted-foreground/55 hover:bg-foreground/[0.05] hover:text-muted-foreground active:cursor-grabbing disabled:cursor-default disabled:opacity-25'
            onDragStart={onDragStart}
            onDragEnd={onDragEnd}
          >
            <GripVertical className='size-3' />
          </button>
          <button
            type='button'
            aria-label={t('layers.edit', { name })}
            aria-expanded={expanded}
            className='flex min-w-0 flex-1 items-center gap-1.5 rounded-lg px-1.5 py-1 text-left hover:bg-foreground/[0.05] focus-visible:ring-2 focus-visible:ring-ring/25'
            onClick={onSelect}
          >
            <Icon className='size-3.5 shrink-0 text-muted-foreground' />
            <span className='min-w-0 flex-1'>
              <span className='block truncate text-[11px] font-medium'>{name}</span>
              <span className='block truncate text-[9px] leading-3 text-muted-foreground capitalize'>
                {detail}
              </span>
            </span>
          </button>
          {!locked && (
            <div
              className={`pointer-events-none absolute top-1/2 z-10 flex -translate-y-1/2 rounded-md bg-background/80 p-0.5 opacity-0 shadow-sm ring-1 ring-border/40 backdrop-blur-md transition-opacity duration-150 group-hover:pointer-events-auto group-hover:opacity-100 focus-within:pointer-events-auto focus-within:opacity-100 motion-reduce:transition-none ${expanded ? 'right-7' : 'right-[3.25rem]'}`}
            >
              <button
                type='button'
                aria-label={t('layers.moveUp', { name })}
                disabled={reordering || !canMoveUp}
                className='grid size-5 place-items-center rounded-sm text-muted-foreground hover:bg-foreground/[0.07] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/25 disabled:pointer-events-none disabled:opacity-30'
                onClick={() => onMove(-1)}
              >
                <ArrowUp className='size-3' />
              </button>
              <button
                type='button'
                aria-label={t('layers.moveDown', { name })}
                disabled={reordering || !canMoveDown}
                className='grid size-5 place-items-center rounded-sm text-muted-foreground hover:bg-foreground/[0.07] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/25 disabled:pointer-events-none disabled:opacity-30'
                onClick={() => onMove(1)}
              >
                <ArrowDown className='size-3' />
              </button>
            </div>
          )}
          {!expanded && (
            <span className='w-7 shrink-0 text-right text-[9px] text-muted-foreground tabular-nums'>
              {Math.round(layer.visibility.opacity * 100)}%
            </span>
          )}
          <div className='flex w-6 shrink-0 flex-col items-center'>
            <button
              type='button'
              aria-label={
                layer.visibility.visible ? t('layers.hide', { name }) : t('layers.show', { name })
              }
              disabled={layer.type === 'artwork'}
              className='grid size-5 shrink-0 place-items-center rounded text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/25 disabled:opacity-40'
              onClick={onToggle}
            >
              {layer.visibility.visible ? (
                <Eye className='size-3.5' />
              ) : (
                <EyeOff className='size-3.5' />
              )}
            </button>
            <button
              type='button'
              aria-label={
                layer.locked ? t('layers.unlockLabel', { name }) : t('layers.lockLabel', { name })
              }
              disabled={!lockable}
              className='grid size-5 shrink-0 place-items-center rounded text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/25 disabled:opacity-40'
              onClick={onLockToggle}
            >
              {locked ? <Lock className='size-3.5' /> : <Unlock className='size-3.5' />}
            </button>
          </div>
        </div>
        {expanded && (
          <div className='animate-in duration-150 fade-in slide-in-from-top-1 motion-reduce:animate-none'>
            <LayerEditor layer={layer} region={region} onDelete={onDelete} />
          </div>
        )}
      </div>
    </div>
  )
}

function LayerEditor({
  layer,
  region,
  onDelete,
}: {
  layer: Layer
  region?: AnalysisRegion
  onDelete?: () => void
}) {
  const { t } = useTranslation()
  const name = localizedLayerName(layer, 0, t)
  const jobs = useKoharuStore((state) => state.jobs)
  const processing = Object.values(jobs).some((job) => job.state === 'running')
  const [opacity, setOpacity] = useState(layer.visibility.opacity * 100)

  useEffect(() => {
    setOpacity(layer.visibility.opacity * 100)
  }, [layer.id, layer.visibility.opacity])

  const commitOpacity = (next: number) => {
    void call(commands.setVisibility, [layer.id], null, next / 100)
      .then(() => refresh(projectKey, pageKey))
      .catch(() => {
        setOpacity(layer.visibility.opacity * 100)
        previewCanvasOpacity(layer.id, null)
      })
  }

  const previewOpacity = (next: number) => {
    setOpacity(next)
    previewCanvasOpacity(layer.id, next / 100)
  }

  const setClassification = (classification: string | null) => {
    if (!isTextLayer(layer) || !classification) return
    void call(
      commands.setTextClassification,
      layer.id,
      classification === 'not_set' ? null : (classification as TextClassification),
    )
      .then(() => refresh(projectKey, pageKey))
      .catch(() => undefined)
  }

  const setRegionShape = (shape: RegionShapeKind | null) => {
    if (!region || !shape) return
    void call(commands.setRegionShape, region.id, shape)
      .then(() => refresh(projectKey, pageKey))
      .catch(() => undefined)
  }

  const setOcrInclusion = (included: boolean) => {
    if (!isTextLayer(layer)) return
    void call(commands.setOcrInclusion, layer.id, included)
      .then(() => refresh(projectKey, pageKey))
      .catch(() => undefined)
  }

  const setBubbleStyle = (
    update: Partial<
      Pick<
        Extract<Layer, { type: 'bubble' }>,
        'shape' | 'fill_color' | 'outline_color' | 'outline_width'
      >
    >,
  ) => {
    if (layer.type !== 'bubble') return
    void call(commands.setBubbleStyle, layer.id, {
      shape: update.shape ?? layer.shape,
      fill_color: update.fill_color ?? layer.fill_color,
      outline_color: update.outline_color ?? layer.outline_color,
      outline_width: update.outline_width ?? layer.outline_width,
    }).then(
      (next) => {
        queryClient.setQueryData(pageKey, next)
        void refresh(projectKey)
      },
      () => undefined,
    )
  }

  const rerunOcr = () => {
    if (!isTextLayer(layer) || !region || !layer.content.include_for_ocr || processing) return
    void call(
      commands.process,
      { scope: 'entities', value: [layer.id] },
      { operation: 'only', stage: 'ocr' },
    ).catch(() => undefined)
  }

  const rerunTranslation = () => {
    if (!isTextLayer(layer) || !layer.content.source?.text.trim() || processing) return
    void call(
      commands.process,
      { scope: 'entities', value: [layer.id] },
      { operation: 'only', stage: 'translation' },
    ).catch(() => undefined)
  }

  return (
    <div className='grid min-w-0 gap-1.5 px-1.5 pt-0.5 pb-1.5'>
      <div className='flex min-w-0 items-center gap-1.5'>
        <span className='shrink-0 text-[8px] font-medium text-muted-foreground uppercase'>
          {t('inspector.opacity')}
        </span>
        <div className='flex min-w-0 flex-1 items-center gap-1.5'>
          <Slider
            aria-label={t('layers.opacityLabel', { name })}
            min={0}
            max={100}
            step={1}
            value={opacity}
            className='[&_[data-slot=slider-thumb]]:size-2'
            onValueChange={previewOpacity}
            onValueCommitted={commitOpacity}
          />
          <span className='w-7 shrink-0 text-right text-[8px] text-muted-foreground tabular-nums'>
            {Math.round(opacity)}%
          </span>
        </div>
        {onDelete && (
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  type='button'
                  variant='ghost'
                  size='icon-xs'
                  aria-label={t('layers.delete', { name })}
                  className='size-5 rounded-md text-muted-foreground hover:text-foreground'
                  onClick={onDelete}
                />
              }
            >
              <Trash2 className='size-3' />
            </TooltipTrigger>
            <TooltipContent side='left'>{t('layers.delete', { name })}</TooltipContent>
          </Tooltip>
        )}
      </div>
      {isTextLayer(layer) && (
        <>
          <div className={`grid min-w-0 gap-1.5 ${region ? 'grid-cols-2' : 'grid-cols-1'}`}>
            <InspectorField label={t('layers.regionType')}>
              <Select
                value={layer.content.classification ?? 'not_set'}
                disabled={processing}
                onValueChange={setClassification}
              >
                <SelectTrigger
                  size='sm'
                  aria-label={t('layers.regionTypeLabel', { name })}
                  className='w-full min-w-0'
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value='not_set'>{t('layers.regionTypes.notSet')}</SelectItem>
                  <SelectItem value='speech'>{t('layers.regionTypes.speech')}</SelectItem>
                  <SelectItem value='narration'>{t('layers.regionTypes.narration')}</SelectItem>
                  <SelectItem value='sfx'>{t('layers.regionTypes.sfx')}</SelectItem>
                </SelectContent>
              </Select>
            </InspectorField>
            {region && (
              <InspectorField label={t('layers.regionShape')}>
                <Select value={region.shape} disabled={processing} onValueChange={setRegionShape}>
                  <SelectTrigger
                    size='sm'
                    aria-label={t('layers.regionShapeLabel', { name })}
                    className='w-full min-w-0'
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value='rectangular'>
                      {t('layers.regionShapes.rectangular')}
                    </SelectItem>
                    <SelectItem value='circular'>{t('layers.regionShapes.circular')}</SelectItem>
                  </SelectContent>
                </Select>
              </InspectorField>
            )}
          </div>
          <div className='flex min-w-0 items-center gap-2'>
            <label className='flex h-6 min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-md px-1 text-[10px] text-foreground/80'>
              <Checkbox
                checked={layer.content.include_for_ocr}
                disabled={processing}
                onCheckedChange={setOcrInclusion}
              />
              <span>{t('layers.includeForOcr')}</span>
            </label>
            {region && (
              <Button
                type='button'
                variant='outline'
                size='sm'
                className='h-6 justify-start text-[10px] font-normal'
                disabled={processing || !layer.content.include_for_ocr}
                aria-label={t('layers.rerunOcrLabel', { name })}
                onClick={rerunOcr}
              >
                <ScanText data-icon='inline-start' />
                {t('layers.rerunOcr')}
              </Button>
            )}
          </div>
          <InspectorField label={t('inspector.source')}>
            <CommitTextarea
              data-testid={`edit-source-${layer.id}`}
              aria-label={t('layers.sourceLabel', { name })}
              wrap='soft'
              className='max-h-44 min-h-8 w-full max-w-full min-w-0 resize-y overflow-y-auto rounded-md bg-background px-1.5 py-1 text-[12px] leading-4 md:text-[12px]'
              value={layer.content.source?.text ?? ''}
              onSelect={(event) => {
                const field = event.currentTarget
                useKoharuStore.getState().setTextSelection({
                  layer: layer.id,
                  content: 'source',
                  start: field.selectionStart,
                  end: field.selectionEnd,
                })
              }}
              onCommit={(text) =>
                void call(commands.setSourceText, layer.id, text)
                  .then(() => refresh(projectKey, pageKey))
                  .catch(() => undefined)
              }
            />
          </InspectorField>
          <InspectorField label={t('inspector.translation')}>
            <div className='grid gap-1'>
              <Button
                type='button'
                variant='outline'
                size='sm'
                className='h-6 justify-self-end text-[10px] font-normal'
                disabled={processing || !layer.content.source?.text.trim()}
                aria-label={t('layers.rerunTranslationLabel', { name })}
                onClick={rerunTranslation}
              >
                <Languages data-icon='inline-start' />
                {t('layers.rerunTranslation')}
              </Button>
              <CommitTextarea
                data-testid={`edit-translation-${layer.id}`}
                aria-label={t('layers.translationLabel', { name })}
                wrap='soft'
                className='max-h-44 min-h-9 w-full max-w-full min-w-0 resize-y overflow-y-auto rounded-md border-primary/25 bg-background px-1.5 py-1 text-[12px] leading-4 md:text-[12px]'
                value={layer.content.translation?.text ?? ''}
                onSelect={(event) => {
                  const field = event.currentTarget
                  useKoharuStore.getState().setTextSelection({
                    layer: layer.id,
                    content: 'translation',
                    start: field.selectionStart,
                    end: field.selectionEnd,
                  })
                }}
                onCommit={(text) =>
                  void call(commands.setTranslation, layer.id, text.trim() ? text : null)
                    .then(() => refresh(projectKey, pageKey))
                    .catch(() => undefined)
                }
              />
            </div>
          </InspectorField>
        </>
      )}
      {layer.type === 'bubble' && (
        <>
          <InspectorField label={t('layers.bubbleShape')}>
            <Select
              value={layer.shape}
              disabled={processing}
              onValueChange={(shape) => setBubbleStyle({ shape: shape as RegionShapeKind })}
            >
              <SelectTrigger
                size='sm'
                aria-label={t('layers.bubbleShapeLabel', { name })}
                className='w-full min-w-0'
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='circular'>{t('layers.bubbleShapes.oval')}</SelectItem>
                <SelectItem value='rectangular'>{t('layers.bubbleShapes.rectangular')}</SelectItem>
              </SelectContent>
            </Select>
          </InspectorField>
          <div className='grid grid-cols-2 gap-1.5'>
            <InspectorField label={t('layers.bubbleFill')}>
              <ColorWell
                size='sm'
                allowTransparent
                label={t('layers.bubbleFillLabel', { name })}
                value={rgbaToOptionalHex(layer.fill_color)}
                onChange={(color) => setBubbleStyle({ fill_color: optionalHexToRgba(color) })}
              />
            </InspectorField>
            <InspectorField label={t('layers.bubbleOutline')}>
              <ColorWell
                size='sm'
                allowTransparent
                label={t('layers.bubbleOutlineLabel', { name })}
                value={rgbaToOptionalHex(layer.outline_color)}
                onChange={(color) => setBubbleStyle({ outline_color: optionalHexToRgba(color) })}
              />
            </InspectorField>
          </div>
          <InspectorField label={t('layers.bubbleOutlineWidth')}>
            <NumberField
              min={0}
              max={100}
              step={0.5}
              value={layer.outline_width}
              disabled={processing}
              onValueCommitted={(width) => {
                if (width !== null) setBubbleStyle({ outline_width: Math.max(0, width) })
              }}
            >
              <NumberFieldGroup>
                <NumberFieldDecrement aria-label={t('layers.decreaseBubbleOutline')}>
                  <Minus />
                </NumberFieldDecrement>
                <NumberFieldInput aria-label={t('layers.bubbleOutlineWidthLabel', { name })} />
                <NumberFieldIncrement aria-label={t('layers.increaseBubbleOutline')}>
                  <Plus />
                </NumberFieldIncrement>
              </NumberFieldGroup>
            </NumberField>
          </InspectorField>
        </>
      )}
    </div>
  )
}

function InspectorField({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className='grid min-w-0 gap-0.5'>
      <span className='text-[8px] font-medium tracking-[0.06em] text-muted-foreground uppercase'>
        {label}
      </span>
      {children}
    </div>
  )
}

const fontSizePresets = [8, 9, 10, 12, 14, 16, 18, 20, 24, 28, 32, 36, 48, 64, 72, 96]

function FontSizeField({
  value,
  autoFit,
  disabled,
  onChange,
  onAutoFit,
}: {
  value: number
  autoFit: boolean
  disabled: boolean
  onChange: (value: number) => void
  onAutoFit: () => void
}) {
  const { t } = useTranslation()
  const id = useId()
  const [draft, setDraft] = useState<number | null>(value)

  useEffect(() => {
    setDraft(autoFit ? null : value)
  }, [autoFit, value])

  const select = (choice: string) => {
    if (choice === 'auto') {
      setDraft(null)
      onAutoFit()
      return
    }
    const size = Number(choice)
    if (Number.isFinite(size) && size > 0 && size <= 300) {
      setDraft(size)
      onChange(size)
    }
  }

  const commit = (next: number | null) => {
    if (next !== null && next > 0 && next <= 300) {
      onChange(next)
    } else {
      setDraft(autoFit ? null : value)
    }
  }

  return (
    <NumberField
      id={id}
      name='font-size'
      className='min-w-0'
      disabled={disabled}
      value={draft}
      min={0.5}
      max={300}
      step={0.5}
      onValueChange={setDraft}
      onValueCommitted={commit}
    >
      <NumberFieldGroup>
        <NumberFieldInput
          data-testid='type-size'
          aria-label={t('inspector.fontSize')}
          placeholder={t('inspector.auto')}
          className='px-2 text-left'
        />
        <DropdownMenu>
          <DropdownMenuTrigger
            disabled={disabled}
            aria-label={t('inspector.chooseFontSize')}
            className='grid size-6 shrink-0 place-items-center border-l border-input text-muted-foreground outline-none hover:bg-muted hover:text-foreground focus-visible:bg-muted focus-visible:text-foreground disabled:pointer-events-none disabled:opacity-40'
          >
            <ChevronDown className='size-3' />
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align='end'
            className='w-28 min-w-28 border border-border/50 p-0.5 shadow-sm ring-0'
          >
            <DropdownMenuRadioGroup value={autoFit ? 'auto' : String(value)} onValueChange={select}>
              <DropdownMenuRadioItem value='auto' className='min-h-6 py-0.5 text-[10px]'>
                {t('inspector.auto')}
              </DropdownMenuRadioItem>
              {fontSizePresets.map((size) => (
                <DropdownMenuRadioItem
                  key={size}
                  value={String(size)}
                  className='min-h-6 py-0.5 text-[10px] tabular-nums'
                >
                  {size}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </NumberFieldGroup>
    </NumberField>
  )
}

function EmptyInspector({ children }: { children: React.ReactNode }) {
  return (
    <div className='px-4 py-8 text-center text-[10px] leading-4 text-muted-foreground'>
      {children}
    </div>
  )
}

function rgbaToHex([red, green, blue]: [number, number, number, number]): string {
  return `#${[red, green, blue]
    .map((channel) => channel.toString(16).padStart(2, '0'))
    .join('')}`.toUpperCase()
}

function rgbaToOptionalHex(color: [number, number, number, number]): string | null {
  return color[3] === 0 ? null : rgbaToHex(color)
}

function optionalHexToRgba(hex: string | null): [number, number, number, number] {
  return hex === null ? [0, 0, 0, 0] : hexToRgba(hex)
}

function hexToRgba(hex: string): [number, number, number, number] {
  const value = hex.replace('#', '')
  return [
    Number.parseInt(value.slice(0, 2), 16),
    Number.parseInt(value.slice(2, 4), 16),
    Number.parseInt(value.slice(4, 6), 16),
    255,
  ]
}

function layerIcon(layer: Layer): typeof Type {
  if (layer.type === 'group') return Folder
  if (layer.type === 'raster') return Brush
  if (layer.type === 'text') return Type
  if (layer.type === 'bubble') return MessageCircle
  return ImageIcon
}

function localizedLayerName(layer: Layer, index: number, t: TFunction): string {
  if (layer.type === 'group') {
    if (layer.role === 'main_text') return t('layers.mainText')
    if (layer.role === 'sfx_text') return t('layers.sfxText')
    return layer.name
  }
  if (layer.type === 'raster') {
    return layer.kind === 'artwork' ? t('layers.editableArtwork') : layer.name
  }
  if (layer.type === 'text') {
    const text = layer.content.translation?.text || layer.content.source?.text
    return text?.trim() || t('layers.textName', { index: index + 1 })
  }
  if (layer.type === 'bubble') return t('layers.bubbleName', { index: index + 1 })
  if (layer.type === 'artwork') return t('layers.originalArtwork')
  return t('layers.imageName', { index: index + 1 })
}

function localizedLayerKind(layer: Layer, t: TFunction): string {
  if (layer.type === 'group') {
    if (layer.role === 'main_text') return t('layers.kinds.mainTextGroup')
    if (layer.role === 'sfx_text') return t('layers.kinds.sfxTextGroup')
    return t('layers.kinds.group')
  }
  if (layer.type === 'raster') {
    return t(`layers.kinds.${layer.kind}`, { defaultValue: layer.kind })
  }
  if (layer.type === 'text') {
    if (layer.content.classification) {
      return t(`layers.regionTypes.${layer.content.classification}`)
    }
    return t('layers.kinds.text')
  }
  if (layer.type === 'bubble') return t('layers.kinds.bubble')
  return t('layers.kinds.image')
}
