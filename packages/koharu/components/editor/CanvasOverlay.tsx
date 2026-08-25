'use client'

import { useId, useMemo, useRef } from 'react'

import { SelectionControls } from '@/components/editor/SelectionControls'
import {
  effectiveLayerLocked,
  effectiveLayerVisibility,
  expandLayerSelection,
  isTextLayer,
} from '@/lib/document'
import { controlFrame, cssFrame, geometryFrame, selectableLayer, type Camera } from '@/lib/geometry'
import type {
  AnalysisRegion,
  EntityId,
  Frame,
  Geometry,
  InpaintSelection,
  Page,
  Point,
  TransformFrame,
} from '@koharu/bridge/protocol'

interface CanvasOverlayProps {
  page: Page
  camera: Camera
  selected: EntityId[]
  hovered: EntityId | null
  frames: Readonly<Record<EntityId, Frame>>
  previews: Readonly<Record<EntityId, Frame>>
  regionPreviews: Readonly<Record<EntityId, Frame>>
  draft: Frame | null
  cursor: Point | null
  brushSize: number
  showBrushCursor: boolean
  inpaintSelections: readonly InpaintSelection[]
  inpaintDraft: InpaintSelection | null
  onTransformStart: (elements: TransformFrame[]) => void
  onTransformFrame: (elements: TransformFrame[]) => void
  onTransformEnd: () => void
  onRegionTransformStart: (elements: TransformFrame[]) => void
  onRegionTransformFrame: (elements: TransformFrame[]) => void
  onRegionTransformEnd: () => void
}

export function CanvasOverlay({
  page,
  camera,
  selected,
  hovered,
  frames,
  previews,
  regionPreviews,
  draft,
  cursor,
  brushSize,
  showBrushCursor,
  inpaintSelections,
  inpaintDraft,
  onTransformStart,
  onTransformFrame,
  onTransformEnd,
  onRegionTransformStart,
  onRegionTransformFrame,
  onRegionTransformEnd,
}: CanvasOverlayProps) {
  const root = useRef<HTMLDivElement>(null)
  const expandedSelection = useMemo(
    () => expandLayerSelection(page.layers, selected),
    [page.layers, selected],
  )
  const selectedIds = useMemo(() => new Set(expandedSelection), [expandedSelection])
  const multipleSelected = expandedSelection.length > 1
  const layers = useMemo(
    () =>
      page.layers.flatMap((layer) => {
        const visibility = effectiveLayerVisibility(page.layers, layer)
        if (!visibility.visible || visibility.opacity <= 0) return []
        const frame = previews[layer.id] ?? controlFrame(layer, frames)
        return frame ? [{ layer, frame, opacity: visibility.opacity }] : []
      }),
    [page.layers, previews, frames],
  )
  const selectedLayer =
    expandedSelection.length === 1
      ? page.layers.find((layer) => layer.id === expandedSelection[0])
      : undefined
  const selectedLayerLocked = selectedLayer
    ? effectiveLayerLocked(page.layers, selectedLayer)
    : false
  const selectedTextLayer =
    selectedLayer && !selectedLayerLocked && isTextLayer(selectedLayer) ? selectedLayer : undefined
  const sourceRegion = selectedTextLayer?.content.source_region
    ? page.regions.find((region) => region.id === selectedTextLayer.content.source_region)
    : undefined
  const textRegions = useMemo(() => {
    const referenced = new Set(
      page.layers.flatMap((layer) =>
        isTextLayer(layer) && layer.content.source_region ? [layer.content.source_region] : [],
      ),
    )
    return page.regions.filter((region) => referenced.has(region.id))
  }, [page.layers, page.regions])
  const regionControlFrame = sourceRegion
    ? (regionPreviews[sourceRegion.id] ?? geometryFrame(sourceRegion.geometry))
    : null
  const selectionControl = multipleSelected
    ? undefined
    : layers.find(
        ({ layer }) =>
          selectedIds.has(layer.id) &&
          selectableLayer(layer) &&
          !effectiveLayerLocked(page.layers, layer),
      )
  const scale = camera.zoom / window.devicePixelRatio

  return (
    <div
      ref={root}
      data-testid='canvas-overlay'
      className='pointer-events-none absolute inset-0 overflow-hidden'
      aria-hidden
    >
      {textRegions.map((region) => (
        <TextRegionOverlay
          key={region.id}
          region={region}
          preview={regionPreviews[region.id]}
          camera={camera}
          active={region.id === sourceRegion?.id}
        />
      ))}
      {layers.map(({ layer, frame, opacity }) => {
        const position = cssFrame(frame, camera)
        const selected = selectedIds.has(layer.id) && selectableLayer(layer)
        const highlighted = !selected && hovered === layer.id
        return (
          <div
            key={layer.id}
            data-element={layer.id}
            className='absolute box-border bg-transparent'
            style={{
              left: position.left,
              top: position.top,
              width: position.width,
              height: position.height,
              transform: `rotate(${position.angle}deg)`,
              transformOrigin: '50% 50%',
              border:
                highlighted || (selected && multipleSelected)
                  ? '1px solid var(--canvas-selection)'
                  : undefined,
              opacity,
              willChange: selected ? 'left, top, width, height, transform' : undefined,
            }}
          />
        )
      })}

      {draft && <DraftOverlay frame={draft} camera={camera} />}
      {(inpaintSelections.length > 0 || inpaintDraft) && (
        <InpaintSelectionOverlay
          page={page}
          camera={camera}
          selections={inpaintDraft ? [...inpaintSelections, inpaintDraft] : inpaintSelections}
        />
      )}
      {showBrushCursor && cursor && (
        <div
          className='absolute rounded-full border border-white/95 shadow-[0_0_0_1px_rgb(0_0_0/0.9),0_1px_3px_rgb(0_0_0/0.45)]'
          style={{
            left: cursor.x / window.devicePixelRatio - (brushSize * scale) / 2,
            top: cursor.y / window.devicePixelRatio - (brushSize * scale) / 2,
            width: brushSize * scale,
            height: brushSize * scale,
          }}
        />
      )}

      {sourceRegion && regionControlFrame ? (
        <SelectionControls
          container={root}
          element={sourceRegion.id}
          frame={regionControlFrame}
          camera={camera}
          edgesOnly={false}
          onTransformStart={onRegionTransformStart}
          onTransformFrame={onRegionTransformFrame}
          onTransformEnd={onRegionTransformEnd}
        />
      ) : selectionControl ? (
        <SelectionControls
          container={root}
          element={selectionControl.layer.id}
          frame={selectionControl.frame}
          camera={camera}
          edgesOnly={Boolean(selectedTextLayer)}
          onTransformStart={onTransformStart}
          onTransformFrame={onTransformFrame}
          onTransformEnd={onTransformEnd}
        />
      ) : null}
    </div>
  )
}

