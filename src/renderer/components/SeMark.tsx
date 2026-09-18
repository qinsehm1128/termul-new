/**
 * Se brand mark.
 *
 * Uses the packaged app icon so the rail, chat empty state, and launcher match
 * the desktop bundle. `currentColor` classes on callers are ignored — the PNG
 * carries its own gold artwork.
 */
export function SeMark({
  size = 22,
  className
}: {
  size?: number
  className?: string
}): React.JSX.Element {
  return (
    <img
      src="/se-mark.png"
      width={size}
      height={size}
      alt="Se"
      className={className}
      draggable={false}
    />
  )
}
