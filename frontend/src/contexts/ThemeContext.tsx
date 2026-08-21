'use client'

import { createContext, useContext, useEffect, useState, ReactNode } from 'react'
import { Theme, DEFAULT_THEME, loadTheme, saveTheme, applyTheme } from '@/types/theme'

interface ThemeContextValue {
  theme: Theme;
  setTheme: (theme: Theme) => void;
}

const ThemeContext = createContext<ThemeContextValue | undefined>(undefined);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(DEFAULT_THEME);

  useEffect(() => {
    setThemeState(loadTheme());
  }, []);

  const setTheme = (next: Theme) => {
    setThemeState(next);
    saveTheme(next);
    applyTheme(next);
  };

  return (
    <ThemeContext.Provider value={{ theme, setTheme }}>
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme(): ThemeContextValue {
  const context = useContext(ThemeContext);
  if (!context) {
    throw new Error('useTheme must be used within a ThemeProvider');
  }
  return context;
}
