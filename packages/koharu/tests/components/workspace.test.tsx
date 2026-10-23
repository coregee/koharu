import { QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { CanvasWorkspace } from '@/components/editor/CanvasWorkspace'
import { pageKey, pagesKey, projectKey, queryClient } from '@/lib/queries'
import { useKoharuStore } from '@/lib/store'
import { commands, type Layer } from '@koharu/bridge/protocol'
import { TooltipProvider } from '@koharu/ui/components/tooltip'

const canvas = vi.hoisted(() => ({
  resize: vi.fn(),
  setView: vi.fn(),
  stageManifest: vi.fn(),
  installResource: vi.fn(),
  activateFrame: vi.fn(),
  activatePage: vi.fn(),
  clear: vi.fn(),
  previewOpacity: vi.fn(),
  beginTransform: vi.fn(),
  updateTransform: vi.fn(),
  finishTransform: vi.fn(),
  cancelTransform: vi.fn(),
  beginStroke: vi.fn(),
  extendStroke: vi.fn(),
  finishStroke: vi.fn(),
  cancelStroke: vi.fn(),
  sampleColor: vi.fn(),
  dispose: vi.fn(),
}))

const canvasState = vi.hoisted(() => ({
  canvas,
  error: null as Error | null,
  generation: 1 as number | null,
  hasFrame: true,
  retry: vi.fn(),
  status: 'ready' as 'loading' | 'switching' | 'ready' | 'recovering' | 'error',
}))
const prefetchCanvasPages = vi.hoisted(() => vi.fn(async () => []))

vi.mock('@/components/editor/useCanvas', () => ({
  useCanvas: () => canvasState,
}))
vi.mock('@koharu/bridge/canvas', () => ({
  prefetchCanvasPages,
  workspaceColor: () => [245, 245, 245],
}))

const layer: Layer = {
  type: 'image',
  locked: false,
  id: 'element',
  parent: 'page',
  geometry: {
    points: [
      { x: 10, y: 20 },
      { x: 110, y: 20 },
      { x: 110, y: 70 },
      { x: 10, y: 70 },
    ],
  },
  visibility: { visible: true, opacity: 1 },
  image: 'image',
}

const paintLayer: Layer = {
  type: 'raster',
  locked: false,
  id: 'paint',
  parent: 'page',
  visibility: { visible: true, opacity: 1 },
  image: 'paint-image',
  name: 'Paint 1',
  kind: 'paint',
  consumed: false,
}

const originalArtworkLayer: Layer = {
  type: 'artwork',
  locked: true,
  id: 'page',
  parent: null,
  geometry: {
    points: [
      { x: 0, y: 0 },
      { x: 1000, y: 0 },
      { x: 1000, y: 1000 },
      { x: 0, y: 1000 },
    ],
  },
  visibility: { visible: true, opacity: 1 },
  image: 'original-image',
}

const artworkLayer: Layer = {
  type: 'raster',
  locked: false,
  id: 'artwork',
  parent: 'page',
  visibility: { visible: true, opacity: 1 },
  image: 'artwork-image',
  name: 'Artwork',
  kind: 'artwork',
  consumed: false,
}

const segmentationLayer: Layer = {
  type: 'raster',
  locked: false,
  id: 'segmentation',
  parent: 'page',
  visibility: { visible: true, opacity: 0.45 },
  image: 'segmentation-image',
  name: 'Text Segmentation',
  kind: 'segmentation',
  consumed: false,
}

const repairLayer: Layer = {
  type: 'raster',
  locked: false,
  id: 'repair',
  parent: 'page',
  visibility: { visible: true, opacity: 0.45 },
  image: 'repair-image',
  name: 'Repair Mask 1',
  kind: 'repair',
  consumed: false,
}

let nextAnimationFrame = 1
let animationFrames = new Map<number, FrameRequestCallback>()

beforeEach(() => {
  nextAnimationFrame = 1
  animationFrames = new Map()
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    const frame = nextAnimationFrame++
    animationFrames.set(frame, callback)
    return frame
  })
  vi.stubGlobal('cancelAnimationFrame', (frame: number) => animationFrames.delete(frame))
  canvasState.error = null
  canvasState.generation = 1
  canvasState.hasFrame = true
  canvasState.status = 'ready'
  prefetchCanvasPages.mockClear()
})

afterEach(() => vi.unstubAllGlobals())

function installProject() {
  const page = {
    id: 'page',
    label: 'Page',
    size: { width: 1000, height: 1000 },
    layers: [layer],
    regions: [],
  }
  queryClient.setQueryData(projectKey, {
    name: 'Book',
    revision: 1,
    active_page: 'page',
    can_undo: false,
    can_redo: false,
    can_paste: false,
  })
  queryClient.setQueryData(pagesKey, [])
  queryClient.setQueryData(pageKey, page)
  useKoharuStore.setState({ selectedLayers: [], toolMode: 'paint', tool: 'select' })
  useKoharuStore.setState({
    canvasPage: 'page',
    canvasRevision: 1,
    canvasGeneration: 1,
    canvasSize: [1000, 1000],
  })
}

