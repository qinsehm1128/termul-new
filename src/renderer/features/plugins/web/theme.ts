import { type ThemeSnapshot, WEB_THEME_VARIABLES } from '@shared/types/plugin-web.types'

/** Only public design tokens cross into the opaque-origin frame. */
export function collectTheme(revision = 0): ThemeSnapshot {
  const root = document.documentElement
  const style = getComputedStyle(root)
  const variables: ThemeSnapshot['variables'] = {}
  for (const name of WEB_THEME_VARIABLES) {
    const value = style.getPropertyValue(name).trim()
    if (value && value.length <= 256) variables[name] = value
  }
  if (!Object.keys(variables).length)
    variables['--background'] = style.backgroundColor || 'transparent'
  return {
    revision,
    appearance:
      root.classList.contains('dark') || root.getAttribute('data-theme') === 'dark'
        ? 'dark'
        : 'light',
    variables,
    fontFamily: (style.fontFamily || 'sans-serif').slice(0, 256),
    fontSize: (style.fontSize || '14px').slice(0, 32)
  }
}

export function observeTheme(handler: (theme: ThemeSnapshot) => void): () => void {
  let revision = 0
  const observer = new MutationObserver(() => handler(collectTheme(++revision)))
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['class', 'style', 'data-theme']
  })
  return () => observer.disconnect()
}
