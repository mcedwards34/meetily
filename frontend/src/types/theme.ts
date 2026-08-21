/**
 * Theme Preference
 *
 * Persisted light/dark mode preference, following the same localStorage
 * load/save pattern as betaFeatures.ts.
 */

export type Theme = 'light' | 'dark';

export const DEFAULT_THEME: Theme = 'light';

const STORAGE_KEY = 'theme';

/**
 * Load the theme preference from localStorage.
 */
export function loadTheme(): Theme {
  if (typeof window === 'undefined') {
    return DEFAULT_THEME;
  }

  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === 'light' || saved === 'dark') {
      return saved;
    }
  } catch (error) {
    console.error('[Theme] Failed to load from localStorage:', error);
  }

  return DEFAULT_THEME;
}

/**
 * Save the theme preference to localStorage.
 */
export function saveTheme(theme: Theme): void {
  if (typeof window === 'undefined') return;

  try {
    localStorage.setItem(STORAGE_KEY, theme);
  } catch (error) {
    console.error('[Theme] Failed to save to localStorage:', error);
  }
}

/**
 * Apply the theme by toggling the `dark` class on the root element.
 */
export function applyTheme(theme: Theme): void {
  if (typeof document === 'undefined') return;
  document.documentElement.classList.toggle('dark', theme === 'dark');
}

/**
 * Inline script executed synchronously before the React bundle mounts, so
 * the correct theme class is applied before first paint (no flash of the
 * wrong theme). Must stay dependency-free plain JS.
 */
export const THEME_BOOTSTRAP_SCRIPT = `
(function() {
  try {
    var theme = localStorage.getItem('${STORAGE_KEY}');
    if (theme === 'dark') {
      document.documentElement.classList.add('dark');
    }
  } catch (e) {}
})();
`;