function renderWorkspace() {
  render(
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>
        <CanvasWorkspace />
      </TooltipProvider>
    </QueryClientProvider>,
  )
  const surface = screen.getByLabelText('Koharu canvas')
  Object.defineProperty(surface, 'getBoundingClientRect', {
    value: () => ({ x: 10, y: 20, width: 800, height: 600 }),
  })
  return surface
}

describe('canvas interaction adapter', () => {
  it('prefetches only after the authoritative canvas page and generation are active', async () => {
    installProject()
    queryClient.setQueryData(pagesKey, [
      {
        id: 'page',
        label: 'Page',
        size: { width: 1000, height: 1000 },
        source_asset: null,
        layer_count: 1,
      },
      {
        id: 'next',
        label: 'Next',
        size: { width: 1000, height: 1000 },
        source_asset: null,
        layer_count: 1,
      },
    ])
    useKoharuStore.setState({ canvasPage: 'previous' })
    renderWorkspace()

    await Promise.resolve()
    expect(prefetchCanvasPages).not.toHaveBeenCalled()

    act(() => useKoharuStore.setState({ canvasPage: 'page' }))
    await waitFor(() => expect(prefetchCanvasPages).toHaveBeenCalledWith(['next']))
  })

  it('renders an accessible browser canvas and keeps camera updates local', async () => {
    installProject()
    const surface = renderWorkspace()
    expect(screen.getByTestId('webgpu-canvas')).toBeInstanceOf(HTMLCanvasElement)

    fireEvent.wheel(surface, { clientX: 100, clientY: 100, deltaY: 4 })
    fireEvent.wheel(surface, { clientX: 100, clientY: 100, deltaY: 6 })

    await waitFor(() => expect(canvas.setView).toHaveBeenLastCalledWith(1, [0, -10]))
  })

  it('preserves a manual camera when a newer canvas generation becomes ready', () => {
    installProject()
    renderWorkspace()
    const camera = { zoom: 2, translation: [-120, -80] as [number, number], fitted: false }

    act(() => useKoharuStore.setState({ camera }))
    act(() => {
      canvasState.status = 'switching'
      useKoharuStore.setState({ canvasGeneration: 2 })
    })
    act(() => {
      canvasState.status = 'ready'
      canvasState.generation = 2
      useKoharuStore.setState({ canvasRevision: 2 })
    })

    expect(useKoharuStore.getState().camera).toEqual(camera)
  })

  it('suppresses Alt browser handling while an editor input retains focus', () => {
    installProject()
    renderWorkspace()
    const input = document.createElement('input')
    document.body.append(input)
    input.focus()

    const event = new KeyboardEvent('keydown', { key: 'Alt', bubbles: true, cancelable: true })
    input.dispatchEvent(event)

    expect(event.defaultPrevented).toBe(true)
    input.remove()
  })

  it('copies and pastes selected layers with the platform editing shortcuts', async () => {
    installProject()
    queryClient.setQueryData(projectKey, {
      ...queryClient.getQueryData(projectKey)!,
      can_paste: true,
    })
    useKoharuStore.setState({ selectedLayers: ['element'] })
    const copy = vi.spyOn(commands, 'copyLayers').mockResolvedValue(null)
    const paste = vi.spyOn(commands, 'pasteLayers').mockResolvedValue(['pasted'])
    renderWorkspace()

    fireEvent.keyDown(window, { key: 'c', ctrlKey: true })
    await waitFor(() => expect(copy).toHaveBeenCalledWith(['element']))

    fireEvent.keyDown(window, { key: 'v', ctrlKey: true })
    await waitFor(() => expect(paste).toHaveBeenCalledWith('page'))
    expect(useKoharuStore.getState().selectedLayers).toEqual(['pasted'])
  })

  it('announces WebGPU startup failures and offers recovery', () => {
    installProject()
    canvasState.status = 'error'
    canvasState.error = new Error('No compatible WebGPU adapter was found.')
    renderWorkspace()
    expect(screen.getByRole('alert')).toHaveTextContent('WebGPU canvas unavailable')
    expect(screen.getByRole('alert')).toHaveTextContent('No compatible WebGPU adapter was found.')
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(canvasState.retry).toHaveBeenCalledOnce()
  })

  it('previews brush input locally and sends only the durable paint commit to Rust', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [originalArtworkLayer, artworkLayer, ...page.layers],
    }))
    useKoharuStore.setState({
      toolMode: 'paint',
      tool: 'brush',
      selectedLayers: ['artwork'],
      brush: { diameter: 48, color: '#FFFFFF' },
    })
    const commit = vi
      .spyOn(commands, 'commitRasterBrush')
      .mockResolvedValue({ revision: 2, layer: 'paint' })
    const surface = renderWorkspace()
    expect(surface).toHaveStyle({ cursor: 'none' })

    fireEvent.pointerDown(surface, { button: 0, pointerId: 7, clientX: 30, clientY: 40 })
    fireEvent.pointerMove(surface, { pointerId: 7, clientX: 55, clientY: 65 })
    fireEvent.pointerUp(surface, { pointerId: 7, clientX: 58, clientY: 70 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(canvas.beginStroke).toHaveBeenCalledWith({
      kind: 'paint',
      layer: 'artwork',
      point: { x: 20, y: 20 },
      diameter: 48,
      color: [255, 255, 255, 255],
    })
    expect(canvas.extendStroke).toHaveBeenCalledWith(expect.arrayContaining([{ x: 45, y: 45 }]))
    expect(canvas.finishStroke).toHaveBeenCalledOnce()
    expect(commit).toHaveBeenCalledWith(
      1,
      'artwork',
      'paint',
      expect.arrayContaining([{ x: 45, y: 45 }]),
      { diameter: 48, color: [255, 255, 255, 255] },
    )
  })

  it('paints a dedicated green censor mask layer', async () => {
    installProject()
    useKoharuStore.setState({
      toolMode: 'decensor',
      tool: 'brush',
      brush: { diameter: 32, color: '#FFFFFF' },
    })
    const commit = vi
      .spyOn(commands, 'commitRasterBrush')
      .mockResolvedValue({ revision: 2, layer: 'censor' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 17, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 17, clientX: 30, clientY: 40 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(canvas.beginStroke).toHaveBeenCalledWith({
      kind: 'paint',
      layer: null,
      point: { x: 20, y: 20 },
      diameter: 32,
      color: [0, 255, 0, 180],
    })
    expect(commit).toHaveBeenCalledWith(
      1,
      null,
      'bar_censor',
      expect.arrayContaining([{ x: 20, y: 20 }]),
      { diameter: 32, color: [0, 255, 0, 180] },
    )
  })

  it.each([
    ['revision', { canvasRevision: 2 }],
    ['generation', { canvasGeneration: 2 }],
  ])('cancels an active gesture when the canvas %s changes', async (_name, update) => {
    installProject()
    useKoharuStore.setState({ tool: 'brush', brush: { diameter: 48, color: '#FFFFFF' } })
    const commit = vi
      .spyOn(commands, 'commitRasterBrush')
      .mockResolvedValue({ revision: 2, layer: 'paint' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 8, clientX: 30, clientY: 40 })
    expect(canvas.beginStroke).toHaveBeenCalledOnce()

    act(() => {
      useKoharuStore.setState(update)
    })

    await waitFor(() => expect(canvas.cancelStroke).toHaveBeenCalledOnce())
    fireEvent.pointerUp(surface, { pointerId: 8, clientX: 30, clientY: 40 })
    expect(canvas.finishStroke).not.toHaveBeenCalled()
    expect(commit).not.toHaveBeenCalled()
  })

  it('uses rendered text bounds for hit testing and semantic transforms', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [
        {
          type: 'text',
          id: 'element',
          locked: false,
          parent: 'page',
          geometry: layer.geometry,
          angle_degrees: 0,
          visibility: { visible: true, opacity: 1 },
          content: {
            id: 'content',
            source: { text: 'Source', language: 'en' },
            translation: { text: 'Rendered', language: null },
            classification: null,
            source_region: null,
            include_for_ocr: false,
          },
          typography: null,
          layout: 'paragraph',
        },
      ],
    }))
    useKoharuStore.setState({
      layerFrames: {
        element: { x: 30, y: 40, width: 50, height: 20, angle_degrees: 0 },
      },
    })
    const commit = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 9, clientX: 50, clientY: 60 })
    fireEvent.pointerMove(surface, { pointerId: 9, clientX: 70, clientY: 80 })
    fireEvent.pointerUp(surface, { pointerId: 9, clientX: 70, clientY: 80 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(useKoharuStore.getState().selectedLayers).toEqual(['element'])
    expect(canvas.beginTransform).toHaveBeenCalledWith([
      {
        element: 'element',
        frame: { x: 30, y: 40, width: 50, height: 20, angle_degrees: 0 },
      },
    ])
    expect(canvas.updateTransform).toHaveBeenCalledWith(
      expect.arrayContaining([
        expect.objectContaining({
          element: 'element',
          frame: expect.objectContaining({ x: 50, y: 60 }),
        }),
      ]),
    )
    expect(canvas.finishTransform).toHaveBeenCalledOnce()
    expect(commit).toHaveBeenCalledWith(
      1,
      expect.arrayContaining([
        expect.objectContaining({
          element: 'element',
          frame: expect.objectContaining({ x: 50, y: 60 }),
        }),
      ]),
    )
  })

  it('resizes a selected layer through Koharu selection controls', async () => {
    installProject()
    useKoharuStore.setState({ selectedLayers: ['element'] })
    const commit = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    renderWorkspace()
    Object.defineProperty(screen.getByTestId('canvas-overlay'), 'getBoundingClientRect', {
      value: () => ({ x: 10, y: 20, width: 800, height: 600 }),
    })
    const handle = document.querySelector<HTMLElement>('[data-resize-handle="e"]')!

    fireEvent.pointerDown(handle, { button: 0, pointerId: 10, clientX: 120, clientY: 65 })
    fireEvent.pointerMove(handle, { pointerId: 10, clientX: 140, clientY: 65 })
    fireEvent.pointerUp(handle, { pointerId: 10, clientX: 140, clientY: 65 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(canvas.beginTransform).toHaveBeenCalledWith([
      { element: 'element', frame: { x: 10, y: 20, width: 100, height: 50, angle_degrees: 0 } },
    ])
    expect(canvas.updateTransform).toHaveBeenCalledWith(
      expect.arrayContaining([
        {
          element: 'element',
          frame: { x: 10, y: 20, width: 120, height: 50, angle_degrees: 0 },
        },
      ]),
    )
    expect(commit).toHaveBeenCalledWith(
      1,
      expect.arrayContaining([
        {
          element: 'element',
          frame: { x: 10, y: 20, width: 120, height: 50, angle_degrees: 0 },
        },
      ]),
    )
  })

  it('resizes the detected source region instead of the rendered text frame', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [
        {
          type: 'text',
          id: 'element',
          locked: false,
          parent: 'page',
          geometry: null,
          visibility: { visible: true, opacity: 1 },
          content: {
            id: 'content',
            source: { text: 'Source', language: 'en' },
            translation: null,
            classification: 'narration',
            source_region: 'region',
            include_for_ocr: true,
          },
          typography: null,
          layout: 'paragraph',
        },
      ],
      regions: [
        {
          id: 'region',
          parent: 'page',
          geometry: {
            points: [
              { x: 10, y: 20 },
              { x: 110, y: 20 },
              { x: 110, y: 70 },
              { x: 10, y: 70 },
            ],
          },
          kind: 'dev.koharu.region.text',
          label: 'text',
          shape: 'rectangular',
        },
      ],
    }))
    useKoharuStore.setState({ selectedLayers: ['element'] })
    const setRegionGeometry = vi.spyOn(commands, 'setRegionGeometry').mockResolvedValue(null)
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(null)
    renderWorkspace()
    Object.defineProperty(screen.getByTestId('canvas-overlay'), 'getBoundingClientRect', {
      value: () => ({ x: 10, y: 20, width: 800, height: 600 }),
    })
    const handle = document.querySelector<HTMLElement>('[data-resize-handle="e"]')!

    fireEvent.pointerDown(handle, { button: 0, pointerId: 11, clientX: 120, clientY: 65 })
    fireEvent.pointerMove(handle, { pointerId: 11, clientX: 140, clientY: 65 })
    fireEvent.pointerUp(handle, { pointerId: 11, clientX: 140, clientY: 65 })

    await waitFor(() =>
      expect(setRegionGeometry).toHaveBeenCalledWith('region', {
        x: 10,
        y: 20,
        width: 120,
        height: 50,
        angle_degrees: 0,
      }),
    )
    expect(canvas.beginTransform).not.toHaveBeenCalled()
    expect(commitTransform).not.toHaveBeenCalled()
  })

  it('rotates a detected source region from its visible rotation handle', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [
        {
          type: 'text',
          id: 'element',
          locked: false,
          parent: 'page',
          geometry: null,
          visibility: { visible: true, opacity: 1 },
          content: {
            id: 'content',
            source: { text: 'Source', language: 'en' },
            translation: null,
            classification: 'narration',
            source_region: 'region',
            include_for_ocr: true,
          },
          typography: null,
          layout: 'paragraph',
        },
      ],
      regions: [
        {
          id: 'region',
          parent: 'page',
          geometry: {
            points: [
              { x: 10, y: 20 },
              { x: 110, y: 20 },
              { x: 110, y: 70 },
              { x: 10, y: 70 },
            ],
          },
          kind: 'dev.koharu.region.text',
          label: 'text',
          shape: 'rectangular',
        },
      ],
    }))
    useKoharuStore.setState({ selectedLayers: ['element'] })
    const setRegionGeometry = vi.spyOn(commands, 'setRegionGeometry').mockResolvedValue(null)
    renderWorkspace()
    Object.defineProperty(screen.getByTestId('canvas-overlay'), 'getBoundingClientRect', {
      value: () => ({ x: 10, y: 20, width: 800, height: 600 }),
    })
    const handle = document.querySelector<HTMLElement>('[data-rotate-handle]')!

    expect(handle).toBeInTheDocument()
    expect(handle.querySelector('.lucide-rotate-cw')).toBeInTheDocument()
    fireEvent.pointerDown(handle, { button: 0, pointerId: 12, clientX: 70, clientY: 15 })
    fireEvent.pointerMove(handle, {
      pointerId: 12,
      clientX: 101,
      clientY: 20,
      shiftKey: true,
    })
    fireEvent.pointerUp(handle, { pointerId: 12, clientX: 101, clientY: 20 })

    await waitFor(() =>
      expect(setRegionGeometry).toHaveBeenCalledWith('region', {
        x: 10,
        y: 20,
        width: 100,
        height: 50,
        angle_degrees: 30,
      }),
    )
  })

  it('moves a detected source region by dragging inside it', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [
        {
          type: 'text',
          id: 'element',
          locked: false,
          parent: 'page',
          geometry: null,
          visibility: { visible: true, opacity: 1 },
          content: {
            id: 'content',
            source: { text: 'Source', language: 'en' },
            translation: null,
            classification: 'narration',
            source_region: 'region',
            include_for_ocr: true,
          },
          typography: null,
          layout: 'paragraph',
        },
      ],
      regions: [
        {
          id: 'region',
          parent: 'page',
          geometry: {
            points: [
              { x: 10, y: 20 },
              { x: 110, y: 20 },
              { x: 110, y: 70 },
              { x: 10, y: 70 },
            ],
          },
          kind: 'dev.koharu.region.text',
          label: 'text',
          shape: 'rectangular',
        },
      ],
    }))
    const setRegionGeometry = vi.spyOn(commands, 'setRegionGeometry').mockResolvedValue(null)
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(null)
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 12, clientX: 50, clientY: 60 })
    fireEvent.pointerMove(surface, { pointerId: 12, clientX: 70, clientY: 80 })
    fireEvent.pointerUp(surface, { pointerId: 12, clientX: 70, clientY: 80 })

    await waitFor(() =>
      expect(setRegionGeometry).toHaveBeenCalledWith('region', {
        x: 30,
        y: 40,
        width: 100,
        height: 50,
        angle_degrees: 0,
      }),
    )
    expect(useKoharuStore.getState().selectedLayers).toEqual(['element'])
    expect(canvas.beginTransform).not.toHaveBeenCalled()
    expect(commitTransform).not.toHaveBeenCalled()
  })

  it('shows the circular source region behind the selected text controls', () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [
        {
          type: 'text',
          id: 'element',
          locked: false,
          parent: 'page',
          geometry: null,
          angle_degrees: null,
          visibility: { visible: true, opacity: 1 },
          content: {
            id: 'content',
            source: { text: 'Source', language: 'en' },
            translation: { text: 'Rendered', language: null },
            classification: null,
            source_region: 'region',
            include_for_ocr: true,
          },
          typography: null,
          layout: 'paragraph',
        },
        {
          type: 'text',
          id: 'other-element',
          locked: false,
          parent: 'page',
          geometry: null,
          visibility: { visible: true, opacity: 1 },
          content: {
            id: 'other-content',
            source: { text: 'Other', language: 'en' },
            translation: null,
            classification: null,
            source_region: 'other-region',
            include_for_ocr: true,
          },
          typography: null,
          layout: 'paragraph',
        },
      ],
      regions: [
        {
          id: 'region',
          parent: 'page',
          geometry: {
            points: [
              { x: 20, y: 30 },
              { x: 100, y: 30 },
              { x: 100, y: 90 },
              { x: 20, y: 90 },
            ],
          },
          kind: 'dev.koharu.region.text',
          label: null,
          shape: 'circular',
        },
        {
          id: 'other-region',
          parent: 'page',
          geometry: {
            points: [
              { x: 120, y: 130 },
              { x: 180, y: 130 },
              { x: 180, y: 170 },
              { x: 120, y: 170 },
            ],
          },
          kind: 'dev.koharu.region.text',
          label: null,
          shape: 'rectangular',
        },
        {
          id: 'removed-layer-region',
          parent: 'page',
          geometry: {
            points: [
              { x: 200, y: 210 },
              { x: 260, y: 210 },
              { x: 260, y: 250 },
              { x: 200, y: 250 },
            ],
          },
          kind: 'dev.koharu.region.text',
          label: null,
          shape: 'rectangular',
        },
      ],
    }))
    useKoharuStore.setState({
      selectedLayers: ['element'],
      layerFrames: {
        element: { x: 30, y: 40, width: 50, height: 20, angle_degrees: 0 },
      },
    })

    renderWorkspace()

    expect(screen.getByTestId('text-source-region').querySelector('ellipse')).toHaveAttribute(
      'rx',
      '40',
    )
    expect(screen.getByTestId('text-source-region')).toHaveAttribute('opacity', '1')
    expect(screen.getByTestId('text-region-preview')).toHaveAttribute('opacity', '0.25')
    expect(screen.getAllByTestId('text-region-preview')).toHaveLength(1)
  })

  it('targets the selected raster layer with the eraser', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [...page.layers, paintLayer],
    }))
    useKoharuStore.setState({ tool: 'eraser', selectedLayers: ['paint'] })
    const commit = vi
      .spyOn(commands, 'commitRasterErase')
      .mockResolvedValue({ revision: 2, layer: 'paint' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 11, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 11, clientX: 30, clientY: 40 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(canvas.beginStroke).toHaveBeenCalledWith({
      kind: 'erase',
      layer: 'paint',
      point: { x: 20, y: 20 },
      diameter: 48,
    })
    expect(commit).toHaveBeenCalledWith(
      1,
      'paint',
      'paint',
      expect.arrayContaining([{ x: 20, y: 20 }]),
      48,
    )
  })

  it('targets the detected segmentation layer with the eraser', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [
        ...page.layers,
        {
          type: 'raster',
          locked: false,
          id: 'segmentation',
          parent: 'page',
          visibility: { visible: true, opacity: 0.45 },
          image: 'segmentation-image',
          name: 'Text Segmentation',
          kind: 'segmentation',
          consumed: false,
        },
      ],
    }))
    useKoharuStore.setState({
      toolMode: 'removal',
      tool: 'eraser',
      selectedLayers: ['segmentation'],
    })
    const commit = vi
      .spyOn(commands, 'commitRasterErase')
      .mockResolvedValue({ revision: 2, layer: 'segmentation' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 16, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 16, clientX: 30, clientY: 40 })

    await waitFor(() =>
      expect(commit).toHaveBeenCalledWith(
        1,
        'segmentation',
        'removal',
        expect.arrayContaining([{ x: 20, y: 20 }]),
        48,
      ),
    )
    expect(canvas.beginStroke).toHaveBeenCalledWith({
      kind: 'erase',
      layer: 'segmentation',
      point: { x: 20, y: 20 },
      diameter: 48,
    })
  })

  it('targets an imported image layer with the eraser', async () => {
    installProject()
    useKoharuStore.setState({ tool: 'eraser', selectedLayers: ['element'] })
    const commit = vi
      .spyOn(commands, 'commitRasterErase')
      .mockResolvedValue({ revision: 2, layer: 'element' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 12, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 12, clientX: 30, clientY: 40 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(canvas.beginStroke).toHaveBeenCalledWith({
      kind: 'erase',
      layer: 'element',
      point: { x: 20, y: 20 },
      diameter: 48,
    })
    expect(commit).toHaveBeenCalledWith(
      1,
      'element',
      'paint',
      expect.arrayContaining([{ x: 20, y: 20 }]),
      48,
    )
  })

  it('fills a contiguous color region on the selected image layer', async () => {
    installProject()
    useKoharuStore.setState({
      tool: 'fill',
      selectedLayers: ['element'],
      brush: { diameter: 48, color: '#336699' },
      fillTolerance: 24,
    })
    const commit = vi
      .spyOn(commands, 'commitFill')
      .mockResolvedValue({ revision: 2, layer: 'element' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 15, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 15, clientX: 30, clientY: 40 })

    await waitFor(() =>
      expect(commit).toHaveBeenCalledWith(1, 'element', { x: 20, y: 20 }, [51, 102, 153, 255], 24),
    )
  })

  it('redirects original artwork erasing to the editable artwork layer', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [originalArtworkLayer, artworkLayer, ...page.layers],
    }))
    useKoharuStore.setState({ tool: 'eraser', selectedLayers: ['page'] })
    const commit = vi
      .spyOn(commands, 'commitRasterErase')
      .mockResolvedValue({ revision: 2, layer: 'artwork' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 13, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 13, clientX: 30, clientY: 40 })

    await waitFor(() =>
      expect(commit).toHaveBeenCalledWith(1, 'artwork', 'paint', expect.any(Array), 48),
    )
    expect(canvas.beginStroke).toHaveBeenCalledWith({
      kind: 'erase',
      layer: 'artwork',
      point: { x: 20, y: 20 },
      diameter: 48,
    })
  })

  it('edits the reusable unconsumed removal mask', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [...page.layers, segmentationLayer],
    }))
    useKoharuStore.setState({ toolMode: 'removal', tool: 'brush' })
    const brushCommit = vi
      .spyOn(commands, 'commitRasterBrush')
      .mockResolvedValue({ revision: 2, layer: 'segmentation' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 12, clientX: 30, clientY: 40 })
    fireEvent.pointerMove(surface, { pointerId: 12, clientX: 40, clientY: 50 })
    fireEvent.pointerUp(surface, { pointerId: 12, clientX: 40, clientY: 50 })

    await waitFor(() => expect(brushCommit).toHaveBeenCalledOnce())
    expect(brushCommit).toHaveBeenCalledWith(
      1,
      'segmentation',
      'removal',
      expect.arrayContaining([
        { x: 20, y: 20 },
        { x: 30, y: 30 },
      ]),
      { diameter: 48, color: [168, 85, 247, 255] },
    )
  })

  it('starts a new removal mask instead of editing one already consumed by inpainting', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [...page.layers, { ...segmentationLayer, consumed: true }],
    }))
    useKoharuStore.setState({
      toolMode: 'removal',
      tool: 'brush',
      selectedLayers: ['segmentation'],
    })
    const commit = vi
      .spyOn(commands, 'commitRasterBrush')
      .mockResolvedValue({ revision: 2, layer: 'new-segmentation' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 27, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 27, clientX: 30, clientY: 40 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(commit).toHaveBeenCalledWith(1, null, 'removal', expect.any(Array), expect.any(Object))
  })

  it('edits a dedicated repair mask', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [...page.layers, repairLayer],
    }))
    useKoharuStore.setState({ toolMode: 'repair', tool: 'brush' })
    const commit = vi
      .spyOn(commands, 'commitRasterBrush')
      .mockResolvedValue({ revision: 2, layer: 'repair' })
    const surface = renderWorkspace()

    expect(screen.getByText('Repair mask ready')).toBeInTheDocument()
    fireEvent.pointerDown(surface, { button: 0, pointerId: 28, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 28, clientX: 30, clientY: 40 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(commit).toHaveBeenCalledWith(1, 'repair', 'repair', expect.any(Array), {
      diameter: 48,
      color: [14, 165, 233, 255],
    })
  })

  it('submits and resets a ready repair mask from the repair controls', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [...page.layers, repairLayer],
    }))
    useKoharuStore.setState({ toolMode: 'repair', tool: 'brush' })
    const repair = vi.spyOn(commands, 'repairInpaint').mockResolvedValue('job')
    const reset = vi.spyOn(commands, 'deleteLayers').mockResolvedValue(null)
    renderWorkspace()
    const controls = screen.getByText('Repair mask ready').parentElement!

    fireEvent.click(within(controls).getByRole('button', { name: 'Repair' }))
    await waitFor(() => expect(repair).toHaveBeenCalledWith(1, 'repair'))
    await waitFor(() =>
      expect(within(controls).getByRole('button', { name: 'Reset' })).toBeEnabled(),
    )
    fireEvent.click(within(controls).getByRole('button', { name: 'Reset' }))
    await waitFor(() => expect(reset).toHaveBeenCalledWith(['repair']))
  })

  it('rasterizes a pending repair shape before submitting it', async () => {
    installProject()
    useKoharuStore.setState({ toolMode: 'repair', tool: 'rectangle' })
    const shape = vi.spyOn(commands, 'commitRasterShape').mockResolvedValue({
      revision: 2,
      layer: 'repair',
    })
    const repair = vi.spyOn(commands, 'repairInpaint').mockResolvedValue('job')
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 29, clientX: 30, clientY: 40 })
    fireEvent.pointerMove(surface, { pointerId: 29, clientX: 70, clientY: 60 })
    fireEvent.pointerUp(surface, { pointerId: 29, clientX: 70, clientY: 60 })

    const controls = screen.getByText('Repair mask ready').parentElement!
    fireEvent.click(within(controls).getByRole('button', { name: 'Repair' }))
    await waitFor(() => expect(shape).toHaveBeenCalledOnce())
    await waitFor(() => expect(repair).toHaveBeenCalledWith(2, 'repair'))
  })

  it('keeps shapes editable for dragging, resizing, and rotation until click-away', async () => {
    installProject()
    useKoharuStore.setState({ toolMode: 'removal', tool: 'rectangle' })
    const commit = vi.spyOn(commands, 'commitRasterShape').mockResolvedValue({
      revision: 2,
      layer: 'segmentation',
    })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 18, clientX: 30, clientY: 40 })
    fireEvent.pointerMove(surface, { pointerId: 18, clientX: 70, clientY: 60 })
    fireEvent.pointerUp(surface, { pointerId: 18, clientX: 70, clientY: 60 })

    expect(commit).not.toHaveBeenCalled()
    expect(screen.getByTestId('raster-shape-overlay')).toBeInTheDocument()
    expect(document.querySelector('[data-transform-body]')).toBeInTheDocument()
    expect(document.querySelectorAll('[data-resize-handle]')).toHaveLength(8)
    expect(document.querySelector('[data-rotate-handle]')).toBeInTheDocument()

    const body = document.querySelector('[data-transform-body]')!
    fireEvent.pointerDown(body, { button: 0, pointerId: 19, clientX: 40, clientY: 30 })
    fireEvent.pointerMove(body, { pointerId: 19, clientX: 50, clientY: 45 })
    fireEvent.pointerUp(body, { pointerId: 19, clientX: 50, clientY: 45 })

    const resize = document.querySelector('[data-resize-handle="se"]')!
    fireEvent.pointerDown(resize, { button: 0, pointerId: 20, clientX: 70, clientY: 55 })
    fireEvent.pointerMove(resize, { pointerId: 20, clientX: 90, clientY: 75 })
    fireEvent.pointerUp(resize, { pointerId: 20, clientX: 90, clientY: 75 })

    const rotate = document.querySelector('[data-rotate-handle]')!
    fireEvent.pointerDown(rotate, { button: 0, pointerId: 21, clientX: 60, clientY: 15 })
    fireEvent.pointerMove(rotate, {
      pointerId: 21,
      clientX: 100,
      clientY: 55,
      ctrlKey: true,
    })
    fireEvent.pointerUp(rotate, {
      pointerId: 21,
      clientX: 100,
      clientY: 55,
      ctrlKey: true,
    })

    fireEvent.pointerDown(document.body, { button: 0, pointerId: 22 })
    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(commit).toHaveBeenCalledWith(
      1,
      null,
      'removal',
      {
        type: 'rectangle',
        frame: { x: 30, y: 35, width: 60, height: 40, angle_degrees: 90 },
      },
      [168, 85, 247, 255],
    )
    expect(screen.queryByTestId('raster-shape-overlay')).not.toBeInTheDocument()
  })

  it('constrains an oval to a circle while Ctrl or Shift is held', async () => {
    installProject()
    useKoharuStore.setState({ toolMode: 'removal', tool: 'oval' })
    const commit = vi.spyOn(commands, 'commitRasterShape').mockResolvedValue({
      revision: 2,
      layer: 'segmentation',
    })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 23, clientX: 30, clientY: 40 })
    fireEvent.pointerMove(surface, {
      pointerId: 23,
      clientX: 70,
      clientY: 60,
      shiftKey: true,
    })
    fireEvent.pointerUp(surface, {
      pointerId: 23,
      clientX: 70,
      clientY: 60,
      shiftKey: true,
    })
    fireEvent.pointerDown(document.body, { button: 0, pointerId: 24 })

    await waitFor(() =>
      expect(commit).toHaveBeenCalledWith(
        1,
        null,
        'removal',
        {
          type: 'oval',
          frame: { x: 20, y: 20, width: 40, height: 40, angle_degrees: 0 },
        },
        [168, 85, 247, 255],
      ),
    )
  })

  it('rasterizes decensor shapes into a mosaic censor layer on click-away', async () => {
    installProject()
    useKoharuStore.setState({ toolMode: 'decensor', tool: 'rectangle' })
    const commit = vi.spyOn(commands, 'commitRasterShape').mockResolvedValue({
      revision: 2,
      layer: 'mosaic',
    })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 25, clientX: 30, clientY: 40 })
    fireEvent.pointerMove(surface, { pointerId: 25, clientX: 70, clientY: 60 })
    fireEvent.pointerUp(surface, { pointerId: 25, clientX: 70, clientY: 60 })

    expect(commit).not.toHaveBeenCalled()
    expect(screen.getByTestId('raster-shape-overlay')).toBeInTheDocument()
    fireEvent.pointerDown(document.body, { button: 0, pointerId: 26 })
    await waitFor(() =>
      expect(commit).toHaveBeenCalledWith(
        1,
        null,
        'mosaic_censor',
        {
          type: 'rectangle',
          frame: { x: 20, y: 20, width: 40, height: 20, angle_degrees: 0 },
        },
        [0, 255, 0, 180],
      ),
    )
    expect(canvas.beginStroke).not.toHaveBeenCalled()
  })

  it('creates the same OCR-candidate text box on click and drag', async () => {
    installProject()
    useKoharuStore.setState({ tool: 'text' })
    const box = vi.spyOn(commands, 'addTextBox').mockResolvedValue({
      revision: 3,
      layer: 'text-box',
    })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 13, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 13, clientX: 30, clientY: 40 })
    await waitFor(() =>
      expect(box).toHaveBeenCalledWith({
        x: 20,
        y: 20,
        width: 320,
        height: 120,
        angle_degrees: 0,
      }),
    )

    fireEvent.pointerDown(surface, { button: 0, pointerId: 14, clientX: 40, clientY: 50 })
    fireEvent.pointerMove(surface, { pointerId: 14, clientX: 140, clientY: 110 })
    fireEvent.pointerUp(surface, { pointerId: 14, clientX: 140, clientY: 110 })
    await waitFor(() => expect(box).toHaveBeenCalledTimes(2))
    expect(box).toHaveBeenLastCalledWith({
      x: 30,
      y: 30,
      width: 100,
      height: 60,
      angle_degrees: 0,
    })
  })
})
