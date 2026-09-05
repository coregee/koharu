import { act, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, expect, it, vi } from 'vitest'

import { ColorWell } from '@/components/controls/ColorWell'
import { useColorPalette } from '@/lib/color-palette'

beforeEach(() => {
  localStorage.clear()
  useColorPalette.setState({ colors: [] })
})

it('saves a color, restores it from storage, and applies it in another color well', async () => {
  const user = userEvent.setup()
  const onChange = vi.fn()
  const first = render(<ColorWell label='First color' value='#aBc' onChange={vi.fn()} />)
  await user.click(screen.getByRole('button', { name: 'First color' }))
  await user.click(screen.getByRole('button', { name: 'Save current color' }))
  expect(screen.getByRole('button', { name: 'Save current color' })).toBeDisabled()
  const persisted = localStorage.getItem('koharu-color-palette')!
  expect(JSON.parse(persisted).state.colors).toEqual(['#AABBCC'])
  first.unmount()

  useColorPalette.setState({ colors: [] })
  localStorage.setItem('koharu-color-palette', persisted)
  await act(async () => {
    await useColorPalette.persist.rehydrate()
  })
  render(<ColorWell label='Second color' value='#FFFFFF' onChange={onChange} />)
  await user.click(screen.getByRole('button', { name: 'Second color' }))
  await user.click(screen.getByRole('button', { name: 'Apply #AABBCC' }))
  expect(onChange).toHaveBeenCalledWith('#AABBCC')
  await user.click(screen.getByRole('button', { name: 'Remove #AABBCC' }))
  expect(screen.queryByRole('button', { name: 'Apply #AABBCC' })).not.toBeInTheDocument()
  expect(JSON.parse(localStorage.getItem('koharu-color-palette')!).state.colors).toEqual([])
})

it('validates and deduplicates stored colors without replacing store actions', async () => {
  localStorage.setItem(
    'koharu-color-palette',
    JSON.stringify({
      state: { colors: ['#abcdef', '#ABCDEF', 'invalid', null, 42] },
      version: 0,
    }),
  )
  await useColorPalette.persist.rehydrate()
  expect(useColorPalette.getState().colors).toEqual(['#ABCDEF'])
  useColorPalette.getState().save('#abcdef')
  useColorPalette.getState().save('invalid')
  expect(useColorPalette.getState().colors).toEqual(['#ABCDEF'])
})