function InpaintSelectionOverlay({
  page,
  camera,
  selections,
}: {
  page: Page
  camera: Camera
  selections: readonly InpaintSelection[]
}) {
  const clipId = useId().replaceAll(':', '')
  const dpr = window.devicePixelRatio
  const scale = camera.zoom / dpr
  const screenPoint = (point: Point) => ({
    x: (point.x * camera.zoom + camera.translation[0]) / dpr,
    y: (point.y * camera.zoom + camera.translation[1]) / dpr,
  })
  const pageOrigin = screenPoint({ x: 0, y: 0 })

  return (
    <svg
      data-testid='inpaint-selection-overlay'
      className='absolute inset-0 size-full overflow-hidden'
    >
      <defs>
        <clipPath id={clipId}>
          <rect
            x={pageOrigin.x}
            y={pageOrigin.y}
            width={page.size.width * scale}
            height={page.size.height * scale}
          />
        </clipPath>
      </defs>
      <g clipPath={`url(#${clipId})`} fill='rgb(168 85 247 / 0.3)' stroke='rgb(216 180 254 / 0.95)'>
        {selections.map((selection, index) => {
          if (selection.type === 'pen') {
            const points = selection.points.map(screenPoint)
            const path = points
              .map((point, pointIndex) => `${pointIndex === 0 ? 'M' : 'L'} ${point.x} ${point.y}`)
              .join(' ')
            return points.length === 1 ? (
              <circle
                key={index}
                cx={points[0].x}
                cy={points[0].y}
                r={(selection.diameter * scale) / 2}
                stroke='none'
              />
            ) : (
              <path
                key={index}
                d={path}
                fill='none'
                stroke='rgb(168 85 247 / 0.55)'
                strokeWidth={selection.diameter * scale}
                strokeLinecap='round'
                strokeLinejoin='round'
              />
            )
          }
          const bounds = constrainedSelectionBounds(selection.start, selection.end)
          const topLeft = screenPoint({ x: bounds.left, y: bounds.top })
          const size = bounds.size * scale
          return selection.type === 'circle' ? (
            <circle
              key={index}
              cx={topLeft.x + size / 2}
              cy={topLeft.y + size / 2}
              r={size / 2}
              strokeWidth={1.5}
            />
          ) : (
            <rect
              key={index}
              x={topLeft.x}
              y={topLeft.y}
              width={size}
              height={size}
              strokeWidth={1.5}
            />
          )
        })}
      </g>
    </svg>
  )
}

