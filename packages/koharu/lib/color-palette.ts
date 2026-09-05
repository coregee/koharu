'use client'

import { create } from 'zustand'
import { createJSONStorage, persist } from 'zustand/middleware'

interface ColorPalette {
  colors: string[]
  save: (color: string) => void
  remove: (color: string) => void
}

export const useColorPalette = create<ColorPalette>()(
  persist(
    (set) => ({
      colors: [],
      save: (color) => {
        if (!/^#[0-9a-f]{6}$/i.test(color)) return
        const normalized = color.toUpperCase()
        set((state) => ({ colors: [...new Set([...state.colors, normalized])] }))
      },
      remove: (color) =>
        set((state) => ({ colors: state.colors.filter((value) => value !== color) })),
    }),
    {
      name: 'koharu-color-palette',
      storage: createJSONStorage(() => localStorage),
      partialize: ({ colors }) => ({ colors }),
      merge: (persisted, current) => {
        const colors = (persisted as { colors?: unknown } | undefined)?.colors
        return {
          ...current,
          colors: Array.isArray(colors)
            ? [
                ...new Set(
                  colors
                    .filter(
                      (color): color is string =>
                        typeof color === 'string' && /^#[0-9a-f]{6}$/i.test(color),
                    )
                    .map((color) => color.toUpperCase()),
                ),
              ]
            : [],
        }
      },
    },
  ),
)
