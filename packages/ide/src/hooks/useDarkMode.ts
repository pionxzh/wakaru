import { atom, useAtom, useAtomValue } from 'jotai'
import { atomWithStorage } from 'jotai/utils'
import { atomEffect } from 'jotai-effect'

type Theme = 'light' | 'dark' | 'system'

const themeAtom = atomWithStorage<Theme>('theme', 'system')

const prefersDarkQuery = matchMedia('(prefers-color-scheme: dark)')
const prefersDarkAtom = atom(prefersDarkQuery.matches)
const syncPrefersDarkEffect = atomEffect((_get, set) => {
    const update = () => set(prefersDarkAtom, prefersDarkQuery.matches)
    prefersDarkQuery.addEventListener('change', update)
    return () => prefersDarkQuery.removeEventListener('change', update)
})

export function useDarkMode() {
    const prefersDark = useAtomValue(prefersDarkAtom)
    useAtom(syncPrefersDarkEffect)

    const [theme, setTheme] = useAtom(themeAtom)
    const toggleTheme = () => {
        if (theme === 'system') {
            setTheme(prefersDark ? 'light' : 'dark')
        }
        else {
            setTheme(theme === 'dark' ? 'light' : 'dark')
        }
    }

    const isDarkMode = theme === 'dark' || (theme === 'system' && prefersDark)

    return {
        theme,
        setTheme,
        toggleTheme,
        isDarkMode,
    }
}
