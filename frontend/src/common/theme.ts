import { createContext, useContext } from 'react';

export type Theme = 'dark' | 'light';

/** The current theme, provided by App so that the charts (which draw their own text and grid) can follow it. */
export const ThemeContext = createContext<Theme>('light');

export const useTheme = () => useContext(ThemeContext);