export function constrainedSelectionBounds(start: Point, end: Point) {
  const deltaX = end.x - start.x
  const deltaY = end.y - start.y
  const size = Math.max(Math.abs(deltaX), Math.abs(deltaY), 1)
  const endX = start.x + size * (deltaX < 0 ? -1 : 1)
  const endY = start.y + size * (deltaY < 0 ? -1 : 1)
  return {
    left: Math.min(start.x, endX),
    top: Math.min(start.y, endY),
    size,
  }
}

function TextRegionOverlay({
  region,
  preview,
  camera,
  active,
}: {
  region: AnalysisRegion
  preview?: Frame
  camera: Camera
  active: boolean
}) {
  const dpr = window.devicePixelRatio
  const frame = preview ?? geometryFrame(region.geometry)
  const geometry = preview ? frameGeometry(preview) : region.geometry
  const points = geometry.points
    .map(
      (point) =>
        `${(point.x * camera.zoom + camera.translation[0]) / dpr},${(point.y * camera.zoom + camera.translation[1]) / dpr}`,
    )
    .join(' ')
  if (!points || !frame) return null

  const position = cssFrame(frame, camera)
  const common = {
    fill: 'none',
    strokeDasharray: '10 6',
    strokeLinecap: 'round' as const,
    strokeLinejoin: 'round' as const,
    vectorEffect: 'non-scaling-stroke' as const,
  }
  const shape = (stroke: string, strokeWidth: string) =>
    region.shape === 'circular' ? (
      <ellipse
        cx={position.left + position.width / 2}
        cy={position.top + position.height / 2}
        rx={position.width / 2}
        ry={position.height / 2}
        transform={`rotate(${position.angle} ${position.left + position.width / 2} ${position.top + position.height / 2})`}
        stroke={stroke}
        strokeWidth={strokeWidth}
        {...common}
      />
    ) : (
      <polygon points={points} stroke={stroke} strokeWidth={strokeWidth} {...common} />
    )

  return (
    <svg
      className='absolute inset-0 size-full overflow-hidden'
      data-testid={active ? 'text-source-region' : 'text-region-preview'}
      opacity={active ? 1 : 0.25}
    >
      {shape('var(--canvas-region-contrast)', active ? '7' : '5')}
      {shape('var(--canvas-region-stroke)', active ? '3' : '1.5')}
    </svg>
  )
}

function frameGeometry(frame: Frame): Geometry {
  const center = { x: frame.x + frame.width / 2, y: frame.y + frame.height / 2 }
  const angle = (frame.angle_degrees * Math.PI) / 180
  const cos = Math.cos(angle)
  const sin = Math.sin(angle)
  const corners: Array<[number, number]> = [
    [-frame.width / 2, -frame.height / 2],
    [frame.width / 2, -frame.height / 2],
    [frame.width / 2, frame.height / 2],
    [-frame.width / 2, frame.height / 2],
  ]
  return {
    points: corners.map(([x, y]) => ({
      x: center.x + x * cos - y * sin,
      y: center.y + x * sin + y * cos,
    })),
  }
}

function DraftOverlay({ frame, camera }: { frame: Frame; camera: Camera }) {
  const position = cssFrame(frame, camera)
  return (
    <div
      className='absolute border border-dashed border-primary bg-primary/5'
      style={{
        left: position.left,
        top: position.top,
        width: position.width,
        height: position.height,
        transform: `rotate(${position.angle}deg)`,
      }}
    />
  )
}
